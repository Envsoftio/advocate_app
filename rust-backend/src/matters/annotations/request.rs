use super::{create_issue, valid_region, Fields, TextStyle, INVALID_REGION};
use crate::{
    matters::{
        boolean,
        document_metadata::request::literal,
        page_tools::request::{at_least, fraction, value_error, Fields as Validator},
        preparation::validation::{issue, string_issue},
        reply, Reply,
    },
    request_order::ordered_fields,
    RequestOrder,
};
use serde_json::{json, Map, Value};
use std::ops::Bound::{Excluded, Included};

pub(super) const KINDS: &[&str] = &[
    "highlight",
    "comment",
    "draw",
    "box",
    "ellipse",
    "line",
    "arrow",
    "check",
    "cross",
    "date",
    "text",
    "redaction",
];
const COLORS: &[&str] = &["yellow", "green", "blue", "pink"];
const SCOPES: &[&str] = &["private", "pdf"];
const CREATE: &[&str] = &[
    "kind",
    "page_index",
    "expected_revision",
    "rect_x",
    "rect_y",
    "rect_width",
    "rect_height",
    "body",
    "color",
    "output_scope",
    "text_style",
];
const STYLE: &[&str] = &["font_family", "font_size", "bold", "alignment"];

pub(super) struct Create {
    pub kind: String,
    pub page: u64,
    pub fields: Fields,
}

pub(super) fn create(body: &Value, order: &RequestOrder) -> Result<Create, Reply> {
    let mut validator = Validator::new(body)?;
    let kind = validator.field("kind", None, |input, loc| {
        annotation_literal(input, loc, KINDS).map(str::to_owned)
    });
    let page = validator.field("page_index", None, |input, loc| {
        at_least(input, loc, false, 0)
    });
    let fields = read_fields(&mut validator, body, order, true);
    validator.extras(CREATE, order);
    let draft = validator.finish(
        kind.zip(page)
            .zip(fields)
            .map(|((kind, page), fields)| Create { kind, page, fields }),
    )?;
    model_validate(body, &draft.fields, Some(&draft.kind))?;
    Ok(draft)
}

pub(super) fn update(body: &Value, order: &RequestOrder) -> Result<Fields, Reply> {
    let mut validator = Validator::new(body)?;
    let fields = read_fields(&mut validator, body, order, false);
    validator.extras(&CREATE[2..], order);
    let fields = validator.finish(fields)?;
    model_validate(body, &fields, None)?;
    Ok(fields)
}

fn read_fields(
    validator: &mut Validator<'_>,
    body: &Value,
    order: &RequestOrder,
    create: bool,
) -> Option<Fields> {
    let revision = validator.revision("expected_revision", true, false);
    let rect_x = validator.field("rect_x", None, |input, loc| fraction(input, loc, 0.0..1.0));
    let rect_y = validator.field("rect_y", None, |input, loc| fraction(input, loc, 0.0..1.0));
    let rect_width = validator.field("rect_width", None, |input, loc| {
        fraction(input, loc, (Excluded(0.0), Included(1.0)))
    });
    let rect_height = validator.field("rect_height", None, |input, loc| {
        fraction(input, loc, (Excluded(0.0), Included(1.0)))
    });
    let text = validator.field("body", Some(String::new()), |input, loc| {
        string_issue(input, loc, 0, 4000).map(str::to_owned)
    });
    let color = validator.field("color", Some("yellow".into()), |input, loc| {
        annotation_literal(input, loc, COLORS).map(str::to_owned)
    });
    let scope = validator.field(
        "output_scope",
        Some(create.then(|| "private".into())),
        |input, loc| {
            if input.is_null() && !create {
                Ok(None)
            } else {
                annotation_literal(input, loc, SCOPES).map(|value| Some(value.to_owned()))
            }
        },
    );
    let style = read_style(body.get("text_style"), order, validator);
    Some(Fields {
        expected_revision: revision?,
        rect_x: rect_x?,
        rect_y: rect_y?,
        rect_width: rect_width?,
        rect_height: rect_height?,
        body: text?,
        color: color?,
        output_scope: scope?,
        text_style: style?,
    })
}

fn model_validate(body: &Value, fields: &Fields, create_kind: Option<&str>) -> Result<(), Reply> {
    let problem = (!valid_region(fields.rect()))
        .then_some(INVALID_REGION)
        .or_else(|| create_kind.and_then(|kind| create_issue(kind, fields)));
    if let Some(problem) = problem {
        return Err(reply(
            422,
            json!({"detail":[value_error(json!(["body"]), body, problem)]}),
        ));
    }
    Ok(())
}

fn annotation_literal<'a>(
    input: &'a Value,
    loc: Value,
    options: &[&str],
) -> Result<&'a str, Value> {
    literal(input, loc.as_array().expect("validation location"), options)
}

fn read_style(
    input: Option<&Value>,
    order: &RequestOrder,
    validator: &mut Validator<'_>,
) -> Option<Option<TextStyle>> {
    let Some(input) = input.filter(|value| !value.is_null()) else {
        return Some(None);
    };
    let Some(object) = input.as_object() else {
        validator.extend_errors([issue(
            "model_attributes_type",
            json!(["body", "text_style"]),
            "Input should be a valid dictionary or object to extract fields from",
            input.clone(),
        )]);
        return None;
    };
    let mut errors = Vec::new();
    let family = style_field(
        object,
        "font_family",
        "helv".into(),
        &mut errors,
        |input, loc| annotation_literal(input, loc, &["helv", "tiro", "cour"]).map(str::to_owned),
    );
    let size = style_field(object, "font_size", json!(12), &mut errors, |input, loc| {
        fraction(input, loc, 6.0..=36.0).map(|size| json!(size))
    });
    let bold = style_field(object, "bold", false, &mut errors, |input, loc| {
        boolean(input).ok_or_else(|| {
            let (kind, message) =
                if input.is_string() || input.as_f64().is_some_and(|value| value.fract() == 0.0) {
                    (
                        "bool_parsing",
                        "Input should be a valid boolean, unable to interpret input",
                    )
                } else {
                    ("bool_type", "Input should be a valid boolean")
                };
            issue(kind, loc, message, input.clone())
        })
    });
    let alignment = style_field(
        object,
        "alignment",
        "left".into(),
        &mut errors,
        |input, loc| {
            annotation_literal(input, loc, &["left", "center", "right", "justify"])
                .map(str::to_owned)
        },
    );
    errors.extend(
        ordered_fields(object, &order.text_style)
            .into_iter()
            .filter(|(key, _)| !STYLE.contains(key))
            .map(|(key, value)| {
                issue(
                    "extra_forbidden",
                    json!(["body", "text_style", key]),
                    "Extra inputs are not permitted",
                    value.clone(),
                )
            }),
    );
    validator.extend_errors(errors);
    Some(Some(TextStyle {
        font_family: family?,
        font_size: size?,
        bold: bold?,
        alignment: alignment?,
    }))
}

fn style_field<T>(
    object: &Map<String, Value>,
    key: &str,
    default: T,
    errors: &mut Vec<Value>,
    parse: impl FnOnce(&Value, Value) -> Result<T, Value>,
) -> Option<T> {
    match object
        .get(key)
        .map(|input| parse(input, json!(["body", "text_style", key])))
    {
        None => Some(default),
        Some(Ok(value)) => Some(value),
        Some(Err(error)) => {
            errors.push(error);
            None
        }
    }
}
