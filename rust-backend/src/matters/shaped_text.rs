//! Position shaped glyphs and preserve each cluster's searchable Unicode.
use super::page_tools::assembly::Overlay;
use icu_properties::{props::Script, CodePointMapData};
use pdf_oxide::{
    fonts::FontInfo,
    writer::{shape_text, ShapeDirection},
};
use std::{cell::RefCell, collections::HashMap, fmt::Write as _, ops::Range, sync::Arc};
use ttf_parser::{Face, GlyphId, OutlineBuilder};
use unicode_bidi::BidiInfo;

const INVALID: &str = "Could not embed the shaped PDF text";
pub(super) const CONTINUATION: char = '\u{200b}';

pub(in crate::matters) struct Font {
    pub source: Arc<FontInfo>,
    pub glyphs: Vec<(u16, String)>,
    pub outlines: bool,
}

pub(super) struct Shaper {
    font: Arc<FontInfo>,
    units: f64,
    outlines: bool,
    lines: RefCell<HashMap<String, Option<Line>>>,
}

struct Line {
    glyphs: Vec<Glyph>,
    width: f64,
    complex: bool,
    bounds: [f64; 4],
}

struct Glyph {
    id: u16,
    text: String,
    x: f64,
    y: f64,
    rtl: bool,
}

impl Shaper {
    pub(super) fn new(font: &Arc<FontInfo>) -> Option<Self> {
        let bytes = font.embedded_font_data.as_deref()?;
        let face = Face::parse(bytes, 0).ok()?;
        // CIDFontType2 supports TrueType programs. OpenType/CFF uses its
        // source outlines because CIDFontType0 cannot use a CIDToGIDMap.
        let outlines = !bytes.starts_with(&[0, 1, 0, 0]) || face.tables().glyf.is_none();
        if outlines && face.tables().cff.is_none() && face.tables().cff2.is_none() {
            return None;
        }
        let units = face.units_per_em();
        (units != 0).then_some(Self {
            font: Arc::clone(font),
            units: f64::from(units),
            outlines,
            lines: RefCell::new(HashMap::new()),
        })
    }

    fn shape(&self, value: &str) -> Option<Line> {
        let face = self.font.embedded_font_data.as_deref()?;
        let metrics = Face::parse(face, 0).ok()?;
        let bidi = BidiInfo::new(value, None);
        let mut width = 0.0;
        let mut glyphs = Vec::new();
        let mut complex = false;
        let mut bounds = [
            0.0,
            f64::from(self.font.descent),
            0.0,
            f64::from(self.font.ascent),
        ];
        for paragraph in &bidi.paragraphs {
            let (levels, runs) = bidi.visual_runs(paragraph, paragraph.range.clone());
            for range in runs {
                let rtl = levels[range.start].is_rtl();
                let direction = if rtl {
                    ShapeDirection::Rtl
                } else {
                    ShapeDirection::Ltr
                };
                let mut scripts = script_runs(value.get(range.clone())?);
                if rtl {
                    scripts.reverse();
                }
                for script in scripts {
                    let start = range.start + script.start;
                    let text = value.get(start..range.start + script.end)?;
                    let shaped = shape_text(text, face, direction)?;
                    let mut logical = shaped
                        .glyphs
                        .iter()
                        .map(|g| g.cluster as usize)
                        .collect::<Vec<_>>();
                    logical.push(text.len());
                    logical.sort_unstable();
                    logical.dedup();
                    if logical.first() != Some(&0) || logical.len() < 2 {
                        return None;
                    }
                    complex |= rtl || shaped.glyphs.len() != text.chars().count();
                    let mut pen_y = 0.0;
                    let mut previous = None;
                    for glyph in shaped.glyphs {
                        if glyph.glyph_id == 0 {
                            return None;
                        }
                        let start = glyph.cluster as usize;
                        let end = *logical.get(logical.binary_search(&start).ok()? + 1)?;
                        let first = previous != Some(start);
                        let characters = text.get(start..end)?;
                        complex |= glyph.x_offset != 0 || glyph.y_offset != 0 || !first;
                        let x = width + f64::from(glyph.x_offset);
                        let y = pen_y + f64::from(glyph.y_offset);
                        let glyph_bounds = metrics.glyph_bounding_box(GlyphId(glyph.glyph_id));
                        if let Some(rect) = glyph_bounds {
                            bounds[0] = bounds[0].min((x + f64::from(rect.x_min)) / self.units);
                            bounds[1] = bounds[1].min((y + f64::from(rect.y_min)) / self.units);
                            bounds[2] = bounds[2].max((x + f64::from(rect.x_max)) / self.units);
                            bounds[3] = bounds[3].max((y + f64::from(rect.y_max)) / self.units);
                        }
                        glyphs.push(Glyph {
                            id: glyph.glyph_id,
                            text: if first {
                                if rtl {
                                    // ReversedChars applies to every Unicode character
                                    // in its scope, including ligature mappings.
                                    characters.chars().rev().collect()
                                } else {
                                    characters.to_owned()
                                }
                            } else {
                                CONTINUATION.to_string()
                            },
                            x,
                            y,
                            rtl,
                        });
                        width += f64::from(glyph.x_advance);
                        pen_y += f64::from(glyph.y_advance);
                        previous = Some(start);
                    }
                }
            }
        }
        bounds[2] = bounds[2].max(width / self.units);
        Some(Line {
            glyphs,
            width: width / self.units,
            complex,
            bounds,
        })
    }

    fn with_line<T>(&self, value: &str, use_line: impl FnOnce(&Line) -> T) -> Option<T> {
        let mut lines = self.lines.borrow_mut();
        if lines.len() >= 16 && !lines.contains_key(value) {
            lines.clear();
        }
        lines
            .entry(value.to_owned())
            .or_insert_with(|| self.shape(value))
            .as_ref()
            .map(use_line)
    }

