use super::{artifacts::refusal, reply, Reply};
use pdf_oxide::PdfDocument;
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub(super) mod assembly;
pub(super) mod pdf_annotations;
pub(super) mod request;

const ROTATION_ERROR: &str = "Rotation must be 90, 180 or 270 degrees clockwise";
const PAGE_SELECTION_ERROR: &str = "Select existing pages using zero-based page indexes";

pub(super) struct PageRequest {
    pub(super) expected_revision: Option<u64>,
    pub(super) expected_source_revision: Option<u64>,
    pub(super) acknowledge_signature_risk: bool,
}

pub(super) struct SourceEdit {
    pub(super) document_id: String,
    selection: Selection,
}

enum Selection {
    Insert { pages: Vec<i64>, before: usize },
    Replace { source: Vec<i64>, target: Vec<i64> },
    Whole,
}

pub(super) enum PageEdit {
    Rotate(usize, i32),
    Delete(usize),
    Reorder(Vec<i64>),
    Source(SourceEdit),
    BatchRotate(Vec<i64>, i32),
    BatchDelete(Vec<i64>),
    Crop(Vec<i64>, [f64; 4]),
    TextOverlay(usize, f64, f64, String, f64),
    ImageOverlay(i64, Value),
}

impl PageEdit {
    pub(super) fn modifies_ocr_status(&self) -> bool {
        matches!(
            self,
            Self::BatchRotate(..)
                | Self::BatchDelete(..)
                | Self::Crop(..)
                | Self::Source(..)
                | Self::Reorder(..)
                | Self::TextOverlay(..)
                | Self::ImageOverlay(..)
        )
    }

    pub(super) fn source(&self) -> Option<&str> {
        match self {
            Self::Source(source) => Some(&source.document_id),
            _ => None,
        }
    }

