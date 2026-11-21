//! Local searchable OCR layers, confidence review and page screening.
use super::{
    artifacts::{refusal, Artifact, PdfProof, PdfSource},
    bundle_inputs::Inputs,
    document_verification::{DocumentFileProof, RevisionInput},
    page_tools::{assembly, signature_guard},
    reading,
    revisions::ValidatedRevisionPdf,
    tesseract::{Engine, Word},
    *,
};
use pdf_oxide::{
    rendering::{render_page, RenderOptions, RenderedImage},
    PdfDocument,
};
use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    sync::atomic::{AtomicBool, Ordering},
};
use unicode_segmentation::UnicodeSegmentation;

mod rerun;

const OUTPUT_ERROR: &str = "Unable to prepare current working PDF for OCR";
const LIMIT: u64 = 40_000_000;
const RENDER_LIMIT: &str = "OCR page exceeds the 40-megapixel processing limit";

pub(super) fn migrate(con: &Connection) -> rusqlite::Result<()> {
    con.execute_batch(
        "CREATE TABLE rust_ocr_reviews(document_id TEXT NOT NULL,revision INTEGER NOT NULL,page INTEGER NOT NULL,
            accepted_at TEXT NOT NULL, PRIMARY KEY(document_id,revision,page));
         CREATE TABLE rust_page_screenings(matter_id TEXT NOT NULL,digest TEXT NOT NULL,report TEXT NOT NULL,
            PRIMARY KEY(matter_id,digest));
         PRAGMA user_version=11;",
    )
}

fn render(pdf: &PdfDocument, page: usize, screen: bool) -> Result<RenderedImage, &'static str> {
    let geometry = assembly::geometry(pdf, page)?;
    let (width, height) = geometry.clipped().unwrap_or(geometry).display_size();
    let unit = assembly::page_user_unit(pdf, page)?;
    let (width, height) = (width * unit, height * unit);
    let scale = if screen {
        (1600.0 / width.max(height).max(1.0)).min(1.0)
    } else {
        240.0 / 72.0
    };
    let (target_width, target_height) = (
        (width * scale).ceil().max(1.0),
        (height * scale).ceil().max(1.0),
    );
    if !target_width.is_finite()
        || !target_height.is_finite()
        || target_width * target_height > LIMIT as f64
    {
        return Err(RENDER_LIMIT);
    }
    let dpi = if screen { scale * 72.0 } else { 240.0 };
    let mut options = RenderOptions::with_dpi((dpi * unit).ceil().max(1.0) as u32).as_raw();
    options.max_output_pixels = LIMIT;
    options.render_annotations = screen;
    let mut image =
        render_page(pdf, page, &options).map_err(|_| "Cannot render the selected page")?;
    let (width, height) = (target_width as u32, target_height as u32);
    if (image.width, image.height) != (width, height) {
        let raster = image::RgbaImage::from_raw(image.width, image.height, image.data)
            .ok_or("Invalid raster for OCR")?;
        image.data = image::imageops::resize(
            &raster,
            width,
            height,
            image::imageops::FilterType::Triangle,
        )
        .into_raw();
        image.width = width;
        image.height = height;
    }
    Ok(image)
}

fn visible_words(
    engine: &Engine,
    pdf: &PdfDocument,
    page: usize,
    cancel: &AtomicBool,
) -> Result<Vec<Word>, &'static str> {
    let image = render(pdf, page, false)?;
    engine.recognize(&image.data, image.width, image.height, cancel)
}

fn layer_words(
    engine: &Engine,
    pdf: &PdfDocument,
    page: usize,
    rotation: i32,
    cancel: &AtomicBool,
) -> Result<Vec<Word>, &'static str> {
    if rotation == 0 {
        return visible_words(engine, pdf, page, cancel);
    }
    let image = render(pdf, page, false)?;
    let raster = image::RgbaImage::from_raw(image.width, image.height, image.data)
        .ok_or("Invalid raster for OCR")?;
    let raster = match rotation {
        90 => image::imageops::rotate270(&raster),
        180 => image::imageops::rotate180(&raster),
        270 => image::imageops::rotate90(&raster),
        _ => return Err("Invalid page rotation for OCR"),
    };
    engine.recognize(raster.as_raw(), raster.width(), raster.height(), cancel)
}

fn rounded(value: f64, digits: usize) -> f64 {
    // Decimal rounding works from the original binary float, including ties.
    format!("{value:.digits$}").parse().unwrap_or(value)
}

fn confidence(words: &[Word]) -> (Option<f64>, Vec<(&Word, f64)>) {
    let mean = (!words.is_empty()).then(|| {
        rounded(
            words
                .iter()
                .map(|word| rounded(word.confidence, 1))
                .sum::<f64>()
                / words.len() as f64,
            1,
        )
    });
    let mut low = words
        .iter()
        .map(|word| (word, rounded(word.confidence, 1)))
        .filter(|(_, score)| *score < 75.0)
        .collect::<Vec<_>>();
    low.sort_by(|a, b| a.1.total_cmp(&b.1));
    low.truncate(25);
    (mean, low)
}

