use super::{PageEdit, PageRequest, Selection, SourceEdit, ROTATION_ERROR};
use crate::{
    matters::{
        integer, integer_error,
        preparation::validation::{array_issue, issue, string_issue},
        reply, Reply,
    },
    request_order::ordered_fields,
    RequestOrder,
};
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeSet,
    ops::{Bound, RangeBounds},
};

const SHA256_PATTERN: &str = "^[0-9a-f]{64}$";

pub(in crate::matters) fn context(mut error: Value, context: Value) -> Value {
    error["ctx"] = context;
    error
}

pub(in crate::matters) fn value_error(loc: Value, input: &Value, message: &str) -> Value {
    context(
        issue(
            "value_error",
            loc,
            &format!("Value error, {message}"),
            input.clone(),
        ),
        json!({"error": {}}),
    )
}

fn invalid(errors: Vec<Value>) -> Reply {
    reply(422, json!({"detail": errors}))
}

fn parse_int(input: &Value, loc: Value, strict: bool) -> Result<i64, Value> {
    if strict && !input.is_i64() && !input.is_u64() {
        return Err(issue(
            "int_type",
            loc,
            "Input should be a valid integer",
            input.clone(),
        ));
    }
    integer(input).ok_or_else(|| {
        let (kind, message) = integer_error(input);
        issue(kind, loc, message, input.clone())
    })
}

pub(in crate::matters) fn at_least(
    input: &Value,
    loc: Value,
    strict: bool,
    min: i64,
) -> Result<u64, Value> {
    // A value outside the range of a stored revision still participates in
    // the normal stale or out-of-range check.
    if let Some(value) = input.as_u64().filter(|&value| value >= min as u64) {
        return Ok(value);
    }
    let parsed = parse_int(input, loc.clone(), strict)?;
    if parsed >= min {
        return Ok(parsed as u64);
    }
    Err(context(
        issue(
            "greater_than_equal",
            loc,
            &format!("Input should be greater than or equal to {min}"),
            input.clone(),
        ),
        json!({"ge": min}),
    ))
}

pub(in crate::matters) fn fraction(
    input: &Value,
    loc: Value,
    limits: impl RangeBounds<f64>,
) -> Result<f64, Value> {
    let value = input
        .as_f64()
        .or_else(|| input.as_bool().map(|flag| f64::from(u8::from(flag))))
        .or_else(|| {
            let text = input.as_str()?.trim();
            if text.as_bytes().iter().enumerate().any(|(index, &byte)| {
                byte == b'_'
                    && (!index
                        .checked_sub(1)
                        .and_then(|i| text.as_bytes().get(i))
                        .is_some_and(u8::is_ascii_digit)
                        || !text
                            .as_bytes()
                            .get(index + 1)
                            .is_some_and(u8::is_ascii_digit))
            }) {
                return None;
            }
            text.replace('_', "").parse().ok()
        })
        .ok_or_else(|| {
            let (kind, message) = if input.is_string() {
                (
                    "float_parsing",
                    "Input should be a valid number, unable to parse string as a number",
                )
            } else {
                ("float_type", "Input should be a valid number")
            };
            issue(kind, loc.clone(), message, input.clone())
        })?;
    let (kind, message, bound) = match (limits.start_bound(), limits.end_bound()) {
        (Bound::Included(&min), _) if value < min => (
            "greater_than_equal",
            format!("Input should be greater than or equal to {min}"),
            json!({"ge": min}),
        ),
        (Bound::Excluded(&min), _) if value <= min => (
            "greater_than",
            format!("Input should be greater than {min}"),
            json!({"gt": min}),
        ),
        (_, Bound::Included(&max)) if value > max || value.is_nan() => (
            "less_than_equal",
            format!("Input should be less than or equal to {max}"),
            json!({"le": max}),
        ),
        (_, Bound::Excluded(&max)) if value >= max || value.is_nan() => (
            "less_than",
            format!("Input should be less than {max}"),
            json!({"lt": max}),
        ),
        _ => return Ok(value),
    };
    Err(context(issue(kind, loc, &message, input.clone()), bound))
}

