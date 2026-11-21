//! Remove covered vector path operators while preserving the rest of the stream.
use super::page_tools::assembly::{
    self, Dictionary, Geometry, Overlay, XObjectEdit, XObjectReplacement,
};
use kurbo::{Cap, Join, Stroke};
use pdf_oxide::{
    content::{parse_content_stream, Matrix, Operator},
    object::Object,
    PdfDocument,
};
use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    ops::Range,
};

const UNSUPPORTED: &str = "Selected vector path cannot be removed safely";
const PARTIAL_PATH: &str =
    "The redaction intersects a vector path that extends outside the selected region";

type Bounds = [f64; 4];

#[derive(Default)]
struct PathCommands {
    ranges: Vec<Range<usize>>,
    clip: Option<Range<usize>>,
}

fn delimiter(value: u8) -> bool {
    value == 0 || value.is_ascii_whitespace() || b"()<>[]{}/%".contains(&value)
}

fn literal(data: &[u8], index: &mut usize) -> Result<(), &'static str> {
    let mut depth = 1usize;
    *index += 1;
    while let Some(&byte) = data.get(*index) {
        *index += 1;
        match byte {
            b'\\' => *index += 1,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(());
                }
            }
            _ => {}
        }
    }
    Err(UNSUPPORTED)
}

#[derive(Default)]
struct StreamCommands {
    paths: Vec<PathCommands>,
    draws: Vec<Range<usize>>,
    shadings: Vec<Range<usize>>,
    clip_paths: Vec<(usize, PathCommands)>,
    path_ends: usize,
    pattern_colors: Vec<Range<usize>>,
    ext_states: Vec<Range<usize>>,
    text_shows: Vec<Range<usize>>,
    text_matrices: Vec<Range<usize>>,
    text_offsets: Vec<Range<usize>>,
    text_modes: Vec<(Range<usize>, u8)>,
    marked_properties: Vec<Range<usize>>,
    inline_images: Vec<(Range<usize>, Object)>,
    scopes: Vec<Range<usize>>,
}

fn stream_commands(data: &[u8]) -> Result<StreamCommands, &'static str> {
    stream_commands_with_scopes(data, false)
}

fn stream_commands_with_scopes(data: &[u8], scopes: bool) -> Result<StreamCommands, &'static str> {
    let mut result = StreamCommands::default();
    let mut pending = PathCommands::default();
    let mut values = None;
    let mut position = 0;
    let mut levels = Vec::new();
    while let Some(&byte) = data.get(position) {
        let start = position;
        position += 1;
        match byte {
            b'%' => {
                while data
                    .get(position)
                    .is_some_and(|&byte| !matches!(byte, b'\n' | b'\r'))
                {
                    position += 1;
                }
                continue;
            }
            b if b == 0 || b.is_ascii_whitespace() => continue,
            b'(' => {
                position = start;
                literal(data, &mut position)?;
            }
            b'/' => {
                while data.get(position).is_some_and(|&byte| !delimiter(byte)) {
                    position += 1;
                }
            }
            b'<' if data.get(position) == Some(&b'<') => {
                levels.push(b'>');
                position += 1;
            }
            b'>' if data.get(position) == Some(&b'>') => {
                if levels.pop() != Some(b'>') {
                    return Err(UNSUPPORTED);
                }
                position += 1;
            }
            b'<' => {
                let end = data[position..]
                    .iter()
                    .position(|&byte| byte == b'>')
                    .ok_or(UNSUPPORTED)?;
                position += end + 1;
            }
            b'[' => levels.push(b']'),
            b']' if levels.pop() != Some(b']') => return Err(UNSUPPORTED),
            b']' => {}
            b')' | b'>' | b'{' | b'}' => return Err(UNSUPPORTED),
            _ => {
                while data.get(position).is_some_and(|&byte| !delimiter(byte)) {
                    position += 1;
                }
                let word = &data[start..position];
                if !levels.is_empty() {
                    continue;
                }
                if std::str::from_utf8(word).is_ok_and(|word| {
                    word.parse::<f64>().is_ok() || matches!(word, "true" | "false" | "null")
                }) {
                    values.get_or_insert(start);
                    continue;
                }
                let range = values.take().unwrap_or(start)..position;
                if scopes
                    && matches!(
                        word,
                        b"q" | b"Q" | b"BT" | b"ET" | b"BX" | b"EX" | b"BMC" | b"BDC" | b"EMC"
                    )
                {
                    result.scopes.push(range.clone());
                }
                match word {
                    b"m" | b"l" | b"c" | b"v" | b"y" | b"re" | b"h" => pending.ranges.push(range),
                    b"W" | b"W*" => pending.clip = Some(range),
                    b"n" => {
                        if pending.clip.is_some() {
                            pending.ranges.push(range);
                            result
                                .clip_paths
                                .push((result.path_ends, std::mem::take(&mut pending)));
                        } else {
                            pending = PathCommands::default();
                        }
                        result.path_ends += 1;
                    }
                    b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" => {
                        pending.ranges.push(range);
                        result.paths.push(std::mem::take(&mut pending));
                    }
                    b"Do" => result.draws.push(range),
                    b"sh" => result.shadings.push(range),
                    b"gs" => result.ext_states.push(range),
                    b"BDC" | b"DP" => result.marked_properties.push(range),
                    b"scn" | b"SCN" => {
                        let operators =
                            parse_content_stream(&data[range.clone()]).map_err(|_| UNSUPPORTED)?;
                        if matches!(
                            operators.as_slice(),
                            [Operator::SetFillColorN { name: Some(_), .. }
                                | Operator::SetStrokeColorN { name: Some(_), .. }]
                        ) {
                            result.pattern_colors.push(range);
                        }
                    }
                    b"Tj" | b"TJ" | b"'" | b"\"" => result.text_shows.push(range),
                    b"Tm" if scopes => result.text_matrices.push(range),
                    b"Td" | b"TD" | b"T*" if scopes => result.text_offsets.push(range),
                    b"Tr" => {
                        let operation =
                            parse_content_stream(&data[range.clone()]).map_err(|_| UNSUPPORTED)?;
                        let [Operator::Tr { render }] = operation.as_slice() else {
                            return Err(UNSUPPORTED);
                        };
                        result.text_modes.push((range, *render));
                    }
                    b"BI" => {
                        if range.start != start {
                            return Err(UNSUPPORTED);
                        }
                        let (end, image) = super::redaction_inline::read(data, position)?;
                        result.inline_images.push((start..end, image));
                        position = end;
                    }
                    _ => {}
                }
                continue;
            }
        }
        values.get_or_insert(start);
    }
    if !levels.is_empty() {
        return Err(UNSUPPORTED);
    }
    Ok(result)
}

pub(super) fn marked_commands(data: &[u8]) -> Result<Vec<Range<usize>>, &'static str> {
    Ok(stream_commands(data)?.marked_properties)
}

pub(super) fn draw_commands(data: &[u8]) -> Result<Vec<Range<usize>>, &'static str> {
    Ok(stream_commands(data)?.draws)
}

pub(super) struct TextCommandRanges {
    pub properties: Vec<Range<usize>>,
    pub draws: Vec<Range<usize>>,
    pub shows: Vec<Range<usize>>,
}