fn measure(pdf: &PdfDocument, page: usize, lang: &str) -> Result<Value, Reply> {
    let image = render(pdf, page, false).map_err(|error| {
        refusal(
            422,
            if error == RENDER_LIMIT {
                "OCR review page exceeds the 40-megapixel inspection limit"
            } else {
                error
            },
        )
    })?;
    let engine = match Engine::for_measurement(lang) {
        Ok(engine) => engine,
        Err(super::tesseract::UNAVAILABLE) => {
            return Ok(json!({"available":false,"confidence":null,"regions":[],
                "message":"Tesseract confidence analysis is unavailable on this device."}));
        }
        Err(error) => return Err(refusal(422, error)),
    };
    let words = engine
        .recognize(
            &image.data,
            image.width,
            image.height,
            &AtomicBool::new(false),
        )
        .map_err(|error| refusal(422, error))?;
    let (mean, low) = confidence(&words);
    Ok(json!({"available":true,"confidence":mean,
        "regions":low.iter().map(|(word, score)| json!({"text":word.text.chars().take(100).collect::<String>(),
            "confidence":score,"rect":word.rect.map(|value| rounded(value,4))})).collect::<Vec<_>>(),
        "message":"Measured by a separate Tesseract pass on the visible page. Compare the embedded OCR text with the scan."}))
}

fn revision(doc: &Value) -> u64 {
    doc["revision"].as_u64().unwrap_or(0)
}

fn runs(doc: &Value) -> Vec<Value> {
    doc["ocr_provenance_json"]
        .as_str()
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or_default()
}

fn can_rerun(doc: &Value, page: usize) -> bool {
    runs(doc).last().is_some_and(|last| {
        last["applied_revision"] == revision(doc)
            && (last["requested_page_index"].is_null() || last["requested_page_index"] == page)
    })
}

fn retain_uncovered_words(words: Vec<Word>, existing: &[[f64; 4]]) -> Vec<Word> {
    let mut retained = Vec::new();
    for word in words {
        let [x, y, width, height] = word.rect;
        let overlaps = |left, right| {
            existing
                .iter()
                .any(|bounds| super::text_edit::intersects(*bounds, [left, y, right, y + height]))
        };
        if !overlaps(x, x + width) {
            retained.push(word);
            continue;
        }
        // The invisible layer uses equal character advances scaled to the
        // recognized word's box. Keep the original coordinates of each run
        // that would remain after removing text covered by native spans.
        let text: String = word
            .text
            .chars()
            .filter(|character| !character.is_control())
            .collect();
        let letters = text.chars().count();
        if letters == 0 {
            continue;
        }
        let advance = width / letters as f64;
        let fragment = |(byte, letter), end_byte, end_letter| Word {
            text: text[byte..end_byte].to_owned(),
            confidence: word.confidence,
            rect: [
                x + letter as f64 * advance,
                y,
                (end_letter - letter) as f64 * advance,
                height,
            ],
        };
        let mut start = None;
        let mut letter = 0;
        for (byte, grapheme) in text.grapheme_indices(true) {
            let next = letter + grapheme.chars().count();
            if overlaps(x + letter as f64 * advance, x + next as f64 * advance) {
                if let Some(start) = start.take() {
                    retained.push(fragment(start, byte, letter));
                }
            } else {
                start.get_or_insert((byte, letter));
            }
            letter = next;
        }
        if let Some(start) = start {
            retained.push(fragment(start, text.len(), letter));
        }
    }
    retained
}

pub(super) fn job_inputs(
    store: &Store,
    id: &str,
    body: &Value,
    rerun: bool,
) -> Result<(String, Value), Reply> {
    if store.key.is_none() {
        return Err(refusal(
            423,
            "Use the protected Rust store for working revisions",
        ));
    }
    let doc = store.artifact_document(id)?.body;
    let current = revision(&doc);
    let mut expected = None;
    if rerun || !body["expected_revision"].is_null() {
        let value = if rerun {
            body["expected_revision"].as_u64()
        } else {
            super::integer(&body["expected_revision"]).and_then(|value| u64::try_from(value).ok())
        };
        expected = Some(
            value
                .filter(|value| *value > 0)
                .ok_or_else(|| refusal(422, "Invalid working revision"))?,
        );
    }
    if expected.is_some_and(|expected| expected != current) {
        return Err(refusal(
            409,
            if rerun {
                "Working revision changed; reopen OCR review"
            } else {
                "Working revision changed before OCR submission"
            },
        ));
    }
    let mut page = None;
    if rerun || !body["page_index"].is_null() {
        let index = body["page_index"]
            .as_u64()
            .and_then(|page| usize::try_from(page).ok())
            .ok_or_else(|| refusal(422, "Invalid OCR page index"))?;
        let outside = doc["page_count"]
            .as_u64()
            .is_none_or(|count| index as u64 >= count);
        if rerun {
            if outside || doc["ocr_status"] != "processed" {
                return Err(refusal(422, "This page is not ready for OCR rerun"));
            }
            if !can_rerun(&doc, index) {
                return Err(refusal(
                    422,
                    "Only a page from the latest OCR revision can be safely rerun",
                ));
            }
        } else if outside {
            return Err(refusal(422, "OCR page is outside the current working PDF"));
        }
        page = Some(index);
    }
    let language = body
        .get("language")
        .map_or(Some("eng"), Value::as_str)
        .filter(|language| matches!(*language, "eng" | "hin" | "eng+hin"))
        .ok_or_else(|| refusal(422, "Unsupported OCR language"))?;
    let acknowledged = body["acknowledge_signature_risk"] == true;
    signature_guard(
        [&doc],
        acknowledged,
        if rerun {
            "rerunning OCR"
        } else {
            "running OCR"
        },
    )?;
    let matter = doc["matter_id"]
        .as_str()
        .ok_or_else(|| refusal(409, "Invalid document matter"))?
        .to_owned();
    Ok((
        matter,
        json!({"revision":current,"language":language,"page_index":page,"rerun":rerun,"acknowledge_signature_risk":acknowledged}),
    ))
}

