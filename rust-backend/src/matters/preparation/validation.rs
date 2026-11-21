use super::{integer, integer_error, python_json, reply, Reply};
use serde::Serialize;
use serde_json::{json, Map, Value};

pub(in crate::matters) fn issue(kind: &str, loc: Value, msg: &str, input: Value) -> Value {
    json!({"type":kind,"loc":loc,"msg":msg,"input":input})
}

pub(super) fn invalid(error: Value) -> Reply {
    reply(422, json!({"detail":[error]}))
}

fn object<'a>(input: &'a Value, loc: Value) -> Result<&'a Map<String, Value>, Reply> {
    input.as_object().ok_or_else(|| {
        let (kind, msg) = if loc == json!(["body"]) && input.is_null() {
            ("missing", "Field required")
        } else {
            (
                "model_attributes_type",
                "Input should be a valid dictionary or object to extract fields from",
            )
        };
        invalid(issue(kind, loc, msg, input.clone()))
    })
}

fn required<'a>(obj: &'a Map<String, Value>, key: &str, loc: &[Value]) -> Result<&'a Value, Reply> {
    obj.get(key).ok_or_else(|| {
        invalid(issue(
            "missing",
            field(loc, key),
            "Field required",
            json!(obj),
        ))
    })
}

fn field(loc: &[Value], key: &str) -> Value {
    let mut loc = loc.to_vec();
    loc.push(json!(key));
    json!(loc)
}

fn extras(obj: &Map<String, Value>, allowed: &[&str], loc: &[Value]) -> Result<(), Reply> {
    if let Some((key, value)) = obj.iter().find(|(key, _)| !allowed.contains(&key.as_str())) {
        return Err(invalid(issue(
            "extra_forbidden",
            field(loc, key),
            "Extra inputs are not permitted",
            value.clone(),
        )));
    }
    Ok(())
}

fn string(input: &Value, loc: Value, max: usize) -> Result<&str, Reply> {
    string_issue(input, loc, 0, max).map_err(invalid)
}

pub(in crate::matters) fn string_issue(
    input: &Value,
    loc: Value,
    min: usize,
    max: usize,
) -> Result<&str, Value> {
    let text = input.as_str().ok_or_else(|| {
        issue(
            "string_type",
            loc.clone(),
            "Input should be a valid string",
            input.clone(),
        )
    })?;
    let length = text.chars().count();
    let (kind, message, context) = if length < min {
        (
            "string_too_short",
            format!(
                "String should have at least {min} character{}",
                if min == 1 { "" } else { "s" }
            ),
            json!({"min_length": min}),
        )
    } else if length > max {
        (
            "string_too_long",
            format!("String should have at most {max} characters"),
            json!({"max_length": max}),
        )
    } else {
        return Ok(text);
    };
    let mut problem = issue(kind, loc, &message, input.clone());
    problem["ctx"] = context;
    Err(problem)
}

fn array(input: &Value, loc: Value, min: usize, max: usize) -> Result<&Vec<Value>, Reply> {
    array_issue(input, loc, min, max).map_err(invalid)
}

pub(in crate::matters) fn array_issue(
    input: &Value,
    loc: Value,
    min: usize,
    max: usize,
) -> Result<&Vec<Value>, Value> {
    let array = input.as_array().ok_or_else(|| {
        issue(
            "list_type",
            loc.clone(),
            "Input should be a valid list",
            input.clone(),
        )
    })?;
    let size = array.len();
    if size < min || size > max {
        let (kind, bound, ctx) = if size < min {
            (
                "too_short",
                format!("at least {min}"),
                json!({"field_type":"List","min_length":min,"actual_length":size}),
            )
        } else {
            (
                "too_long",
                format!("at most {max}"),
                json!({"field_type":"List","max_length":max,"actual_length":size}),
            )
        };
        let mut problem = issue(
            kind,
            loc,
            &format!(
                "List should have {bound} item{} after validation, not {size}",
                if size < min && min == 1 { "" } else { "s" }
            ),
            input.clone(),
        );
        problem["ctx"] = ctx;
        return Err(problem);
    }
    Ok(array)
}

fn literal<'a>(
    input: &'a Value,
    loc: Value,
    values: &[&str],
    expected: &str,
) -> Result<&'a str, Reply> {
    input
        .as_str()
        .filter(|text| values.contains(text))
        .ok_or_else(|| {
            let mut error = issue(
                "literal_error",
                loc,
                &format!("Input should be {expected}"),
                input.clone(),
            );
            error["ctx"] = json!({"expected":expected});
            invalid(error)
        })
}

pub(super) fn parse_parties(body: &Value) -> Result<Value, Reply> {
    let base = [json!("body")];
    let obj = object(body, json!(base))?;
    let parties = required(obj, "parties", &base)?;
    for (index, party) in array(parties, field(&base, "parties"), 1, 100)?
        .iter()
        .enumerate()
    {
        let loc = [json!("body"), json!("parties"), json!(index)];
        let obj = object(party, json!(loc))?;
        literal(
            required(obj, "role", &loc)?,
            field(&loc, "role"),
            &["Petitioner", "Respondent"],
            "'Petitioner' or 'Respondent'",
        )?;
        for (key, limit) in [
            ("name", 300),
            ("fatherOrHusbandName", 300),
            ("address", 1000),
            ("email", 300),
            ("mobile", 100),
        ] {
            string(required(obj, key, &loc)?, field(&loc, key), limit)?;
        }
        extras(
            obj,
            &[
                "role",
                "name",
                "fatherOrHusbandName",
                "address",
                "email",
                "mobile",
            ],
            &loc,
        )?;
    }
    Ok(parties.clone())
}

