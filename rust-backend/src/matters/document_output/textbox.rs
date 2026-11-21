use super::assembly;
use pdf_oxide::{writer::FontManager, PdfDocument};

pub(in crate::matters) struct Textbox {
    pub name: &'static str,
    metrics: FontManager,
    ascender: f64,
    descender: f64,
}

impl Textbox {
    pub fn new(pdf: &PdfDocument, page: usize, name: &'static str) -> Self {
        // Font metrics are read differently for a newly installed Base14 font
        // and a font already registered under its textbox resource name.
        if assembly::has_textbox_font(pdf, page, name) {
            Self::registered(name)
        } else {
            let (ascender, descender) = match name {
                "Helvetica-Bold" => (1.070_f32, -0.307_f32),
                "Times-Roman" => (1.053, -0.281),
                "Times-Bold" => (1.044, -0.341),
                "Courier" => (0.932, -0.317),
                "Courier-Bold" => (1.007, -0.393),
                _ => (1.075, -0.299),
            };
            Self::with_metrics(name, f64::from(ascender), f64::from(descender))
        }
    }

    pub fn registered(name: &'static str) -> Self {
        Self::with_metrics(name, 0.96, -0.24)
    }

    fn with_metrics(name: &'static str, ascender: f64, descender: f64) -> Self {
        Self {
            name,
            metrics: FontManager::new(),
            ascender,
            descender,
        }
    }

    pub fn measure(&self, text: &str, size: f64) -> f64 {
        let font = self.metrics.get_font_or_default(self.name);
        text.chars()
            .map(|ch| f64::from(font.char_width(ch) / 1000.0))
            .sum::<f64>()
            * size
    }

    pub fn helvetica_ink(&self, text: &str, size: f64, x: f64, baseline: f64) -> [f64; 4] {
        let font = self.metrics.get_font_or_default(self.name);
        let mut left = x;
        let mut ink = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        for byte in text.bytes() {
            let [x0, y0, x1, y1] = super::helvetica_bounds::ASCII_BOUNDS[usize::from(byte - b' ')]
                .map(|unit| f64::from(unit) * size / 1000.0);
            ink[0] = ink[0].min(left + x0);
            ink[1] = ink[1].min(baseline + y0);
            ink[2] = ink[2].max(left + x1);
            ink[3] = ink[3].max(baseline + y1);
            left += f64::from(font.char_width(char::from(byte)) / 1000.0) * size;
        }
        // The reference keeps one point of tolerance around the glyph block.
        [ink[0] - 1.0, ink[1] - 1.0, ink[2] + 1.0, ink[3] + 1.0]
    }

    pub fn baseline(&self, top: f64, size: f64, line: usize) -> f64 {
        top - size * self.ascender * (1.0 + line as f64 * self.line_height())
    }

    pub fn fits(&self, lines: usize, size: f64, height: f64) -> bool {
        // The rotated rectangle was narrowed to PDF's float coordinates before
        // the reference renderer checked this height.
        (lines as f64 * self.line_height() - self.descender) * size <= height + 1e-5
    }

    fn line_height(&self) -> f64 {
        let height = self.ascender - self.descender;
        if height <= 1.0 {
            1.2
        } else {
            height
        }
    }

    pub fn wrap<'a>(&self, text: &'a str, width: f64, size: f64) -> Vec<(&'a str, bool)> {
        let mut lines: Vec<(&str, bool)> = Vec::new();
        let space = self.measure(" ", size);
        for paragraph in text.lines() {
            let mut start = None;
            let mut room = width;
            let mut offset = 0;
            // Empty words consume spaces too. A trailing space that cannot fit
            // can produce a blank line before the next explicit paragraph.
            for word in paragraph.split(' ') {
                let word_at = offset;
                offset += word.len() + 1;
                let word_width = self.measure(word, size);
                if word_width <= room {
                    start.get_or_insert(word_at);
                    room -= word_width + space;
                    continue;
                }

                if let Some(at) = start {
                    lines.push((paragraph[at..word_at].trim_end_matches(' '), true));
                }
                let mut fragment = word_at;
                if word_width > width {
                    if let Some((_, justify)) = lines.last_mut() {
                        *justify = false;
                    }
                    for (index, ch) in word.char_indices() {
                        let end = word_at + index;
                        let char_width = self.measure(&word[index..index + ch.len_utf8()], size);
                        if self.measure(&paragraph[fragment..end], size) > width - char_width {
                            lines.push((&paragraph[fragment..end], false));
                            fragment = end;
                        }
                    }
                }
                start = Some(fragment);
                room = width - self.measure(&paragraph[fragment..offset - 1], size) - space;
            }
            if let Some(at) = start {
                lines.push((paragraph[at..].trim_end_matches(' '), false));
            }
        }
        if lines.len() > 1 && lines.last().is_some_and(|(line, _)| line.is_empty()) {
            lines.pop();
        }
        if let Some((_, justify)) = lines.last_mut() {
            *justify = false;
        }
        lines
    }
}