pub(super) struct Work {
    pub document: String,
    pub revision: u64,
    source: RevisionInput,
    base: Option<PdfSource>,
    base_revision: u64,
    selected: Option<usize>,
    language: String,
    history: Vec<Value>,
}

pub(super) struct Product {
    pub document: String,
    pub revision: u64,
    pub candidate: Option<ValidatedRevisionPdf>,
    pub files: DocumentFileProof,
    pub retained: Option<PdfProof>,
    pub ocr: Option<(String, String)>,
    pub result: Value,
}

fn is_ocr_font(font: &str) -> bool {
    let name = font.to_ascii_lowercase();
    name.contains("glyphless") || name.contains("matterdraftocr")
}

impl Work {
    pub(super) fn prepare(store: &Store, id: &str, context: &Value) -> Result<Self, Reply> {
        let saved = store.artifact_document(id)?.body;
        let revision = revision(&saved);
        let rerun = context["rerun"] == true;
        if context["revision"] != revision {
            return Err(refusal(
                409,
                if rerun {
                    "Working revision changed before OCR rerun"
                } else {
                    "Document changed before OCR started; submit OCR again for the current revision"
                },
            ));
        }
        signature_guard(
            [&saved],
            context["acknowledge_signature_risk"] == true,
            if rerun {
                "rerunning OCR"
            } else {
                "running OCR"
            },
        )?;
        let selected = context["page_index"].as_u64().map(|value| value as usize);
        let language = context["language"]
            .as_str()
            .ok_or_else(|| refusal(422, "Unsupported OCR language"))?
            .to_owned();
        let source = RevisionInput::capture(store, &saved, id, revision)?;
        let history = runs(&saved);
        let mut base_revision = revision;
        let base = if rerun {
            base_revision = history
                .last()
                .and_then(|run| run["base_revision"].as_u64())
                .unwrap_or_else(|| revision.saturating_sub(1));
            let old = super::revisions::record(&store.con, id, base_revision)
                .ok()
                .flatten()
                .ok_or_else(|| refusal(409, "Pre-OCR working revision is unavailable"))?;
            Some(store.pdf_source(
                id,
                &Artifact::Retained {
                    revision: base_revision,
                    sha256: old.hash,
                },
            )?)
        } else {
            None
        };
        Ok(Self {
            document: id.to_owned(),
            revision,
            source,
            base,
            base_revision,
            selected,
            language,
            history,
        })
    }

