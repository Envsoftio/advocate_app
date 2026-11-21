use super::artifacts::refusal;
use super::*;
use serde_json::Map;
use std::collections::{BTreeMap, BTreeSet};

pub(super) mod request;
mod sort_labels;

fn error(status: u16, message: impl Into<String>) -> Reply {
    reply(status, json!({"detail":message.into()}))
}

pub(super) fn iso_date(text: &str) -> bool {
    chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .is_ok_and(|date| date.format("%Y-%m-%d").to_string() == text && !text.starts_with("0000"))
}

fn validate_review_fields(fields: &Map<String, Value>) -> Result<(), &'static str> {
    let get = |field| fields.get(field).and_then(Value::as_str);
    let date_fields = (get("document_date"), get("date_status"), get("date_source"));
    let note = get("date_note");
    match date_fields {
        (None, None, None) if note.is_none() => (),
        (Some(date), Some(status), Some(source)) => match status {
            "unknown"
                if !date.is_empty() || source != "none" || note.is_some_and(|s| !s.is_empty()) =>
            {
                return Err("Unknown dates must not have a value or source")
            }
            "ambiguous"
                if !date.is_empty()
                    || ["none", "user"].contains(&source)
                    || note.is_none_or(str::is_empty) =>
            {
                return Err("Ambiguous dates require a source and the unparsed date text")
            }
            "confirmed" | "suggested" if date.is_empty() => {
                return Err("Suggested and confirmed dates require a date value")
            }
            "confirmed" if source != "user" => {
                return Err("Only a user-reviewed date can be confirmed")
            }
            "suggested" if ["none", "user"].contains(&source) => {
                return Err("Suggested dates require an extracted, filename or filesystem source")
            }
            _ => (),
        },
        _ => return Err("Document date, status and source must be updated together"),
    }

    match (get("label"), get("label_status"), get("label_source")) {
        (_, None, None) => Ok(()),
        (Some(""), Some(status), Some(source)) if status != "unreviewed" || source != "none" => {
            Err("An empty label cannot be confirmed or have a source")
        }
        (Some(label), Some("confirmed"), Some(source))
            if !label.is_empty() && !["user", "proposal"].contains(&source) =>
        {
            Err("Confirmed labels require a user or reviewed proposal source")
        }
        (Some(label), Some("unreviewed"), Some(source))
            if !label.is_empty() && !["legacy", "filename"].contains(&source) =>
        {
            Err("Unreviewed labels require a legacy or filename source")
        }
        (Some(_), Some(_), Some(_)) => Ok(()),
        _ => Err("Label, confirmation status and source must be updated together"),
    }
}

pub(super) fn matter(con: &Connection, id: &str) -> rusqlite::Result<Option<Value>> {
    con.query_row(
        "SELECT body FROM matters WHERE id=? AND json_extract(body,'$.archived_at') IS NULL",
        [id],
        |row| row.get::<_, String>(0),
    )
    .optional()?
    .map(|raw| decode(&raw))
    .transpose()
}

pub(super) fn signature_status(doc: &Value, visible: &Value) -> Value {
    json!(match (
        doc["certificate_signature_status"].as_str(),
        visible.as_str()
    ) {
        (Some("detected"), _) => "certificate_signature_detected",
        (Some("unknown"), _) => "inspection_inconclusive",
        (_, Some("present")) => "visible_signature_only",
        (_, Some("absent")) => "unsigned",
        _ => "no_certificate_signature_detected",
    })
}

pub(super) fn part_allowed(matter: &Value, fields: &Map<String, Value>) -> bool {
    fields.get("part").is_none_or(|part| {
        matter["parts"]
            .as_array()
            .is_some_and(|parts| parts.contains(part))
    })
}

