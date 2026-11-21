//! Page geometry for image originals, as used by image import and original printing.
use super::page_tools::assembly;
use image::{metadata::Orientation, DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use pdf_oxide::{
    object::Object,
    writer::{ColorSpace as WriterColorSpace, ImageData},
};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::OnceLock,
};

const INVALID: &str = "Cannot normalize imported image";
const INVALID_CMYK_PROFILE: &str =
    "Configured default CMYK profile does not match the expected SWOP profile";
const REFERENCE_CMYK_BYTES: usize = 187_484;
const REFERENCE_CMYK_SHA256: &str =
    "8472fa1493a024b800b67dee9424835ec0c41ab79490200ae8ec4a689fd1b9a9";
static PROFILE_ROOT: OnceLock<PathBuf> = OnceLock::new();
static DEFAULT_CMYK: OnceLock<Result<Option<Object>, &'static str>> = OnceLock::new();

pub(super) fn set_resource_root(root: PathBuf) {
    if root.is_absolute() {
        let _ = PROFILE_ROOT.set(root.join("matterdraft-pdf/iccprofiles"));
    }
}

fn reference_cmyk_profile(path: &Path) -> Result<Option<Object>, &'static str> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(INVALID_CMYK_PROFILE),
    };
    if !metadata.is_file() || metadata.len() != REFERENCE_CMYK_BYTES as u64 {
        return Err(INVALID_CMYK_PROFILE);
    }
    let mut bytes = Vec::with_capacity(REFERENCE_CMYK_BYTES);
    File::open(path)
        .and_then(|file| {
            file.take((REFERENCE_CMYK_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
        })
        .map_err(|_| INVALID_CMYK_PROFILE)?;
    if bytes.len() != REFERENCE_CMYK_BYTES || super::sha256_hex(&bytes) != REFERENCE_CMYK_SHA256 {
        return Err(INVALID_CMYK_PROFILE);
    }
    embedded_profile(WriterColorSpace::DeviceCMYK, bytes)
        .map(Some)
        .ok_or(INVALID_CMYK_PROFILE)
}

fn default_cmyk_profile() -> Result<Option<Object>, &'static str> {
    DEFAULT_CMYK
        .get_or_init(|| {
            if let Some(root) = PROFILE_ROOT.get() {
                if let Some(profile) = reference_cmyk_profile(&root.join("default_cmyk.icc"))? {
                    return Ok(Some(profile));
                }
            }
            if let Some(path) = std::env::var_os("MATTERDRAFT_CMYK_PROFILE") {
                return reference_cmyk_profile(Path::new(&path))?
                    .map(Some)
                    .ok_or(INVALID_CMYK_PROFILE);
            }
            Ok(None)
        })
        .clone()
}

pub(super) fn to_pdf(
    bytes: &[u8],
    format: ImageFormat,
    decoded: DynamicImage,
) -> Result<Vec<u8>, &'static str> {
    let mut decoder = ImageReader::with_format(Cursor::new(bytes), format)
        .into_decoder()
        .map_err(|_| INVALID)?;
    let (resolution, orientation) = if format == ImageFormat::Jpeg {
        jpeg_page_metadata(bytes)
    } else {
        (
            png_dpi(bytes).map(|dpi| (dpi, dpi)),
            Orientation::NoTransforms,
        )
    };
    let (horizontal, vertical) = corrected_dpi(resolution.unwrap_or((96, 96)));
    let icc = decoder.icc_profile().map_err(|_| INVALID)?;
    let (image, [width, height]) = if format == ImageFormat::Png {
        png_resource(decoded, icc)?
    } else {
        drop(decoded);
        jpeg_resource(bytes, icc)?
    };
    let width = width as f32 * 72.0 / horizontal as f32;
    let height = height as f32 * 72.0 / vertical as f32;
    let (page_width, page_height, matrix) = match orientation {
        Orientation::NoTransforms => (width, height, None),
        Orientation::FlipHorizontal => (width, height, Some([-1., 0., 0., 1., width, 0.])),
        Orientation::Rotate180 => (width, height, Some([-1., 0., 0., -1., width, height])),
        Orientation::FlipVertical => (width, height, Some([1., 0., 0., -1., 0., height])),
        Orientation::Rotate90 => (height, width, Some([0., -1., 1., 0., 0., width])),
        Orientation::Rotate270 => (height, width, Some([0., 1., -1., 0., height, 0.])),
        Orientation::Rotate90FlipH => (height, width, Some([0., -1., -1., 0., height, width])),
        Orientation::Rotate270FlipH => (height, width, Some([0., 1., 1., 0., 0., 0.])),
    };
    let [a, b, c, d, e, f] = matrix.unwrap_or([1., 0., 0., 1., 0., 0.]);
    let overlay = assembly::Overlay {
        commands: format!("q {a:.8} {b:.8} {c:.8} {d:.8} {e:.8} {f:.8} cm {width:.8} 0 0 {height:.8} 0 0 cm /@I0 Do Q\n"),
        images: vec![image],
        ..Default::default()
    };
    let source = assembly::blank((page_width.into(), page_height.into()))?;
    assembly::overlay_document(source, &HashMap::from([(0, overlay)]))
}

