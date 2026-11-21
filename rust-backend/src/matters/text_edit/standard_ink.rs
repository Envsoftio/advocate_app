use super::Rect;

include!("standard_ink_data.rs");

pub(super) fn face(name: &str) -> Option<usize> {
    Some(match name {
        "Helvetica" => 0,
        "Helvetica-Bold" => 1,
        "Helvetica-Oblique" => 2,
        "Helvetica-BoldOblique" => 3,
        "Times-Roman" => 4,
        "Times-Bold" => 5,
        "Times-Italic" => 6,
        "Times-BoldItalic" => 7,
        "Courier" => 8,
        "Courier-Bold" => 9,
        "Courier-Oblique" => 10,
        "Courier-BoldOblique" => 11,
        _ => return None,
    })
}

fn metric(face: usize, byte: u8) -> Option<[i32; 5]> {
    let glyph = usize::from(byte.checked_sub(32)?);
    Some(METRICS[face][glyph].map(i32::from))
}

pub(super) fn glyph_bounds(face: usize, byte: u8, size: f64) -> Option<Rect> {
    let [_, x0, y0, x1, y1] = metric(face, byte)?;
    Some([x0, y0, x1, y1].map(|coordinate| f64::from(coordinate) * size / 1000.0))
}

pub(super) fn bounds(name: &str, encoded: &[u8], size: f64) -> Option<Rect> {
    let face = face(name)?;
    let mut cursor = 0i32;
    let mut ink = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
    for &byte in encoded {
        let [advance, x0, y0, x1, y1] = metric(face, byte)?;
        if x0 != x1 || y0 != y1 {
            ink = [
                ink[0].min(cursor + x0),
                ink[1].min(y0),
                ink[2].max(cursor + x1),
                ink[3].max(y1),
            ];
        }
        cursor += advance;
    }
    (ink[0] != i32::MAX).then(|| ink.map(|coordinate| f64::from(coordinate) * size / 1000.0))
}
