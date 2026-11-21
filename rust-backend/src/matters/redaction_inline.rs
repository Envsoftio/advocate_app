//! Promote inline images to independently editable image resources before redaction.
use super::page_tools::assembly::{Overlay, XObjectEdit, XObjectReplacement};
use hayro_ccitt::{DecodeSettings, Decoder, DecoderContext, EncodingMode};
use pdf_oxide::{
    extractors::expand_inline_image_dict, object::Object, parser::parse_object, PdfDocument,
};
use std::{
    collections::HashMap,
    io::{self, Read, Write},
};

const INVALID: &str = "Cannot identify the inline PDF image safely";
const MAX_DECODED_BYTES: u64 = 320 * 1024 * 1024;

fn whitespace(byte: u8) -> bool {
    matches!(byte, 0 | b'\t' | b'\n' | 12 | b'\r' | b' ')
}

fn skip_trivia(mut input: &[u8]) -> &[u8] {
    loop {
        input = input.trim_ascii_start();
        if input.first().is_some_and(|&byte| whitespace(byte)) {
            input = &input[1..];
        } else if input.starts_with(b"%") {
            input = input
                .iter()
                .position(|byte| matches!(byte, b'\r' | b'\n'))
                .map_or(&[], |index| &input[index..]);
        } else {
            return input;
        }
    }
}

fn unfiltered_length(fields: &HashMap<String, Object>) -> Option<usize> {
    let positive = |key: &str| {
        usize::try_from(fields.get(key)?.as_integer()?)
            .ok()
            .filter(|&value| value > 0)
    };
    let mask = fields.get("ImageMask") == Some(&Object::Boolean(true));
    let bits = if mask {
        1
    } else {
        positive("BitsPerComponent")?
    };
    if !matches!(bits, 1 | 2 | 4 | 8 | 16) {
        return None;
    }
    let components = if mask {
        1
    } else {
        let space = fields.get("ColorSpace")?;
        match space.as_name() {
            Some("DeviceGray") => 1,
            Some("DeviceRGB") => 3,
            Some("DeviceCMYK") => 4,
            _ if space.as_array()?.first()?.as_name() == Some("Indexed") => 1,
            _ => return None,
        }
    };
    let length = positive("Width")?
        .checked_mul(components)?
        .checked_mul(bits)?
        .div_ceil(8)
        .checked_mul(positive("Height")?)?;
    (length as u64 <= MAX_DECODED_BYTES).then_some(length)
}

fn run_length_end(data: &[u8]) -> Option<usize> {
    let mut index = 0usize;
    while let Some(&byte) = data.get(index) {
        index += 1;
        match byte {
            128 => return Some(index),
            0..=127 => index += usize::from(byte) + 1,
            129..=255 => index += 1,
        }
    }
    None
}

fn jpeg_end(data: &[u8]) -> Option<usize> {
    if !data.starts_with(b"\xff\xd8") {
        return None;
    }
    let mut remaining = &data[2..];
    let mut scan = false;
    loop {
        if scan {
            let marker = remaining.iter().position(|&byte| byte == 0xff)?;
            remaining = &remaining[marker..];
        }
        remaining = remaining.strip_prefix(b"\xff")?;
        while remaining.first() == Some(&0xff) {
            remaining = &remaining[1..];
        }
        let (&marker, tail) = remaining.split_first()?;
        remaining = tail;
        match marker {
            0xd9 => return Some(data.len() - remaining.len()),
            0x00 | 0xd0..=0xd7 if scan => continue,
            0x01 => continue,
            0xd8 | 0x00 | 0xd0..=0xd7 => return None,
            _ => {
                let length = usize::from(u16::from_be_bytes(remaining.get(..2)?.try_into().ok()?));
                if length < 2 {
                    return None;
                }
                remaining = remaining.get(length..)?;
                scan = marker == 0xda;
            }
        }
    }
}

fn lzw_end(data: &[u8], early_change: usize) -> Option<usize> {
    let mut bit = 0usize;
    let mut width = 9usize;
    let mut next_code = 258usize;
    let mut previous = false;
    loop {
        let mut code = 0usize;
        for _ in 0..width {
            code = (code << 1) | usize::from((data.get(bit / 8)? >> (7 - bit % 8)) & 1);
            bit += 1;
        }
        match code {
            256 => {
                width = 9;
                next_code = 258;
                previous = false;
            }
            257 => return Some(bit.div_ceil(8)),
            code if code < next_code || (code == next_code && previous && next_code < 4096) => {
                if !previous && code > 255 {
                    return None;
                }
                if previous && next_code < 4096 {
                    next_code += 1;
                    if width < 12 && next_code + early_change == 1 << width {
                        width += 1;
                    }
                }
                previous = true;
            }
            _ => return None,
        }
    }
}

