//! Local reading state and page search use the same working revision identity as PDF reads.
use super::artifacts::{refusal, valid_id, PdfSource};
use super::*;
use pdf_oxide::PdfDocument;
use std::{collections::HashMap, sync::LazyLock};

pub(super) fn migrate(con: &Connection) -> rusqlite::Result<()> {
    con.execute_batch(
        "CREATE TABLE reader_positions (
            resource_key TEXT PRIMARY KEY, matter_id TEXT NOT NULL,
            payload TEXT NOT NULL, updated_at TEXT NOT NULL
         );
         PRAGMA user_version=5;",
    )
}

const ZOOMS: &[&str] = &[
    "fit-width",
    "fit-page",
    "25",
    "50",
    "75",
    "100",
    "125",
    "150",
    "200",
    "300",
    "400",
];

pub(super) fn route(
    store: &mut Store,
    method: &str,
    route: &[&str],
    query: &str,
    body: &Value,
) -> Option<Reply> {
    match (method, route) {
        ("GET", ["reader-position"]) => Some(
            lookup(query, "key")
                .ok_or_else(|| missing_query("key"))
                .and_then(|key| position(store, &key, None))
                .unwrap_or_else(|e| e),
        ),
        ("PUT", ["reader-position"]) => Some(
            parse_position(body)
                .and_then(|(key, data)| position(store, key, Some(data)))
                .unwrap_or_else(|e| e),
        ),
        ("GET", ["documents" | "matters", _, "search"]) => {
            let target = SearchTarget::select(method, route).expect("search route");
            Some(
                store
                    .prepare_search(target, query)
                    .and_then(SearchWork::compute)
                    .unwrap_or_else(|error| error),
            )
        }
        _ => None,
    }
}

// Decimal digit blocks from Python's Unicode 15.0.0 data (unicodedata.decimal).
pub(in crate::matters) const UNICODE_DECIMAL_ZEROS: &[u32] = &[
    0x30, 0x660, 0x6F0, 0x7C0, 0x966, 0x9E6, 0xA66, 0xAE6, 0xB66, 0xBE6, 0xC66, 0xCE6, 0xD66,
    0xDE6, 0xE50, 0xED0, 0xF20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80, 0x1A90,
    0x1B50, 0x1BB0, 0x1C40, 0x1C50, 0xA620, 0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0, 0xFF10,
    0x104A0, 0x10D30, 0x11066, 0x110F0, 0x11136, 0x111D0, 0x112F0, 0x11450, 0x114D0, 0x11650,
    0x116C0, 0x11730, 0x118E0, 0x11950, 0x11C50, 0x11D50, 0x11DA0, 0x11F50, 0x16A60, 0x16AC0,
    0x16B50, 0x1D7CE, 0x1D7D8, 0x1D7E2, 0x1D7EC, 0x1D7F6, 0x1E140, 0x1E2F0, 0x1E4F0, 0x1E950,
    0x1FBF0,
];

pub(in crate::matters) fn decimal_digit(ch: char) -> Option<u8> {
    let value = u32::from(ch);
    let block = UNICODE_DECIMAL_ZEROS
        .partition_point(|&zero| zero <= value)
        .checked_sub(1)?;
    let digit = value - UNICODE_DECIMAL_ZEROS[block];
    (digit <= 9).then_some(digit as u8)
}

pub(super) fn lookup<'a>(query: &'a str, name: &str) -> Option<Cow<'a, str>> {
    form_urlencoded::parse(query.as_bytes())
        .filter(|(key, _)| key == name)
        .last()
        .map(|(_, value)| value)
}

pub(super) fn missing_query(name: &str) -> Reply {
    reply(
        422,
        json!({"detail":[{"type":"missing","loc":["query",name],"msg":"Field required","input":null}]}),
    )
}

fn parse_position(body: &Value) -> Result<(&str, Value), Reply> {
    let invalid = || refusal(422, "Invalid reading position");
    let obj = body
        .as_object()
        .filter(|value| value.len() == 6)
        .ok_or_else(invalid)?;
    let n = |key: &str| {
        let input = obj.get(key)?;
        let value = input
            .as_f64()
            .or_else(|| input.as_bool().map(|b| f64::from(u8::from(b))))
            .or_else(|| input.as_str()?.trim().parse().ok())?;
        (0.0..=1.0).contains(&value).then_some(value)
    };
    let key = body["key"].as_str().ok_or_else(invalid)?;
    let page = body["page"]
        .as_u64()
        .filter(|p| (1..=100000).contains(p))
        .ok_or_else(invalid)?;
    let zoom = body["zoom"]
        .as_str()
        .filter(|zoom| ZOOMS.contains(zoom))
        .ok_or_else(invalid)?;
    let mode = body["mode"]
        .as_str()
        .filter(|mode| ["single", "continuous"].contains(mode))
        .ok_or_else(invalid)?;
    Ok((
        key,
        json!({"page":page,"x":n("x").ok_or_else(invalid)?,"y":n("y").ok_or_else(invalid)?,"zoom":zoom,"mode":mode}),
    ))
}

