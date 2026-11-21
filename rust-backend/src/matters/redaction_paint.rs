//! Preserve a combined fill and stroke while rebuilding its covered contours.
use super::{redaction_clip, redaction_vectors};
use kurbo::{Affine, BezPath, PathEl, Point, Shape, Stroke, StrokeOpts};
use pdf_oxide::content::{parse_content_stream, Matrix, Operator};
use std::{fmt::Write as _, ops::Range};

const INVALID: &str = "Cannot safely reconstruct the PDF paint outside the redaction";
const TOLERANCE: f64 = 0.015;
const MAX_POINTS: usize = 1_024;
// At 72 DPI this is at most three square pixels of estimated stroke coverage.
const MAX_OUTLINE_CHANGE: f64 = 3.0;
const LENGTH_EPSILON: f64 = 1e-8;

type Bounds = [f64; 4];

struct Contour {
    points: Vec<Point>,
    closed: bool,
}

struct PositionedPoint {
    point: Point,
    distance: f64,
}

struct DashedStroke {
    cycle: f64,
    to_local: Affine,
    margin: f64,
}

pub(super) fn reconstruct(
    data: &[u8],
    geometry: &[Range<usize>],
    paint: &Range<usize>,
    media: Bounds,
    matrix: Matrix,
    regions: &[Bounds],
    style: &Stroke,
) -> Result<String, &'static str> {
    let (even_odd, close_last) = match &data[paint.clone()] {
        b"B" => (false, false),
        b"B*" => (true, false),
        b"b" => (false, true),
        b"b*" => (true, true),
        _ => return Err(INVALID),
    };
    let operators =
        parse_content_stream(&data[geometry[0].start..paint.start]).map_err(|_| INVALID)?;
    if operators.iter().any(|op| {
        matches!(
            op,
            Operator::Cm { .. } | Operator::SaveState | Operator::RestoreState
        )
    }) {
        return Err(INVALID);
    }
    let [a, b, c, d, e, f] =
        [matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f].map(f64::from);
    let transform = Affine::new([a, b, c, d, e, f]);
    if !transform.is_finite() || transform.determinant().abs() < 1e-12 {
        return Err(INVALID);
    }
    let mut source = redaction_clip::extract_path(data, geometry)?;
    if close_last && source.elements().last() != Some(&PathEl::ClosePath) {
        source.close_path();
    }
    let in_page = transform * &source;
    let bounds = in_page.bounding_box();
    if !bounds.is_finite() || bounds.width().max(bounds.height()) > 1_000_000.0 {
        return Err(INVALID);
    }
    let mut contours = flatten(&in_page)?;
    let local = transform.inverse();
    let scale = a.hypot(b).hypot(c.hypot(d)).max(1.0);
    let dashed = (!style.dash_pattern.is_empty()).then(|| DashedStroke {
        cycle: style.dash_pattern.iter().sum(),
        to_local: local,
        margin: style.width * scale * 0.5 + TOLERANCE,
    });
    let before = contour_path(&contours, local)?;
    let mut check_outline = dashed.is_none()
        || source
            .elements()
            .iter()
            .any(|element| matches!(element, PathEl::QuadTo(..) | PathEl::CurveTo(..)));
    for &region in regions {
        for contour in &mut contours {
            check_outline |= contour.erase(region, dashed.as_ref())?;
        }
        if contours
            .iter()
            .map(|contour| contour.points.len())
            .sum::<usize>()
            > MAX_POINTS
        {
            return Err(INVALID);
        }
    }
    let candidate = contour_path(&contours, local)?;
    // A straight run with the same arclength still has the same cap, join
    // and dash positions. Changes in contour shape require an outline check.
    if check_outline {
        validate_outline(
            &before,
            &candidate,
            transform,
            scale,
            style,
            dashed.as_ref(),
            regions,
        )?;
    }
    if candidate.is_empty() {
        return Ok(String::new());
    }
    let [left, bottom, right, top] = media;
    let outer = [[left, bottom], [right, bottom], [right, top], [left, top]]
        .into_iter()
        .map(|[x, y]| {
            let point = local * Point::new(x, y);
            [point.x, point.y]
        })
        .collect();
    let Some(clip) = redaction_vectors::redact_polygons(vec![outer], matrix, regions)? else {
        return Ok(String::new());
    };
    let mut commands = String::new();
    for element in candidate.elements() {
        match *element {
            PathEl::MoveTo(point) => {
                let _ = writeln!(commands, "{:.10} {:.10} m", point.x, point.y);
            }
            PathEl::LineTo(point) => {
                let _ = writeln!(commands, "{:.10} {:.10} l", point.x, point.y);
            }
            PathEl::ClosePath => commands.push_str("h\n"),
            _ => return Err(INVALID),
        }
    }
    let paint = if even_odd { "B*" } else { "B" };
    Ok(format!("q\n{clip}W n\n{commands}{paint}\nQ\n"))
}