pub(super) fn parse_values(body: &Value) -> Result<Value, Reply> {
    let base = [json!("body")];
    let obj = object(body, json!(base))?;
    let values = required(obj, "values", &base)?;
    let map = values.as_object().ok_or_else(|| {
        invalid(issue(
            "dict_type",
            json!(["body", "values"]),
            "Input should be a valid dictionary",
            values.clone(),
        ))
    })?;
    if map.len() > 100 {
        let mut error = issue(
            "too_long",
            json!(["body", "values"]),
            &format!(
                "Dictionary should have at most 100 items after validation, not {}",
                map.len()
            ),
            values.clone(),
        );
        error["ctx"] =
            json!({"field_type":"Dictionary","max_length":100,"actual_length":map.len()});
        return Err(invalid(error));
    }
    for (key, value) in map {
        string(value, json!(["body", "values", key]), usize::MAX)?;
    }
    for (key, value) in map {
        let mut bytes = key.bytes();
        let message = if key.len() > 64
            || !bytes.next().is_some_and(|b| b.is_ascii_lowercase())
            || !bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            Some("Matter value keys must be lowercase identifiers up to 64 characters")
        } else if value.as_str().unwrap().chars().count() > 2000 {
            Some("Matter values must be at most 2000 characters")
        } else {
            None
        };
        if let Some(message) = message {
            return Err(value_error(json!(["body", "values"]), values, message));
        }
    }
    extras(obj, &["values"], &base)?;
    Ok(values.clone())
}

fn value_error(loc: Value, input: &Value, message: &str) -> Reply {
    let mut error = issue(
        "value_error",
        loc,
        &format!("Value error, {message}"),
        input.clone(),
    );
    error["ctx"] = json!({"error":{}});
    invalid(error)
}

#[derive(Default, Serialize)]
pub(super) struct FilingReview {
    #[serde(flatten)]
    pub(super) meta: FilingReviewMeta,
    pub(super) items: Vec<ReviewItem>,
}
#[derive(Default, Serialize)]
pub(super) struct FilingReviewMeta {
    court: String,
    case_type: String,
    stage: String,
    source_note: String,
}
#[derive(Serialize)]
pub(super) struct ReviewItem {
    title: String,
    pub(super) part: Option<i64>,
    status: String,
}

pub(super) fn parse_review(body: &Value) -> Result<FilingReview, Reply> {
    let loc = [json!("body")];
    let obj = object(body, json!(loc))?;
    let text = |key, max| {
        obj.get(key)
            .map(|v| string(v, field(&loc, key), max))
            .transpose()
            .map(|v| v.unwrap_or_default().to_owned())
    };
    let meta = FilingReviewMeta {
        court: text("court", 120)?,
        case_type: text("case_type", 120)?,
        stage: text("stage", 120)?,
        source_note: text("source_note", 500)?,
    };
    let mut items = Vec::new();
    if let Some(input) = obj.get("items") {
        for (index, item) in array(input, field(&loc, "items"), 0, 12)?
            .iter()
            .enumerate()
        {
            let loc = [json!("body"), json!("items"), json!(index)];
            let obj = object(item, json!(loc))?;
            let raw = required(obj, "title", &loc)?;
            let title = string(raw, field(&loc, "title"), 80)?;
            if title.is_empty() {
                let mut error = issue(
                    "string_too_short",
                    field(&loc, "title"),
                    "String should have at least 1 character",
                    raw.clone(),
                );
                error["ctx"] = json!({"min_length":1});
                return Err(invalid(error));
            }
            let title = title.trim();
            if title.is_empty() {
                return Err(value_error(
                    field(&loc, "title"),
                    raw,
                    "Checklist item title is required",
                ));
            }
            let part = obj
                .get("part")
                .filter(|v| !v.is_null())
                .map(|value| {
                    let located = field(&loc, "part");
                    let num = integer(value).ok_or_else(|| {
                        let (kind, msg) = integer_error(value);
                        invalid(issue(kind, located.clone(), msg, value.clone()))
                    })?;
                    if !(1..=99).contains(&num) {
                        let (kind, comparison, limit, ctx) = if num < 1 {
                            ("greater_than_equal", "greater", 1, "ge")
                        } else {
                            ("less_than_equal", "less", 99, "le")
                        };
                        let mut error = issue(
                            kind,
                            located,
                            &format!("Input should be {comparison} than or equal to {limit}"),
                            value.clone(),
                        );
                        error["ctx"] = json!({ctx: limit});
                        return Err(invalid(error));
                    }
                    Ok(num)
                })
                .transpose()?;
            let status = obj
                .get("status")
                .map(|value| {
                    literal(
                        value,
                        field(&loc, "status"),
                        &["unreviewed", "present", "not_applicable"],
                        "'unreviewed', 'present' or 'not_applicable'",
                    )
                })
                .transpose()?
                .unwrap_or("unreviewed");
            extras(obj, &["title", "part", "status"], &loc)?;
            items.push(ReviewItem {
                title: title.to_owned(),
                part,
                status: status.to_owned(),
            });
        }
    }
    extras(
        obj,
        &["court", "case_type", "stage", "source_note", "items"],
        &loc,
    )?;
    if python_json(&items).chars().count() > 2000 {
        return Err(value_error(
            json!(loc),
            body,
            "Checklist is too long; shorten item titles",
        ));
    }
    Ok(FilingReview { meta, items })
}

pub(in crate::matters) fn parse_order(body: &Value) -> Result<Vec<&str>, Reply> {
    let base = [json!("body")];
    let obj = object(body, json!(base))?;
    let ids = required(obj, "ids", &base)?;
    array(ids, field(&base, "ids"), 0, usize::MAX)?
        .iter()
        .enumerate()
        .map(|(index, id)| string(id, json!(["body", "ids", index]), usize::MAX))
        .collect()
}
