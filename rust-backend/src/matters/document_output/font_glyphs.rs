//! Resolve outline geometry only when the source encoding identifies the glyphs.
use super::base14_bounds::Font as Base14;
use super::source_codes::{CidEncoding, Code, Codes};
use super::type3_paint;
use pdf_oxide::{
    fonts::FontInfo,
    object::{Object, ObjectRef},
    PdfDocument,
};
use read_fonts::{
    model::pen::OutlinePen,
    ps::{
        cff::{dict, index::Index, CffFontRef, Subfont},
        cs::{NopFilterSink, TransformSink},
        error::Error as FontError,
        transform::{combine_scaled, ScaledFontMatrix, Transform},
    },
    types::{Fixed, GlyphId as CffGlyphId, Tag},
    FontRead, FontRef, TableProvider,
};
use std::collections::{hash_map::Entry, HashMap, HashSet};
use ttf_parser::{Face, GlyphId};

#[derive(Clone, Copy)]
pub(super) enum Font {
    Base14(Base14, Option<usize>),
    Embedded(usize),
    Type3(usize),
}

#[derive(Default)]
pub(super) struct Fonts {
    by_reference: HashMap<ObjectRef, Option<Font>>,
    base_metrics: Vec<FontInfo>,
    embedded: Vec<(FontInfo, EmbeddedOutline, Option<CidEncoding>)>,
    type3: Vec<type3_paint::Font>,
    type3_budget: type3_paint::Budget,
}

#[derive(Clone, Copy)]
enum EmbeddedOutline {
    CidTrueType,
    CidCff,
    SimpleCff,
}

pub(super) enum Glyphs {
    Base14(Base14),
    Embedded(HashMap<char, [f32; 4]>),
}

pub(super) struct GlyphAdvance {
    pub em: f32,
    pub word: bool,
    pub vertical_origin: Option<[f32; 2]>,
    pub paints_glyph: bool,
}

impl Glyphs {
    pub fn bounds(&self, character: char) -> Option<[f32; 4]> {
        match self {
            Self::Base14(font) => font
                .glyph(character)
                .map(|rect| rect.map(|v| f32::from(v) / 1000.0)),
            Self::Embedded(glyphs) => glyphs.get(&character).copied(),
        }
    }
}

impl Fonts {
    pub fn select(
        &mut self,
        pdf: &PdfDocument,
        binding: &Object,
        resources: &Object,
    ) -> Option<Font> {
        let reference = binding.as_reference();
        if let Some(font) = reference.and_then(|reference| self.by_reference.get(&reference)) {
            return *font;
        }
        let font = self.load(pdf, binding, resources);
        if let Some(reference) = reference {
            self.by_reference.insert(reference, font);
        }
        font
    }

