use super::*;

const POSITIONS: &[&str] = &[
    "top_left",
    "top_center",
    "top_right",
    "center",
    "bottom_left",
    "bottom_center",
    "bottom_right",
];
const ANNEXURES: &[&str] = &["none", "label_stamp", "separator"];
const MATTER_OVERRIDES: &[&str] = &[
    "volume_splits",
    "cover_heading",
    "cover_reference",
    "cover_title",
    "annexure_overrides",
];

fn invalid(message: impl Into<String>) -> Reply {
    artifacts::refusal(422, message)
}

fn enum_value(value: &Value, choices: &[&str]) -> bool {
    value.as_str().is_some_and(|value| choices.contains(&value))
}

fn text(value: &Value, max: usize) -> bool {
    value
        .as_str()
        .is_some_and(|text| text.chars().count() <= max)
}

fn integer_value(value: &Value, max: i64) -> Option<Value> {
    integer(value)
        .filter(|value| (1..=max).contains(value))
        .map(|value| json!(value))
}

fn float_value(value: &Value, min: f64, max: f64) -> Option<Value> {
    let value = value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse::<f64>().ok())?;
    (min..=max).contains(&value).then(|| json!(value))
}

fn splits(value: &Value) -> Option<Value> {
    let objects = value.as_object().filter(|values| values.len() <= 1000)?;
    let mut normalized = Map::new();
    for (id, split) in objects {
        if !artifacts::valid_id(id)
            || !split
                .as_object()?
                .keys()
                .all(|key| ["revision", "after_pages"].contains(&key.as_str()))
        {
            return None;
        }
        let revision = integer(&split["revision"]).filter(|value| *value > 0)?;
        let list = split["after_pages"]
            .as_array()
            .filter(|list| (1..=2000).contains(&list.len()))?;
        let pages = list.iter().map(integer).collect::<Option<Vec<_>>>()?;
        if pages[0] < 1 || pages.windows(2).any(|pair| pair[0] >= pair[1]) {
            return None;
        }
        normalized.insert(id.clone(), json!({"revision":revision,"after_pages":pages}));
    }
    Some(Value::Object(normalized))
}

fn request_defaults() -> Value {
    let mut result = marks();
    for (key, value) in [
        ("watermark_font_size", 32),
        ("timestamp_font_size", 8),
        ("bates_font_size", 8),
        ("annexure_stamp_font_size", 10),
    ] {
        result[key] = json!(value);
    }
    result
}

fn equivalent(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => {
            a == b || ((a.is_f64() || b.is_f64()) && a.as_f64() == b.as_f64())
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(left, right)| equivalent(left, right))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, left)| b.get(key).is_some_and(|right| equivalent(left, right)))
        }
        _ => a == b,
    }
}