pub(super) fn text_command_ranges(data: &[u8]) -> Result<TextCommandRanges, &'static str> {
    let commands = stream_commands(data)?;
    Ok(TextCommandRanges {
        properties: commands.marked_properties,
        draws: commands.draws,
        shows: commands.text_shows,
    })
}

pub(super) fn text_and_scope_ranges(
    data: &[u8],
) -> Result<(Vec<Range<usize>>, Vec<Range<usize>>), &'static str> {
    let commands = stream_commands_with_scopes(data, true)?;
    Ok((commands.text_shows, commands.scopes))
}

pub(super) struct TextPositionRanges {
    pub shows: Vec<Range<usize>>,
    pub boundaries: Vec<Range<usize>>,
    pub matrices: Vec<Range<usize>>,
    pub offsets: Vec<Range<usize>>,
}

pub(super) fn text_position_ranges(data: &[u8]) -> Result<TextPositionRanges, &'static str> {
    let commands = stream_commands_with_scopes(data, true)?;
    Ok(TextPositionRanges {
        shows: commands.text_shows,
        boundaries: commands
            .scopes
            .into_iter()
            .filter(|range| matches!(&data[range.clone()], b"BT" | b"ET"))
            .collect(),
        matrices: commands.text_matrices,
        offsets: commands.text_offsets,
    })
}

pub(super) fn inline_images(data: &[u8]) -> Result<Vec<(Range<usize>, Object)>, &'static str> {
    Ok(stream_commands(data)?.inline_images)
}

pub(super) fn retain_paint_resources(
    pdf: &PdfDocument,
    page: usize,
    data: &[u8],
    overlay: &mut Overlay,
) -> Result<(), &'static str> {
    let mut patterns = HashSet::new();
    let mut shadings = HashSet::new();
    let resources = pdf.get_page_resources(page).map_err(|_| UNSUPPORTED)?;
    let objects = resources
        .as_dict()
        .and_then(|dict| dict.get("XObject"))
        .map(|object| pdf.resolve_object(object))
        .transpose()
        .map_err(|_| UNSUPPORTED)?;
    let mut pending = vec![data.to_vec()];
    let mut visited = HashSet::new();
    let mut decoded = data.len();
    while let Some(content) = pending.pop() {
        for operator in parse_content_stream(&content).map_err(|_| UNSUPPORTED)? {
            match operator {
                Operator::SetFillColorN {
                    name: Some(name), ..
                }
                | Operator::SetStrokeColorN {
                    name: Some(name), ..
                } => {
                    patterns.insert(*name);
                }
                Operator::PaintShading { name } => {
                    shadings.insert(name);
                }
                Operator::Do { name } if visited.insert(name.clone()) => {
                    if visited.len() > 4_096 {
                        return Err(UNSUPPORTED);
                    }
                    let object = objects
                        .as_ref()
                        .and_then(Object::as_dict)
                        .and_then(|objects| objects.get(&name));
                    let Some(object) = object else {
                        if overlay.xobject_edits.iter().any(|edit| edit.target == name) {
                            continue;
                        }
                        return Err(UNSUPPORTED);
                    };
                    let object = pdf.resolve_object(object).map_err(|_| UNSUPPORTED)?;
                    let fields = object.as_dict().ok_or(UNSUPPORTED)?;
                    if fields.get("Subtype").and_then(Object::as_name) == Some("Form")
                        && !fields.contains_key("Resources")
                    {
                        let content = object.decode_stream_data().map_err(|_| UNSUPPORTED)?;
                        decoded = decoded.checked_add(content.len()).ok_or(UNSUPPORTED)?;
                        if decoded > 64 * 1024 * 1024 {
                            return Err(UNSUPPORTED);
                        }
                        pending.push(content);
                    }
                }
                _ => {}
            }
        }
    }
    overlay.retain_patterns = Some(patterns);
    overlay.retain_shadings = Some(shadings);
    Ok(())
}

pub(super) fn remove_covered(
    pdf: &PdfDocument,
    bytes: &[u8],
    page: usize,
    regions: &[[f32; 4]],
) -> Result<Option<Overlay>, &'static str> {
    let data = pdf.get_page_content_data(page).map_err(|_| UNSUPPORTED)?;
    let commands = stream_commands(&data)?;
    if commands.paths.is_empty()
        && commands.clip_paths.is_empty()
        && commands.shadings.is_empty()
        && commands.pattern_colors.is_empty()
    {
        return Ok(None);
    }
    let operators = parse_content_stream(&data).map_err(|_| UNSUPPORTED)?;
    let draws = if commands.pattern_colors.is_empty() {
        HashMap::new()
    } else {
        pattern_draws(pdf, page, &operators)?
    };
    let states = paint_states(pdf, page, &operators, &draws)?;
    if commands.paths.len() != states.paths.len()
        || commands.shadings.len() != states.shadings.len()
        || commands.path_ends != states.path_ends.len()
        || commands.pattern_colors.len() != states.pattern_assignments
        || commands.ext_states.len() != states.ext_stroke_operators.len()
    {
        return Err(UNSUPPORTED);
    }
    let paths = if states.paths.is_empty() {
        Vec::new()
    } else if commands.draws.is_empty() {
        pdf.extract_paths(page).map_err(|_| UNSUPPORTED)?
    } else {
        let direct = omit_ranges(&data, commands.draws.clone())?;
        let proxy = assembly::page_for_vector_analysis(bytes, page, direct)?;
        PdfDocument::from_bytes(proxy)
            .and_then(|pdf| pdf.extract_paths(0))
            .map_err(|_| UNSUPPORTED)?
    };
    let geometry = assembly::geometry(pdf, page)?;
    let regions: Vec<_> = regions.iter().map(|area| area.map(f64::from)).collect();
    let mut edits = Vec::new();
    let mut rasters = Vec::new();
    for (index, (path, state)) in commands.paths.iter().zip(states.paths).enumerate() {
        let (paint, operators) = path.ranges.split_last().ok_or(UNSUPPORTED)?;
        let bounds = if paths.len() == commands.paths.len() {
            state.bounds(paths[index].bbox).unwrap_or(geometry.crop)
        } else {
            geometry.crop
        };
        let hit = regions
            .iter()
            .any(|region| super::text_edit::intersects(*region, bounds));
        if !hit && !state.pattern {
            continue;
        }
        if operators.is_empty() {
            return Err(UNSUPPORTED);
        }
        let mut clip_suffix = String::new();
        if let Some(clip) = &path.clip {
            if !clip_ends_here(&data[paint.end..])? {
                clip_suffix = super::redaction_clip::outside(
                    &data,
                    operators,
                    clip,
                    state.graphics.matrix,
                    &regions,
                )?
                .unwrap_or(clip_with_original_geometry(&data, operators, clip)?);
            }
            edits.push((clip.clone(), String::new()));
        }
        edits.extend(
            operators
                .iter()
                .cloned()
                .map(|range| (range, String::new())),
        );
        if hit
            && regions.iter().any(|area| {
                area[0] <= bounds[0]
                    && area[1] <= bounds[1]
                    && area[2] >= bounds[2]
                    && area[3] >= bounds[3]
            })
        {
            edits.push((paint.clone(), clip_suffix));
        } else if !state.pattern && !state.stroke && path.clip.is_none() {
            match clipped_fill(&data, operators, paint, state.graphics.matrix, &regions) {
                Ok(content) => edits.push((paint.clone(), content)),
                Err(_) => rasters.push(RasterPaint {
                    operator: paint.clone(),
                    matrix: state.graphics.matrix,
                    bounds,
                    clip: None,
                    clip_suffix,
                }),
            }
        } else {
            if let Some(style) = state.vector_stroke(&states.dash_patterns) {
                if let Ok(mut content) = super::redaction_paint::reconstruct(
                    &data,
                    operators,
                    paint,
                    geometry.media,
                    state.graphics.matrix,
                    &regions,
                    &style,
                ) {
                    content.push_str(&clip_suffix);
                    edits.push((paint.clone(), content));
                    continue;
                }
            }
            rasters.push(RasterPaint {
                operator: paint.clone(),
                matrix: state.graphics.matrix,
                bounds,
                clip: path.clip.clone(),
                clip_suffix,
            });
        }
    }
    for (index, path) in &commands.clip_paths {
        let (end, geometry) = path.ranges.split_last().ok_or(UNSUPPORTED)?;
        let clip = path.clip.as_ref().ok_or(UNSUPPORTED)?;
        if let Some(replacement) = super::redaction_clip::outside(
            &data,
            geometry,
            clip,
            states.path_ends[*index],
            &regions,
        )? {
            edits.extend(
                geometry
                    .iter()
                    .chain(&path.clip)
                    .cloned()
                    .map(|range| (range, String::new())),
            );
            edits.push((end.clone(), replacement));
        }
    }
    for (operator, matrix) in commands.shadings.iter().cloned().zip(states.shadings) {
        rasters.push(RasterPaint {
            operator,
            matrix,
            bounds: geometry.crop,
            clip: None,
            clip_suffix: String::new(),
        });
    }
    edits.extend(
        commands
            .pattern_colors
            .iter()
            .enumerate()
            .filter_map(|(index, range)| {
                (!states.used_by_text_or_images.contains(&index))
                    .then_some((range.clone(), String::new()))
            }),
    );
    if edits.is_empty() && rasters.is_empty() {
        return Ok(None);
    }
    let mut overlay = Overlay {
        redact_content: true,
        ..Default::default()
    };
    if !rasters.is_empty() {
        rasterize(
            pdf,
            bytes,
            page,
            geometry,
            &regions,
            &data,
            &commands,
            &states.ext_stroke_operators,
            rasters,
            &mut edits,
            &mut overlay,
        )?;
    }
    let content = rewrite_ranges(&data, edits)?;
    if !commands.pattern_colors.is_empty() || !commands.shadings.is_empty() {
        retain_paint_resources(pdf, page, &content, &mut overlay)?;
    }
    overlay.replace_page_content = Some(content);
    Ok(Some(overlay))
}