    fn load(&mut self, pdf: &PdfDocument, binding: &Object, resources: &Object) -> Option<Font> {
        let object = pdf.resolve_object(binding).ok()?;
        let fields = object.as_dict()?;
        let subtype = fields.get("Subtype")?.as_name()?;
        let encoding = fields.get("Encoding").and_then(Object::as_name);
        let outline = match subtype {
            "Type1" => {
                let descriptor = match fields.get("FontDescriptor") {
                    Some(value) => Some(pdf.resolve_object(value).ok()?),
                    None => None,
                };
                let descriptor = match &descriptor {
                    Some(value) => Some(value.as_dict()?),
                    None => None,
                };
                if descriptor.is_some_and(|dict| dict.contains_key("FontFile3")) {
                    EmbeddedOutline::SimpleCff
                } else if encoding == Some("WinAnsiEncoding")
                    && !fields.contains_key("ToUnicode")
                    && !descriptor.is_some_and(|dict| {
                        ["FontFile", "FontFile2"]
                            .iter()
                            .any(|key| dict.contains_key(*key))
                    })
                {
                    let font = Base14::named(fields.get("BaseFont")?.as_name()?)?;
                    let metrics = FontInfo::from_dict(&object, pdf).ok().map(|info| {
                        let index = self.base_metrics.len();
                        self.base_metrics.push(info);
                        index
                    });
                    return Some(Font::Base14(font, metrics));
                } else {
                    return None;
                }
            }
            "Type0" => {
                let mut font = FontInfo::from_dict(&object, pdf).ok()?;
                let outline = match font.cid_font_type.as_deref()? {
                    "CIDFontType2" => EmbeddedOutline::CidTrueType,
                    "CIDFontType0" => EmbeddedOutline::CidCff,
                    _ => return None,
                };
                let cid_encoding = if matches!(encoding, Some("Identity-H" | "Identity-V")) {
                    None
                } else {
                    let encoding = CidEncoding::load(pdf, fields.get("Encoding")?, 0)?;
                    font.wmode = u8::from(encoding.vertical);
                    Some(encoding)
                };
                return self.add(outline, font, cid_encoding);
            }
            "Type3" => {
                let font = type3_paint::Font::load(pdf, &object, resources)?;
                let index = self.type3.len();
                self.type3.push(font);
                return Some(Font::Type3(index));
            }
            _ => return None,
        };
        self.add(outline, FontInfo::from_dict(&object, pdf).ok()?, None)
    }

    fn add(
        &mut self,
        outline: EmbeddedOutline,
        font: FontInfo,
        encoding: Option<CidEncoding>,
    ) -> Option<Font> {
        let data = font.embedded_font_data.as_deref()?;
        match outline {
            EmbeddedOutline::CidTrueType => {
                Face::parse(data, 0).ok()?.tables().glyf.as_ref()?;
            }
            EmbeddedOutline::CidCff => {
                if cff_font(data)?.is_cid() {
                    font.cff_cid_to_gid.as_ref()?;
                }
            }
            EmbeddedOutline::SimpleCff => {
                cff_font(data)?;
                font.cff_gid_map.as_ref()?;
            }
        }
        let index = self.embedded.len();
        self.embedded.push((font, outline, encoding));
        Some(Font::Embedded(index))
    }

    pub fn measure(&self, font: Font, bytes: &[u8]) -> Option<(Glyphs, usize)> {
        match font {
            Font::Base14(font, _) => Some((Glyphs::Base14(font), bytes.len())),
            Font::Embedded(index) => {
                let (font, outline, encoding) = &self.embedded[index];
                let codes = Codes::new(bytes, outline.code_width(), encoding.as_ref());
                let mut glyphs = HashMap::new();
                let mut count = 0;
                outline.visit(font, codes, |code, bounds| {
                    let Some(bounds) = bounds else {
                        return Some(());
                    };
                    let unicode = font.char_to_unicode(code.raw)?;
                    let mut chars = unicode.chars();
                    let character = chars.next()?;
                    if chars.next().is_some()
                        || glyphs
                            .insert(character, bounds)
                            .is_some_and(|previous| previous != bounds)
                    {
                        return None;
                    }
                    count += 1;
                    Some(())
                })?;
                Some((Glyphs::Embedded(glyphs), count))
            }
            Font::Type3(index) => {
                let font = &self.type3[index];
                let &[code] = bytes else {
                    return None;
                };
                let type3_paint::GlyphBounds::Finite(bounds) = font.cached_glyph(code)?.bounds
                else {
                    return None;
                };
                Some((
                    Glyphs::Embedded(HashMap::from([(font.character(code)?, bounds)])),
                    1,
                ))
            }
        }
    }