pub(super) fn parse_marks(body: &Value) -> Result<Value, Reply> {
    let mut result = request_defaults();
    let input = body
        .as_object()
        .ok_or_else(|| invalid("Expected output marks"))?;
    for (key, value) in input {
        let preserved = |valid: bool| valid.then(|| value.clone());
        let normalized = match key.as_str() {
            "volume_max_bytes" if value.is_null() => Some(Value::Null),
            "volume_max_bytes" => integer_value(value, 10_000_000_000),
            "volume_splits" => splits(value),
            "processing_quality" => preserved(enum_value(
                value,
                &["original", "lossless", "balanced", "small"],
            )),
            "print_layout" => preserved(value == "screen"),
            "numbering_style" => {
                preserved(enum_value(value, &["arabic", "roman_lower", "roman_upper"]))
            }
            "numbering_restart" => {
                preserved(enum_value(value, &["volume", "continuous", "section"]))
            }
            "numbering_start" => integer_value(value, 3999),
            "numbering_prefix" => preserved(value.as_str().is_some_and(|prefix| {
                let literal = prefix.replace("{volume}", "").replace("{section}", "");
                text(value, 24)
                    && !literal.contains(['{', '}'])
                    && prefix
                        .chars()
                        .all(|character| (' '..='ÿ').contains(&character))
            })),
            "numbering_position" => preserved(enum_value(
                value,
                &[
                    "bottom_left",
                    "bottom_center",
                    "bottom_right",
                    "top_left",
                    "top_right",
                ],
            )),
            "numbering_front_matter" => {
                preserved(enum_value(value, &["physical", "roman", "none"]))
            }
            "index_style" => preserved(enum_value(value, &["compact", "readable", "review"])),
            "front_matter" => preserved(enum_value(value, &["index_only", "cover_index"])),
            "cover_heading" | "cover_title" => preserved(text(value, 200)),
            "cover_reference" => preserved(text(value, 100)),
            "watermark_text" | "timestamp_prefix" | "bates_prefix" => {
                let limit = match key.as_str() {
                    "watermark_text" => 120,
                    "timestamp_prefix" => 40,
                    _ => 24,
                };
                preserved(
                    text(value, limit)
                        && value
                            .as_str()
                            .unwrap()
                            .bytes()
                            .all(|byte| (32..=126).contains(&byte)),
                )
            }
            "watermark_position"
            | "timestamp_position"
            | "bates_position"
            | "annexure_stamp_position" => preserved(enum_value(value, POSITIONS)),
            "watermark_font_size" => float_value(value, 8.0, 72.0),
            "watermark_opacity" => float_value(value, 0.05, 0.8),
            "timestamp_format" => preserved(enum_value(value, &["date", "datetime"])),
            "timestamp_timezone" => preserved(value.as_str().is_some_and(|zone| {
                !zone.is_empty()
                    && zone.len() <= 64
                    && zone.split('/').all(|piece| {
                        !piece.is_empty()
                            && piece != "."
                            && piece != ".."
                            && piece
                                .bytes()
                                .all(|byte| byte.is_ascii_alphanumeric() || b"_+-".contains(&byte))
                    })
                    && jiff::tz::TimeZone::get(zone).is_ok()
            })),
            "timestamp_font_size" | "bates_font_size" | "annexure_stamp_font_size" => {
                float_value(value, 6.0, 24.0)
            }
            "bates_start" => integer_value(value, 999_999_999),
            "bates_padding" => integer_value(value, 12),
            "annexure_mode" => preserved(enum_value(value, ANNEXURES)),
            "annexure_stamp_scope" => preserved(enum_value(value, &["first", "all"])),
            "annexure_overrides" => preserved(value.as_object().is_some_and(|overrides| {
                overrides.len() <= 1000
                    && overrides
                        .iter()
                        .all(|(id, mode)| artifacts::valid_id(id) && enum_value(mode, ANNEXURES))
            })),
            key if result[key].is_boolean() => boolean(value).map(|value| json!(value)),
            _ => None,
        };
        let value = normalized.ok_or_else(|| invalid(format!("Invalid output mark {key}")))?;
        result[key] = value;
    }
    if result["watermark_enabled"] == true
        && result["watermark_text"].as_str().unwrap().trim().is_empty()
    {
        return Err(invalid("Enabled watermark requires visible text"));
    }
    Ok(result)
}

