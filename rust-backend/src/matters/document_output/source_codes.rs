//! Decode PDF character codes before outline lookup. A composite font's
//! encoding supplies glyph identities, while ToUnicode supplies reading text.
use hayro_cmap::{CMap, CMapName, WritingMode};
use hayro_postscript::{Scanner, Token};
use pdf_oxide::{object::Object, PdfDocument};
use std::ops::RangeInclusive;

type CodeSpaces = [Vec<RangeInclusive<u32>>; 4];

pub(in crate::matters) struct CidEncoding {
    cmap: CMap,
    spaces: CodeSpaces,
    base: Option<Box<CidEncoding>>,
    pub vertical: bool,
}

impl CidEncoding {
    pub fn load(pdf: &PdfDocument, binding: &Object, depth: usize) -> Option<Self> {
        if depth >= 12 {
            return None;
        }
        let binding = pdf.resolve_object(binding).ok()?;
        if let Some(name) = binding.as_name() {
            let (cmap, spaces) = match name {
                "Identity-H" => (CMap::identity_h(), identity_spaces()),
                "Identity-V" => (CMap::identity_v(), identity_spaces()),
                _ => {
                    let data = hayro_cmap::load_embedded(CMapName::from_bytes(name.as_bytes()))?;
                    let spaces = named_spaces(data, depth)?;
                    (CMap::parse(data, hayro_cmap::load_embedded)?, spaces)
                }
            };
            let vertical = cmap.metadata().writing_mode == Some(WritingMode::Vertical);
            return Some(Self {
                cmap,
                spaces,
                base: None,
                vertical,
            });
        }

        let mut data = binding.decode_stream_data().ok()?;
        if data.len() > 1024 * 1024 {
            return None;
        }
        let mut spaces = CodeSpaces::default();
        let mut scanner = Scanner::new(&data);
        let mut previous = None;
        let mut use_cmap = None;
        while !scanner.at_end() {
            let start = scanner.offset();
            let token = scanner.parse_token().ok()?;
            let prior = previous.replace((start, token.clone()));
            let Token::Name(name) = token else { continue };
            if name.is_literal() || name.is_immediate() {
                continue;
            }
            match name.as_bytes() {
                b"begincodespacerange" => loop {
                    let first = match scanner.parse_token().ok()? {
                        Token::Name(end)
                            if !end.is_literal() && end.as_bytes() == b"endcodespacerange" =>
                        {
                            break
                        }
                        Token::String(first) => first.decode().ok()?,
                        _ => return None,
                    };
                    let last = scanner.parse_string().ok()?.decode().ok()?;
                    insert_space(&mut spaces, &first, &last)?;
                },
                b"usecmap" => {
                    let (offset, Token::Name(base)) = prior? else {
                        return None;
                    };
                    if !base.is_literal() || use_cmap.is_some() {
                        return None;
                    }
                    use_cmap = Some((
                        offset..scanner.offset(),
                        Object::Name(base.as_str()?.to_owned()),
                    ));
                }
                _ => {}
            }
        }
        // Follow the stream dictionary first. Remove the textual directive
        // from the parsed program so mapping and code-space inheritance agree.
        let fields = binding.as_dict()?;
        let declared = if let Some((range, base)) = use_cmap {
            data[range].fill(b' ');
            Some(base)
        } else {
            None
        };
        let base = match fields.get("UseCMap").or(declared.as_ref()) {
            Some(binding) => Some(Box::new(Self::load(pdf, binding, depth + 1)?)),
            None => None,
        };
        if spaces.iter().all(Vec::is_empty) {
            spaces = base.as_ref()?.spaces.clone();
        }
        let cmap = CMap::parse(&data, hayro_cmap::load_embedded)?;
        let vertical = fields
            .get("WMode")
            .and_then(Object::as_integer)
            .map_or_else(
                || cmap.metadata().writing_mode == Some(WritingMode::Vertical),
                |mode| mode == 1,
            );
        Some(Self {
            cmap,
            spaces,
            base,
            vertical,
        })
    }

