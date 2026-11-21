//! Working and generated PDF compression with explicit image-quality presets.
use super::{
    artifacts::refusal,
    document_verification::RevisionInput,
    image_uses::{self, ImageColors, ImagePaint},
    ocr::Product,
    page_tools::{assembly, signature_guard},
    revisions::ValidatedRevisionPdf,
    *,
};
use pdf_oxide::{
    extractors::{extract_image_from_xobject, images::ColorSpace},
    object::{Object, ObjectRef},
    PdfDocument,
};
use sha2::Digest as _;
use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, Ordering},
};

pub(super) fn job_inputs(store: &Store, id: &str, body: &Value) -> Result<(String, Value), Reply> {
    if store.key.is_none() {
        return Err(refusal(
            423,
            "Use the protected Rust store for working revisions",
        ));
    }
    let doc = store.artifact_document(id)?.body;
    let revision = super::integer(&body["expected_revision"])
        .and_then(|value| u64::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| refusal(422, "Choose a working revision"))?;
    let preset = body
        .get("preset")
        .map_or(Some("lossless"), Value::as_str)
        .filter(|preset| matches!(*preset, "lossless" | "balanced" | "small"))
        .ok_or_else(|| refusal(422, "Unknown compression preset"))?;
    let acknowledged = body["acknowledge_signature_risk"] == true;
    signature_guard([&doc], acknowledged, "compressing this working copy")?;
    let matter = doc["matter_id"]
        .as_str()
        .ok_or_else(|| refusal(409, "Invalid document matter"))?;
    Ok((
        matter.to_owned(),
        json!({"revision":revision,"preset":preset,"acknowledge_signature_risk":acknowledged}),
    ))
}

pub(super) struct Work {
    id: String,
    revision: u64,
    source: RevisionInput,
    preset: String,
}

impl Work {
    pub fn prepare(store: &Store, id: &str, context: &Value) -> Result<Self, Reply> {
        let revision = context["revision"]
            .as_u64()
            .ok_or_else(|| refusal(409, "Invalid saved revision"))?;
        let doc = store.artifact_document(id)?.body;
        signature_guard(
            [&doc],
            context["acknowledge_signature_risk"] == true,
            "compressing this working copy",
        )?;
        if doc["revision"] != revision {
            return Err(refusal(
                409,
                "Document changed before compression; reload and try again",
            ));
        }
        let source = RevisionInput::capture(store, &doc, id, revision)?;
        Ok(Self {
            id: id.to_owned(),
            revision,
            source,
            preset: context["preset"].as_str().unwrap_or("lossless").to_owned(),
        })
    }

    pub fn run(
        self,
        cancel: &AtomicBool,
        progress: impl Fn(u8, &str) -> Result<(), Reply>,
    ) -> Result<Product, Reply> {
        let pages = self.source.pages;
        let (source, files) = self.source.read(|| cancel.load(Ordering::Acquire))?;
        let (bytes, mut result) = compress(source, &self.preset, progress)?;
        let candidate = (result["changed"] == true)
            .then(|| ValidatedRevisionPdf::prepare(bytes, pages))
            .transpose()?;
        result["document_id"] = json!(self.id);
        Ok(Product {
            document: self.id,
            revision: self.revision,
            candidate,
            files,
            retained: None,
            ocr: None,
            result,
        })
    }
}

#[derive(Default)]
pub(super) struct Images {
    replacements: HashMap<ObjectRef, RecompressedImage>,
    rewritten: usize,
    skipped: usize,
}

impl Images {
    pub fn replace(&mut self, reference: ObjectRef, source: Object) -> (Object, bool) {
        self.replacements
            .remove(&reference)
            .map(|image| (image.stream, image.attach_srgb))
            .unwrap_or((source, false))
    }

    pub fn deflate(fields: &mut assembly::Dictionary, data: &[u8]) -> Option<Vec<u8>> {
        if fields.contains_key("Filter") || fields.contains_key("F") || data.len() < 64 {
            return None;
        }
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).ok()?;
        let encoded = encoder.finish().ok()?;
        if encoded.len() + 24 >= data.len() {
            return None;
        }
        fields.insert("Filter".into(), Object::Name("FlateDecode".into()));
        Some(encoded)
    }
}

const MAX_IMAGE_PIXELS: u64 = 40_000_000;
type Digest = [u8; 32];

struct Raster {
    bitmap: image::DynamicImage,
    gray: bool,
    space: Object,
    attach_srgb: bool,
    applied_decode: bool,
}

