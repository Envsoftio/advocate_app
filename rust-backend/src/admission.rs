//! Prepares a PDF before any original or working revision is published.
use pdf_oxide::editor::DocumentEditor;
use pdf_oxide::encryption::{
    compute_encryption_key, compute_user_password_hash, EncryptDict, EncryptionHandler,
};
use pdf_oxide::object::{Object, ObjectRef};
use pdf_oxide::xref::{parse_xref, XRefEntryType};
use pdf_oxide::PdfDocument;
use serde::Serialize;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::io::Cursor;

pub(crate) const FILE_LIMIT_BYTES: usize = 80 * 1024 * 1024;
const SIGNATURE_OBJECT_LIMIT: usize = 200_000;

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct SignatureInspection {
    pub certificate_signature_status: &'static str,
    pub certificate_signature_count: usize,
    pub signature_inspection_note: String,
    pub visible_signature_status: &'static str,
    pub signature_status: &'static str,
}

impl SignatureInspection {
    fn new(count: usize, inconclusive: bool) -> Self {
        let (certificate_status, signature_status, note) = if count > 0 {
            let noun = if count == 1 {
                "dictionary"
            } else {
                "dictionaries"
            };
            (
                "detected",
                "certificate_signature_detected",
                format!("Found {count} PDF certificate-signature indicator {noun}. Cryptographic validity and coverage were not checked."),
            )
        } else if inconclusive {
            (
                "unknown",
                "inspection_inconclusive",
                "PDF signature inspection was inconclusive. No cryptographic validation was attempted; treat transformations cautiously.".into(),
            )
        } else {
            (
                "not_detected",
                "no_certificate_signature_detected",
                "No PDF certificate-signature dictionary with ByteRange and Contents indicators was detected. This does not rule out a visible or scanned signature.".into(),
            )
        };
        Self {
            certificate_signature_status: certificate_status,
            certificate_signature_count: count,
            signature_inspection_note: note,
            visible_signature_status: "unreviewed",
            signature_status,
        }
    }

    pub(crate) fn requires_acknowledgement(&self) -> bool {
        self.certificate_signature_status != "not_detected"
    }
}

#[derive(Debug, Serialize)]
pub struct AdmissionError {
    pub status: u16,
    pub detail: &'static str,
}

impl AdmissionError {
    const fn invalid(detail: &'static str) -> Self {
        Self {
            status: 422,
            detail,
        }
    }
}

#[derive(Debug)]
pub struct PreparedPdf<'a> {
    pub working_pdf: Cow<'a, [u8]>,
    pub page_count: usize,
    pub needs_ocr: bool,
    pub signature: SignatureInspection,
}

fn signature_dictionary(dict: &HashMap<String, Object>, file_len: usize) -> bool {
    let Some(ranges) = dict.get("ByteRange").and_then(Object::as_array) else {
        return false;
    };
    if ranges.len() < 4 || ranges.len() % 2 != 0 || ranges[0].as_integer() != Some(0) {
        return false;
    }
    let mut previous_end = 0;
    for range in ranges.chunks_exact(2) {
        let (Some(start), Some(length)) = (range[0].as_integer(), range[1].as_integer()) else {
            return false;
        };
        let Some(end) = start.checked_add(length) else {
            return false;
        };
        if start < previous_end || length <= 0 || end > file_len as i64 {
            return false;
        }
        previous_end = end;
    }
    dict.get("Contents")
        .and_then(Object::as_string)
        .is_some_and(|contents| !contents.is_empty())
}

fn populated(value: &Object) -> bool {
    !value.is_null()
}

fn signature_target(pdf: &PdfDocument, value: &Object, file_len: usize) -> bool {
    pdf.resolve_object(value)
        .ok()
        .and_then(|object| {
            object
                .as_dict()
                .map(|dict| signature_dictionary(dict, file_len))
        })
        .unwrap_or(false)
}

