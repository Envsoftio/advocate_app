use super::{integer, integer_error, iso_date, reply, validate_review_fields, Reply};
use crate::{
    matters::preparation::validation::{array_issue, issue, string_issue},
    request_order::ordered_fields,
    RequestOrder,
};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::{collections::HashSet, sync::LazyLock};

const DATE_PATTERN: &str = r"^(|\d{4}-\d{2}-\d{2})$";
static DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(DATE_PATTERN).expect("document date pattern"));

enum Field {
    Text(usize, usize),
    Literal(&'static [&'static str]),
    Part,
    Date,
}

use Field::{Date, Literal, Part, Text};

const KINDS: &[&str] = &[
    "petition",
    "affidavit",
    "order",
    "annexure",
    "application",
    "evidence",
    "other",
];
const SORT_MODES: &[&str] = &["manual", "upload", "filename", "document_date"];
const PATCH: &[(&str, Field)] = &[
    ("title", Text(1, 250)),
    ("notes", Text(0, 2000)),
    ("kind", Literal(KINDS)),
    ("part", Part),
    ("label", Text(0, 60)),
    ("label_status", Literal(&["unreviewed", "confirmed"])),
    (
        "label_source",
        Literal(&["none", "legacy", "user", "proposal", "filename"]),
    ),
    ("group_key", Text(0, 100)),
    ("group_role", Literal(&["member", "parent"])),
    ("document_date", Date),
    (
        "date_status",
        Literal(&["unknown", "ambiguous", "suggested", "confirmed"]),
    ),
    (
        "date_source",
        Literal(&["none", "user", "extracted", "filename", "filesystem"]),
    ),
    ("date_note", Text(0, 120)),
    (
        "visible_signature_status",
        Literal(&["unreviewed", "present", "absent"]),
    ),
];
const BATCH: &[(&str, Field)] = &[
    ("kind", Literal(KINDS)),
    ("part", Part),
    ("group_key", Text(0, 100)),
];

fn location(base: &[Value], key: impl Into<Value>) -> Vec<Value> {
    let mut loc = base.to_vec();
    loc.push(key.into());
    loc
}

fn with_context(mut error: Value, context: Value) -> Value {
    error["ctx"] = context;
    error
}

fn invalid(errors: Vec<Value>) -> Reply {
    reply(422, json!({"detail": errors}))
}

fn collected<T>(errors: &mut Vec<Value>, value: Result<T, Value>) -> Option<T> {
    match value {
        Ok(value) => Some(value),
        Err(error) => {
            errors.push(error);
            None
        }
    }
}

fn value_error(loc: &[Value], input: &Value, message: &str) -> Value {
    with_context(
        issue(
            "value_error",
            json!(loc),
            &format!("Value error, {message}"),
            input.clone(),
        ),
        json!({"error": {}}),
    )
}

fn object<'a>(
    input: &'a Value,
    loc: &[Value],
    class: Option<&str>,
) -> Result<&'a Map<String, Value>, Value> {
    input.as_object().ok_or_else(|| {
        if let Some(class) = class {
            with_context(
                issue(
                    "model_type",
                    json!(loc),
                    &format!("Input should be a valid dictionary or instance of {class}"),
                    input.clone(),
                ),
                json!({"class_name": class}),
            )
        } else if input.is_null() {
            issue("missing", json!(loc), "Field required", Value::Null)
        } else {
            issue(
                "model_attributes_type",
                json!(loc),
                "Input should be a valid dictionary or object to extract fields from",
                input.clone(),
            )
        }
    })
}

fn required<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    base: &[Value],
    input: &Value,
) -> Result<&'a Value, Value> {
    object.get(key).ok_or_else(|| {
        issue(
            "missing",
            json!(location(base, key)),
            "Field required",
            input.clone(),
        )
    })
}

fn extras(
    object: &Map<String, Value>,
    base: &[Value],
    order: &[String],
    allowed: impl Fn(&str) -> bool,
    errors: &mut Vec<Value>,
) {
    errors.extend(
        ordered_fields(object, order)
            .into_iter()
            .filter_map(|(key, value)| {
                (!allowed(key)).then(|| {
                    issue(
                        "extra_forbidden",
                        json!(location(base, key)),
                        "Extra inputs are not permitted",
                        value.clone(),
                    )
                })
            }),
    );
}

fn array<'a>(input: &'a Value, loc: &[Value], max: usize) -> Result<&'a [Value], Value> {
    array_issue(input, json!(loc), 1, max).map(Vec::as_slice)
}