fn clip_with_original_geometry(
    data: &[u8],
    geometry: &[Range<usize>],
    clip: &Range<usize>,
) -> Result<String, &'static str> {
    let mut result = String::new();
    for range in geometry.iter().chain(std::iter::once(clip)) {
        result.push_str(std::str::from_utf8(&data[range.clone()]).map_err(|_| UNSUPPORTED)?);
        result.push('\n');
    }
    result.push_str("n\n");
    Ok(result)
}

fn clip_ends_here(data: &[u8]) -> Result<bool, &'static str> {
    Ok(matches!(
        parse_content_stream(data).map_err(|_| UNSUPPORTED)?.first(),
        None | Some(Operator::RestoreState)
    ))
}

fn clipped_fill(
    data: &[u8],
    geometry: &[Range<usize>],
    paint: &Range<usize>,
    matrix: Matrix,
    regions: &[Bounds],
) -> Result<String, &'static str> {
    let drawing =
        parse_content_stream(&data[geometry[0].start..paint.start]).map_err(|_| PARTIAL_PATH)?;
    if drawing.iter().any(|op| {
        matches!(
            op,
            Operator::Cm { .. } | Operator::SaveState | Operator::RestoreState
        )
    }) {
        return Err(PARTIAL_PATH);
    }
    let polygons = convex_paths(data, geometry)?;
    Ok(match redact_polygons(polygons, matrix, regions)? {
        Some(polygons) => {
            polygons + std::str::from_utf8(&data[paint.clone()]).map_err(|_| PARTIAL_PATH)?
        }
        None => String::new(),
    })
}

#[derive(Clone, Copy)]
struct Graphics {
    matrix: Matrix,
    width: Option<f64>,
    miter: Option<f64>,
    cap: Option<Cap>,
    join: Option<Join>,
    dash: DashState,
    soft_mask: Option<bool>,
    stroke_adjust: Option<bool>,
    fill_pattern: Option<usize>,
    stroke_pattern: Option<usize>,
    text_mode: u8,
}

#[derive(Clone, Copy)]
enum DashState {
    Solid,
    Pattern(usize),
    Unknown,
}

struct DashPattern {
    lengths: Vec<f64>,
    phase: f64,
}

impl DashState {
    fn set(patterns: &mut Vec<DashPattern>, mut lengths: Vec<f64>, phase: f64) -> Self {
        if lengths.is_empty() {
            return Self::Solid;
        }
        if !phase.is_finite()
            || phase < 0.0
            || lengths.len() > 256
            || lengths
                .iter()
                .any(|value| !value.is_finite() || *value < 0.0)
        {
            return Self::Unknown;
        }
        if lengths.len() % 2 == 1 {
            lengths.extend_from_within(..);
        }
        let cycle = lengths.iter().sum::<f64>();
        if !cycle.is_finite() || cycle <= 0.0 {
            return Self::Unknown;
        }
        let index = patterns.len();
        patterns.push(DashPattern {
            lengths,
            phase: phase.rem_euclid(cycle),
        });
        Self::Pattern(index)
    }
}

impl Graphics {
    fn line_cap(style: u8) -> Option<Cap> {
        match style {
            0 => Some(Cap::Butt),
            1 => Some(Cap::Round),
            2 => Some(Cap::Square),
            _ => None,
        }
    }

    fn line_join(style: u8) -> Option<Join> {
        match style {
            0 => Some(Join::Miter),
            1 => Some(Join::Round),
            2 => Some(Join::Bevel),
            _ => None,
        }
    }

