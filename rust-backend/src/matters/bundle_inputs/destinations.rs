use super::*;

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Bookmark,
    Link,
}

impl Kind {
    pub(super) fn from_route(route: &str) -> Option<Self> {
        match route {
            "bookmarks" => Some(Self::Bookmark),
            "links" => Some(Self::Link),
            _ => None,
        }
    }

    fn from_history(kind: &str) -> Option<Self> {
        match kind {
            "bookmarks" => Some(Self::Bookmark),
            "reviewed_links" => Some(Self::Link),
            _ => None,
        }
    }

    fn history(self) -> &'static str {
        match self {
            Self::Bookmark => "bookmarks",
            Self::Link => "reviewed_links",
        }
    }

    fn audit(self) -> &'static str {
        match self {
            Self::Bookmark => "manual_bookmarks_updated",
            Self::Link => "reviewed_links_updated",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Bookmark => "Bookmark",
            Self::Link => "Reviewed link",
        }
    }
}

pub(in crate::matters) fn history_kind(kind: &str) -> bool {
    Kind::from_history(kind).is_some()
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Bookmark {
    id: String,
    title: String,
    document_id: String,
    page_index: u64,
    target_revision: u64,
    parent_id: Option<String>,
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Link {
    id: String,
    title: String,
    source_document_id: String,
    source_page_index: u64,
    source_revision: u64,
    rect_x: f64,
    rect_y: f64,
    rect_width: f64,
    rect_height: f64,
    kind: String,
    target_document_id: Option<String>,
    target_page_index: Option<u64>,
    target_revision: Option<u64>,
    #[serde(default)]
    uri: String,
}

impl Link {
    fn validate(&self) -> Result<(), &'static str> {
        let origin_valid = [self.rect_x, self.rect_y]
            .iter()
            .all(|value| (0.0..1.0).contains(value));
        let size_valid = [self.rect_width, self.rect_height]
            .iter()
            .all(|&value| 0.0 < value && value <= 1.0);
        if !origin_valid || !size_valid {
            return Err("Invalid link rectangle");
        }
        if self.rect_x + self.rect_width > 1.0 || self.rect_y + self.rect_height > 1.0 {
            return Err("Link region must stay within the displayed page");
        }
        if self.source_revision == 0 || self.target_revision == Some(0) {
            return Err("Invalid link revision");
        }
        if self.uri.chars().count() > 2000 {
            return Err("Link URL must contain at most 2000 characters");
        }
        match self.kind.as_str() {
            "internal" => {
                if self.target_document_id.as_deref().is_none_or(str::is_empty)
                    || self.target_page_index.is_none()
                    || self.target_revision.is_none()
                    || !self.uri.is_empty()
                {
                    return Err("Internal links require one current document page and no URL");
                }
            }
            "external" => {
                if self.target_document_id.is_some()
                    || self.target_page_index.is_some()
                    || self.target_revision.is_some()
                {
                    return Err("External links cannot also have an internal target");
                }
                let (scheme, destination) = self
                    .uri
                    .trim_start_matches(|c| c <= ' ')
                    .split_once(':')
                    .unwrap_or_default();
                match scheme.to_ascii_lowercase().as_str() {
                    "https" | "http" => {
                        let host = destination
                            .strip_prefix("//")
                            .unwrap_or_default()
                            .split(['/', '?', '#'])
                            .next()
                            .unwrap_or_default();
                        if host.is_empty() {
                            return Err("External web links require a host name");
                        }
                    }
                    "mailto" => {
                        if destination
                            .split(['?', '#'])
                            .next()
                            .is_none_or(str::is_empty)
                        {
                            return Err("Mail links require an address");
                        }
                    }
                    _ => {
                        return Err(
                            "External links support only https, http or mailto destinations",
                        )
                    }
                }
            }
            _ => return Err("Unknown link kind"),
        }
        Ok(())
    }
}

pub(super) fn load(con: &Connection, matter_id: &str, kind: Kind) -> rusqlite::Result<Vec<Value>> {
    let mut query = con.prepare(
        "SELECT body FROM bundle_destinations WHERE matter_id=? AND kind=? ORDER BY position",
    )?;
    let rows = query.query_map(params![matter_id, kind.history()], |row| {
        row.get::<_, String>(0)
    })?;
    rows.map(|row| decode(&row?)).collect()
}

pub(super) fn write(
    con: &Connection,
    matter_id: &str,
    kind: Kind,
    entries: &[Value],
) -> rusqlite::Result<()> {
    con.execute(
        "DELETE FROM bundle_destinations WHERE matter_id=? AND kind=?",
        params![matter_id, kind.history()],
    )?;
    for (position, entry) in entries.iter().enumerate() {
        con.execute(
            "INSERT INTO bundle_destinations(kind,id,matter_id,position,body) VALUES (?,?,?,?,?)",
            params![
                kind.history(),
                entry["id"].as_str(),
                matter_id,
                position,
                entry.to_string()
            ],
        )?;
    }
    Ok(())
}

fn documents_by_id(documents: &[Value]) -> HashMap<&str, &Value> {
    documents
        .iter()
        .filter_map(|doc| Some((doc["id"].as_str()?, doc)))
        .collect()
}

fn current_destination(
    documents: &HashMap<&str, &Value>,
    id: &Value,
    page: &Value,
    revision: &Value,
) -> bool {
    id.as_str()
        .and_then(|id| documents.get(id))
        .is_some_and(|doc| {
            doc["revision"] == *revision
                && doc["archived_at"].is_null()
                && page
                    .as_u64()
                    .zip(doc["page_count"].as_u64())
                    .is_some_and(|(page, count)| page < count)
        })
}

pub(super) fn resolve(entries: &[Value], documents: &[Value], kind: Kind) -> Vec<Value> {
    let documents = documents_by_id(documents);
    entries
        .iter()
        .map(|entry| {
            let mut result = entry.clone();
            result["resolved"] = json!(match kind {
                Kind::Bookmark => current_destination(
                    &documents,
                    &entry["document_id"],
                    &entry["page_index"],
                    &entry["target_revision"]
                ),
                Kind::Link =>
                    current_destination(
                        &documents,
                        &entry["source_document_id"],
                        &entry["source_page_index"],
                        &entry["source_revision"]
                    ) && (entry["kind"] == "external"
                        || current_destination(
                            &documents,
                            &entry["target_document_id"],
                            &entry["target_page_index"],
                            &entry["target_revision"]
                        )),
            });
            result
        })
        .collect()
}

pub(super) fn parse(kind: Kind, body: &Value) -> Result<Vec<Value>, Reply> {
    let invalid = || {
        artifacts::refusal(
            422,
            format!("Invalid {} request", kind.label().to_lowercase()),
        )
    };
    let entries = body["entries"]
        .as_array()
        .filter(|entries| entries.len() <= 500)
        .ok_or_else(invalid)?;
    entries
        .iter()
        .map(|entry| {
            let value = match kind {
                Kind::Bookmark => {
                    let bookmark = Bookmark::deserialize(entry)
                        .ok()
                        .filter(|item| item.target_revision > 0)
                        .ok_or_else(invalid)?;
                    serde_json::to_value(bookmark)
                }
                Kind::Link => {
                    let link = Link::deserialize(entry).map_err(|_| invalid())?;
                    link.validate()
                        .map_err(|message| artifacts::refusal(422, message))?;
                    serde_json::to_value(link)
                }
            }
            .map_err(|_| invalid())?;
            if !value["id"].as_str().is_some_and(artifacts::valid_id)
                || !value["title"]
                    .as_str()
                    .is_some_and(|title| (1..=200).contains(&title.chars().count()))
            {
                return Err(invalid());
            }
            Ok(value)
        })
        .collect()
}

const LINK_ANCHOR: &[&str] = &[
    "source_document_id",
    "source_page_index",
    "source_revision",
    "rect_x",
    "rect_y",
    "rect_width",
    "rect_height",
    "kind",
    "target_document_id",
    "target_page_index",
    "target_revision",
    "uri",
];

fn normalize(
    con: &Connection,
    matter_id: &str,
    kind: Kind,
    documents: &[Value],
    old: &[Value],
    mut entries: Vec<Value>,
) -> rusqlite::Result<Result<Vec<Value>, Reply>> {
    let ids: BTreeSet<_> = entries
        .iter()
        .map(|entry| entry["id"].as_str().unwrap())
        .collect();
    if ids.len() != entries.len() {
        let label = match kind {
            Kind::Bookmark => "Manual bookmark",
            Kind::Link => "Reviewed link",
        };
        return Ok(Err(artifacts::refusal(
            422,
            format!("{label} IDs must be unique"),
        )));
    }
    for id in ids {
        let foreign: bool = con.query_row(
            "SELECT EXISTS(SELECT 1 FROM bundle_destinations WHERE kind=? AND id=? AND matter_id<>?)",
            params![kind.history(), id, matter_id], |row| row.get(0),
        )?;
        if foreign {
            return Ok(Err(artifacts::refusal(
                422,
                format!("{} ID already belongs to another matter", kind.label()),
            )));
        }
    }
    let docs = documents_by_id(documents);
    let prior: HashMap<_, _> = old
        .iter()
        .map(|entry| (entry["id"].as_str(), entry))
        .collect();
    let mut seen = BTreeSet::new();
    let mut children: HashMap<Option<String>, Vec<usize>> = HashMap::new();
    for (index, entry) in entries.iter_mut().enumerate() {
        let unchanged = |keys: &[&str]| {
            prior
                .get(&entry["id"].as_str())
                .is_some_and(|previous| keys.iter().all(|&key| previous[key] == entry[key]))
        };
        let refusal = match kind {
            Kind::Bookmark => {
                let current = current_destination(
                    &docs,
                    &entry["document_id"],
                    &entry["page_index"],
                    &entry["target_revision"],
                );
                let exists = entry["document_id"]
                    .as_str()
                    .is_some_and(|id| docs.contains_key(id));
                let parent = entry["parent_id"].as_str();
                if !exists
                    || (!current && !unchanged(&["document_id", "page_index", "target_revision"]))
                {
                    Some("Choose a current page in an active document")
                } else if parent.is_some_and(|id| !seen.contains(id)) {
                    Some("Bookmark parent must precede its child")
                } else if entry["title"].as_str().unwrap().trim().is_empty() {
                    Some("Bookmark title must contain visible text")
                } else {
                    children
                        .entry(parent.map(str::to_owned))
                        .or_default()
                        .push(index);
                    seen.insert(entry["id"].as_str().unwrap().to_owned());
                    None
                }
            }
            Kind::Link => {
                if unchanged(LINK_ANCHOR) {
                    None
                } else if !current_destination(
                    &docs,
                    &entry["source_document_id"],
                    &entry["source_page_index"],
                    &entry["source_revision"],
                ) {
                    Some("Choose a current source region in an active document")
                } else if entry["kind"] == "internal"
                    && !current_destination(
                        &docs,
                        &entry["target_document_id"],
                        &entry["target_page_index"],
                        &entry["target_revision"],
                    )
                {
                    Some("Choose a current target page in an active document")
                } else {
                    None
                }
            }
        };
        if let Some(message) = refusal {
            return Ok(Err(artifacts::refusal(422, message)));
        }
    }
    if matches!(kind, Kind::Bookmark) {
        let mut stack: Vec<_> = children
            .remove(&None)
            .unwrap_or_default()
            .into_iter()
            .rev()
            .collect();
        let mut ordered = Vec::with_capacity(entries.len());
        while let Some(index) = stack.pop() {
            let entry = std::mem::take(&mut entries[index]);
            let key = entry["id"].as_str().map(str::to_owned);
            ordered.push(entry);
            if let Some(list) = children.remove(&key) {
                stack.extend(list.into_iter().rev());
            }
        }
        entries = ordered;
    }
    for (position, entry) in entries.iter_mut().enumerate() {
        entry["position"] = json!(position);
    }
    Ok(Ok(entries))
}

pub(super) fn replay(
    con: &Connection,
    id: &str,
    kind: &str,
    target: &Value,
    expected: &Value,
) -> rusqlite::Result<Option<Reply>> {
    let kind = Kind::from_history(kind).ok_or(rusqlite::Error::InvalidQuery)?;
    let refusals = || {
        Some(artifacts::refusal(
            409,
            format!(
                "{} state changed; this edit cannot be restored",
                kind.label()
            ),
        ))
    };
    let before = load(con, id, kind)?;
    if expected["entries"] != json!(before) {
        return Ok(refusals());
    }
    let Some(entries) = target["entries"].as_array() else {
        return Ok(refusals());
    };
    for (position, entry) in entries.iter().enumerate() {
        let Some(entry_id) = entry["id"]
            .as_str()
            .filter(|value| artifacts::valid_id(value))
        else {
            return Ok(refusals());
        };
        let foreign: bool = con.query_row(
            "SELECT EXISTS(SELECT 1 FROM bundle_destinations WHERE kind=? AND id=? AND matter_id<>?)",
            params![kind.history(), entry_id, id], |row| row.get(0),
        )?;
        if foreign || entry["position"] != position {
            return Ok(refusals());
        }
    }
    write(con, id, kind, entries)?;
    Ok(None)
}

impl Store {
    pub(super) fn destination_request(
        &mut self,
        method: &str,
        id: &str,
        kind: Kind,
        body: &Value,
    ) -> rusqlite::Result<Option<Reply>> {
        if !["GET", "PUT"].contains(&method) {
            return Ok(None);
        }
        let entries = if method == "PUT" {
            let result = match parse(kind, body) {
                Ok(entries) => entries,
                Err(refusal) => return Ok(Some(refusal)),
            };
            Some(result)
        } else {
            None
        };
        let expected = if matches!(kind, Kind::Bookmark) && method == "PUT" {
            match body.get("expected_snapshot") {
                None | Some(Value::Null) => None,
                Some(Value::String(value)) if value.chars().count() == 64 => Some(value.as_str()),
                _ => {
                    return Ok(Some(artifacts::refusal(
                        422,
                        "Invalid bookmark input snapshot",
                    )))
                }
            }
        } else {
            None
        };
        let tx = self.con.transaction_with_behavior(if method == "PUT" {
            TransactionBehavior::Immediate
        } else {
            TransactionBehavior::Deferred
        })?;
        let Some(_) = document_metadata::matter(&tx, id)? else {
            return Ok(Some(artifacts::refusal(404, "Matter not found")));
        };
        if let Some(expected) = expected {
            let inputs = match Inputs::read(&tx, &self.root, self.key.as_ref(), id)? {
                Ok(inputs) => inputs,
                Err(refusal) => return Ok(Some(refusal)),
            };
            if inputs.digest(&tx)? != expected {
                return Ok(Some(artifacts::refusal(
                    409,
                    "Matter changed; discard and reload bookmarks before saving",
                )));
            }
        }
        let mut documents = Vec::new();
        let mut query = tx.prepare("SELECT body FROM documents WHERE matter_id=?")?;
        for row in query.query_map([id], |row| row.get::<_, String>(0))? {
            documents.push(decode(&row?)?);
        }
        drop(query);
        let before = load(&tx, id, kind)?;
        let after = if let Some(entries) = entries {
            let after = match normalize(&tx, id, kind, &documents, &before, entries)? {
                Ok(entries) => entries,
                Err(refusal) => return Ok(Some(refusal)),
            };
            if before != after {
                let stamp = now();
                write(&tx, id, kind, &after)?;
                record_edit(
                    &tx,
                    id,
                    kind.history(),
                    &json!({"entries":before}),
                    &json!({"entries":after}),
                    &stamp,
                )?;
                tx.execute(
                    "INSERT INTO audit_events(matter_id,action,created_at) VALUES (?,?,?)",
                    params![id, kind.audit(), stamp],
                )?;
                document_metadata::touch(&tx, id, &stamp)?;
            }
            after
        } else {
            before
        };
        let result = resolve(&after, &documents, kind);
        tx.commit()?;
        Ok(Some(reply(200, json!(result))))
    }
}
