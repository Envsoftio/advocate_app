//! Stored annotation and widget appearances in PDF view order.
use super::{assembly::Dictionary, form_geometry};
use pdf_oxide::{
    content::{parse_content_stream, Matrix, Operator},
    object::{Object, ObjectRef},
    optional_content::{annotation_is_excluded, decode_pdf_text_string},
    PdfDocument,
};
use std::collections::HashSet;

mod rich_text;

pub(super) enum Appearance {
    Stored {
        reference: ObjectRef,
        placement: Matrix,
    },
    Generated(Vec<GeneratedPaint>),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum GeneratedPaint {
    Paths(usize),
    Text(bool),
}

pub(super) fn visible(
    pdf: &PdfDocument,
    page: usize,
    off_layers: &HashSet<String>,
) -> Vec<Appearance> {
    let annotations = pdf
        .get_page_for_debug(page)
        .ok()
        .and_then(|page| pdf.resolve_object(page.as_dict()?.get("Annots")?).ok());
    let Some(annotations) = annotations.as_ref().and_then(Object::as_array) else {
        return Vec::new();
    };
    let regenerate_widgets = pdf
        .catalog()
        .ok()
        .and_then(|catalog| resolved(pdf, catalog.as_dict()?, "AcroForm"))
        .and_then(|form| resolved(pdf, form.as_dict()?, "NeedAppearances"))
        .and_then(|flag| flag.as_bool())
        .unwrap_or(false);
    let mut visible = Vec::new();
    let mut widgets = Vec::new();
    for annotation in annotations {
        let Ok(annotation) = pdf.resolve_object(annotation) else {
            continue;
        };
        let Some(fields) = annotation.as_dict() else {
            continue;
        };
        let subtype = resolved(pdf, fields, "Subtype");
        let subtype = subtype.as_ref().and_then(Object::as_name);
        if matches!(subtype, Some("Link" | "Popup")) {
            continue;
        }
        let flags = resolved(pdf, fields, "F")
            .as_ref()
            .and_then(Object::as_integer)
            .unwrap_or(0);
        if flags & (1 | 2 | 32) != 0
            || fields
                .get("OC")
                .is_some_and(|oc| annotation_is_excluded(oc, pdf, off_layers))
        {
            continue;
        }
        let stored = || normal_appearance(pdf, fields, flags);
        let generated = || generated_appearance(pdf, fields, subtype?);
        let signature = (subtype == Some("Widget"))
            .then(|| signature_widget(pdf, fields))
            .flatten();
        let appearance = if let Some(signed_value_present) = signature {
            if signed_value_present {
                stored()
            } else {
                generated().or_else(stored)
            }
        } else if subtype == Some("Widget") && regenerate_widgets {
            generated().or_else(stored)
        } else {
            stored().or_else(generated)
        };
        if let Some(appearance) = appearance {
            if subtype == Some("Widget") {
                widgets.push(appearance);
            } else {
                visible.push(appearance);
            }
        }
    }
    visible.extend(widgets);
    visible
}

fn normal_appearance(pdf: &PdfDocument, annot: &Dictionary, flags: i64) -> Option<Appearance> {
    let appearances = pdf.resolve_object(annot.get("AP")?).ok()?;
    let normal = appearances.as_dict()?.get("N")?;
    let resolved = pdf.resolve_object(normal).ok()?;
    let (reference, stream) = match resolved {
        Object::Dictionary(states) => {
            let state = pdf.resolve_object(annot.get("AS")?).ok()?;
            let chosen = states.get(state.as_name()?)?;
            (chosen.as_reference()?, pdf.resolve_object(chosen).ok()?)
        }
        Object::Stream { .. } => (normal.as_reference()?, resolved),
        _ => return None,
    };
    let Object::Stream { dict, .. } = stream else {
        return None;
    };
    let (bbox, local) = form_geometry(pdf, &dict, Matrix::identity()).ok()?;
    let [bx0, by0, bx1, by1] = projected(local, bbox);
    let [mut x0, mut y0, mut x1, mut y1] = annotation_rect(pdf, annot.get("Rect")?)?;
    (x0, x1) = (x0.min(x1), x0.max(x1));
    (y0, y1) = (y0.min(y1), y0.max(y1));
    // The reference measures the unrotated View. NoZoom anchors the
    // original appearance size at the annotation's upper-left corner.
    if flags & 8 != 0 {
        x1 = x0 + (bbox[2] - bbox[0]).abs();
        y0 = y1 - (bbox[3] - bbox[1]).abs();
    }
    let scale = |from: f32, to: f32| if from > 0.0 { to / from } else { 0.0 };
    let a = scale(bx1 - bx0, x1 - x0);
    let d = scale(by1 - by0, y1 - y0);
    let placement = Matrix {
        a,
        b: 0.0,
        c: 0.0,
        d,
        e: x0 - a * bx0,
        f: y0 - d * by0,
    };
    Some(Appearance::Stored {
        reference,
        placement,
    })
}

fn resolved(pdf: &PdfDocument, fields: &Dictionary, key: &str) -> Option<Object> {
    pdf.resolve_object(fields.get(key)?).ok()
}

fn inherited(pdf: &PdfDocument, mut fields: Dictionary, key: &str) -> Option<Object> {
    for _ in 0..32 {
        if let Some(value) =
            resolved(pdf, &fields, key).filter(|value| !matches!(value, Object::Null))
        {
            return Some(value);
        }
        fields = resolved(pdf, &fields, "Parent")?.as_dict()?.clone();
    }
    None
}

fn color(value: Option<&Object>) -> bool {
    value
        .and_then(Object::as_array)
        .is_some_and(|values| matches!(values.len(), 1 | 3 | 4))
}

fn text_value(value: &Object) -> Option<String> {
    match value {
        Object::String(bytes) => Some(decode_pdf_text_string(bytes)),
        Object::Name(name) => Some(name.clone()),
        _ => None,
    }
}

fn appearance_text(value: &Object) -> Option<bool> {
    plain_text(&text_value(value)?)
}

fn plain_text(text: &str) -> Option<bool> {
    text.chars()
        .any(|ch| !matches!(ch, '\n' | '\r'))
        .then(|| text.chars().any(|ch| !ch.is_whitespace()))
}

fn annotation_font_size(pdf: &PdfDocument, fields: &Dictionary, widget: bool) -> f64 {
    let value = if widget {
        inherited(pdf, fields.clone(), "DA").or_else(|| {
            let catalog = pdf.catalog().ok()?;
            let form = resolved(pdf, catalog.as_dict()?, "AcroForm")?;
            resolved(pdf, form.as_dict()?, "DA")
        })
    } else {
        resolved(pdf, fields, "DA")
    };
    let size = (|| {
        let da = text_value(value.as_ref()?)?;
        parse_content_stream(da.as_bytes())
            .ok()?
            .into_iter()
            .rev()
            .find_map(|op| match op {
                Operator::Tf { size, .. } => Some(f64::from(size)),
                _ => None,
            })
    })();
    size.unwrap_or(if widget { 0.0 } else { 12.0 })
}

fn add_text_paints(
    paints: &mut Vec<GeneratedPaint>,
    pdf: &PdfDocument,
    fields: &Dictionary,
    value: Option<&str>,
    rich_key: &str,
    widget: bool,
) {
    let markup = resolved(pdf, fields, rich_key)
        .as_ref()
        .and_then(text_value);
    let style = resolved(pdf, fields, "DS").as_ref().and_then(text_value);
    if let Some(markup) = markup {
        let size = annotation_font_size(pdf, fields, widget);
        paints.extend(rich_text::paint(
            &markup,
            style.as_deref().unwrap_or_default(),
            size,
        ));
    } else if let Some(value) = value {
        if let Some(style) = style {
            let size = annotation_font_size(pdf, fields, widget);
            let escaped = value
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            paints.extend(rich_text::paint(&escaped, &style, size));
        } else {
            with_text(paints, plain_text(value));
        }
    }
}

fn signature_widget(pdf: &PdfDocument, fields: &Dictionary) -> Option<bool> {
    if inherited(pdf, fields.clone(), "FT")?.as_name()? != "Sig" {
        return None;
    }
    // This selects the display appearance. Signature admission and validity
    // are handled independently from the presence of a form-field value.
    let value = inherited(pdf, fields.clone(), "V");
    Some(
        value
            .as_ref()
            .and_then(Object::as_dict)
            .is_some_and(|dict| {
                resolved(pdf, dict, "Type").is_none_or(|value| value.as_name() == Some("Sig"))
            }),
    )
}

fn ending_paints(ending: &Object, stroke: bool, fill: bool) -> usize {
    match ending.as_name() {
        Some("Square" | "Circle" | "Diamond" | "ClosedArrow" | "RClosedArrow") => {
            usize::from(stroke) + usize::from(fill)
        }
        Some("OpenArrow" | "ROpenArrow" | "Butt" | "Slash") => usize::from(stroke),
        _ => 0,
    }
}

fn with_text(paints: &mut Vec<GeneratedPaint>, text: Option<bool>) {
    if let Some(ink) = text {
        paints.push(GeneratedPaint::Text(ink));
    }
}

fn generated_appearance(
    pdf: &PdfDocument,
    fields: &Dictionary,
    subtype: &str,
) -> Option<Appearance> {
    let rect = annotation_rect(pdf, fields.get("Rect")?)?;
    if rect[0] == rect[2] || rect[1] == rect[3] {
        return None;
    }
    let number = |object: &Object| {
        object
            .as_real()
            .or_else(|| object.as_integer().map(|n| n as f64))
    };
    let border = resolved(pdf, fields, "BS")
        .as_ref()
        .and_then(Object::as_dict)
        .and_then(|dict| dict.get("W"))
        .and_then(number)
        .or_else(|| {
            resolved(pdf, fields, "Border")
                .as_ref()
                .and_then(Object::as_array)
                .and_then(|border| border.get(2))
                .and_then(number)
        })
        .unwrap_or(1.0)
        > 0.0;
    let exterior = color(resolved(pdf, fields, "C").as_ref());
    let interior = color(resolved(pdf, fields, "IC").as_ref());
    let path_count = |key: &str| {
        resolved(pdf, fields, key)
            .as_ref()
            .and_then(Object::as_array)
            .map_or(0, |items| items.len())
    };
    let ending = |key: &str, stroke, fill| {
        resolved(pdf, fields, key)
            .as_ref()
            .and_then(Object::as_array)
            .filter(|endings| endings.len() == 2)
            .into_iter()
            .flatten()
            .filter_map(|entry| pdf.resolve_object(entry).ok())
            .map(|entry| ending_paints(&entry, stroke, fill))
            .sum::<usize>()
    };
    let mut paints = Vec::new();
    let paths = match subtype {
        "FreeText" => {
            let callout = resolved(pdf, fields, "IT")
                .as_ref()
                .and_then(Object::as_name)
                == Some("FreeTextCallout")
                && matches!(path_count("CL"), 4 | 6);
            let mut paths = usize::from(exterior) + usize::from(border);
            if callout {
                paths += 1 + resolved(pdf, fields, "LE")
                    .map_or(0, |end| ending_paints(&end, true, exterior));
            }
            paints.push(GeneratedPaint::Paths(paths));
            let value = resolved(pdf, fields, "Contents")
                .as_ref()
                .and_then(text_value);
            add_text_paints(&mut paints, pdf, fields, value.as_deref(), "RC", false);
            0
        }
        "Square" | "Circle" => usize::from(interior) + usize::from(exterior),
        "Polygon" if path_count("Vertices") >= 4 => usize::from(interior) + usize::from(exterior),
        "PolyLine" if path_count("Vertices") >= 4 => {
            usize::from(exterior) + ending("LE", exterior, interior)
        }
        "Polygon" | "PolyLine" => 0,
        "Ink" => usize::from(exterior),
        "Line" => {
            if resolved(pdf, fields, "Cap")
                .as_ref()
                .and_then(Object::as_bool)
                == Some(true)
            {
                // The caption is emitted before the line and ending marks.
                // Its fitted text box is present even for a space-only caption.
                with_text(
                    &mut paints,
                    resolved(pdf, fields, "Contents")
                        .as_ref()
                        .and_then(appearance_text)
                        .map(|_| true),
                );
            }
            usize::from(exterior) + ending("LE", exterior, interior)
        }
        "Highlight" | "Underline" | "Squiggly" | "StrikeOut" => path_count("QuadPoints") / 8,
        "Text" | "FileAttachment" | "Sound" => 3,
        "Caret" => 1,
        "Redact" => path_count("QuadPoints").div_ceil(8).max(1),
        "Stamp" => {
            paints.push(GeneratedPaint::Paths(1));
            paints.push(GeneratedPaint::Text(true));
            0
        }
        "Widget" => {
            paints = widget_paints(pdf, fields, border)?;
            0
        }
        _ => return None,
    };
    if paths > 0 {
        paints.push(GeneratedPaint::Paths(paths));
    }
    Some(Appearance::Generated(paints))
}

fn widget_paints(
    pdf: &PdfDocument,
    fields: &Dictionary,
    border: bool,
) -> Option<Vec<GeneratedPaint>> {
    let field_type = inherited(pdf, fields.clone(), "FT")?;
    let flags = inherited(pdf, fields.clone(), "Ff")
        .as_ref()
        .and_then(Object::as_integer)
        .unwrap_or(0);
    let empty = Dictionary::new();
    let style = resolved(pdf, fields, "MK");
    let style = style.as_ref().and_then(Object::as_dict).unwrap_or(&empty);
    let background = color(resolved(pdf, style, "BG").as_ref());
    let outline = border && color(resolved(pdf, style, "BC").as_ref());
    let frame = usize::from(background) + usize::from(outline);
    let mut paints = vec![GeneratedPaint::Paths(frame)];
    match field_type.as_name()? {
        "Tx" | "Ch" => {
            let value = if field_type.as_name() == Some("Ch") && flags & (1 << 17) == 0 {
                let options = resolved(pdf, fields, "Opt");
                let first = resolved(pdf, fields, "TI")
                    .as_ref()
                    .and_then(Object::as_integer)
                    .unwrap_or(0)
                    .max(0) as usize;
                let rows = options
                    .as_ref()
                    .and_then(Object::as_array)
                    .into_iter()
                    .flatten()
                    .skip(first)
                    .filter_map(|option| {
                        let option = pdf.resolve_object(option).ok()?;
                        let display = option
                            .as_array()
                            .and_then(|pair| pair.get(1))
                            .unwrap_or(&option);
                        text_value(&pdf.resolve_object(display).ok()?)
                    })
                    .collect::<Vec<_>>();
                Some(rows.join("\n"))
            } else {
                inherited(pdf, fields.clone(), "V")
                    .as_ref()
                    .and_then(text_value)
            };
            if field_type.as_name() == Some("Tx")
                && flags & (1 << 24) != 0
                && flags & (1 << 12) == 0
                && outline
                && resolved(pdf, fields, "RV")
                    .as_ref()
                    .and_then(text_value)
                    .is_none()
                && resolved(pdf, fields, "DS")
                    .as_ref()
                    .and_then(text_value)
                    .is_none()
            {
                let slots = inherited(pdf, fields.clone(), "MaxLen")
                    .as_ref()
                    .and_then(Object::as_integer)
                    .unwrap_or(1)
                    .max(1);
                paints.push(GeneratedPaint::Paths((slots - 1) as usize));
            }
            add_text_paints(&mut paints, pdf, fields, value.as_deref(), "RV", true);
        }
        "Btn" if flags & (1 << 16) != 0 => {
            paints.push(GeneratedPaint::Paths(2 * usize::from(background)));
            with_text(
                &mut paints,
                resolved(pdf, style, "CA")
                    .as_ref()
                    .and_then(appearance_text),
            );
        }
        "Btn" => {
            let on = resolved(pdf, fields, "AS")
                .or_else(|| inherited(pdf, fields.clone(), "V"))
                .as_ref()
                .and_then(Object::as_name)
                .is_some_and(|state| state != "Off");
            if flags & (1 << 15) != 0 {
                paints.push(GeneratedPaint::Paths(usize::from(on)));
            } else {
                with_text(&mut paints, on.then_some(true));
            }
        }
        "Sig" => paints = vec![GeneratedPaint::Paths(1), GeneratedPaint::Text(true)],
        _ => return None,
    }
    Some(paints)
}

fn annotation_rect(pdf: &PdfDocument, value: &Object) -> Option<[f32; 4]> {
    let value = pdf.resolve_object(value).ok()?;
    let mut rect = [0.0; 4];
    let values = value.as_array().filter(|values| values.len() == 4)?;
    for (point, number) in rect.iter_mut().zip(values) {
        *point = match number {
            Object::Integer(value) => *value as f32,
            Object::Real(value) => *value as f32,
            _ => return None,
        };
        if !point.is_finite() {
            return None;
        }
    }
    Some(rect)
}

fn projected(matrix: Matrix, [x0, y0, x1, y1]: [f32; 4]) -> [f32; 4] {
    let mut bounds = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    for (x, y) in [(x0, y0), (x0, y1), (x1, y0), (x1, y1)] {
        let p = matrix.transform_point(x, y);
        bounds[0] = bounds[0].min(p.x);
        bounds[1] = bounds[1].min(p.y);
        bounds[2] = bounds[2].max(p.x);
        bounds[3] = bounds[3].max(p.y);
    }
    bounds
}

pub(super) fn other_paints(op: &Operator) -> usize {
    match op {
        Operator::Fill
        | Operator::FillEvenOdd
        | Operator::Stroke
        | Operator::CloseAndStroke
        | Operator::PaintShading { .. } => 1,
        Operator::FillStroke
        | Operator::FillStrokeEvenOdd
        | Operator::CloseFillStroke
        | Operator::CloseFillStrokeEvenOdd => 2,
        _ => 0,
    }
}
