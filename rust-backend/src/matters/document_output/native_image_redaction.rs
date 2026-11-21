use super::{
    assembly::XObjectReplacement, compress, decode_image, explicit_stencil_mask, image_stream,
    redact_samples, HashMap, ImageFormat, ImageSoftMask, Object, PdfDocument,
};
use pdf_oxide::{content::Matrix, object::ObjectRef};

const INVALID: &str = "Cannot read the image's original color samples for redaction";

pub(super) fn redact(
    pdf: &PdfDocument,
    original: Option<ObjectRef>,
    image: &Object,
    fields: &HashMap<String, Object>,
    color_space: Object,
    channels: usize,
    (width, height): (u32, u32),
    soft_mask: Option<ImageSoftMask>,
    placement: Matrix,
    selected: &[&[f32; 4]],
) -> Result<XObjectReplacement, &'static str> {
    let bits = bits_per_component(pdf, fields)?;
    let mut samples = packed_image(
        pdf,
        image,
        original,
        fields,
        (width, height),
        channels,
        bits,
    )?;
    let (mut opacity, matte) = match soft_mask {
        Some(mask) => mask,
        None => (
            match fields
                .get("Mask")
                .map(|mask| pdf.resolve_object(mask))
                .transpose()
                .map_err(|_| INVALID)?
            {
                None | Some(Object::Null) => {
                    image::GrayImage::from_pixel(width, height, image::Luma([255]))
                }
                Some(Object::Array(range)) => samples.color_key(pdf, &range)?,
                Some(stencil) => explicit_stencil_mask(&stencil, width, height)?,
            },
            None,
        ),
    };
    // Transparency works for any ink set. Zero all original selected samples
    // as well so the masked image cannot disclose removed pixels.
    redact_samples(width, height, placement, selected, |x, y| {
        samples.clear(x, y);
        opacity.put_pixel(x, y, image::Luma([0]));
    })?;

    let mut dict = HashMap::from([
        ("Type".into(), Object::Name("XObject".into())),
        ("Subtype".into(), Object::Name("Image".into())),
        ("Width".into(), Object::Integer(i64::from(width))),
        ("Height".into(), Object::Integer(i64::from(height))),
        ("ColorSpace".into(), color_space),
        ("BitsPerComponent".into(), Object::Integer(bits as i64)),
        ("Filter".into(), Object::Name("FlateDecode".into())),
    ]);
    for name in ["Decode", "Intent", "Interpolate"] {
        if let Some(value) = fields.get(name) {
            if let Some(value) = retained_property(pdf, name, value)? {
                dict.insert(name.into(), value);
            }
        }
    }
    let mut mask = image_stream(width, height, "DeviceGray", opacity.as_raw())?;
    if let (Some(matte), Object::Stream { dict, .. }) = (matte, &mut mask) {
        let value = retained_property(pdf, "Matte", &matte)?.ok_or(INVALID)?;
        if !value
            .as_array()
            .is_some_and(|matte| matte.len() == channels)
        {
            return Err(INVALID);
        }
        dict.insert("Matte".into(), value);
    }
    Ok(XObjectReplacement::Image(
        Object::Stream {
            dict,
            data: compress(&samples.bytes)?.into(),
        },
        Some(mask),
    ))
}

fn retained_property(
    pdf: &PdfDocument,
    name: &str,
    value: &Object,
) -> Result<Option<Object>, &'static str> {
    let value = pdf.resolve_object(value).map_err(|_| INVALID)?;
    match (name, value) {
        (_, Object::Null) => Ok(None),
        ("Decode" | "Matte", Object::Array(values)) => Ok(Some(Object::Array(
            values
                .iter()
                .map(
                    |value| match pdf.resolve_object(value).map_err(|_| INVALID)? {
                        number @ Object::Integer(_) => Ok(number),
                        number @ Object::Real(real) if real.is_finite() => Ok(number),
                        _ => Err(INVALID),
                    },
                )
                .collect::<Result<_, _>>()?,
        ))),
        ("Intent", value @ Object::Name(_)) | ("Interpolate", value @ Object::Boolean(_)) => {
            Ok(Some(value))
        }
        _ => Err(INVALID),
    }
}

