use super::{
    super::page_tools::pdf_annotations::{annotations, field, has_link},
    is_ocr_font, OUTPUT_ERROR,
};
use pdf_oxide::{object::Object, PdfDocument};

fn has_annotation(pdf: &PdfDocument, entries: &[Object]) -> Result<bool, &'static str> {
    for entry in entries {
        if entry.as_dict().is_some()
            && !matches!(
                field(pdf, entry, &["Subtype"])
                    .map_err(|_| OUTPUT_ERROR)?
                    .as_ref()
                    .and_then(Object::as_name),
                Some("Link" | "Popup" | "Widget")
            )
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn check(
    current: &PdfDocument,
    retained: &PdfDocument,
    page: usize,
    count: usize,
) -> Result<(), &'static str> {
    if retained.page_count().ok() != Some(count) || page >= count {
        return Err("Page structure changed; this OCR page cannot be safely rerun");
    }
    let catalog = current.catalog().map_err(|_| OUTPUT_ERROR)?;
    let bookmarks = field(current, &catalog, &["Outlines", "First"])
        .map_err(|_| OUTPUT_ERROR)?
        .is_some_and(|first| first.as_dict().is_some());
    let has_entries = |path: &[&str]| -> Result<bool, &'static str> {
        Ok(field(current, &catalog, path)
            .map_err(|_| OUTPUT_ERROR)?
            .and_then(|entries| entries.as_array().map(|entries| !entries.is_empty()))
            .unwrap_or(false))
    };
    if bookmarks
        || has_entries(&["Names", "EmbeddedFiles", "Names"])?
        || has_entries(&["AcroForm", "Fields"])?
    {
        return Err(
            "This PDF has bookmarks, attachments or forms; page OCR rerun needs manual review",
        );
    }
    let mut selected_has_annotation = false;
    for index in 0..count {
        let entries = annotations(current, index).map_err(|_| OUTPUT_ERROR)?;
        for entry in &entries {
            if has_link(current, entry).map_err(|_| OUTPUT_ERROR)? {
                return Err("This PDF has links; page OCR rerun needs manual review");
            }
        }
        if index == page {
            selected_has_annotation = has_annotation(current, &entries)?;
        }
    }
    if selected_has_annotation
        || has_annotation(
            retained,
            &annotations(retained, page).map_err(|_| OUTPUT_ERROR)?,
        )?
    {
        return Err("This page has annotations; page OCR rerun needs manual review");
    }
    if retained
        .extract_spans(page)
        .map_err(|_| OUTPUT_ERROR)?
        .iter()
        .any(|span| is_ocr_font(&span.font_name))
    {
        return Err("Pre-OCR source page already contains OCR text");
    }
    Ok(())
}