    pub(super) fn run(
        self,
        cancel: &AtomicBool,
        progress: impl Fn(u8, &str) -> Result<(), Reply>,
    ) -> Result<Product, Reply> {
        let invalid = |error: &'static str| refusal(422, error);
        let cancelled = || cancel.load(Ordering::Acquire);
        let expected_pages = self.source.pages;
        let (source_bytes, files) = self.source.read(cancelled)?;
        let base = self
            .base
            .map(|source| source.read_with_proof(cancelled))
            .transpose()?;
        let (base_bytes, retained) = match base {
            Some((bytes, proof)) => (Some(bytes), Some(proof)),
            None => (None, None),
        };
        let native =
            PdfDocument::from_bytes(source_bytes.clone()).map_err(|_| invalid(OUTPUT_ERROR))?;
        let count = native.page_count().map_err(|_| invalid(OUTPUT_ERROR))?;
        let rerun = base_bytes.is_some();
        let base = base_bytes
            .clone()
            .map(PdfDocument::from_bytes)
            .transpose()
            .map_err(|_| invalid(OUTPUT_ERROR))?;
        if let Some(retained) = &base {
            rerun::check(
                &native,
                retained,
                self.selected.ok_or_else(|| invalid(OUTPUT_ERROR))?,
                count,
            )
            .map_err(invalid)?;
        }
        if self.selected.is_some_and(|page| page >= count) {
            return Err(invalid("OCR page is outside the current working PDF"));
        }
        let engine = Engine::new(&self.language).map_err(invalid)?;
        let pages = self.selected.map_or(0..count, |page| page..page + 1);
        let selected_count = pages.len();
        let source = base.as_ref().unwrap_or(&native);
        let mut overlays = HashMap::new();
        for (checked, page) in pages.enumerate() {
            if cancelled() {
                return Err(refusal(409, "Operation cancelled"));
            }
            let existing = source
                .extract_spans(page)
                .map_err(|_| invalid(OUTPUT_ERROR))?;
            let existing_text = existing.iter().any(|span| !span.text.trim().is_empty());
            let already = existing.iter().any(|span| is_ocr_font(&span.font_name))
                || (!rerun
                    && self.history.iter().any(|run| {
                        run["applied_revision"] == self.revision
                            && run["applied_pages"]
                                .as_array()
                                .is_some_and(|pages| pages.contains(&json!(page)))
                    }));
            if !already
                && (!existing_text
                    || super::image_uses::resources(source, page)
                        .map_err(invalid)?
                        .has_images)
            {
                let original_geometry = assembly::geometry(source, page).map_err(invalid)?;
                let original_geometry = original_geometry.clipped().unwrap_or(original_geometry);
                let results = layer_words(&engine, source, page, original_geometry.rotation, cancel)
                    .map_err(|error| {
                        invalid(if error == RENDER_LIMIT {
                            "OCR page exceeds the 40-megapixel processing limit; crop or resize a working copy first"
                        } else {
                            error
                        })
                    })?;
                let geo = assembly::Geometry {
                    rotation: 0,
                    ..original_geometry
                };
                let (width, height) = geo.display_size();
                let native_rects = existing
                    .iter()
                    .filter(|span| !span.text.trim().is_empty())
                    .map(|span| {
                        super::text_edit::displayed(
                            geo,
                            super::text_edit::span_user_bbox(original_geometry, span),
                        )
                    })
                    .collect::<Vec<_>>();
                let filtered = retain_uncovered_words(results, &native_rects);
                if !filtered.is_empty() {
                    let mut overlay = layer(&filtered, (width, height)).map_err(invalid)?;
                    overlay.unrotated_page_coordinates = true;
                    overlays.insert(page, overlay);
                }
            }
            let percent = ((checked + 1) as f64 / selected_count as f64 * 95.0) as u8;
            progress(percent, &format!("OCR page {} of {count}", page + 1))?;
        }
        if cancelled() {
            return Err(refusal(409, "Operation cancelled"));
        }
        if rerun && overlays.is_empty() {
            return Err(invalid(
                "Rerun produced no OCR text; prior working revision retained",
            ));
        }
        // Once pages have gained text, only the unaffected pages can still
        // contain painted images without searchable text.
        let mut remaining = false;
        if !overlays.is_empty() {
            for page in (0..count).filter(|page| !overlays.contains_key(page)) {
                if cancelled() {
                    return Err(refusal(409, "Operation cancelled"));
                }
                let spans = native
                    .extract_spans(page)
                    .map_err(|_| invalid(OUTPUT_ERROR))?;
                if spans.iter().all(|span| span.text.trim().is_empty())
                    && super::image_uses::has_painted_image(&native, page).map_err(invalid)?
                {
                    remaining = true;
                    break;
                }
            }
        }
        drop(native);
        drop(base);
        let applied = overlays.len();
        let mut history = self.history;
        let mut provenance = Value::Null;
        let candidate = if applied > 0 {
            let mut pages = overlays.keys().copied().collect::<Vec<_>>();
            pages.sort_unstable();
            provenance = engine.provenance;
            provenance["pages_ocr_applied"] = json!(applied);
            provenance["applied_pages"] = json!(pages);
            provenance["base_revision"] = json!(self.base_revision);
            provenance["applied_revision"] = json!(self.revision + 1);
            provenance["requested_page_index"] = json!(self.selected);
            provenance["applied_at"] = json!(now());
            if rerun {
                provenance["rerun"] = json!(true);
            }
            history.push(provenance.clone());
            let bytes = assembly::append_document_overlays(
                source_bytes,
                &overlays,
                base_bytes.zip(self.selected),
            )
            .map_err(invalid)?;
            Some(ValidatedRevisionPdf::prepare(bytes, expected_pages)?)
        } else {
            None
        };
        let status = if applied == 0 {
            "no_new_text"
        } else if remaining {
            "needs_ocr"
        } else {
            "processed"
        };
        let result = if rerun {
            json!({"document_id":self.document,"page_index":self.selected,"provenance":provenance})
        } else {
            json!({"document_id":self.document,"pages_checked":selected_count,
                "pages_ocr_applied":applied,"language":self.language,"requested_page_index":self.selected,"provenance":provenance})
        };
        Ok(Product {
            document: self.document,
            revision: self.revision,
            candidate,
            files,
            retained,
            ocr: Some((status.to_owned(), json!(history).to_string())),
            result,
        })
    }
}

pub(super) fn layer(
    words: &[Word],
    (width, height): (f64, f64),
) -> Result<assembly::Overlay, &'static str> {
    let mut overlay = assembly::Overlay {
        ocr: true,
        ..Default::default()
    };
    let mut fonts = vec![HashMap::<char, u8>::new()];
    overlay.glyph_fonts.push(Vec::new());
    for word in words {
        let chars = word
            .text
            .chars()
            .filter(|char| !char.is_control())
            .collect::<Vec<_>>();
        if chars.is_empty() || chars.len() > 255 {
            continue;
        }
        let font = fonts.last().ok_or(OUTPUT_ERROR)?;
        let additional = chars
            .iter()
            .filter(|char| !font.contains_key(char))
            .copied()
            .collect::<HashSet<_>>()
            .len();
        if font.len() + additional > 255 {
            fonts.push(HashMap::new());
            overlay.glyph_fonts.push(Vec::new());
        }
        let index = fonts.len() - 1;
        let font = &mut fonts[index];
        let alphabet = &mut overlay.glyph_fonts[index];
        let mut encoded = String::new();
        for character in chars.iter().copied() {
            let code = *font.entry(character).or_insert_with(|| {
                alphabet.push(character);
                alphabet.len() as u8
            });
            write!(encoded, "{code:02x}").unwrap();
        }
        let [x, y, w, h] = word.rect;
        let size = (height * h).max(1.0);
        let scaling = (width * w / (size * 0.6 * chars.len() as f64) * 100.0).max(1.0);
        writeln!(overlay.commands,"BT /@O{index} {size:.5} Tf 3 Tr {scaling:.5} Tz 1 0 0 1 {:.5} {:.5} Tm <{encoded}> Tj ET",width*x,height*(1.0-y-h)).unwrap();
    }
    Ok(overlay)
}

