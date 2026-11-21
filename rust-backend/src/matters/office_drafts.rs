//! Editable Office review drafts from a frozen PDF or index.
use icu_properties::{props::GeneralCategory, CodePointMapData};
use pdf_oxide::{converters::ConversionOptions, geometry::Rect, layout::Word, PdfDocument};
use std::{
    collections::{BTreeMap, VecDeque},
    fmt::Write as _,
    io::{Cursor, Read, Write as _},
};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

const CONTENT: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
const REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const OFFICE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const SHEET: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const WORD: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const PRESENTATION: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
const DRAWING: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const CONTENT_BASE: &str = "application/vnd.openxmlformats-officedocument";
const INVALID: &str = "Unable to create editable Office review draft";
const MAX_REVIEW_CHARS: usize = 10_000_000;
const TEXT_LIMIT: &str = "Office review draft exceeds the 10 million character limit";
const PDF_TEXT_ERROR: &str = "Unable to extract completed PDF text";

fn escape(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '\t' | '\n' | '\r' => escaped.push(character),
            c if c >= ' ' && c != '\u{fffe}' && c != '\u{ffff}' => escaped.push(c),
            _ => {}
        }
    }
    escaped
}

fn package(parts: BTreeMap<String, Vec<u8>>) -> Result<Vec<u8>, &'static str> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, data) in parts {
        zip.start_file(name, options).map_err(|_| INVALID)?;
        zip.write_all(&data).map_err(|_| INVALID)?;
    }
    zip.finish().map(Cursor::into_inner).map_err(|_| INVALID)
}

fn xml_parts(items: impl IntoIterator<Item = (&'static str, String)>) -> BTreeMap<String, Vec<u8>> {
    items
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value.into_bytes()))
        .collect()
}

fn root_rels(target: &str) -> String {
    format!(
        r#"<Relationships xmlns="{REL}"><Relationship Id="rId1" Type="{OFFICE}/officeDocument" Target="{target}"/></Relationships>"#
    )
}

fn types(overrides: &[(&str, &str)]) -> String {
    let entries = overrides
        .iter()
        .map(|(name, ty)| {
            format!(r#"<Override PartName="/{name}" ContentType="{CONTENT_BASE}.{ty}+xml"/>"#)
        })
        .collect::<String>();
    format!(
        r#"<Types xmlns="{CONTENT}"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/>{entries}</Types>"#
    )
}

struct ReviewLine {
    top: f64,
    bottom: f64,
    words: Vec<Word>,
}

fn top(word: &Word) -> f64 {
    -f64::from(word.bbox.y) - f64::from(word.bbox.height)
}

fn bottom(word: &Word) -> f64 {
    -f64::from(word.bbox.y)
}

// Match the reference's three-point row tolerance and average-glyph spacing.
fn review_lines(words: Vec<Word>) -> Vec<ReviewLine> {
    let mut lines: Vec<ReviewLine> = Vec::new();
    for word in words {
        let (top, bottom) = (top(&word), bottom(&word));
        if let Some(line) = lines
            .last_mut()
            .filter(|line| (line.top - top).abs() <= 3.0 || (line.bottom - bottom).abs() <= 3.0)
        {
            line.top = line.top.min(top);
            line.bottom = line.bottom.max(bottom);
            line.words.push(word);
        } else {
            lines.push(ReviewLine {
                top,
                bottom,
                words: vec![word],
            });
        }
    }
    for line in &mut lines {
        line.words.sort_by(|a, b| a.bbox.x.total_cmp(&b.bbox.x));
    }
    lines
}

fn review_text(mut words: Vec<Word>, remaining: usize) -> Result<String, &'static str> {
    words.sort_by(|a, b| {
        bottom(a)
            .total_cmp(&bottom(b))
            .then_with(|| a.bbox.x.total_cmp(&b.bbox.x))
    });
    let ordered = review_lines(words)
        .into_iter()
        .flat_map(|line| line.words)
        .collect();
    let mut lines = review_lines(ordered);
    lines.sort_by(|a, b| a.bottom.total_cmp(&b.bottom));
    let mut text = String::new();
    let mut consumed = 0;
    // Office drops blank lines and left indentation, so neither contributes to the limit.
    for line in lines {
        if !text.is_empty() {
            text.push('\n');
        }
        let mut end = None;
        for word in line.words {
            let width = f64::from(word.bbox.width);
            let left = f64::from(word.bbox.x);
            let length = word.text.chars().count();
            let gap = left - end.unwrap_or(left);
            let spaces = if gap > 0.0 && width > 0.0 {
                (gap / width * length as f64).round_ties_even().max(1.0) as usize
            } else {
                usize::from(gap > 0.0)
            };
            if spaces.saturating_add(length) > remaining - consumed {
                return Err(TEXT_LIMIT);
            }
            consumed += spaces + length;
            text.extend(std::iter::repeat_n(' ', spaces));
            text.push_str(&word.text);
            end = Some(left + width);
        }
    }
    Ok(text)
}