fn signature_field(pdf: &PdfDocument, fields: &HashMap<String, Object>) -> Result<bool, ()> {
    let widget = fields
        .get("Subtype")
        .and_then(|value| pdf.resolve_object(value).ok())
        .is_some_and(|value| value.as_name() == Some("Widget"));
    let mut ancestor = None;
    let mut seen = HashSet::new();
    for _ in 0..128 {
        let current = match &ancestor {
            Some(parent) => Object::as_dict(parent).ok_or(())?,
            None => fields,
        };
        if let Some(kind) = current.get("FT") {
            let kind = pdf.resolve_object(kind).map_err(|_| ())?;
            if !kind.is_null() {
                return kind.as_name().map(|name| name == "Sig").ok_or(());
            }
        }
        if !widget {
            return Ok(false);
        }
        let Some(parent) = current.get("Parent").filter(|parent| !parent.is_null()) else {
            return Ok(false);
        };
        if parent
            .as_reference()
            .is_some_and(|reference| !seen.insert(reference))
        {
            return Err(());
        }
        ancestor = Some(pdf.resolve_object(parent).map_err(|_| ())?);
    }
    Err(())
}

fn scan_object(
    pdf: &PdfDocument,
    object: &Object,
    file_len: usize,
    count: &mut usize,
    inconclusive: &mut bool,
) {
    let mut pending = vec![object];
    while let Some(object) = pending.pop() {
        let dict = match object {
            Object::Array(values) => {
                pending.extend(values);
                continue;
            }
            Object::Dictionary(dict) | Object::Stream { dict, .. } => dict,
            _ => continue,
        };
        let object_type = dict.get("Type").and_then(Object::as_name);
        if matches!(object_type, Some("Sig" | "DocTimeStamp")) || dict.contains_key("ByteRange") {
            if signature_dictionary(dict, file_len) {
                *count += 1;
            } else {
                *inconclusive = true;
            }
        }
        if let Some(value) = dict.get("V").filter(|value| populated(value)) {
            match signature_field(pdf, dict) {
                Ok(true) => *inconclusive |= !signature_target(pdf, value, file_len),
                Ok(false) => {}
                Err(()) => *inconclusive = true,
            }
        }
        // AcroForm SigFlags can be set when a signature field is empty. Inspect
        // populated values and permissions references instead.
        for key in ["DocMDP", "UR", "UR3"] {
            if let Some(value) = dict.get(key).filter(|value| populated(value)) {
                *inconclusive |= !signature_target(pdf, value, file_len);
            }
        }
        pending.extend(dict.values());
    }
}

fn xref_table(bytes: &[u8]) -> Result<pdf_oxide::xref::CrossRefTable, AdmissionError> {
    const INVALID_XREF: AdmissionError =
        AdmissionError::invalid("Unable to inspect PDF cross-reference table");
    let mut footer = bytes
        .rsplit(|byte| is_pdf_whitespace(*byte))
        .filter(|word| !word.is_empty());
    if footer.next() != Some(b"%%EOF") {
        return Err(AdmissionError::invalid(
            "Unable to inspect PDF protection: FileDataError",
        ));
    }
    let offset: usize = footer
        .next()
        .and_then(|word| std::str::from_utf8(word).ok())
        .and_then(|word| word.parse().ok())
        .ok_or(INVALID_XREF)?;
    if footer.next() != Some(b"startxref") || offset >= bytes.len() {
        return Err(INVALID_XREF);
    }
    let mut reader = Cursor::new(bytes);
    parse_xref(&mut reader, offset as u64).map_err(|_| INVALID_XREF)
}

fn is_pdf_whitespace(byte: u8) -> bool {
    matches!(byte, 0 | b'\t' | b'\n' | 12 | b'\r' | b' ')
}