    pub(super) fn width(&self, value: &str) -> Option<f64> {
        self.with_line(value, |line| line.width)
    }

    pub(super) fn bounds(&self, value: &str) -> Option<[f64; 4]> {
        self.with_line(value, |line| line.bounds)
    }

    pub(super) fn needs_shaping(&self, value: &str) -> bool {
        self.with_line(value, |line| line.complex).unwrap_or(false)
    }

    pub(super) fn write(
        &self,
        overlay: &mut Overlay,
        value: &str,
        size: f64,
        x: f64,
        y: f64,
        (r, g, b): (f64, f64, f64),
    ) -> Result<(), &'static str> {
        let mut lines = self.lines.borrow_mut();
        let line = lines
            .entry(value.to_owned())
            .or_insert_with(|| self.shape(value))
            .as_ref()
            .ok_or(INVALID)?;
        let limit = if self.outlines {
            u8::MAX as usize
        } else {
            u16::MAX as usize
        };
        let code_width = if self.outlines { 2 } else { 4 };
        writeln!(overlay.commands, "{r:.5} {g:.5} {b:.5} rg BT").map_err(|_| INVALID)?;
        let mut active = None;
        let mut reversed = false;
        for glyph in &line.glyphs {
            if reversed != glyph.rtl {
                overlay.commands.push_str(if glyph.rtl {
                    "/ReversedChars BMC\n"
                } else {
                    "EMC\n"
                });
                reversed = glyph.rtl;
            }
            let matching = |font: &Font| Arc::ptr_eq(&font.source, &self.font);
            let available = overlay
                .shaped_fonts
                .iter()
                .enumerate()
                .filter(|(_, font)| matching(font))
                .find_map(|(index, font)| {
                    font.glyphs
                        .iter()
                        .position(|(id, text)| *id == glyph.id && *text == glyph.text)
                        .map(|code| (index, code + 1))
                });
            let (index, code) = if let Some(available) = available {
                available
            } else {
                let index = overlay
                    .shaped_fonts
                    .iter()
                    .position(|font| matching(font) && font.glyphs.len() < limit)
                    .unwrap_or_else(|| {
                        overlay.shaped_fonts.push(Font {
                            source: Arc::clone(&self.font),
                            glyphs: Vec::new(),
                            outlines: self.outlines,
                        });
                        overlay.shaped_fonts.len() - 1
                    });
                let entries = &mut overlay.shaped_fonts[index].glyphs;
                entries.push((glyph.id, glyph.text.clone()));
                (index, entries.len())
            };
            if active != Some(index) {
                writeln!(overlay.commands, "/@S{index} {size:.6} Tf").map_err(|_| INVALID)?;
                active = Some(index);
            }
            let x = x + glyph.x * size / self.units;
            let y = y + glyph.y * size / self.units;
            writeln!(
                overlay.commands,
                "1 0 0 1 {x:.6} {y:.6} Tm <{code:0code_width$X}> Tj"
            )
            .map_err(|_| INVALID)?;
        }
        if reversed {
            overlay.commands.push_str("EMC\n");
        }
        overlay.commands.push_str("ET\n");
        Ok(())
    }
}

fn script_runs(value: &str) -> Vec<Range<usize>> {
    let scripts = CodePointMapData::<Script>::new();
    let mut previous = None;
    let mut start = 0;
    let mut runs = Vec::new();
    for (index, character) in value.char_indices() {
        let script = scripts.get(character);
        if matches!(script, Script::Common | Script::Inherited | Script::Unknown) {
            continue;
        }
        if previous.is_some_and(|previous| previous != script) {
            runs.push(start..index);
            start = index;
        }
        previous = Some(script);
    }
    runs.push(start..value.len());
    runs
}

#[derive(Default)]
struct Outline {
    path: String,
    point: (f32, f32),
}

impl OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        writeln!(self.path, "{x:.6} {y:.6} m").unwrap();
        self.point = (x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        writeln!(self.path, "{x:.6} {y:.6} l").unwrap();
        self.point = (x, y);
    }

    fn quad_to(&mut self, x: f32, y: f32, end_x: f32, end_y: f32) {
        let (start_x, start_y) = self.point;
        self.curve_to(
            start_x + (x - start_x) * (2.0 / 3.0),
            start_y + (y - start_y) * (2.0 / 3.0),
            end_x + (x - end_x) * (2.0 / 3.0),
            end_y + (y - end_y) * (2.0 / 3.0),
            end_x,
            end_y,
        );
    }

    fn curve_to(&mut self, ax: f32, ay: f32, bx: f32, by: f32, x: f32, y: f32) {
        writeln!(self.path, "{ax:.6} {ay:.6} {bx:.6} {by:.6} {x:.6} {y:.6} c").unwrap();
        self.point = (x, y);
    }

    fn close(&mut self) {
        self.path.push_str("h\n");
    }
}

pub(super) fn glyph_outline(face: &Face<'_>, gid: u16) -> Result<(i64, String), &'static str> {
    let glyph = GlyphId(gid);
    let width = i64::from(face.glyph_hor_advance(glyph).ok_or(INVALID)?);
    let mut outline = Outline::default();
    let commands = if let Some(bounds) = face.outline_glyph(glyph, &mut outline) {
        let ttf_parser::Rect {
            x_min,
            y_min,
            x_max,
            y_max,
        } = bounds;
        format!(
            "{width} 0 {x_min} {y_min} {x_max} {y_max} d1\n{}f\n",
            outline.path
        )
    } else {
        format!("{width} 0 d0\n")
    };
    Ok((width, commands))
}