fn write_document(
    con: &Connection,
    doc: &mut Value,
    fields: &Map<String, Value>,
    stamp: &str,
) -> rusqlite::Result<()> {
    doc.as_object_mut()
        .ok_or(rusqlite::Error::InvalidQuery)?
        .extend(fields.clone());
    doc["updated_at"] = json!(stamp);
    con.execute(
        "UPDATE documents SET body=? WHERE id=?",
        params![doc.to_string(), doc["id"].as_str()],
    )?;
    Ok(())
}

pub(super) fn touch(con: &Connection, matter_id: &str, stamp: &str) -> rusqlite::Result<()> {
    con.execute(
        "UPDATE matters SET body=json_set(body,'$.updated_at',?),updated_at=? WHERE id=?",
        params![stamp, stamp, matter_id],
    )?;
    Ok(())
}

pub(super) fn parse_patch(body: &Value) -> Result<Map<String, Value>, Reply> {
    request::patch(body, &[])
}

pub(super) struct ReviewSnapshot {
    pub matter: Value,
    pub documents: Vec<Value>,
}

impl ReviewSnapshot {
    pub(super) fn capture(con: &Connection, id: &str) -> Result<Self, Reply> {
        let matter = matter(con, id)
            .map_err(|_| review_store_error())?
            .ok_or_else(|| refusal(404, "Matter not found"))?;
        let rows = Store::document_rows(con, id, false).map_err(|_| review_store_error())?;
        let mut documents = Vec::with_capacity(rows.len());
        for (row_id, matter_id, raw, archived) in rows {
            let doc = decode(&raw).map_err(|_| review_store_error())?;
            Store::check_document_identity(&doc, &row_id, &matter_id, archived.as_deref())
                .map_err(|error| refusal(409, error))?;
            documents.push(doc);
        }
        Ok(Self { matter, documents })
    }

    pub(super) fn matches(&self, current: &Self) -> bool {
        self.matter == current.matter && self.documents == current.documents
    }
}

pub(super) fn review_store_error() -> Reply {
    refusal(500, "Rust matter transaction failed; no changes committed")
}

pub(super) fn write_batch(
    tx: &Connection,
    id: &str,
    docs: &mut [Value],
    changes: impl IntoIterator<Item = (usize, Map<String, Value>)>,
    audit: &str,
) -> rusqlite::Result<()> {
    write_batch_with_evidence(tx, id, docs, changes, audit, None)
}

pub(super) fn write_batch_with_evidence(
    tx: &Connection,
    id: &str,
    docs: &mut [Value],
    changes: impl IntoIterator<Item = (usize, Map<String, Value>)>,
    audit: &str,
    evidence: Option<&[Value]>,
) -> rusqlite::Result<()> {
    let mut before = Vec::new();
    let mut after = Vec::new();
    let stamp = now();
    for (index, fields) in changes {
        let doc = &mut docs[index];
        if fields.iter().any(|(field, value)| doc[field] != *value) {
            let previous: Map<_, _> = fields
                .keys()
                .map(|key| (key.clone(), doc[key].clone()))
                .collect();
            before.push(json!({"id":doc["id"],"fields":previous}));
            after.push(json!({"id":doc["id"],"fields":fields}));
            write_document(tx, doc, &fields, &stamp)?;
        }
    }
    if !after.is_empty() {
        let mut recorded = json!({"items":after});
        if let Some(evidence) = evidence {
            recorded["metadata_evidence"] = json!(evidence);
        }
        record_edit(
            tx,
            id,
            "document_batch",
            &json!({"items":before}),
            &recorded,
            &stamp,
        )?;
        tx.execute(
            "INSERT INTO audit_events (matter_id,action,created_at) VALUES (?,?,?)",
            params![id, audit, stamp],
        )?;
        touch(tx, id, &stamp)?;
    }
    Ok(())
}

