//! Revision-bound review annotations and their immutable image assets.
use super::{
    artifacts::refusal,
    document_metadata::{self, review_store_error},
    *,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;

mod request;

const INVALID_REGION: &str = "Annotation region must stay within the displayed page";
const INVALID_PAGE: &str = "Choose a current page in an active document";

pub(super) fn migrate(con: &Connection) -> rusqlite::Result<()> {
    con.execute_batch(
        "CREATE TABLE rust_annotations (
            id TEXT PRIMARY KEY, matter_id TEXT NOT NULL, position INTEGER NOT NULL, body TEXT NOT NULL);
         CREATE INDEX rust_annotations_order ON rust_annotations(matter_id, position);
         CREATE TABLE rust_annotation_assets (id TEXT PRIMARY KEY, bytes BLOB NOT NULL);
         PRAGMA user_version=8;",
    )
}

struct TextStyle {
    font_family: String,
    font_size: Value,
    bold: bool,
    alignment: String,
}

struct Fields {
    expected_revision: u64,
    rect_x: f64,
    rect_y: f64,
    rect_width: f64,
    rect_height: f64,
    body: String,
    color: String,
    output_scope: Option<String>,
    text_style: Option<TextStyle>,
}

impl Fields {
    fn rect(&self) -> [f64; 4] {
        [self.rect_x, self.rect_y, self.rect_width, self.rect_height]
    }
}

fn valid_region([x, y, width, height]: [f64; 4]) -> bool {
    (0.0..1.0).contains(&x)
        && (0.0..1.0).contains(&y)
        && width > 0.0
        && width <= 1.0
        && height > 0.0
        && height <= 1.0
        && x + width <= 1.0
        && y + height <= 1.0
}

pub(super) fn points(body: &str) -> Option<Vec<[f64; 2]>> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct PathData {
        v: u8,
        points: Vec<[f64; 2]>,
    }
    let value: Value = serde_json::from_str(body).ok()?;
    let path: PathData = serde_json::from_value(value).ok()?;
    (path.v == 1
        && (2..=160).contains(&path.points.len())
        && path
            .points
            .iter()
            .flatten()
            .all(|value| (0.0..=1.0).contains(value)))
    .then_some(path.points)
}

fn scope<'a>(entry: &'a Value, fields: &'a Fields) -> &'a str {
    fields
        .output_scope
        .as_deref()
        .or_else(|| entry["output_scope"].as_str())
        .unwrap_or("private")
}

fn missing_comment(kind: &str, body: &str) -> bool {
    kind == "comment" && body.trim_matches(reading::python_whitespace).is_empty()
}

fn valid_text(body: &str) -> bool {
    !body.trim_matches(reading::python_whitespace).is_empty()
        && body
            .bytes()
            .all(|byte| byte == b'\n' || (32..=126).contains(&byte))
}

fn valid_date(body: &str) -> bool {
    body.len() == 10 && document_metadata::iso_date(body)
}

fn create_issue(kind: &str, fields: &Fields) -> Option<&'static str> {
    let scope = fields.output_scope.as_deref().unwrap_or("private");
    let body = fields.body.as_str();
    match kind {
        "comment" if missing_comment(kind, body) => Some("Comments require visible note text"),
        "date" if !valid_date(body) => Some("Date marks require a YYYY-MM-DD label"),
        "draw" if points(body).is_none() => Some("Draw marks require a bounded point path"),
        "text" if !valid_text(body) => Some("Text marks require printable ASCII text"),
        _ if kind != "text" && fields.text_style.is_some() => {
            Some("Text style applies only to added text")
        }
        "redaction" if scope != "pdf" || !body.is_empty() => {
            Some("Redaction marks require PDF output scope and no text")
        }
        "highlight" | "comment" if scope != "private" => {
            Some("Review highlights and comments must remain private")
        }
        _ => None,
    }
}

fn update_issue(entry: &Value, fields: &Fields) -> Option<&'static str> {
    let kind = entry["kind"].as_str().unwrap_or_default();
    let scope = scope(entry, fields);
    let body = fields.body.as_str();
    match kind {
        _ if kind != "text" && fields.text_style.is_some() => {
            Some("Text style applies only to added text")
        }
        "highlight" | "comment" if scope != "private" => {
            Some("Review highlights and comments must remain private")
        }
        "redaction" if scope != "pdf" => Some("Redaction marks must be included in PDF output"),
        "draw" if points(body).is_none() => Some("Draw marks require a bounded point path"),
        "date" if !valid_date(body) => Some("Date marks require a valid YYYY-MM-DD date"),
        "text" if !valid_text(body) => Some("Text marks require printable ASCII text"),
        "redaction" if !body.is_empty() => Some("Redaction marks cannot contain text"),
        _ => None,
    }
}

fn merge(entry: &mut Value, fields: Fields) {
    let scope = scope(entry, &fields).to_owned();
    entry["rect_x"] = json!(fields.rect_x);
    entry["rect_y"] = json!(fields.rect_y);
    entry["rect_width"] = json!(fields.rect_width);
    entry["rect_height"] = json!(fields.rect_height);
    entry["body"] = json!(fields.body);
    entry["color"] = json!(fields.color);
    entry["output_scope"] = json!(scope);
    if let Some(style) = fields.text_style {
        let value = json!({"alignment":style.alignment,"bold":style.bold,
            "font_family":style.font_family,"font_size":style.font_size});
        entry["text_style_json"] = json!(preparation::python_json(&value));
    }
}