    pub(super) fn apply(
        self,
        bytes: Vec<u8>,
        count: usize,
        related: Option<(Vec<u8>, usize)>,
        target_id: &str,
        reserved: &[Value],
        image: Option<super::annotations::ImageUpload>,
    ) -> Result<(Vec<u8>, usize), Reply> {
        let invalid = |detail| refusal(422, detail);
        let mut remove = Vec::new();
        let mut rotate = None;
        let mut crop = None;
        match self {
            Self::TextOverlay(page, x, y, text, size) => {
                if text.trim().is_empty()
                    || !text
                        .bytes()
                        .all(|byte| byte == b'\n' || (32..=126).contains(&byte))
                {
                    return Err(invalid(
                        "Text overlay supports printable ASCII and line breaks only",
                    ));
                }
                if page >= count {
                    return Err(invalid("Invalid page number"));
                }
                return super::document_output::overlay_working_text(
                    bytes, page, x, y, &text, size, reserved,
                )
                .map(|bytes| (bytes, count))
                .map_err(invalid);
            }
            Self::ImageOverlay(page, body) => {
                let image = image.ok_or_else(|| invalid("Image file is required"))?;
                return super::document_output::overlay_working_image(
                    bytes, page, count, &body, image,
                )
                .map(|bytes| (bytes, count));
            }
            Self::Reorder(pages) => {
                let valid =
                    pages.len() == count && pages.iter().collect::<BTreeSet<_>>().len() == count;
                let pages = check_pages(&pages, count, false)
                    .ok()
                    .filter(|_| valid)
                    .ok_or_else(|| {
                        invalid("Page order must be a permutation of every page index")
                    })?;
                let selected = pages.into_iter().map(|page| (0, page)).collect::<Vec<_>>();
                return assembly::assemble(bytes, None, &selected)
                    .map(|result| (result, count))
                    .map_err(invalid);
            }
            Self::Source(source) => {
                let (source_bytes, source_count) =
                    related.ok_or_else(|| invalid("Source PDF is unavailable"))?;
                let selected = match source.selection {
                    Selection::Insert { pages, before } => {
                        let pages = check_pages(&pages, source_count, false).map_err(invalid)?;
                        if before > count {
                            return Err(invalid("Invalid insertion position"));
                        }
                        (0..before)
                            .map(|page| (0, page))
                            .chain(pages.into_iter().map(|page| (1, page)))
                            .chain((before..count).map(|page| (0, page)))
                            .collect::<Vec<_>>()
                    }
                    Selection::Replace { source, target } => {
                        let target = check_pages(&target, count, false).map_err(invalid)?;
                        let source = check_pages(&source, source_count, false).map_err(invalid)?;
                        if target.len() != source.len()
                            || target.iter().collect::<BTreeSet<_>>().len() != target.len()
                        {
                            return Err(invalid(
                                "Replacement needs one source page for each distinct target page",
                            ));
                        }
                        let replacements = target
                            .into_iter()
                            .zip(source)
                            .collect::<std::collections::BTreeMap<_, _>>();
                        (0..count)
                            .map(|page| {
                                replacements
                                    .get(&page)
                                    .map_or((0, page), |&source| (1, source))
                            })
                            .collect()
                    }
                    Selection::Whole => {
                        if source.document_id == target_id {
                            return Err(invalid(
                                "Choose a different document as the replacement source",
                            ));
                        }
                        (0..source_count).map(|page| (1, page)).collect()
                    }
                };
                return assembly::assemble(bytes, Some(source_bytes), &selected)
                    .map(|result| (result, selected.len()))
                    .map_err(invalid);
            }
            Self::Rotate(page, degrees) => {
                if ![90, 180, 270].contains(&degrees) {
                    return Err(invalid(ROTATION_ERROR));
                }
                if page >= count {
                    return Err(invalid("Invalid page number"));
                }
                rotate = Some((vec![page], degrees));
            }
            Self::Delete(page) => {
                if page >= count {
                    return Err(invalid("Invalid page number"));
                }
                if count == 1 {
                    return Err(invalid("Cannot delete the only remaining page"));
                }
                remove.push(page);
            }
            Self::BatchRotate(pages, degrees) => {
                rotate = Some((check_pages(&pages, count, true).map_err(invalid)?, degrees));
            }
            Self::BatchDelete(pages) => {
                remove = check_pages(&pages, count, true).map_err(invalid)?;
                if remove.len() == count {
                    return Err(invalid("Keep at least one page in the working PDF"));
                }
            }
            Self::Crop(pages, [left, top, right, bottom]) => {
                if left >= right || top >= bottom {
                    return Err(invalid(
                        "Crop edges must be ordered fractions between 0 and 1",
                    ));
                }
                let pages = check_pages(&pages, count, false)
                    .map_err(invalid)?
                    .into_iter()
                    .collect::<BTreeSet<_>>();
                crop = Some((pages, [left, top, right, bottom]));
            }
        }
        if !remove.is_empty() {
            let remove: BTreeSet<_> = remove.into_iter().collect();
            return assembly::delete_pages(bytes, &remove)
                .map(|result| (result, count - remove.len()))
                .map_err(invalid);
        }
        let pdf =
            PdfDocument::from_bytes(bytes).map_err(|_| invalid("Unable to open working PDF"))?;
        let mut rotations = Vec::new();
        if let Some((pages, degrees)) = rotate {
            for page in pages {
                let original = assembly::geometry(&pdf, page)
                    .map_err(|_| invalid("Unable to rotate working PDF"))?
                    .rotation;
                rotations.push((page, (original + degrees).rem_euclid(360)));
            }
        }
        let mut crop_boxes = Vec::new();
        if let Some((pages, [left, top, right, bottom])) = crop {
            for page in pages {
                let geo = assembly::geometry(&pdf, page)
                    .map_err(|_| invalid("Unable to read page crop box"))?;
                let [x0, y0, x1, y1] = geo.crop;
                // Fractions start at the top of the stored, unrotated CropBox.
                let (width, height) = (x1 - x0, y1 - y0);
                let [l, b, r, t] = [
                    x0 + width * left,
                    y1 - height * bottom,
                    x0 + width * right,
                    y1 - height * top,
                ];
                if l < geo.media[0] || b < geo.media[1] || r > geo.media[2] || t > geo.media[3] {
                    return Err(invalid("CropBox not in MediaBox"));
                }
                let rect = [l, b, r, t].map(|value| value as f32);
                if rect[0] >= rect[2] || rect[1] >= rect[3] {
                    return Err(invalid("CropBox is empty"));
                }
                crop_boxes.push((page, rect));
            }
        }
        let bytes = assembly::set_page_geometry(pdf, rotations, crop_boxes)
            .map_err(|_| invalid("Unable to write working PDF"))?;
        Ok((bytes, count))
    }
}