    pub fn advances<'a>(
        &'a self,
        font: Font,
        bytes: &'a [u8],
    ) -> Option<impl Iterator<Item = Option<GlyphAdvance>> + Clone + 'a> {
        let (font, code_width, encoding) = match font {
            Font::Base14(_, index) => (&self.base_metrics[index?], 1, None),
            Font::Embedded(index) => {
                let (font, outline, encoding) = &self.embedded[index];
                (font, outline.code_width(), encoding.as_ref())
            }
            Font::Type3(index) => (self.type3[index].metrics(), 1, None),
        };
        Some(Codes::new(bytes, code_width, encoding).map(move |code| {
            let Code { cid, word, .. } = code?;
            let Some(cid) = cid else {
                return Some(GlyphAdvance {
                    em: 0.0,
                    word,
                    vertical_origin: (font.wmode == 1).then_some([0.0, 0.0]),
                    paints_glyph: false,
                });
            };
            let width = if font.subtype == "Type0" {
                let width = font
                    .cid_widths
                    .as_ref()
                    .and_then(|widths| widths.get(&cid))
                    .copied()
                    .unwrap_or(font.cid_default_width);
                (width + 0.5).floor()
            } else {
                font.get_glyph_width(cid)
            };
            let (em, vertical_origin) = if font.wmode == 1 {
                let (x, y, advance) = match font
                    .cid_vertical_metrics
                    .as_ref()
                    .and_then(|metrics| metrics.get(&cid))
                {
                    Some(metrics) => ((metrics.v_x + 0.5).floor(), metrics.v_y, metrics.w1y),
                    None => {
                        let metrics = font.cid_default_vertical_metrics;
                        ((width / 2.0).trunc(), metrics.v_y, metrics.w1y)
                    }
                };
                (
                    (advance + 0.5).floor() * 0.001,
                    Some([x * 0.001, (y + 0.5).floor() * 0.001]),
                )
            } else {
                (width * font.font_matrix_a, None)
            };
            (em.is_finite() && vertical_origin.is_none_or(|[x, y]| x.is_finite() && y.is_finite()))
                .then_some(GlyphAdvance {
                    em,
                    word,
                    vertical_origin,
                    paints_glyph: true,
                })
        }))
    }

    pub fn vertical(&self, font: Font) -> bool {
        matches!(font, Font::Embedded(index) if self.embedded[index].0.wmode == 1)
    }

    pub fn visit_outlines(
        &self,
        font: Font,
        bytes: &[u8],
        mut visit: impl FnMut(Option<[f32; 4]>) -> Option<()>,
    ) -> Option<()> {
        let Font::Embedded(index) = font else {
            return None;
        };
        let (font, outline, encoding) = &self.embedded[index];
        let codes = Codes::new(bytes, outline.code_width(), encoding.as_ref());
        outline.visit(font, codes, |_, bounds| visit(bounds))
    }

    pub fn visit_nested_outlines(
        &self,
        font: Font,
        bytes: &[u8],
        mut visit: impl FnMut(Option<[f32; 4]>) -> Option<()>,
    ) -> Option<()> {
        let Font::Base14(family, index) = font else {
            return self.visit_outlines(font, bytes, visit);
        };
        let metrics = &self.base_metrics[index?];
        for &byte in bytes {
            let value = metrics.char_to_unicode(u32::from(byte))?;
            let mut chars = value.chars();
            let glyph = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            visit(Some(Glyphs::Base14(family).bounds(glyph)?))?;
        }
        Some(())
    }

    pub fn type3_glyph(
        &mut self,
        pdf: &PdfDocument,
        index: usize,
        code: u8,
        off_layers: &HashSet<String>,
    ) -> Option<type3_paint::Glyph> {
        self.type3[index].glyph(pdf, code, off_layers, &mut self.type3_budget)
    }
}

impl EmbeddedOutline {
    fn code_width(self) -> usize {
        if matches!(self, Self::SimpleCff) {
            1
        } else {
            2
        }
    }