#[derive(PartialEq)]
struct Samples {
    digest: Digest,
    gray: bool,
    space: Object,
    applied_decode: bool,
}

struct Observation {
    samples: Samples,
    attach_srgb: bool,
}

impl Raster {
    fn samples(&self) -> Samples {
        Samples {
            digest: Sha256::digest(self.bitmap.as_bytes()).into(),
            gray: self.gray,
            space: self.space.clone(),
            applied_decode: self.applied_decode,
        }
    }
}

fn integer_field(pdf: &PdfDocument, fields: &assembly::Dictionary, key: &str) -> Option<u64> {
    u64::try_from(pdf.resolve_object(fields.get(key)?).ok()?.as_integer()?).ok()
}

fn decode(
    pdf: &PdfDocument,
    source: &Object,
    reference: Option<ObjectRef>,
    colors: &ImageColors,
) -> Option<Raster> {
    let fields = source.as_dict()?;
    let width = integer_field(pdf, fields, "Width")?;
    let height = integer_field(pdf, fields, "Height")?;
    if !(1..=MAX_IMAGE_PIXELS).contains(&width.checked_mul(height)?) {
        return None;
    }
    let binding = fields.get("ColorSpace");
    let effective = match binding {
        Some(color) => Some(colors.resolve(pdf, color)?),
        None => None,
    };
    let resolved;
    let input = if let Some(effective) = &effective {
        let Object::Stream { data, .. } = source else {
            return None;
        };
        let mut fields = fields.clone();
        fields.insert("ColorSpace".into(), effective.clone());
        resolved = Object::Stream {
            dict: fields,
            data: data.clone(),
        };
        &resolved
    } else {
        source
    };
    let image = extract_image_from_xobject(Some(pdf), input, reference, None).ok()?;
    let bitmap = image.to_dynamic_image().ok()?;
    if (u64::from(bitmap.width()), u64::from(bitmap.height())) != (width, height)
        || bitmap.color().has_alpha()
    {
        return None;
    }
    let gray = matches!(
        image.color_space(),
        ColorSpace::DeviceGray | ColorSpace::CalGray
    );
    // Raw gray/RGB samples can retain their calibration or embedded profile.
    // Four-component and expanded indexed samples are already converted.
    let compatible = effective.as_ref().is_some_and(|space| match space {
        Object::Name(name) => name == if gray { "DeviceGray" } else { "DeviceRGB" },
        Object::Array(items) => match items.first().and_then(Object::as_name) {
            Some("ICCBased") => items
                .get(1)
                .and_then(|profile| pdf.resolve_object(profile).ok())
                .and_then(|profile| profile.as_dict()?.get("N")?.as_integer())
                .is_some_and(|n| n == if gray { 1 } else { 3 }),
            Some("CalGray") => gray,
            Some("CalRGB") => !gray,
            _ => false,
        },
        _ => false,
    });
    let space = if compatible {
        binding?.clone()
    } else {
        Object::Name("DeviceRGB".into())
    };
    let gray = compatible && gray;
    let bitmap = if gray {
        image::DynamicImage::ImageLuma8(bitmap.into_luma8())
    } else {
        image::DynamicImage::ImageRgb8(bitmap.into_rgb8())
    };
    Some(Raster {
        bitmap,
        gray,
        space,
        attach_srgb: !compatible && colors.remaps_device(pdf, "DeviceRGB"),
        applied_decode: image.decode_folded_in() || !compatible,
    })
}

type SampleCache = HashMap<ObjectRef, Vec<(ImageColors, Option<Observation>)>>;

fn resource_samples<'a>(
    pdf: &PdfDocument,
    reference: ObjectRef,
    colors: &ImageColors,
    cache: &'a mut SampleCache,
) -> Option<&'a Observation> {
    let contexts = cache.entry(reference).or_default();
    let index = contexts.iter().position(|(scope, _)| scope == colors);
    let index = index.unwrap_or_else(|| {
        let samples = pdf
            .load_object(reference)
            .ok()
            .and_then(|source| decode(pdf, &source, Some(reference), colors))
            .map(|raster| Observation {
                samples: raster.samples(),
                attach_srgb: raster.attach_srgb,
            });
        let index = contexts.len();
        contexts.push((colors.clone(), samples));
        index
    });
    contexts[index].1.as_ref()
}

fn keep_max(value: &mut [f64; 2], size: [f64; 2]) {
    value[0] = value[0].max(size[0]);
    value[1] = value[1].max(size[1]);
}