fn validate_outline(
    before: &BezPath,
    after: &BezPath,
    transform: Affine,
    scale: f64,
    style: &Stroke,
    dash: Option<&DashedStroke>,
    regions: &[Bounds],
) -> Result<(), &'static str> {
    let tolerance = TOLERANCE / scale;
    if style.width * scale > 1_000_000.0 {
        return Err(INVALID);
    }
    if let Some(dash) = dash {
        let length = before.perimeter(tolerance).max(after.perimeter(tolerance));
        let subpaths = before
            .elements()
            .iter()
            .filter(|element| matches!(element, PathEl::MoveTo(_)))
            .count();
        let fragments = (length / dash.cycle + subpaths as f64) * style.dash_pattern.len() as f64;
        if !fragments.is_finite() || fragments > MAX_POINTS as f64 {
            return Err(INVALID);
        }
    }
    let outline = |path: &BezPath| {
        transform * kurbo::stroke(path.iter(), style, &StrokeOpts::default(), tolerance)
    };
    let before = outline(before);
    let after = outline(after);
    if !before.is_finite() || !after.is_finite() {
        return Err(INVALID);
    }
    // A synthetic chord preserves fill winding outside its rectangle, while
    // its stroke can reach across the edge. Check the exterior outlines.
    let mut changed_area = 0.0;
    redaction_clip::stroke_difference(&before, &after, TOLERANCE, |fragment| {
        let visible = redaction_vectors::subtract_regions(vec![fragment.to_vec()], regions)?;
        changed_area += visible.iter().map(|polygon| area(polygon)).sum::<f64>();
        if !changed_area.is_finite() || changed_area > MAX_OUTLINE_CHANGE {
            return Err(INVALID);
        }
        Ok(())
    })
}

fn contour_path(contours: &[Contour], transform: Affine) -> Result<BezPath, &'static str> {
    let mut path = BezPath::new();
    for contour in contours {
        let [first, rest @ ..] = contour.points.as_slice() else {
            continue;
        };
        if rest.is_empty() {
            continue;
        }
        path.move_to(transform * *first);
        for &point in rest {
            path.line_to(transform * point);
        }
        if contour.closed {
            path.close_path();
        }
    }
    path.is_finite().then_some(path).ok_or(INVALID)
}

fn flatten(path: &BezPath) -> Result<Vec<Contour>, &'static str> {
    let mut contours = Vec::new();
    let mut current: Option<Contour> = None;
    let mut accepted = true;
    let mut count = 0;
    kurbo::flatten(path.iter(), TOLERANCE, |element| {
        count += 1;
        if !accepted || count > MAX_POINTS {
            accepted = false;
            return;
        }
        match element {
            PathEl::MoveTo(point) => {
                contours.extend(current.take());
                current = Some(Contour {
                    points: vec![point],
                    closed: false,
                });
            }
            PathEl::LineTo(point) => {
                if let Some(contour) = current.as_mut().filter(|contour| !contour.closed) {
                    contour.points.push(point);
                } else {
                    accepted = false;
                }
            }
            PathEl::ClosePath => {
                if let Some(contour) = current.as_mut() {
                    contour.closed = true;
                } else {
                    accepted = false;
                }
            }
            _ => accepted = false,
        }
    });
    contours.extend(current);
    accepted.then_some(contours).ok_or(INVALID)
}

