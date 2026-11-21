//! Assemble selected PDF pages with explicit page and navigation identity.
use pdf_oxide::{
    object::{Object, ObjectRef},
    writer::ObjectSerializer,
    PdfDocument,
};
use std::{
    collections::{HashMap, HashSet},
    io::Write,
    sync::OnceLock,
};

mod deletion;
mod forms;
mod preservation;
mod tagging;
pub(in crate::matters) use preservation::verify_edit_context;

pub(in crate::matters) type Dictionary = HashMap<String, Object>;
type Result<T> = std::result::Result<T, &'static str>;
const INVALID: &str = "Unable to assemble working PDF";
const COMPLEX: &str = "PDF assembly exceeds the object or page limit";
const MAX_PAGES: usize = 100_000;
const MAX_OBJECTS: usize = 500_000;
const MAX_DEPTH: usize = 128;
const MAX_OUTPUT: usize = 160 * 1024 * 1024;
const INHERITED_PAGE_KEYS: [&str; 4] = ["Resources", "MediaBox", "CropBox", "Rotate"];

fn name(value: &str) -> Object {
    Object::Name(value.into())
}

fn dict(fields: impl IntoIterator<Item = (&'static str, Object)>) -> Dictionary {
    fields
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

pub(in crate::matters) fn pdf_text(text: &str) -> Object {
    let mut bytes = vec![0xfe, 0xff];
    bytes.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
    Object::String(bytes)
}

fn outline_children(fields: &mut Dictionary, children: &[u32]) {
    if let (Some(first), Some(last)) = (children.first(), children.last()) {
        fields.insert("First".into(), reference(*first));
        fields.insert("Last".into(), reference(*last));
        fields.insert("Count".into(), Object::Integer(-(children.len() as i64)));
    }
}

fn reference(id: u32) -> Object {
    Object::Reference(ObjectRef::new(id, 0))
}

pub(in crate::matters) struct Source {
    pdf: PdfDocument,
    catalog: Object,
    pages: Vec<ObjectRef>,
    indexes: HashMap<ObjectRef, usize>,
    branches: HashSet<ObjectRef>,
    destinations: OnceLock<Result<HashMap<String, Object>>>,
    form: OnceLock<Result<Option<Dictionary>>>,
}

fn page_tree(pdf: &PdfDocument, catalog: &Object) -> Result<(Vec<ObjectRef>, HashSet<ObjectRef>)> {
    let root = catalog
        .as_dict()
        .and_then(|catalog| catalog.get("Pages"))
        .and_then(Object::as_reference)
        .ok_or(INVALID)?;
    let mut pages = Vec::new();
    let mut visited = HashSet::new();
    let mut branches = HashSet::new();
    let mut stack = vec![(root, 0)];
    while let Some((item, depth)) = stack.pop() {
        if depth > MAX_DEPTH || visited.len() >= MAX_OBJECTS || pages.len() >= MAX_PAGES {
            return Err(COMPLEX);
        }
        if !visited.insert(item) {
            return Err(INVALID);
        }
        let object = pdf.load_object(item).map_err(|_| INVALID)?;
        let fields = object.as_dict().ok_or(INVALID)?;
        match fields.get("Type").and_then(Object::as_name) {
            Some("Page") => pages.push(item),
            Some("Pages") => {
                branches.insert(item);
                let kids = fields.get("Kids").ok_or(INVALID)?;
                let kids = pdf.resolve_object(kids).map_err(|_| INVALID)?;
                for kid in kids.as_array().ok_or(INVALID)?.iter().rev() {
                    stack.push((kid.as_reference().ok_or(INVALID)?, depth + 1));
                }
            }
            _ => return Err(INVALID),
        }
    }
    if pages.len() != pdf.page_count().map_err(|_| INVALID)? {
        return Err(INVALID);
    }
    Ok((pages, branches))
}

impl Source {
    pub(in crate::matters) fn open(bytes: Vec<u8>) -> Result<Self> {
        let pdf = PdfDocument::from_bytes(bytes).map_err(|_| INVALID)?;
        Self::from_document(pdf)
    }

    fn from_document(pdf: PdfDocument) -> Result<Self> {
        let catalog = pdf.catalog().map_err(|_| INVALID)?;
        let (pages, branches) = page_tree(&pdf, &catalog)?;
        let indexes = pages
            .iter()
            .enumerate()
            .map(|(index, &page)| (page, index))
            .collect();
        Ok(Self {
            pdf,
            catalog,
            pages,
            indexes,
            branches,
            destinations: OnceLock::new(),
            form: OnceLock::new(),
        })
    }

    pub(in crate::matters) fn geometry(&self, index: usize) -> Result<Geometry> {
        geometry(&self.pdf, index)
    }

    pub(in crate::matters) fn document(&self) -> &PdfDocument {
        &self.pdf
    }

    fn resolve(&self, value: &Object) -> Result<Object> {
        self.pdf.resolve_object(value).map_err(|_| INVALID)
    }

    fn destination(&self, value: &Object, depth: usize) -> Result<Option<(usize, Vec<Object>)>> {
        if depth > MAX_DEPTH {
            return Err(COMPLEX);
        }
        match self.resolve(value)? {
            Object::Array(mut parts) if parts.len() >= 2 => {
                let page = parts.remove(0);
                let target = match page {
                    Object::Reference(page) => self.indexes.get(&page).copied(),
                    Object::Integer(index) => usize::try_from(index)
                        .ok()
                        .filter(|&index| index < self.pages.len()),
                    _ => None,
                };
                Ok(target.map(|target| (target, parts)))
            }
            Object::Dictionary(fields) => match fields.get("D") {
                Some(value) => self.destination(value, depth + 1),
                None => Ok(None),
            },
            Object::Name(value) => self.named_destination(&value, depth + 1),
            Object::String(value) => self.named_destination(
                &pdf_oxide::optional_content::decode_pdf_text_string(&value),
                depth + 1,
            ),
            _ => Ok(None),
        }
    }

    fn named_destination(
        &self,
        name_value: &str,
        depth: usize,
    ) -> Result<Option<(usize, Vec<Object>)>> {
        let destinations = self.destinations.get_or_init(|| self.read_destinations());
        match destinations {
            Ok(destinations) => destinations
                .get(name_value)
                .map_or(Ok(None), |value| self.destination(value, depth)),
            Err(error) => Err(*error),
        }
    }

    fn read_destinations(&self) -> Result<HashMap<String, Object>> {
        let catalog = self.catalog.as_dict().ok_or(INVALID)?;
        let mut destinations = HashMap::new();
        if let Some(dictionary) = catalog.get("Dests") {
            destinations.extend(self.resolve(dictionary)?.as_dict().ok_or(INVALID)?.clone());
        }
        let Some(names) = catalog.get("Names") else {
            return Ok(destinations);
        };
        let names = self.resolve(names)?;
        let Some(tree) = names.as_dict().ok_or(INVALID)?.get("Dests") else {
            return Ok(destinations);
        };
        let mut pending = vec![(tree.clone(), 0)];
        let mut seen = HashSet::new();
        let mut visited = 0;
        while let Some((node, depth)) = pending.pop() {
            visited += 1;
            if depth > MAX_DEPTH || visited > MAX_OBJECTS || destinations.len() > MAX_OBJECTS {
                return Err(COMPLEX);
            }
            if node.as_reference().is_some_and(|id| !seen.insert(id)) {
                return Err(INVALID);
            }
            let node = self.resolve(&node)?;
            let fields = node.as_dict().ok_or(INVALID)?;
            if let Some(entries) = fields.get("Names") {
                let entries = self.resolve(entries)?;
                let entries = entries.as_array().ok_or(INVALID)?;
                if entries.len() % 2 != 0 || entries.len() / 2 > MAX_OBJECTS - destinations.len() {
                    return Err(INVALID);
                }
                for pair in entries.chunks_exact(2) {
                    let key = self.resolve(&pair[0])?;
                    let key = key.as_string().ok_or(INVALID)?;
                    let name = pdf_oxide::optional_content::decode_pdf_text_string(key);
                    destinations.insert(name, pair[1].clone());
                }
            }
            if let Some(kids) = fields.get("Kids") {
                let kids = self.resolve(kids)?;
                let kids = kids.as_array().ok_or(INVALID)?;
                if kids.len() > MAX_OBJECTS.saturating_sub(visited + pending.len()) {
                    return Err(COMPLEX);
                }
                pending.extend(kids.iter().rev().cloned().map(|kid| (kid, depth + 1)));
            }
        }
        Ok(destinations)
    }
}

pub(in crate::matters) struct UnicodeLine {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub bold: bool,
    pub ink: (f32, f32, f32),
}

#[derive(Default)]
pub(in crate::matters) struct Overlay {
    pub commands: String,
    pub fonts: Vec<&'static str>,
    pub textbox_fonts: bool,
    pub images: Vec<OverlayImage>,
    pub glyph_fonts: Vec<Vec<char>>,
    pub shaped_fonts: Vec<super::super::shaped_text::Font>,
    pub ocr: bool,
    pub user_space_text: bool,
    pub paint_before_source: bool,
    pub unrotated_page_coordinates: bool,
    pub unicode_lines: Vec<UnicodeLine>,
    pub xobject_edits: Vec<XObjectEdit>,
    pub replacement_tag: Option<ReplacementTag>,
    pub shared_actual_text: Option<SharedActualTextEdit>,
    pub retain_xobjects: Option<HashSet<String>>,
    pub retain_patterns: Option<HashSet<String>>,
    pub retain_shadings: Option<HashSet<String>>,
    pub retain_properties: Option<HashSet<String>>,
    pub replace_properties: Option<Dictionary>,
    pub clear_soft_masks: HashSet<String>,
    pub reset_raster_alpha: Option<String>,
    pub redact_content: bool,
    pub omit_annotations: Vec<usize>,
    pub strip_page_metadata: bool,
    pub replace_page_content: Option<Vec<u8>>,
    pub page_number: Option<Box<Overlay>>,
    pub remove_page_number: bool,
    pub output_marks: Option<Box<Overlay>>,
    pub opacity: Vec<f64>,
}

pub(in crate::matters) fn textbox_font_alias(font: &str) -> Option<&'static str> {
    match font {
        "Helvetica" => Some("helv"),
        "Helvetica-Bold" => Some("hebo"),
        "Times-Roman" => Some("tiro"),
        "Times-Bold" => Some("tibo"),
        "Courier" => Some("cour"),
        "Courier-Bold" => Some("cobo"),
        _ => None,
    }
}

fn matches_textbox_font(fields: &Dictionary, font: &str) -> bool {
    fields.get("BaseFont").and_then(Object::as_name) == Some(font)
        && fields.get("Subtype").and_then(Object::as_name) == Some("Type1")
}

pub(in crate::matters) fn has_textbox_font(pdf: &PdfDocument, page: usize, font: &str) -> bool {
    (|| {
        let alias = textbox_font_alias(font)?;
        let resources = pdf.get_page_resources(page).ok()?;
        let fonts = pdf.resolve_object(resources.as_dict()?.get("Font")?).ok()?;
        let object = pdf.resolve_object(fonts.as_dict()?.get(alias)?).ok()?;
        Some(matches_textbox_font(object.as_dict()?, font))
    })()
    .unwrap_or(false)
}

pub(in crate::matters) struct OverlayImage {
    pub samples: Object,
    pub mask: Option<Object>,
    pub profile: Option<Object>,
}

const OCR_STREAM: &str = "MatterDraftOCR";
pub(in crate::matters) struct XObjectEdit {
    pub source: Option<String>,
    pub target: String,
    pub replacement: XObjectReplacement,
}

pub(in crate::matters) enum XObjectReplacement {
    Image(Object, Option<Object>),
    Form {
        body: Vec<u8>,
        fields: Dictionary,
        inverse: [f32; 6],
    },
    EditedForm(EditedForm),
    EmptyForm,
}

pub(in crate::matters) struct EditedForm {
    pub original: ObjectRef,
    pub fields: Dictionary,
    pub data: Vec<u8>,
    pub children: Vec<(String, EditedForm)>,
}

pub(in crate::matters) struct ReboundTextStream {
    pub data: Vec<u8>,
    pub resources: Object,
    pub forms: Vec<(String, usize)>,
}

pub(in crate::matters) struct ReboundTextForm {
    pub original: ObjectRef,
    pub fields: Dictionary,
    pub stream: ReboundTextStream,
}

#[derive(Clone, Copy)]
pub(in crate::matters) enum RetainedAnchor {
    InsertBefore,
    InsertAfter,
    InsertBetween { next_source_mcid: usize },
}

#[derive(Clone, Copy)]
pub(in crate::matters) struct ReplacementTag {
    pub parent_key: i64,
    pub mcid: Option<usize>,
    pub from_page: bool,
    pub remove_actual_text: bool,
    pub retained_anchor: Option<RetainedAnchor>,
}

pub(in crate::matters) struct SharedActualTextEdit {
    pub original: String,
    pub replacement: String,
    pub anchors: usize,
}

const NUMBER_STREAM: &str = "MatterDraftPageLabel";

#[derive(Default)]
pub(in crate::matters) struct Navigation {
    pub labels: Vec<String>,
    pub title: Option<String>,
    pub cleanup_metadata: bool,
    pub outlines: Vec<(usize, String, usize)>,
    pub links: Vec<(usize, [f64; 4], LinkTarget)>,
}

pub(in crate::matters) enum LinkTarget {
    Page(usize),
    Uri(String),
}

#[derive(Clone, Copy)]
pub(in crate::matters) struct Paper {
    pub width: f64,
    pub height: f64,
    pub fit: bool,
}

#[derive(Clone, Copy)]
pub(in crate::matters) struct Geometry {
    pub crop: [f64; 4],
    pub media: [f64; 4],
    pub rotation: i32,
}

impl Geometry {
    pub fn clipped(self) -> Option<Self> {
        let [left, bottom, right, top] = self.crop;
        let [media_left, media_bottom, media_right, media_top] = self.media;
        let crop = [
            left.max(media_left),
            bottom.max(media_bottom),
            right.min(media_right),
            top.min(media_top),
        ];
        (crop[0] < crop[2] && crop[1] < crop[3]).then_some(Self { crop, ..self })
    }

    pub fn display_size(self) -> (f64, f64) {
        let [left, bottom, right, top] = self.crop;
        if self.rotation % 180 == 0 {
            (right - left, top - bottom)
        } else {
            (top - bottom, right - left)
        }
    }

    // From displayed bottom-left points to PDF user space. Positive rotation
    // in the page dictionary describes a clockwise viewer rotation.
    pub fn display_matrix(self) -> [f64; 6] {
        let [left, bottom, right, top] = self.crop;
        match self.rotation {
            90 => [0.0, 1.0, -1.0, 0.0, right, bottom],
            180 => [-1.0, 0.0, 0.0, -1.0, right, top],
            270 => [0.0, -1.0, 1.0, 0.0, left, top],
            _ => [1.0, 0.0, 0.0, 1.0, left, bottom],
        }
    }

    pub fn user_point(self, x: f64, y: f64) -> (f64, f64) {
        let [a, b, c, d, e, f] = self.display_matrix();
        (a * x + c * y + e, b * x + d * y + f)
    }
}

#[derive(Clone, Copy)]
pub(in crate::matters) struct DisplayFrame {
    pub geometry: Geometry,
    pub visible: Geometry,
    pub size: (f64, f64),
    pub unit: f64,
}

impl DisplayFrame {
    pub fn new(pdf: &PdfDocument, page: usize) -> Result<Self> {
        let geometry = geometry(pdf, page)?;
        let visible = geometry.clipped().unwrap_or(Geometry {
            crop: geometry.media,
            ..geometry
        });
        let unit = page_user_unit(pdf, page)?;
        let (width, height) = visible.display_size();
        let size = (width * unit, height * unit);
        if !size.0.is_finite() || !size.1.is_finite() {
            return Err(INVALID);
        }
        Ok(Self {
            geometry,
            visible,
            size,
            unit,
        })
    }

    // Drawing operations derotate the physical box and then use the page's
    // public transformation matrix. Rotated pages use a zero-based CropBox.
    pub fn shape_matrix(self) -> [f64; 6] {
        let [left, bottom, right, top] = self.geometry.crop;
        let (width, height) = (right - left, top - bottom);
        match self.geometry.rotation {
            90 => [0.0, 1.0, -1.0, 0.0, self.size.1, 0.0],
            180 => [-1.0, 0.0, 0.0, -1.0, width, self.size.1],
            270 => [0.0, -1.0, 1.0, 0.0, width - self.size.1, height],
            _ => [
                1.0 / self.unit,
                0.0,
                0.0,
                1.0 / self.unit,
                self.visible.crop[0],
                self.visible.crop[1],
            ],
        }
    }

    // Text boxes use the stored crop origin and keep the font in user units.
    pub fn text_matrix(self, rotate: bool) -> [f64; 6] {
        let [left, bottom, right, top] = self.geometry.crop;
        let height = self.size.1;
        match if rotate { self.geometry.rotation } else { 0 } {
            90 => [0.0, 1.0, -1.0, 0.0, left + height, bottom],
            180 => [-1.0, 0.0, 0.0, -1.0, right, bottom + height],
            270 => [0.0, -1.0, 1.0, 0.0, right - height, top],
            _ => [1.0, 0.0, 0.0, 1.0, left, top - height],
        }
    }

    // Paint inspection and annotation rects use the effective crop origin
    // and the physical size, including on rotated pages.
    pub fn paint_matrix(self) -> [f64; 6] {
        let [left, bottom, right, top] = self.geometry.crop;
        let (width, height) = (right - left, top - bottom);
        let [x, _, _, y] = self.visible.crop;
        let u = 1.0 / self.unit;
        let h = self.size.1 * u;
        match self.geometry.rotation {
            90 => [0.0, u, -u, 0.0, x + h, y - height * u],
            180 => [-u, 0.0, 0.0, -u, x + width * u, y + h - height * u],
            270 => [0.0, -u, u, 0.0, x + width * u - h, y],
            _ => [u, 0.0, 0.0, u, x, self.visible.crop[1]],
        }
    }

    pub fn visible_matrix(self) -> [f64; 6] {
        let [a, b, c, d, e, f] = self.visible.display_matrix();
        [
            a / self.unit,
            b / self.unit,
            c / self.unit,
            d / self.unit,
            e,
            f,
        ]
    }
}

pub(in crate::matters) fn transform_box(
    [a, b, c, d, e, f]: [f64; 6],
    [left, bottom, right, top]: [f64; 4],
) -> [f64; 4] {
    // Each caller's frame is scale/translation with a quarter-turn.
    let (x0, y0) = (a * left + c * bottom + e, b * left + d * bottom + f);
    let (x1, y1) = (a * right + c * top + e, b * right + d * top + f);
    [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)]
}

