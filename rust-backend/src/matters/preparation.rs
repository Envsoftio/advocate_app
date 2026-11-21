//! Matter-level preparation data and history shared by both Rust stores.
use super::{decode, integer, integer_error, now, record_edit, reply, Reply};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use serde_json::{json, Value};

pub(super) mod validation;
use std::io::{self, Write};
use validation::{invalid, issue, parse_parties, parse_review, parse_values, FilingReview};

pub(super) fn migrate(con: &Connection) -> rusqlite::Result<()> {
    con.execute_batch(
        "CREATE TABLE matter_preparation (
            matter_id TEXT NOT NULL, kind TEXT NOT NULL, payload TEXT NOT NULL,
            PRIMARY KEY(matter_id, kind)
         );
         PRAGMA user_version=4;",
    )
}

#[derive(Clone, Copy)]
enum Resource {
    Parties,
    Values,
}
impl Resource {
    fn kind(self) -> &'static str {
        match self {
            Self::Parties => "memo_parties",
            Self::Values => "matter_values",
        }
    }
    fn field(self) -> &'static str {
        match self {
            Self::Parties => "parties",
            Self::Values => "values",
        }
    }
    fn load(self, con: &Connection, matter_id: &str) -> rusqlite::Result<Value> {
        let raw: Option<String> = con
            .query_row(
                "SELECT payload FROM matter_preparation WHERE matter_id=? AND kind=?",
                params![matter_id, self.kind()],
                |row| row.get(0),
            )
            .optional()?;
        match raw {
            Some(raw) => decode(&raw),
            None => Ok(match self {
                Self::Parties => json!([]),
                Self::Values => json!({}),
            }),
        }
    }
    fn write(self, con: &Connection, matter_id: &str, payload: &Value) -> rusqlite::Result<()> {
        con.execute(
            "INSERT INTO matter_preparation VALUES (?,?,?)
             ON CONFLICT(matter_id, kind) DO UPDATE SET payload=excluded.payload",
            params![matter_id, self.kind(), payload.to_string()],
        )?;
        Ok(())
    }
}

pub(super) fn backup(con: &Connection, id: &str) -> rusqlite::Result<(Value, Value)> {
    Ok((
        Resource::Parties.load(con, id)?,
        Resource::Values.load(con, id)?,
    ))
}

pub(super) fn validate_portable_review(values: &Value, matter: &Value) -> Result<(), String> {
    let review = stored_review(values).ok_or("Invalid filing review in backup")?;
    if review
        .items
        .iter()
        .filter_map(|item| item.part)
        .any(|part| {
            !matter["parts"]
                .as_array()
                .is_some_and(|parts| parts.contains(&json!(part)))
        })
    {
        return Err("Filing review uses a Part missing from the matter".into());
    }
    Ok(())
}

pub(super) fn restore_backup(
    con: &Connection,
    id: &str,
    parties: &Value,
    values: &Value,
) -> Result<(), String> {
    let parties = if parties.as_array().is_some_and(Vec::is_empty) {
        parties.clone()
    } else {
        parse_parties(&json!({"parties":parties})).map_err(|_| "Invalid memo parties in backup")?
    };
    let values =
        parse_values(&json!({"values":values})).map_err(|_| "Invalid matter values in backup")?;
    if stored_review(&values).is_none() {
        return Err("Invalid filing review in backup".into());
    }
    Resource::Parties
        .write(con, id, &parties)
        .map_err(|error| error.to_string())?;
    Resource::Values
        .write(con, id, &values)
        .map_err(|error| error.to_string())?;
    Ok(())
}

enum Action {
    AddPart,
    RemovePart(i64),
    Read(Resource),
    Write(Resource, Value),
    ReadReview,
    WriteReview(FilingReview),
}

