//! Render read-only views of an authenticated working revision.
use super::{
    artifacts::{refusal, PdfSource},
    reading, *,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use pdf_oxide::{
    rendering::{render_page, RenderOptions},
    PdfDocument,
};

const MAX_PREVIEW_PIXELS: u64 = 4_000_000;

fn number(query: &str, key: &str, default: Option<i64>) -> Result<i64, Reply> {
    match reading::lookup(query,key) {
        Some(value) => integer(&json!(value)).ok_or_else(|| reply(422,json!({"detail":[{
            "type":"int_parsing","loc":["query",key],"msg":"Input should be a valid integer, unable to parse string as an integer","input":value
        }]}))),
        None => default.ok_or_else(|| reading::missing_query(key)),
    }
}

pub(super) fn render(
    doc: &PdfDocument,
    index: usize,
    thumbnail: bool,
) -> Result<Vec<u8>, &'static str> {
    render_at_scale(doc, index, (!thumbnail).then_some(1.4))
}

pub(super) fn render_at_scale(
    doc: &PdfDocument,
    index: usize,
    scale: Option<f32>,
) -> Result<Vec<u8>, &'static str> {
    let img = raster_at_scale(doc, index, scale, MAX_PREVIEW_PIXELS)?;
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::DynamicImage::ImageRgba8(img).to_rgb8())
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|_| "Cannot encode page preview")?;
    Ok(png.into_inner())
}

pub(super) fn raster_at_scale(
    doc: &PdfDocument,
    index: usize,
    scale: Option<f32>,
    limit: u64,
) -> Result<image::RgbaImage, &'static str> {
    let (x0, y0, x1, y1) = doc
        .get_page_visible_box(index)
        .map_err(|_| "Cannot read page bounds")?;
    let rotation = doc
        .get_page_rotation(index)
        .map_err(|_| "Cannot read page rotation")?;
    let unit = super::page_tools::assembly::page_user_unit(doc, index)?;
    let (mut width, mut height) = (f64::from(x1 - x0) * unit, f64::from(y1 - y0) * unit);
    if [90, 270].contains(&rotation) {
        std::mem::swap(&mut width, &mut height);
    }
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return Err("Page geometry is invalid");
    }
    let scale = scale
        .map(f64::from)
        .unwrap_or_else(|| (180.0 / width).min(220.0 / height).min(1.0));
    let (target_width, target_height) = (
        (width * scale).ceil().max(1.0) as u32,
        (height * scale).ceil().max(1.0) as u32,
    );
    if u64::from(target_width) * u64::from(target_height) > limit {
        return Err("Page preview exceeds the raster size limit");
    }
    let mut options =
        RenderOptions::with_dpi((scale * unit * 72.0).ceil().max(1.0) as u32).as_raw();
    options.max_output_pixels = limit;
    let image = render_page(doc, index, &options).map_err(|_| "Cannot render page content")?;
    let rgba = image::RgbaImage::from_raw(image.width, image.height, image.data)
        .ok_or("Invalid raster output")?;
    Ok(
        if (image.width, image.height) == (target_width, target_height) {
            rgba
        } else {
            image::imageops::resize(
                &rgba,
                target_width,
                target_height,
                image::imageops::FilterType::Triangle,
            )
        },
    )
}

pub struct PagePreviewTarget {
    document: String,
    thumbnail: bool,
}

impl PagePreviewTarget {
    pub fn from_path(method: &str, path: &str) -> Option<Self> {
        let route = path.trim_start_matches('/').split('/').collect::<Vec<_>>();
        Self::select(method, &route)
    }

    fn select(method: &str, route: &[&str]) -> Option<Self> {
        let ("GET", ["documents", id, resource @ ("thumbnail" | "metadata-source")]) =
            (method, route)
        else {
            return None;
        };
        Some(Self {
            document: (*id).to_owned(),
            thumbnail: *resource == "thumbnail",
        })
    }
}

pub struct PagePreviewWork {
    document: String,
    source: PdfSource,
    thumbnail: bool,
    revision: i64,
    page: i64,
    pages: Value,
}

fn unavailable(thumbnail: bool, message: &str) -> Reply {
    let prefix = if thumbnail {
        "Cannot preview working PDF"
    } else {
        "Source preview unavailable"
    };
    refusal(422, format!("{prefix}: {message}"))
}

impl Store {
    pub fn prepare_page_preview(
        &self,
        target: PagePreviewTarget,
        query: &str,
    ) -> Result<PagePreviewWork, Reply> {
        let PagePreviewTarget {
            document,
            thumbnail,
        } = target;
        let revision = number(query, "revision", None)?;
        let page = if thumbnail {
            1
        } else {
            number(query, "page", Some(1))?
        };
        let doc = self.artifact_document(&document)?;
        if doc.body["revision"].as_i64() != Some(revision) {
            return Err(refusal(
                409,
                if thumbnail {
                    "Working PDF revision changed; reload the document"
                } else {
                    "Source revision changed; reload suggestions"
                },
            ));
        }
        if page < 1
            || doc.body["page_count"]
                .as_i64()
                .is_none_or(|count| page > count)
        {
            return Err(unavailable(
                thumbnail,
                "Source page is outside the document",
            ));
        }
        let source = self.document_pdf_source(
            &document,
            &doc,
            &Artifact::Working {
                revision: Some(revision as u64),
            },
        )?;
        Ok(PagePreviewWork {
            document,
            source,
            thumbnail,
            revision,
            page,
            pages: doc.body["page_count"].clone(),
        })
    }
}

impl PagePreviewWork {
    pub fn compute(self) -> Result<Reply, Reply> {
        let Self {
            document,
            source,
            thumbnail,
            revision,
            page,
            pages,
        } = self;
        let unavailable = |message| unavailable(thumbnail, message);
        let bytes = source
            .open()?
            .into_bytes()
            .map_err(|_| unavailable("Cannot read working bytes"))?;
        let pdf = PdfDocument::from_bytes(bytes).map_err(|_| unavailable("Invalid PDF"))?;
        let png = render(&pdf, page as usize - 1, thumbnail).map_err(unavailable)?;
        let encoded = STANDARD.encode(&png);
        if thumbnail {
            return Ok(reply(200, json!({"png":encoded})));
        }
        Ok(reply(
            200,
            json!({"image":format!("data:image/png;base64,{encoded}"),
            "document_id":document,"revision":revision,"page":page,"pages":pages}),
        ))
    }
}

pub(super) fn route(store: &Store, method: &str, route: &[&str], query: &str) -> Option<Reply> {
    let target = PagePreviewTarget::select(method, route)?;
    Some(
        store
            .prepare_page_preview(target, query)
            .and_then(PagePreviewWork::compute)
            .unwrap_or_else(|refusal| refusal),
    )
}