impl Store {
    pub(super) fn metadata_request(
        &mut self,
        method: &str,
        route: &[&str],
        body: &Value,
        order: &crate::RequestOrder,
    ) -> rusqlite::Result<Option<Reply>> {
        match (method, route) {
            ("PATCH", ["documents", id]) => self.patch_document(id, body, &order.root).map(Some),
            ("PATCH", ["matters", id, "documents", "batch"]) => {
                self.batch_metadata(id, body, &order.root).map(Some)
            }
            ("POST", ["matters", id, "documents", "sort-preview"]) => {
                self.sort_preview(id, body, &order.root).map(Some)
            }
            _ => Ok(None),
        }
    }

    fn patch_document(
        &mut self,
        id: &str,
        body: &Value,
        keys: &[String],
    ) -> rusqlite::Result<Reply> {
        let mut fields = match request::patch(body, keys) {
            Ok(fields) => fields,
            Err(refusal) => return Ok(refusal),
        };
        let tx = self
            .con
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(mut doc) = revisions::active_document(&tx, id)? else {
            return Ok(error(404, "Document not found"));
        };
        if body.as_object().unwrap().values().any(Value::is_null) {
            return Ok(error(
                422,
                "Do not clear document fields with null; use empty string instead",
            ));
        }
        if fields.is_empty() {
            return Ok(reply(200, doc));
        }
        let matter_id = doc["matter_id"]
            .as_str()
            .ok_or(rusqlite::Error::InvalidQuery)?
            .to_owned();
        let owner = matter(&tx, &matter_id)?.ok_or(rusqlite::Error::InvalidQuery)?;
        if !part_allowed(&owner, &fields) {
            return Ok(error(422, "Document Part is not in this matter"));
        }
        if let Err(message) = Self::verify_document_at(
            &tx,
            &self.root,
            self.key.as_ref(),
            &doc,
            id,
            &matter_id,
            None,
        ) {
            return Ok(error(409, message));
        }
        if fields.contains_key("label") && !fields.contains_key("label_status") {
            let label = fields["label"]
                .as_str()
                .unwrap()
                .trim_matches(reading::python_whitespace);
            let (status, source) = if label.is_empty() {
                ("unreviewed", "none")
            } else {
                ("confirmed", "user")
            };
            fields.insert("label".into(), json!(label));
            fields.insert("label_status".into(), json!(status));
            fields.insert("label_source".into(), json!(source));
        }
        if let Some(visible) = fields.get("visible_signature_status") {
            fields.insert("signature_status".into(), signature_status(&doc, visible));
        }
        let role = fields.get("group_role").unwrap_or(&doc["group_role"]);
        if role == "parent" {
            let group = fields
                .get("group_key")
                .unwrap_or(&doc["group_key"])
                .as_str()
                .unwrap_or_default()
                .trim_matches(reading::python_whitespace);
            if group.is_empty() {
                return Ok(error(422, "A group parent must have a group name"));
            }
            let duplicate: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM documents WHERE matter_id=? AND archived_at IS NULL AND id<>?
                    AND json_extract(body,'$.group_key')=? AND json_extract(body,'$.group_role')='parent')",
                params![matter_id, id, group], |row| row.get(0))?;
            if duplicate {
                return Ok(error(
                    422,
                    format!("Group “{group}” already has a designated parent"),
                ));
            }
        }
        fields.retain(|field, value| doc[field.as_str()] != *value);
        if fields.is_empty() {
            return Ok(reply(200, doc));
        }
        let before: Map<_, _> = fields
            .keys()
            .map(|key| (key.clone(), doc[key].clone()))
            .collect();
        let stamp = now();
        let document_stamp = if fields.len() == 1 && fields.contains_key("notes") {
            doc["updated_at"]
                .as_str()
                .ok_or(rusqlite::Error::InvalidQuery)?
                .to_owned()
        } else {
            stamp.clone()
        };
        write_document(&tx, &mut doc, &fields, &document_stamp)?;
        record_edit(
            &tx,
            &matter_id,
            "document",
            &json!({"id":id,"fields":before,"preserved_annotation_ids":[]}),
            &json!({"id":id,"fields":fields,"preserved_annotation_ids":[]}),
            &stamp,
        )?;
        tx.execute("INSERT INTO audit_events (matter_id,document_id,action,created_at) VALUES (?,?,'document_updated',?)",
            params![matter_id, id, stamp])?;
        touch(&tx, &matter_id, &document_stamp)?;
        tx.commit()?;
        Ok(reply(200, doc))
    }

    fn batch_metadata(
        &mut self,
        id: &str,
        body: &Value,
        keys: &[String],
    ) -> rusqlite::Result<Reply> {
        let (ids, fields) = match request::batch(body, keys) {
            Ok(parsed) => parsed,
            Err(refusal) => return Ok(refusal),
        };
        let selected: BTreeSet<_> = ids.iter().copied().collect();
        let tx = self
            .con
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(owner) = matter(&tx, id)? else {
            return Ok(error(404, "Matter not found"));
        };
        if !part_allowed(&owner, &fields) {
            return Ok(error(422, "Document Part is not in this matter"));
        }
        let mut docs =
            match Self::verified_documents(&tx, &self.root, self.key.as_ref(), id, false)? {
                Ok(docs) => docs,
                Err(refusal) => return Ok(refusal),
            };
        let positions: BTreeMap<_, _> = docs
            .iter()
            .enumerate()
            .map(|(index, doc)| (doc["id"].as_str().unwrap().to_owned(), index))
            .collect();
        if !ids.iter().all(|doc_id| positions.contains_key(*doc_id)) {
            return Ok(error(
                422,
                "Selection contains a missing, archived or duplicate document",
            ));
        }
        if let Some(group) = fields.get("group_key") {
            let groups = docs
                .iter()
                .filter(|doc| doc["group_role"] == "parent")
                .map(|doc| {
                    let final_group = if selected.contains(doc["id"].as_str().unwrap()) {
                        group
                    } else {
                        &doc["group_key"]
                    };
                    final_group
                        .as_str()
                        .unwrap_or_default()
                        .trim_matches(reading::python_whitespace)
                });
            if groups.clone().any(str::is_empty) {
                return Ok(error(
                    422,
                    "Clear or change the designated parent role before removing its group",
                ));
            }
            let mut parents = BTreeSet::new();
            if let Some(name) = groups.into_iter().find(|name| !parents.insert(*name)) {
                return Ok(error(
                    422,
                    format!("Group “{name}” would have more than one designated parent"),
                ));
            }
        }
        let changes = ids
            .into_iter()
            .map(|doc_id| (positions[doc_id], fields.clone()));
        write_batch(&tx, id, &mut docs, changes, "documents_batch_updated")?;
        tx.commit()?;
        Ok(reply(200, json!(docs)))
    }

    fn sort_preview(&self, id: &str, body: &Value, keys: &[String]) -> rusqlite::Result<Reply> {
        let mode = match request::sort(body, keys) {
            Ok(mode) => mode,
            Err(refusal) => return Ok(refusal),
        };
        let Some(owner) = matter(&self.con, id)? else {
            return Ok(error(404, "Matter not found"));
        };
        let docs =
            match Self::verified_documents(&self.con, &self.root, self.key.as_ref(), id, false)? {
                Ok(docs) => docs,
                Err(refusal) => return Ok(refusal),
            };
        let grouped = owner["keep_groups_intact"] == true;
        let mut units: Vec<Vec<usize>> = Vec::new();
        let mut groups = BTreeMap::new();
        for (index, doc) in docs.iter().enumerate() {
            let group = doc["group_key"]
                .as_str()
                .unwrap_or_default()
                .trim_matches(reading::python_whitespace);
            if mode != "manual" && grouped && !group.is_empty() {
                let group_index = *groups.entry(group).or_insert(units.len());
                if let Some(unit) = units.get_mut(group_index) {
                    unit.push(index);
                    continue;
                }
            }
            units.push(vec![index]);
        }
        if mode != "manual" {
            units.sort_by_cached_key(|unit| {
                let doc = &docs[unit[0]];
                let string = |key| doc[key].as_str().unwrap_or_default();
                let primary = match mode {
                    "upload" => string("created_at").to_owned(),
                    "filename" => reading::folded(string("original_name")),
                    _ => string("document_date").to_owned(),
                };
                (
                    mode == "document_date" && primary.is_empty(),
                    primary,
                    doc["position"].as_i64(),
                    string("id").to_owned(),
                )
            });
        }
        let order = units.into_iter().flatten().collect::<Vec<_>>();
        let rows = order
            .iter()
            .enumerate()
            .map(|(new, &old)| {
                let fields = [
                    "id",
                    "title",
                    "original_name",
                    "document_date",
                    "date_status",
                    "date_source",
                    "date_note",
                    "group_key",
                    "label",
                ];
                let mut row: Map<_, _> = fields
                    .into_iter()
                    .map(|key| (key.into(), docs[old][key].clone()))
                    .collect();
                row.insert("old_position".into(), json!(old));
                row.insert("new_position".into(), json!(new));
                row
            })
            .collect::<Vec<_>>();
        let label_review = sort_labels::review(&docs);
        Ok(reply(
            200,
            json!({
                "mode":mode, "ids":order.iter().map(|&i| &docs[i]["id"]).collect::<Vec<_>>(), "rows":rows,
                "unknown_dates":"last","stable_ties":"current manual order",
                "group_policy":if grouped {"keep grouped documents together in their current internal order"} else {"sort documents individually"},
                "label_review":label_review
            }),
        ))
    }
}