    fn apply_stroke_parameters(
        &mut self,
        pdf: &PdfDocument,
        state: Option<&Object>,
        dash_patterns: &mut Vec<DashPattern>,
    ) -> String {
        let state = state.and_then(|state| pdf.resolve_object(state).ok());
        let Some(fields) = state.as_ref().and_then(Object::as_dict) else {
            self.width = None;
            self.miter = None;
            self.cap = None;
            self.join = None;
            self.dash = DashState::Unknown;
            self.soft_mask = None;
            self.stroke_adjust = None;
            return String::new();
        };
        let resolve = |value: &Object| pdf.resolve_object(value).ok();
        let number = |value: &Object| {
            let value = resolve(value)?;
            let number = value
                .as_real()
                .or_else(|| value.as_integer().map(|n| n as f64))?;
            (number >= 0.0 && (number as f32).is_finite()).then_some(number)
        };
        if let Some(mask) = fields.get("SMask") {
            self.soft_mask = resolve(mask).map(|mask| mask.as_name() != Some("None"));
        }
        if let Some(adjust) = fields.get("SA") {
            self.stroke_adjust = resolve(adjust).and_then(|adjust| adjust.as_bool());
        }
        // PDF Oxide's raster renderer reads transparency from gs but leaves
        // stroke style to the content operators. Supply the equivalent style
        // operators only in the disposable rasterization proxy.
        let mut operators = String::new();
        if let Some(width) = fields.get("LW") {
            self.width = number(width);
            if let Some(width) = self.width {
                let _ = writeln!(operators, "{width} w");
            }
        }
        if let Some(miter) = fields.get("ML") {
            let miter = number(miter);
            self.miter = miter;
            if let Some(miter) = miter {
                let _ = writeln!(operators, "{miter} M");
            }
        }
        for (key, operator) in [("LC", 'J'), ("LJ", 'j')] {
            let Some(value) = fields.get(key) else {
                continue;
            };
            let style = resolve(value)
                .and_then(|value| value.as_integer())
                .and_then(|value| u8::try_from(value).ok());
            if key == "LC" {
                self.cap = style.and_then(Self::line_cap);
            } else {
                self.join = style.and_then(Self::line_join);
            }
            if let Some(style @ 0..=2) = style {
                let _ = writeln!(operators, "{style} {operator}");
            }
        }
        let dash = || {
            let dash = pdf.resolve_object(fields.get("D")?).ok()?;
            let [lengths, phase] = dash.as_array()?.as_slice() else {
                return None;
            };
            let phase = number(phase)?;
            let lengths = pdf.resolve_object(lengths).ok()?;
            let lengths = lengths
                .as_array()?
                .iter()
                .map(number)
                .collect::<Option<Vec<_>>>()?;
            Some((lengths, phase))
        };
        if fields.contains_key("D") {
            self.dash = DashState::Unknown;
        }
        if let Some((lengths, phase)) = dash() {
            operators.push('[');
            for length in &lengths {
                let _ = write!(operators, "{length} ");
            }
            let _ = writeln!(operators, "] {phase} d");
            self.dash = DashState::set(dash_patterns, lengths, phase);
        }
        operators
    }
}

struct Paint {
    graphics: Graphics,
    stroke: bool,
    pattern: bool,
}

impl Paint {
    fn vector_stroke(&self, dash_patterns: &[DashPattern]) -> Option<Stroke> {
        let graphics = self.graphics;
        if !self.stroke || self.pattern || graphics.soft_mask? || graphics.stroke_adjust? {
            return None;
        }
        let width = graphics.width?;
        let miter = graphics.miter?;
        if width <= 0.0 || miter < 1.0 || !width.is_finite() || !miter.is_finite() {
            return None;
        }
        let mut style = Stroke::new(width);
        style.join = graphics.join?;
        style.miter_limit = miter;
        style.start_cap = graphics.cap?;
        style.end_cap = style.start_cap;
        match graphics.dash {
            DashState::Solid => {}
            DashState::Pattern(index) => {
                let dash = &dash_patterns[index];
                style.dash_pattern.extend_from_slice(&dash.lengths);
                style.dash_offset = dash.phase;
            }
            DashState::Unknown => return None,
        }
        Some(style)
    }

    fn bounds(&self, path: pdf_oxide::geometry::Rect) -> Option<Bounds> {
        let margin = if self.stroke {
            let Matrix { a, b, c, d, .. } = self.graphics.matrix;
            let [a, b, c, d] = [a, b, c, d].map(f64::from);
            // Bound the stretch in any direction, including under shear, and
            // allow for miter joins, square caps and device hairlines.
            let max_stretch = a.hypot(b).hypot(c.hypot(d));
            (self.graphics.width? * max_stretch * self.graphics.miter?.max(2.0) * 0.5).max(1.0)
        } else {
            0.0
        };
        let bounds = [
            f64::from(path.x) - margin,
            f64::from(path.y) - margin,
            f64::from(path.x) + f64::from(path.width) + margin,
            f64::from(path.y) + f64::from(path.height) + margin,
        ];
        bounds
            .iter()
            .all(|value| value.is_finite())
            .then_some(bounds)
    }
}

#[derive(Default)]
struct PaintStates {
    paths: Vec<Paint>,
    shadings: Vec<Matrix>,
    path_ends: Vec<Matrix>,
    ext_stroke_operators: Vec<String>,
    dash_patterns: Vec<DashPattern>,
    pattern_assignments: usize,
    used_by_paths: HashSet<usize>,
    used_by_text_or_images: HashSet<usize>,
}

impl PaintStates {
    fn paint(&mut self, graphics: Graphics, fill: bool, stroke: bool) {
        let used = [
            graphics.fill_pattern.filter(|_| fill),
            graphics.stroke_pattern.filter(|_| stroke),
        ];
        let pattern = used.iter().any(Option::is_some);
        self.used_by_paths.extend(used.into_iter().flatten());
        self.paths.push(Paint {
            graphics,
            stroke,
            pattern,
        });
    }

    fn assignment(&mut self, has_name: bool) -> Option<usize> {
        if !has_name {
            return None;
        }
        let index = self.pattern_assignments;
        self.pattern_assignments += 1;
        Some(index)
    }

    fn keep_pattern(&mut self, graphics: Graphics, fill: bool, stroke: bool) {
        self.used_by_text_or_images.extend(
            [
                graphics.fill_pattern.filter(|_| fill),
                graphics.stroke_pattern.filter(|_| stroke),
            ]
            .into_iter()
            .flatten(),
        );
    }
}