fn page_review_text(
    pdf: &PdfDocument,
    index: usize,
    remaining: usize,
) -> Result<String, &'static str> {
    let spans = pdf.extract_spans(index).map_err(|_| PDF_TEXT_ERROR)?;
    let rotation = pdf.get_page_rotation(index).map_err(|_| PDF_TEXT_ERROR)?;
    // Coordinate joins assume left-to-right, horizontal text. Keep Oxide's
    // directional extraction for pages where that would break a writing system.
    if rotation != 0
        || spans.iter().any(|span| {
            span.wmode != 0
                || span.rotation_degrees.abs() > 0.5
                || span.text.chars().any(|c| {
                    use unicode_bidi::BidiClass::{AL, R};
                    matches!(unicode_bidi::bidi_class(c), AL | R)
                })
        })
    {
        return pdf
            .extract_text_with_options(
                index,
                &ConversionOptions {
                    extract_tables: false,
                    ..Default::default()
                },
            )
            .map_err(|_| PDF_TEXT_ERROR);
    }
    drop(spans);
    let (left, bottom, right, top) = pdf
        .get_page_visible_box(index)
        .map_err(|_| PDF_TEXT_ERROR)?;
    let visible = |rect: Rect| {
        rect.x < right
            && rect.x + rect.width > left
            && rect.y < top
            && rect.y + rect.height > bottom
    };
    let words = pdf
        .extract_words(index)
        .map_err(|_| PDF_TEXT_ERROR)?
        .into_iter()
        .filter_map(|mut word| {
            if !visible(word.bbox) {
                return None;
            }
            let count = word.chars.len();
            word.chars.retain(|glyph| visible(glyph.bbox));
            if word.chars.len() == count {
                Some(word)
            } else if word.chars.is_empty() {
                None
            } else {
                Some(Word::from_chars(word.chars))
            }
        })
        .collect();
    review_text(words, remaining)
}

fn pdf_pages(bytes: Vec<u8>) -> Result<Vec<Vec<String>>, &'static str> {
    let pdf = PdfDocument::from_bytes(bytes).map_err(|_| "Unable to open completed PDF")?;
    let count = pdf.page_count().map_err(|_| INVALID)?;
    if count > 2000 {
        return Err("Office review draft is limited to 2,000 PDF pages");
    }
    let mut pages = Vec::new();
    let mut size = 0;
    for index in 0..count {
        let text = page_review_text(&pdf, index, MAX_REVIEW_CHARS - size)?;
        let lines = text
            .split(|c| matches!(c, '\n'..='\r' | '\u{1c}'..='\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}'))
            .map(|line| {
                line.chars()
                    .filter(|c| *c == '\t' || (*c >= ' ' && *c != '\u{fffe}' && *c != '\u{ffff}'))
                    .collect::<String>()
            })
            .map(|line| line.trim().to_owned())
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();
        size += lines.iter().map(|line| line.chars().count()).sum::<usize>();
        if size > MAX_REVIEW_CHARS {
            return Err(TEXT_LIMIT);
        }
        pages.push(lines);
    }
    if pages.iter().all(Vec::is_empty) {
        return Err("This PDF has no extractable text. Review OCR before making an editable draft");
    }
    Ok(pages)
}