impl Store {
    pub(super) fn ocr_review_request(
        &self,
        method: &str,
        route: &[&str],
        query: &str,
        body: &Value,
    ) -> Option<Reply> {
        match (method, route) {
            ("GET", ["documents", id, "ocr-review"]) => Some(
                self.prepare_ocr_review(id, query)
                    .and_then(OcrReviewWork::compute)
                    .and_then(|result| self.complete_ocr_review(result))
                    .unwrap_or_else(|error| error),
            ),
            ("POST", ["documents", id, "ocr-review", "accept"]) => {
                Some(self.accept_ocr(id, body).unwrap_or_else(|error| error))
            }
            _ => None,
        }
    }

    pub fn prepare_ocr_review(&self, id: &str, query: &str) -> Result<OcrReviewWork, Reply> {
        let doc = self.artifact_document(id)?.body;
        let expected = reading::lookup(query, "revision")
            .ok_or_else(|| reading::missing_query("revision"))?
            .parse::<u64>()
            .ok()
            .filter(|revision| *revision > 0)
            .ok_or_else(|| refusal(422, "Invalid working revision"))?;
        if expected != revision(&doc) {
            return Err(refusal(409, "Working revision changed; reopen OCR review"));
        }
        let page = reading::lookup(query, "page_index")
            .ok_or_else(|| reading::missing_query("page_index"))?
            .parse::<usize>()
            .map_err(|_| refusal(422, "Invalid OCR page index"))?;
        if doc["ocr_status"] != "processed" || Some(page as u64) >= doc["page_count"].as_u64() {
            return Err(refusal(422, "This page is not ready for OCR review"));
        }
        let source = self.pdf_source(
            id,
            &Artifact::Working {
                revision: Some(expected),
            },
        )?;
        Ok(OcrReviewWork {
            source,
            root: self.root.clone(),
            document_id: id.to_owned(),
            revision: expected,
            page,
            language: review_language(&doc),
        })
    }

    pub fn complete_ocr_review(&self, measured: OcrReviewMeasurement) -> Result<Reply, Reply> {
        let OcrReviewMeasurement {
            root,
            document_id,
            revision: expected,
            page,
            language,
            source,
            mut result,
        } = measured;
        if self.root != root {
            return Err(refusal(409, "Working revision changed; reopen OCR review"));
        }
        let doc = self.artifact_document(&document_id)?.body;
        if expected != revision(&doc) || language != review_language(&doc) {
            return Err(refusal(409, "Working revision changed; reopen OCR review"));
        }
        if doc["ocr_status"] != "processed" || Some(page as u64) >= doc["page_count"].as_u64() {
            return Err(refusal(422, "This page is not ready for OCR review"));
        }
        let current = self.pdf_source(
            &document_id,
            &Artifact::Working {
                revision: Some(expected),
            },
        )?;
        if !source.matches(&current) {
            return Err(refusal(409, "Working revision changed; reopen OCR review"));
        }
        source.check_current()?;
        let accepted: Option<String> = self
            .con
            .query_row(
                "SELECT accepted_at FROM rust_ocr_reviews WHERE document_id=? AND revision=? AND page=?",
                params![document_id, expected, page],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| refusal(500, "Unable to load OCR review"))?;
        result["document_id"] = json!(document_id);
        result["revision"] = json!(expected);
        result["page_index"] = json!(page);
        result["language"] = json!(language);
        result["accepted_at"] = json!(accepted);
        result["can_rerun"] = json!(can_rerun(&doc, page));
        Ok(reply(200, result))
    }

    fn accept_ocr(&self, id: &str, body: &Value) -> Result<Reply, Reply> {
        let doc = self.artifact_document(id)?.body;
        let expected = body["expected_revision"]
            .as_u64()
            .filter(|revision| *revision > 0)
            .ok_or_else(|| refusal(422, "Invalid working revision"))?;
        let page = body["page_index"]
            .as_u64()
            .ok_or_else(|| refusal(422, "Invalid OCR page index"))?;
        if expected != revision(&doc) {
            return Err(refusal(409, "Working revision changed; reopen OCR review"));
        }
        if doc["ocr_status"] != "processed"
            || doc["page_count"].as_u64().is_none_or(|count| page >= count)
        {
            return Err(refusal(409, "This page is not ready for OCR review"));
        }
        let accepted = now();
        self.con
            .execute(
                "INSERT INTO rust_ocr_reviews VALUES (?,?,?,?)
                 ON CONFLICT(document_id,revision,page) DO UPDATE SET accepted_at=excluded.accepted_at",
                params![id, expected, page, accepted],
            )
            .map_err(|_| refusal(500, "Unable to save OCR review"))?;
        Ok(reply(
            200,
            json!({"document_id":id,"revision":expected,"page_index":page,"accepted_at":accepted}),
        ))
    }
}

fn review_language(doc: &Value) -> String {
    runs(doc)
        .last()
        .and_then(|run| run["language"].as_str())
        .unwrap_or("eng")
        .to_owned()
}

/// An OCR inspection of a specific immutable revision, independent of the store lane.
pub struct OcrReviewWork {
    source: PdfSource,
    root: PathBuf,
    document_id: String,
    revision: u64,
    page: usize,
    language: String,
}

pub struct OcrReviewMeasurement {
    root: PathBuf,
    document_id: String,
    revision: u64,
    page: usize,
    language: String,
    source: PdfProof,
    result: Value,
}