struct FaxSamples {
    rows: usize,
    column: usize,
    width: usize,
    stride: usize,
    data: Vec<u8>,
}

impl Decoder for FaxSamples {
    fn push_pixels(&mut self, white: bool, count: u32) {
        let end = (self.column + count as usize).min(self.width);
        if white {
            for column in self.column..end {
                self.data[self.rows * self.stride + column / 8] |= 0x80 >> (column % 8);
            }
        }
        self.column = end;
    }

    fn next_line(&mut self) {
        self.rows += 1;
        self.column = 0;
    }
}

fn decode_fax(data: &[u8], fields: &HashMap<String, Object>) -> Option<(usize, Vec<u8>)> {
    let width = u32::try_from(fields.get("Width")?.as_integer()?).ok()?;
    let height = u32::try_from(fields.get("Height")?.as_integer()?).ok()?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 40_000_000 {
        return None;
    }
    let mask = fields.get("ImageMask") == Some(&Object::Boolean(true));
    if !mask
        && (fields.get("ColorSpace")?.as_name() != Some("DeviceGray")
            || fields.get("BitsPerComponent")?.as_integer() != Some(1))
    {
        return None;
    }
    let parameters = fields.get("DecodeParms").and_then(|value| {
        value
            .as_dict()
            .or_else(|| value.as_array()?.first()?.as_dict())
    });
    let integer = |name, default| match parameters.and_then(|dict| dict.get(name)) {
        Some(value) => value.as_integer(),
        None => Some(default),
    };
    let boolean = |name, default| match parameters.and_then(|dict| dict.get(name)) {
        Some(Object::Boolean(value)) => Some(*value),
        Some(_) => None,
        None => Some(default),
    };
    if integer("Columns", 1728)? != i64::from(width)
        || !matches!(integer("Rows", 0)?, rows if rows == 0 || rows == i64::from(height))
        || integer("DamagedRowsBeforeError", 0)? != 0
    {
        return None;
    }
    let encoding = match integer("K", 0)? {
        k if k < 0 => EncodingMode::Group4,
        0 => EncodingMode::Group3_1D,
        k => EncodingMode::Group3_2D {
            k: u32::try_from(k).ok()?,
        },
    };
    let mut context = DecoderContext::new(DecodeSettings {
        columns: width,
        rows: height,
        end_of_block: boolean("EndOfBlock", true)?,
        end_of_line: boolean("EndOfLine", false)?,
        rows_are_byte_aligned: boolean("EncodedByteAlign", false)?,
        encoding,
        invert_black: boolean("BlackIs1", false)?,
    });
    let stride = width.div_ceil(8) as usize;
    let mut image = FaxSamples {
        width: width as usize,
        rows: 0,
        column: 0,
        stride,
        data: vec![0; stride * height as usize],
    };
    let length = hayro_ccitt::decode(data, &mut image, &mut context).ok()?;
    (image.rows == height as usize).then_some((length, image.data))
}

fn image_length(data: &[u8], fields: &HashMap<String, Object>) -> Option<usize> {
    let Some(filters) = fields.get("Filter") else {
        return unfiltered_length(fields);
    };
    let outer = filters
        .as_name()
        .or_else(|| filters.as_array()?.first()?.as_name())?;
    match outer {
        "ASCIIHexDecode" => data
            .iter()
            .position(|&byte| !byte.is_ascii_hexdigit() && !whitespace(byte))
            .filter(|&index| data[index] == b'>')
            .map(|index| index + 1),
        "ASCII85Decode" => data
            .windows(2)
            .position(|pair| pair == b"~>")
            .map(|i| i + 2),
        "RunLengthDecode" => run_length_end(data),
        "FlateDecode" => {
            let mut decoder = flate2::bufread::ZlibDecoder::new(data);
            let decoded = io::copy(
                &mut decoder.by_ref().take(MAX_DECODED_BYTES + 1),
                &mut io::sink(),
            )
            .ok()?;
            (decoded <= MAX_DECODED_BYTES).then(|| decoder.total_in() as usize)
        }
        "DCTDecode" => jpeg_end(data),
        "LZWDecode" => {
            let params = fields.get("DecodeParms").and_then(|value| {
                value
                    .as_dict()
                    .or_else(|| value.as_array()?.first()?.as_dict())
            });
            let early = match params.and_then(|dict| dict.get("EarlyChange")) {
                None | Some(Object::Integer(1)) => 1,
                Some(Object::Integer(0)) => 0,
                _ => return None,
            };
            lzw_end(data, early)
        }
        _ => None,
    }
}