pub(super) fn request(
    con: &mut Connection,
    method: &str,
    route: &[&str],
    body: &Value,
) -> rusqlite::Result<Option<Reply>> {
    let ["matters", id, tail @ ..] = route else {
        return Ok(None);
    };
    let action = match (method, tail) {
        ("POST", ["parts"]) => Ok(Action::AddPart),
        ("DELETE", ["parts", number]) => integer(&json!(number))
            .map(Action::RemovePart)
            .ok_or_else(|| {
                let (kind, message) = integer_error(&json!(number));
                invalid(issue(kind, json!(["path", "part"]), message, json!(number)))
            }),
        ("GET", ["memo-parties"]) => Ok(Action::Read(Resource::Parties)),
        ("PUT", ["memo-parties"]) => {
            parse_parties(body).map(|v| Action::Write(Resource::Parties, v))
        }
        ("GET", ["values"]) => Ok(Action::Read(Resource::Values)),
        ("PUT", ["values"]) => parse_values(body).map(|v| Action::Write(Resource::Values, v)),
        ("GET", ["filing-review"]) => Ok(Action::ReadReview),
        ("PUT", ["filing-review"]) => parse_review(body).map(Action::WriteReview),
        _ => return Ok(None),
    };
    let action = match action {
        Ok(action) => action,
        Err(response) => return Ok(Some(response)),
    };
    let behavior = if method == "GET" {
        TransactionBehavior::Deferred
    } else {
        TransactionBehavior::Immediate
    };
    let tx = con.transaction_with_behavior(behavior)?;
    let raw: Option<String> = tx
        .query_row(
            "SELECT body FROM matters WHERE id=? AND json_extract(body,'$.archived_at') IS NULL",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(raw) = raw else {
        return Ok(Some(detail(404, "Matter not found")));
    };
    let mut matter = decode(&raw)?;
    let stamp = now();
    let (status, kind, before, after, audit, response) = match action {
        Action::Read(resource) => {
            return Ok(Some(reply(
                200,
                json!({resource.field(): resource.load(&tx, id)?}),
            )));
        }
        Action::ReadReview => {
            return Ok(Some(
                match stored_review(&Resource::Values.load(&tx, id)?) {
                    Some(review) => reply(200, json!(review)),
                    None => detail(
                        409,
                        "Stored filing review is invalid; restore or correct it before building",
                    ),
                },
            ));
        }
        Action::AddPart | Action::RemovePart(_) => {
            let before = part_snapshot(&matter);
            let Some((mut parts, mut next)) = snapshot_parts(&before) else {
                return Ok(Some(detail(409, "Stored matter Parts are invalid")));
            };
            let (status, audit) = match action {
                Action::AddPart => {
                    if parts.len() >= 50 || next > 99 {
                        return Ok(Some(detail(
                            409,
                            "This matter has reached the 50-Part or Part 99 limit",
                        )));
                    }
                    parts.push(next);
                    next += 1;
                    (201, "matter_part_added")
                }
                Action::RemovePart(part) => {
                    if !parts.contains(&part) {
                        return Ok(Some(detail(409, "Part is not in this matter")));
                    }
                    if parts.len() == 1 {
                        return Ok(Some(detail(409, "A matter must retain at least one Part")));
                    }
                    if let Some(response) = part_in_use(&tx, id, part)? {
                        return Ok(Some(response));
                    }
                    parts.retain(|value| *value != part);
                    (200, "matter_part_removed")
                }
                _ => unreachable!(),
            };
            matter["parts"] = json!(parts);
            matter["next_part"] = json!(next);
            matter["updated_at"] = json!(stamp);
            (
                status,
                "matter_parts",
                before,
                part_snapshot(&matter),
                audit,
                matter.clone(),
            )
        }
        Action::Write(resource, payload) => {
            let previous = resource.load(&tx, id)?;
            let response = json!({resource.field(): payload});
            if previous == payload {
                return Ok(Some(reply(200, response)));
            }
            resource.write(&tx, id, &payload)?;
            let audit = match resource {
                Resource::Parties => "memo_parties_updated",
                Resource::Values => "matter_values_updated",
            };
            (
                200,
                resource.kind(),
                json!({resource.field(): previous}),
                response.clone(),
                audit,
                response,
            )
        }
        Action::WriteReview(review) => {
            if review
                .items
                .iter()
                .filter_map(|item| item.part)
                .any(|part| {
                    !matter["parts"]
                        .as_array()
                        .is_some_and(|parts| parts.contains(&json!(part)))
                })
            {
                return Ok(Some(detail(
                    422,
                    "Filing review item uses a Part that is not in this matter",
                )));
            }
            let resource = Resource::Values;
            let previous = resource.load(&tx, id)?;
            let mut payload = previous.clone();
            if !payload.is_object() {
                return Ok(Some(detail(409, "Stored matter values are invalid")));
            }
            payload["filing_review_meta"] = json!(python_json(&review.meta));
            payload["filing_review_items"] = json!(python_json(&review.items));
            if let Err(error) = parse_values(&json!({"values": payload})) {
                return Ok(Some(detail(
                    422,
                    error.body["detail"][0]["msg"]
                        .as_str()
                        .unwrap_or("Invalid matter values"),
                )));
            }
            if previous == payload {
                return Ok(Some(reply(200, json!(review))));
            }
            resource.write(&tx, id, &payload)?;
            (
                200,
                resource.kind(),
                json!({"values":previous}),
                json!({"values":payload}),
                "matter_values_updated",
                json!(review),
            )
        }
    };
    matter["updated_at"] = json!(stamp);
    tx.execute(
        "UPDATE matters SET body=?,updated_at=? WHERE id=?",
        params![matter.to_string(), stamp, id],
    )?;
    record_edit(&tx, id, kind, &before, &after, &stamp)?;
    tx.execute(
        "INSERT INTO audit_events (matter_id,action,created_at) VALUES (?,?,?)",
        params![id, audit, stamp],
    )?;
    tx.commit()?;
    Ok(Some(reply(status, response)))
}

fn part_in_use(con: &Connection, id: &str, part: i64) -> rusqlite::Result<Option<Reply>> {
    let count: i64 = con.query_row(
        "SELECT count(*) FROM documents WHERE matter_id=? AND json_extract(body,'$.part')=?",
        params![id, part],
        |row| row.get(0),
    )?;
    if count > 0 {
        return Ok(Some(detail(409, format!(
            "Part {part} has {count} document(s), including archived documents. Reassign them before removal."
        ))));
    }
    let values = Resource::Values.load(con, id)?;
    if let Some(raw) = values.get("filing_review_items") {
        let items = raw
            .as_str()
            .and_then(|text| serde_json::from_str::<Vec<Value>>(text).ok());
        let Some(items) = items else {
            return Ok(Some(detail(
                409,
                "Stored filing review is invalid; restore or correct it before building",
            )));
        };
        if items.iter().any(|item| {
            item["part"].as_f64() == Some(part as f64)
                || item["part"].as_bool().map(i64::from) == Some(part)
        }) {
            return Ok(Some(detail(409, format!(
                "Part {part} is used by the filing review checklist. Change those items before removal."
            ))));
        }
    }
    Ok(None)
}

fn part_snapshot(matter: &Value) -> Value {
    json!({"parts_json":python_json(&matter["parts"]), "next_part":matter["next_part"]})
}

fn snapshot_parts(snapshot: &Value) -> Option<(Vec<i64>, i64)> {
    if snapshot.as_object()?.len() != 2 {
        return None;
    }
    let parts: Vec<i64> = serde_json::from_str(snapshot["parts_json"].as_str()?).ok()?;
    let next = snapshot["next_part"].as_i64()?;
    if parts.is_empty()
        || parts.len() > 50
        || !(2..=100).contains(&next)
        || parts.iter().any(|part| *part < 1 || *part >= next)
        || parts.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return None;
    }
    Some((parts, next))
}