impl OcrReviewWork {
    pub fn compute(self) -> Result<OcrReviewMeasurement, Reply> {
        let (bytes, source) = self.source.read_with_proof(|| false)?;
        let pdf = PdfDocument::from_bytes(bytes).map_err(|_| refusal(422, OUTPUT_ERROR))?;
        let result = measure(&pdf, self.page, &self.language)?;
        Ok(OcrReviewMeasurement {
            root: self.root,
            document_id: self.document_id,
            revision: self.revision,
            page: self.page,
            language: self.language,
            source,
            result,
        })
    }
}

pub(super) struct Screening {
    snapshot: String,
    sources: Vec<ScreeningDocument>,
    marked: HashSet<(String, usize)>,
}

pub(super) struct ScreeningDocument {
    doc: Value,
    pub(super) original: PdfSource,
    pub(super) working: PdfSource,
}

pub(super) fn screening_documents(
    store: &Store,
    input: &Inputs,
) -> Result<Vec<ScreeningDocument>, Reply> {
    let (_, documents, _) = input.generation_inputs();
    documents
        .into_iter()
        .map(|doc| {
            let id = doc["id"]
                .as_str()
                .ok_or_else(|| refusal(409, "Invalid document identity"))?;
            let (original, working) = store.screening_sources(id, revision(doc))?;
            Ok(ScreeningDocument {
                doc: doc.clone(),
                original,
                working,
            })
        })
        .collect()
}

pub(super) struct ScreeningProduct {
    pub(super) report: Value,
    pub(super) result: Value,
    pub(super) files: Vec<PdfProof>,
}

impl Screening {
    pub(super) fn prepare(store: &Store, matter: &str, snapshot: &str) -> Result<Self, Reply> {
        let db = |_| refusal(500, "Unable to load saved screening inputs");
        let input = Inputs::metadata(&store.con, matter).map_err(db)??;
        if input.effective_digest(&store.con).map_err(db)? != snapshot {
            return Err(refusal(
                409,
                "Saved inputs changed before page screening. Run checks and start again.",
            ));
        }
        let sources = screening_documents(store, &input)?;
        if sources.is_empty() {
            return Err(refusal(422, "Include documents before screening pages"));
        }
        let marked = super::annotations::list(&store.con, matter)
            .map_err(db)?
            .into_iter()
            .filter(|mark| mark["resolved"] == true && mark["output_scope"] == "pdf")
            .filter_map(|mark| {
                Some((
                    mark["document_id"].as_str()?.to_owned(),
                    mark["page_index"].as_u64()? as usize,
                ))
            })
            .collect();
        Ok(Self {
            snapshot: snapshot.to_owned(),
            sources,
            marked,
        })
    }

    pub(super) fn run(
        self,
        cancel: &AtomicBool,
        progress: impl Fn(u8, &str) -> Result<(), Reply>,
    ) -> Result<ScreeningProduct, Reply> {
        let total = self
            .sources
            .iter()
            .filter_map(|source| source.doc["page_count"].as_u64())
            .sum::<u64>();
        let mut pages = Vec::new();
        let mut engines = HashMap::new();
        let mut files = Vec::new();
        let cancelled = || cancel.load(Ordering::Acquire);
        for ScreeningDocument {
            doc,
            original,
            working,
        } in self.sources
        {
            let invalid = |_| refusal(422, "Unable to screen the current working PDF");
            files.push(original.authenticate(cancelled)?);
            let (bytes, working) = working.read_with_proof(cancelled)?;
            files.push(working);
            let pdf = PdfDocument::from_bytes(bytes).map_err(invalid)?;
            let count = pdf.page_count().map_err(invalid)?;
            if doc["page_count"] != count {
                return Err(refusal(
                    422,
                    "Working page count changed; repair the document before screening",
                ));
            }
            let id = doc["id"].as_str().unwrap_or_default();
            let lang = review_language(&doc);
            for index in 0..count {
                progress(
                    (95.0 * pages.len() as f64 / total.max(1) as f64).round_ties_even() as u8,
                    &format!("Screening page {} of {total}", pages.len() + 1),
                )?;
                let mut row = json!({"document_id":id,"target_revision":doc["revision"],
                    "page_index":index,"language":lang,"ocr_outcome":"unavailable"});
                if let (Ok(image), Ok(spans), Ok(images)) = (
                    render(&pdf, index, true),
                    pdf.extract_spans(index),
                    super::image_uses::resources(&pdf, index).map(|images| images.has_images),
                ) {
                    let pixels = image.data.chunks_exact(4);
                    let dark = pixels
                        .clone()
                        .filter(|pixel| {
                            pixel[3] > 10
                                && (u16::from(pixel[0]) * 30
                                    + u16::from(pixel[1]) * 59
                                    + u16::from(pixel[2]) * 11)
                                    < 24_500
                        })
                        .count();
                    let ink = dark as f64 / pixels.len().max(1) as f64;
                    let text = spans.iter().any(|span| !span.text.trim().is_empty());
                    let blank =
                        !text && ink <= 0.0001 && !self.marked.contains(&(id.to_owned(), index));
                    row["blank_candidate"] = json!(blank);
                    row["ink_fraction"] = json!(rounded(ink, 7));
                    row["has_text"] = json!(text);
                    row["has_images"] = json!(images);
                    row["render_width"] = json!(image.width);
                    row["render_height"] = json!(image.height);
                    row["ocr_outcome"] = if blank {
                        json!("not_applicable_blank_candidate")
                    } else if !images
                        && !matches!(
                            doc["ocr_status"].as_str(),
                            Some("processed" | "needs_ocr" | "no_new_text")
                        )
                    {
                        json!("not_applicable_native_text")
                    } else {
                        let engine = engines
                            .entry(lang.clone())
                            .or_insert_with(|| Engine::for_measurement(&lang));
                        if let Ok(engine) = engine {
                            if let Ok(words) = visible_words(engine, &pdf, index, cancel) {
                                let (mean, low) = confidence(&words);
                                row["confidence"] = json!(mean);
                                row["low_region_count"] = json!(low.len());
                                json!(match mean {
                                    None => "no_words",
                                    Some(mean) if mean < 75.0 || !low.is_empty() =>
                                        "low_confidence",
                                    _ => "measured",
                                })
                            } else {
                                json!("unavailable")
                            }
                        } else {
                            if matches!(engine, Err(super::tesseract::UNAVAILABLE)) {
                                row["confidence"] = Value::Null;
                                row["low_region_count"] = json!(0);
                            }
                            json!("unavailable")
                        }
                    };
                }
                pages.push(row);
            }
        }
        let providers = engines
            .into_iter()
            .map(|(language, engine)| {
                let provider = engine.map_or_else(
                    |_| json!({"language":language,"provider":"unavailable"}),
                    |engine| engine.provenance,
                );
                (language, provider)
            })
            .collect::<serde_json::Map<_, _>>();
        let result = json!({"pages_screened":pages.len(),"input_digest":self.snapshot,
            "blank_candidates":pages.iter().filter(|row|row["blank_candidate"]==true).count(),
            "ocr_attention":pages.iter().filter(|row|matches!(row["ocr_outcome"].as_str(),Some("unavailable" | "no_words" | "low_confidence"))).count()});
        let report = json!({"method":"page-screening-v1","input_digest":self.snapshot,"created_at":now(),"pages":pages,
            "providers":providers,"threshold":75,"blank_ink_fraction_threshold":0.0001});
        Ok(ScreeningProduct {
            report,
            result,
            files,
        })
    }
}