fn paint_states(
    pdf: &PdfDocument,
    page: usize,
    operators: &[Operator],
    draws: &HashMap<String, (bool, bool)>,
) -> Result<PaintStates, &'static str> {
    let mut graphics = Graphics {
        matrix: Matrix::identity(),
        width: Some(1.0),
        miter: Some(10.0),
        cap: Some(Cap::Butt),
        join: Some(Join::Miter),
        dash: DashState::Solid,
        soft_mask: Some(false),
        stroke_adjust: Some(false),
        fill_pattern: None,
        stroke_pattern: None,
        text_mode: 0,
    };
    let mut stack = Vec::new();
    let mut states = PaintStates::default();
    let ext_states = std::cell::OnceCell::new();
    for operator in operators {
        match *operator {
            Operator::SaveState => stack.push(graphics),
            Operator::RestoreState => graphics = stack.pop().ok_or(UNSUPPORTED)?,
            Operator::Cm { a, b, c, d, e, f } => {
                graphics.matrix = Matrix { a, b, c, d, e, f }.multiply(&graphics.matrix)
            }
            Operator::SetLineWidth { width } => graphics.width = Some(f64::from(width).abs()),
            Operator::SetMiterLimit { limit } => graphics.miter = Some(f64::from(limit)),
            Operator::SetLineCap { cap_style } => graphics.cap = Graphics::line_cap(cap_style),
            Operator::SetLineJoin { join_style } => graphics.join = Graphics::line_join(join_style),
            Operator::SetDash { ref array, phase } => {
                graphics.dash = DashState::set(
                    &mut states.dash_patterns,
                    array.iter().map(|&value| f64::from(value)).collect(),
                    f64::from(phase),
                );
            }
            Operator::SetExtGState { ref dict_name } => {
                let entries = ext_states.get_or_init(|| {
                    let resources = pdf.get_page_resources(page).ok()?;
                    pdf.resolve_object(resources.as_dict()?.get("ExtGState")?)
                        .ok()
                });
                let state = entries
                    .as_ref()
                    .and_then(Object::as_dict)
                    .and_then(|dict| dict.get(dict_name));
                states
                    .ext_stroke_operators
                    .push(graphics.apply_stroke_parameters(pdf, state, &mut states.dash_patterns));
            }
            Operator::SetFillColorN { ref name, .. } => {
                graphics.fill_pattern = states.assignment(name.is_some())
            }
            Operator::SetStrokeColorN { ref name, .. } => {
                graphics.stroke_pattern = states.assignment(name.is_some())
            }
            Operator::SetFillColorSpace { .. }
            | Operator::SetFillColor { .. }
            | Operator::SetFillRgb { .. }
            | Operator::SetFillCmyk { .. }
            | Operator::SetFillGray { .. } => graphics.fill_pattern = None,
            Operator::SetStrokeColorSpace { .. }
            | Operator::SetStrokeColor { .. }
            | Operator::SetStrokeRgb { .. }
            | Operator::SetStrokeCmyk { .. }
            | Operator::SetStrokeGray { .. } => graphics.stroke_pattern = None,
            Operator::Fill | Operator::FillEvenOdd => states.paint(graphics, true, false),
            Operator::Stroke | Operator::CloseAndStroke => states.paint(graphics, false, true),
            Operator::FillStroke
            | Operator::FillStrokeEvenOdd
            | Operator::CloseFillStroke
            | Operator::CloseFillStrokeEvenOdd => states.paint(graphics, true, true),
            Operator::PaintShading { .. } => states.shadings.push(graphics.matrix),
            Operator::EndPath => states.path_ends.push(graphics.matrix),
            Operator::Tr { render } => graphics.text_mode = render,
            Operator::Tj { .. }
            | Operator::TJ { .. }
            | Operator::Quote { .. }
            | Operator::DoubleQuote { .. } => {
                let fill = matches!(graphics.text_mode, 0 | 2 | 4 | 6);
                let stroke = matches!(graphics.text_mode, 1 | 2 | 5 | 6);
                states.keep_pattern(graphics, fill, stroke);
            }
            Operator::Do { ref name } => {
                if let Some(&(fill, stroke)) = draws.get(name) {
                    states.keep_pattern(graphics, fill, stroke);
                }
            }
            Operator::InlineImage { ref dict, .. } => {
                if dict.get("ImageMask").or_else(|| dict.get("IM")) == Some(&Object::Boolean(true))
                {
                    states.keep_pattern(graphics, true, false);
                }
            }
            _ => {}
        }
    }
    Ok(states)
}

fn pattern_draws(
    pdf: &PdfDocument,
    page: usize,
    operators: &[Operator],
) -> Result<HashMap<String, (bool, bool)>, &'static str> {
    let mut resolved = HashMap::new();
    let resources = pdf.get_page_resources(page).map_err(|_| UNSUPPORTED)?;
    let objects = resources
        .as_dict()
        .and_then(|dict| dict.get("XObject"))
        .map(|object| pdf.resolve_object(object))
        .transpose()
        .map_err(|_| UNSUPPORTED)?;
    for operator in operators {
        let Operator::Do { name } = operator else {
            continue;
        };
        if resolved.contains_key(name) {
            continue;
        }
        let object = objects
            .as_ref()
            .and_then(Object::as_dict)
            .and_then(|objects| objects.get(name))
            .ok_or(UNSUPPORTED)?;
        let object = pdf.resolve_object(object).map_err(|_| UNSUPPORTED)?;
        let fields = object.as_dict().ok_or(UNSUPPORTED)?;
        let uses = match fields.get("Subtype").and_then(Object::as_name) {
            Some("Image") => (
                fields.get("ImageMask") == Some(&Object::Boolean(true)),
                false,
            ),
            Some("Form") => (true, true),
            _ => return Err(UNSUPPORTED),
        };
        resolved.insert(name.clone(), uses);
    }
    Ok(resolved)
}

pub(super) fn prune_unpainted_patterns(
    pdf: &PdfDocument,
    page: usize,
) -> Result<Option<Overlay>, &'static str> {
    let resources = pdf.get_page_resources(page).map_err(|_| UNSUPPORTED)?;
    if !resources
        .as_dict()
        .is_some_and(|fields| fields.contains_key("Pattern"))
    {
        return Ok(None);
    }
    let data = pdf.get_page_content_data(page).map_err(|_| UNSUPPORTED)?;
    let commands = stream_commands(&data)?;
    let operators = parse_content_stream(&data).map_err(|_| UNSUPPORTED)?;
    let draws = if commands.pattern_colors.is_empty() {
        HashMap::new()
    } else {
        pattern_draws(pdf, page, &operators)?
    };
    let states = paint_states(pdf, page, &operators, &draws)?;
    if commands.pattern_colors.len() != states.pattern_assignments {
        return Err(UNSUPPORTED);
    }
    let remove = commands
        .pattern_colors
        .into_iter()
        .enumerate()
        .filter_map(|(index, range)| {
            (!states.used_by_paths.contains(&index)
                && !states.used_by_text_or_images.contains(&index))
            .then_some(range)
        })
        .collect::<Vec<_>>();
    let mut overlay = Overlay {
        redact_content: true,
        ..Default::default()
    };
    if remove.is_empty() {
        retain_paint_resources(pdf, page, &data, &mut overlay)?;
    } else {
        let content = omit_ranges(&data, remove)?;
        retain_paint_resources(pdf, page, &content, &mut overlay)?;
        overlay.replace_page_content = Some(content);
    }
    Ok(Some(overlay))
}

struct RasterPaint {
    operator: Range<usize>,
    matrix: Matrix,
    bounds: Bounds,
    clip: Option<Range<usize>>,
    clip_suffix: String,
}

