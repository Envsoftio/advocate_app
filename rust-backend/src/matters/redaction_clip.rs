//! Rebuild a clipping path without its geometry in selected redaction areas.
use super::redaction_vectors::redact_polygons;
use kurbo::{BezPath, PathEl, Point, Shape};
use pdf_oxide::content::{parse_content_stream, Matrix, Operator};
use std::ops::Range;

const INVALID: &str = "Cannot safely reconstruct the shared PDF clipping path";
const MAX_EDGES: usize = 1_024;
const MAX_BANDS: usize = 4_096;
const MAX_FRAGMENTS: usize = 4_096;

type Bounds = [f64; 4];

struct Edge {
    low: Point,
    high: Point,
    winding: i32,
}

impl Edge {
    fn at(&self, y: f64) -> f64 {
        self.low.x + (self.high.x - self.low.x) * (y - self.low.y) / (self.high.y - self.low.y)
    }

    fn crossing(&self, other: &Self) -> Option<f64> {
        let low = self.low.y.max(other.low.y);
        let high = self.high.y.min(other.high.y);
        if low >= high {
            return None;
        }
        let before = self.at(low) - other.at(low);
        let after = self.at(high) - other.at(high);
        if before * after < 0.0 {
            Some(low + (high - low) * (before / (before - after)))
        } else {
            None
        }
    }
}

/// Return a replacement clip when the path touches the redaction. The
/// selected geometry is absent from the reconstructed trapezoids.
pub(super) fn outside(
    data: &[u8],
    geometry: &[Range<usize>],
    clip: &Range<usize>,
    matrix: Matrix,
    regions: &[Bounds],
) -> Result<Option<String>, &'static str> {
    let Some(first) = geometry.first() else {
        return Ok(None);
    };
    let ops = parse_content_stream(&data[first.start..clip.start]).map_err(|_| INVALID)?;
    if ops.iter().any(|op| {
        matches!(
            op,
            Operator::Cm { .. } | Operator::SaveState | Operator::RestoreState
        )
    }) {
        return Err(INVALID);
    }
    let path = extract_path(data, geometry)?;
    let rect = path.bounding_box();
    let [a, b, c, d, e, f] =
        [matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f].map(f64::from);
    let mut bounds = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for (x, y) in [
        (rect.x0, rect.y0),
        (rect.x0, rect.y1),
        (rect.x1, rect.y0),
        (rect.x1, rect.y1),
    ] {
        let (x, y) = (a * x + c * y + e, b * x + d * y + f);
        bounds[0] = bounds[0].min(x);
        bounds[1] = bounds[1].min(y);
        bounds[2] = bounds[2].max(x);
        bounds[3] = bounds[3].max(y);
    }
    if !bounds.iter().all(|coordinate| coordinate.is_finite()) {
        return Err(INVALID);
    }
    if !regions
        .iter()
        .any(|region| super::text_edit::intersects(*region, bounds))
    {
        return Ok(None);
    }
    let scale = a.hypot(b).max(c.hypot(d)).max(1.0);
    let tolerance = (0.04 / scale).max(0.0001);
    if rect.width().max(rect.height()) / tolerance > 100_000_000.0 {
        return Err(INVALID);
    }
    let edges = edges(&path, tolerance)?;
    let even_odd = &data[clip.clone()] == b"W*";
    let pieces = trapezoids(&edges, even_odd)?;
    Ok(Some(match redact_polygons(pieces, matrix, regions)? {
        Some(commands) => format!("{commands}W n\n"),
        None => "0 0 0 0 re W n\n".into(),
    }))
}

pub(super) fn extract_path(
    data: &[u8],
    geometry: &[Range<usize>],
) -> Result<BezPath, &'static str> {
    let mut path = BezPath::new();
    let mut current = None;
    let mut start = None;
    let point = |x: f32, y: f32| Point::new(x.into(), y.into());
    if geometry.len() > MAX_EDGES {
        return Err(INVALID);
    }
    for range in geometry {
        let operation = parse_content_stream(&data[range.clone()]).map_err(|_| INVALID)?;
        match operation.as_slice() {
            [Operator::MoveTo { x, y }] => {
                let at = point(*x, *y);
                path.move_to(at);
                start = Some(at);
                current = start;
            }
            [Operator::LineTo { x, y }] if current.is_some() => {
                let at = point(*x, *y);
                path.line_to(at);
                current = Some(at);
            }
            [Operator::CurveTo {
                x1,
                y1,
                x2,
                y2,
                x3,
                y3,
            }] if current.is_some() => {
                let end = point(*x3, *y3);
                path.curve_to(point(*x1, *y1), point(*x2, *y2), end);
                current = Some(end);
            }
            [Operator::CurveToV { x2, y2, x3, y3 }] => {
                let end = point(*x3, *y3);
                path.curve_to(current.ok_or(INVALID)?, point(*x2, *y2), end);
                current = Some(end);
            }
            [Operator::CurveToY { x1, y1, x3, y3 }] if current.is_some() => {
                let end = point(*x3, *y3);
                path.curve_to(point(*x1, *y1), end, end);
                current = Some(end);
            }
            [Operator::Rectangle {
                x,
                y,
                width,
                height,
            }] => {
                let at = point(*x, *y);
                path.move_to(at);
                path.line_to(point(x + width, *y));
                path.line_to(point(x + width, y + height));
                path.line_to(point(*x, y + height));
                path.close_path();
                start = Some(at);
                current = start;
            }
            [Operator::ClosePath] if current.is_some() => {
                path.close_path();
                current = start;
            }
            _ => return Err(INVALID),
        }
    }
    if !path.is_finite() {
        return Err(INVALID);
    }
    Ok(path)
}