fn word(pages: &[Vec<String>]) -> Result<Vec<u8>, &'static str> {
    let mut body = String::new();
    for (index, lines) in pages.iter().enumerate() {
        if index > 0 {
            body.push_str(r#"<w:p><w:r><w:br w:type="page"/></w:r></w:p>"#);
        }
        let title = format!("PDF page {} — editable review draft", index + 1);
        let lines = if lines.is_empty() {
            vec!["[No extractable text on this page; inspect the source PDF]"]
        } else {
            lines.iter().map(String::as_str).collect()
        };
        for line in std::iter::once(title.as_str()).chain(lines) {
            write!(
                body,
                r#"<w:p><w:r><w:t xml:space="preserve">{}</w:t></w:r></w:p>"#,
                escape(line)
            )
            .unwrap();
        }
    }
    let target = "word/document.xml";
    package(xml_parts([
        (
            "[Content_Types].xml",
            types(&[(target, "wordprocessingml.document.main")]),
        ),
        ("_rels/.rels", root_rels(target)),
        (
            "word/document.xml",
            format!(
                r#"<w:document xmlns:w="{WORD}"><w:body>{body}<w:sectPr/></w:body></w:document>"#
            ),
        ),
    ]))
}

pub(super) fn parse_csv(bytes: &[u8]) -> Result<Vec<Vec<String>>, &'static str> {
    let input = std::str::from_utf8(bytes).map_err(|_| "Invalid index CSV encoding")?;
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    let mut characters = input.chars().peekable();
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut field_start = true;
    let mut seen = false;
    while let Some(character) = characters.next() {
        match character {
            '"' if quoted => {
                if characters.next_if_eq(&'"').is_some() {
                    field.push('"');
                } else {
                    quoted = false;
                }
            }
            '"' if field_start => quoted = true,
            ',' if !quoted => {
                row.push(std::mem::take(&mut field));
                if row.len() >= 16_384 {
                    return Err("Index exceeds Excel worksheet limits");
                }
                seen = true;
                field_start = true;
                continue;
            }
            '\r' | '\n' if !quoted => {
                if character == '\r' {
                    characters.next_if_eq(&'\n');
                }
                if seen {
                    row.push(std::mem::take(&mut field));
                }
                rows.push(std::mem::take(&mut row));
                if rows.len() > 1_048_576 {
                    return Err("Index exceeds Excel worksheet limits");
                }
                seen = false;
                field_start = true;
                continue;
            }
            value => field.push(value),
        }
        seen = true;
        field_start = false;
        if field.len() > 32_767 * 4 {
            return Err("Index cell exceeds Excel text limit");
        }
    }
    if seen {
        row.push(field);
        rows.push(row);
    }
    if rows.len() > 1_048_576 {
        return Err("Index exceeds Excel worksheet limits");
    }
    if rows.first().is_none_or(Vec::is_empty) {
        return Err("The completed build has no index rows to export");
    }
    Ok(rows)
}

fn letters(mut column: usize) -> String {
    let mut output = Vec::new();
    while column > 0 {
        column -= 1;
        output.push((b'A' + (column % 26) as u8) as char);
        column /= 26;
    }
    output.iter().rev().collect()
}

