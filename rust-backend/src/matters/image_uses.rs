//! Resolve PDF image resources and authored placements without decoding pixels.
use super::{document_output::form_geometry, page_tools::assembly::Dictionary};
use pdf_oxide::{
    content::{parse_content_stream, parse_content_stream_images_only, Matrix, Operator},
    object::{Object, ObjectRef},
    optional_content::{
        check_ocg_excluded, compute_default_off_ocgs, resolve_and_check_ocg_excluded,
    },
    PdfDocument,
};
use std::collections::{HashMap, HashSet};

const INVALID: &str = "Cannot inspect the PDF's image resources";
const MAX_FORM_DEPTH: usize = 32;
const MAX_RESOURCES: usize = 100_000;
const MAX_OPERATORS: usize = 1_000_000;
const MAX_DECODED: usize = 64 * 1024 * 1024;

pub(super) struct ImageResources {
    pub images: Vec<ImageResource>,
    pub has_images: bool,
}

#[derive(Clone, PartialEq)]
pub(super) struct ImageResource {
    pub reference: ObjectRef,
    pub colors: ImageColors,
}

#[derive(Clone, Default, PartialEq)]
pub(super) struct ImageColors {
    gray: Option<Object>,
    rgb: Option<Object>,
    cmyk: Option<Object>,
}

impl ImageColors {
    fn from_resources(pdf: &PdfDocument, resources: &Object) -> Self {
        let Some(spaces) = color_spaces(pdf, resources) else {
            return Self::default();
        };
        let device = |name| spaces.as_dict()?.get(name).cloned();
        Self {
            gray: device("DefaultGray"),
            rgb: device("DefaultRGB"),
            cmyk: device("DefaultCMYK"),
        }
    }

    pub(super) fn resolve(&self, pdf: &PdfDocument, color: &Object) -> Option<Object> {
        let color = pdf.resolve_object(color).ok()?;
        match color {
            Object::Name(ref name) => {
                let alternate = match name.as_str() {
                    "DeviceGray" => self.gray.as_ref(),
                    "DeviceRGB" => self.rgb.as_ref(),
                    "DeviceCMYK" => self.cmyk.as_ref(),
                    _ => return None,
                };
                match alternate
                    .map(|value| pdf.resolve_object(value))
                    .transpose()
                    .ok()?
                {
                    Some(Object::Null) | None => Some(color),
                    Some(profile) => Some(profile),
                }
            }
            Object::Array(mut parts) => {
                let base = match parts.first()?.as_name()? {
                    "Indexed" => Some(1),
                    "Separation" | "DeviceN" => Some(2),
                    _ => None,
                };
                if let Some(base) = base {
                    // The underlying space of a special color space is also
                    // subject to device defaults. XObject entries are explicit.
                    let value = pdf.resolve_object(parts.get(base)?).ok()?;
                    if let Object::Name(_) = value {
                        parts[base] = self.resolve(pdf, &value)?;
                    }
                }
                Some(Object::Array(parts))
            }
            _ => None,
        }
    }

    pub(super) fn remaps_device(&self, pdf: &PdfDocument, name: &str) -> bool {
        let device = Object::Name(name.to_owned());
        self.resolve(pdf, &device)
            .is_none_or(|resolved| resolved != device)
    }
}

fn color_spaces(pdf: &PdfDocument, resources: &Object) -> Option<Object> {
    pdf.resolve_object(resources.as_dict()?.get("ColorSpace")?)
        .ok()
}

pub(super) enum ImagePaint {
    Resource(ImageResource),
    Inline {
        fields: Dictionary,
        data: Vec<u8>,
        colors: ImageColors,
    },
}

pub(super) struct ImagePlacement {
    pub paint: ImagePaint,
    pub size: [f64; 2],
}

fn xobjects(pdf: &PdfDocument, resources: &Object) -> Result<Dictionary, &'static str> {
    let Some(binding) = resources.as_dict().and_then(|dict| dict.get("XObject")) else {
        return Ok(Dictionary::new());
    };
    let value = pdf.resolve_object(binding).map_err(|_| INVALID)?;
    Ok(value.as_dict().cloned().unwrap_or_default())
}

