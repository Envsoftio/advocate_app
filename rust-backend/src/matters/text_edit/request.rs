use crate::matters::{integer, integer_error, preparation::validation::issue, reply, Reply};
use serde_json::{json, Map, Value};

pub(super) enum Model {
    Selection,
    Replacement,
    Move,
}

enum Field {
    Index(u64),
    Text { min: usize, max: usize },
    OptionalText { max: usize },
    Offset(Option<f64>),
}

pub(super) fn parse(body: &Value, model: Model) -> Result<Value, Reply> {
    let Some(object) = body.as_object() else {
        let (kind, message) = if body.is_null() {
            ("missing", "Field required")
        } else {
            (
                "model_attributes_type",
                "Input should be a valid dictionary or object to extract fields from",
            )
        };
        return Err(reply(
            422,
            json!({"detail":[issue(kind, json!(["body"]), message, body.clone())]}),
        ));
    };
    let mut fields = vec![
        ("page_index", Field::Index(0)),
        ("span_index", Field::Index(0)),
        ("expected_revision", Field::Index(1)),
        ("expected_text", Field::Text { min: 1, max: 2000 }),
    ];
    match model {
        Model::Selection => (),
        Model::Replacement => fields.extend([
            ("replacement", Field::Text { min: 1, max: 2000 }),
            ("expected_paragraph", Field::OptionalText { max: 4000 }),
        ]),
        Model::Move => fields.extend([
            ("horizontal_offset", Field::Offset(Some(0.0))),
            ("vertical_offset", Field::Offset(None)),
        ]),
    }
    let mut normalized = Map::new();
    normalized.insert(
        "acknowledge_signature_risk".into(),
        json!(body["acknowledge_signature_risk"] == true),
    );
    let mut errors = Vec::new();
    for (key, field) in &fields {
        let loc = json!(["body", key]);
        let result = match (object.get(*key), field) {
            (None, Field::Offset(Some(default))) => Ok(json!(default)),
            (None | Some(Value::Null), Field::OptionalText { .. }) => Ok(Value::Null),
            (None, _) => Err(issue("missing", loc, "Field required", body.clone())),
            (Some(input), _) => field.parse(input, loc),
        };
        match result {
            Ok(value) => {
                normalized.insert((*key).into(), value);
            }
            Err(error) => errors.push(error),
        }
    }
    errors.extend(object.iter().filter_map(|(key, input)| {
        (key != "acknowledge_signature_risk" && !fields.iter().any(|(field, _)| field == key)).then(
            || {
                issue(
                    "extra_forbidden",
                    json!(["body", key]),
                    "Extra inputs are not permitted",
                    input.clone(),
                )
            },
        )
    }));
    if errors.is_empty() {
        Ok(Value::Object(normalized))
    } else {
        Err(reply(422, json!({"detail":errors})))
    }
}

impl Field {
    fn parse(&self, input: &Value, loc: Value) -> Result<Value, Value> {
        let error = |kind, message: &str| issue(kind, loc.clone(), message, input.clone());
        let (kind, message, context) = match *self {
            Self::Index(min) => {
                let signed = integer(input);
                if let Some(value) = input
                    .as_u64()
                    .or_else(|| signed.and_then(|value| u64::try_from(value).ok()))
                {
                    if value >= min {
                        return Ok(json!(value));
                    }
                } else if signed.is_none() {
                    let (kind, message) = integer_error(input);
                    return Err(error(kind, message));
                }
                (
                    "greater_than_equal",
                    format!("Input should be greater than or equal to {min}"),
                    json!({"ge": min}),
                )
            }
            Self::Text { max, .. } | Self::OptionalText { max } => {
                let min = match self {
                    Self::Text { min, .. } => *min,
                    _ => 0,
                };
                let text = input
                    .as_str()
                    .ok_or_else(|| error("string_type", "Input should be a valid string"))?;
                let length = text.chars().count();
                if length < min {
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
                    return Ok(input.clone());
                }
            }
            Self::Offset(_) => {
                let value = input
                    .as_f64()
                    .or_else(|| input.as_bool().map(|value| f64::from(u8::from(value))));
                let value = if let Some(text) = input.as_str() {
                    text.trim().parse::<f64>().map_err(|_| {
                        error(
                            "float_parsing",
                            "Input should be a valid number, unable to parse string as a number",
                        )
                    })?
                } else {
                    value.ok_or_else(|| error("float_type", "Input should be a valid number"))?
                };
                if value < -1.0 {
                    (
                        "greater_than_equal",
                        "Input should be greater than or equal to -1".into(),
                        json!({"ge": -1.0}),
                    )
                } else if value > 1.0 || value.is_nan() {
                    (
                        "less_than_equal",
                        "Input should be less than or equal to 1".into(),
                        json!({"le": 1.0}),
                    )
                } else {
                    return Ok(json!(value));
                }
            }
        };
        let mut detail = error(kind, &message);
        detail["ctx"] = context;
        Err(detail)
    }
}