enum ImageCandidate {
    Retained,
    Rewritten(RecompressedImage),
}

struct RecompressedImage {
    stream: Object,
    attach_srgb: bool,
}

fn image_candidate(
    pdf: &PdfDocument,
    reference: ObjectRef,
    dpi: f64,
    quality: u8,
    size: [f64; 2],
    colors: &ImageColors,
    attach_srgb: bool,
) -> Option<ImageCandidate> {
    let source = pdf.load_object(reference).ok()?;
    let Object::Stream { dict: fields, data } = &source else {
        return None;
    };
    let masked = [
        "SMask",
        "Mask",
        "ImageMask",
        "SMaskInData",
        "Alternates",
        "OPI",
    ]
    .into_iter()
    .any(|key| {
        fields
            .get(key)
            .is_some_and(|value| match pdf.resolve_object(value) {
                Ok(Object::Null | Object::Boolean(false) | Object::Integer(0)) => false,
                Ok(Object::Name(name)) if name == "None" => false,
                _ => true,
            })
    });
    if masked || integer_field(pdf, fields, "BitsPerComponent")? <= 1 {
        return None;
    }
    let raster = decode(pdf, &source, Some(reference), colors)?;
    let ratio = (size[0] * dpi / 72.0 / f64::from(raster.bitmap.width()))
        .max(size[1] * dpi / 72.0 / f64::from(raster.bitmap.height()))
        .min(1.0);
    let bitmap = if ratio < 0.95 {
        raster.bitmap.resize_exact(
            (f64::from(raster.bitmap.width()) * ratio).max(1.0) as u32,
            (f64::from(raster.bitmap.height()) * ratio).max(1.0) as u32,
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        raster.bitmap
    };
    let mut encoded = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, quality)
        .encode_image(&bitmap)
        .ok()?;
    if encoded.len() >= data.len() {
        return Some(ImageCandidate::Retained);
    }
    let mut fields = fields.clone();
    fields.insert("Filter".into(), Object::Name("DCTDecode".into()));
    fields.insert("BitsPerComponent".into(), Object::Integer(8));
    fields.insert("ColorSpace".into(), raster.space);
    fields.remove("DecodeParms");
    if raster.applied_decode
        || fields.get("Decode").is_some_and(|value| {
            value
                .as_array()
                .is_some_and(|pairs| pairs.len() != if raster.gray { 2 } else { 6 })
        })
    {
        fields.remove("Decode");
    }
    fields.insert("Width".into(), Object::Integer(bitmap.width().into()));
    fields.insert("Height".into(), Object::Integer(bitmap.height().into()));
    Some(ImageCandidate::Rewritten(RecompressedImage {
        stream: Object::Stream {
            dict: fields,
            data: encoded.into(),
        },
        attach_srgb,
    }))
}