pub(super) fn resources(pdf: &PdfDocument, page: usize) -> Result<ImageResources, &'static str> {
    let mut pending = vec![pdf.get_page_resources(page).map_err(|_| INVALID)?];
    let mut seen_forms = HashSet::new();
    let mut seen_images = HashMap::<ObjectRef, Vec<ImageColors>>::new();
    let mut images = ImageResources {
        images: Vec::new(),
        has_images: false,
    };
    let mut inspected = 0;
    while let Some(resources) = pending.pop() {
        let colors = ImageColors::from_resources(pdf, &resources);
        let mut entries = xobjects(pdf, &resources)?.into_iter().collect::<Vec<_>>();
        entries.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
        for (_, binding) in entries {
            inspected += 1;
            if inspected > MAX_RESOURCES {
                return Err("Page image resources exceed the inspection limit");
            }
            let reference = binding.as_reference();
            let object = pdf.resolve_object(&binding).map_err(|_| INVALID)?;
            let Some(fields) = object.as_dict() else {
                continue;
            };
            match fields.get("Subtype").and_then(Object::as_name) {
                Some("Image") => {
                    images.has_images = true;
                    if let Some(reference) = reference {
                        let scopes = seen_images.entry(reference).or_default();
                        if !scopes.contains(&colors) {
                            scopes.push(colors.clone());
                            images.images.push(ImageResource {
                                reference,
                                colors: colors.clone(),
                            });
                        }
                    }
                }
                Some("Form") => {
                    if reference.is_some_and(|id| !seen_forms.insert(id)) {
                        continue;
                    }
                    if let Some(local) = fields.get("Resources") {
                        pending.push(pdf.resolve_object(local).map_err(|_| INVALID)?);
                    }
                }
                _ => {}
            }
        }
    }
    Ok(images)
}

pub(super) fn painted(
    pdf: &PdfDocument,
    page: usize,
    mut visit: impl FnMut(ImagePlacement) -> bool,
) -> Result<(), &'static str> {
    let resources = pdf.get_page_resources(page).map_err(|_| INVALID)?;
    let data = pdf.get_page_content_data(page).map_err(|_| INVALID)?;
    let mut walk = Painting {
        pdf,
        visit: &mut visit,
        excluded: compute_default_off_ocgs(pdf),
        active: HashSet::new(),
        decoded: 0,
        operators: 0,
    };
    walk.stream(&data, &resources, Matrix::identity(), 0)?;
    Ok(())
}

struct Painting<'a, F> {
    pdf: &'a PdfDocument,
    visit: &'a mut F,
    excluded: HashSet<String>,
    active: HashSet<ObjectRef>,
    decoded: usize,
    operators: usize,
}