pub(super) fn replay(
    con: &Connection,
    matter_id: &str,
    kind: &str,
    target: &Value,
    expected: &Value,
) -> rusqlite::Result<Option<Reply>> {
    let refusal = || {
        Some(error(
            409,
            "Document metadata changed; this edit cannot be restored",
        ))
    };
    let (targets, expected) = if kind == "document_batch" {
        match (target["items"].as_array(), expected["items"].as_array()) {
            (Some(target), Some(expected))
                if target.len() == expected.len() && !target.is_empty() =>
            {
                (target.as_slice(), expected.as_slice())
            }
            _ => return Ok(refusal()),
        }
    } else {
        (std::slice::from_ref(target), std::slice::from_ref(expected))
    };
    let stamp = now();
    let mut pending = Vec::new();
    let mut seen = BTreeSet::new();
    for (target, expected) in targets.iter().zip(expected) {
        let Some(id) = target["id"]
            .as_str()
            .filter(|id| expected["id"] == *id && seen.insert(*id))
        else {
            return Ok(refusal());
        };
        let Some(doc) =
            revisions::active_document(con, id)?.filter(|doc| doc["matter_id"] == matter_id)
        else {
            return Ok(Some(error(
                409,
                "Document was removed; this edit cannot be restored",
            )));
        };
        let Some(fields) = target["fields"].as_object() else {
            return Ok(refusal());
        };
        if fields.is_empty()
            || expected["fields"].as_object().is_none_or(|state| {
                fields.len() != state.len()
                    || fields.keys().any(|key| {
                        !request::is_patch_field(key)
                            && key != "signature_status"
                            && !(kind == "document_batch" && key == "position")
                            || state.get(key).is_none_or(|value| doc[key] != *value)
                    })
            })
        {
            return Ok(refusal());
        }
        pending.push((doc, fields));
    }
    for (mut doc, fields) in pending {
        let previous_stamp = doc["updated_at"].as_str().unwrap_or(&stamp).to_owned();
        let stamp = if kind == "document" && fields.len() == 1 && fields.contains_key("notes") {
            &previous_stamp
        } else {
            &stamp
        };
        write_document(con, &mut doc, fields, stamp)?;
    }
    Ok(None)
}