pub(in crate::matters) fn geometry(pdf: &PdfDocument, index: usize) -> Result<Geometry> {
    let fields = effective_page(pdf, index)?;
    let bounds = |object: &Object| -> Result<[f64; 4]> {
        let object = pdf.resolve_object(object).map_err(|_| INVALID)?;
        let mut bounds = [0.0; 4];
        let values = object
            .as_array()
            .filter(|array| array.len() == 4)
            .ok_or(INVALID)?;
        for (slot, value) in bounds.iter_mut().zip(values) {
            *slot = match pdf.resolve_object(value).map_err(|_| INVALID)? {
                Object::Integer(value) => value as f64,
                Object::Real(value) if value.is_finite() => value,
                _ => return Err(INVALID),
            };
        }
        let [a, b, c, d] = bounds;
        if a == c || b == d {
            return Err(INVALID);
        }
        Ok([a.min(c), b.min(d), a.max(c), b.max(d)])
    };
    let media = bounds(fields.get("MediaBox").ok_or(INVALID)?)?;
    let crop = fields
        .get("CropBox")
        .and_then(|crop| bounds(crop).ok())
        .unwrap_or(media);
    let rotation = fields
        .get("Rotate")
        .and_then(|value| pdf.resolve_object(value).ok())
        .and_then(|value| match value {
            Object::Integer(value) => Some((value as i32).rem_euclid(360)),
            Object::Real(value) if value.is_finite() => Some((value as i32).rem_euclid(360)),
            _ => None,
        })
        .filter(|degrees| degrees % 90 == 0)
        .unwrap_or(0);
    Ok(Geometry {
        crop,
        media,
        rotation,
    })
}

fn effective_page(pdf: &PdfDocument, index: usize) -> Result<Dictionary> {
    let Object::Dictionary(mut fields) = pdf.get_page(index).map_err(|_| INVALID)? else {
        return Err(INVALID);
    };
    let null = |field: &Object| {
        matches!(field, Object::Null)
            || (field.as_reference().is_some()
                && pdf
                    .resolve_object(field)
                    .is_ok_and(|value| matches!(value, Object::Null)))
    };
    // A null dictionary value is treated as absent. The page reader may
    // already have merged the closest ancestor's null into this dictionary.
    let mut inherit = INHERITED_PAGE_KEYS
        .into_iter()
        .filter(|key| fields.get(*key).is_some_and(null))
        .collect::<Vec<_>>();
    for key in &inherit {
        fields.remove(*key);
    }
    let mut parent = fields.get("Parent").and_then(Object::as_reference);
    let mut depth = 0;
    while !inherit.is_empty() {
        let Some(id) = parent else { break };
        if depth >= MAX_DEPTH {
            return Err(COMPLEX);
        }
        let object = pdf.load_object(id).map_err(|_| INVALID)?;
        let attributes = object.as_dict().ok_or(INVALID)?;
        inherit.retain(|key| {
            if let Some(value) = attributes.get(*key).filter(|value| !null(value)) {
                fields.insert((*key).into(), value.clone());
                false
            } else {
                true
            }
        });
        parent = attributes.get("Parent").and_then(Object::as_reference);
        depth += 1;
    }
    Ok(fields)
}

pub(in crate::matters) fn page_user_unit(pdf: &PdfDocument, index: usize) -> Result<f64> {
    let page = pdf.get_page(index).map_err(|_| INVALID)?;
    let value = page
        .as_dict()
        .ok_or(INVALID)?
        .get("UserUnit")
        .map(|value| pdf.resolve_object(value))
        .transpose()
        .map_err(|_| INVALID)?;
    let unit = match value {
        Some(Object::Integer(n)) => n as f64,
        Some(Object::Real(n)) => n,
        _ => 1.0,
    };
    Ok(if unit.is_finite() && unit > 0.0 {
        unit
    } else {
        1.0
    })
}

struct Page {
    source: usize,
    page: usize,
    output: u32,
}

#[derive(Clone, Copy)]
struct CopySource<'a> {
    pdf: &'a PdfDocument,
    page_tree: Option<&'a Source>,
}

impl<'a> From<&'a Source> for CopySource<'a> {
    fn from(source: &'a Source) -> Self {
        Self {
            pdf: &source.pdf,
            page_tree: Some(source),
        }
    }
}

impl<'a> From<&'a PdfDocument> for CopySource<'a> {
    fn from(pdf: &'a PdfDocument) -> Self {
        Self {
            pdf,
            page_tree: None,
        }
    }
}

struct Writer<'a> {
    preserve_page_tree: bool,
    compression: Option<&'a mut crate::matters::compression::Images>,
    compressed_streams: HashMap<([u8; 32], bool), u32>,
    compression_srgb: Option<u32>,
    strip_resource_metadata: bool,
    replaced_visual_objects: HashSet<u32>,
    painted_visual_objects: HashSet<u32>,
    replaced_content_objects: HashSet<u32>,
    objects: Vec<Object>,
    imported: HashMap<(usize, ObjectRef), u32>,
    first: HashMap<(usize, usize), u32>,
    form_copies: Vec<(ObjectRef, u32)>,
}