    fn visit(
        self,
        font: &FontInfo,
        codes: Codes<'_>,
        mut visit: impl FnMut(Code, Option<[f32; 4]>) -> Option<()>,
    ) -> Option<()> {
        let data = font.embedded_font_data.as_deref()?;
        let true_type = matches!(self, Self::CidTrueType);
        let face = true_type.then(|| Face::parse(data, 0).ok()).flatten();
        let cff = (!true_type).then(|| cff_font(data)).flatten();
        let mut subfonts = HashMap::new();
        let mut cff_bounds = HashMap::new();
        for code in codes {
            let code = code?;
            let Some(cid) = code.cid else {
                visit(code, None)?;
                continue;
            };
            let bounds = if let Some(face) = &face {
                let gid = font
                    .cid_to_gid_map
                    .as_ref()
                    .map_or(cid, |map| map.get_gid(cid));
                let units = f32::from(face.units_per_em());
                match face.glyph_bounding_box(GlyphId(gid)) {
                    Some(rect) => [rect.x_min, rect.y_min, rect.x_max, rect.y_max]
                        .map(|v| f32::from(v) / units),
                    None if gid < face.number_of_glyphs() => [0.0; 4],
                    None => return None,
                }
            } else {
                let cff = cff.as_ref()?;
                let gid = if cff.is_cid() {
                    *font.cff_cid_to_gid.as_ref()?.get(&cid)?
                } else if matches!(self, Self::SimpleCff) {
                    *font.cff_gid_map.as_ref()?.get(&(cid as u8))?
                } else {
                    cid
                };
                if let Some(bounds) = cff_bounds.get(&gid) {
                    *bounds
                } else {
                    let bounds = cff_glyph_bounds(cff, &mut subfonts, gid)?;
                    cff_bounds.insert(gid, bounds);
                    bounds
                }
            };
            visit(code, Some(bounds))?;
        }
        Some(())
    }
}

fn cff_font(data: &[u8]) -> Option<CffFontRef<'_>> {
    if data.first() == Some(&1) {
        return CffFontRef::new_cff(data, 0, None).ok();
    }
    let face = FontRef::from_index(data, 0).ok()?;
    if let Some(cff) = face.table_data(Tag::new(b"CFF ")) {
        // MuPDF loads the CFF1 table of an OTTO font as standalone CFF.
        // PDF Oxide can also wrap standalone CFF in an OTTO container.
        let units = if data.starts_with(b"OTTO") {
            None
        } else {
            Some(i32::from(face.head().ok()?.units_per_em()))
        };
        return CffFontRef::new_cff(cff.as_bytes(), 0, units).ok();
    }
    let cff2 = face.table_data(Tag::new(b"CFF2"))?;
    let units = i32::from(face.head().ok()?.units_per_em());
    CffFontRef::new_cff2(cff2.as_bytes(), Some(units)).ok()
}

fn cff_glyph_bounds(
    font: &CffFontRef<'_>,
    subfonts: &mut HashMap<u16, CffSubfont>,
    gid: u16,
) -> Option<[f32; 4]> {
    let gid = CffGlyphId::from(gid);
    let index = font.subfont_index(gid)?;
    let units = f32::from(u16::try_from(font.upem()).ok()?);
    if units == 0.0 {
        return None;
    }
    // The reference measures at one 26.6 position unit per font unit.
    // Use the same size so the selected font dictionary's rounding is retained.
    let size = units / 64.0;
    let ppem = size.max(1.0);
    let subfont = match subfonts.entry(index) {
        Entry::Occupied(entry) => entry.into_mut(),
        Entry::Vacant(entry) => entry.insert(cff_subfont(font, index, ppem)?),
    };
    let mut points = ControlBounds::default();
    let mut sink = NopFilterSink::new(&mut points);
    let mut transform_sink = TransformSink::new(&mut sink, subfont.transform);
    font.evaluate_charstring(&subfont.program, gid, &[], &mut transform_sink)
        .ok()?;
    Some(points.0.unwrap_or([0.0; 4]).map(|point| point / size))
}

struct CffSubfont {
    program: Subfont,
    transform: Transform,
}