pub(in crate::matters) struct Fields<'a> {
    input: &'a Value,
    object: &'a Map<String, Value>,
    errors: Vec<Value>,
}

impl<'a> Fields<'a> {
    pub(in crate::matters) fn new(input: &'a Value) -> Result<Self, Reply> {
        let object = input.as_object().ok_or_else(|| {
            let (kind, message) = if input.is_null() {
                ("missing", "Field required")
            } else {
                (
                    "model_attributes_type",
                    "Input should be a valid dictionary or object to extract fields from",
                )
            };
            invalid(vec![issue(kind, json!(["body"]), message, input.clone())])
        })?;
        Ok(Self {
            input,
            object,
            errors: Vec::new(),
        })
    }

    pub(in crate::matters) fn field<T>(
        &mut self,
        key: &str,
        default: Option<T>,
        parse: impl FnOnce(&'a Value, Value) -> Result<T, Value>,
    ) -> Option<T> {
        let loc = json!(["body", key]);
        let result = match self.object.get(key) {
            Some(input) => parse(input, loc),
            None => match default {
                Some(value) => return Some(value),
                None => Err(issue("missing", loc, "Field required", self.input.clone())),
            },
        };
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                self.errors.push(error);
                None
            }
        }
    }

    pub(in crate::matters) fn revision(
        &mut self,
        key: &str,
        required: bool,
        strict: bool,
    ) -> Option<u64> {
        self.field(key, (!required).then_some(None), |input, loc| {
            if input.is_null() && !required {
                Ok(None)
            } else {
                at_least(input, loc, strict, 1).map(Some)
            }
        })
        .flatten()
    }

    fn pages(&mut self, key: &str, limits: Option<(usize, usize)>) -> Option<Vec<i64>> {
        let (min, max) = limits.unwrap_or((0, usize::MAX));
        let input = self.field(key, None, |input, loc| {
            array_issue(input, loc, min, max).map(Vec::as_slice)
        })?;
        let count = self.errors.len();
        let pages = input
            .iter()
            .enumerate()
            .filter_map(|(index, value)| {
                match parse_int(value, json!(["body", key, index]), false) {
                    Ok(page) => Some(page),
                    Err(error) => {
                        self.errors.push(error);
                        None
                    }
                }
            })
            .collect();
        (self.errors.len() == count).then_some(pages)
    }

    pub(in crate::matters) fn extras(&mut self, allowed: &[&str], order: &RequestOrder) {
        self.errors.extend(
            ordered_fields(self.object, &order.root)
                .into_iter()
                .filter(|(key, _)| !allowed.contains(key))
                .map(|(key, value)| {
                    issue(
                        "extra_forbidden",
                        json!(["body", key]),
                        "Extra inputs are not permitted",
                        value.clone(),
                    )
                }),
        );
    }

    pub(in crate::matters) fn extend_errors(&mut self, errors: impl IntoIterator<Item = Value>) {
        self.errors.extend(errors);
    }

    pub(in crate::matters) fn finish<T>(self, parsed: Option<T>) -> Result<T, Reply> {
        if !self.errors.is_empty() {
            return Err(invalid(self.errors));
        }
        Ok(parsed.expect("validated required fields"))
    }
}