fn embedded_profile(space: WriterColorSpace, bytes: Vec<u8>) -> Option<Object> {
    let color = match space {
        WriterColorSpace::DeviceGray => b"GRAY",
        WriterColorSpace::DeviceRGB => b"RGB ",
        WriterColorSpace::DeviceCMYK => b"CMYK",
    };
    if !(128..=4 * 1024 * 1024).contains(&bytes.len())
        || bytes.get(36..40)? != b"acsp"
        || bytes.get(16..20)? != color
        || u32::from_be_bytes(bytes[..4].try_into().ok()?) as usize != bytes.len()
    {
        return None;
    }
    Some(Object::Stream {
        dict: HashMap::from([
            ("N".into(), Object::Integer(space.components() as i64)),
            ("Alternate".into(), Object::Name(space.pdf_name().into())),
        ]),
        data: bytes.into(),
    })
}

pub(super) fn jpeg_overlay(
    bytes: &[u8],
) -> Result<(assembly::OverlayImage, [u32; 2]), &'static str> {
    let mut decoder = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Jpeg)
        .into_decoder()
        .map_err(|_| INVALID)?;
    jpeg_resource(bytes, decoder.icc_profile().map_err(|_| INVALID)?)
}

fn jpeg_resource(
    bytes: &[u8],
    profile: Option<Vec<u8>>,
) -> Result<(assembly::OverlayImage, [u32; 2]), &'static str> {
    let image = ImageData::from_jpeg(bytes.to_vec()).map_err(|_| INVALID)?;
    let dimensions = [image.width, image.height];
    let cmyk = image.color_space == WriterColorSpace::DeviceCMYK;
    let mut profile = profile.and_then(|bytes| embedded_profile(image.color_space, bytes));
    if cmyk && profile.is_none() {
        profile = default_cmyk_profile()?;
    }
    // Four-component JPEG image streams use inverted sample ranges in the
    // reference PDF writer, regardless of the presence of an APP14 marker.
    Ok((overlay_resource(image, profile, cmyk), dimensions))
}

pub(super) fn png_overlay(
    bytes: &[u8],
    image: DynamicImage,
) -> Result<(assembly::OverlayImage, [u32; 2]), &'static str> {
    let mut decoder = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png)
        .into_decoder()
        .map_err(|_| INVALID)?;
    let profile = decoder.icc_profile().map_err(|_| INVALID)?;
    png_resource(image, profile)
}