fn inspect_signature(
    pdf: &PdfDocument,
    xref: &pdf_oxide::xref::CrossRefTable,
    file_len: usize,
) -> SignatureInspection {
    if xref.is_empty() || xref.len() > SIGNATURE_OBJECT_LIMIT {
        return SignatureInspection::new(0, true);
    }
    let mut count = 0;
    let mut inconclusive = false;
    for id in xref.all_object_numbers() {
        let Some(entry) = xref.get(id).filter(|entry| entry.in_use) else {
            continue;
        };
        let generation = match entry.entry_type {
            XRefEntryType::Compressed => 0,
            _ => entry.generation,
        };
        match pdf.load_object(ObjectRef::new(id, generation)) {
            Ok(object) => scan_object(pdf, &object, file_len, &mut count, &mut inconclusive),
            Err(_) => inconclusive = true,
        }
    }
    SignatureInspection::new(count, inconclusive)
}

fn authenticate(pdf: &PdfDocument, password: Option<&str>) -> Result<bool, AdmissionError> {
    if !pdf.is_encrypted() || pdf.is_authenticated() {
        return Ok(false);
    }
    let password = password
        .filter(|password| !password.is_empty())
        .ok_or_else(|| AdmissionError::invalid("PDF password required"))?;
    if !pdf.authenticate(password.as_bytes()).unwrap_or(false) {
        return Err(AdmissionError::invalid("Incorrect PDF password"));
    }
    let user_permissions_allow_editing = pdf
        .permissions()
        .is_some_and(|rights| rights.copy && rights.modify && rights.assemble);
    if !user_permissions_allow_editing && !authenticated_as_owner(pdf, password.as_bytes()) {
        return Err(AdmissionError::invalid(
            "PDF permissions do not allow copying, modification and assembly",
        ));
    }
    Ok(true)
}

fn authenticated_as_owner(pdf: &PdfDocument, password: &[u8]) -> bool {
    let Some(trailer) = pdf.trailer().as_dict() else {
        return false;
    };
    let Some(encrypt) = trailer.get("Encrypt") else {
        return false;
    };
    let Ok(Object::Dictionary(mut dictionary)) = pdf.resolve_object(encrypt) else {
        return false;
    };
    for value in dictionary.values_mut() {
        if let Object::Reference(reference) = value {
            let Ok(resolved) = pdf.load_object(*reference) else {
                return false;
            };
            *value = resolved;
        }
    }
    if dictionary.get("Filter").and_then(Object::as_name) != Some("Standard") {
        return false;
    }
    match dictionary.get("R").and_then(Object::as_integer) {
        Some(5 | 6) if dictionary.get("V").and_then(Object::as_integer) == Some(5) => {
            // Retain /U for the owner hash and disable user authentication in this check.
            dictionary.remove("UE");
            EncryptionHandler::new(&Object::Dictionary(dictionary), Vec::new())
                .and_then(|mut handler| handler.authenticate(password))
                .unwrap_or(false)
        }
        Some(2..=4) => {
            let Ok(fields) = EncryptDict::from_object(&Object::Dictionary(dictionary)) else {
                return false;
            };
            let file_id = trailer
                .get("ID")
                .and_then(Object::as_array)
                .and_then(|ids| ids.first())
                .and_then(Object::as_string)
                .unwrap_or_default();
            // The document already authenticated this password. Distinguish the two
            // valid credential types with the same user check the parser performs.
            let candidate = compute_encryption_key(
                password,
                &fields.owner_password,
                fields.permissions,
                file_id,
                fields.revision,
                fields.key_length_bytes(),
                fields.encrypt_metadata,
            )
            .map(zeroize::Zeroizing::new)
            .and_then(|key| compute_user_password_hash(&key, file_id, fields.revision));
            candidate.is_ok_and(|candidate| {
                candidate
                    .get(..16)
                    .zip(fields.user_password.get(..16))
                    .is_some_and(|(expected, stored)| !openssl::memcmp::eq(expected, stored))
            })
        }
        _ => false,
    }
}