#[allow(clippy::too_many_arguments)]
fn rasterize(
    pdf: &PdfDocument,
    bytes: &[u8],
    page: usize,
    geometry: Geometry,
    regions: &[Bounds],
    data: &[u8],
    commands: &StreamCommands,
    ext_stroke_operators: &[String],
    rasters: Vec<RasterPaint>,
    edits: &mut Vec<(Range<usize>, String)>,
    overlay: &mut Overlay,
) -> Result<(), &'static str> {
    use pdf_oxide::{
        editor::DocumentEditor,
        rendering::{ImageFormat, PageRenderer, RenderOptions},
    };
    let invalid = "Cannot safely render the selected PDF graphics";
    let max_pixels = 10_000_000.0;
    if rasters.len() > 128 {
        return Err(invalid);
    }
    let resources = pdf.get_page_resources(page).map_err(|_| invalid)?;
    let names = |kind| -> Result<HashSet<String>, &'static str> {
        resources
            .as_dict()
            .and_then(|dict| dict.get(kind))
            .map(|dict| {
                let dict = pdf.resolve_object(dict).map_err(|_| invalid)?;
                Ok(dict.as_dict().ok_or(invalid)?.keys().cloned().collect())
            })
            .transpose()
            .map(Option::unwrap_or_default)
    };
    let images = names("XObject")?;
    let states = names("ExtGState")?;
    let prefix = (0..)
        .map(|n| format!("MatterDraftRaster{n}_"))
        .find(|prefix| {
            images
                .iter()
                .chain(&states)
                .all(|key| !key.starts_with(prefix))
        })
        .ok_or(invalid)?;
    let reset = format!("{prefix}alpha");
    let mut keep = retained_xobjects(pdf, page, data)?;
    let text_clip = commands.text_modes.iter().any(|(_, render)| *render >= 4);
    let mut pixels = 0.0;
    for (index, paint) in rasters.into_iter().enumerate() {
        let viewport = raster_viewport(geometry, paint.bounds, max_pixels);
        let (width, height) = viewport.display_size();
        let dpi = (72.0 * (max_pixels / (width * height)).sqrt())
            .min(216.0)
            .floor() as u32;
        let scale = f64::from(dpi as f32 / 72.0);
        let render_width = (width * scale).ceil();
        let render_height = (height * scale).ceil();
        pixels += render_width * render_height;
        if dpi < 72 || pixels > 120_000_000.0 {
            return Err(invalid);
        }
        let masked = regions
            .iter()
            .map(|&area| super::text_edit::displayed(viewport, area))
            .map(|[l, t, r, b]| {
                [
                    l * width * scale,
                    t * height * scale,
                    r * width * scale,
                    b * height * scale,
                ]
            })
            .collect::<Vec<_>>();
        let selected = paint.operator;
        let mut skip = commands
            .ext_states
            .iter()
            .zip(ext_stroke_operators)
            .filter(|(_, operators)| !operators.is_empty())
            .map(|(range, operators)| (range.end..range.end, operators.clone()))
            .collect::<Vec<_>>();
        skip.extend(
            commands
                .paths
                .iter()
                .filter_map(|path| path.ranges.last())
                .filter(|range| **range != selected)
                .map(|range| (range.clone(), "n".to_owned()))
                .chain(
                    commands
                        .shadings
                        .iter()
                        .filter(|range| **range != selected)
                        .chain(&commands.draws)
                        .map(|range| (range.clone(), String::new())),
                ),
        );
        if let Some(clip) = paint.clip {
            // W applies after the selected paint. The replacement keeps the
            // reconstructed clip for subsequent paints in the real stream.
            skip.push((clip, String::new()));
        }
        if text_clip {
            skip.extend(commands.text_modes.iter().map(|(range, mode)| {
                (
                    range.clone(),
                    if *mode >= 4 { "7 Tr" } else { "3 Tr" }.to_owned(),
                )
            }));
        } else {
            skip.extend(
                commands
                    .text_shows
                    .iter()
                    .cloned()
                    .map(|range| (range, String::new())),
            );
        }
        let mut isolated = rewrite_ranges(data, skip)?;
        if text_clip {
            // Preserve the source text clipping without painting its ink.
            isolated.splice(..0, b"3 Tr\n".iter().copied());
        }
        let proxy = assembly::page_for_vector_analysis(bytes, page, isolated)?;
        let proxy = if viewport.crop == geometry.crop {
            proxy
        } else {
            // Limit the actual renderer viewport. Cropping its output later
            // would still allocate a raster for the full source sheet.
            let mut editor = DocumentEditor::from_bytes(proxy).map_err(|_| invalid)?;
            editor
                .set_page_crop_box(0, viewport.crop.map(|value| value as f32))
                .map_err(|_| invalid)?;
            editor.save_to_bytes().map_err(|_| invalid)?
        };
        let proxy = PdfDocument::from_bytes(proxy).map_err(|_| invalid)?;
        let mut options = RenderOptions::with_dpi(dpi)
            .with_transparent_background()
            .as_raw();
        options.render_annotations = false;
        options.max_output_pixels = 10_100_000;
        let mut renderer = PageRenderer::new(options);
        let image = renderer.render_page(&proxy, 0).map_err(|_| invalid)?;
        if f64::from(image.width) != render_width
            || f64::from(image.height) != render_height
            || image.format != ImageFormat::RawRgba8
            || image.data.len() != image.width as usize * image.height as usize * 4
        {
            return Err(invalid);
        }
        let name = format!("{prefix}{index}");
        let replacement =
            raster_image(image, &masked, viewport, scale, paint.matrix, &name, &reset)?;
        if let Some((placement, image)) = replacement {
            edits.push((selected, format!("{placement}\n{}", paint.clip_suffix)));
            keep.insert(name.clone());
            overlay.xobject_edits.push(XObjectEdit {
                source: None,
                target: name,
                replacement: image,
            });
        } else {
            edits.push((selected, paint.clip_suffix));
        }
    }
    overlay.retain_xobjects = Some(keep);
    overlay.reset_raster_alpha = Some(reset);
    Ok(())
}

fn raster_viewport(geometry: Geometry, bounds: Bounds, max_pixels: f64) -> Geometry {
    let (width, height) = geometry.display_size();
    if width * height <= max_pixels {
        return geometry;
    }
    // Include the effect of pixel rounding and antialiasing around the
    // painted outline. The stored page origin and rotation stay intact.
    let [left, bottom, right, top] = bounds;
    let crop = [
        (left - 2.0).floor().max(geometry.crop[0]),
        (bottom - 2.0).floor().max(geometry.crop[1]),
        (right + 2.0).ceil().min(geometry.crop[2]),
        (top + 2.0).ceil().min(geometry.crop[3]),
    ];
    if crop[0] < crop[2] && crop[1] < crop[3] {
        Geometry { crop, ..geometry }
    } else {
        geometry
    }
}

#[cfg(test)]
mod raster_tests {
    use super::*;
    use pdf_oxide::{
        editor::DocumentEditor,
        rendering::{render_page, RenderOptions},
    };