impl PageEdit {
    pub(in crate::matters) fn parse(
        tool: &str,
        body: &Value,
        order: &RequestOrder,
    ) -> Result<(PageRequest, Self), Reply> {
        if tool == "image-overlay" {
            let page = integer(&body["page_index"]);
            let revision = body["expected_revision"]
                .as_u64()
                .filter(|revision| *revision > 0);
            return page.zip(revision).map(|(page, revision)| {
                (
                    PageRequest {
                        expected_revision: Some(revision),
                        expected_source_revision: None,
                        acknowledge_signature_risk: body["acknowledge_signature_risk"] == true,
                    },
                    Self::ImageOverlay(page, json!({
                        "rect_x": body["rect_x"], "rect_y": body["rect_y"],
                        "rect_width": body["rect_width"], "rect_height": body["rect_height"],
                    })),
                )
            }).ok_or_else(|| super::refusal(422, "Invalid working revision request"));
        }
        let mut fields = Fields::new(body)?;
        let mut request = PageRequest {
            expected_revision: None,
            expected_source_revision: None,
            acknowledge_signature_risk: body["acknowledge_signature_risk"] == true,
        };
        let page = |fields: &mut Fields<'_>, key| {
            fields.field(key, None, |input, loc| {
                at_least(input, loc, false, 0)
                    .map(|value| usize::try_from(value).unwrap_or(usize::MAX))
            })
        };
        let edit = match tool {
            "rotate" | "delete" => {
                let page = page(&mut fields, "page_index");
                let edit = if tool == "rotate" {
                    let degrees =
                        fields.field("degrees", None, |input, loc| parse_int(input, loc, false));
                    page.zip(degrees).map(|(page, degrees)| {
                        Self::Rotate(page, i32::try_from(degrees).unwrap_or(0))
                    })
                } else {
                    page.map(Self::Delete)
                };
                request.expected_revision = fields.revision("expected_revision", false, true);
                edit
            }
            "reorder" => {
                let pages = fields.pages("order", None);
                request.expected_revision = fields.revision("expected_revision", false, true);
                pages.map(Self::Reorder)
            }
            "insert" | "replace" | "replace-document" => {
                let source = fields.field("source_document_id", None, |input, loc| {
                    string_issue(input, loc, 0, usize::MAX)
                });
                request.expected_revision = fields.revision("expected_revision", false, true);
                request.expected_source_revision =
                    fields.revision("expected_source_revision", false, true);
                let limits = Some((1, 2000));
                let selection = match tool {
                    "insert" => fields
                        .pages("source_pages", limits)
                        .zip(page(&mut fields, "before_index"))
                        .map(|(pages, before)| Selection::Insert { pages, before }),
                    "replace" => fields
                        .pages("target_pages", limits)
                        .zip(fields.pages("source_pages", limits))
                        .map(|(target, source)| Selection::Replace { source, target }),
                    _ => Some(Selection::Whole),
                };
                source.zip(selection).map(|(document_id, selection)| {
                    Self::Source(SourceEdit {
                        document_id: document_id.into(),
                        selection,
                    })
                })
            }
            "batch" => {
                request.expected_revision = fields.revision("expected_revision", true, true);
                batch(&mut fields, order)
            }
            "crop" => {
                let pages = fields.pages("pages", Some((1, 2000)));
                request.expected_revision = fields.revision("expected_revision", false, true);
                let edges = ["left", "top", "right", "bottom"].map(|key| {
                    fields.field(key, None, |input, loc| fraction(input, loc, 0.0..=1.0))
                });
                match edges {
                    [Some(left), Some(top), Some(right), Some(bottom)] => {
                        pages.map(|pages| Self::Crop(pages, [left, top, right, bottom]))
                    }
                    _ => None,
                }
            }
            "text-overlay" => {
                let page = page(&mut fields, "page_index");
                let [x, y] = ["x", "y"].map(|key| {
                    fields.field(key, None, |input, loc| fraction(input, loc, 0.0..1.0))
                });
                let text =
                    fields.field("text", None, |input, loc| string_issue(input, loc, 1, 1000));
                let size = fields.field("font_size", Some(12.0), |input, loc| {
                    fraction(input, loc, 6.0..=36.0)
                });
                fields.extras(
                    &[
                        "acknowledge_signature_risk",
                        "page_index",
                        "x",
                        "y",
                        "text",
                        "font_size",
                    ],
                    order,
                );
                page.zip(x)
                    .zip(y)
                    .zip(text)
                    .zip(size)
                    .map(|((((page, x), y), text), size)| {
                        Self::TextOverlay(page, x, y, text.into(), size)
                    })
            }
            _ => return Err(super::refusal(422, "Invalid working revision request")),
        };
        fields.finish(edit.map(|edit| (request, edit)))
    }
}