fn check_pages(indices: &[i64], count: usize, batch: bool) -> Result<Vec<usize>, &'static str> {
    indices
        .iter()
        .map(|&index| usize::try_from(index).ok().filter(|&index| index < count))
        .collect::<Option<Vec<_>>>()
        .ok_or(if batch {
            "Select unique existing pages using zero-based page indexes"
        } else {
            PAGE_SELECTION_ERROR
        })
}

#[derive(Clone, Copy)]
pub enum PageOutput {
    Extract,
    Split,
}

pub struct PageOutputTarget {
    pub document: String,
    pub output: PageOutput,
    pub selection: String,
    pub acknowledged: bool,
}

impl PageOutputTarget {
    pub fn from_path(method: &str, path: &str, query: &str) -> Option<Result<Self, Reply>> {
        let route = path.trim_start_matches('/').split('/').collect::<Vec<_>>();
        Self::select(method, &route, query)
    }

    pub(super) fn select(method: &str, route: &[&str], query: &str) -> Option<Result<Self, Reply>> {
        use super::{preparation::validation::issue, reading::lookup};
        let (document, output, parameter) = match (method, route) {
            ("GET", ["documents", id, "pages", "extract"]) => {
                (id, PageOutput::Extract, "selection")
            }
            ("GET", ["documents", id, "pages", "split"]) => (id, PageOutput::Split, "after"),
            _ => return None,
        };
        let selection = lookup(query, parameter);
        let mut errors = Vec::new();
        if selection.is_none() {
            errors.push(issue(
                "missing",
                json!(["query", parameter]),
                "Field required",
                Value::Null,
            ));
        }
        let acknowledged = lookup(query, "acknowledge_signature_risk").is_some_and(|value| {
            let input = json!(value);
            match super::boolean(&input) {
                Some(acknowledged) => acknowledged,
                None => {
                    errors.push(issue(
                        "bool_parsing",
                        json!(["query", "acknowledge_signature_risk"]),
                        "Input should be a valid boolean, unable to interpret input",
                        input,
                    ));
                    false
                }
            }
        });
        Some(if errors.is_empty() {
            Ok(Self {
                document: (*document).to_owned(),
                output,
                selection: selection.expect("validated selection").into_owned(),
                acknowledged,
            })
        } else {
            Err(reply(422, json!({"detail": errors})))
        })
    }

    pub fn into_reply(output: PageOutput, bytes: Vec<u8>) -> Reply {
        let mime = match output {
            PageOutput::Extract => "application/pdf",
            PageOutput::Split => "application/zip",
        };
        super::document_output::binary(bytes, mime)
    }
}

pub struct PageOutputWork {
    source: super::artifacts::PdfSource,
    output: PageOutput,
    selection: Vec<i64>,
    count: usize,
}

pub(super) fn signature_guard<'a>(
    documents: impl IntoIterator<Item = &'a Value>,
    acknowledged: bool,
    action: &str,
) -> Result<(), Reply> {
    if acknowledged {
        return Ok(());
    }
    let risks = documents
        .into_iter()
        .filter(|doc| {
            matches!(
                doc["signature_status"].as_str(),
                Some(
                    "certificate_signature_detected"
                        | "inspection_inconclusive"
                        | "visible_signature_only"
                )
            )
        })
        .filter_map(|doc| doc["title"].as_str())
        .collect::<Vec<_>>();
    if risks.is_empty() {
        return Ok(());
    }
    let title = risks.join(", ");
    Err(super::artifacts::refusal(409, format!("Signature-risk acknowledgment required before {action}: {title}. Certificate indicators are not proof of validity, and transforming a working copy or generated bundle may invalidate or omit signature protections. Imported originals will remain unchanged.")))
}

impl super::Store {
    /// Prepare an independent PDF or ZIP without publishing a new document revision.
    pub fn page_output(
        &self,
        id: &str,
        output: PageOutput,
        selection: &str,
        acknowledged: bool,
    ) -> Result<Vec<u8>, Reply> {
        self.prepare_page_output(id, output, selection, acknowledged)?
            .compute()
    }

