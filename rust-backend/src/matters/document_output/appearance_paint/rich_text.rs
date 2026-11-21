//! Paint events from local rich text used by regenerated annotations.
use super::GeneratedPaint;
use regex::Regex;
use std::sync::LazyLock;

static STYLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(?:^|\s)style\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#)
        .expect("valid style attribute pattern")
});

#[derive(Clone, Default)]
struct Style {
    size: f64,
    color: String,
    hidden: bool,
    background: bool,
    height: bool,
}

impl Style {
    fn nested(&self, declarations: &str) -> Self {
        let mut style = Self {
            size: self.size,
            color: self.color.clone(),
            hidden: self.hidden,
            ..Default::default()
        };
        for (property, value) in declarations
            .split(';')
            .filter_map(|part| part.split_once(':'))
        {
            let property = property.trim().to_ascii_lowercase();
            let value = value
                .split('!')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            match property.as_str() {
                "color" => style.color = value,
                "display" if value == "none" => style.hidden = true,
                "visibility" if matches!(value.as_str(), "hidden" | "collapse") => {
                    style.hidden = true
                }
                "background" | "background-color" => {
                    style.background = !matches!(value.as_str(), "" | "transparent" | "none");
                }
                "height" => style.height = length(&value, self.size).is_some_and(|n| n > 0.0),
                "font-size" => style.size = length(&value, self.size).unwrap_or(self.size),
                _ => {}
            }
        }
        style
    }
}

fn length(value: &str, inherited: f64) -> Option<f64> {
    let at = value
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, '-' | '+' | '.')))
        .unwrap_or(value.len());
    let number = value[..at].parse::<f64>().ok()?.max(0.0);
    let factor = match value[at..].trim() {
        "pt" | "" => 1.0,
        "px" => 0.75,
        "em" | "rem" => inherited,
        "%" => inherited / 100.0,
        "in" => 72.0,
        "cm" => 72.0 / 2.54,
        "mm" => 72.0 / 25.4,
        _ => return None,
    };
    let result = number * factor;
    result.is_finite().then_some(result)
}

struct Element {
    name: String,
    style: Style,
    painted_background: bool,
}

impl Element {
    fn background(&mut self, line: bool) -> usize {
        if self.style.hidden || !self.style.background || self.painted_background {
            return 0;
        }
        self.painted_background = true;
        1 + usize::from(
            line && matches!(
                self.name.as_str(),
                "p" | "li" | "pre" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
            ),
        )
    }
}

fn tag_end(input: &str) -> Option<usize> {
    let mut quote = None;
    for (index, character) in input.char_indices() {
        if let Some(open) = quote {
            if character == open {
                quote = None;
            }
        } else if matches!(character, '\'' | '"') {
            quote = Some(character);
        } else if character == '>' {
            return Some(index);
        }
    }
    None
}

fn visible_text(mut text: &str) -> bool {
    while let Some((plain, entity)) = text.split_once('&') {
        if !plain.trim().is_empty() {
            return true;
        }
        let Some((entity, tail)) = entity.split_once(';') else {
            return true;
        };
        let whitespace = if let Some(hex) = entity
            .strip_prefix("#x")
            .or_else(|| entity.strip_prefix("#X"))
        {
            u32::from_str_radix(hex, 16)
                .ok()
                .and_then(char::from_u32)
                .is_some_and(char::is_whitespace)
        } else if let Some(decimal) = entity.strip_prefix('#') {
            decimal
                .parse::<u32>()
                .ok()
                .and_then(char::from_u32)
                .is_some_and(char::is_whitespace)
        } else {
            matches!(
                entity,
                "nbsp" | "ensp" | "emsp" | "thinsp" | "Tab" | "NewLine"
            )
        };
        if !whitespace {
            return true;
        }
        text = tail;
    }
    !text.trim().is_empty()
}

#[derive(Default)]
struct Trace {
    paints: Vec<GeneratedPaint>,
    text_color: Option<String>,
}

impl Trace {
    fn background(&mut self, paths: usize) {
        if paths > 0 {
            self.paints.push(GeneratedPaint::Paths(paths));
            self.text_color = None;
        }
    }