pub(super) fn bits_per_component(
    pdf: &PdfDocument,
    fields: &HashMap<String, Object>,
) -> Result<usize, &'static str> {
    let bits = pdf
        .resolve_object(fields.get("BitsPerComponent").ok_or(INVALID)?)
        .map_err(|_| INVALID)?;
    match bits.as_integer() {
        Some(bits @ (1 | 2 | 4 | 8 | 16)) => Ok(bits as usize),
        _ => Err(INVALID),
    }
}

pub(super) fn packed_image(
    pdf: &PdfDocument,
    image: &Object,
    original: Option<ObjectRef>,
    fields: &HashMap<String, Object>,
    (width, height): (u32, u32),
    channels: usize,
    bits: usize,
) -> Result<PackedImage, &'static str> {
    let bytes = match last_filter(pdf, fields)?.as_deref() {
        Some("DCTDecode" | "DCT") => {
            jpeg_samples(pdf, image, fields, (width, height), channels, bits)?
        }
        Some("JBIG2Decode") if channels == 1 && bits == 1 => {
            let Object::Stream { mut dict, data } = image.clone() else {
                return Err(INVALID);
            };
            dict.insert("ColorSpace".into(), Object::Name("DeviceGray".into()));
            dict.remove("Decode");
            let decoded = pdf_oxide::extractors::extract_image_from_xobject(
                Some(pdf),
                &Object::Stream { dict, data },
                original,
                None,
            )
            .and_then(|image| image.to_png_bytes())
            .map_err(|_| INVALID)?;
            let gray = decode_image(&decoded, ImageFormat::Png)?.into_luma8();
            if gray.dimensions() != (width, height) {
                return Err(INVALID);
            }
            let stride = width.div_ceil(8) as usize;
            let mut data = vec![0; stride * height as usize];
            for (x, y, pixel) in gray.enumerate_pixels() {
                if pixel[0] >= 128 {
                    data[y as usize * stride + (x / 8) as usize] |= 0x80 >> (x % 8);
                }
            }
            data
        }
        Some("JBIG2Decode" | "JPXDecode") => return Err(INVALID),
        _ => image.decode_stream_data().map_err(|_| INVALID)?,
    };
    PackedImage::new(bytes, width, height, channels, bits)
}

pub(super) fn last_filter(
    pdf: &PdfDocument,
    fields: &HashMap<String, Object>,
) -> Result<Option<String>, &'static str> {
    let filter = fields
        .get("Filter")
        .map(|value| pdf.resolve_object(value))
        .transpose()
        .map_err(|_| INVALID)?;
    let filter = filter.as_ref().map(|filter| {
        filter
            .as_array()
            .and_then(|filters| filters.last())
            .unwrap_or(filter)
    });
    Ok(filter
        .map(|value| pdf.resolve_object(value))
        .transpose()
        .map_err(|_| INVALID)?
        .as_ref()
        .and_then(Object::as_name)
        .map(str::to_owned))
}

fn jpeg_samples(
    pdf: &PdfDocument,
    image: &Object,
    fields: &HashMap<String, Object>,
    (width, height): (u32, u32),
    channels: usize,
    bits: usize,
) -> Result<Vec<u8>, &'static str> {
    use jpeg_decoder::{ColorTransform, Decoder};

    let bytes = image.decode_stream_data().map_err(|_| INVALID)?;
    let adobe = super::super::image_import::jpeg_segments(&bytes).find_map(|(marker, data)| {
        if marker == 0xee && data.starts_with(b"Adobe") {
            data.get(11).copied()
        } else {
            None
        }
    });
    let parameters = fields
        .get("DecodeParms")
        .map(|value| pdf.resolve_object(value))
        .transpose()
        .map_err(|_| INVALID)?;
    let parameters = parameters.as_ref().map(|value| {
        value
            .as_array()
            .and_then(|array| array.last())
            .unwrap_or(value)
    });
    let color_transform = parameters
        .and_then(|value| pdf.resolve_object(value).ok())
        .and_then(|value| {
            pdf.resolve_object(value.as_dict()?.get("ColorTransform")?)
                .ok()
        })
        .and_then(|value| value.as_integer());
    let transform = match (channels, adobe.map(i64::from).or(color_transform)) {
        (1, _) => ColorTransform::Grayscale,
        (3, Some(0)) => ColorTransform::RGB,
        (4, None | Some(0)) => ColorTransform::CMYK,
        (3, None | Some(1)) => ColorTransform::YCbCr,
        (4, Some(1 | 2)) => ColorTransform::YCCK,
        _ => return Err(INVALID),
    };
    let mut decoder = Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_max_decoding_buffer_size(width as usize * height as usize * channels * (bits / 8));
    decoder.set_color_transform(transform);
    decoder.read_info().map_err(|_| INVALID)?;
    let info = decoder.info().ok_or(INVALID)?;
    if (u32::from(info.width), u32::from(info.height)) != (width, height)
        || info.pixel_format.pixel_bytes() * 8 != channels * bits
    {
        return Err(INVALID);
    }
    let mut samples = decoder.decode().map_err(|_| INVALID)?;
    if matches!(transform, ColorTransform::CMYK | ColorTransform::YCCK) {
        // The JPEG decoder emits inverted ink in this mode. The PDF Decode
        // array and color-key bounds address the original PDF-side samples.
        for sample in &mut samples {
            *sample = 255 - *sample;
        }
    } else if bits == 16 && cfg!(target_endian = "little") {
        for sample in samples.chunks_exact_mut(2) {
            sample.swap(0, 1);
        }
    }
    Ok(samples)
}