fn cff_subfont(font: &CffFontRef<'_>, index: u16, ppem: f32) -> Option<CffSubfont> {
    let parsed = font.subfont(index, &[]);
    let own_matrix = if font.is_cid() && (font.matrix().is_some() || parsed.is_err()) {
        cff_dict_matrix(font, index)?
    } else {
        None
    };
    let (program, mut transform) = match parsed {
        Ok(program) => (program, font.transform(&program, Some(ppem))),
        // The reference accepts omitted Private DICTs and still applies the
        // Font DICT matrix. There are no local subroutines to load.
        Err(FontError::MissingPrivateDict) if font.is_cid() => (
            Subfont::default(),
            match own_matrix {
                Some(matrix) => cff_font_dict_transform(font, matrix, ppem)?,
                None => font.transform(&Subfont::default(), Some(ppem)),
            },
        ),
        Err(_) => return None,
    };
    if font.is_cid() && font.matrix().is_some() && own_matrix.is_none() {
        // FreeType converts the top offset to integer font units, copies it to
        // the subfont and shifts the offset again during subfont normalization.
        for offset in [&mut transform.matrix.dx, &mut transform.matrix.dy] {
            *offset = Fixed::from_bits(offset.to_bits() >> 16);
        }
    }
    Some(CffSubfont { program, transform })
}

fn cff_dict_matrix(font: &CffFontRef<'_>, index: u16) -> Option<Option<ScaledFontMatrix>> {
    let raw = read_fonts::tables::cff::Cff::read(font.data().into()).ok()?;
    let top_dict = raw.top_dicts().get(0).ok()?;
    let fd_array_offset = dict::entries(top_dict, None).find_map(|entry| match entry.ok()? {
        dict::Entry::FdArrayOffset(offset) => Some(offset),
        _ => None,
    })?;
    let dictionaries = Index::new(font.data().get(fd_array_offset..)?, false).ok()?;
    let selected = dictionaries.get(usize::from(index)).ok()?;
    Some(
        dict::entries(selected, None).find_map(|entry| match entry.ok()? {
            dict::Entry::FontMatrix(matrix) => Some(matrix),
            _ => None,
        }),
    )
}

fn cff_font_dict_transform(
    font: &CffFontRef<'_>,
    mut matrix: ScaledFontMatrix,
    ppem: f32,
) -> Option<Transform> {
    if let Some(top) = font.matrix() {
        let scale = if top.scale > 1 && matrix.scale > 1 {
            top.scale.min(matrix.scale)
        } else {
            1
        };
        matrix = ScaledFontMatrix {
            matrix: combine_scaled(&top.matrix, &matrix.matrix, scale),
            scale: Fixed::from_bits(matrix.scale)
                .mul_div(Fixed::from_bits(top.scale), Fixed::from_bits(scale))
                .to_bits(),
        };
    }
    let matrix = matrix.normalize();
    if matrix.scale <= 0 {
        return None;
    }
    let scale = Transform::compute_scale(ppem, font.upem());
    Some(Transform {
        matrix: matrix.matrix,
        scale: Some(scale.mul_div(
            Fixed::from_bits(font.upem()),
            Fixed::from_bits(matrix.scale),
        )),
    })
}

// The reference paint bounds include off-curve control points.
#[derive(Default)]
struct ControlBounds(Option<[f32; 4]>);

impl OutlinePen for ControlBounds {
    fn move_to(&mut self, x: f32, y: f32) {
        let [x0, y0, x1, y1] = self.0.unwrap_or([x, y, x, y]);
        self.0 = Some([x0.min(x), y0.min(y), x1.max(x), y1.max(y)]);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.move_to(x, y);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.move_to(x1, y1);
        self.move_to(x, y);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.move_to(x1, y1);
        self.move_to(x2, y2);
        self.move_to(x, y);
    }

    fn close(&mut self) {}
}