fn text<'a>(input: &'a Value, loc: &[Value], min: usize, max: usize) -> Result<&'a str, Value> {
    string_issue(input, json!(loc), min, max)
}

pub(in crate::matters) fn literal<'a>(
    input: &'a Value,
    loc: &[Value],
    options: &[&str],
) -> Result<&'a str, Value> {
    if let Some(value) = input.as_str().filter(|value| options.contains(value)) {
        return Ok(value);
    }
    let mut quoted = options
        .iter()
        .map(|option| format!("'{option}'"))
        .collect::<Vec<_>>();
    let last = quoted.pop().expect("nonempty literal options");
    let expected = if quoted.is_empty() {
        last
    } else {
        format!("{} or {last}", quoted.join(", "))
    };
    Err(with_context(
        issue(
            "literal_error",
            json!(loc),
            &format!("Input should be {expected}"),
            input.clone(),
        ),
        json!({"expected": expected}),
    ))
}

fn part(input: &Value, loc: &[Value]) -> Result<Value, Value> {
    let number = integer(input).ok_or_else(|| {
        let (kind, msg) = integer_error(input);
        issue(kind, json!(loc), msg, input.clone())
    })?;
    if number < 1 {
        Err(with_context(
            issue(
                "greater_than_equal",
                json!(loc),
                "Input should be greater than or equal to 1",
                input.clone(),
            ),
            json!({"ge": 1}),
        ))
    } else if number > 99 {
        Err(with_context(
            issue(
                "less_than_equal",
                json!(loc),
                "Input should be less than or equal to 99",
                input.clone(),
            ),
            json!({"le": 99}),
        ))
    } else {
        Ok(json!(number))
    }
}

fn date(input: &Value, loc: &[Value]) -> Result<(), Value> {
    let value = text(input, loc, 0, usize::MAX)?;
    if !DATE.is_match(value) {
        return Err(with_context(
            issue(
                "string_pattern_mismatch",
                json!(loc),
                &format!("String should match pattern '{DATE_PATTERN}'"),
                input.clone(),
            ),
            json!({"pattern": DATE_PATTERN}),
        ));
    }
    if value.is_empty() || iso_date(value) {
        return Ok(());
    }
    let message = if !value.is_ascii() {
        return Err(value_error(
            loc,
            input,
            &format!("Invalid isoformat string: '{value}'"),
        ));
    } else if &value[..4] == "0000" {
        "year 0 is out of range"
    } else if !value[5..7]
        .parse::<u8>()
        .is_ok_and(|month| (1..=12).contains(&month))
    {
        "month must be in 1..12"
    } else {
        "day is out of range for month"
    };
    Err(value_error(loc, input, message))
}

fn fields(
    object: &Map<String, Value>,
    base: &[Value],
    spec: &[(&str, Field)],
    errors: &mut Vec<Value>,
) -> Map<String, Value> {
    let mut normalized = Map::new();
    for &(key, ref field) in spec {
        let Some(input) = object.get(key).filter(|value| !value.is_null()) else {
            continue;
        };
        let loc = location(base, key);
        let value = match field {
            Text(min, max) => text(input, &loc, *min, *max).map(|_| input.clone()),
            Literal(options) => literal(input, &loc, options).map(|_| input.clone()),
            Part => part(input, &loc),
            Date => date(input, &loc).map(|_| input.clone()),
        };
        if let Some(value) = collected(errors, value) {
            normalized.insert(key.into(), value);
        }
    }
    normalized
}

fn document_patch(
    input: &Value,
    base: &[Value],
    keys: &[String],
    class: Option<&str>,
    errors: &mut Vec<Value>,
) -> Option<Map<String, Value>> {
    let object = collected(errors, object(input, base, class))?;
    let before = errors.len();
    let result = fields(object, base, PATCH, errors);
    extras(object, base, keys, is_patch_field, errors);
    if errors.len() == before {
        if let Err(message) = validate_review_fields(&result) {
            errors.push(value_error(base, input, message));
        }
    }
    Some(result)
}

pub(super) fn is_patch_field(key: &str) -> bool {
    PATCH.iter().any(|(field, _)| *field == key)
}

pub(super) fn patch(body: &Value, keys: &[String]) -> Result<Map<String, Value>, Reply> {
    let mut errors = Vec::new();
    let fields = document_patch(body, &[json!("body")], keys, None, &mut errors);
    fields
        .filter(|_| errors.is_empty())
        .ok_or_else(|| invalid(errors))
}