pub(super) struct PackedImage {
    bytes: Vec<u8>,
    width: u32,
    height: u32,
    channels: usize,
    bits: usize,
    stride: usize,
}

impl PackedImage {
    fn new(
        bytes: Vec<u8>,
        width: u32,
        height: u32,
        channels: usize,
        bits: usize,
    ) -> Result<Self, &'static str> {
        let stride = (width as usize)
            .checked_mul(channels * bits)
            .ok_or(INVALID)?
            .div_ceil(8);
        if stride == 0 || stride.checked_mul(height as usize) != Some(bytes.len()) {
            return Err(INVALID);
        }
        Ok(Self {
            bytes,
            width,
            height,
            channels,
            bits,
            stride,
        })
    }

    fn sample_bit(&self, x: u32, y: u32) -> (usize, usize) {
        let bit = x as usize * self.channels * self.bits;
        (y as usize * self.stride + bit / 8, bit % 8)
    }

    fn clear(&mut self, x: u32, y: u32) {
        let (start, offset) = self.sample_bit(x, y);
        if self.bits >= 8 {
            self.bytes[start..start + self.channels * self.bits / 8].fill(0);
        } else {
            let mask = (1 << self.bits) - 1;
            for channel in 0..self.channels {
                let bit = offset + channel * self.bits;
                self.bytes[start + bit / 8] &= !(mask << (8 - bit % 8 - self.bits));
            }
        }
    }

    pub fn color_key(
        &self,
        pdf: &PdfDocument,
        ranges: &[Object],
    ) -> Result<image::GrayImage, &'static str> {
        if ranges.len() != self.channels * 2 {
            return Err(INVALID);
        }
        let max = ((1_u32 << self.bits) - 1) as u16;
        let endpoint = |value: &Object| {
            u16::try_from(pdf.resolve_object(value).ok()?.as_integer()?)
                .ok()
                .filter(|&value| value <= max)
        };
        let ranges = ranges
            .chunks_exact(2)
            .map(|pair| {
                let (start, end) = (endpoint(&pair[0])?, endpoint(&pair[1])?);
                (start <= end).then_some(start..=end)
            })
            .collect::<Option<Vec<_>>>()
            .ok_or(INVALID)?;
        Ok(image::GrayImage::from_fn(
            self.width,
            self.height,
            |x, y| {
                let (start, offset) = self.sample_bit(x, y);
                let opaque = !ranges.iter().enumerate().all(|(channel, range)| {
                    let bit = offset + channel * self.bits;
                    let byte = start + bit / 8;
                    let sample = if self.bits == 16 {
                        u16::from_be_bytes([self.bytes[byte], self.bytes[byte + 1]])
                    } else {
                        u16::from(self.bytes[byte] >> (8 - bit % 8 - self.bits)) & max
                    };
                    range.contains(&sample)
                });
                image::Luma([if opaque { 255 } else { 0 }])
            },
        ))
    }
}