    #[test]
    fn oversized_sheet_uses_the_paint_bounds_for_a_redaction_raster() {
        let original = assembly::blank((6000.0, 4800.0)).unwrap();
        let overlay = Overlay {
            commands: "q 0 0 1 RG 12 w 110 250 m 360 250 l S Q".into(),
            user_space_text: true,
            ..Default::default()
        };
        let original =
            assembly::append_document_overlays(original, &HashMap::from([(0, overlay)]), None)
                .unwrap();

        let edited = super::super::document_output::destructive_redaction(
            original,
            &[(0, [200.0, 235.0, 260.0, 265.0])],
        )
        .unwrap();
        let document = PdfDocument::from_bytes(edited.clone()).unwrap();
        let geometry = assembly::geometry(&document, 0).unwrap();
        assert_eq!(geometry.crop, [0.0, 0.0, 6000.0, 4800.0]);
        assert_eq!(geometry.media, geometry.crop);
        let content = String::from_utf8(document.get_page_content_data(0).unwrap()).unwrap();
        assert!(content.contains("/MatterDraftRaster"));
        assert!(!content.contains("110 250 m 360 250 l"));

        let mut viewport = DocumentEditor::from_bytes(edited).unwrap();
        viewport
            .set_page_crop_box(0, [90.0, 200.0, 390.0, 270.0])
            .unwrap();
        let viewport = PdfDocument::from_bytes(viewport.save_to_bytes().unwrap()).unwrap();
        let image = render_page(&viewport, 0, &RenderOptions::with_dpi(72).as_raw()).unwrap();
        assert_eq!((image.width, image.height), (300, 70));
        let sample = |x, y| {
            let offset = ((y * image.width + x) * 4) as usize;
            &image.data[offset..offset + 3]
        };
        assert_eq!(sample(40, 20), [0, 0, 255]);
        assert_eq!(sample(140, 20), [0, 0, 0]);
        assert_eq!(sample(240, 20), [0, 0, 255]);
    }
}

fn draw_resource_names(data: &[u8]) -> Result<HashSet<String>, &'static str> {
    Ok(parse_content_stream(data)
        .map_err(|_| UNSUPPORTED)?
        .into_iter()
        .filter_map(|op| match op {
            Operator::Do { name } => Some(name),
            _ => None,
        })
        .collect())
}

pub(super) fn inherited_form_content(pdf: &PdfDocument, form: &Object) -> Option<Option<Vec<u8>>> {
    let fields = form.as_dict()?;
    if fields.get("Subtype")?.as_name()? != "Form" {
        return Some(None);
    }
    if let Some(resources) = fields.get("Resources") {
        match pdf.resolve_object(resources).ok()? {
            Object::Dictionary(_) => return Some(None),
            Object::Null => {}
            _ => return None,
        }
    }
    Some(Some(form.decode_stream_data().ok()?))
}

pub(super) fn retained_xobjects(
    pdf: &PdfDocument,
    page: usize,
    data: &[u8],
) -> Result<HashSet<String>, &'static str> {
    let mut keep = draw_resource_names(data)?;
    if keep.is_empty() {
        return Ok(keep);
    }
    let resources = pdf.get_page_resources(page).map_err(|_| UNSUPPORTED)?;
    let objects = resources
        .as_dict()
        .and_then(|fields| fields.get("XObject"))
        .ok_or(UNSUPPORTED)?;
    let objects = pdf.resolve_object(objects).map_err(|_| UNSUPPORTED)?;
    retain_inherited_xobjects(pdf, objects.as_dict().ok_or(UNSUPPORTED)?, &mut keep)?;
    Ok(keep)
}

pub(super) fn retain_inherited_xobjects(
    pdf: &PdfDocument,
    objects: &Dictionary,
    keep: &mut HashSet<String>,
) -> Result<(), &'static str> {
    let mut pending = keep.iter().cloned().collect::<Vec<_>>();
    let mut decoded = 0usize;
    while let Some(name) = pending.pop() {
        let object = objects.get(&name).ok_or(UNSUPPORTED)?;
        let object = pdf.resolve_object(object).map_err(|_| UNSUPPORTED)?;
        let Some(content) = inherited_form_content(pdf, &object).ok_or(UNSUPPORTED)? else {
            continue;
        };
        decoded = decoded.checked_add(content.len()).ok_or(UNSUPPORTED)?;
        if decoded > 64 * 1024 * 1024 || keep.len() > 4096 {
            return Err(UNSUPPORTED);
        }
        for name in draw_resource_names(&content)? {
            if keep.insert(name.clone()) {
                pending.push(name);
            }
        }
    }
    Ok(())
}

fn raster_image(
    mut image: pdf_oxide::rendering::RenderedImage,
    masked: &[Bounds],
    geometry: Geometry,
    scale: f64,
    matrix: Matrix,
    name: &str,
    reset: &str,
) -> Result<Option<(String, XObjectReplacement)>, &'static str> {
    let mut bounds = [image.width, image.height, 0, 0];
    for (i, pixel) in image.data.chunks_exact_mut(4).enumerate() {
        let x = (i % image.width as usize) as u32;
        let y = (i / image.width as usize) as u32;
        let (px, py) = (f64::from(x), f64::from(y));
        if masked
            .iter()
            .any(|r| px < r[2] && px + 1.0 > r[0] && py < r[3] && py + 1.0 > r[1])
        {
            pixel.fill(0);
        }
        if pixel[3] != 0 {
            bounds[0] = bounds[0].min(x);
            bounds[1] = bounds[1].min(y);
            bounds[2] = bounds[2].max(x + 1);
            bounds[3] = bounds[3].max(y + 1);
        }
    }
    let [l, t, r, b] = bounds;
    if l >= r || t >= b {
        return Ok(None);
    }
    let (w, h) = (r - l, b - t);
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    let mut alpha = Vec::with_capacity((w * h) as usize);
    for row in t..b {
        let offset = (row * image.width + l) as usize * 4;
        for pixel in image.data[offset..offset + w as usize * 4].chunks_exact(4) {
            let a = u32::from(pixel[3]);
            alpha.push(pixel[3]);
            rgb.extend(pixel[..3].iter().map(|&component| {
                if a == 0 {
                    0
                } else {
                    ((u32::from(component) * 255 + a / 2) / a).min(255) as u8
                }
            }));
        }
    }
    let [a, b_matrix, c, d, e, f] = geometry.display_matrix().map(|v| v as f32);
    let display = Matrix {
        a,
        b: b_matrix,
        c,
        d,
        e,
        f,
    };
    let [a, b_matrix, c, d, e, f] =
        [matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f].map(f64::from);
    let det = a * d - b_matrix * c;
    if !det.is_finite() || det.abs() < 1e-12 {
        return Err(UNSUPPORTED);
    }
    let inverse = Matrix {
        a: (d / det) as f32,
        b: (-b_matrix / det) as f32,
        c: (-c / det) as f32,
        d: (a / det) as f32,
        e: ((c * f - d * e) / det) as f32,
        f: ((b_matrix * e - a * f) / det) as f32,
    };
    let placement = Matrix {
        a: (f64::from(w) / scale) as f32,
        b: 0.0,
        c: 0.0,
        d: (f64::from(h) / scale) as f32,
        e: (f64::from(l) / scale) as f32,
        f: (geometry.display_size().1 - f64::from(b) / scale) as f32,
    }
    .multiply(&display)
    .multiply(&inverse);
    let Matrix { a, b, c, d, e, f } = placement;
    if ![a, b, c, d, e, f].iter().all(|x| x.is_finite()) {
        return Err(UNSUPPORTED);
    }
    let command =
        format!("q /{reset} gs {a:.8} {b:.8} {c:.8} {d:.8} {e:.8} {f:.8} cm /{name} Do Q");
    let object = super::document_output::image_stream(w, h, "DeviceRGB", &rgb)?;
    let mask = super::document_output::image_stream(w, h, "DeviceGray", &alpha)?;
    Ok(Some((
        command,
        XObjectReplacement::Image(object, Some(mask)),
    )))
}