fn png_resource(
    image: DynamicImage,
    profile: Option<Vec<u8>>,
) -> Result<(assembly::OverlayImage, [u32; 2]), &'static str> {
    use WriterColorSpace::{DeviceGray as Gray, DeviceRGB as Rgb};
    let (width, height) = (image.width(), image.height());
    let high_bytes = |samples: Vec<u16>| {
        samples
            .into_iter()
            .map(|sample| sample.to_be_bytes()[0])
            .collect::<Vec<_>>()
    };
    let (pixels, space, has_alpha) = match image {
        DynamicImage::ImageLuma8(image) => (image.into_raw(), Gray, false),
        DynamicImage::ImageLumaA8(image) => (image.into_raw(), Gray, true),
        DynamicImage::ImageRgb8(image) => (image.into_raw(), Rgb, false),
        DynamicImage::ImageRgba8(image) => (image.into_raw(), Rgb, true),
        DynamicImage::ImageLuma16(image) => (high_bytes(image.into_raw()), Gray, false),
        DynamicImage::ImageLumaA16(image) => (high_bytes(image.into_raw()), Gray, true),
        DynamicImage::ImageRgb16(image) => (high_bytes(image.into_raw()), Rgb, false),
        DynamicImage::ImageRgba16(image) => (high_bytes(image.into_raw()), Rgb, true),
        _ => return Err(INVALID),
    };
    let (pixels, mask) = if has_alpha {
        let components = usize::from(space.components());
        let count = pixels.len() / (components + 1);
        let mut color = Vec::with_capacity(count * components);
        let mut mask = Vec::with_capacity(count);
        for pixel in pixels.chunks_exact(components + 1) {
            let alpha = u32::from(pixel[components]);
            let inverse = (255 * 256u32).checked_div(alpha).unwrap_or(0);
            // Match the reference's 8-bit premultiplication and the
            // straight-color samples it writes next to a separate alpha mask.
            color.extend(pixel[..components].iter().map(|&sample| {
                let covered = (u32::from(sample) * alpha + 127) / 255;
                ((covered * inverse) >> 8) as u8
            }));
            mask.push(alpha as u8);
        }
        let mask = super::document_output::image_stream(width, height, Gray.pdf_name(), &mask)?;
        (color, Some(mask))
    } else {
        (pixels, None)
    };
    let samples = super::document_output::image_stream(width, height, space.pdf_name(), &pixels)?;
    let profile = profile.and_then(|bytes| embedded_profile(space, bytes));
    Ok((
        assembly::OverlayImage {
            samples,
            mask,
            profile,
        },
        [width, height],
    ))
}

fn overlay_resource(
    image: ImageData,
    profile: Option<Object>,
    invert_cmyk: bool,
) -> assembly::OverlayImage {
    let mut fields = image.build_xobject_dict();
    if invert_cmyk {
        fields.insert(
            "Decode".into(),
            Object::Array([1, 0, 1, 0, 1, 0, 1, 0].map(Object::Integer).into()),
        );
    }
    let mask = image
        .build_soft_mask_dict()
        .zip(image.soft_mask)
        .map(|(dict, data)| Object::Stream {
            dict,
            data: data.into(),
        });
    assembly::OverlayImage {
        samples: Object::Stream {
            dict: fields,
            data: image.data.into(),
        },
        mask,
        profile,
    }
}

fn corrected_dpi((x, y): (u32, u32)) -> (u32, u32) {
    let x = if x == 0 { 96 } else { x };
    let y = if y == 0 { 96 } else { y };
    if x > 4800 || y > 4800 {
        return (72, 72);
    }
    let scale = x.min(y);
    if scale < 72 {
        (x * 72 / scale, y * 72 / scale)
    } else {
        (x, y)
    }
}

fn png_dpi(bytes: &[u8]) -> Option<u32> {
    let mut remaining = bytes.get(8..)?;
    while remaining.len() >= 12 {
        let length = u32::from_be_bytes(remaining[..4].try_into().ok()?) as usize;
        let content = remaining.get(8..8usize.checked_add(length)?)?;
        match &remaining[4..8] {
            b"pHYs" if content.len() == 9 && content[8] == 1 => {
                let pixels_per_meter = u32::from_be_bytes(content[..4].try_into().ok()?);
                return Some(((u64::from(pixels_per_meter) * 254 + 5000) / 10000) as u32);
            }
            b"IDAT" | b"IEND" => return None,
            _ => remaining = remaining.get(12usize.checked_add(length)?..)?,
        }
    }
    None
}

fn jpeg_page_metadata(bytes: &[u8]) -> (Option<(u32, u32)>, Orientation) {
    // The reference consults the first saved APP1, APP2 or APP13 marker for
    // physical geometry. A Photoshop density there takes precedence over JFIF.
    let primary = jpeg_segments(bytes).find(|(marker, _)| matches!(marker, 0xe1 | 0xe2 | 0xed));
    let exif = primary
        .filter(|(marker, _)| *marker == 0xe1)
        .and_then(|(_, data)| data.strip_prefix(b"Exif\0\0"));
    let resolution = exif
        .and_then(exif_dpi)
        .or_else(|| {
            let (marker, data) = primary?;
            (marker == 0xed).then(|| photoshop_dpi(data)).flatten()
        })
        .or_else(|| jfif_dpi(bytes));
    let orientation = exif
        .and_then(Orientation::from_exif_chunk)
        .unwrap_or(Orientation::NoTransforms);
    (resolution, orientation)
}