pub(super) fn findings(
    con: &Connection,
    matter: &str,
    digest: Option<&str>,
) -> rusqlite::Result<Vec<Value>> {
    let report = con
        .query_row(
            "SELECT report FROM rust_page_screenings WHERE matter_id=? AND digest=?",
            params![matter, digest],
            |row| decode(&row.get::<_, String>(0)?),
        )
        .optional()?
        .filter(|report: &Value| {
            report["method"] == "page-screening-v1" && report["input_digest"].as_str() == digest
        });
    let Some(report) = report else {
        return Ok(vec![
            json!({"severity":"info","code":"PAGE_SCREENING_NOT_RUN","document_id":null,
            "message":"Blank-page and batch OCR confidence screening has not run for these saved inputs. Use Screen pages to measure candidates; this is separate from legal review."}),
        ]);
    };
    let rows = report["pages"]
        .as_array()
        .ok_or(rusqlite::Error::InvalidQuery)?;
    let mut findings = Vec::new();
    let mut measured = 0;
    for row in rows {
        let number = row["page_index"].as_u64().unwrap_or(0) + 1;
        let mut at = |code: &str, message: String| {
            findings.push(json!({"code":code,"message":message,"severity":"warning","artifact":"working",
                "document_id":row["document_id"],"target_revision":row["target_revision"],"page_index":row["page_index"]}));
        };
        if row["blank_candidate"] == true {
            at("POSSIBLE_BLANK_PAGE",format!("Source page {number} appears nearly blank in the rendered crop. Faint marks, signatures or cropped content may still exist. Review before deciding to keep or remove it; nothing was deleted."));
        }
        let outcome = row["ocr_outcome"].as_str();
        if matches!(outcome, Some("measured" | "low_confidence" | "no_words")) {
            measured += 1;
        }
        match outcome {
            Some("low_confidence") => {
                let (confidence, count) = (row["confidence"].as_f64(), row["low_region_count"].as_u64());
                let threshold = report["threshold"].as_u64().unwrap_or(75);
                let message = match (confidence, count) {
                    (Some(confidence), Some(count)) => format!("Source page {number}: separate Tesseract pass measured {confidence:.1}% mean word confidence with {count} flagged regions (threshold {threshold}%). This is not text accuracy; compare the embedded text with the scan."),
                    _ => format!("Source page {number} has low measured OCR confidence. Compare embedded text with the scan."),
                };
                at("LOW_OCR_CONFIDENCE", message);
            }
            Some("no_words") => at("OCR_NO_WORDS",format!("Source page {number}: OCR screening recognized no words. Inspect readability and language; this does not prove the page is blank.")),
            Some("unavailable") => at("PAGE_SCREENING_UNAVAILABLE",format!("Source page {number}: rendering or OCR confidence could not be measured. Inspect the source and local OCR availability, then retry screening.")),
            _ => {}
        }
    }
    findings.push(json!({"severity":"info","code":"PAGE_SCREENING_COMPLETED","document_id":null,
        "message":format!("Screened {} source pages; OCR measurements available for {measured}. Native-text and possible blank pages skip OCR. Results are review aids, not verification of text, signatures or filing completeness.",rows.len())}));
    Ok(findings)
}

#[cfg(test)]
mod tests {
    use super::{
        confidence, findings, migrate, retain_uncovered_words, rounded, OcrReviewMeasurement, Word,
    };
    use crate::matters::{Store, PILOT_PDF_BYTES};
    use rusqlite::{params, Connection};
    use serde_json::json;