fn edges(path: &BezPath, tolerance: f64) -> Result<Vec<Edge>, &'static str> {
    let mut edges = Vec::new();
    let mut start = None;
    let mut current = None;
    let mut overflow = false;
    let mut add = |from: Point, to: Point| {
        if from.y == to.y {
            return;
        }
        if edges.len() >= MAX_EDGES {
            overflow = true;
            return;
        }
        let (low, high, winding) = if from.y < to.y {
            (from, to, 1)
        } else {
            (to, from, -1)
        };
        edges.push(Edge { low, high, winding });
    };
    kurbo::flatten(path.iter(), tolerance, |element| match element {
        PathEl::MoveTo(at) => {
            if let Some((from, to)) = current.zip(start) {
                add(from, to);
            }
            start = Some(at);
            current = start;
        }
        PathEl::LineTo(at) => {
            if let Some(from) = current {
                add(from, at);
            }
            current = Some(at);
        }
        PathEl::ClosePath => {
            if let Some((from, to)) = current.zip(start) {
                add(from, to);
            }
            current = start;
        }
        _ => {}
    });
    if let Some((from, to)) = current.zip(start) {
        add(from, to);
    }
    if overflow {
        return Err(INVALID);
    }
    Ok(edges)
}

fn trapezoids(edges: &[Edge], even_odd: bool) -> Result<Vec<Vec<[f64; 2]>>, &'static str> {
    let inside = |[winding, _]: [i32; 2]| {
        if even_odd {
            winding % 2 != 0
        } else {
            winding != 0
        }
    };
    let mut result = Vec::new();
    sweep(edges, &[], inside, |fragment| {
        if result.len() == MAX_FRAGMENTS {
            return Err(INVALID);
        }
        result.push(fragment.to_vec());
        Ok(())
    })?;
    Ok(result)
}

pub(super) fn stroke_difference(
    before: &BezPath,
    after: &BezPath,
    tolerance: f64,
    visit: impl FnMut([[f64; 2]; 4]) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    let left = edges(before, tolerance)?;
    let right = edges(after, tolerance)?;
    sweep(&left, &right, |[a, b]| (a != 0) != (b != 0), visit)
}

fn sweep(
    left: &[Edge],
    right: &[Edge],
    inside: impl Fn([i32; 2]) -> bool,
    mut visit: impl FnMut([[f64; 2]; 4]) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    if left.len() + right.len() > MAX_EDGES * 2 {
        return Err(INVALID);
    }
    let edges: Vec<_> = left
        .iter()
        .map(|edge| (0, edge))
        .chain(right.iter().map(|edge| (1, edge)))
        .collect();
    let mut ys = edges
        .iter()
        .flat_map(|(_, edge)| [edge.low.y, edge.high.y])
        .collect::<Vec<_>>();
    for (index, (_, edge)) in edges.iter().enumerate() {
        for (_, other) in &edges[index + 1..] {
            if let Some(crossing) = edge.crossing(other) {
                ys.push(crossing);
                if ys.len() > MAX_BANDS {
                    return Err(INVALID);
                }
            }
        }
    }
    ys.sort_by(f64::total_cmp);
    ys.dedup();
    for window in ys.windows(2) {
        let [low, high] = [window[0], window[1]];
        let mid = (low + high) * 0.5;
        if mid <= low || mid >= high {
            continue;
        }
        let mut active: Vec<_> = edges
            .iter()
            .filter(|(_, edge)| edge.low.y < mid && mid < edge.high.y)
            .map(|&(group, edge)| (edge.at(mid), group, edge))
            .collect();
        active.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut winding = [0; 2];
        let mut left: Option<&Edge> = None;
        for (_, group, edge) in active {
            let was_inside = inside(winding);
            winding[group] += edge.winding;
            if !was_inside && inside(winding) {
                left = Some(edge);
            } else if was_inside && !inside(winding) {
                let left = left.take().ok_or(INVALID)?;
                if left.at(mid) < edge.at(mid) {
                    visit([
                        [left.at(low), low],
                        [edge.at(low), low],
                        [edge.at(high), high],
                        [left.at(high), high],
                    ])?;
                }
            }
        }
        if winding != [0; 2] {
            return Err(INVALID);
        }
    }
    Ok(())
}