fn excel(bytes: &[u8]) -> Result<Vec<u8>, &'static str> {
    let rows = parse_csv(bytes)?;
    let maximum = rows.iter().map(Vec::len).max().unwrap_or(1);
    let mut widths = vec![14; maximum];
    let mut cells = String::new();
    for (i, row) in rows.iter().enumerate() {
        let number = i + 1;
        write!(cells, r#"<row r="{number}">"#).unwrap();
        for (column, value) in row.iter().enumerate() {
            let len = value.chars().count();
            if len > 32_767 {
                return Err("Index cell exceeds Excel text limit");
            }
            widths[column] = widths[column].max((len + 2).min(64));
            let col = letters(column + 1);
            write!(
                cells,
                r#"<c r="{col}{number}" t="inlineStr"><is><t xml:space="preserve">{}</t></is></c>"#,
                escape(value)
            )
            .unwrap();
        }
        cells.push_str("</row>");
    }
    let columns = widths
        .iter()
        .enumerate()
        .map(|(i, width)| {
            let number = i + 1;
            format!(r#"<col min="{number}" max="{number}" width="{width}" customWidth="1"/>"#)
        })
        .collect::<String>();
    let last = format!("{}{}", letters(maximum), rows.len());
    let worksheet = format!(
        r#"<worksheet xmlns="{SHEET}"><sheetViews><sheetView workbookViewId="0"><pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/></sheetView></sheetViews><cols>{columns}</cols><sheetData>{cells}</sheetData><autoFilter ref="A1:{last}"/></worksheet>"#
    );
    package(xml_parts([
        (
            "[Content_Types].xml",
            types(&[
                ("xl/workbook.xml", "spreadsheetml.sheet.main"),
                ("xl/worksheets/sheet1.xml", "spreadsheetml.worksheet"),
            ]),
        ),
        ("_rels/.rels", root_rels("xl/workbook.xml")),
        (
            "xl/workbook.xml",
            format!(
                r#"<workbook xmlns="{SHEET}" xmlns:r="{OFFICE}"><sheets><sheet name="Bundle index" sheetId="1" r:id="rId1"/></sheets></workbook>"#
            ),
        ),
        (
            "xl/_rels/workbook.xml.rels",
            format!(
                r#"<Relationships xmlns="{REL}"><Relationship Id="rId1" Type="{OFFICE}/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#
            ),
        ),
        ("xl/worksheets/sheet1.xml", worksheet),
    ]))
}

fn read_template() -> Result<BTreeMap<String, Vec<u8>>, &'static str> {
    let bytes = include_bytes!("../../../backend/app/assets/editable-draft-template.pptx");
    let mut reader = ZipArchive::new(Cursor::new(&bytes[..])).map_err(|_| INVALID)?;
    let mut parts = BTreeMap::new();
    for index in 0..reader.len() {
        let mut part = reader.by_index(index).map_err(|_| INVALID)?;
        let mut value = Vec::new();
        part.read_to_end(&mut value).map_err(|_| INVALID)?;
        parts.insert(part.name().map_err(|_| INVALID)?.into_owned(), value);
    }
    Ok(parts)
}

fn take_text(parts: &mut BTreeMap<String, Vec<u8>>, name: &str) -> Result<String, &'static str> {
    String::from_utf8(parts.remove(name).ok_or(INVALID)?).map_err(|_| INVALID)
}

fn slide_chunks(text: &[char]) -> VecDeque<&[char]> {
    let category = CodePointMapData::<GeneralCategory>::new();
    let is_word = |c: char| c == '_' || c.is_alphanumeric();
    let letter_at = |index| {
        text.get(index)
            .copied()
            .is_some_and(|c| is_word(c) && category.get(c) != GeneralCategory::DecimalNumber)
    };
    let mut chunks = VecDeque::new();
    let mut start = 0;
    let mut index = 0;
    while index < text.len() {
        if text[index] == ' ' {
            if start < index {
                chunks.push_back(&text[start..index]);
            }
            start = index;
            while text.get(index) == Some(&' ') {
                index += 1;
            }
            chunks.push_back(&text[start..index]);
            start = index;
            continue;
        }
        if text[index] == '-' {
            let previous = index.checked_sub(1).map(|i| text[i]);
            if previous.is_some_and(|c| is_word(c) || "!\"'&.,?".contains(c)) {
                let end = index + text[index..].iter().take_while(|c| **c == '-').count();
                if end > index + 1 && text.get(end).copied().is_some_and(is_word) {
                    chunks.push_back(&text[start..index]);
                    chunks.push_back(&text[index..end]);
                    start = end;
                    index = end;
                    continue;
                }
                let left = letter_at(index - 1)
                    && (index >= 2 && letter_at(index - 2)
                        || index >= 3 && text[index - 2] == '-' && letter_at(index - 3));
                let next_letter = end + usize::from(text.get(end + 1) == Some(&'-')) + 1;
                if end == index + 1 && left && letter_at(end) && letter_at(next_letter) {
                    chunks.push_back(&text[start..end]);
                    start = end;
                }
            }
        }
        index += 1;
    }
    if start < text.len() {
        chunks.push_back(&text[start..]);
    }
    chunks
}