fn plan_images(
    pdf: &PdfDocument,
    pages: usize,
    dpi: f64,
    quality: u8,
    progress: &impl Fn(u8, &str) -> Result<(), Reply>,
) -> Result<Images, Reply> {
    let error = |detail| refusal(422, detail);
    let mut references = Vec::new();
    let mut placements = HashMap::new();
    let mut samples = SampleCache::new();
    for page in 0..pages {
        let message = format!(
            "Inspecting image placements on page {} of {pages}",
            page + 1
        );
        progress(5, &message)?;
        let resources = image_uses::resources(pdf, page).map_err(error)?;
        if resources.images.is_empty() {
            continue;
        }
        let mut by_reference = HashMap::new();
        let mut by_samples = HashMap::new();
        let mut interrupted = None;
        image_uses::painted(pdf, page, |placement| {
            if let Err(error) = progress(5, &message) {
                interrupted = Some(error);
                return true;
            }
            let [width, height] = placement.size;
            if width <= 0.0 || height <= 0.0 {
                return false;
            }
            let hash = match placement.paint {
                ImagePaint::Resource(resource) => {
                    keep_max(
                        by_reference.entry(resource.reference).or_insert([0.0; 2]),
                        placement.size,
                    );
                    resource_samples(pdf, resource.reference, &resource.colors, &mut samples)
                        .map(|record| record.samples.digest)
                }
                ImagePaint::Inline {
                    fields,
                    data,
                    colors,
                } => {
                    let image = Object::Stream {
                        dict: fields,
                        data: data.into(),
                    };
                    decode(pdf, &image, None, &colors).map(|raster| raster.samples().digest)
                }
            };
            if let Some(hash) = hash {
                keep_max(by_samples.entry(hash).or_insert([0.0; 2]), placement.size);
            }
            false
        })
        .map_err(error)?;
        if let Some(interrupted) = interrupted {
            return Err(interrupted);
        }
        for resource in resources.images {
            let reference = resource.reference;
            progress(5, &message)?;
            let entry = placements.entry(reference).or_insert_with(|| {
                references.push(reference);
                [0.0; 2]
            });
            if let Some(&size) = by_reference.get(&reference) {
                keep_max(entry, size);
            }
            // The reference looks up placements by decoded image samples, but
            // only on pages where this object is in the resource catalogue.
            if let Some(size) = resource_samples(pdf, reference, &resource.colors, &mut samples)
                .and_then(|record| by_samples.get(&record.samples.digest))
            {
                keep_max(entry, *size);
            }
        }
    }

    let mut images = Images::default();
    let count = references.len();
    for (index, reference) in references.into_iter().enumerate() {
        progress(
            (5 + 70 * index / count.max(1)) as u8,
            &format!("Compressing image {} of {count}", index + 1),
        )?;
        let size = placements[&reference];
        let candidate = samples.get(&reference).and_then(|contexts| {
            let (colors, first) = contexts.first()?;
            let first = first.as_ref()?;
            // An image stream is shared across its resource scopes. Rewrite
            // only if one sample representation works in every declared scope.
            if size[0] <= 0.0
                || size[1] <= 0.0
                || contexts.iter().any(|(_, observation)| {
                    observation.as_ref().map(|value| &value.samples) != Some(&first.samples)
                })
            {
                return None;
            }
            let attach_srgb = contexts.iter().any(|(_, observation)| {
                observation.as_ref().is_some_and(|value| value.attach_srgb)
            });
            image_candidate(pdf, reference, dpi, quality, size, colors, attach_srgb)
        });
        match candidate {
            Some(ImageCandidate::Rewritten(replacement)) => {
                images.replacements.insert(reference, replacement);
                images.rewritten += 1;
            }
            Some(ImageCandidate::Retained) => {}
            None => images.skipped += 1,
        }
    }
    Ok(images)
}

pub(super) fn compress(
    bytes: Vec<u8>,
    preset: &str,
    progress: impl Fn(u8, &str) -> Result<(), Reply>,
) -> Result<(Vec<u8>, Value), Reply> {
    let error = |_| refusal(422, "Unable to compress this PDF safely");
    let pdf = PdfDocument::from_bytes(bytes.clone()).map_err(error)?;
    let pages = pdf.page_count().map_err(error)?;
    let mut images = match preset {
        "lossless" => Images::default(),
        "balanced" => plan_images(&pdf, pages, 240.0, 85, &progress)?,
        "small" => plan_images(&pdf, pages, 150.0, 70, &progress)?,
        _ => return Err(refusal(422, "Unknown compression preset")),
    };
    progress(80, "Writing compressed candidate")?;
    let before = bytes.len();
    let candidate = assembly::compress_document(bytes.clone(), &mut images).map_err(|_| {
        refusal(
            422,
            "Unable to compress this PDF while preserving its resources",
        )
    })?;
    let changed = candidate.len() < before;
    if changed {
        let check = PdfDocument::from_bytes(candidate.clone()).map_err(error)?;
        if check.page_count().map_err(error)? != pages {
            return Err(refusal(422, "Compression changed the page count"));
        }
        for page in 0..pages {
            progress(
                (85 + 10 * page / pages.max(1)) as u8,
                "Checking compressed PDF page contents",
            )?;
            if pdf.get_page_visible_box(page).map_err(error)?
                != check.get_page_visible_box(page).map_err(error)?
                || pdf.get_page_rotation(page).map_err(error)?
                    != check.get_page_rotation(page).map_err(error)?
                || pdf.extract_text(page).map_err(error)?
                    != check.extract_text(page).map_err(error)?
            {
                return Err(refusal(
                    422,
                    "Compression changed page geometry or text; prior bytes retained",
                ));
            }
        }
    }
    let after = if changed { candidate.len() } else { before };
    let result = json!({"preset":preset,"before_bytes":before,"after_bytes":after,"changed":changed,
        "images_rewritten":if changed { images.rewritten } else {0},"images_skipped":images.skipped,
        "message":if changed {"Compressed working copy; review visual quality."} else {"No smaller result; working copy retained."}});
    Ok((if changed { candidate } else { bytes }, result))
}