    #[test]
    fn confidence_uses_decimal_rounding_at_the_review_threshold() {
        assert_eq!(rounded(74.95, 1), 75.0);
        assert_eq!(rounded(75.05, 1), 75.0);
        assert_eq!(rounded(86.25, 1), 86.2);
        let scans = [74.75, 75.05].map(|score| Word {
            text: "Synthetic".into(),
            confidence: score,
            rect: [0.1, 0.1, 0.5, 0.1],
        });
        let (mean, low) = confidence(&scans);
        assert_eq!(mean, Some(74.9));
        assert_eq!(low.len(), 1);
        assert_eq!(low[0].1, 74.8);
    }

    #[test]
    fn review_completion_checks_revision_and_reads_current_acceptance() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::open(&root.path().join("matter")).unwrap();
        let matter = store.request(
            "POST",
            "/matters",
            json!({ "name": "Synthetic OCR review" }),
        );
        let matter = matter.body["id"].as_str().unwrap();
        let document = store.import_pdf(matter, "review.pdf", PILOT_PDF_BYTES.to_vec());
        assert_eq!(document.status, 201);
        let id = document.body[0]["id"].as_str().unwrap();
        let mut document = store.artifact_document(id).unwrap().body;
        document["ocr_status"] = json!("processed");
        store
            .con
            .execute(
                "UPDATE documents SET body=? WHERE id=?",
                params![document.to_string(), id],
            )
            .unwrap();
        let measured = || {
            let work = store
                .prepare_ocr_review(id, "page_index=0&revision=1")
                .ok()
                .unwrap();
            OcrReviewMeasurement {
                root: work.root,
                document_id: work.document_id,
                revision: work.revision,
                page: work.page,
                language: work.language,
                source: work.source.authenticate(|| false).unwrap(),
                result: json!({"available":true,"confidence":90.0,"regions":[]}),
            }
        };
        let pending = measured();
        let accepted = store
            .accept_ocr(id, &json!({"expected_revision":1,"page_index":0}))
            .unwrap();
        let reviewed = store.complete_ocr_review(pending).unwrap();
        assert_eq!(reviewed.body["accepted_at"], accepted.body["accepted_at"]);
        assert_eq!(reviewed.body["revision"], 1);

        let changed_file = measured();
        let working = store
            .root
            .join("documents-v1")
            .join(id)
            .join("revision-1.pdf");
        std::fs::write(&working, b"Changed synthetic working bytes").unwrap();
        assert_eq!(
            store.complete_ocr_review(changed_file).unwrap_err().status,
            409
        );
        std::fs::write(&working, PILOT_PDF_BYTES).unwrap();

        let stale = measured();
        document["revision"] = json!(2);
        store
            .con
            .execute(
                "UPDATE documents SET body=? WHERE id=?",
                params![document.to_string(), id],
            )
            .unwrap();
        assert_eq!(store.complete_ocr_review(stale).unwrap_err().status, 409);
    }

    #[test]
    fn ocr_overlap_keeps_disjoint_fragments_and_removes_whole_graphemes() {
        let words = vec![Word {
            text: "A\u{0301}BCD".into(),
            confidence: 95.0,
            rect: [0.0, 0.1, 0.5, 0.1],
        }];
        let native = [[0.125, 0.1, 0.15, 0.2], [0.32, 0.1, 0.35, 0.2]];
        let fragments = retain_uncovered_words(words, &native);
        assert_eq!(fragments.len(), 2);
        for (word, text, left) in [(&fragments[0], "B", 0.2), (&fragments[1], "D", 0.4)] {
            assert_eq!(word.text, text);
            assert_eq!(word.confidence, 95.0);
            for (actual, expected) in word.rect.into_iter().zip([left, 0.1, 0.1, 0.1]) {
                assert!((actual - expected).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn screening_exposes_current_measurement_and_review_scope() {
        let con = Connection::open_in_memory().unwrap();
        migrate(&con).unwrap();
        let report = json!({"method":"page-screening-v1", "input_digest":"current", "threshold":75, "pages":[
            {"document_id":"fixture", "target_revision":3, "page_index":0, "ocr_outcome":"low_confidence", "confidence":58.5, "low_region_count":2},
            {"document_id":"fixture", "target_revision":3, "page_index":1, "ocr_outcome":"no_words"},
            {"document_id":"fixture", "target_revision":3, "page_index":2, "blank_candidate":true, "ocr_outcome":"not_applicable_blank_candidate"}
        ]});
        con.execute(
            "INSERT INTO rust_page_screenings VALUES ('fixture', 'current', ?)",
            [report.to_string()],
        )
        .unwrap();
        let current = findings(&con, "fixture", Some("current")).unwrap();
        assert_eq!(current[0]["target_revision"], 3);
        assert_eq!(current[0]["page_index"], 0);
        assert!(current[0]["message"]
            .as_str()
            .unwrap()
            .contains("58.5% mean word confidence with 2 flagged regions"));
        assert_eq!(current[1]["code"], "OCR_NO_WORDS");
        assert_eq!(current[2]["code"], "POSSIBLE_BLANK_PAGE");
        assert!(current[3]["message"]
            .as_str()
            .unwrap()
            .contains("OCR measurements available for 2"));
        assert_eq!(
            findings(&con, "fixture", Some("changed")).unwrap()[0]["code"],
            "PAGE_SCREENING_NOT_RUN"
        );
    }
}