fn batch(fields: &mut Fields<'_>, order: &RequestOrder) -> Option<PageEdit> {
    let operation = fields.field("operation", None, |input, loc| {
        const EXPECTED: &str = "'rotate' or 'delete'";
        input
            .as_str()
            .filter(|value| matches!(*value, "rotate" | "delete"))
            .ok_or_else(|| {
                context(
                    issue(
                        "literal_error",
                        loc,
                        &format!("Input should be {EXPECTED}"),
                        input.clone(),
                    ),
                    json!({"expected": EXPECTED}),
                )
            })
    });
    let pages = fields.field("pages", None, |input, loc| {
        let values = input
            .as_array()
            .filter(|values| values.iter().all(|value| value.is_i64() || value.is_u64()))
            .ok_or_else(|| value_error(loc.clone(), input, "Page indexes must be integers"))?;
        let mut distinct = BTreeSet::new();
        if values.iter().any(|value| {
            let number = value
                .as_u64()
                .map(i128::from)
                .unwrap_or_else(|| i128::from(value.as_i64().expect("validated integer")));
            !distinct.insert(number)
        }) {
            return Err(value_error(loc, input, "Select each page only once"));
        }
        array_issue(input, loc, 1, 100000)?;
        Ok(values
            .iter()
            .map(|value| integer(value).expect("validated integer"))
            .collect())
    });
    let degrees = fields.field("degrees", Some(None), |input, loc| {
        if input.is_null() {
            Ok(None)
        } else {
            parse_int(input, loc, true).map(Some)
        }
    });
    fields.extras(
        &[
            "acknowledge_signature_risk",
            "expected_revision",
            "operation",
            "pages",
            "degrees",
        ],
        order,
    );
    let (operation, pages, degrees) = (operation?, pages?, degrees?);
    if !fields.errors.is_empty() {
        return None;
    }
    let reason = match (operation, degrees) {
        ("rotate", Some(degrees @ (90 | 180 | 270))) => {
            return Some(PageEdit::BatchRotate(pages, degrees as i32))
        }
        ("rotate", _) => ROTATION_ERROR,
        ("delete", None) => return Some(PageEdit::BatchDelete(pages)),
        _ => "Deletion does not accept a rotation",
    };
    fields
        .errors
        .push(value_error(json!(["body"]), fields.input, reason));
    None
}

pub(in crate::matters) struct RestoreRequest {
    pub expected_revision: u64,
    pub expected_sha256: String,
    pub acknowledge_signature_risk: bool,
}

pub(in crate::matters) fn restore_request(
    revision: &str,
    body: &Value,
) -> Result<(u64, RestoreRequest), Reply> {
    let input = json!(revision);
    let path = parse_int(&input, json!(["path", "revision"]), false);
    let mut fields = match Fields::new(body) {
        Ok(fields) => fields,
        Err(mut error) => {
            if let Err(problem) = path {
                error.body["detail"]
                    .as_array_mut()
                    .expect("validation errors")
                    .insert(0, problem);
            }
            return Err(error);
        }
    };
    let revision = match path {
        Ok(revision) => Some(u64::try_from(revision).unwrap_or(0)),
        Err(problem) => {
            fields.errors.push(problem);
            None
        }
    };
    let expected_revision = fields.revision("expected_revision", true, false);
    let expected_sha256 = fields.field("expected_sha256", None, |input, loc| {
        let text = string_issue(input, loc.clone(), 0, usize::MAX)?;
        if text.len() == 64
            && text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            Ok(text)
        } else {
            Err(context(
                issue(
                    "string_pattern_mismatch",
                    loc,
                    "String should match pattern '^[0-9a-f]{64}$'",
                    input.clone(),
                ),
                json!({"pattern": SHA256_PATTERN}),
            ))
        }
    });
    fields.finish(revision.zip(expected_revision).zip(expected_sha256).map(
        |((revision, expected_revision), expected_sha256)| {
            (
                revision,
                RestoreRequest {
                    expected_revision,
                    expected_sha256: expected_sha256.into(),
                    acknowledge_signature_risk: body["acknowledge_signature_risk"] == true,
                },
            )
        },
    ))
}