fn slide_lines(value: &str) -> Vec<String> {
    const WIDTH: usize = 110;
    let mut expanded = Vec::new();
    for c in value.chars() {
        match c {
            '\t' => expanded.extend(std::iter::repeat_n(' ', 8 - expanded.len() % 8)),
            '\n'..='\r' => expanded.push(' '),
            c => expanded.push(c),
        }
    }
    let mut chunks = slide_chunks(&expanded);
    let whitespace = |chunk: &[char]| chunk.iter().all(|c| c.is_whitespace());
    let mut lines = Vec::new();
    while !chunks.is_empty() {
        let mut current = Vec::new();
        let mut remaining = WIDTH;
        if !lines.is_empty() && chunks.front().is_some_and(|chunk| whitespace(chunk)) {
            chunks.pop_front();
        }
        while let Some(chunk) = chunks.pop_front() {
            if chunk.len() <= remaining {
                remaining -= chunk.len();
                current.push(chunk);
                continue;
            }
            if chunk.len() > WIDTH && remaining > 0 {
                let end = chunk[..remaining]
                    .iter()
                    .rposition(|c| *c == '-')
                    .filter(|end| chunk[..*end].iter().any(|c| *c != '-'))
                    .map_or(remaining, |end| end + 1);
                current.push(&chunk[..end]);
                chunks.push_front(&chunk[end..]);
            } else {
                chunks.push_front(chunk);
            }
            break;
        }
        if current.last().is_some_and(|chunk| whitespace(chunk)) {
            current.pop();
        }
        if !current.is_empty() {
            lines.push(current.into_iter().flatten().collect());
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn replace_template_node(
    xml: String,
    predicate: impl Fn(roxmltree::Node<'_, '_>) -> bool,
    content: &str,
) -> Result<String, &'static str> {
    let document = roxmltree::Document::parse(&xml).map_err(|_| INVALID)?;
    let range = document
        .descendants()
        .find(|node| node.is_element() && predicate(*node))
        .ok_or("The Office template is missing a required component")?
        .range();
    let mut result = xml;
    result.replace_range(range, content);
    Ok(result)
}

fn pptx(pages: &[Vec<String>]) -> Result<Vec<u8>, &'static str> {
    let mut parts = read_template()?;
    let slide = take_text(&mut parts, "ppt/slides/slide1.xml")?;
    let layout = parts
        .remove("ppt/slides/_rels/slide1.xml.rels")
        .ok_or(INVALID)?;
    let presentation = take_text(&mut parts, "ppt/presentation.xml")?;
    let relationships = take_text(&mut parts, "ppt/_rels/presentation.xml.rels")?;
    let content_types = take_text(&mut parts, "[Content_Types].xml")?;
    let template = roxmltree::Document::parse(&slide).map_err(|_| INVALID)?;
    let title = template
        .descendants()
        .find(|node| node.has_tag_name((DRAWING, "t")))
        .ok_or("PowerPoint template is missing the editable title")?
        .range();
    let body = template
        .descendants()
        .filter(|node| node.has_tag_name((PRESENTATION, "txBody")))
        .nth(1)
        .ok_or("PowerPoint template is missing the editable content")?;
    let paragraphs = body
        .children()
        .filter(|node| node.has_tag_name((DRAWING, "p")))
        .map(|node| node.range())
        .collect::<Vec<_>>();
    let content = paragraphs.first().ok_or(INVALID)?.start..paragraphs.last().ok_or(INVALID)?.end;
    if content.start < title.end {
        return Err(INVALID);
    }
    let mut identities = String::new();
    let mut rels = String::new();
    let mut overrides = String::new();
    let mut index = 0;
    let paragraph = |line: &str| {
        format!(
            r#"<a:p><a:pPr lvl="0" indent="0" marL="0"><a:buNone /></a:pPr><a:r><a:rPr /><a:t>{}</a:t></a:r></a:p>"#,
            escape(line)
        )
    };
    for (page, lines) in pages.iter().enumerate() {
        let lines = if lines.is_empty() {
            vec!["[No extractable text; inspect the source PDF]"]
        } else {
            lines.iter().map(String::as_str).collect()
        };
        let mut chunks = Vec::<Vec<String>>::new();
        let mut chars = 0;
        for line in lines.into_iter().flat_map(slide_lines) {
            let length = line.chars().count();
            if chunks
                .last()
                .is_none_or(|current| current.len() == 10 || chars + length > 900)
            {
                chunks.push(Vec::new());
                chars = 0;
            }
            chunks.last_mut().ok_or(INVALID)?.push(line);
            chars += length;
        }
        for (part, chunk) in chunks.iter().enumerate() {
            index += 1;
            let text = if chunks.len() > 1 {
                format!("PDF page {} · part {}", page + 1, part + 1)
            } else {
                format!("PDF page {}", page + 1)
            };
            let lines = chunk.iter().map(|line| paragraph(line)).collect::<String>();
            let mut filled = slide.clone();
            filled.replace_range(content.clone(), &lines);
            filled.replace_range(title.clone(), &format!("<a:t>{}</a:t>", escape(&text)));
            let name = format!("slide{index}.xml");
            parts.insert(format!("ppt/slides/{name}"), filled.into_bytes());
            parts.insert(format!("ppt/slides/_rels/{name}.rels"), layout.clone());
            let relationship = format!("rId{}", 100 + index);
            write!(
                identities,
                r#"<p:sldId id="{}" r:id="{relationship}"/>"#,
                255 + index
            )
            .unwrap();
            write!(rels,r#"<Relationship Id="{relationship}" Type="{OFFICE}/slide" Target="slides/{name}"/>"#).unwrap();
            write!(overrides,r#"<Override PartName="/ppt/slides/{name}" ContentType="{CONTENT_BASE}.presentationml.slide+xml"/>"#).unwrap();
        }
    }
    let presentation = replace_template_node(
        presentation,
        |node| node.has_tag_name((PRESENTATION, "sldId")),
        &identities,
    )?;
    let relationships = replace_template_node(
        relationships,
        |node| {
            node.attribute("Type")
                .is_some_and(|value| value.ends_with("/slide"))
        },
        &rels,
    )?;
    let content_types = replace_template_node(
        content_types,
        |node| node.attribute("PartName") == Some("/ppt/slides/slide1.xml"),
        &overrides,
    )?;
    parts.extend(xml_parts([
        ("ppt/presentation.xml", presentation),
        ("ppt/_rels/presentation.xml.rels", relationships),
        ("[Content_Types].xml", content_types),
    ]));
    package(parts)
}

pub(super) fn convert(filename: &str, kind: &str, bytes: Vec<u8>) -> Result<Vec<u8>, &'static str> {
    match kind {
        "xlsx" if filename == "index.csv" => excel(&bytes),
        "xlsx" => Err("Excel export is derived from the completed index CSV"),
        "docx" if filename.ends_with(".pdf") => word(&pdf_pages(bytes)?),
        "pptx" if filename.ends_with(".pdf") => pptx(&pdf_pages(bytes)?),
        _ => Err("Word and PowerPoint drafts require a completed PDF"),
    }
}

pub(super) fn review_note(kind: &str) -> Option<&'static str> {
    match kind {
        "docx" => Some("Editable text draft; PDF layout, images and signatures are not reproduced"),
        "xlsx" => Some("Editable index workbook derived from the identified build CSV; verify page ranges against PDF"),
        "pptx" => Some("Editable text slides; PDF layout, images and signatures are not reproduced"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_oxide::layout::TextChar;

    #[test]
    fn review_lines_follow_page_positions_and_preserve_column_spacing() {
        let word = |text: &str, x, y| {
            Word::from_chars(
                text.chars()
                    .enumerate()
                    .map(|(i, c)| TextChar {
                        char: c,
                        bbox: Rect::new(x + i as f32 * 5.0, y, 5.0, 10.0),
                        ..TextChar::default()
                    })
                    .collect(),
            )
        };
        let words = vec![
            word("C", 10.0, 30.0),
            word("B", 35.0, 60.0),
            word("A", 10.0, 61.5),
        ];
        assert_eq!(review_text(words.clone(), 7), Ok("A    B\nC".into()));
        assert_eq!(review_text(words, 6), Err(TEXT_LIMIT));
    }

    #[test]
    fn index_parser_preserves_physical_rows_and_quoted_cells() {
        let rows = parse_csv(
            b"\xef\xbb\xbfid,title\r\n\r\n001,\"two\r\nlines and \"\"quotes\"\"\"\rb,=1+2\n\"\",\"\"\n",
        )
        .unwrap();
        assert_eq!(rows[0], ["id", "title"]);
        assert!(rows[1].is_empty());
        assert_eq!(rows[2], ["001", "two\r\nlines and \"quotes\""]);
        assert_eq!(rows[3], ["b", "=1+2"]);
        assert_eq!(rows[4], ["", ""]);
        assert_eq!(parse_csv(b"h\n\"quoted\n").unwrap()[1], ["quoted\n"]);
        assert_eq!(
            parse_csv(b"\nh"),
            Err("The completed build has no index rows to export")
        );
    }

    #[test]
    fn spreadsheet_counts_unicode_characters_and_keeps_values_as_editable_text() {
        let value = "😀".repeat(32_767);
        let input = format!("value,code\n{value},001\n=1+2,\n");
        let bytes = excel(input.as_bytes()).unwrap();
        let mut package = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        package
            .by_name("xl/worksheets/sheet1.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        let document = roxmltree::Document::parse(&xml).unwrap();
        let cells = document
            .descendants()
            .filter(|node| node.has_tag_name((SHEET, "c")))
            .map(|cell| {
                assert_eq!(cell.attribute("t"), Some("inlineStr"));
                cell.descendants()
                    .find(|node| node.has_tag_name((SHEET, "t")))
                    .unwrap()
                    .text()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            &cells[2..],
            &[Some(value.as_str()), Some("001"), Some("=1+2"), None]
        );
        assert_eq!(
            excel(format!("value\n{value}😀\n").as_bytes()),
            Err("Index cell exceeds Excel text limit")
        );
    }

    #[test]
    fn slide_wrap_preserves_spacing_hyphens_and_long_words() {
        assert_eq!(slide_lines("A\tB  C\u{a0}D"), ["A       B  C\u{a0}D"]);
        let prefix = "x".repeat(99);
        assert_eq!(
            slide_lines(&format!("{prefix} repeat-able")),
            [format!("{prefix} repeat-"), "able".into()]
        );
        let word = "क".repeat(115);
        assert_eq!(
            slide_lines(&format!("Open {word} end")),
            [
                format!("Open {}", "क".repeat(105)),
                format!("{} end", "क".repeat(10))
            ]
        );
    }
}