impl Contour {
    fn erase(&mut self, rect: Bounds, dash: Option<&DashedStroke>) -> Result<bool, &'static str> {
        if self.points.len() < 2 {
            return Ok(false);
        }
        let first = self.points[0];
        let last = *self.points.last().ok_or(INVALID)?;
        if self.points.iter().all(|point| *point == first) {
            return Err(INVALID);
        }
        let inside = |point: Point| {
            point.x > rect[0] && point.x < rect[2] && point.y > rect[1] && point.y < rect[3]
        };
        // Moving an open endpoint would change its cap and the implicit
        // fill closure, which can cross the exterior of the selection.
        if !self.closed && (inside(first) || inside(last)) {
            return Err(INVALID);
        }
        let mut exterior: Vec<PositionedPoint> = Vec::new();
        let mut add = |point, distance| {
            if exterior
                .last()
                .is_none_or(|last| last.point != point || last.distance != distance)
            {
                exterior.push(PositionedPoint { point, distance });
            }
        };
        let segments = self.points.windows(2).map(|pair| (pair[0], pair[1]));
        let mut distance = 0.0;
        for (start, end) in segments.chain(self.closed.then_some((last, first))) {
            let length = dash.map_or(0.0, |dash| dash.distance(start, end));
            if let Some((enter, exit)) = interior_interval(start, end, rect) {
                if enter > 0.0 {
                    add(start, distance);
                    add(start.lerp(end, enter), distance + length * enter);
                } else if !inside(start) {
                    add(start, distance);
                }
                if exit < 1.0 {
                    add(start.lerp(end, exit), distance + length * exit);
                    add(end, distance + length);
                } else if !inside(end) {
                    add(end, distance + length);
                }
            } else {
                add(start, distance);
                add(end, distance + length);
            }
            distance += length;
        }
        let changed_shape = if let Some(dash) = dash {
            let (points, changed) =
                dash.reconnect(&exterior, distance, rect, self.closed, inside(first))?;
            self.points = points;
            changed
        } else {
            self.points = exterior.into_iter().map(|point| point.point).collect();
            true
        };
        if self.closed && self.points.len() > 1 && self.points.first() == self.points.last() {
            self.points.pop();
        }
        Ok(changed_shape)
    }
}

impl DashedStroke {
    fn distance(&self, start: Point, end: Point) -> f64 {
        let [a, b, c, d, _, _] = self.to_local.as_coeffs();
        let delta = end - start;
        (a * delta.x + c * delta.y).hypot(b * delta.x + d * delta.y)
    }