pub(super) fn batch<'a>(
    body: &'a Value,
    keys: &[String],
) -> Result<(Vec<&'a str>, Map<String, Value>), Reply> {
    let base = [json!("body")];
    let object = object(body, &base, None).map_err(|error| invalid(vec![error]))?;
    let ids_loc = location(&base, "ids");
    let mut errors = Vec::new();
    let ids = required(object, "ids", &base, body).and_then(|input| array(input, &ids_loc, 1000));
    let ids = collected(&mut errors, ids)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, input)| {
            collected(
                &mut errors,
                text(input, &location(&ids_loc, index), 0, usize::MAX),
            )
        })
        .collect::<Vec<_>>();
    let fields = fields(object, &base, BATCH, &mut errors);
    extras(
        object,
        &base,
        keys,
        |key| key == "ids" || BATCH.iter().any(|(field, _)| *field == key),
        &mut errors,
    );
    if !errors.is_empty() {
        return Err(invalid(errors));
    }
    if fields.is_empty() {
        return Err(invalid(vec![value_error(
            &base,
            body,
            "Choose at least one batch field",
        )]));
    }
    if ids.iter().copied().collect::<HashSet<_>>().len() != ids.len() {
        return Err(invalid(vec![value_error(
            &base,
            body,
            "Select each document once",
        )]));
    }
    Ok((ids, fields))
}

pub(super) fn sort<'a>(body: &'a Value, keys: &[String]) -> Result<&'a str, Reply> {
    let base = [json!("body")];
    let object = object(body, &base, None).map_err(|error| invalid(vec![error]))?;
    let mut errors = Vec::new();
    let mode = required(object, "mode", &base, body)
        .and_then(|input| literal(input, &location(&base, "mode"), SORT_MODES));
    let mode = collected(&mut errors, mode);
    if let Some(value) = object.get("unknown_dates") {
        collected(
            &mut errors,
            literal(value, &location(&base, "unknown_dates"), &["last"]),
        );
    }
    extras(
        object,
        &base,
        keys,
        |key| matches!(key, "mode" | "unknown_dates"),
        &mut errors,
    );
    mode.filter(|_| errors.is_empty())
        .ok_or_else(|| invalid(errors))
}

pub(in crate::matters) struct ReviewRow {
    pub document_id: String,
    pub changes: Map<String, Value>,
}

pub(in crate::matters) struct Save {
    pub expected_snapshot: String,
    pub rows: Vec<ReviewRow>,
}

pub(in crate::matters) fn save(body: &Value, order: &RequestOrder) -> Result<Save, Reply> {
    let base = [json!("body")];
    let object = object(body, &base, None).map_err(|error| invalid(vec![error]))?;
    let mut errors = Vec::new();
    let snapshot = required(object, "expected_snapshot", &base, body)
        .and_then(|value| text(value, &location(&base, "expected_snapshot"), 0, usize::MAX));
    let snapshot = collected(&mut errors, snapshot);
    let rows_loc = location(&base, "rows");
    let rows = required(object, "rows", &base, body).and_then(|value| array(value, &rows_loc, 500));
    let rows = collected(&mut errors, rows)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, row)| {
            let loc = location(&rows_loc, index);
            let fields = collected(&mut errors, self::object(row, &loc, Some("ReviewRow")))?;
            let (row_keys, change_keys) = order
                .rows
                .get(index)
                .map_or((&[][..], &[][..]), |(row, changes)| {
                    (row.as_slice(), changes.as_slice())
                });
            let id = required(fields, "document_id", &loc, row)
                .and_then(|value| text(value, &location(&loc, "document_id"), 0, usize::MAX));
            let id = collected(&mut errors, id);
            let patch =
                collected(&mut errors, required(fields, "changes", &loc, row)).and_then(|value| {
                    document_patch(
                        value,
                        &location(&loc, "changes"),
                        change_keys,
                        Some("DocumentPatch"),
                        &mut errors,
                    )
                });
            extras(
                fields,
                &loc,
                row_keys,
                |key| matches!(key, "document_id" | "changes"),
                &mut errors,
            );
            Some(ReviewRow {
                document_id: id?.to_owned(),
                changes: patch?,
            })
        })
        .collect::<Vec<_>>();
    extras(
        object,
        &base,
        &order.root,
        |key| matches!(key, "expected_snapshot" | "rows"),
        &mut errors,
    );
    snapshot
        .filter(|_| errors.is_empty())
        .map(|expected_snapshot| Save {
            expected_snapshot: expected_snapshot.to_owned(),
            rows,
        })
        .ok_or_else(|| invalid(errors))
}