pub(super) fn raw(con: &Connection, matter: &str) -> rusqlite::Result<Vec<Value>> {
    let mut query =
        con.prepare("SELECT body FROM rust_annotations WHERE matter_id=? ORDER BY position")?;
    let result = query
        .query_map([matter], |row| decode(&row.get::<_, String>(0)?))?
        .collect();
    result
}

#[derive(Clone, Copy)]
pub(super) enum Retention {
    None,
    Pdf,
    All,
}

pub(super) fn retained_ids(
    con: &Connection,
    document: &str,
    revision: u64,
    retention: Retention,
) -> rusqlite::Result<Vec<String>> {
    if matches!(retention, Retention::None) {
        return Ok(Vec::new());
    }
    let mut query = con.prepare(
        "SELECT id FROM rust_annotations
         WHERE json_extract(body,'$.document_id')=? AND json_extract(body,'$.target_revision')=?
         AND (? OR json_extract(body,'$.output_scope')='pdf') ORDER BY position",
    )?;
    let ids = query.query_map(
        params![document, revision, matches!(retention, Retention::All)],
        |row| row.get(0),
    )?;
    ids.collect()
}

pub(super) fn retarget(
    con: &Connection,
    document: &str,
    revision: u64,
    ids: impl IntoIterator<Item = impl AsRef<str>>,
) -> rusqlite::Result<()> {
    let mut update = con.prepare(
        "UPDATE rust_annotations SET body=json_set(body,'$.target_revision',?)
         WHERE id=? AND json_extract(body,'$.document_id')=?",
    )?;
    for id in ids {
        update.execute(params![revision, id.as_ref(), document])?;
    }
    Ok(())
}

pub(super) fn list(con: &Connection, matter: &str) -> rusqlite::Result<Vec<Value>> {
    let mut entries = raw(con, matter)?;
    let mut docs = std::collections::HashMap::new();
    for entry in &mut entries {
        let id = entry["document_id"]
            .as_str()
            .ok_or(rusqlite::Error::InvalidQuery)?;
        let doc = match docs.entry(id.to_owned()) {
            std::collections::hash_map::Entry::Occupied(doc) => doc.into_mut(),
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(revisions::active_document(con, id)?)
            }
        };
        entry["resolved"] = json!(doc.as_ref().is_some_and(|doc| {
            doc["revision"] == entry["target_revision"]
                && entry["page_index"].as_u64() < doc["page_count"].as_u64()
        }));
        entry["text_style"] = decode(entry["text_style_json"].as_str().unwrap_or("{}"))?;
    }
    Ok(entries)
}

fn write(con: &Connection, matter: &str, entries: &[Value]) -> rusqlite::Result<()> {
    con.execute("DELETE FROM rust_annotations WHERE matter_id=?", [matter])?;
    let mut insert = con.prepare("INSERT INTO rust_annotations VALUES (?,?,?,?)")?;
    for entry in entries {
        insert.execute(params![
            entry["id"].as_str(),
            matter,
            entry["position"].as_u64(),
            entry.to_string()
        ])?;
    }
    Ok(())
}

pub(super) fn restore_backup(
    con: &Connection,
    matter: &str,
    entries: &[Value],
) -> Result<(), String> {
    for entry in entries {
        let fields = super::bundle_inputs::select(
            entry,
            &[
                "rect_x",
                "rect_y",
                "rect_width",
                "rect_height",
                "body",
                "color",
                "output_scope",
            ],
        );
        let mut fields = fields;
        fields["expected_revision"] = entry["target_revision"].clone();
        let style = entry["text_style_json"]
            .as_str()
            .ok_or("Invalid text style in backup")?;
        let style: Value =
            serde_json::from_str(style).map_err(|_| "Invalid text style in backup")?;
        if style.as_object().is_some_and(|map| !map.is_empty()) {
            fields["text_style"] = style;
        }
        let document = entry["document_id"]
            .as_str()
            .ok_or("Invalid annotation source")?;
        let revision = entry["target_revision"]
            .as_u64()
            .ok_or("Invalid annotation revision")?;
        let target = super::revisions::record(con, document, revision)
            .map_err(|error| error.to_string())?
            .ok_or("Annotation target revision is missing")?;
        if !entry["page_index"]
            .as_u64()
            .is_some_and(|page| page < target.pages)
        {
            return Err("Annotation page lies outside its retained revision".into());
        }
        let fields = request::update(&fields, &crate::RequestOrder::default())
            .map_err(|_| "Invalid annotation in backup")?;
        let kind = entry["kind"].as_str().unwrap_or_default();
        if kind != "image" && !request::KINDS.contains(&kind) {
            return Err("Unsupported annotation kind in backup".into());
        }
        if update_issue(entry, &fields).is_some() || missing_comment(kind, &fields.body) {
            return Err("Invalid annotation in backup".into());
        }
    }
    write(con, matter, entries).map_err(|error| error.to_string())
}