fn inline_value(value: &Object) -> bool {
    match value {
        Object::Array(items) => items.iter().all(inline_value),
        Object::Dictionary(items) => items.values().all(inline_value),
        Object::Stream { .. } | Object::Reference(_) => false,
        _ => true,
    }
}

pub(super) fn read(data: &[u8], start: usize) -> Result<(usize, Object), &'static str> {
    let mut input = &data[start..];
    let mut fields = HashMap::new();
    let samples = loop {
        input = skip_trivia(input);
        if let Some(payload) = input
            .strip_prefix(b"ID")
            .filter(|tail| tail.first().is_some_and(|&byte| whitespace(byte)))
        {
            break payload.strip_prefix(b"\r\n").unwrap_or(&payload[1..]);
        }
        let (remaining, key) = parse_object(input).map_err(|_| INVALID)?;
        let (remaining, value) = parse_object(remaining).map_err(|_| INVALID)?;
        let key = key.as_name().ok_or(INVALID)?;
        if !matches!(
            key,
            "W" | "Width"
                | "H"
                | "Height"
                | "BPC"
                | "BitsPerComponent"
                | "CS"
                | "ColorSpace"
                | "D"
                | "Decode"
                | "DP"
                | "DecodeParms"
                | "F"
                | "Filter"
                | "I"
                | "Interpolate"
                | "IM"
                | "ImageMask"
                | "Intent"
        ) {
            return Err(INVALID);
        }
        if !inline_value(&value) || fields.insert(key.to_owned(), value).is_some() {
            return Err(INVALID);
        }
        input = remaining;
    };
    let mut fields = expand_inline_image_dict(fields);
    let fax = fields.get("Filter").is_some_and(|value| {
        value.as_name() == Some("CCITTFaxDecode")
            || matches!(value.as_array().map(Vec::as_slice), Some([filter])
                if filter.as_name() == Some("CCITTFaxDecode"))
    });
    let (length, payload) = if fax {
        let (length, decoded) = decode_fax(samples, &fields).ok_or(INVALID)?;
        fields.remove("Filter");
        fields.remove("DecodeParms");
        (length, decoded)
    } else {
        let length = image_length(samples, &fields)
            .filter(|&size| size > 0)
            .ok_or(INVALID)?;
        (length, samples.get(..length).ok_or(INVALID)?.to_vec())
    };
    let mut end = samples.get(length..).ok_or(INVALID)?;
    if !end.first().is_some_and(|&byte| whitespace(byte)) {
        return Err(INVALID);
    }
    while end.first().is_some_and(|&byte| whitespace(byte)) {
        end = &end[1..];
    }
    let end = end
        .strip_prefix(b"EI")
        .filter(|tail| {
            tail.first()
                .is_none_or(|&byte| whitespace(byte) || b"()<>[]{}/%".contains(&byte))
        })
        .ok_or(INVALID)?;
    fields.insert("Type".into(), Object::Name("XObject".into()));
    Ok((
        data.len() - end.len(),
        Object::Stream {
            dict: fields,
            data: payload.into(),
        },
    ))
}

pub(super) fn promote(pdf: &PdfDocument, page: usize) -> Result<Option<Overlay>, &'static str> {
    let content = pdf.get_page_content_data(page).map_err(|_| INVALID)?;
    let images = super::redaction_vectors::inline_images(&content)?;
    if images.is_empty() {
        return Ok(None);
    }
    let resources = pdf.get_page_resources(page).map_err(|_| INVALID)?;
    let xobjects = resources
        .as_dict()
        .and_then(|dict| dict.get("XObject"))
        .map(|resource| pdf.resolve_object(resource))
        .transpose()
        .map_err(|_| INVALID)?;
    let prefix = (0..)
        .map(|index| format!("MatterDraftInline{index}_"))
        .find(|prefix| {
            xobjects
                .as_ref()
                .and_then(Object::as_dict)
                .is_none_or(|dict| dict.keys().all(|name| !name.starts_with(prefix)))
        })
        .ok_or(INVALID)?;
    let mut copied = Vec::with_capacity(content.len());
    let mut edits = Vec::new();
    let mut previous = 0;
    for (index, (range, image)) in images.into_iter().enumerate() {
        copied.extend_from_slice(&content[previous..range.start]);
        let target = format!("{prefix}{index}");
        write!(copied, "/{target} Do ").map_err(|_| INVALID)?;
        previous = range.end;
        edits.push(XObjectEdit {
            source: None,
            target,
            replacement: XObjectReplacement::Image(image, None),
        });
    }
    copied.extend_from_slice(&content[previous..]);
    Ok(Some(Overlay {
        replace_page_content: Some(copied),
        xobject_edits: edits,
        redact_content: true,
        ..Default::default()
    }))
}