fn convex_paths(data: &[u8], paths: &[Range<usize>]) -> Result<Vec<Vec<[f64; 2]>>, &'static str> {
    fn store(
        polygons: &mut Vec<Vec<[f64; 2]>>,
        points: &mut Vec<[f64; 2]>,
    ) -> Result<(), &'static str> {
        let mut points = std::mem::take(points);
        points.dedup();
        if points.len() > 1 && points.first() == points.last() {
            points.pop();
        }
        if points.len() < 3 {
            return Ok(());
        }
        if points.len() > 256 || points.iter().flatten().any(|value| !value.is_finite()) {
            return Err(PARTIAL_PATH);
        }
        let mut direction = 0.0;
        for (first, end) in points.iter().zip(points.iter().cycle().skip(1)) {
            for point in &points {
                let side = (end[0] - first[0]) * (point[1] - first[1])
                    - (end[1] - first[1]) * (point[0] - first[0]);
                if side == 0.0 {
                    continue;
                }
                if direction != 0.0 && side * direction < 0.0 {
                    return Err(PARTIAL_PATH);
                }
                direction = side.signum();
            }
        }
        if direction != 0.0 {
            polygons.push(points);
        }
        Ok(())
    }

    let mut polygons = Vec::new();
    let mut points = Vec::new();
    for range in paths {
        let operators = parse_content_stream(&data[range.clone()]).map_err(|_| PARTIAL_PATH)?;
        match operators.as_slice() {
            [Operator::MoveTo { x, y }] => {
                store(&mut polygons, &mut points)?;
                points.push([(*x).into(), (*y).into()]);
            }
            [Operator::LineTo { x, y }] if !points.is_empty() && points.len() < 257 => {
                points.push([(*x).into(), (*y).into()]);
            }
            [Operator::ClosePath] => store(&mut polygons, &mut points)?,
            [Operator::Rectangle {
                x,
                y,
                width,
                height,
            }] => {
                store(&mut polygons, &mut points)?;
                let [x, y, width, height] = [*x, *y, *width, *height].map(f64::from);
                points = vec![
                    [x, y],
                    [x + width, y],
                    [x + width, y + height],
                    [x, y + height],
                ];
                store(&mut polygons, &mut points)?;
            }
            _ => return Err(PARTIAL_PATH),
        }
    }
    store(&mut polygons, &mut points)?;
    // Convex contours preserve their original winding when split. Emitting
    // every contour together preserves both PDF fill rules, including holes.
    Ok(polygons)
}

pub(super) fn redact_polygons(
    polygons: Vec<Vec<[f64; 2]>>,
    Matrix { a, b, c, d, e, f }: Matrix,
    regions: &[Bounds],
) -> Result<Option<String>, &'static str> {
    let [a, b, c, d, e, f] = [a, b, c, d, e, f].map(f64::from);
    let determinant = a * d - b * c;
    if ![a, b, c, d, e, f].iter().all(|n| n.is_finite()) || determinant.abs() < 1e-12 {
        return Err(PARTIAL_PATH);
    }
    let to_page = |x, y| [a * x + c * y + e, b * x + d * y + f];
    let to_local = |[x, y]: [f64; 2]| {
        [
            (d * (x - e) - c * (y - f)) / determinant,
            (-b * (x - e) + a * (y - f)) / determinant,
        ]
    };
    let fragments = polygons
        .into_iter()
        .map(|polygon| {
            polygon
                .into_iter()
                .map(|[x, y]| to_page(x, y))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let fragments = subtract_regions(fragments, regions)?;
    if fragments.is_empty() {
        return Ok(None);
    }
    let mut replacement = String::new();
    for polygon in fragments {
        for (index, point) in polygon.into_iter().enumerate() {
            let [x, y] = to_local(point);
            if !x.is_finite() || !y.is_finite() {
                return Err(PARTIAL_PATH);
            }
            let operator = if index == 0 { 'm' } else { 'l' };
            writeln!(replacement, "{x:.12} {y:.12} {operator}").map_err(|_| UNSUPPORTED)?;
        }
        replacement.push_str("h\n");
    }
    Ok(Some(replacement))
}

pub(super) fn subtract_regions(
    mut fragments: Vec<Vec<[f64; 2]>>,
    regions: &[Bounds],
) -> Result<Vec<Vec<[f64; 2]>>, &'static str> {
    for &[left, bottom, right, top] in regions {
        let mut outside = Vec::new();
        for mut remaining in fragments {
            // Each edge splits the part that still intersects the mask. The
            // outside pieces have disjoint interiors and keep the original winding.
            for (axis, edge, direction) in [
                (0, left, 1.0),
                (1, bottom, 1.0),
                (0, right, -1.0),
                (1, top, -1.0),
            ] {
                let piece = clip_polygon(&remaining, axis, edge, -direction);
                if piece.len() >= 3 {
                    outside.push(piece);
                }
                remaining = clip_polygon(&remaining, axis, edge, direction);
                if remaining.len() < 3 {
                    break;
                }
            }
            if outside.len() > 4096 {
                return Err(PARTIAL_PATH);
            }
        }
        fragments = outside;
    }
    Ok(fragments)
}

fn clip_polygon(polygon: &[[f64; 2]], axis: usize, edge: f64, direction: f64) -> Vec<[f64; 2]> {
    let mut result = Vec::new();
    let Some(mut previous) = polygon.last().copied() else {
        return result;
    };
    for &point in polygon {
        let previous_inside = (previous[axis] - edge) * direction >= 0.0;
        let inside = (point[axis] - edge) * direction >= 0.0;
        if inside != previous_inside {
            let other = 1 - axis;
            let ratio = (edge - previous[axis]) / (point[axis] - previous[axis]);
            let mut crossing = point;
            crossing[axis] = edge;
            crossing[other] = previous[other] + ratio * (point[other] - previous[other]);
            result.push(crossing);
        }
        if inside {
            result.push(point);
        }
        previous = point;
    }
    result.dedup();
    if result.len() > 1 && result.first() == result.last() {
        result.pop();
    }
    result
}

fn omit_ranges(data: &[u8], remove: Vec<Range<usize>>) -> Result<Vec<u8>, &'static str> {
    rewrite_ranges(
        data,
        remove
            .into_iter()
            .map(|range| (range, String::new()))
            .collect(),
    )
}

pub(super) fn rewrite_ranges(
    data: &[u8],
    mut edits: Vec<(Range<usize>, String)>,
) -> Result<Vec<u8>, &'static str> {
    edits.sort_by_key(|(range, _)| range.start);
    let mut output = Vec::new();
    let mut previous = 0;
    for (range, replacement) in edits {
        if range.start < previous {
            return Err(UNSUPPORTED);
        }
        output.extend_from_slice(&data[previous..range.start]);
        output.push(b'\n');
        output.extend_from_slice(replacement.as_bytes());
        output.push(b'\n');
        previous = range.end;
    }
    output.extend_from_slice(&data[previous..]);
    Ok(output)
}
