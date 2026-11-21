use super::{bbox, displayed, first_glyph, intersects, refusal, Rect, Reply, INVALID};
use crate::matters::page_tools::{
    assembly::{self, DisplayFrame},
    pdf_annotations,
};
use pdf_oxide::{
    layout::{TextChar, TextSpan},
    object::Object,
    PdfDocument,
};
use serde_json::Value;

pub(super) fn move_offsets(body: &Value) -> Result<(f64, f64), Reply> {
    let invalid = || refusal(422, INVALID);
    let horizontal = body
        .get("horizontal_offset")
        .map_or(Some(0.0), Value::as_f64)
        .filter(|value| (-1.0..=1.0).contains(value))
        .ok_or_else(invalid)?;
    let vertical = body["vertical_offset"]
        .as_f64()
        .filter(|value| (-1.0..=1.0).contains(value))
        .ok_or_else(invalid)?;
    if horizontal.abs().max(vertical.abs()) < 0.0001 {
        return Err(refusal(
            422,
            "Drag the text to a new position before applying the move",
        ));
    }
    Ok((horizontal, vertical))
}

#[derive(Default)]
pub(super) struct RunAnnotations {
    pub links: Vec<Rect>,
    pub removed: Vec<usize>,
}

pub(super) fn page_bounds(frame: DisplayFrame) -> Rect {
    assembly::transform_box(frame.paint_matrix(), [0.0, 0.0, frame.size.0, frame.size.1])
}

pub(super) fn visual_point(frame: DisplayFrame, x: f64, y: f64) -> (f64, f64) {
    let [left, _, _, top] = frame.visible.crop;
    ((x - left) * frame.unit, (top - y) * frame.unit)
}

fn rise(span: &TextSpan, glyph: Option<&TextChar>) -> (f64, f64) {
    let Some([_, b, c, d, _, _]) = glyph.and_then(|glyph| glyph.matrix) else {
        return (0.0, 0.0);
    };
    let vertical = f64::from(b.hypot(d));
    let rise = f64::from(span.text_rise) * f64::from(span.font_size) / vertical;
    let shift = [c, d].map(|axis| f64::from(axis) * rise);
    if shift.into_iter().all(f64::is_finite) {
        (shift[0], shift[1])
    } else {
        (0.0, 0.0)
    }
}

pub(super) fn selection_bounds(span: &TextSpan, chars: &[TextChar]) -> Rect {
    let [left, bottom, right, top] = bbox(span);
    if span.text_rise == 0.0 {
        return [left, bottom, right, top];
    }
    let (x, y) = rise(span, first_glyph(span, chars));
    [left + x, bottom + y, right + x, top + y]
}

pub(super) fn origin(span: &TextSpan, glyph: &TextChar) -> (f64, f64) {
    let (x, y) = rise(span, Some(glyph));
    (f64::from(glyph.origin_x) + x, f64::from(glyph.origin_y) + y)
}

pub(super) fn font_size(span: &TextSpan, glyph: Option<&TextChar>, horizontal: f64) -> f64 {
    let height = f64::from(span.font_size);
    let Some([a, b, c, d, _, _]) = glyph.and_then(|glyph| glyph.matrix) else {
        return height;
    };
    let [a, b, c, d] = [a, b, c, d].map(f64::from);
    // The selected-text tool reports one scalar size and redraws with that
    // size. Match MuPDF's area expansion of the original run's transform.
    let size = height / b.hypot(d) * (((a * d) - (b * c)) * horizontal).abs().sqrt();
    if size.is_finite() && size > 0.0 {
        size
    } else {
        height
    }
}

pub(super) fn relative_bounds(frame: DisplayFrame, bounds: Rect) -> Rect {
    let [a, b, c, d, e, f] = frame.paint_matrix();
    let [a, b, c, d] = [a, c, b, d].map(|coefficient| (coefficient * frame.unit) * frame.unit);
    let [left, bottom, right, top] =
        assembly::transform_box([a, b, c, d, -a * e - c * f, -b * e - d * f], bounds);
    [
        left / frame.size.0,
        1.0 - top / frame.size.1,
        right / frame.size.0,
        1.0 - bottom / frame.size.1,
    ]
}

pub(super) fn redraw_baseline(frame: DisplayFrame, x: f64, y: f64) -> (f64, f64) {
    let (x, y) = visual_point(frame, x, y);
    let [left, _, _, top] = frame.geometry.crop;
    (left + x, top - y)
}

pub(super) fn annotations(
    pdf: &PdfDocument,
    page: usize,
    frame: DisplayFrame,
    selected: Rect,
    moving: bool,
) -> Result<RunAnnotations, Reply> {
    let invalid = |_| refusal(422, INVALID);
    let entries = pdf_annotations::annotations(pdf, page).map_err(invalid)?;
    let mut result = RunAnnotations::default();
    for (index, entry) in entries.into_iter().enumerate() {
        let subtype = pdf_annotations::field(pdf, &entry, &["Subtype"]).map_err(invalid)?;
        let subtype = subtype.as_ref().and_then(Object::as_name);
        if subtype == Some("Redact") {
            return Err(refusal(
                422,
                if moving {
                    "This page has pending redaction marks. Review them before moving text."
                } else {
                    "This page has pending redaction marks. Text replacement would apply them; review them outside this tool first."
                },
            ));
        }
        if !matches!(subtype, Some("Link" | "FreeText")) {
            continue;
        }
        let rect = pdf_annotations::field(pdf, &entry, &["Rect"]).map_err(invalid)?;
        let Some(rect) = rect
            .as_ref()
            .and_then(Object::as_array)
            .filter(|r| r.len() == 4)
        else {
            continue;
        };
        let mut coords = [0.0; 4];
        for (coord, value) in coords.iter_mut().zip(rect) {
            *coord = match pdf.resolve_object(value).map_err(invalid)? {
                Object::Integer(value) => value as f64,
                Object::Real(value) => value,
                _ => 0.0,
            };
        }
        let [x0, y0, x1, y1] = coords;
        let coords = [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)];
        // Applying the text redaction removes touched links and FreeText
        // annotations in source PDF coordinates.
        if intersects(selected, coords) {
            result.removed.push(index);
        }
        if !pdf_annotations::has_link(pdf, &entry).map_err(invalid)? {
            continue;
        }
        let [x0, y0, x1, y1] = displayed(frame.visible, coords);
        // The reference exposes link bounds in display coordinates and
        // compares those numbers with the unrotated text run.
        let (width, height) = frame.visible.display_size();
        let [left, _, _, top] = frame.visible.crop;
        result.links.push([
            left + x0 * width,
            top - y1 * height,
            left + x1 * width,
            top - y0 * height,
        ]);
    }
    Ok(result)
}
