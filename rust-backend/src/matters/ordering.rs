//! Update document order without rewriting the source or working PDF.
use super::{now, record_edit, reply, DataKey, Reply, Store};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::Path};

impl Store {
    pub(super) fn reorder_documents(&mut self, id: &str, body: &Value) -> rusqlite::Result<Reply> {
        let ids = match super::preparation::validation::parse_order(body) {
            Ok(ids) => ids,
            Err(refusal) => return Ok(refusal),
        };
        let tx = self
            .con
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let active: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM matters WHERE id=? AND json_extract(body,'$.archived_at') IS NULL",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        if active.is_none() {
            return Ok(reply(404, json!({"detail":"Matter not found"})));
        }
        let mut documents =
            match Self::verified_documents(&tx, &self.root, self.key.as_ref(), id, false)? {
                Ok(documents) => documents,
                Err(refusal) => return Ok(refusal),
            };
        let before = document_ids(&documents);
        if !same_document_set(&before, &ids) {
            return Ok(reply(
                422,
                json!({"detail":"Order must contain every document exactly once"}),
            ));
        }
        if before == ids {
            return Ok(reply(200, json!(documents)));
        }
        let before = json!({"ids":before});
        let stamp = now();
        apply_order(&tx, &mut documents, &ids, &stamp)?;
        record_edit(&tx, id, "order", &before, &json!({"ids":ids}), &stamp)?;
        tx.execute(
            "INSERT INTO audit_events (matter_id,action,created_at) VALUES (?,'documents_reordered',?)",
            params![id, stamp],
        )?;
        tx.execute(
            "UPDATE matters SET body=json_set(body,'$.updated_at',?),updated_at=? WHERE id=?",
            params![stamp, stamp, id],
        )?;
        tx.commit()?;
        Ok(reply(200, json!(documents)))
    }
}

fn document_ids(documents: &[Value]) -> Vec<&str> {
    documents
        .iter()
        .map(|doc| doc["id"].as_str().expect("verified document ID"))
        .collect()
}

fn same_document_set(current: &[&str], target: &[&str]) -> bool {
    current.len() == target.len()
        && current.iter().collect::<BTreeSet<_>>() == target.iter().collect()
}

fn apply_order(
    con: &Connection,
    documents: &mut [Value],
    ids: &[&str],
    stamp: &str,
) -> rusqlite::Result<()> {
    let positions = ids
        .iter()
        .enumerate()
        .map(|(index, &id)| (id, index))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut update = con.prepare("UPDATE documents SET body=? WHERE id=?")?;
    for doc in documents.iter_mut() {
        let id = doc["id"].as_str().expect("verified document ID");
        doc["position"] = json!(positions[id]);
        doc["updated_at"] = json!(stamp);
        update.execute(params![doc.to_string(), doc["id"].as_str()])?;
    }
    documents.sort_unstable_by_key(|doc| doc["position"].as_u64());
    Ok(())
}

pub(super) fn replay(
    con: &Connection,
    root: &Path,
    key: Option<&DataKey>,
    id: &str,
    target: &Value,
    expected: &Value,
) -> rusqlite::Result<Option<Reply>> {
    let mut documents = match Store::verified_documents(con, root, key, id, false)? {
        Ok(documents) => documents,
        Err(refusal) => return Ok(Some(refusal)),
    };
    let before = document_ids(&documents);
    if *expected == json!({"ids":before}) {
        if let Ok(ids) = super::preparation::validation::parse_order(target) {
            if *target == json!({"ids":ids}) && same_document_set(&before, &ids) {
                apply_order(con, &mut documents, &ids, &now())?;
                return Ok(None);
            }
        }
    }
    Ok(Some(reply(
        409,
        json!({"detail":"Document set changed; order cannot be restored"}),
    )))
}