fn history_resource(kind: &str) -> Option<Resource> {
    match kind {
        "memo_parties" => Some(Resource::Parties),
        "matter_values" => Some(Resource::Values),
        _ => None,
    }
}

pub(super) fn history_supported(kind: &str) -> bool {
    kind == "matter_parts" || history_resource(kind).is_some()
}

pub(super) fn replay(
    con: &Connection,
    id: &str,
    kind: &str,
    matter: &mut Value,
    target: &Value,
    expected: &Value,
) -> rusqlite::Result<bool> {
    if kind == "matter_parts" {
        let (Some((parts, next)), Some((current, _))) =
            (snapshot_parts(target), snapshot_parts(expected))
        else {
            return Ok(false);
        };
        if part_snapshot(matter) != *expected {
            return Ok(false);
        }
        for part in current.into_iter().filter(|part| !parts.contains(part)) {
            if part_in_use(con, id, part)?.is_some() {
                return Ok(false);
            }
        }
        matter["parts"] = json!(parts);
        matter["next_part"] = json!(next);
        return Ok(true);
    }
    let Some(resource) = history_resource(kind) else {
        return Ok(false);
    };
    if *expected != json!({resource.field(): resource.load(con, id)?}) {
        return Ok(false);
    }
    let payload = match resource {
        Resource::Parties if *target == json!({"parties":[]}) => Ok(json!([])),
        Resource::Parties => parse_parties(target),
        Resource::Values => parse_values(target),
    };
    let Ok(payload) = payload else {
        return Ok(false);
    };
    if *target != json!({resource.field(): payload}) {
        return Ok(false);
    }
    resource.write(con, id, &payload)?;
    Ok(true)
}

fn detail(status: u16, message: impl Into<String>) -> Reply {
    reply(status, json!({"detail": message.into()}))
}

pub(super) fn filing_review(con: &Connection, id: &str) -> rusqlite::Result<Option<Value>> {
    Ok(stored_review(&Resource::Values.load(con, id)?).map(|review| json!(review)))
}

fn stored_review(values: &Value) -> Option<FilingReview> {
    let mut meta: Value = match values.get("filing_review_meta") {
        Some(raw) => serde_json::from_str(raw.as_str()?).ok()?,
        None => json!({}),
    };
    let items = match values.get("filing_review_items") {
        Some(raw) => serde_json::from_str(raw.as_str()?).ok()?,
        None => json!([]),
    };
    meta.as_object_mut()?.insert("items".to_owned(), items);
    parse_review(&meta).ok()
}

// Python stores the review as JSON strings inside matter values.
pub(super) fn python_json(value: &impl Serialize) -> String {
    struct Formatter;
    impl serde_json::ser::Formatter for Formatter {
        fn begin_array_value<W: Write + ?Sized>(
            &mut self,
            writer: &mut W,
            first: bool,
        ) -> io::Result<()> {
            if first {
                Ok(())
            } else {
                writer.write_all(b", ")
            }
        }
        fn begin_object_key<W: Write + ?Sized>(
            &mut self,
            writer: &mut W,
            first: bool,
        ) -> io::Result<()> {
            self.begin_array_value(writer, first)
        }
        fn begin_object_value<W: Write + ?Sized>(&mut self, writer: &mut W) -> io::Result<()> {
            writer.write_all(b": ")
        }
    }
    let mut bytes = Vec::new();
    value
        .serialize(&mut serde_json::Serializer::with_formatter(
            &mut bytes, Formatter,
        ))
        .unwrap();
    String::from_utf8(bytes).unwrap()
}