fn needs_ocr(pdf: &PdfDocument, page_count: usize) -> Result<bool, AdmissionError> {
    for page in 0..page_count {
        let text = pdf
            .extract_text(page)
            .map_err(|_| AdmissionError::invalid("Unable to inspect PDF page text"))?;
        if text.trim().is_empty()
            && crate::matters::image_uses::has_painted_image(pdf, page)
                .map_err(|_| AdmissionError::invalid("Unable to inspect PDF page images"))?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn open_inspected_pdf(
    bytes: &[u8],
    password: Option<&str>,
) -> Result<(PdfDocument, bool, SignatureInspection), AdmissionError> {
    if bytes.len() > FILE_LIMIT_BYTES {
        return Err(AdmissionError {
            status: 413,
            detail: "Import limit is 80 MB per file",
        });
    }
    let xref = xref_table(bytes)?;
    let pdf = PdfDocument::from_bytes(bytes.to_vec())
        .map_err(|_| AdmissionError::invalid("Unable to inspect PDF protection: FileDataError"))?;
    let decrypt_working_copy = authenticate(&pdf, password)?;
    let signature = inspect_signature(&pdf, &xref, bytes.len());
    Ok((pdf, decrypt_working_copy, signature))
}

pub(crate) fn replacement_pdf_pages(bytes: &[u8]) -> Result<usize, AdmissionError> {
    const POSSIBLE_SIGNATURE: AdmissionError = AdmissionError::invalid(
        "The replacement source has possible signature content. Import and review it in this matter first.",
    );
    let (pdf, _, signature) = open_inspected_pdf(bytes, None).map_err(|error| {
        if error.status == 413 {
            error
        } else {
            POSSIBLE_SIGNATURE
        }
    })?;
    if signature.requires_acknowledgement() {
        return Err(POSSIBLE_SIGNATURE);
    }
    page_count(&pdf)
}

fn page_count(pdf: &PdfDocument) -> Result<usize, AdmissionError> {
    let count = pdf
        .page_count()
        .map_err(|_| AdmissionError::invalid("Unable to inspect PDF pages"))?;
    if count == 0 {
        return Err(AdmissionError::invalid("PDF has no pages"));
    }
    Ok(count)
}

/// Prepare an input from its original bytes. In-memory only, so the caller
/// can persist the original and the working copy in a single publication.
pub fn prepare_pdf<'a>(
    bytes: &'a [u8],
    password: Option<&str>,
    acknowledge_signature_risk: bool,
) -> Result<PreparedPdf<'a>, AdmissionError> {
    let (pdf, decrypt_working_copy, signature) = open_inspected_pdf(bytes, password)?;
    if decrypt_working_copy && signature.requires_acknowledgement() && !acknowledge_signature_risk {
        return Err(AdmissionError {
            status: 409,
            detail: "Protected PDF signature review required before creating a decrypted working copy. Certificate validity is not verified; the imported original will remain unchanged.",
        });
    }
    let page_count = page_count(&pdf)?;
    let needs_ocr = needs_ocr(&pdf, page_count)?;
    let working_pdf = if decrypt_working_copy {
        const ERROR: AdmissionError =
            AdmissionError::invalid("Unable to create decrypted PDF working copy");
        let output = DocumentEditor::from_document(pdf)
            .map_err(|_| ERROR)?
            .save_to_bytes()
            .map_err(|_| ERROR)?;
        {
            let check = PdfDocument::from_bytes(output.clone()).map_err(|_| ERROR)?;
            if check.is_encrypted() || check.page_count().map_err(|_| ERROR)? != page_count {
                return Err(ERROR);
            }
        }
        Cow::Owned(output)
    } else {
        Cow::Borrowed(bytes)
    };
    Ok(PreparedPdf {
        working_pdf,
        page_count,
        needs_ocr,
        signature,
    })
}