pub(super) fn replay(
    con: &Connection,
    matter: &str,
    target: &Value,
    _expected: &Value,
) -> rusqlite::Result<Option<Reply>> {
    let entries = target["entries"]
        .as_array()
        .ok_or(rusqlite::Error::InvalidQuery)?;
    for entry in entries {
        let document = entry["document_id"]
            .as_str()
            .ok_or(rusqlite::Error::InvalidQuery)?;
        let selected_page = revisions::active_document(con, document)?.is_some_and(|doc| {
            doc["matter_id"] == matter
                && entry["page_index"]
                    .as_u64()
                    .zip(doc["page_count"].as_u64())
                    .is_some_and(|(page, count)| page < count)
        });
        if !selected_page {
            return Ok(Some(refusal(
                409,
                "Choose a current page in an active document",
            )));
        }
        if entry["kind"] == "image" {
            let asset_id = entry["id"].as_str().ok_or(rusqlite::Error::InvalidQuery)?;
            if asset(con, asset_id)?.is_none() {
                return Ok(Some(refusal(
                    409,
                    "Image overlay asset is missing; this edit cannot be restored",
                )));
            }
        }
    }
    write(con, matter, entries)?;
    Ok(None)
}

fn find(con: &Connection, id: &str) -> rusqlite::Result<Option<(String, Value)>> {
    con.query_row(
        "SELECT matter_id,body FROM rust_annotations WHERE id=?",
        [id],
        |row| Ok((row.get(0)?, decode(&row.get::<_, String>(1)?)?)),
    )
    .optional()
}

pub(super) fn asset(con: &Connection, id: &str) -> rusqlite::Result<Option<Vec<u8>>> {
    con.query_row(
        "SELECT bytes FROM rust_annotation_assets WHERE id=?",
        [id],
        |row| row.get(0),
    )
    .optional()
}

pub(super) struct ImageUpload {
    pub bytes: Vec<u8>,
    pub format: image::ImageFormat,
    pub decoded: image::DynamicImage,
    extension: &'static str,
    mime: &'static str,
}

pub(super) enum ImageUploadTarget {
    Annotation,
    WorkingCopy,
}

pub(super) fn image_upload(body: &Value, target: ImageUploadTarget) -> Result<ImageUpload, Reply> {
    const LIMIT: usize = 10 * 1024 * 1024;
    let too_large = || {
        refusal(
            413,
            match target {
                ImageUploadTarget::Annotation => "Image overlay exceeds 10 MiB",
                ImageUploadTarget::WorkingCopy => "Image exceeds 10 MiB",
            },
        )
    };
    // The desktop transport signals oversized files without transferring bytes.
    if matches!(target, ImageUploadTarget::Annotation)
        && body["file_size"]
            .as_u64()
            .is_some_and(|size| size > LIMIT as u64)
    {
        return Err(too_large());
    }
    let encoded = body["data"]
        .as_str()
        .ok_or_else(|| refusal(422, "Image file is required"))?;
    if encoded.len() > 4 * LIMIT.div_ceil(3) {
        return Err(too_large());
    }
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| refusal(422, "Invalid image encoding"))?;
    if bytes.len() > LIMIT {
        return Err(too_large());
    }
    let suffix = Path::new(body["filename"].as_str().unwrap_or_default())
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let (format, extension, mime, signature): (_, _, _, &[u8]) = match suffix.as_str() {
        "png" => (
            image::ImageFormat::Png,
            "png",
            "image/png",
            b"\x89PNG\r\n\x1a\n",
        ),
        "jpg" | "jpeg" => (
            image::ImageFormat::Jpeg,
            "jpg",
            "image/jpeg",
            b"\xff\xd8\xff",
        ),
        _ => return Err(refusal(422, "Image overlays support PNG and JPEG only")),
    };
    if !bytes.starts_with(signature) {
        return Err(refusal(
            422,
            "Overlay image bytes do not match the PNG/JPEG filename",
        ));
    }
    let decoded = super::document_output::decode_overlay_image(&bytes, format)?;
    Ok(ImageUpload {
        bytes,
        format,
        decoded,
        extension,
        mime,
    })
}

pub fn annotation_image_document<'a>(method: &str, path: &'a str) -> Option<&'a str> {
    if method != "POST" {
        return None;
    }
    path.trim_start_matches('/')
        .strip_prefix("documents/")?
        .strip_suffix("/annotations/image")
        .filter(|id| !id.contains('/'))
}

pub fn annotation_asset_id<'a>(method: &str, path: &'a str) -> Option<&'a str> {
    if method != "GET" {
        return None;
    }
    path.trim_start_matches('/')
        .strip_prefix("annotations/")?
        .strip_suffix("/asset")
        .filter(|id| !id.contains('/'))
}

struct AnnotationDraft {
    document: String,
    matter: String,
    entry: Value,
}

pub struct AnnotationImageWork {
    root: PathBuf,
    document: String,
    matter: String,
    revision: u64,
    page_count: u64,
    fields: ImageFields,
    body: Value,
}

struct ImageFields {
    page: i64,
    revision: i64,
    region: [f64; 4],
}