    fn text(&mut self, elements: &mut [Element], ink: bool) {
        let style = &elements.last().expect("root style").style;
        if style.hidden {
            return;
        }
        let ink = ink && style.size > 0.0;
        let color = style.color.clone();
        let paths = elements
            .iter_mut()
            .map(|element| element.background(true))
            .sum();
        self.background(paths);
        if self.text_color.as_ref() == Some(&color) {
            if let Some(GeneratedPaint::Text(previous)) = self.paints.last_mut() {
                *previous |= ink;
                return;
            }
        }
        self.paints.push(GeneratedPaint::Text(ink));
        self.text_color = Some(color);
    }

    fn close(&mut self, elements: &mut Vec<Element>, index: usize) {
        for mut element in elements.drain(index..) {
            if element.style.height {
                self.background(element.background(false));
            }
        }
    }
}

pub(super) fn paint(mut input: &str, declarations: &str, size: f64) -> Vec<GeneratedPaint> {
    let mut trace = Trace::default();
    let mut elements = vec![Element {
        name: String::new(),
        style: Style {
            size,
            ..Default::default()
        }
        .nested(declarations),
        painted_background: false,
    }];
    while !input.is_empty() {
        if let Some(comment) = input.strip_prefix("<!--") {
            input = comment.split_once("-->").map_or("", |(_, tail)| tail);
            continue;
        }
        if let Some(tag) = input.strip_prefix('<') {
            let Some(end) = tag_end(tag) else { break };
            input = &tag[end + 1..];
            let tag = tag[..end].trim();
            if tag.starts_with(['!', '?']) {
                continue;
            }
            let closing = tag.starts_with('/');
            let body = tag.trim_start_matches('/');
            let name_end = body
                .find(|c: char| c.is_whitespace() || c == '/')
                .unwrap_or(body.len());
            let name = body[..name_end].to_ascii_lowercase();
            if closing {
                if let Some(index) = elements
                    .iter()
                    .rposition(|element| element.name == name)
                    .filter(|&index| index > 0)
                {
                    trace.close(&mut elements, index);
                }
                continue;
            }
            let declarations = STYLE.captures(&body[name_end..]).and_then(|matches| {
                (1..=3).find_map(|index| matches.get(index).map(|item| item.as_str()))
            });
            let mut style = elements
                .last()
                .expect("root style")
                .style
                .nested(declarations.unwrap_or_default());
            style.hidden |= matches!(name.as_str(), "head" | "script" | "style" | "title");
            let empty = tag.ends_with('/')
                || matches!(
                    name.as_str(),
                    "br" | "img" | "hr" | "meta" | "link" | "input" | "wbr"
                );
            let paints_text = matches!(name.as_str(), "li" | "img");
            elements.push(Element {
                name,
                style,
                painted_background: false,
            });
            if paints_text {
                trace.text(&mut elements, true);
            }
            if empty {
                let index = elements.len() - 1;
                trace.close(&mut elements, index);
            }
        } else {
            let end = input.find('<').unwrap_or(input.len());
            let value = &input[..end];
            let ink = visible_text(value);
            let inside_text = elements
                .iter()
                .any(|element| !matches!(element.name.as_str(), "" | "html" | "body" | "div"));
            if ink || inside_text && !value.contains(['\n', '\r']) {
                trace.text(&mut elements, ink);
            }
            input = &input[end..];
        }
    }
    trace.close(&mut elements, 0);
    trace.paints
}

#[cfg(test)]
mod tests {
    use super::*;
    use GeneratedPaint::{Paths, Text};

    #[test]
    fn text_and_box_paints_keep_their_order() {
        let markup = r#"<p style="background-color:red">One</p><p style="background-color:green;height:20pt"></p>"#;
        assert_eq!(paint(markup, "", 12.0), [Paths(2), Text(true), Paths(1)]);
        let markup = r#"<p><span style="color:red">One</span><i style="color:green">Two</i></p>"#;
        assert_eq!(paint(markup, "", 12.0), [Text(true), Text(true)]);
    }

    #[test]
    fn rich_runs_distinguish_empty_layout_from_visible_glyphs() {
        assert_eq!(paint("<p>&nbsp;</p>", "", 12.0), [Text(false)]);
        assert_eq!(paint("<p>Zero size</p>", "", 0.0), [Text(false)]);
        assert_eq!(paint("<p>Styled</p>", "font-size:12pt", 0.0), [Text(true)]);
        assert!(paint(
            r#"<p style="display:none">Hidden</p><!--ignored-->"#,
            "",
            12.0
        )
        .is_empty());
    }
}
