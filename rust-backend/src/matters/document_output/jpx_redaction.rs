use super::{HashMap, Object, PdfDocument, IMAGE_LIMIT};
use hayro_jpeg2000::{ColorSpace, DecodeSettings, DecoderContext, Image};
use std::ops::Range;

const INVALID: &str = "Cannot decode the JPEG 2000 image and its opacity for redaction";

pub(super) fn mask_mode(
    pdf: &PdfDocument,
    fields: &HashMap<String, Object>,
) -> Result<Option<i64>, &'static str> {
    let Some(filter) = fields.get("Filter") else {
        return Ok(None);
    };
    let filter = pdf.resolve_object(filter).map_err(|_| INVALID)?;
    let last = filter
        .as_array()
        .and_then(|filters| filters.last())
        .unwrap_or(&filter);
    let last = pdf.resolve_object(last).map_err(|_| INVALID)?;
    if last.as_name() != Some("JPXDecode") {
        return Ok(None);
    }
    let Some(mask) = fields.get("SMaskInData") else {
        return Ok(Some(0));
    };
    match pdf.resolve_object(mask).map_err(|_| INVALID)? {
        Object::Null => Ok(Some(0)),
        Object::Integer(mode @ 0..=2) => Ok(Some(mode)),
        _ => Err(INVALID),
    }
}

pub(super) fn samples(
    image: &Object,
    size: (u32, u32),
    space: Option<&Object>,
    indexed: bool,
    mode: i64,
) -> Result<(Object, Option<image::GrayImage>), &'static str> {
    let mut data = image.decode_stream_data().map_err(|_| INVALID)?;
    let premultiplied = channel_opacity(&mut data)?.unwrap_or(mode == 2);
    let header = Image::new(
        &data,
        &DecodeSettings {
            resolve_palette_indices: !indexed,
            ..Default::default()
        },
    )
    .map_err(|_| INVALID)?;
    let (width, height) = (header.width(), header.height());
    let count = u64::from(width) * u64::from(height);
    if (width, height) != size
        || count == 0
        || count > IMAGE_LIMIT
        || header.color_space().num_channels() > 5
    {
        return Err(INVALID);
    }

    let mut context = DecoderContext::default();
    let decoded = header.decode(&mut context).map_err(|_| INVALID)?;
    let planes = decoded.components();
    let components = planes.len();
    let has_alpha = header.has_alpha() || mode > 0;
    let colors = components
        .checked_sub(usize::from(has_alpha))
        .ok_or(INVALID)?;
    if !matches!(colors, 1 | 3 | 4)
        || (indexed && (colors != 1 || planes[0].bit_depth() > 8 || premultiplied))
    {
        return Err(INVALID);
    }
    let mut interleaved = if planes
        .iter()
        .all(|plane| plane.samples().len() == count as usize)
    {
        decoded.data_u8()
    } else {
        // Accept the same full-size and 2×2 reduced 8-bit planes as the
        // linked renderer. Other shapes do not identify a safe sample grid.
        let (w, h) = (width as usize, height as usize);
        let (sw, sh) = (w.div_ceil(2), h.div_ceil(2));
        if indexed
            || planes.iter().any(|plane| {
                plane.bit_depth() != 8 || ![w * h, sw * sh].contains(&plane.samples().len())
            })
        {
            return Err(INVALID);
        }
        let mut bytes = vec![0; count as usize * components];
        for (i, pixel) in bytes.chunks_exact_mut(components).enumerate() {
            for (output, plane) in pixel.iter_mut().zip(planes) {
                let samples = plane.samples();
                let index = if samples.len() == w * h {
                    i
                } else {
                    (i / w) * sh / h * sw + (i % w) * sw / w
                };
                *output = samples[index].round().clamp(0.0, 255.0) as u8;
            }
        }
        bytes
    };
    if indexed {
        for (pixel, index) in interleaved
            .chunks_exact_mut(components)
            .zip(planes[0].samples())
        {
            pixel[0] = index.round().clamp(0.0, 255.0) as u8;
        }
    }
    let color_space = match space {
        Some(space) => space.clone(),
        None => embedded_space(header.color_space(), colors),
    };
    let mask = if has_alpha {
        let mut mask = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            let at = i * components;
            let opacity = interleaved[at + colors];
            mask.push(opacity);
            for component in 0..colors {
                let sample = interleaved[at + component];
                interleaved[i * colors + component] = if premultiplied && opacity > 0 {
                    let opacity = u16::from(opacity);
                    ((u16::from(sample) * 255 + opacity / 2) / opacity).min(255) as u8
                } else if premultiplied {
                    0
                } else {
                    sample
                };
            }
        }
        interleaved.truncate(count as usize * colors);
        Some(image::GrayImage::from_raw(width, height, mask).ok_or(INVALID)?)
    } else {
        None
    };
    let dict = HashMap::from([
        ("Subtype".into(), Object::Name("Image".into())),
        ("ColorSpace".into(), color_space),
        ("BitsPerComponent".into(), Object::Integer(8)),
        ("Width".into(), Object::Integer(i64::from(width))),
        ("Height".into(), Object::Integer(i64::from(height))),
    ]);
    Ok((
        Object::Stream {
            dict,
            data: interleaved.into(),
        },
        mask,
    ))
}