impl Writer<'_> {
    fn isolated() -> Self {
        Self {
            preserve_page_tree: false,
            compression: None,
            compressed_streams: HashMap::new(),
            compression_srgb: None,
            strip_resource_metadata: false,
            replaced_visual_objects: HashSet::new(),
            painted_visual_objects: HashSet::new(),
            replaced_content_objects: HashSet::new(),
            objects: vec![Object::Null; 3],
            imported: HashMap::new(),
            first: HashMap::new(),
            form_copies: Vec::new(),
        }
    }

    fn isolated_page(&mut self, resources: Object, data: Vec<u8>, geo: Geometry) -> Result<()> {
        let content = self.add(Object::Stream {
            dict: Dictionary::new(),
            data: data.into(),
        })?;
        self.set(
            1,
            Object::Dictionary(dict([("Type", name("Catalog")), ("Pages", reference(2))])),
        );
        self.set(
            2,
            Object::Dictionary(dict([
                ("Type", name("Pages")),
                ("Kids", Object::Array(vec![reference(3)])),
                ("Count", Object::Integer(1)),
            ])),
        );
        let area = |bounds: [f64; 4]| Object::Array(bounds.into_iter().map(Object::Real).collect());
        self.set(
            3,
            Object::Dictionary(dict([
                ("Type", name("Page")),
                ("Parent", reference(2)),
                ("MediaBox", area(geo.media)),
                ("CropBox", area(geo.crop)),
                ("Rotate", Object::Integer(i64::from(geo.rotation))),
                ("Resources", resources),
                ("Contents", reference(content)),
            ])),
        );
        Ok(())
    }

    fn reserve(&mut self) -> Result<u32> {
        if self.objects.len() >= MAX_OBJECTS {
            return Err(COMPLEX);
        }
        self.objects.push(Object::Null);
        Ok(self.objects.len() as u32)
    }

    fn set(&mut self, id: u32, obj: Object) {
        self.objects[id as usize - 1] = obj;
    }

    fn omit_null_page_attributes(&mut self, source: &Source) -> Result<()> {
        for reference in source.pages.iter().chain(&source.branches) {
            let id = self
                .imported
                .get(&(0, *reference))
                .copied()
                .ok_or(INVALID)?;
            let index = id as usize - 1;
            let fields = self.objects[index].as_dict().ok_or(INVALID)?;
            let nulls = INHERITED_PAGE_KEYS
                .into_iter()
                .filter(|key| {
                    fields.get(*key).is_some_and(|value| {
                        let value = value
                            .as_reference()
                            .and_then(|reference| self.objects.get(reference.id as usize - 1))
                            .unwrap_or(value);
                        matches!(value, Object::Null)
                    })
                })
                .collect::<Vec<_>>();
            let Object::Dictionary(fields) = &mut self.objects[index] else {
                return Err(INVALID);
            };
            for key in nulls {
                fields.remove(key);
            }
        }
        Ok(())
    }

    fn add(&mut self, obj: Object) -> Result<u32> {
        let id = self.reserve()?;
        self.set(id, obj);
        Ok(id)
    }

    fn copy_dict<'s>(
        &mut self,
        source: impl Into<CopySource<'s>>,
        index: usize,
        fields: Dictionary,
        depth: usize,
    ) -> Result<Dictionary> {
        let source = source.into();
        let mut fields: Vec<_> = fields.into_iter().collect();
        fields.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        fields
            .into_iter()
            .map(|(key, value)| Ok((key, self.copy(source, index, value, depth + 1)?)))
            .collect()
    }

    fn copy<'s>(
        &mut self,
        source: impl Into<CopySource<'s>>,
        index: usize,
        value: Object,
        depth: usize,
    ) -> Result<Object> {
        let source = source.into();
        if depth > MAX_DEPTH {
            return Err(COMPLEX);
        }
        match value {
            Object::Reference(original) => {
                if let Some(tree) = source.page_tree.filter(|_| !self.preserve_page_tree) {
                    if let Some(&page) = tree.indexes.get(&original) {
                        return Ok(self
                            .first
                            .get(&(index, page))
                            .copied()
                            .map(reference)
                            .unwrap_or(Object::Null));
                    }
                    if tree.branches.contains(&original) {
                        return Ok(reference(2));
                    }
                }
                if let Some(&existing) = self.imported.get(&(index, original)) {
                    return Ok(reference(existing));
                }
                let value = source.pdf.load_object(original).map_err(|_| INVALID)?;
                let (value, attach_srgb) = match self.compression.as_deref_mut() {
                    Some(compression) => compression.replace(original, value),
                    None => (value, false),
                };
                let key = self
                    .compression
                    .as_ref()
                    .and_then(|_| lossless_stream_key(&value))
                    .map(|hash| (hash, attach_srgb));
                if let Some(id) = key.and_then(|key| self.compressed_streams.get(&key).copied()) {
                    self.imported.insert((index, original), id);
                    return Ok(reference(id));
                }
                let id = self.reserve()?;
                self.imported.insert((index, original), id);
                let mut written = self.copy(source, index, value, depth + 1)?;
                if attach_srgb {
                    match &mut written {
                        Object::Stream { dict, .. } => {
                            let profile = self.compression_srgb()?;
                            dict.insert(
                                "ColorSpace".into(),
                                Object::Array(vec![name("ICCBased"), reference(profile)]),
                            );
                        }
                        _ => return Err(INVALID),
                    }
                }
                self.set(id, written);
                if let Some(key) = key {
                    self.compressed_streams.insert(key, id);
                }
                Ok(reference(id))
            }
            Object::Array(values) => Ok(Object::Array(
                values
                    .into_iter()
                    .map(|value| self.copy(source, index, value, depth + 1))
                    .collect::<Result<_>>()?,
            )),
            Object::Dictionary(fields) => Ok(Object::Dictionary(
                self.copy_dict(source, index, fields, depth)?,
            )),
            Object::Stream { mut dict, data } => {
                dict.remove("Length");
                if self.strip_resource_metadata {
                    for key in [
                        "Metadata",
                        "PieceInfo",
                        "AF",
                        "OPI",
                        "Alternates",
                        "StructParent",
                    ] {
                        dict.remove(key);
                    }
                }
                let data = self
                    .compression
                    .as_deref_mut()
                    .and_then(|_| crate::matters::compression::Images::deflate(&mut dict, &data))
                    .map_or(data, Into::into);
                Ok(Object::Stream {
                    dict: self.copy_dict(source, index, dict, depth)?,
                    data,
                })
            }
            _ => Ok(value),
        }
    }

    fn compression_srgb(&mut self) -> Result<u32> {
        if let Some(profile) = self.compression_srgb {
            return Ok(profile);
        }
        let data = moxcms::ColorProfile::new_srgb()
            .encode()
            .map_err(|_| INVALID)?;
        let profile = self.add(Object::Stream {
            dict: dict([("N", Object::Integer(3)), ("Alternate", name("DeviceRGB"))]),
            data: data.into(),
        })?;
        self.compression_srgb = Some(profile);
        Ok(profile)
    }

    fn remap_link(
        &mut self,
        source: &Source,
        page: &Page,
        fields: &mut Dictionary,
    ) -> Result<bool> {
        let action = match fields.get("A") {
            Some(value) => Some(source.resolve(value)?),
            None => None,
        };
        let internal_action = action
            .as_ref()
            .and_then(Object::as_dict)
            .filter(|fields| fields.get("S").and_then(Object::as_name) == Some("GoTo"));
        let destination = fields
            .get("Dest")
            .or_else(|| internal_action.and_then(|action| action.get("D")));
        let Some(destination) = destination else {
            return Ok(internal_action.is_none());
        };
        let Some((target, tail)) = source.destination(destination, 0)? else {
            return Ok(false);
        };
        let output = if target == page.page {
            Some(page.output)
        } else {
            self.first.get(&(page.source, target)).copied()
        };
        let Some(output) = output else {
            return Ok(false);
        };
        let mut mapped = vec![reference(output)];
        for attr in tail {
            mapped.push(self.copy(source, page.source, attr, 0)?);
        }
        fields.remove("A");
        fields.remove("Dest");
        if let Some(action) = internal_action {
            let mut action = self.copy_dict(
                source,
                page.source,
                action
                    .iter()
                    .filter(|(key, _)| key.as_str() != "D")
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
                0,
            )?;
            action.insert("D".into(), Object::Array(mapped));
            fields.insert("A".into(), Object::Dictionary(action));
        } else {
            fields.insert("Dest".into(), Object::Array(mapped));
        }
        Ok(true)
    }

    fn page(
        &mut self,
        source: &Source,
        page: &Page,
        overlay: Option<&Overlay>,
        print: Option<Option<Paper>>,
        mut forms: Option<&mut forms::Forms>,
    ) -> Result<()> {
        let mut fields = effective_page(&source.pdf, page.page)?;
        fields.remove("Parent");
        let annotations = fields.remove("Annots");
        if print.is_some() {
            fields.remove("AA");
        }
        if forms.is_some() || overlay.is_some_and(|overlay| overlay.strip_page_metadata) {
            fields.retain(|key, _| {
                matches!(
                    key.as_str(),
                    "Type"
                        | "Contents"
                        | "Resources"
                        | "MediaBox"
                        | "CropBox"
                        | "BleedBox"
                        | "TrimBox"
                        | "ArtBox"
                        | "Rotate"
                        | "Group"
                        | "UserUnit"
                )
            });
        }
        let mut fields = self.copy_dict(source, page.source, fields, 0)?;
        fields.insert("Parent".into(), reference(2));
        if overlay.is_some_and(|overlay| overlay.strip_page_metadata) {
            self.scrub_shaped_fonts(source, page, &mut fields)?;
        }
        if print.is_none() {
            let list = annotations
                .as_ref()
                .map(|list| source.resolve(list))
                .transpose()?;
            let list = match &list {
                Some(Object::Array(items)) => items.as_slice(),
                None | Some(Object::Null) => &[],
                _ => return Err(INVALID),
            };
            let mut refs = Vec::new();
            let mut widgets = Vec::new();
            let mut links = Vec::new();
            for annotation in list {
                let object = source.resolve(annotation)?;
                let mut fields = object.as_dict().cloned().ok_or(INVALID)?;
                let subtype = fields.get("Subtype").and_then(Object::as_name);
                let link = subtype == Some("Link");
                if forms.is_some() {
                    if subtype == Some("Widget") {
                        widgets.push(annotation.clone());
                        continue;
                    }
                    if subtype == Some("Popup")
                        || fields
                            .get("IRT")
                            .is_some_and(|v| !matches!(v, Object::Null))
                    {
                        continue;
                    }
                    fields.remove("Popup");
                }
                if link && !self.remap_link(source, page, &mut fields)? {
                    continue;
                }
                // Destination was already mapped; copy the remaining page-local keys.
                let navigation = link.then(|| fields.remove("Dest")).flatten();
                let action = link
                    && navigation.is_none()
                    && fields
                        .get("A")
                        .and_then(Object::as_dict)
                        .and_then(|action| action.get("S"))
                        .and_then(Object::as_name)
                        == Some("GoTo");
                let action = action.then(|| fields.remove("A")).flatten();
                fields.remove("P");
                let mut fields = self.copy_dict(source, page.source, fields, 0)?;
                fields.insert("P".into(), reference(page.output));
                if let Some(navigation) = navigation {
                    fields.insert("Dest".into(), navigation);
                }
                if let Some(action) = action {
                    fields.insert("A".into(), action);
                }
                let id = self.add(Object::Dictionary(fields))?;
                if forms.is_some() && link {
                    links.push(reference(id));
                } else {
                    refs.push(reference(id));
                }
            }
            if let Some(forms) = forms.take() {
                refs.extend(forms.page(self, source, page, widgets)?);
                refs.extend(links);
            }
            if !refs.is_empty() {
                fields.insert("Annots".into(), Object::Array(refs));
            }
        }
        if let Some(overlay) = overlay {
            if let Some(content) = &overlay.replace_page_content {
                for old in self.contents(&mut fields)? {
                    if let Some(reference) = old.as_reference().filter(|_| !overlay.redact_content)
                    {
                        self.replaced_content_objects.insert(reference.id);
                    }
                }
                let replacement = self.add(Object::Stream {
                    dict: Dictionary::new(),
                    data: content.clone().into(),
                })?;
                fields.insert("Contents".into(), reference(replacement));
            }
            if overlay.remove_page_number {
                self.remove_tagged_contents(&mut fields, NUMBER_STREAM)?;
            } else if !overlay.strip_page_metadata {
                self.append_overlay(
                    &mut fields,
                    source,
                    page,
                    overlay,
                    overlay.ocr.then_some(OCR_STREAM),
                )?;
                if let Some(label) = &overlay.page_number {
                    self.append_overlay(&mut fields, source, page, label, Some(NUMBER_STREAM))?;
                }
                if let Some(marks) = &overlay.output_marks {
                    self.append_overlay(&mut fields, source, page, marks, None)?;
                }
            }
        }
        if let Some(Some(paper)) = print {
            self.reframe(&mut fields, source, page.page, paper)?;
        }
        self.set(page.output, Object::Dictionary(fields));
        Ok(())
    }

    fn reference_dict(&self, value: Option<&Object>) -> Result<Dictionary> {
        let object = match value {
            Some(Object::Reference(reference)) => {
                Some(self.objects.get(reference.id as usize - 1).ok_or(INVALID)?)
            }
            direct => direct,
        };
        match object {
            Some(Object::Dictionary(fields)) => Ok(fields.clone()),
            Some(Object::Null) | None => Ok(Dictionary::new()),
            Some(_) => Err(INVALID),
        }
    }

    fn add_stream(&mut self, data: String) -> Result<Object> {
        self.content_stream(data, None)
    }

    fn content_stream(&mut self, data: String, tag: Option<&str>) -> Result<Object> {
        let fields = tag
            .into_iter()
            .map(|tag| (tag.to_owned(), Object::Boolean(true)))
            .collect();
        Ok(reference(self.add(Object::Stream {
            dict: fields,
            data: data.into(),
        })?))
    }

    fn contents(&self, fields: &mut Dictionary) -> Result<Vec<Object>> {
        Ok(match fields.remove("Contents") {
            Some(Object::Array(items)) => items,
            Some(Object::Reference(reference)) => match self.objects.get(reference.id as usize - 1)
            {
                Some(Object::Array(items)) => items.clone(),
                Some(Object::Stream { .. }) => vec![Object::Reference(reference)],
                _ => return Err(INVALID),
            },
            None | Some(Object::Null) => Vec::new(),
            Some(_) => return Err(INVALID),
        })
    }

    fn wrap_contents(
        &mut self,
        fields: &mut Dictionary,
        start: String,
        end: String,
        tag: Option<&str>,
    ) -> Result<()> {
        let mut contents = self.contents(fields)?;
        contents.insert(0, self.content_stream(start, tag)?);
        contents.push(self.content_stream(end, tag)?);
        fields.insert("Contents".into(), Object::Array(contents));
        Ok(())
    }

    fn remove_tagged_contents(&self, fields: &mut Dictionary, tag: &str) -> Result<()> {
        let mut contents = self.contents(fields)?;
        let count = contents.len();
        contents.retain(|value| {
            !value
                .as_reference()
                .and_then(|reference| self.objects.get(reference.id as usize - 1))
                .and_then(Object::as_dict)
                .is_some_and(|dict| dict.get(tag) == Some(&Object::Boolean(true)))
        });
        if count - contents.len() != 2 {
            return Err("The page's generated content cannot be isolated");
        }
        fields.insert("Contents".into(), Object::Array(contents));
        Ok(())
    }

    fn append_overlay(
        &mut self,
        fields: &mut Dictionary,
        source: &Source,
        page: &Page,
        overlay: &Overlay,
        tag: Option<&str>,
    ) -> Result<()> {
        let mut resources = self.reference_dict(fields.get("Resources"))?;
        if let Some(properties) = &overlay.replace_properties {
            resources.insert(
                "Properties".into(),
                Object::Dictionary(self.copy_dict(source, page.source, properties.clone(), 0)?),
            );
        }
        let mut fonts = self.reference_dict(resources.get("Font"))?;
        let mut images = self.reference_dict(resources.get("XObject"))?;
        let mut states = self.reference_dict(resources.get("ExtGState"))?;
        let prefix = (0..)
            .map(|index| format!("MD{index}_"))
            .find(|prefix| {
                fonts
                    .keys()
                    .chain(images.keys())
                    .chain(states.keys())
                    .all(|key| !key.starts_with(prefix))
            })
            .ok_or(COMPLEX)?;
        let mut textbox_resources = Vec::new();
        for (index, font) in overlay.fonts.iter().enumerate() {
            let alias = overlay
                .textbox_fonts
                .then(|| textbox_font_alias(font))
                .flatten();
            let key = if let Some(alias) = alias {
                if let Some(existing) = fonts.get(alias) {
                    if self
                        .reference_dict(Some(existing))
                        .is_ok_and(|fields| matches_textbox_font(&fields, font))
                    {
                        textbox_resources.push((index, alias));
                        continue;
                    }
                    format!("{prefix}F{index}")
                } else {
                    textbox_resources.push((index, alias));
                    alias.to_owned()
                }
            } else {
                format!("{prefix}F{index}")
            };
            let object = self.add(Object::Dictionary(dict([
                ("Type", name("Font")),
                ("Subtype", name("Type1")),
                ("BaseFont", name(font)),
                ("Encoding", name("WinAnsiEncoding")),
            ])))?;
            fonts.insert(key, reference(object));
        }
        for (index, glyphs) in overlay.glyph_fonts.iter().enumerate() {
            let font = self.glyph_font(glyphs)?;
            fonts.insert(format!("{prefix}O{index}"), reference(font));
        }
        for (index, glyphs) in overlay.shaped_fonts.iter().enumerate() {
            let font = self.shaped_font(glyphs)?;
            fonts.insert(format!("{prefix}S{index}"), reference(font));
        }
        for (index, opacity) in overlay.opacity.iter().copied().enumerate() {
            states.insert(
                format!("{prefix}G{index}"),
                Object::Dictionary(dict([
                    ("Type", name("ExtGState")),
                    ("ca", Object::Real(opacity)),
                ])),
            );
        }
        if let Some(key) = &overlay.reset_raster_alpha {
            if states.contains_key(key) {
                return Err(INVALID);
            }
            states.insert(
                key.clone(),
                Object::Dictionary(dict([
                    ("Type", name("ExtGState")),
                    ("ca", Object::Integer(1)),
                    ("CA", Object::Integer(1)),
                    ("SMask", name("None")),
                ])),
            );
        }
        for key in &overlay.clear_soft_masks {
            let state = states.get_mut(key).ok_or(INVALID)?;
            let mut fields = self.reference_dict(Some(state))?;
            fields.insert("SMask".into(), name("None"));
            *state = Object::Dictionary(fields);
        }
        for (kind, keep) in [
            ("Pattern", &overlay.retain_patterns),
            ("Shading", &overlay.retain_shadings),
            ("Properties", &overlay.retain_properties),
        ] {
            if let Some(keep) = keep {
                let mut entries = self.reference_dict(resources.get(kind))?;
                for (key, entry) in &entries {
                    if let Some(reference) = entry.as_reference() {
                        if keep.contains(key) {
                            self.painted_visual_objects.insert(reference.id);
                        } else {
                            self.replaced_visual_objects.insert(reference.id);
                            self.retire_nested_visual_resources(reference.id)?;
                        }
                    }
                }
                entries.retain(|key, _| keep.contains(key));
                if entries.is_empty() {
                    resources.remove(kind);
                } else {
                    resources.insert(kind.into(), Object::Dictionary(entries));
                }
            }
        }
        for (index, image) in overlay.images.iter().enumerate() {
            let mut samples = image.samples.clone();
            if let Object::Stream { dict, .. } = &mut samples {
                if let Some(mask) = &image.mask {
                    dict.insert("SMask".into(), reference(self.add(mask.clone())?));
                }
                if let Some(profile) = &image.profile {
                    dict.insert(
                        "ColorSpace".into(),
                        Object::Array(vec![
                            name("ICCBased"),
                            reference(self.add(profile.clone())?),
                        ]),
                    );
                }
            }
            let image = self.add(samples)?;
            images.insert(format!("{prefix}I{index}"), reference(image));
        }
        if !overlay.unicode_lines.is_empty() {
            let form = self.unicode_form(source, page, &overlay.unicode_lines)?;
            images.insert(format!("{prefix}U"), reference(form));
        }
        for edit in &overlay.xobject_edits {
            if let Some(source) = &edit.source {
                let original = images.get(source).ok_or(INVALID)?;
                if let Some(reference) = original.as_reference() {
                    self.replaced_visual_objects.insert(reference.id);
                    self.retire_nested_visual_resources(reference.id)?;
                }
            }
            if images.contains_key(&edit.target) {
                return Err(INVALID);
            }
            let object = match &edit.replacement {
                XObjectReplacement::Image(image, mask) => {
                    let mut image = self.copy(source, page.source, image.clone(), 0)?;
                    if let (Object::Stream { dict, .. }, Some(mask)) = (&mut image, mask) {
                        dict.insert("SMask".into(), reference(self.add(mask.clone())?));
                    }
                    image
                }
                XObjectReplacement::Form {
                    body,
                    fields,
                    inverse,
                } => {
                    let temporary = Source::open(body.clone())?;
                    let page_fields = temporary.pdf.get_page_for_debug(0).map_err(|_| INVALID)?;
                    let page_fields = page_fields.as_dict().ok_or(INVALID)?;
                    let mut dict = self.copy_dict(source, page.source, fields.clone(), 0)?;
                    if let Some(resources) = page_fields.get("Resources") {
                        let copied = self.copy(
                            &temporary,
                            usize::MAX / 2 + self.objects.len(),
                            resources.clone(),
                            0,
                        )?;
                        dict.insert("Resources".into(), copied);
                    }
                    let content = temporary
                        .pdf
                        .get_page_content_data(0)
                        .map_err(|_| INVALID)?;
                    let mut data = form_transform(*inverse);
                    data.extend(content);
                    data.extend_from_slice(b"\nQ\n");
                    Object::Stream {
                        dict,
                        data: data.into(),
                    }
                }
                XObjectReplacement::EditedForm(form) => {
                    let id = self.copy_edited_form(source, page.source, form)?;
                    images.insert(edit.target.clone(), reference(id));
                    continue;
                }
                XObjectReplacement::EmptyForm => Object::Stream {
                    dict: dict([
                        ("Type", name("XObject")),
                        ("Subtype", name("Form")),
                        (
                            "BBox",
                            Object::Array([0, 0, 1, 1].into_iter().map(Object::Integer).collect()),
                        ),
                    ]),
                    data: Vec::<u8>::new().into(),
                },
            };
            images.insert(edit.target.clone(), reference(self.add(object)?));
        }
        if let Some(keep) = &overlay.retain_xobjects {
            images.retain(|key, _| keep.contains(key));
            for object in images.values() {
                if let Some(reference) = object.as_reference() {
                    self.painted_visual_objects.insert(reference.id);
                }
            }
        }
        if !fonts.is_empty() {
            resources.insert("Font".into(), Object::Dictionary(fonts));
        }
        if !images.is_empty() {
            resources.insert("XObject".into(), Object::Dictionary(images));
        } else {
            resources.remove("XObject");
        }
        if !states.is_empty() {
            resources.insert("ExtGState".into(), Object::Dictionary(states));
        }
        fields.insert("Resources".into(), Object::Dictionary(resources));
        let [a, b, c, d, e, f] = if overlay.user_space_text {
            [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]
        } else {
            let mut geometry = geometry(&source.pdf, page.page)?;
            if overlay.unrotated_page_coordinates {
                geometry = geometry.clipped().unwrap_or(geometry);
                geometry.rotation = 0;
            }
            geometry.display_matrix()
        };
        let mut commands = overlay.commands.clone();
        for (index, alias) in textbox_resources {
            commands = commands.replace(&format!("/@F{index} "), &format!("/{alias} "));
        }
        let mut commands = commands
            .replace("/@F", &format!("/{prefix}F"))
            .replace("/@I", &format!("/{prefix}I"))
            .replace("/@O", &format!("/{prefix}O"))
            .replace("/@S", &format!("/{prefix}S"))
            .replace("/@G", &format!("/{prefix}G"));
        if !overlay.unicode_lines.is_empty() {
            use std::fmt::Write as _;
            writeln!(commands, "q /{prefix}U Do Q").map_err(|_| INVALID)?;
        }
        if let Some(tag) = &overlay.replacement_tag {
            let mcid = self.tag_replacement(
                fields,
                page.output,
                tag,
                overlay.shared_actual_text.as_ref(),
            )?;
            commands = format!("/Span <</MCID {mcid}>> BDC\n{commands}\nEMC\n");
        }
        let paint = format!("q {a} {b} {c} {d} {e} {f} cm\n{commands}Q\n");
        let (prefix, suffix) = if overlay.paint_before_source {
            (format!("{paint}q\n"), "\nQ\n".into())
        } else {
            ("q\n".into(), format!("\nQ\n{paint}"))
        };
        self.wrap_contents(fields, prefix, suffix, tag)
    }

    fn copy_edited_form<'s>(
        &mut self,
        source: impl Into<CopySource<'s>>,
        index: usize,
        form: &EditedForm,
    ) -> Result<u32> {
        let source = source.into();
        let mut fields = self.copy_dict(source, index, form.fields.clone(), 0)?;
        if !form.children.is_empty() {
            let mut resources = self.reference_dict(fields.get("Resources"))?;
            let mut xobjects = self.reference_dict(resources.get("XObject"))?;
            for (key, child) in &form.children {
                if xobjects.contains_key(key) {
                    return Err(INVALID);
                }
                let child = self.copy_edited_form(source, index, child)?;
                xobjects.insert(key.clone(), reference(child));
            }
            resources.insert("XObject".into(), Object::Dictionary(xobjects));
            fields.insert("Resources".into(), Object::Dictionary(resources));
        }
        let id = self.add(Object::Stream {
            dict: fields,
            data: form.data.clone().into(),
        })?;
        self.form_copies.push((form.original, id));
        Ok(id)
    }

    fn unicode_form(&mut self, source: &Source, page: &Page, lines: &[UnicodeLine]) -> Result<u32> {
        use pdf_oxide::writer::{DocumentBuilder, EmbeddedFont, PageSize};
        let regular = EmbeddedFont::from_data(
            None,
            include_bytes!("../../../assets/fonts/NotoSansDevanagari-Regular.ttf").to_vec(),
        )
        .map_err(|_| "Cannot read bundled Unicode font")?;
        let bold = EmbeddedFont::from_data(
            None,
            include_bytes!("../../../assets/fonts/NotoSansDevanagari-Bold.ttf").to_vec(),
        )
        .map_err(|_| "Cannot read bundled Unicode font")?;
        if lines.iter().any(|line| {
            let font = if line.bold { &bold } else { &regular };
            line.text.chars().any(|character| {
                !matches!(character, '\u{200c}' | '\u{200d}')
                    && font
                        .glyph_id(character as u32)
                        .is_none_or(|glyph| glyph == 0)
            })
        }) {
            return Err("A generated title uses characters outside the bundled PDF fonts");
        }
        let (width, height) = source.geometry(page.page)?.display_size();
        let mut builder = DocumentBuilder::new()
            .register_embedded_font("MatterDraftDevanagari", regular)
            .register_embedded_font("MatterDraftDevanagariBold", bold);
        let mut generated = builder.page(PageSize::Custom(width as f32, height as f32));
        for line in lines {
            let name = if line.bold {
                "MatterDraftDevanagariBold"
            } else {
                "MatterDraftDevanagari"
            };
            generated = generated
                .font(name, line.size)
                .at(line.x, line.y)
                .inline_color(line.ink.0, line.ink.1, line.ink.2, &line.text);
        }
        generated.done();
        let bytes = builder
            .build()
            .map_err(|_| "Cannot shape Unicode PDF text")?;
        let temporary = Source::open(bytes)?;
        let generated = temporary.pdf.get_page_for_debug(0).map_err(|_| INVALID)?;
        let resources = generated
            .as_dict()
            .and_then(|fields| fields.get("Resources"))
            .cloned()
            .ok_or(INVALID)?;
        let copied = self.copy(
            &temporary,
            usize::MAX / 2 + self.objects.len(),
            resources,
            0,
        )?;
        let contents = temporary
            .pdf
            .get_page_content_data(0)
            .map_err(|_| INVALID)?;
        self.add(Object::Stream {
            dict: dict([
                ("Type", name("XObject")),
                ("Subtype", name("Form")),
                (
                    "BBox",
                    Object::Array(vec![
                        Object::Integer(0),
                        Object::Integer(0),
                        Object::Real(width),
                        Object::Real(height),
                    ]),
                ),
                ("Resources", copied),
            ]),
            data: contents.into(),
        })
    }

    fn scrub_shaped_fonts(
        &mut self,
        source: &Source,
        page: &Page,
        fields: &mut Dictionary,
    ) -> Result<()> {
        use pdf_oxide::content::{parse_content_stream, Operator, TextElement};

        let mut resources = self.reference_dict(fields.get("Resources"))?;
        let mut fonts = self.reference_dict(resources.get("Font"))?;
        let mut shaped = HashMap::new();
        for (key, object) in &fonts {
            let font = self.reference_dict(Some(object))?;
            let Some(cmap) = font.get("ToUnicode").and_then(Object::as_reference) else {
                continue;
            };
            let stream = self.objects.get(cmap.id as usize - 1).ok_or(INVALID)?;
            let Some(dictionary) = stream.as_dict() else {
                continue;
            };
            if dictionary.get("CMapName").and_then(Object::as_name) != Some("MatterDraftShaped") {
                continue;
            }
            let data = stream.decode_stream_data().map_err(|_| INVALID)?;
            let width = if font.get("Subtype").and_then(Object::as_name) == Some("Type3") {
                1
            } else {
                2
            };
            shaped.insert(key.clone(), (font, cmap.id, data, width, HashSet::new()));
        }
        if shaped.is_empty() {
            return Ok(());
        }

        let data = source
            .pdf
            .get_page_content_data(page.page)
            .map_err(|_| INVALID)?;
        let mut active: Option<String> = None;
        let mut stack = Vec::new();
        for operator in parse_content_stream(&data).map_err(|_| INVALID)? {
            match operator {
                Operator::SaveState => stack.push(active.clone()),
                Operator::RestoreState => active = stack.pop().ok_or(INVALID)?,
                Operator::Tf { font, .. } => active = Some(font),
                Operator::Tj { text }
                | Operator::Quote { text }
                | Operator::DoubleQuote { text, .. } => {
                    if let Some((_, _, _, width, keep)) =
                        active.as_ref().and_then(|font| shaped.get_mut(font))
                    {
                        extend_shaped_codes(keep, &text, *width)?;
                    }
                }
                Operator::TJ { array } => {
                    if let Some((_, _, _, width, keep)) =
                        active.as_ref().and_then(|font| shaped.get_mut(font))
                    {
                        for element in array {
                            if let TextElement::String(bytes) = element {
                                extend_shaped_codes(keep, &bytes, *width)?;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        for (key, (mut font, id, data, width, keep)) in shaped {
            let cmap = prune_shaped_cmap(&data, &keep)?;
            // All page resource dictionaries get a placement-specific map. If
            // anything still references the old map, refuse to serialize it.
            self.replaced_content_objects.insert(id);
            font.insert("ToUnicode".into(), self.shaped_cmap_stream(cmap)?);
            if width == 1 {
                let mut procedures = self.reference_dict(font.get("CharProcs"))?;
                procedures.retain(|name, _| {
                    name.strip_prefix('g')
                        .and_then(|code| code.parse().ok())
                        .is_some_and(|code| keep.contains(&code))
                });
                font.insert("CharProcs".into(), Object::Dictionary(procedures));
            } else {
                let descendant = font
                    .get("DescendantFonts")
                    .and_then(Object::as_array)
                    .and_then(|fonts| fonts.first())
                    .ok_or(INVALID)?;
                let mut descendant = self.reference_dict(Some(descendant))?;
                let map = descendant
                    .get("CIDToGIDMap")
                    .and_then(Object::as_reference)
                    .ok_or(INVALID)?;
                let mut bytes = self
                    .objects
                    .get(map.id.checked_sub(1).ok_or(INVALID)? as usize)
                    .ok_or(INVALID)?
                    .decode_stream_data()
                    .map_err(|_| INVALID)?;
                if bytes.len() % 2 != 0 {
                    return Err(INVALID);
                }
                self.replaced_content_objects.insert(map.id);
                for (code, glyph) in bytes.chunks_exact_mut(2).enumerate() {
                    if !u16::try_from(code).is_ok_and(|code| keep.contains(&code)) {
                        glyph.fill(0);
                    }
                }
                let replacement = self.add(Object::Stream {
                    dict: Dictionary::new(),
                    data: bytes.into(),
                })?;
                descendant.insert("CIDToGIDMap".into(), reference(replacement));
                let descendant = self.add(Object::Dictionary(descendant))?;
                font.insert(
                    "DescendantFonts".into(),
                    Object::Array(vec![reference(descendant)]),
                );
            }
            fonts.insert(key, Object::Dictionary(font));
        }
        resources.insert("Font".into(), Object::Dictionary(fonts));
        fields.insert("Resources".into(), Object::Dictionary(resources));
        Ok(())
    }

    fn shaped_font(&mut self, font: &super::super::shaped_text::Font) -> Result<u32> {
        use pdf_oxide::fonts::TrueTypeFont;

        if font.outlines {
            return self.shaped_outline_font(font);
        }
        let bytes = font.source.embedded_font_data.as_deref().ok_or(INVALID)?;
        let face = TrueTypeFont::parse(bytes).map_err(|_| INVALID)?;
        let scale = 1000.0 / f64::from(face.units_per_em());
        let (l, b, r, t) = face.bbox();
        let base = name(&font.source.base_font);
        let font_file = self.add(Object::Stream {
            dict: dict([("Length1", Object::Integer(bytes.len() as i64))]),
            data: bytes.as_slice().to_vec().into(),
        })?;
        let descriptor = self.add(Object::Dictionary(dict([
            ("Type", name("FontDescriptor")),
            ("FontName", base.clone()),
            (
                "Flags",
                Object::Integer((font.source.flags.unwrap_or(4) | 4) as i64 & !32),
            ),
            (
                "FontBBox",
                Object::Array(
                    [l, b, r, t]
                        .map(|v| Object::Real(f64::from(v) * scale))
                        .to_vec(),
                ),
            ),
            (
                "Ascent",
                Object::Real(f64::from(font.source.ascent) * 1000.0),
            ),
            (
                "Descent",
                Object::Real(f64::from(font.source.descent) * 1000.0),
            ),
            (
                "CapHeight",
                Object::Real(f64::from(face.cap_height().unwrap_or(face.ascender())) * scale),
            ),
            ("ItalicAngle", Object::Real(f64::from(face.italic_angle()))),
            (
                "StemV",
                Object::Real(f64::from(font.source.stem_v.unwrap_or(80.0))),
            ),
            ("FontFile2", reference(font_file)),
        ])))?;
        let mut glyph_map = vec![0, 0];
        let mut widths = Vec::with_capacity(font.glyphs.len());
        for (gid, _) in &font.glyphs {
            glyph_map.extend_from_slice(&gid.to_be_bytes());
            widths.push(Object::Integer(i64::from(face.glyph_width(*gid))));
        }
        let unicode = self.shaped_unicode_map(&font.glyphs, false)?;
        let mapping = self.add(Object::Stream {
            dict: Dictionary::new(),
            data: glyph_map.into(),
        })?;
        let descendant = self.add(Object::Dictionary(dict([
            ("Type", name("Font")),
            ("Subtype", name("CIDFontType2")),
            ("BaseFont", base.clone()),
            (
                "CIDSystemInfo",
                Object::Dictionary(dict([
                    ("Registry", Object::String(b"Adobe".to_vec())),
                    ("Ordering", Object::String(b"Identity".to_vec())),
                    ("Supplement", Object::Integer(0)),
                ])),
            ),
            ("FontDescriptor", reference(descriptor)),
            ("CIDToGIDMap", reference(mapping)),
            (
                "W",
                Object::Array(vec![Object::Integer(1), Object::Array(widths)]),
            ),
        ])))?;
        self.add(Object::Dictionary(dict([
            ("Type", name("Font")),
            ("Subtype", name("Type0")),
            ("BaseFont", base),
            ("Encoding", name("Identity-H")),
            (
                "DescendantFonts",
                Object::Array(vec![reference(descendant)]),
            ),
            ("ToUnicode", unicode),
        ])))
    }

    fn shaped_outline_font(&mut self, font: &super::super::shaped_text::Font) -> Result<u32> {
        let bytes = font.source.embedded_font_data.as_deref().ok_or(INVALID)?;
        let face = ttf_parser::Face::parse(bytes, 0).map_err(|_| INVALID)?;
        let scale = 1.0 / f64::from(face.units_per_em());
        let bbox = face.global_bounding_box();
        let bounds = [bbox.x_min, bbox.y_min, bbox.x_max, bbox.y_max]
            .map(|value| Object::Integer(i64::from(value)));
        let mut widths = Vec::new();
        let mut names = vec![Object::Integer(1)];
        let mut procedures = Dictionary::new();
        for (index, (gid, _)) in font.glyphs.iter().enumerate() {
            let glyph = format!("g{}", index + 1);
            let (width, path) = super::super::shaped_text::glyph_outline(&face, *gid)?;
            widths.push(Object::Integer(width));
            names.push(name(&glyph));
            procedures.insert(glyph, self.add_stream(path)?);
        }
        let unicode = self.shaped_unicode_map(&font.glyphs, true)?;
        self.add(Object::Dictionary(dict([
            ("Type", name("Font")),
            ("Subtype", name("Type3")),
            ("Name", name(&font.source.base_font)),
            ("BaseFont", name(&font.source.base_font)),
            ("FontBBox", Object::Array(bounds.into())),
            (
                "FontMatrix",
                Object::Array([scale, 0.0, 0.0, scale, 0.0, 0.0].map(Object::Real).into()),
            ),
            ("CharProcs", Object::Dictionary(procedures)),
            (
                "Encoding",
                Object::Dictionary(dict([("Differences", Object::Array(names))])),
            ),
            ("FirstChar", Object::Integer(1)),
            ("LastChar", Object::Integer(font.glyphs.len() as i64)),
            ("Widths", Object::Array(widths)),
            ("Resources", Object::Dictionary(Dictionary::new())),
            ("ToUnicode", unicode),
        ])))
    }

    fn shaped_unicode_map(&mut self, glyphs: &[(u16, String)], simple: bool) -> Result<Object> {
        use std::fmt::Write as _;

        let (width, space) = if simple {
            (2, "<00> <FF>")
        } else {
            (4, "<0000> <FFFF>")
        };
        let mut cmap = format!("/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /MatterDraftShaped def\n/CMapType 2 def\n1 begincodespacerange\n{space}\nendcodespacerange\n");
        for (block, chunk) in glyphs.chunks(100).enumerate() {
            writeln!(cmap, "{} beginbfchar", chunk.len()).map_err(|_| INVALID)?;
            for (index, (_, unicode)) in chunk.iter().enumerate() {
                let code = block * 100 + index + 1;
                write!(cmap, "<{code:0width$X}> <").map_err(|_| INVALID)?;
                for unit in unicode.encode_utf16() {
                    write!(cmap, "{unit:04X}").map_err(|_| INVALID)?;
                }
                cmap.push_str(">\n");
            }
            cmap.push_str("endbfchar\n");
        }
        cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
        self.shaped_cmap_stream(cmap)
    }

    fn shaped_cmap_stream(&mut self, data: String) -> Result<Object> {
        Ok(reference(self.add(Object::Stream {
            dict: dict([
                ("Type", name("CMap")),
                ("CMapName", name("MatterDraftShaped")),
            ]),
            data: data.into_bytes().into(),
        })?))
    }

    fn glyph_font(&mut self, glyphs: &[char]) -> Result<u32> {
        use std::fmt::Write as _;
        if glyphs.is_empty() || glyphs.len() > 255 {
            return Err(INVALID);
        }
        let glyph = self.add_stream("600 0 d0\n".into())?;
        let mut mapping = String::from("/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /MatterDraftOCR def\n/CMapType 2 def\n1 begincodespacerange\n<00> <FF>\nendcodespacerange\n");
        let mut names = vec![Object::Integer(1)];
        let mut procedures = Dictionary::new();
        for (block, chunk) in glyphs.chunks(100).enumerate() {
            writeln!(mapping, "{} beginbfchar", chunk.len()).unwrap();
            for (index, character) in chunk.iter().enumerate() {
                let code = 1 + block * 100 + index;
                let name = format!("g{code}");
                names.push(Object::Name(name.clone()));
                procedures.insert(name, glyph.clone());
                let mut utf16 = [0; 2];
                write!(mapping, "<{code:02X}> <").unwrap();
                for unit in character.encode_utf16(&mut utf16) {
                    write!(mapping, "{unit:04X}").unwrap();
                }
                mapping.push_str(">\n");
            }
            mapping.push_str("endbfchar\n");
        }
        mapping.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
        let unicode = self.add_stream(mapping)?;
        let integer_array = |values: &[i64]| {
            Object::Array(values.iter().map(|&value| Object::Integer(value)).collect())
        };
        self.add(Object::Dictionary(dict([
            ("Type", name("Font")),
            ("Subtype", name("Type3")),
            ("Name", name("MatterDraftOCR")),
            ("FontBBox", integer_array(&[0, 0, 600, 1000])),
            (
                "FontMatrix",
                Object::Array(vec![
                    Object::Real(0.001),
                    Object::Integer(0),
                    Object::Integer(0),
                    Object::Real(0.001),
                    Object::Integer(0),
                    Object::Integer(0),
                ]),
            ),
            (
                "Encoding",
                Object::Dictionary(dict([
                    ("Type", name("Encoding")),
                    ("Differences", Object::Array(names)),
                ])),
            ),
            ("FirstChar", Object::Integer(1)),
            ("LastChar", Object::Integer(glyphs.len() as i64)),
            (
                "Widths",
                Object::Array(vec![Object::Integer(600); glyphs.len()]),
            ),
            ("CharProcs", Object::Dictionary(procedures)),
            ("Resources", Object::Dictionary(Dictionary::new())),
            ("ToUnicode", unicode),
        ])))
    }

    fn reframe(
        &mut self,
        fields: &mut Dictionary,
        source: &Source,
        page: usize,
        paper: Paper,
    ) -> Result<()> {
        let geometry = source.geometry(page)?;
        let unit = page_user_unit(&source.pdf, page)?;
        let intersect = |a: [f64; 4], b: [f64; 4]| {
            [
                a[0].max(b[0]),
                a[1].max(b[1]),
                a[2].min(b[2]),
                a[3].min(b[3]),
            ]
        };
        let visible = geometry
            .clipped()
            .map_or([0.0, 0.0, 612.0, 792.0], |visible| visible.crop);
        let (width, height) = Geometry {
            crop: visible,
            ..geometry
        }
        .display_size();
        let (width, height) = (width * unit, height * unit);
        // The reference maps the displayed rectangle back through the page
        // transformation matrix. For a rotated page that matrix flips Y at
        // the stored crop height and omits the unit scaling.
        let frame = if geometry.rotation == 0 {
            visible
        } else {
            let top = geometry.crop[3] - geometry.crop[1];
            [0.0, top - height, width, top]
        };
        let (target_width, target_height) = if paper.fit {
            (paper.width - 36.0, paper.height - 36.0)
        } else {
            (width, height)
        };
        let [left, bottom, right, top] = frame;
        let scale = (target_width / (right - left)).min(target_height / (top - bottom));
        let x = (paper.width - (left + right) * scale) / 2.0;
        let y = (paper.height - (bottom + top) * scale) / 2.0;
        if scale <= 0.0 || ![width, height, scale, x, y].into_iter().all(f64::is_finite) {
            return Err(INVALID);
        }
        let [l, b, r, t] = intersect(frame, geometry.media);
        let start = format!(
            "q {scale:.9} 0 0 {scale:.9} {x:.9} {y:.9} cm\n{l:.9} {b:.9} {:.9} {:.9} re W n\n",
            (r - l).max(0.0),
            (t - b).max(0.0)
        );
        for key in [
            "CropBox", "BleedBox", "TrimBox", "ArtBox", "Rotate", "UserUnit",
        ] {
            fields.remove(key);
        }
        fields.insert(
            "MediaBox".into(),
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Real(paper.width),
                Object::Real(paper.height),
            ]),
        );
        self.wrap_contents(fields, start, "\nQ\n".into(), None)
    }

    fn generated_navigation(
        &mut self,
        catalog: &mut Dictionary,
        sources: &[Source],
        pages: &[Page],
        navigation: &Navigation,
    ) -> Result<()> {
        if !navigation.labels.is_empty() {
            if navigation.labels.len() != pages.len() {
                return Err(INVALID);
            }
            let values = navigation
                .labels
                .iter()
                .enumerate()
                .flat_map(|(index, label)| {
                    [
                        Object::Integer(index as i64),
                        Object::Dictionary(dict([("P", pdf_text(label))])),
                    ]
                })
                .collect();
            catalog.insert(
                "PageLabels".into(),
                Object::Dictionary(dict([("Nums", Object::Array(values))])),
            );
        }
        for (index, region, target) in &navigation.links {
            let page = pages.get(*index).ok_or(INVALID)?;
            let source_geometry = geometry(&sources[page.source].pdf, page.page)?;
            let (a, b) = source_geometry.user_point(region[0], region[1]);
            let (c, d) = source_geometry.user_point(region[2], region[3]);
            let mut link = dict([
                ("Type", name("Annot")),
                ("Subtype", name("Link")),
                ("P", reference(page.output)),
                (
                    "Rect",
                    Object::Array(
                        [a.min(c), b.min(d), a.max(c), b.max(d)]
                            .into_iter()
                            .map(Object::Real)
                            .collect(),
                    ),
                ),
                ("Border", Object::Array(vec![Object::Integer(0); 3])),
            ]);
            match target {
                LinkTarget::Page(target) => {
                    let target = pages.get(*target).ok_or(INVALID)?;
                    let [left, top] = super::super::output_navigation::generated_link_origin(
                        &sources[target.source].pdf,
                        target.page,
                    )?;
                    link.insert(
                        "Dest".into(),
                        Object::Array(vec![
                            reference(target.output),
                            name("XYZ"),
                            Object::Real(left),
                            Object::Real(top),
                            Object::Integer(0),
                        ]),
                    );
                }
                LinkTarget::Uri(uri) => {
                    link.insert(
                        "A".into(),
                        Object::Dictionary(dict([
                            ("S", name("URI")),
                            ("URI", Object::String(uri.as_bytes().to_vec())),
                        ])),
                    );
                }
            }
            let link = reference(self.add(Object::Dictionary(link))?);
            let Object::Dictionary(fields) = &mut self.objects[page.output as usize - 1] else {
                return Err(INVALID);
            };
            let entry = fields
                .entry("Annots".into())
                .or_insert_with(|| Object::Array(Vec::new()));
            let Object::Array(annotations) = entry else {
                return Err(INVALID);
            };
            annotations.push(link);
        }
        if navigation.outlines.is_empty() {
            return Ok(());
        }
        let root = self.reserve()?;
        let mut nodes = Vec::new();
        let mut lineage = vec![root];
        let mut siblings = HashMap::<u32, Vec<u32>>::new();
        for (level, title, target) in &navigation.outlines {
            let target = pages.get(*target).ok_or(INVALID)?;
            if *level == 0 || *level > lineage.len() {
                return Err(INVALID);
            }
            lineage.truncate(*level);
            let parent = *lineage.last().ok_or(INVALID)?;
            let id = self.reserve()?;
            siblings.entry(parent).or_default().push(id);
            nodes.push((id, parent, title, target.output));
            lineage.push(id);
        }
        for (id, parent, title, target) in nodes {
            let mut fields = dict([
                ("Title", pdf_text(title)),
                ("Parent", reference(parent)),
                ("Dest", Object::Array(vec![reference(target), name("Fit")])),
            ]);
            let peers = &siblings[&parent];
            let index = peers.iter().position(|peer| *peer == id).ok_or(INVALID)?;
            if index > 0 {
                fields.insert("Prev".into(), reference(peers[index - 1]));
            }
            if let Some(next) = peers.get(index + 1) {
                fields.insert("Next".into(), reference(*next));
            }
            if let Some(children) = siblings.get(&id) {
                outline_children(&mut fields, children);
            }
            self.set(id, Object::Dictionary(fields));
        }
        let mut fields = dict([("Type", name("Outlines"))]);
        outline_children(&mut fields, &siblings[&root]);
        self.set(root, Object::Dictionary(fields));
        catalog.insert("Outlines".into(), reference(root));
        Ok(())
    }

    fn outline(
        &mut self,
        sources: &[Source],
        order: &[usize],
        preserve_missing: bool,
    ) -> Result<Option<u32>> {
        let mut entries = Vec::<(usize, Object, Option<u32>)>::new();
        for &index in order {
            let source = &sources[index];
            let Some(root) = source
                .catalog
                .as_dict()
                .and_then(|dict| dict.get("Outlines"))
            else {
                continue;
            };
            let root = source.resolve(root)?;
            let Some(first) = root
                .as_dict()
                .and_then(|root| root.get("First"))
                .and_then(Object::as_reference)
            else {
                continue;
            };
            let mut nodes = vec![(1, first)];
            let mut visited = HashSet::new();
            let mut previous = 0;
            while let Some((level, id)) = nodes.pop() {
                if level > MAX_DEPTH || visited.len() >= 10000 {
                    return Err(COMPLEX);
                }
                if !visited.insert(id) {
                    return Err(INVALID);
                }
                let item = source.pdf.load_object(id).map_err(|_| INVALID)?;
                let fields = item.as_dict().ok_or(INVALID)?;
                if let Some(next) = fields.get("Next").and_then(Object::as_reference) {
                    nodes.push((level, next));
                }
                if let Some(child) = fields.get("First").and_then(Object::as_reference) {
                    nodes.push((level + 1, child));
                }
                let action = fields
                    .get("A")
                    .map(|value| source.resolve(value))
                    .transpose()?;
                let destination = fields.get("Dest").or_else(|| {
                    action
                        .as_ref()
                        .and_then(Object::as_dict)
                        .filter(|action| action.get("S").and_then(Object::as_name) == Some("GoTo"))
                        .and_then(|action| action.get("D"))
                });
                let target = match destination {
                    Some(destination) => source
                        .destination(destination, 0)?
                        .and_then(|(page, _)| self.first.get(&(index, page)).copied()),
                    None => None,
                };
                if target.is_some() || preserve_missing {
                    let level = level.min(previous + 1);
                    let title = fields
                        .get("Title")
                        .map(|title| source.resolve(title))
                        .transpose()?
                        .unwrap_or(Object::String(Vec::new()));
                    entries.push((level, title, target));
                    previous = level;
                }
            }
        }
        if entries.is_empty() {
            return Ok(None);
        }
        let root = self.reserve()?;
        let mut ids = Vec::new();
        let mut children = HashMap::<u32, Vec<u32>>::new();
        let mut parents = vec![root];
        for (level, title, target) in entries {
            let id = self.reserve()?;
            parents.truncate(level);
            let parent = *parents.last().ok_or(INVALID)?;
            parents.push(id);
            children.entry(parent).or_default().push(id);
            ids.push((id, parent, title, target));
        }
        let structural = |fields: &mut Dictionary, id| {
            if let Some(kids) = children.get(&id) {
                fields.insert("First".into(), reference(kids[0]));
                fields.insert("Last".into(), reference(kids[kids.len() - 1]));
                fields.insert("Count".into(), Object::Integer(-(kids.len() as i64)));
            }
        };
        let mut root_dict = dict([("Type", name("Outlines"))]);
        structural(&mut root_dict, root);
        root_dict.insert(
            "Count".into(),
            Object::Integer(children[&root].len() as i64),
        );
        self.set(root, Object::Dictionary(root_dict));
        let mut cursors = HashMap::<u32, usize>::new();
        for (id, parent, title, target) in ids {
            let mut fields = dict([("Title", title), ("Parent", reference(parent))]);
            if let Some(target) = target {
                fields.insert(
                    "Dest".into(),
                    Object::Array(vec![
                        reference(target),
                        name("XYZ"),
                        Object::Null,
                        Object::Null,
                        Object::Null,
                    ]),
                );
            }
            let group = &children[&parent];
            let index = cursors.entry(parent).or_default();
            if *index > 0 {
                fields.insert("Prev".into(), reference(group[*index - 1]));
            }
            if let Some(&next) = group.get(*index + 1) {
                fields.insert("Next".into(), reference(next));
            }
            *index += 1;
            structural(&mut fields, id);
            self.set(id, Object::Dictionary(fields));
        }
        Ok(Some(root))
    }

    fn retire_nested_visual_resources(&mut self, id: u32) -> Result<()> {
        let initial = reference(id);
        let mut pending = vec![&initial];
        let mut seen = HashSet::new();
        let mut visual = Vec::new();
        while let Some(object) = pending.pop() {
            match object {
                Object::Reference(reference) if seen.insert(reference.id) => {
                    let child = self
                        .objects
                        .get(reference.id.checked_sub(1).ok_or(INVALID)? as usize)
                        .ok_or(INVALID)?;
                    if child.as_dict().is_some_and(|fields| {
                        matches!(
                            fields.get("Subtype").and_then(Object::as_name),
                            Some("Image" | "Form")
                        ) || fields.contains_key("PatternType")
                            || fields.contains_key("ShadingType")
                    }) {
                        visual.push(reference.id);
                    }
                    pending.push(child);
                }
                Object::Array(items) => pending.extend(items),
                Object::Dictionary(fields) | Object::Stream { dict: fields, .. } => {
                    pending.extend(fields.values())
                }
                _ => {}
            }
        }
        self.replaced_visual_objects.extend(visual);
        Ok(())
    }

    fn reachable(&self, info: Option<u32>) -> Result<HashSet<u32>> {
        let root = reference(1);
        let info = info.map(reference);
        let mut stack = vec![&root];
        stack.extend(info.iter());
        let mut seen = HashSet::new();
        while let Some(value) = stack.pop() {
            match value {
                Object::Reference(reference) => {
                    if seen.insert(reference.id) {
                        let value = self
                            .objects
                            .get(reference.id.checked_sub(1).ok_or(INVALID)? as usize)
                            .ok_or(INVALID)?;
                        stack.push(value);
                    }
                }
                Object::Stream { dict, .. } | Object::Dictionary(dict) => {
                    stack.extend(dict.values())
                }
                Object::Array(items) => stack.extend(items),
                _ => {}
            }
        }
        Ok(seen)
    }

    fn painted_resource_closure(&self) -> Result<HashSet<u32>> {
        use pdf_oxide::content::{parse_content_stream, Operator};
        let mut painted = HashSet::new();
        let mut pending = self
            .painted_visual_objects
            .iter()
            .copied()
            .collect::<Vec<_>>();
        while let Some(id) = pending.pop() {
            if !painted.insert(id) {
                continue;
            }
            let object = self
                .objects
                .get(id.checked_sub(1).ok_or(INVALID)? as usize)
                .ok_or(INVALID)?;
            let fields = object.as_dict().ok_or(INVALID)?;
            if fields.get("Subtype").and_then(Object::as_name) == Some("Image") {
                pending.extend(["SMask", "Mask"].into_iter().filter_map(|key| {
                    fields
                        .get(key)
                        .and_then(Object::as_reference)
                        .map(|reference| reference.id)
                }));
            } else if fields.get("PatternType").and_then(Object::as_integer) == Some(2) {
                pending.extend(
                    fields
                        .get("Shading")
                        .and_then(Object::as_reference)
                        .map(|reference| reference.id),
                );
            } else if fields.get("Subtype").and_then(Object::as_name) == Some("Form")
                || fields.get("PatternType").and_then(Object::as_integer) == Some(1)
            {
                let resources = self.reference_dict(fields.get("Resources"))?;
                let objects = self.reference_dict(resources.get("XObject"))?;
                let patterns = self.reference_dict(resources.get("Pattern"))?;
                let shadings = self.reference_dict(resources.get("Shading"))?;
                if objects.is_empty() && patterns.is_empty() && shadings.is_empty() {
                    continue;
                }
                let stream = object.decode_stream_data().map_err(|_| INVALID)?;
                for op in parse_content_stream(&stream).map_err(|_| INVALID)? {
                    let referenced = match op {
                        Operator::Do { name } => objects.get(&name),
                        Operator::SetFillColorN {
                            name: Some(name), ..
                        }
                        | Operator::SetStrokeColorN {
                            name: Some(name), ..
                        } => patterns.get(name.as_str()),
                        Operator::PaintShading { name } => shadings.get(&name),
                        _ => None,
                    };
                    pending.extend(
                        referenced
                            .and_then(Object::as_reference)
                            .map(|reference| reference.id),
                    );
                }
            }
        }
        Ok(painted)
    }

    fn finish(self, info: Option<u32>) -> Result<Vec<u8>> {
        self.finish_with_trailer(info, (1, 7), None)
    }

    fn finish_with_trailer(
        self,
        info: Option<u32>,
        version: (u8, u8),
        identifiers: Option<Object>,
    ) -> Result<Vec<u8>> {
        let reachable = self.reachable(info)?;
        if !reachable.is_disjoint(&self.replaced_visual_objects) {
            let painted = self.painted_resource_closure()?;
            if self
                .replaced_visual_objects
                .iter()
                .any(|id| reachable.contains(id) && !painted.contains(id))
            {
                return Err("A source visual stream remains reachable outside a painted placement");
            }
        }
        if !reachable.is_disjoint(&self.replaced_content_objects) {
            return Err("A source content stream is shared by another PDF resource and cannot be redacted safely");
        }
        let serializer = ObjectSerializer::compact();
        let (major, minor) = version;
        let mut output = format!("%PDF-{major}.{minor}\n").into_bytes();
        output.extend_from_slice(b"%\xe2\xe3\xcf\xd3\n");
        let mut offsets = Vec::with_capacity(self.objects.len());
        for (index, object) in self.objects.into_iter().enumerate() {
            let id = index as u32 + 1;
            let object = if reachable.contains(&id) {
                object
            } else {
                Object::Null
            };
            offsets.push(output.len());
            let encoded = serializer.serialize_indirect(id, 0, &object);
            if output.len().saturating_add(encoded.len()) > MAX_OUTPUT {
                return Err(COMPLEX);
            }
            output.extend(encoded);
        }
        let xref = output.len();
        write!(
            output,
            "xref\n0 {}\n0000000000 65535 f \n",
            offsets.len() + 1
        )
        .map_err(|_| INVALID)?;
        for offset in &offsets {
            writeln!(output, "{offset:010} 00000 n ").map_err(|_| INVALID)?;
        }
        let mut trailer = dict([
            ("Root", reference(1)),
            ("Size", Object::Integer(offsets.len() as i64 + 1)),
        ]);
        if let Some(info) = info {
            trailer.insert("Info".into(), reference(info));
        }
        if let Some(identifiers) = identifiers {
            trailer.insert("ID".into(), identifiers);
        }
        output.extend_from_slice(b"trailer\n");
        output.extend(serializer.serialize(&Object::Dictionary(trailer)));
        write!(output, "\nstartxref\n{xref}\n%%EOF\n").map_err(|_| INVALID)?;
        Ok(output)
    }
}

/// Assemble fresh page copies with navigation and forms from the sources.
pub(in crate::matters) fn assemble(
    first: Vec<u8>,
    second: Option<Vec<u8>>,
    selections: &[(usize, usize)],
) -> Result<Vec<u8>> {
    let mut sources = vec![Source::open(first)?];
    if let Some(second) = second {
        sources.push(Source::open(second)?);
    }
    assemble_sources(&sources, selections)
}

pub(in crate::matters) fn assemble_sources(
    sources: &[Source],
    selections: &[(usize, usize)],
) -> Result<Vec<u8>> {
    assemble_with_options(
        sources,
        selections,
        false,
        &HashMap::new(),
        None,
        None,
        None,
    )
}

pub(in crate::matters) fn delete_pages(
    source: Vec<u8>,
    removed: &std::collections::BTreeSet<usize>,
) -> Result<Vec<u8>> {
    deletion::write(source, removed)
}

pub(super) fn set_page_geometry(
    pdf: PdfDocument,
    rotations: Vec<(usize, i32)>,
    crop_boxes: Vec<(usize, [f32; 4])>,
) -> Result<Vec<u8>> {
    let source = Source::from_document(pdf)?;
    let mut writer = copy_catalog(&source, None)?;
    writer.omit_null_page_attributes(&source)?;
    let settings = rotations
        .into_iter()
        .map(|(page, degrees)| (page, "Rotate", Object::Integer(i64::from(degrees))))
        .chain(crop_boxes.into_iter().map(|(page, bounds)| {
            let bounds = bounds
                .into_iter()
                .map(|value| Object::Real(f64::from(value)))
                .collect();
            (page, "CropBox", Object::Array(bounds))
        }));
    for (page, key, value) in settings {
        let original = source.pages.get(page).ok_or(INVALID)?;
        let id = *writer.imported.get(&(0, *original)).ok_or(INVALID)?;
        let Object::Dictionary(fields) = &mut writer.objects[id as usize - 1] else {
            return Err(INVALID);
        };
        fields.insert(key.into(), value);
    }
    finish_catalog(writer, &source)
}

pub(in crate::matters) fn page_for_vector_analysis(
    bytes: &[u8],
    page: usize,
    content: Vec<u8>,
) -> Result<Vec<u8>> {
    let source = Source::open(bytes.to_vec())?;
    let overlay = Overlay {
        replace_page_content: Some(content),
        retain_xobjects: Some(HashSet::new()),
        redact_content: true,
        ..Default::default()
    };
    assemble_with_options(
        &[source],
        &[(0, page)],
        false,
        &HashMap::from([(0, overlay)]),
        Some(None),
        None,
        None,
    )
}

fn lossless_stream_key(value: &Object) -> Option<[u8; 32]> {
    use sha2::{Digest as _, Sha256};
    let Object::Stream { dict, data } = value else {
        return None;
    };
    let image = dict.get("Subtype").and_then(Object::as_name) == Some("Image")
        && !dict.keys().any(|key| {
            matches!(
                key.as_str(),
                "StructParent" | "Metadata" | "OC" | "Alternates"
            )
        });
    let plain = dict
        .keys()
        .all(|key| matches!(key.as_str(), "Length" | "Filter" | "DecodeParms"));
    if !image && !plain {
        return None;
    }
    let mut fields = dict.clone();
    fields.remove("Length");
    let header = ObjectSerializer::compact().serialize(&Object::Dictionary(fields));
    let mut digest = Sha256::new();
    digest.update(header.len().to_be_bytes());
    digest.update(header);
    digest.update(data);
    Some(digest.finalize().into())
}

fn extend_shaped_codes(keep: &mut HashSet<u16>, bytes: &[u8], width: usize) -> Result<()> {
    let codes = bytes.chunks_exact(width);
    if !codes.remainder().is_empty() {
        return Err(INVALID);
    }
    for code in codes {
        keep.insert(if width == 1 {
            u16::from(code[0])
        } else {
            u16::from_be_bytes([code[0], code[1]])
        });
    }
    Ok(())
}

fn prune_shaped_cmap(data: &[u8], keep: &HashSet<u16>) -> Result<String> {
    use std::fmt::Write as _;
    let data = std::str::from_utf8(data).map_err(|_| INVALID)?;
    let (header, body) = data.split_once("endcodespacerange\n").ok_or(INVALID)?;
    let (entries, footer) = body.split_once("endcmap\n").ok_or(INVALID)?;
    let mut retained = Vec::new();
    for line in entries.lines().filter(|line| line.starts_with('<')) {
        let code = line
            .strip_prefix('<')
            .and_then(|line| line.split_once('>'))
            .ok_or(INVALID)?
            .0;
        let code = u16::from_str_radix(code, 16).map_err(|_| INVALID)?;
        if keep.contains(&code) {
            retained.push(line);
        }
    }
    let mut output = format!("{header}endcodespacerange\n");
    for chunk in retained.chunks(100) {
        writeln!(output, "{} beginbfchar", chunk.len()).map_err(|_| INVALID)?;
        for line in chunk {
            writeln!(output, "{line}").map_err(|_| INVALID)?;
        }
        output.push_str("endbfchar\n");
    }
    write!(output, "endcmap\n{footer}").map_err(|_| INVALID)?;
    Ok(output)
}

pub(in crate::matters) fn form_transform([a, b, c, d, e, f]: [f32; 6]) -> Vec<u8> {
    format!("q {a:.9} {b:.9} {c:.9} {d:.9} {e:.9} {f:.9} cm\n").into_bytes()
}

pub(in crate::matters) fn text_geometry_document(
    source: &PdfDocument,
    page: usize,
    data: Vec<u8>,
    forms: &[(String, EditedForm)],
) -> Result<(PdfDocument, HashMap<u32, ObjectRef>)> {
    let geometry = geometry(source, page)?;
    let resources = source.get_page_resources(page).map_err(|_| INVALID)?;
    let mut writer = Writer::isolated();
    let resources = writer.copy(source, 0, resources, 0)?;
    let mut resources = writer.reference_dict(Some(&resources))?;
    if !forms.is_empty() {
        let mut xobjects = writer.reference_dict(resources.get("XObject"))?;
        for (key, form) in forms {
            let id = writer.copy_edited_form(source, 0, form)?;
            xobjects.insert(key.clone(), reference(id));
        }
        resources.insert("XObject".into(), Object::Dictionary(xobjects));
    }
    writer.isolated_page(Object::Dictionary(resources), data, geometry)?;
    let mut origins = writer
        .imported
        .iter()
        .map(|(&(_, source), &copy)| (copy, source))
        .collect::<HashMap<_, _>>();
    origins.extend(
        writer
            .form_copies
            .iter()
            .map(|&(source, copy)| (copy, source)),
    );
    let pdf = PdfDocument::from_bytes(writer.finish(None)?).map_err(|_| INVALID)?;
    Ok((pdf, origins))
}

pub(in crate::matters) fn form_document(
    bytes: &[u8],
    form: &Object,
    parent_resources: &Object,
    bounds: [f64; 4],
    transform: [f32; 6],
) -> Result<Vec<u8>> {
    let source = PdfDocument::from_bytes(bytes.to_vec()).map_err(|_| INVALID)?;
    let mut writer = Writer::isolated();
    let fields = form.as_dict().ok_or(INVALID)?;
    let resources = fields.get("Resources").unwrap_or(parent_resources);
    let resources = writer.copy(&source, 0, resources.clone(), 0)?;
    let stream = form.decode_stream_data().map_err(|_| INVALID)?;
    let mut body = form_transform(transform);
    body.extend(stream);
    body.extend_from_slice(b"\nQ\n");
    writer.isolated_page(
        resources,
        body,
        Geometry {
            media: bounds,
            crop: bounds,
            rotation: 0,
        },
    )?;
    writer.finish(None)
}

pub(in crate::matters) fn overlay_document(
    bytes: Vec<u8>,
    overlays: &HashMap<usize, Overlay>,
) -> Result<Vec<u8>> {
    let source = Source::open(bytes)?;
    let selection = (0..source.pages.len())
        .map(|page| (0, page))
        .collect::<Vec<_>>();
    assemble_with_options(&[source], &selection, true, overlays, None, None, None)
}

/// Append marks to existing pages without reassembling navigation, structure,
/// annotations or the page tree. A text edit can replace the selected page's
/// content stream. An OCR rerun takes its content and resources from a retained
/// revision before adding the layer.
pub(in crate::matters) fn append_document_overlays(
    bytes: Vec<u8>,
    overlays: &HashMap<usize, Overlay>,
    retained: Option<(Vec<u8>, usize)>,
) -> Result<Vec<u8>> {
    append_overlays(bytes, overlays, retained, None).map(|(data, _)| data)
}

pub(in crate::matters) fn remove_text_for_replacement(
    bytes: Vec<u8>,
    page: usize,
    overlay: Overlay,
    tag: Option<ReplacementTag>,
) -> Result<(Vec<u8>, Option<ReplacementTag>)> {
    append_overlays(bytes, &HashMap::from([(page, overlay)]), None, tag)
}

pub(in crate::matters) fn rebind_text_page(
    pdf: PdfDocument,
    page_index: usize,
    page: ReboundTextStream,
    forms: Vec<ReboundTextForm>,
) -> Result<Vec<u8>> {
    fn resources(
        writer: &mut Writer<'_>,
        source: &Source,
        resources: Object,
        children: &[(String, usize)],
        written: &[u32],
    ) -> Result<Object> {
        let resources = writer.copy(source, 0, resources, 0)?;
        if children.is_empty() {
            return Ok(resources);
        }
        let mut resources = writer.reference_dict(Some(&resources))?;
        let mut xobjects = writer.reference_dict(resources.get("XObject"))?;
        for (name, index) in children {
            let binding = reference(*written.get(*index).ok_or(INVALID)?);
            if xobjects.insert(name.clone(), binding).is_some() {
                return Err(INVALID);
            }
        }
        resources.insert("XObject".into(), Object::Dictionary(xobjects));
        Ok(Object::Dictionary(resources))
    }

    let source = Source::from_document(pdf)?;
    let mut writer = copy_catalog(&source, None)?;
    let original = *source.pages.get(page_index).ok_or(INVALID)?;
    let output = *writer.imported.get(&(0, original)).ok_or(INVALID)?;
    let mut written = Vec::with_capacity(forms.len());
    // Child bindings are written before their parent, once per effective font
    // and resource context. Repeated page invocations can share the same copy.
    for form in forms {
        let local = resources(
            &mut writer,
            &source,
            form.stream.resources,
            &form.stream.forms,
            &written,
        )?;
        let mut fields = writer.copy_dict(&source, 0, form.fields, 0)?;
        fields.insert("Resources".into(), local);
        let copy = writer.add(Object::Stream {
            dict: fields,
            data: form.stream.data.into(),
        })?;
        writer.form_copies.push((form.original, copy));
        written.push(copy);
    }
    let mut fields = writer.objects[output as usize - 1]
        .as_dict()
        .cloned()
        .ok_or(INVALID)?;
    let local = resources(&mut writer, &source, page.resources, &page.forms, &written)?;
    let stream = Object::Stream {
        dict: Dictionary::new(),
        data: page.data.into(),
    };
    fields.insert("Contents".into(), reference(writer.add(stream)?));
    fields.insert("Resources".into(), local);
    writer.set(output, Object::Dictionary(fields));
    writer.retarget_rebound_form_tags(output)?;
    finish_catalog(writer, &source)
}

fn append_overlays(
    bytes: Vec<u8>,
    overlays: &HashMap<usize, Overlay>,
    retained: Option<(Vec<u8>, usize)>,
    selected_tag: Option<ReplacementTag>,
) -> Result<(Vec<u8>, Option<ReplacementTag>)> {
    let source = Source::open(bytes)?;
    let retained = retained
        .map(|(bytes, page)| Source::open(bytes).map(|source| (source, page)))
        .transpose()?;
    if let Some((prior, page)) = &retained {
        let previous = prior.geometry(*page)?;
        let current = source.geometry(*page)?;
        if current.crop != previous.crop || current.rotation != previous.rotation {
            return Err("Page size or rotation changed; this OCR page cannot be safely rerun");
        }
    }
    let mut writer = copy_catalog(&source, None)?;
    let mut indexes = overlays.keys().copied().collect::<Vec<_>>();
    indexes.sort_unstable();
    let mut replacement_tag = selected_tag.filter(|tag| tag.from_page);
    for index in indexes {
        let overlay = &overlays[&index];
        let original = *source.pages.get(index).ok_or(INVALID)?;
        let output = *writer.imported.get(&(0, original)).ok_or(INVALID)?;
        let mut page = writer.objects[output as usize - 1]
            .as_dict()
            .cloned()
            .ok_or(INVALID)?;
        if !overlay.omit_annotations.is_empty() {
            let entries = match page.get("Annots") {
                Some(Object::Reference(reference)) => writer.objects.get(reference.id as usize - 1),
                entries => entries,
            }
            .and_then(Object::as_array)
            .ok_or(INVALID)?;
            let kept = entries
                .iter()
                .enumerate()
                .filter(|(index, _)| overlay.omit_annotations.binary_search(index).is_err())
                .map(|(_, entry)| entry.clone())
                .collect::<Vec<_>>();
            if kept.is_empty() {
                page.remove("Annots");
            } else {
                page.insert("Annots".into(), Object::Array(kept));
            }
        }
        let (content_source, source_index) = match &retained {
            Some((prior, selected)) if index == *selected => (prior, 1),
            _ => (&source, 0),
        };
        let content = content_source
            .pdf
            .get_page_for_debug(index)
            .map_err(|_| INVALID)?;
        let content = content.as_dict().ok_or(INVALID)?;
        for key in ["Contents", "Resources"] {
            if let Some(value) = content.get(key) {
                let value = writer.copy(content_source, source_index, value.clone(), 0)?;
                page.insert(key.into(), value);
            } else {
                page.remove(key);
            }
        }
        if let Some(data) = &overlay.replace_page_content {
            let content = writer.add(Object::Stream {
                dict: Dictionary::new(),
                data: data.clone().into(),
            })?;
            page.insert("Contents".into(), reference(content));
        }
        writer.append_overlay(
            &mut page,
            &source,
            &Page {
                source: 0,
                page: index,
                output,
            },
            overlay,
            overlay.ocr.then_some(OCR_STREAM),
        )?;
        writer.set(output, Object::Dictionary(page));
        replacement_tag = writer
            .retarget_form_tags(output, selected_tag)?
            .or(replacement_tag);
    }
    Ok((finish_catalog(writer, &source)?, replacement_tag))
}

pub(in crate::matters) fn without_generated_page_number(
    bytes: Vec<u8>,
    page: usize,
) -> Result<PdfDocument> {
    let source = Source::open(bytes)?;
    let overlays = HashMap::from([(
        0,
        Overlay {
            remove_page_number: true,
            ..Default::default()
        },
    )]);
    let bytes = assemble_with_options(
        &[source],
        &[(0, page)],
        false,
        &overlays,
        Some(None),
        None,
        None,
    )?;
    PdfDocument::from_bytes(bytes).map_err(|_| INVALID)
}

pub(in crate::matters) fn clean_redacted_document(bytes: Vec<u8>) -> Result<Vec<u8>> {
    let source = Source::open(bytes)?;
    let pages = (0..source.pages.len())
        .map(|page| (0, page))
        .collect::<Vec<_>>();
    let overlays = pages
        .iter()
        .map(|&(_, page)| {
            (
                page,
                Overlay {
                    strip_page_metadata: true,
                    ..Default::default()
                },
            )
        })
        .collect();
    assemble_with_options(
        &[source],
        &pages,
        false,
        &overlays,
        Some(None),
        Some(&Navigation {
            cleanup_metadata: true,
            ..Default::default()
        }),
        None,
    )
}

pub(in crate::matters) fn print_document(
    bytes: Vec<u8>,
    pages: &[usize],
    paper: Option<Paper>,
) -> Result<Vec<u8>> {
    let source = Source::open(bytes)?;
    let selection = pages.iter().map(|&page| (0, page)).collect::<Vec<_>>();
    assemble_with_options(
        &[source],
        &selection,
        false,
        &HashMap::new(),
        Some(paper),
        None,
        None,
    )
}

pub(in crate::matters) fn assemble_bundle(
    sources: &[Source],
    selections: &[(usize, usize)],
    overlays: &HashMap<usize, Overlay>,
    navigation: &Navigation,
) -> Result<Vec<u8>> {
    assemble_with_options(
        sources,
        selections,
        false,
        overlays,
        None,
        Some(navigation),
        None,
    )
}

pub(in crate::matters) fn blank(size: (f32, f32)) -> Result<Vec<u8>> {
    use pdf_oxide::writer::{DocumentBuilder, PageSize};
    let mut builder = DocumentBuilder::new();
    builder.page(PageSize::Custom(size.0, size.1)).done();
    builder.build().map_err(|_| INVALID)
}

pub(in crate::matters) fn compress_document(
    bytes: Vec<u8>,
    images: &mut crate::matters::compression::Images,
) -> Result<Vec<u8>> {
    let source = Source::open(bytes)?;
    let writer = copy_catalog(&source, Some(images))?;
    finish_catalog(writer, &source)
}

fn copy_catalog<'a>(
    source: &Source,
    compression: Option<&'a mut crate::matters::compression::Images>,
) -> Result<Writer<'a>> {
    let trailer = source.pdf.trailer().as_dict().ok_or(INVALID)?;
    let root = trailer
        .get("Root")
        .and_then(Object::as_reference)
        .ok_or(INVALID)?;
    let mut writer = Writer {
        preserve_page_tree: true,
        compression,
        compressed_streams: HashMap::new(),
        compression_srgb: None,
        strip_resource_metadata: false,
        replaced_visual_objects: HashSet::new(),
        painted_visual_objects: HashSet::new(),
        replaced_content_objects: HashSet::new(),
        objects: vec![Object::Null],
        imported: HashMap::from([((0, root), 1)]),
        first: HashMap::new(),
        form_copies: Vec::new(),
    };
    let catalog = writer.copy(source, 0, source.catalog.clone(), 0)?;
    writer.set(1, catalog);
    Ok(writer)
}

fn finish_catalog(mut writer: Writer<'_>, source: &Source) -> Result<Vec<u8>> {
    let trailer = source.pdf.trailer().as_dict().ok_or(INVALID)?;
    let info = trailer
        .get("Info")
        .map(|info| writer.copy(source, 0, info.clone(), 0))
        .transpose()?
        .map(|info| match info.as_reference() {
            Some(info) => Ok(info.id),
            None => writer.add(info),
        })
        .transpose()?;
    let identifiers = trailer.get("ID").map(|id| source.resolve(id)).transpose()?;
    if identifiers.as_ref().is_some_and(|id| {
        !id.as_array()
            .is_some_and(|items| items.iter().all(|item| item.as_string().is_some()))
    }) {
        return Err(INVALID);
    }
    writer.finish_with_trailer(info, source.pdf.version().max((1, 2)), identifiers)
}

fn assemble_with_options(
    sources: &[Source],
    selections: &[(usize, usize)],
    preserve_context: bool,
    overlays: &HashMap<usize, Overlay>,
    print: Option<Option<Paper>>,
    navigation: Option<&Navigation>,
    compression: Option<&mut crate::matters::compression::Images>,
) -> Result<Vec<u8>> {
    if selections.is_empty() || selections.len() > MAX_PAGES {
        return Err(COMPLEX);
    }
    let mut writer = Writer {
        preserve_page_tree: false,
        compression,
        compressed_streams: HashMap::new(),
        compression_srgb: None,
        strip_resource_metadata: overlays.values().any(|overlay| overlay.strip_page_metadata),
        replaced_visual_objects: HashSet::new(),
        painted_visual_objects: HashSet::new(),
        replaced_content_objects: HashSet::new(),
        objects: vec![Object::Null, Object::Null],
        imported: HashMap::new(),
        first: HashMap::new(),
        form_copies: Vec::new(),
    };
    let mut pages = Vec::new();
    let mut source_order = Vec::new();
    let mut forms =
        (!preserve_context && print.is_none() && navigation.is_none() && overlays.is_empty())
            .then(forms::Forms::default);
    for &(source, page) in selections {
        if sources
            .get(source)
            .is_none_or(|pdf| page >= pdf.pages.len())
        {
            return Err(INVALID);
        }
        let output = writer.reserve()?;
        writer.first.entry((source, page)).or_insert(output);
        if !source_order.contains(&source) {
            source_order.push(source);
        }
        pages.push(Page {
            source,
            page,
            output,
        });
    }
    for (index, page) in pages.iter().enumerate() {
        writer.page(
            &sources[page.source],
            page,
            overlays.get(&index),
            if navigation.is_some() {
                Some(None)
            } else {
                print
            },
            forms.as_mut(),
        )?;
    }
    writer.set(
        2,
        Object::Dictionary(dict([
            ("Type", name("Pages")),
            ("Count", Object::Integer(pages.len() as i64)),
            (
                "Kids",
                Object::Array(pages.iter().map(|page| reference(page.output)).collect()),
            ),
        ])),
    );
    let first = source_order[0];
    let source = &sources[first];
    let mut catalog = if preserve_context {
        let entries = source
            .catalog
            .as_dict()
            .ok_or(INVALID)?
            .iter()
            .filter(|(key, _)| {
                !["Pages", "Outlines", "Dests", "OpenAction", "Perms"].contains(&key.as_str())
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        writer.copy_dict(source, first, entries, 0)?
    } else {
        dict([("Type", name("Catalog"))])
    };
    catalog.insert("Pages".into(), reference(2));
    if let Some(forms) = forms {
        forms.install(&mut writer, &mut catalog)?;
    }
    if let Some(navigation) = navigation {
        writer.generated_navigation(&mut catalog, sources, &pages, navigation)?;
    } else if print.is_none() {
        if let Some(outline) = writer.outline(sources, &source_order, preserve_context)? {
            catalog.insert("Outlines".into(), reference(outline));
        }
    }
    writer.set(1, Object::Dictionary(catalog));
    let generated_title = navigation
        .filter(|nav| !nav.cleanup_metadata)
        .and_then(|nav| nav.title.as_deref());
    let info = if let Some(title) = generated_title {
        Some(writer.add(Object::Dictionary(dict([
            ("Title", pdf_text(title)),
            (
                "Subject",
                pdf_text("DRAFT MatterDraft Studio - not court certified"),
            ),
            ("Creator", pdf_text("MatterDraft Studio")),
        ])))?)
    } else if let Some(value) = source
        .pdf
        .trailer()
        .as_dict()
        .and_then(|trailer| trailer.get("Info"))
        .filter(|_| navigation.is_none_or(|nav| !nav.cleanup_metadata))
    {
        let value = source.resolve(value)?;
        let metadata = value
            .as_dict()
            .ok_or(INVALID)?
            .iter()
            .filter(|(key, _)| {
                preserve_context
                    || [
                        "Title",
                        "Author",
                        "Subject",
                        "Keywords",
                        "Creator",
                        "Producer",
                        "CreationDate",
                        "ModDate",
                        "Trapped",
                    ]
                    .contains(&key.as_str())
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let metadata = writer.copy_dict(source, first, metadata, 0)?;
        Some(writer.add(Object::Dictionary(metadata))?)
    } else {
        None
    };
    writer.finish(info)
}