impl ImageFields {
    fn read(body: &Value) -> Result<Self, Reply> {
        let mut errors = Vec::new();
        let page = image_form_field(body, "page_index", &mut errors, integer, integer_error);
        let revision = image_form_field(
            body,
            "expected_revision",
            &mut errors,
            integer,
            integer_error,
        );
        let region = ["rect_x", "rect_y", "rect_width", "rect_height"].map(|key| {
            image_form_field(body, key, &mut errors, image_form_float, |value| {
                if value.is_string() {
                    (
                        "float_parsing",
                        "Input should be a valid number, unable to parse string as a number",
                    )
                } else {
                    ("float_type", "Input should be a valid number")
                }
            })
        });
        let (Some(page), Some(revision), [Some(x), Some(y), Some(width), Some(height)]) =
            (page, revision, region)
        else {
            return Err(reply(422, json!({"detail":errors})));
        };
        Ok(Self {
            page,
            revision,
            region: [x, y, width, height],
        })
    }
}

fn image_form_field<T>(
    body: &Value,
    key: &str,
    errors: &mut Vec<Value>,
    parse: impl FnOnce(&Value) -> Option<T>,
    parse_error: impl FnOnce(&Value) -> (&'static str, &'static str),
) -> Option<T> {
    use super::preparation::validation::issue;
    let loc = json!(["body", key]);
    let value = &body[key];
    if value.is_null() || value.as_str() == Some("") {
        errors.push(issue("missing", loc, "Field required", Value::Null));
        return None;
    }
    let parsed = parse(value);
    if parsed.is_none() {
        let (kind, message) = parse_error(value);
        errors.push(issue(kind, loc, message, value.clone()));
    }
    parsed
}

fn image_form_float(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| {
        let text = value.as_str()?.trim();
        let bytes = text.as_bytes();
        for (i, &byte) in bytes.iter().enumerate() {
            if byte == b'_'
                && (!i
                    .checked_sub(1)
                    .and_then(|previous| bytes.get(previous))
                    .is_some_and(u8::is_ascii_digit)
                    || !bytes.get(i + 1).is_some_and(u8::is_ascii_digit))
            {
                return None;
            }
        }
        text.replace('_', "").parse().ok()
    })
}

pub struct MeasuredAnnotationImage {
    root: PathBuf,
    draft: AnnotationDraft,
    bytes: Vec<u8>,
}

pub struct AnnotationAssetWork {
    reader: StoreReader,
    identifier: String,
    matter: String,
    path: Value,
    mime: Value,
}

impl AnnotationAssetWork {
    pub fn compute(self) -> Result<Reply, Reply> {
        let db = || refusal(500, "Unable to read the image overlay asset");
        let con = self.reader.open().map_err(|_| db())?;
        let tx = con.unchecked_transaction().map_err(|_| db())?;
        let current = find(&tx, &self.identifier).map_err(|_| db())?;
        if !current.is_some_and(|(matter, entry)| {
            matter == self.matter
                && entry["kind"] == "image"
                && entry["asset_relpath"] == self.path
                && entry["asset_mime"] == self.mime
        }) || document_metadata::matter(&tx, &self.matter)
            .map_err(|_| db())?
            .is_none()
        {
            return Err(refusal(404, "Image overlay not found"));
        }
        let bytes = asset(&tx, &self.identifier)
            .map_err(|_| db())?
            .ok_or_else(|| refusal(404, "Image overlay asset missing"))?;
        tx.commit().map_err(|_| db())?;
        Ok(super::document_output::binary(
            bytes,
            self.mime.as_str().unwrap_or("image/png"),
        ))
    }
}