    fn code(&self, bytes: &[u8]) -> (u32, u8) {
        let mut raw = 0;
        for (index, &byte) in bytes.iter().take(4).enumerate() {
            raw = raw * 256 + u32::from(byte);
            if self.spaces[index].iter().any(|range| range.contains(&raw)) {
                return (raw, index as u8 + 1);
            }
        }
        (0, 1)
    }

    fn cid(&self, raw: u32, len: u8) -> Option<u32> {
        self.cmap
            .lookup_cid_code(raw, len)
            .or_else(|| self.base.as_ref()?.cid(raw, len))
    }
}

fn insert_space(spaces: &mut CodeSpaces, start: &[u8], end: &[u8]) -> Option<()> {
    let bucket = spaces.get_mut(start.len().checked_sub(1)?)?;
    if start.len() != end.len() || start > end || bucket.len() >= 256 {
        return None;
    }
    bucket.push(code_value(start)..=code_value(end));
    Some(())
}

fn identity_spaces() -> CodeSpaces {
    [Vec::new(), vec![0..=0xffff], Vec::new(), Vec::new()]
}

// The pinned hayro CMap bundle exposes its bytes but keeps the parsed
// code-space ranges private. Only the range and base segments are needed here.
fn named_spaces(data: &[u8], depth: usize) -> Option<CodeSpaces> {
    if depth >= 12 || !data.starts_with(b"bcmap\x01") {
        return None;
    }
    let size = usize::try_from(u32::from_be_bytes(data.get(6..10)?.try_into().ok()?)).ok()?;
    let mut segments = data.get(10..size)?;
    let mut spaces = CodeSpaces::default();
    let mut base = None;
    while !segments.is_empty() {
        let size = u32::from_be_bytes(segments.get(1..5)?.try_into().ok()?) as usize;
        let segment = segments.get(5..size)?;
        match segments[0] {
            0x09 => base = Some(segment),
            0x0c => {
                let (&count, mut ranges) = segment.split_first()?;
                for _ in 0..count {
                    let (&width, data) = ranges.split_first()?;
                    let width = usize::from(width);
                    insert_space(&mut spaces, data.get(..width)?, data.get(width..width * 2)?)?;
                    ranges = data.get(width * 2..)?;
                }
            }
            _ => {}
        }
        segments = segments.get(size..)?;
    }
    if spaces.iter().all(Vec::is_empty) {
        let data = hayro_cmap::load_embedded(CMapName::from_bytes(base?))?;
        return named_spaces(data, depth + 1);
    }
    Some(spaces)
}

fn code_value(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .fold(0, |value, &byte| value * 256 + u32::from(byte))
}

pub(in crate::matters) struct Code {
    pub raw: u32,
    pub bytes: u8,
    pub cid: Option<u16>,
    pub word: bool,
}

#[derive(Clone)]
pub(in crate::matters) struct Codes<'a> {
    bytes: &'a [u8],
    fixed_width: usize,
    encoding: Option<&'a CidEncoding>,
}

impl<'a> Codes<'a> {
    pub fn new(bytes: &'a [u8], fixed_width: usize, encoding: Option<&'a CidEncoding>) -> Self {
        Self {
            bytes,
            fixed_width,
            encoding,
        }
    }
}

impl Iterator for Codes<'_> {
    type Item = Option<Code>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.bytes.is_empty() {
            return None;
        }
        let (raw, len, cid) = if let Some(encoding) = self.encoding {
            let (raw, len) = encoding.code(self.bytes);
            (raw, usize::from(len), encoding.cid(raw, len))
        } else {
            let Some(bytes) = self.bytes.get(..self.fixed_width) else {
                self.bytes = &[];
                return Some(None);
            };
            let raw = code_value(bytes);
            (raw, self.fixed_width, Some(raw))
        };
        self.bytes = &self.bytes[len..];
        let cid = cid.map(u16::try_from).transpose().ok();
        Some(cid.map(|cid| Code {
            raw,
            bytes: len as u8,
            cid,
            word: raw == 32 && len == 1,
        }))
    }
}