pub(super) fn load_state(con: &Connection, id: &str) -> rusqlite::Result<(Value, Value)> {
    let mut output = marks();
    let mut preset = Value::Null;
    if let Some((saved, applied)) = con
        .query_row(
            "SELECT marks, preset FROM bundle_settings WHERE matter_id=?",
            [id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?
    {
        output.as_object_mut().unwrap().extend(
            decode(&saved)?
                .as_object()
                .ok_or(rusqlite::Error::InvalidQuery)?
                .clone(),
        );
        preset = decode(&applied)?;
    }
    Ok((output, preset))
}

pub(super) fn save(
    con: &Connection,
    id: &str,
    marks: &Value,
    preset: &Value,
) -> rusqlite::Result<()> {
    con.execute(
        "INSERT INTO bundle_settings(matter_id,marks,preset) VALUES (?,?,?)
         ON CONFLICT(matter_id) DO UPDATE SET marks=excluded.marks,preset=excluded.preset",
        params![id, marks.to_string(), preset.to_string()],
    )?;
    Ok(())
}

fn state(matter: &Value, marks: &Value, preset: &Value) -> Value {
    json!({"max_pages_per_volume":matter["max_pages_per_volume"],"keep_groups_intact":matter["keep_groups_intact"],
        "output_marks":marks,"preset":preset})
}

fn apply(
    con: &Connection,
    id: &str,
    matter: &mut Value,
    next: &Value,
    stamp: &str,
) -> rusqlite::Result<()> {
    matter["max_pages_per_volume"] = next["max_pages_per_volume"].clone();
    matter["keep_groups_intact"] = next["keep_groups_intact"].clone();
    matter["updated_at"] = json!(stamp);
    con.execute(
        "UPDATE matters SET body=?,updated_at=? WHERE id=?",
        params![matter.to_string(), stamp, id],
    )?;
    save(con, id, &next["output_marks"], &next["preset"])
}

pub(in crate::matters) fn replay(
    con: &Connection,
    id: &str,
    matter: &mut Value,
    kind: &str,
    target: &Value,
    expected: &Value,
) -> rusqlite::Result<Option<Reply>> {
    let mismatch = || {
        Some(artifacts::refusal(
            409,
            "Output settings changed; this edit cannot be restored",
        ))
    };
    let (marks, preset) = load_state(con, id)?;
    if kind == "output_marks" {
        if expected["settings"] != marks || parse_marks(&target["settings"]).is_err() {
            return Ok(mismatch());
        }
        save(con, id, &target["settings"], &preset)?;
    } else {
        if *expected != state(matter, &marks, &preset)
            || parse_marks(&target["output_marks"]).is_err()
            || target["max_pages_per_volume"]
                .as_u64()
                .is_none_or(|cap| !(2..=2000).contains(&cap))
            || !target["keep_groups_intact"].is_boolean()
        {
            return Ok(mismatch());
        }
        apply(con, id, matter, target, &now())?;
    }
    Ok(None)
}

fn get_preset(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    Ok(
        json!({"id":row.get::<_, String>(0)?,"name":row.get::<_, String>(1)?,"version":row.get::<_, u64>(2)?,
        "settings":decode(&row.get::<_, String>(3)?)?,"created_at":row.get::<_, String>(4)?}),
    )
}

fn options(body: &Value) -> Option<Preview> {
    Some(Preview {
        max_pages_per_volume: integer(&body["max_pages_per_volume"])
            .filter(|cap| (2..=2000).contains(cap))? as usize,
        keep_groups_intact: boolean(&body["keep_groups_intact"])?,
    })
}

pub(super) fn layout(body: &Value) -> Result<(Value, Preview), Reply> {
    let marks = parse_marks(&body["output_marks"])?;
    let valid_fields = body.as_object().is_some_and(|fields| {
        fields.keys().all(|key| {
            [
                "max_pages_per_volume",
                "keep_groups_intact",
                "output_marks",
                "expected_snapshot",
            ]
            .contains(&key.as_str())
        })
    });
    if !valid_fields {
        return Err(invalid("Invalid bundle settings fields"));
    }
    let settings = options(body).ok_or_else(|| invalid("Invalid bundle settings request"))?;
    Ok((marks, settings))
}

pub(super) fn layout_snapshot(body: &Value) -> Result<&str, Reply> {
    body["expected_snapshot"]
        .as_str()
        .filter(|value| value.chars().count() == 64)
        .ok_or_else(|| invalid("Invalid settings input snapshot"))
}

fn references(
    con: &Connection,
    id: &str,
    marks: &Value,
    check_splits: bool,
) -> rusqlite::Result<Option<Reply>> {
    let mut query = con.prepare("SELECT id FROM documents WHERE matter_id=?")?;
    let ids = query
        .query_map([id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?;
    for (option, message) in [
        (
            "volume_splits",
            "Split exceptions must target documents in this matter",
        ),
        (
            "annexure_overrides",
            "Annexure exceptions must target documents in this matter",
        ),
    ] {
        if (option != "volume_splits" || check_splits)
            && marks[option]
                .as_object()
                .unwrap()
                .keys()
                .any(|id| !ids.contains(id))
        {
            return Ok(Some(invalid(message)));
        }
    }
    Ok(None)
}

fn preview_settings(inputs: &Inputs, con: &Connection) -> rusqlite::Result<Value> {
    let mut response = select(
        &inputs.owner,
        &["max_pages_per_volume", "keep_groups_intact"],
    );
    response["output_marks"] = inputs.output_marks.clone();
    response["applied"] = inputs.preset.clone();
    response["input_snapshot"] = json!(inputs.digest(con)?);
    Ok(response)
}

pub(super) fn preset_settings(body: &Value) -> Result<(String, Value), Reply> {
    let Some(mut fields) = body.as_object().cloned() else {
        return Err(invalid("Expected a preset object"));
    };
    if !fields.keys().all(|key| {
        [
            "name",
            "max_pages_per_volume",
            "keep_groups_intact",
            "output_marks",
        ]
        .contains(&key.as_str())
    }) {
        return Err(invalid("Invalid preset fields"));
    }
    let body = &mut fields;
    let value = body.get("name").and_then(Value::as_str);
    let Some(name) = value
        .filter(|name| (1..=80).contains(&name.chars().count()))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
    else {
        return Err(invalid("Preset name is required"));
    };
    body.entry("keep_groups_intact").or_insert(json!(true));
    let Some(options) = options(&json!(body)) else {
        return Err(invalid("Invalid preset page cap or group policy"));
    };
    let output = body
        .get("output_marks")
        .map(parse_marks)
        .unwrap_or_else(|| Ok(request_defaults()))?;
    let defaults = marks();
    for (fields, message) in [
        (
            &["cover_heading", "cover_reference", "cover_title"][..],
            "Reusable presets cannot contain matter-specific cover details",
        ),
        (
            &["volume_splits"][..],
            "Reusable presets cannot contain document split boundaries",
        ),
        (
            &["annexure_overrides"][..],
            "Reusable presets cannot contain matter-specific document exceptions",
        ),
    ] {
        if fields.iter().any(|&key| output[key] != defaults[key]) {
            return Err(invalid(message));
        }
    }
    let settings = json!({"name":name,"max_pages_per_volume":options.max_pages_per_volume,
            "keep_groups_intact":options.keep_groups_intact, "output_marks":output});
    Ok((name, settings))
}

impl Store {
    fn presets(&mut self, method: &str, body: &Value) -> rusqlite::Result<Option<Reply>> {
        if method == "GET" {
            let mut query = self.con.prepare(
                "SELECT id,name,version,settings,created_at FROM output_presets ORDER BY name COLLATE NOCASE,version DESC",
            )?;
            let rows = query
                .query_map([], get_preset)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            return Ok(Some(reply(200, json!(rows))));
        }
        if method != "POST" {
            return Ok(None);
        }
        let (name, settings) = match preset_settings(body) {
            Ok(parsed) => parsed,
            Err(refusal) => return Ok(Some(refusal)),
        };
        let tx = self
            .con
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: u64 = tx.query_row(
            "SELECT COALESCE(MAX(version),0)+1 FROM output_presets WHERE name=? COLLATE NOCASE",
            [&name],
            |row| row.get(0),
        )?;
        let id = uuid::Uuid::new_v4().simple().to_string();
        let stamp = now();
        tx.execute(
            "INSERT INTO output_presets VALUES (?,?,?,?,?)",
            params![id, name, version, settings.to_string(), stamp],
        )?;
        tx.commit()?;
        Ok(Some(reply(
            201,
            json!({"id":id,"name":name,"version":version,"settings":settings,"created_at":stamp}),
        )))
    }

    pub(super) fn output_settings_request(
        &mut self,
        method: &str,
        route: &[&str],
        body: &Value,
    ) -> rusqlite::Result<Option<Reply>> {
        if route == ["output-presets"] {
            return self.presets(method, body);
        }
        let ["matters", id, resource, rest @ ..] = route else {
            return Ok(None);
        };
        if (method, *resource, rest) == ("POST", "volume-planning", &[][..]) {
            let response = self
                .prepare_volume_planning(id, body)
                .and_then(BundlePreviewWork::measure)
                .and_then(|measured| self.complete_bundle_preview(measured));
            return Ok(Some(response.unwrap_or_else(|error| error)));
        }
        let applies = match (method, *resource, rest) {
            ("POST", "output-preset", [preset_id]) => Some(*preset_id),
            ("GET", "output-preset" | "output-marks" | "bundle-settings", [])
            | ("PUT", "output-marks" | "bundle-settings", []) => None,
            _ => return Ok(None),
        };
        let write_marks = method == "PUT" && *resource == "output-marks";
        let changes_layout = *resource == "bundle-settings" && method == "PUT";
        let (new_marks, options) = if changes_layout {
            match layout(body) {
                Ok((marks, options)) => (Some(marks), Some(options)),
                Err(response) => return Ok(Some(response)),
            }
        } else if write_marks {
            match parse_marks(body) {
                Ok(marks) => (Some(marks), None),
                Err(response) => return Ok(Some(response)),
            }
        } else {
            (None, None)
        };
        let preserve_overrides = if applies.is_some() && !body.is_null() {
            let Some(fields) = body.as_object().filter(|fields| {
                fields
                    .keys()
                    .all(|key| ["preserve_overrides", "expected_snapshot"].contains(&key.as_str()))
            }) else {
                return Ok(Some(invalid("Invalid preset application")));
            };
            match fields
                .get("preserve_overrides")
                .map(boolean)
                .unwrap_or(Some(false))
            {
                Some(value) => value,
                None => return Ok(Some(invalid("Invalid preset override policy"))),
            }
        } else {
            false
        };
        let expected = if changes_layout {
            match layout_snapshot(body) {
                Ok(value) => Some(value),
                Err(response) => return Ok(Some(response)),
            }
        } else if applies.is_some() {
            match body.get("expected_snapshot") {
                Some(Value::String(value)) if value.chars().count() == 64 => Some(value.as_str()),
                None | Some(Value::Null) => None,
                _ => return Ok(Some(invalid("Invalid settings input snapshot"))),
            }
        } else {
            None
        };

        let tx = self
            .con
            .transaction_with_behavior(if method == "PUT" || applies.is_some() {
                TransactionBehavior::Immediate
            } else {
                TransactionBehavior::Deferred
            })?;
        let Some(mut matter) = document_metadata::matter(&tx, id)? else {
            return Ok(Some(artifacts::refusal(404, "Matter not found")));
        };
        let (current_marks, current_preset) = load_state(&tx, id)?;
        if method == "GET" && *resource != "bundle-settings" {
            return Ok(Some(reply(
                200,
                if *resource == "output-marks" {
                    current_marks
                } else {
                    current_preset
                },
            )));
        }
        if write_marks {
            let updated = new_marks.as_ref().unwrap();
            if let Some(refusal) = references(&tx, id, updated, false)? {
                return Ok(Some(refusal));
            }
            if !equivalent(updated, &current_marks) {
                let stamp = now();
                save(&tx, id, updated, &current_preset)?;
                record_edit(
                    &tx,
                    id,
                    "output_marks",
                    &json!({"settings":current_marks}),
                    &json!({"settings":updated}),
                    &stamp,
                )?;
                tx.execute("INSERT INTO audit_events(matter_id,action,created_at) VALUES (?,'output_marks_updated',?)", params![id, stamp])?;
                document_metadata::touch(&tx, id, &stamp)?;
            }
            tx.commit()?;
            return Ok(Some(reply(200, updated.clone())));
        }
        let mut inputs = match Inputs::read(&tx, &self.root, self.key.as_ref(), id)? {
            Ok(inputs) => inputs,
            Err(refusal) => return Ok(Some(refusal)),
        };
        if method == "GET" {
            return Ok(Some(reply(200, preview_settings(&inputs, &tx)?)));
        }
        let snapshot = inputs.digest(&tx)?;
        if expected.is_some_and(|expected| expected != snapshot) {
            let message = if applies.is_some() {
                "Matter changed; reload settings before applying a preset"
            } else {
                "Matter changed; reload settings and review your changes"
            };
            return Ok(Some(artifacts::refusal(409, message)));
        }
        let before = state(&matter, &current_marks, &current_preset);
        let (after, action) = if let Some(preset_id) = applies {
            let Some(mut preset) = tx
                .query_row(
                    "SELECT id,name,version,settings,created_at FROM output_presets WHERE id=?",
                    [preset_id],
                    get_preset,
                )
                .optional()?
            else {
                return Ok(Some(artifacts::refusal(404, "Output preset not found")));
            };
            preset["provenance"] = json!("Local user preset");
            let mut after = state(
                &preset["settings"],
                &preset["settings"]["output_marks"],
                &preset,
            );
            for &key in MATTER_OVERRIDES {
                after["output_marks"][key] = current_marks[key].clone();
            }
            if preserve_overrides && !current_preset.is_null() {
                for key in ["max_pages_per_volume", "keep_groups_intact"] {
                    if before[key] != current_preset["settings"][key] {
                        after[key] = before[key].clone();
                    }
                }
                for (key, value) in current_marks.as_object().unwrap() {
                    if !equivalent(value, &current_preset["settings"]["output_marks"][key]) {
                        after["output_marks"][key] = value.clone();
                    }
                }
            }
            (after, "output_preset_applied")
        } else {
            let options = options.unwrap();
            let new_marks = new_marks.unwrap();
            let (documents, _) = inputs.planner_documents()?;
            let settings: planner::Settings =
                serde_json::from_value(new_marks.clone()).map_err(json_sql)?;
            if let Err(message) =
                planner::validate_splits(&documents, options.keep_groups_intact, &settings)
            {
                return Ok(Some(invalid(message)));
            }
            if let Some(refusal) = references(&tx, id, &new_marks, true)? {
                return Ok(Some(refusal));
            }
            (
                state(&json!(options), &new_marks, &current_preset),
                "bundle_settings_updated",
            )
        };
        if !equivalent(&before, &after) {
            let stamp = now();
            apply(&tx, id, &mut matter, &after, &stamp)?;
            record_edit(&tx, id, "output_preset", &before, &after, &stamp)?;
            tx.execute(
                "INSERT INTO audit_events(matter_id,action,created_at) VALUES (?,?,?)",
                params![id, action, stamp],
            )?;
        }
        inputs.owner = matter;
        (inputs.output_marks, inputs.preset) = load_state(&tx, id)?;
        let result = if applies.is_some() {
            inputs.preset.clone()
        } else {
            preview_settings(&inputs, &tx)?
        };
        tx.commit()?;
        Ok(Some(reply(200, result)))
    }
}