impl AnnotationImageWork {
    pub fn compute(self) -> Result<MeasuredAnnotationImage, Reply> {
        let Self {
            root,
            document,
            matter,
            revision,
            page_count,
            fields,
            body,
        } = self;
        let ImageUpload {
            bytes,
            extension,
            mime,
            ..
        } = image_upload(&body, ImageUploadTarget::Annotation)?;
        if !valid_region(fields.region) {
            return Err(refusal(422, INVALID_REGION));
        }
        let page = u64::try_from(fields.page).map_err(|_| refusal(422, "Invalid page number"))?;
        if page >= page_count {
            return Err(refusal(422, INVALID_PAGE));
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        let [x, y, width, height] = fields.region;
        let entry = json!({
            "id":id, "document_id":document, "page_index":page, "target_revision":revision,
            "kind":"image", "asset_relpath":format!("assets/{id}.{extension}"),
            "asset_mime":mime, "text_style_json":"{}", "output_scope":"pdf",
            "body":"", "color":"yellow", "rect_x":x, "rect_y":y,
            "rect_width":width, "rect_height":height,
        });
        let draft = AnnotationDraft {
            document,
            matter,
            entry,
        };
        Ok(MeasuredAnnotationImage { root, draft, bytes })
    }
}

fn annotation_stale(image: bool) -> Reply {
    refusal(
        409,
        if image {
            "Document changed; reload and place the image again"
        } else {
            "Document changed; reload and draw the annotation again"
        },
    )
}

fn prepare_annotation(
    con: &Connection,
    document: &str,
    request::Create { kind, page, fields }: request::Create,
) -> Result<AnnotationDraft, Reply> {
    let doc = revisions::active_document(con, document)
        .map_err(|_| review_store_error())?
        .ok_or_else(|| refusal(404, "Document not found"))?;
    if doc["revision"] != fields.expected_revision {
        return Err(annotation_stale(false));
    }
    if Some(page) >= doc["page_count"].as_u64() {
        return Err(refusal(422, INVALID_PAGE));
    }
    let matter = doc["matter_id"]
        .as_str()
        .ok_or_else(review_store_error)?
        .to_owned();
    let mut entry = json!({
        "id":uuid::Uuid::new_v4().simple().to_string(), "document_id":document, "page_index":page,
        "target_revision":fields.expected_revision,"kind":kind,"asset_relpath":null,"asset_mime":null,
        "text_style_json":"{}","output_scope":"private"
    });
    merge(&mut entry, fields);
    Ok(AnnotationDraft {
        document: document.to_owned(),
        matter,
        entry,
    })
}

impl Store {
    pub(super) fn annotation_request(
        &mut self,
        method: &str,
        route: &[&str],
        body: &Value,
        order: &crate::RequestOrder,
    ) -> rusqlite::Result<Option<Reply>> {
        let result = match (method, route) {
            ("GET", ["matters", matter, "annotations"]) => {
                if document_metadata::matter(&self.con, matter)?.is_none() {
                    refusal(404, "Matter not found")
                } else {
                    reply(200, json!(list(&self.con, matter)?))
                }
            }
            ("POST", ["documents", id, "annotations"]) => request::create(body, order)
                .and_then(|draft| prepare_annotation(&self.con, id, draft))
                .and_then(|draft| self.commit_annotation(draft, None))
                .unwrap_or_else(|error| error),
            ("POST", ["documents", id, "annotations", "image"]) => self
                .prepare_annotation_image(id, body.clone())
                .and_then(AnnotationImageWork::compute)
                .and_then(|image| self.complete_annotation_image(image))
                .unwrap_or_else(|error| error),
            ("PATCH", ["annotations", id]) => match request::update(body, order) {
                Ok(fields) => self.annotation_change(id, Some(fields))?,
                Err(error) => error,
            },
            ("DELETE", ["annotations", id]) => self.annotation_change(id, None)?,
            ("GET", ["annotations", id, "asset"]) => self
                .prepare_annotation_asset(id)
                .and_then(AnnotationAssetWork::compute)
                .unwrap_or_else(|error| error),
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    pub fn prepare_annotation_image(
        &self,
        id: &str,
        body: Value,
    ) -> Result<AnnotationImageWork, Reply> {
        let fields = ImageFields::read(&body)?;
        let doc = revisions::active_document(&self.con, id)
            .map_err(|_| review_store_error())?
            .ok_or_else(|| refusal(404, "Document not found"))?;
        let revision = doc["revision"].as_u64().ok_or_else(review_store_error)?;
        if u64::try_from(fields.revision) != Ok(revision) {
            return Err(annotation_stale(true));
        }
        Ok(AnnotationImageWork {
            root: self.root.clone(),
            document: id.to_owned(),
            matter: doc["matter_id"]
                .as_str()
                .ok_or_else(review_store_error)?
                .to_owned(),
            revision,
            page_count: doc["page_count"].as_u64().ok_or_else(review_store_error)?,
            fields,
            body,
        })
    }

    pub fn complete_annotation_image(
        &mut self,
        measured: MeasuredAnnotationImage,
    ) -> Result<Reply, Reply> {
        if measured.root != self.root {
            return Err(annotation_stale(true));
        }
        self.commit_annotation(measured.draft, Some(measured.bytes))
    }

    pub fn prepare_annotation_asset(&self, id: &str) -> Result<AnnotationAssetWork, Reply> {
        let db = || refusal(500, "Unable to read the image overlay asset");
        let (matter, entry) = find(&self.con, id)
            .map_err(|_| db())?
            .filter(|(_, entry)| entry["kind"] == "image" && entry["asset_relpath"].is_string())
            .ok_or_else(|| refusal(404, "Image overlay not found"))?;
        if document_metadata::matter(&self.con, &matter)
            .map_err(|_| db())?
            .is_none()
        {
            return Err(refusal(404, "Matter not found"));
        }
        let reader = StoreReader::capture(self).map_err(|_| db())?;
        Ok(AnnotationAssetWork {
            reader,
            matter,
            identifier: id.to_owned(),
            path: entry["asset_relpath"].clone(),
            mime: entry["asset_mime"].clone(),
        })
    }

    fn commit_annotation(
        &mut self,
        draft: AnnotationDraft,
        asset: Option<Vec<u8>>,
    ) -> Result<Reply, Reply> {
        let AnnotationDraft {
            document,
            matter,
            mut entry,
        } = draft;
        let tx = self
            .con
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| review_store_error())?;
        let doc = revisions::active_document(&tx, &document)
            .map_err(|_| review_store_error())?
            .ok_or_else(|| refusal(404, "Document not found"))?;
        if doc["revision"] != entry["target_revision"] || doc["matter_id"] != matter {
            return Err(annotation_stale(asset.is_some()));
        }
        if entry["page_index"].as_u64() >= doc["page_count"].as_u64() {
            return Err(refusal(422, INVALID_PAGE));
        }
        let before = raw(&tx, &matter).map_err(|_| review_store_error())?;
        let stamp = now();
        entry["position"] = json!(before.len());
        entry["created_at"] = json!(stamp);
        entry["updated_at"] = json!(stamp);
        if let Some(bytes) = asset {
            tx.execute(
                "INSERT INTO rust_annotation_assets VALUES (?,?)",
                params![entry["id"].as_str(), bytes],
            )
            .map_err(|_| review_store_error())?;
        }
        let mut after = before.clone();
        after.push(entry);
        let result = publish(
            &tx,
            &matter,
            &document,
            &before,
            &after,
            "page_annotation_created",
            &stamp,
            201,
        )
        .map_err(|_| review_store_error())?;
        tx.commit().map_err(|_| review_store_error())?;
        Ok(result)
    }

    fn annotation_change(&mut self, id: &str, fields: Option<Fields>) -> rusqlite::Result<Reply> {
        let tx = self
            .con
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some((matter, current)) = find(&tx, id)? else {
            return Ok(refusal(404, "Annotation not found"));
        };
        let document = current["document_id"]
            .as_str()
            .ok_or(rusqlite::Error::InvalidQuery)?;
        let before = raw(&tx, &matter)?;
        let mut after = before.clone();
        let position = before
            .iter()
            .position(|entry| entry["id"] == id)
            .ok_or(rusqlite::Error::InvalidQuery)?;
        let stamp = now();
        let action = if let Some(fields) = fields {
            let Some(doc) = revisions::active_document(&tx, document)? else {
                return Ok(refusal(404, "Document not found"));
            };
            if doc["revision"] != fields.expected_revision
                || current["target_revision"] != fields.expected_revision
            {
                return Ok(refusal(
                    409,
                    "Document changed; this annotation needs review",
                ));
            }
            if let Some(problem) = update_issue(&current, &fields) {
                return Ok(refusal(422, problem));
            }
            if current["page_index"].as_u64() >= doc["page_count"].as_u64() {
                return Ok(refusal(422, INVALID_PAGE));
            }
            if missing_comment(current["kind"].as_str().unwrap_or_default(), &fields.body) {
                return Ok(refusal(422, "Comments require visible note text"));
            }
            let entry = &mut after[position];
            merge(entry, fields);
            if *entry == current {
                return Ok(reply(200, json!(list(&tx, &matter)?)));
            }
            entry["updated_at"] = json!(stamp);
            "page_annotation_updated"
        } else {
            after.remove(position);
            for (position, entry) in after.iter_mut().enumerate() {
                entry["position"] = json!(position);
            }
            "page_annotation_deleted"
        };
        let result = publish(&tx, &matter, document, &before, &after, action, &stamp, 200)?;
        tx.commit()?;
        Ok(result)
    }
}

fn publish(
    con: &Connection,
    matter: &str,
    doc: &str,
    before: &[Value],
    after: &[Value],
    action: &str,
    stamp: &str,
    status: u16,
) -> rusqlite::Result<Reply> {
    write(con, matter, after)?;
    record_edit(
        con,
        matter,
        "page_annotations",
        &json!({"entries":before}),
        &json!({"entries":after}),
        stamp,
    )?;
    document_metadata::touch(con, matter, stamp)?;
    con.execute(
        "INSERT INTO audit_events (matter_id,document_id,action,created_at) VALUES (?,?,?,?)",
        params![matter, doc, action, stamp],
    )?;
    Ok(reply(status, json!(list(con, matter)?)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault_store::VaultStore;

    #[test]
    fn editable_annotation_validation_keeps_schema_and_revision_order() {
        let root = tempfile::tempdir().unwrap();
        let mut vault = VaultStore::open(&root.path().join("vault")).unwrap();
        assert_eq!(
            vault
                .request(
                    "POST",
                    "/vault/setup",
                    json!({"password":"Synthetic annotated request validation"}),
                )
                .status,
            200
        );
        let matter = vault
            .request(
                "POST",
                "/matters",
                json!({"name":"Editable annotation validation"}),
            )
            .body;
        let matter = matter["id"].as_str().unwrap();
        let source = include_bytes!("../../../samples/rust_pdf_corpus/unsigned.pdf");
        let imported = vault.import_pdf(matter, "source.pdf", source, ImportOptions::default());
        assert_eq!(imported.status, 201, "{imported:?}");
        let document = imported.body[0]["id"].as_str().unwrap();
        let route = format!("/documents/{document}/annotations");
        let raw = r#"{
            "kind":"unknown","page_index":-1,"expected_revision":2,
            "rect_x":1,"rect_y":0.2,"rect_width":0,"rect_height":2,"color":null,
            "text_style":{"font_family":"bad","font_size":41,"bold":2,"alignment":null,"odd_z":true,"odd_a":false},
            "extra_z":0,"extra_a":1
        }"#;
        let malformed = vault.request_with_order(
            "POST",
            &route,
            serde_json::from_str(raw).unwrap(),
            &crate::RequestOrder::from_json(Some(raw)),
        );
        assert_eq!(malformed.status, 422);
        let issues = malformed.body["detail"].as_array().unwrap();
        assert_eq!(
            issues
                .iter()
                .map(|issue| issue["loc"]
                    .as_array()
                    .unwrap()
                    .last()
                    .unwrap()
                    .as_str()
                    .unwrap())
                .collect::<Vec<_>>(),
            [
                "kind",
                "page_index",
                "rect_x",
                "rect_width",
                "rect_height",
                "color",
                "font_family",
                "font_size",
                "bold",
                "alignment",
                "odd_z",
                "odd_a",
                "extra_z",
                "extra_a"
            ],
        );
        assert_eq!(issues[2]["ctx"], json!({"lt":1.0}));
        assert_eq!(issues[8]["type"], "bool_parsing");
        assert_eq!(issues[10]["loc"], json!(["body", "text_style", "odd_z"]));
        assert_eq!(issues[12]["loc"], json!(["body", "extra_z"]));

        let invalid_comment = json!({"kind":"comment","page_index":0,"expected_revision":2,
            "rect_x":0.1,"rect_y":0.2,"rect_width":0.3,"rect_height":0.4,"body":"","output_scope":"pdf"});
        let response = vault.request("POST", &route, invalid_comment.clone());
        assert_eq!(response.status, 422);
        assert_eq!(
            response.body["detail"],
            json!([{
                "type":"value_error","loc":["body"],"msg":"Value error, Comments require visible note text",
                "input":invalid_comment,"ctx":{"error":{}}
            }])
        );
        let create = vault.request(
            "POST",
            &route,
            json!({
                "kind":"text","page_index":"0.0","expected_revision":"1",
                "rect_x":0.1,"rect_y":"0.2","rect_width":0.3,"rect_height":0.4,
                "body":"First line","output_scope":"pdf",
                "text_style":{"bold":"yes","font_size":"11"}
            }),
        );
        assert_eq!(create.status, 201, "{create:?}");
        assert_eq!(create.body.as_array().unwrap().len(), 1);
        let mark = &create.body[0];
        let id = mark["id"].as_str().unwrap();
        assert_eq!(
            mark["text_style"],
            json!({
                "alignment":"left","bold":true,"font_family":"helv","font_size":11.0
            })
        );

        let malformed_patch = vault.request(
            "PATCH",
            "/annotations/missing",
            json!({
                "expected_revision":1,"rect_x":0.1,"rect_y":0.2,"rect_width":0,"rect_height":0.4
            }),
        );
        assert_eq!(malformed_patch.status, 422);
        assert_eq!(
            malformed_patch.body["detail"][0]["loc"],
            json!(["body", "rect_width"])
        );

        let patch = format!("/annotations/{id}");
        let update = json!({"expected_revision":1,"rect_x":0.1,"rect_y":0.2,
            "rect_width":0.3,"rect_height":0.4,"body":"Edited line","output_scope":null,"text_style":null});
        let mut stale = update.clone();
        stale["expected_revision"] = json!(2);
        let response = vault.request("PATCH", &patch, stale);
        assert_eq!(response.status, 409);
        assert_eq!(
            response.body["detail"],
            "Document changed; this annotation needs review"
        );
        let saved = vault.request("PATCH", &patch, update);
        assert_eq!(saved.status, 200, "{saved:?}");
        assert_eq!(saved.body[0]["body"], "Edited line");
        assert_eq!(saved.body[0]["target_revision"], 1);
        assert_eq!(saved.body[0]["output_scope"], "pdf");
        assert_eq!(saved.body[0]["text_style"], mark["text_style"]);

        let style_patch = |style| {
            json!({"expected_revision":1,"rect_x":0.1,"rect_y":0.2,
                "rect_width":0.3,"rect_height":0.4,"body":"Edited line","text_style":style})
        };
        let default_size = vault.request("PATCH", &patch, style_patch(json!({})));
        assert_eq!(default_size.status, 200);
        assert_eq!(
            default_size.body[0]["text_style_json"],
            r#"{"alignment": "left", "bold": false, "font_family": "helv", "font_size": 12}"#
        );
        let explicit_size = vault.request("PATCH", &patch, style_patch(json!({"font_size":12})));
        assert_eq!(explicit_size.status, 200);
        assert_eq!(
            explicit_size.body[0]["text_style_json"],
            r#"{"alignment": "left", "bold": false, "font_family": "helv", "font_size": 12.0}"#
        );
    }

    #[test]
    fn editable_image_failures_preserve_form_revision_and_upload_order() {
        let root = tempfile::tempdir().unwrap();
        let mut vault = VaultStore::open(&root.path().join("vault")).unwrap();
        assert_eq!(
            vault
                .request(
                    "POST",
                    "/vault/setup",
                    json!({"password":"Synthetic image precedence"}),
                )
                .status,
            200
        );
        let matter = vault
            .request("POST", "/matters", json!({"name":"Editable image order"}))
            .body;
        let matter = matter["id"].as_str().unwrap();
        let source = include_bytes!("../../../samples/rust_pdf_corpus/unsigned.pdf");
        let document = vault
            .import_pdf(matter, "source.pdf", source, ImportOptions::default())
            .body;
        let document = document[0]["id"].as_str().unwrap();
        let endpoint = format!("/documents/{document}/annotations/image");
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            4,
            4,
            image::Rgb([31, 130, 152]),
        ))
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
        let image = STANDARD.encode(encoded.into_inner());
        let normal = json!({
            "filename":"synthetic.png","data":image,"expected_revision":"1","page_index":"0",
            "rect_x":"0.1","rect_y":"0.1","rect_width":"0.4","rect_height":"0.3"
        });

        let cases = [
            (
                json!({"expected_revision":"2","rect_width":"1.2","file_size":10485761}),
                409,
                "Document changed; reload and place the image again",
            ),
            (
                json!({"file_size":10485761,"rect_width":"1.2"}),
                413,
                "Image overlay exceeds 10 MiB",
            ),
            (
                json!({"page_index":"-1","rect_width":"1.2","data":"aW52YWxpZA=="}),
                422,
                "Overlay image bytes do not match the PNG/JPEG filename",
            ),
            (
                json!({"page_index":"-1","rect_width":"1.2"}),
                422,
                "Annotation region must stay within the displayed page",
            ),
            (json!({"page_index":"-1"}), 422, "Invalid page number"),
            (
                json!({"page_index":"8"}),
                422,
                "Choose a current page in an active document",
            ),
        ];
        for (changes, status, detail) in cases {
            let mut request = normal.clone();
            request
                .as_object_mut()
                .unwrap()
                .extend(changes.as_object().unwrap().clone());
            let response = vault.request("POST", &endpoint, request);
            assert_eq!(response.status, status, "{response:?}");
            assert_eq!(response.body["detail"], detail);
        }
        let mut malformed = normal.clone();
        malformed["page_index"] = json!("0.5");
        malformed["expected_revision"] = json!("2");
        malformed["rect_width"] = json!("not a number");
        let response = vault.request("POST", &endpoint, malformed);
        assert_eq!(response.status, 422);
        assert_eq!(
            response.body["detail"][0]["loc"],
            json!(["body", "page_index"])
        );
        assert_eq!(response.body["detail"][0]["input"], "0.5");
        assert_eq!(
            response.body["detail"][1]["loc"],
            json!(["body", "rect_width"])
        );
        assert_eq!(
            vault
                .request(
                    "GET",
                    &format!("/matters/{matter}/annotations"),
                    Value::Null
                )
                .body,
            json!([])
        );

        let created = vault.request("POST", &endpoint, normal);
        assert_eq!(created.status, 201, "{created:?}");
        let id = created.body[0]["id"].as_str().unwrap();
        assert_eq!(created.body[0]["target_revision"], 1);
        assert_eq!(created.body[0]["output_scope"], "pdf");
        let asset = vault.request("GET", &format!("/annotations/{id}/asset"), Value::Null);
        assert_eq!(asset.status, 200, "{asset:?}");
        assert_eq!(asset.body["media_type"], "image/png");
        assert_eq!(asset.body["data"], image);
    }