    pub fn prepare_page_output(
        &self,
        id: &str,
        output: PageOutput,
        selection: &str,
        acknowledged: bool,
    ) -> Result<PageOutputWork, Reply> {
        use super::{artifacts::refusal, revisions::active_document, Artifact};
        let unavailable = || refusal(500, "Unable to read the working PDF");
        let doc = active_document(&self.con, id)
            .map_err(|_| unavailable())?
            .ok_or_else(|| refusal(404, "Document not found"))?;
        let action = match output {
            PageOutput::Extract => "creating an extracted PDF",
            PageOutput::Split => "creating split PDFs",
        };
        signature_guard([&doc], acknowledged, action)?;
        let selection = parse_selection(selection)?;
        let count = doc["page_count"].as_u64().ok_or_else(unavailable)? as usize;
        let source = self.pdf_source(
            id,
            &Artifact::Working {
                revision: doc["revision"].as_u64(),
            },
        )?;
        Ok(PageOutputWork {
            source,
            output,
            selection,
            count,
        })
    }
}

impl PageOutputWork {
    pub fn compute(self) -> Result<Vec<u8>, Reply> {
        use super::artifacts::refusal;
        let Self {
            source,
            output,
            selection,
            count,
        } = self;
        let source = source
            .open()?
            .into_bytes()
            .map_err(|_| refusal(500, "Unable to read the working PDF"))?;
        match output {
            PageOutput::Extract => {
                if selection.len() > 2000 {
                    return Err(refusal(422, PAGE_SELECTION_ERROR));
                }
                let pages = check_pages(&selection, count, false)
                    .map_err(|message| refusal(422, message))?;
                let selected = pages.into_iter().map(|page| (0, page)).collect::<Vec<_>>();
                assembly::assemble(source, None, &selected).map_err(|message| refusal(422, message))
            }
            PageOutput::Split => {
                split(source, &selection, count).map_err(|message| refusal(422, message))
            }
        }
    }
}

fn parse_selection(input: &str) -> Result<Vec<i64>, Reply> {
    let error = || super::artifacts::refusal(422, "Use comma-separated zero-based page indexes");
    if input.is_empty() || input.chars().count() > 10_000 {
        return Err(error());
    }
    input
        .split(',')
        .map(|piece| {
            if piece.is_empty() {
                return Err(error());
            }
            // A decimal beyond the range of stored indexes cannot select a page.
            piece.chars().try_fold(0i64, |index, ch| {
                let digit = super::reading::decimal_digit(ch).ok_or_else(error)?;
                Ok(index.saturating_mul(10).saturating_add(i64::from(digit)))
            })
        })
        .collect()
}

fn split(source: Vec<u8>, selection: &[i64], count: usize) -> Result<Vec<u8>, &'static str> {
    use std::io::{Cursor, Write};
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
    const SPLIT_ERROR: &str = "Split points must be distinct ascending pages before the last page";
    const ZIP_ERROR: &str = "Unable to create split ZIP";
    const MAX_SPLIT_OUTPUT: usize = 160 * 1024 * 1024;
    if selection.len() > 1999 || selection.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(SPLIT_ERROR);
    }
    let cuts = selection
        .iter()
        .map(|&end| {
            usize::try_from(end)
                .ok()
                .and_then(|end| end.checked_add(1))
                .filter(|&end| end < count)
        })
        .collect::<Option<Vec<_>>>()
        .ok_or(SPLIT_ERROR)?;
    let source = assembly::Source::open(source)?;
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    let mut start = 0;
    let mut size = 0usize;
    for (part, end) in cuts.into_iter().chain([count]).enumerate() {
        let selected = (start..end).map(|page| (0, page)).collect::<Vec<_>>();
        let pdf = assembly::assemble_sources(std::slice::from_ref(&source), &selected)?;
        size = size.saturating_add(pdf.len());
        if size > MAX_SPLIT_OUTPUT {
            return Err("Split output exceeds the size limit");
        }
        archive
            .start_file(
                format!("part-{:03}.pdf", part + 1),
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
            )
            .map_err(|_| ZIP_ERROR)?;
        archive.write_all(&pdf).map_err(|_| ZIP_ERROR)?;
        start = end;
    }
    Ok(archive.finish().map_err(|_| ZIP_ERROR)?.into_inner())
}