    fn reconnect(
        &self,
        exterior: &[PositionedPoint],
        length: f64,
        rect: Bounds,
        closed: bool,
        start_was_covered: bool,
    ) -> Result<(Vec<Point>, bool), &'static str> {
        let Some(first) = exterior.first() else {
            return Ok((Vec::new(), true));
        };
        let last = exterior.last().ok_or(INVALID)?;
        let mut changed = start_was_covered;
        let mut points = if start_was_covered {
            // Restart at an invented interior point, then reach the first
            // exterior point at its original phase. The closing leg keeps
            // its own phase so the stroke also wraps correctly at this seam.
            let [left, bottom, right, top] = self.inset(rect)?;
            let mid = first.point.midpoint(last.point);
            let mut points = vec![Point::new(
                mid.x.clamp(left, right),
                mid.y.clamp(bottom, top),
            )];
            self.connect(&mut points, first.point, first.distance, rect)?;
            points
        } else {
            vec![first.point]
        };
        for pair in exterior.windows(2) {
            changed |= self.connect(
                &mut points,
                pair[1].point,
                pair[1].distance - pair[0].distance,
                rect,
            )?;
        }
        if closed {
            let closing_length = length - last.distance
                + if start_was_covered {
                    0.0
                } else {
                    first.distance
                };
            let start = points[0];
            changed |= self.connect(&mut points, start, closing_length, rect)?;
        }
        Ok((points, changed))
    }

    fn inset(&self, [left, bottom, right, top]: Bounds) -> Result<Bounds, &'static str> {
        let bounds = [
            left + self.margin,
            bottom + self.margin,
            right - self.margin,
            top - self.margin,
        ];
        (bounds[0] < bounds[2] && bounds[1] < bounds[3])
            .then_some(bounds)
            .ok_or(INVALID)
    }

    fn connect(
        &self,
        points: &mut Vec<Point>,
        end: Point,
        original_length: f64,
        rect: Bounds,
    ) -> Result<bool, &'static str> {
        let start = *points.last().ok_or(INVALID)?;
        let direct = self.distance(start, end);
        if (original_length - direct).abs() <= LENGTH_EPSILON {
            if start != end {
                points.push(end);
            }
            return Ok(false);
        }
        let [left, bottom, right, top] = rect;
        let in_selection = |point: Point| {
            (left - LENGTH_EPSILON..=right + LENGTH_EPSILON).contains(&point.x)
                && (bottom - LENGTH_EPSILON..=top + LENGTH_EPSILON).contains(&point.y)
        };
        if !original_length.is_finite()
            || original_length < -LENGTH_EPSILON
            || !in_selection(start)
            || !in_selection(end)
        {
            return Err(INVALID);
        }

        // Both endpoints of an erased run lie in this convex rectangle.
        // Use a synthetic waypoint and spend only the missing cycle remainder
        // on interior travel. No source vertex or full hidden length is saved.
        let [left, bottom, right, top] = self.inset(rect)?;
        let mid = start.midpoint(end);
        let pivot = Point::new(mid.x.clamp(left, right), mid.y.clamp(bottom, top));
        let base = self.distance(start, pivot) + self.distance(pivot, end);
        let mut remainder = (original_length - base).rem_euclid(self.cycle);
        if self.cycle - remainder <= LENGTH_EPSILON {
            remainder = 0.0;
        }
        let corner = [
            Point::new(left, bottom),
            Point::new(left, top),
            Point::new(right, bottom),
            Point::new(right, top),
        ]
        .into_iter()
        .max_by(|&a, &b| self.distance(a, pivot).total_cmp(&self.distance(b, pivot)))
        .ok_or(INVALID)?;
        let round_trip = 2.0 * self.distance(pivot, corner);
        if !round_trip.is_finite() || round_trip <= LENGTH_EPSILON {
            return Err(INVALID);
        }
        if start != pivot {
            points.push(pivot);
        }
        while remainder > LENGTH_EPSILON {
            if points.len() >= MAX_POINTS - 2 {
                return Err(INVALID);
            }
            let length = remainder.min(round_trip);
            points.push(pivot.lerp(corner, length / round_trip));
            points.push(pivot);
            remainder -= length;
        }
        if pivot != end {
            points.push(end);
        }
        Ok(true)
    }
}

fn interior_interval(start: Point, end: Point, rect: Bounds) -> Option<(f64, f64)> {
    let [left, bottom, right, top] = rect;
    let mut enter: f64 = 0.0;
    let mut exit: f64 = 1.0;
    for (start, end, low, high) in [(start.x, end.x, left, right), (start.y, end.y, bottom, top)] {
        let delta = end - start;
        if delta == 0.0 {
            if start <= low || start >= high {
                return None;
            }
        } else {
            let a = (low - start) / delta;
            let b = (high - start) / delta;
            enter = enter.max(a.min(b));
            exit = exit.min(a.max(b));
        }
    }
    (enter < exit).then_some((enter, exit))
}

fn area(polygon: &[[f64; 2]]) -> f64 {
    let Some((&[x, y], rest)) = polygon.split_first() else {
        return 0.0;
    };
    rest.windows(2)
        .map(|edge| (edge[0][0] - x) * (edge[1][1] - y) - (edge[1][0] - x) * (edge[0][1] - y))
        .sum::<f64>()
        .abs()
        * 0.5
}