    #[test]
    fn decoded_image_is_not_committed_after_working_revision_changes() {
        let root = tempfile::tempdir().unwrap();
        let mut vault = VaultStore::open(&root.path().join("vault")).unwrap();
        let setup = vault.request(
            "POST",
            "/vault/setup",
            json!({"password":"Synthetic annotation worker"}),
        );
        assert_eq!(setup.status, 200, "{setup:?}");
        let matter = vault.request(
            "POST",
            "/matters",
            json!({"name":"Image annotation fixture"}),
        );
        let matter = matter.body["id"].as_str().unwrap();
        let source = include_bytes!("../../../samples/rust_pdf_corpus/unsigned.pdf");
        let imported = vault.import_pdf(matter, "source.pdf", source, ImportOptions::default());
        assert_eq!(imported.status, 201, "{imported:?}");
        let document = imported.body[0]["id"].as_str().unwrap();
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            4,
            4,
            image::Rgb([12, 142, 201]),
        ))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
        let fields = json!({"filename":"fixture.png","data":STANDARD.encode(png.into_inner()),
            "expected_revision":1, "page_index":0,"rect_x":0.1,"rect_y":0.1,
            "rect_width":0.4,"rect_height":0.3});
        let measured = vault
            .prepare_annotation_image(document, fields.clone())
            .unwrap()
            .compute()
            .unwrap();
        let rotated = vault.request(
            "POST",
            &format!("/documents/{document}/pages/rotate"),
            json!({"expected_revision":1,"page_index":0,"degrees":90}),
        );
        assert_eq!(rotated.status, 200, "{rotated:?}");
        let refusal = vault.complete_annotation_image(measured).unwrap_err();
        assert_eq!(refusal.status, 409);
        assert_eq!(
            refusal.body["detail"],
            "Document changed; reload and place the image again"
        );
        let unmodified = vault.request(
            "GET",
            &format!("/matters/{matter}/annotations"),
            Value::Null,
        );
        assert_eq!(unmodified.body, json!([]));

        let mut fields = fields;
        fields["expected_revision"] = json!(2);
        let prepared = vault
            .prepare_annotation_image(document, fields)
            .unwrap()
            .compute()
            .unwrap();
        let committed = vault.complete_annotation_image(prepared).unwrap();
        assert_eq!(committed.status, 201);
        let identifier = committed.body[0]["id"].as_str().unwrap();
        let asset = vault
            .prepare_annotation_asset(identifier)
            .unwrap()
            .compute()
            .unwrap();
        assert_eq!(asset.body["media_type"], "image/png");
        assert!(asset.body["data"].as_str().unwrap().starts_with("iVBOR"));
    }
}