impl<F: FnMut(ImagePlacement) -> bool> Painting<'_, F> {
    fn show(&mut self, paint: ImagePaint, matrix: Matrix) -> bool {
        // The reference reports the entire image rectangle, even when the
        // painted result is clipped or lies outside the visible page.
        let [a, b, c, d] = [matrix.a, matrix.b, matrix.c, matrix.d].map(f64::from);
        let size = [a.abs() + c.abs(), b.abs() + d.abs()];
        size.iter().all(|value| value.is_finite()) && (self.visit)(ImagePlacement { paint, size })
    }

    fn excluded(&self, fields: &Dictionary) -> bool {
        !self.excluded.is_empty()
            && fields
                .get("OC")
                .and_then(|value| self.pdf.resolve_object(value).ok())
                .and_then(|object| object.as_dict().cloned())
                .is_some_and(|props| check_ocg_excluded(&props, self.pdf, &self.excluded))
    }

    fn stream(
        &mut self,
        data: &[u8],
        resources: &Object,
        mut matrix: Matrix,
        depth: usize,
    ) -> Result<bool, &'static str> {
        self.decoded = self.decoded.checked_add(data.len()).ok_or(INVALID)?;
        if self.decoded > MAX_DECODED || depth > MAX_FORM_DEPTH {
            return Err("Page image placements exceed the inspection limit");
        }
        let operations = if self.excluded.is_empty() {
            parse_content_stream_images_only(data)
        } else {
            parse_content_stream(data)
        }
        .map_err(|_| INVALID)?;
        self.operators += operations.len();
        if self.operators > MAX_OPERATORS {
            return Err("Page image placements exceed the inspection limit");
        }
        let xobjects = xobjects(self.pdf, resources)?;
        let colors = ImageColors::from_resources(self.pdf, resources);
        let mut graphics = Vec::new();
        let mut marks = Vec::new();
        let mut excluded = false;
        for op in operations {
            match op {
                Operator::SaveState => graphics.push(matrix),
                Operator::RestoreState => {
                    if let Some(saved) = graphics.pop() {
                        matrix = saved;
                    }
                }
                Operator::Cm { a, b, c, d, e, f } => {
                    matrix = Matrix { a, b, c, d, e, f }.multiply(&matrix);
                }
                Operator::BeginMarkedContentDict { tag, properties } => {
                    marks.push(excluded);
                    excluded |= tag == "OC"
                        && resolve_and_check_ocg_excluded(
                            &properties,
                            Some(resources),
                            Some(self.pdf),
                            &self.excluded,
                        );
                }
                Operator::BeginMarkedContent { .. } => marks.push(excluded),
                Operator::EndMarkedContent => excluded = marks.pop().unwrap_or(false),
                Operator::InlineImage { dict, data } if !excluded => {
                    if self.show(
                        ImagePaint::Inline {
                            fields: inline_fields(*dict, self.pdf, resources),
                            data,
                            colors: colors.clone(),
                        },
                        matrix,
                    ) {
                        return Ok(true);
                    }
                }
                Operator::Do { name } if !excluded => {
                    let Some(binding) = xobjects.get(&name) else {
                        continue;
                    };
                    let Some(reference) = binding.as_reference() else {
                        continue;
                    };
                    let object = self.pdf.resolve_object(binding).map_err(|_| INVALID)?;
                    let Some(fields) = object.as_dict() else {
                        continue;
                    };
                    if self.excluded(fields) {
                        continue;
                    }
                    match fields.get("Subtype").and_then(Object::as_name) {
                        Some("Image") => {
                            if self.show(
                                ImagePaint::Resource(ImageResource {
                                    reference,
                                    colors: colors.clone(),
                                }),
                                matrix,
                            ) {
                                return Ok(true);
                            }
                        }
                        Some("Form") if self.active.insert(reference) => {
                            let local = fields
                                .get("Resources")
                                .map(|binding| self.pdf.resolve_object(binding))
                                .transpose()
                                .map_err(|_| INVALID)?;
                            let local = local
                                .as_ref()
                                .filter(|value| !matches!(value, Object::Null))
                                .unwrap_or(resources);
                            let (_, placement) = form_geometry(self.pdf, fields, matrix)?;
                            let body = object.decode_stream_data().map_err(|_| INVALID)?;
                            let found = self.stream(&body, local, placement, depth + 1)?;
                            self.active.remove(&reference);
                            if found {
                                return Ok(true);
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        Ok(false)
    }
}

fn inline_fields(dict: Dictionary, pdf: &PdfDocument, resources: &Object) -> Dictionary {
    let mut fields = Dictionary::new();
    let map_name = |value: Object, aliases: &[(&str, &str)]| {
        value
            .as_name()
            .and_then(|name| aliases.iter().find(|(short, _)| *short == name))
            .map_or(value, |(_, full)| Object::Name((*full).into()))
    };
    for (key, value) in dict {
        let key = match key.as_str() {
            "W" => "Width",
            "H" => "Height",
            "BPC" => "BitsPerComponent",
            "CS" => "ColorSpace",
            "D" => "Decode",
            "DP" => "DecodeParms",
            "F" => "Filter",
            "IM" => "ImageMask",
            other => other,
        };
        let value = match key {
            "Filter" => {
                let filter = |value| {
                    map_name(
                        value,
                        &[
                            ("AHx", "ASCIIHexDecode"),
                            ("A85", "ASCII85Decode"),
                            ("LZW", "LZWDecode"),
                            ("Fl", "FlateDecode"),
                            ("RL", "RunLengthDecode"),
                            ("CCF", "CCITTFaxDecode"),
                            ("DCT", "DCTDecode"),
                        ],
                    )
                };
                match value {
                    Object::Array(items) => Object::Array(items.into_iter().map(filter).collect()),
                    value => filter(value),
                }
            }
            "ColorSpace" => {
                let value = map_name(
                    value,
                    &[
                        ("G", "DeviceGray"),
                        ("RGB", "DeviceRGB"),
                        ("CMYK", "DeviceCMYK"),
                        ("I", "Indexed"),
                    ],
                );
                let source = value.as_name().and_then(|name| {
                    let colors = color_spaces(pdf, resources)?;
                    colors.as_dict()?.get(name).cloned()
                });
                source.unwrap_or(value)
            }
            _ => value,
        };
        fields.insert(key.to_owned(), value);
    }
    fields.insert("Subtype".into(), Object::Name("Image".into()));
    fields
}

pub(crate) fn has_painted_image(pdf: &PdfDocument, page: usize) -> Result<bool, &'static str> {
    let mut found = false;
    match painted(pdf, page, |_| {
        found = true;
        true
    }) {
        Ok(()) => Ok(found),
        Err(_) => pdf
            .page_image_handles(page)
            .map(|images| !images.is_empty())
            .map_err(|_| INVALID),
    }
}