fn find_box(
    data: &[u8],
    range: Range<usize>,
    kind: &[u8; 4],
) -> Result<Option<Range<usize>>, &'static str> {
    let mut offset = range.start;
    while offset < range.end {
        let header = data
            .get(offset..offset + 8)
            .filter(|_| offset + 8 <= range.end)
            .ok_or(INVALID)?;
        let length = u32::from_be_bytes(header[..4].try_into().unwrap());
        let (header_len, length) = match length {
            0 => (8, range.end - offset),
            1 => {
                let length = data.get(offset + 8..offset + 16).ok_or(INVALID)?;
                (
                    16,
                    usize::try_from(u64::from_be_bytes(length.try_into().unwrap()))
                        .map_err(|_| INVALID)?,
                )
            }
            length => (8, length as usize),
        };
        let end = offset
            .checked_add(length)
            .filter(|&end| end <= range.end && length >= header_len)
            .ok_or(INVALID)?;
        if &header[4..] == kind {
            return Ok(Some(offset + header_len..end));
        }
        offset = end;
    }
    Ok(None)
}

fn channel_opacity(data: &mut [u8]) -> Result<Option<bool>, &'static str> {
    if !data.starts_with(b"\0\0\0\x0cjP  \r\n\x87\n") {
        return Ok(None);
    }
    let Some(header) = find_box(data, 0..data.len(), b"jp2h")? else {
        return Ok(None);
    };
    let Some(definition) = find_box(data, header, b"cdef")? else {
        return Ok(None);
    };
    let payload = &mut data[definition];
    let count = u16::from_be_bytes(payload.get(..2).ok_or(INVALID)?.try_into().unwrap()) as usize;
    if count == 0 || payload.len() != 2 + 6 * count {
        return Err(INVALID);
    }
    let mut associated = None;
    for entry in payload[2..].chunks_exact_mut(6) {
        let kind = u16::from_be_bytes([entry[2], entry[3]]);
        if matches!(kind, 1 | 2) {
            if associated.replace(kind == 2).is_some() || entry[4..] != [0, 0] {
                return Err(INVALID);
            }
            // The decoder accepts type 1. Keep the original type so associated
            // samples are unpremultiplied exactly once before color conversion.
            entry[2..4].copy_from_slice(&[0, 1]);
        }
    }
    Ok(associated)
}

fn embedded_space(space: &ColorSpace, colors: usize) -> Object {
    if let ColorSpace::Icc {
        profile,
        num_channels,
    } = space
    {
        if usize::from(*num_channels) == colors {
            return Object::Array(vec![
                Object::Name("ICCBased".into()),
                Object::Stream {
                    dict: HashMap::from([("N".into(), Object::Integer(colors as i64))]),
                    data: profile.clone().into(),
                },
            ]);
        }
    }
    Object::Name(
        match colors {
            1 => "DeviceGray",
            4 => "DeviceCMYK",
            _ => "DeviceRGB",
        }
        .into(),
    )
}