fn position(store: &mut Store, key: &str, state: Option<Value>) -> Result<Reply, Reply> {
    let invalid_key = || refusal(422, "Invalid reading position key");
    if key.len() > 200 {
        return Err(invalid_key());
    }
    let behavior = if state.is_some() {
        TransactionBehavior::Immediate
    } else {
        TransactionBehavior::Deferred
    };
    let tx = rusqlite::Transaction::new_unchecked(&store.con, behavior).map_err(|_| db_error())?;
    let parts: Vec<_> = key.split(':').collect();
    let (matter, pages) = match parts.as_slice() {
        ["original", doc] if valid_id(doc) => {
            let item = store.artifact_document(doc)?;
            store.open_document_pdf(doc, &item, &Artifact::Original)?;
            (
                item.body["matter_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                None,
            )
        }
        ["export", matter, export, filename]
            if valid_id(matter)
                && valid_id(export)
                && filename.strip_suffix(".pdf").is_some_and(|stem| {
                    !stem.is_empty()
                        && stem
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
                }) =>
        {
            if !store.active_matter(matter).map_err(|_| db_error())? {
                return Err(refusal(404, "Matter not found"));
            }
            (matter.to_string(), None)
        }
        [doc, revision]
            if valid_id(doc)
                && !revision.starts_with('0')
                && !revision.is_empty()
                && revision.bytes().all(|b| b.is_ascii_digit()) =>
        {
            let item = store.artifact_document(doc)?;
            if revision.parse::<u64>().ok() != item.body["revision"].as_u64() {
                return Err(refusal(
                    409,
                    "Reading position belongs to an earlier working revision",
                ));
            }
            (
                item.body["matter_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                item.body["page_count"].as_u64(),
            )
        }
        _ => return Err(invalid_key()),
    };
    if let Some(state) = state {
        if pages.is_some_and(|count| state["page"].as_u64().unwrap() > count) {
            return Err(refusal(422, "Reading page is outside this working PDF"));
        }
        tx.execute(
            "INSERT INTO reader_positions VALUES (?,?,?,?)
             ON CONFLICT(resource_key) DO UPDATE SET payload=excluded.payload,updated_at=excluded.updated_at",
            params![key, matter, state.to_string(), now()],
        ).map_err(|_| db_error())?;
        tx.execute(
            "DELETE FROM reader_positions WHERE resource_key NOT IN
             (SELECT resource_key FROM reader_positions ORDER BY updated_at DESC, resource_key LIMIT 100)", [],
        ).map_err(|_| db_error())?;
        tx.commit().map_err(|_| db_error())?;
        return Ok(reply(200, state));
    }
    let raw: Option<String> = tx
        .query_row(
            "SELECT payload FROM reader_positions WHERE resource_key=?",
            [key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| db_error())?;
    let data = raw
        .map(|raw| decode(&raw))
        .transpose()
        .map_err(|_| db_error())?;
    Ok(reply(200, data.unwrap_or(Value::Null)))
}

fn db_error() -> Reply {
    refusal(500, "Cannot access reading position")
}

pub(super) fn python_whitespace(ch: char) -> bool {
    ch.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&ch)
}

static CASE_FOLD: LazyLock<HashMap<char, &str>> = LazyLock::new(|| {
    include_str!("case_fold.txt")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| {
            let (source, target) = line.split_once('\t').expect("case fold entry");
            (source.chars().next().expect("case fold letter"), target)
        })
        .collect()
});

pub(super) fn folded(text: &str) -> String {
    let mut folded = String::with_capacity(text.len());
    for ch in text.chars() {
        if let Some(mapped) = CASE_FOLD.get(&ch) {
            folded.push_str(mapped);
        } else {
            folded.push(ch);
        }
    }
    folded
}

pub struct SearchTarget {
    id: String,
    matter: bool,
}

impl SearchTarget {
    pub fn from_path(method: &str, path: &str) -> Option<Self> {
        let route = path.trim_start_matches('/').split('/').collect::<Vec<_>>();
        Self::select(method, &route)
    }

    fn select(method: &str, route: &[&str]) -> Option<Self> {
        let ("GET", [kind @ ("documents" | "matters"), id, "search"]) = (method, route) else {
            return None;
        };
        Some(Self {
            id: (*id).to_owned(),
            matter: *kind == "matters",
        })
    }
}

struct SearchDocument {
    id: String,
    title: Value,
    revision: Value,
    pdf: Result<PdfSource, Reply>,
}

pub struct SearchWork {
    query: String,
    matter: bool,
    documents: Vec<Result<SearchDocument, Reply>>,
}

impl Store {
    pub fn prepare_search(&self, target: SearchTarget, query: &str) -> Result<SearchWork, Reply> {
        let SearchTarget { id, matter } = target;
        let query_text = lookup(query, "q").ok_or_else(|| missing_query("q"))?;
        let query_text = query_text.trim_matches(python_whitespace);
        let (expected, ids) = if matter {
            if !self
                .active_matter(&id)
                .map_err(|_| refusal(500, "Cannot access matter"))?
            {
                return Err(refusal(404, "Matter not found"));
            }
            let mut stmt = self
                .con
                .prepare(
                    "SELECT id FROM documents WHERE matter_id=? AND archived_at IS NULL
                 ORDER BY json_extract(body,'$.position'),created_at,id",
                )
                .map_err(|_| refusal(500, "Cannot list search documents"))?;
            let ids = stmt
                .query_map([&id], |row| row.get::<_, String>(0))
                .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
                .map_err(|_| refusal(500, "Cannot list search documents"))?;
            (None, ids)
        } else {
            let expected = lookup(query, "revision")
                .map(|input| {
                    integer(&json!(input))
                        .ok_or_else(|| refusal(422, "Invalid working PDF revision"))
                })
                .transpose()?;
            self.artifact_document(&id)?;
            (expected, vec![id.to_string()])
        };
        let query_len = query_text.chars().count();
        if !(2..=120).contains(&query_len) {
            return Err(refusal(422, "Search for 2 to 120 characters"));
        }
        let documents = ids
            .into_iter()
            .map(|id| {
                let item = self.artifact_document(&id)?;
                if expected.is_some_and(|revision| item.body["revision"] != revision) {
                    return Err(refusal(409, "Working PDF revision changed; search again"));
                }
                let pdf =
                    self.document_pdf_source(&id, &item, &Artifact::Working { revision: None });
                Ok(SearchDocument {
                    id,
                    title: item.body["title"].clone(),
                    revision: item.body["revision"].clone(),
                    pdf,
                })
            })
            .collect();
        Ok(SearchWork {
            query: query_text.to_owned(),
            matter,
            documents,
        })
    }
}

impl SearchWork {
    pub fn compute(self) -> Result<Reply, Reply> {
        let Self {
            query,
            matter,
            documents,
        } = self;
        let needle = folded(&query);
        let query_len = query.chars().count();
        let mut matches = Vec::new();
        for document in documents {
            let SearchDocument {
                id,
                title,
                revision,
                pdf,
            } = document?;
            let failed = |detail: &str| {
                let message = if matter {
                    format!(
                        "Cannot search working copy of {}: {detail}",
                        title.as_str().unwrap_or_default()
                    )
                } else {
                    format!("Cannot search working PDF: {detail}")
                };
                refusal(409, message)
            };
            let snapshot = pdf.and_then(PdfSource::open).map_err(|error| {
                failed(
                    error.body["detail"]
                        .as_str()
                        .unwrap_or("Working PDF unavailable"),
                )
            })?;
            let bytes = snapshot
                .into_bytes()
                .map_err(|error| failed(&error.to_string()))?;
            let pdf = PdfDocument::from_bytes(bytes).map_err(|error| failed(&error.to_string()))?;
            let pages = pdf.page_count().map_err(|err| failed(&err.to_string()))?;
            for page in 0..pages {
                let text = pdf
                    .extract_text(page)
                    .map_err(|err| failed(&err.to_string()))?;
                let text = text
                    .split(python_whitespace)
                    .filter(|word| !word.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                let candidate = folded(&text);
                let Some(offset) = candidate.find(&needle) else {
                    continue;
                };
                let position = candidate[..offset].chars().count();
                let start = position.saturating_sub(70);
                let count = text.chars().count();
                let end = count.min(position + query_len + 100);
                let excerpt: String = text
                    .chars()
                    .skip(start)
                    .take(end.saturating_sub(start))
                    .collect();
                let snippet = format!(
                    "{}{}{}",
                    if start > 0 { "…" } else { "" },
                    excerpt,
                    if end < count { "…" } else { "" }
                );
                matches.push(json!({"document_id":id,"title":title,"revision":revision,"page":page+1,"snippet":snippet}));
                if matches.len() == 100 {
                    return Ok(reply(200, json!({"matches":matches,"truncated":true})));
                }
            }
        }
        Ok(reply(200, json!({"matches":matches,"truncated":false})))
    }
}