fn jfif_dpi(bytes: &[u8]) -> Option<(u32, u32)> {
    let (_, segment) = jpeg_segments(bytes)
        .find(|(marker, data)| *marker == 0xe0 && data.starts_with(b"JFIF\0"))?;
    let x = u16::from_be_bytes(segment.get(8..10)?.try_into().ok()?) as u32;
    let y = u16::from_be_bytes(segment.get(10..12)?.try_into().ok()?) as u32;
    match segment[7] {
        1 => Some((x, y)),
        2 => Some((x * 254 / 100, y * 254 / 100)),
        _ => None,
    }
}

pub(super) fn jpeg_segments(bytes: &[u8]) -> impl Iterator<Item = (u8, &[u8])> {
    let mut remaining = bytes.strip_prefix(b"\xff\xd8").unwrap_or_default();
    std::iter::from_fn(move || {
        let marker_bytes = remaining.strip_prefix(b"\xff")?;
        let prefix = marker_bytes.iter().position(|&byte| byte != 0xff)?;
        let (&marker, contents) = marker_bytes[prefix..].split_first()?;
        if matches!(marker, 0xda | 0xd9) {
            return None;
        }
        let length = usize::from(u16::from_be_bytes(contents.get(..2)?.try_into().ok()?));
        let segment = contents.get(2..length)?;
        remaining = contents.get(length..)?;
        Some((marker, segment))
    })
}

fn photoshop_dpi(segment: &[u8]) -> Option<(u32, u32)> {
    // JPEG APP13 Photoshop image resources are padded to even boundaries.
    let mut resources = segment.strip_prefix(b"Photoshop 3.0\0")?;
    while let Some(block) = resources.strip_prefix(b"8BIM") {
        let identifier = u16::from_be_bytes(block.get(..2)?.try_into().ok()?);
        let name_bytes = usize::from(*block.get(2)?) + 1;
        let size_at = 2 + name_bytes.next_multiple_of(2);
        let size = u32::from_be_bytes(block.get(size_at..size_at + 4)?.try_into().ok()?) as usize;
        let data = block.get(size_at + 4..)?;
        let resource = data.get(..size)?;
        if identifier == 0x03ed && resource.len() == 16 {
            // ResolutionInfo stores 16.16 pixels per inch, even when its
            // additional unit fields select centimetres for display.
            let dpi = |offset: usize| {
                let fixed = u32::from_be_bytes(resource.get(offset..offset + 4)?.try_into().ok()?);
                (fixed >= 65536).then_some(fixed >> 16)
            };
            return Some((dpi(0)?, dpi(8)?));
        }
        resources = data.get(size.checked_add(size % 2)?..)?;
    }
    None
}

fn exif_dpi(bytes: &[u8]) -> Option<(u32, u32)> {
    let little = match bytes.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let short = |offset: usize| {
        let pair = bytes.get(offset..offset.checked_add(2)?)?.try_into().ok()?;
        Some(if little {
            u16::from_le_bytes(pair)
        } else {
            u16::from_be_bytes(pair)
        })
    };
    let long = |offset: usize| {
        let value = bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?;
        Some(if little {
            u32::from_le_bytes(value)
        } else {
            u32::from_be_bytes(value)
        })
    };
    if short(2)? != 42 {
        return None;
    }
    let ifd = long(4)? as usize;
    let count = usize::from(short(ifd)?);
    let mut unit = 0;
    let mut resolution = [None, None];
    for index in 0..count {
        let entry = ifd.checked_add(2 + 12 * index)?;
        match (short(entry)?, short(entry + 2)?, long(entry + 4)?) {
            (tag @ (282 | 283), 5, 1) => {
                let value = long(entry + 8)? as usize;
                resolution[usize::from(tag - 282)] =
                    Some((long(value)?, long(value.checked_add(4)?)?));
            }
            (296, 3, 1) => unit = short(entry + 8)?,
            _ => {}
        }
    }
    if resolution
        .iter()
        .any(|axis| !axis.is_some_and(|(numerator, denominator)| numerator > 0 && denominator > 0))
    {
        return None;
    }
    let numerator = match unit {
        2 => 100_u64,
        3 => 254,
        _ => return Some((0, 0)),
    };
    let scale = |value: Option<(u32, u32)>| {
        let (top, bottom) = value?;
        let dpi = (u64::from(top) * numerator).checked_div(u64::from(bottom) * 100)?;
        u32::try_from(dpi).ok()
    };
    Some((scale(resolution[0])?, scale(resolution[1])?))
}
