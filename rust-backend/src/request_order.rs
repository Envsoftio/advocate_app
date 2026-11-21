use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{value::RawValue, Map, Value};
use std::{collections::HashSet, fmt};

/// Retains the request's object key order for validation errors. Data passed to
/// storage and digest code continues to use the core's canonical JSON maps.
#[derive(Default)]
pub struct RequestOrder {
    pub(crate) root: Vec<String>,
    pub(crate) rows: Vec<(Vec<String>, Vec<String>)>,
    pub(crate) text_style: Vec<String>,
}

impl RequestOrder {
    pub fn from_json(raw: Option<&str>) -> Self {
        let Some(object) = raw.and_then(|raw| serde_json::from_str::<OrderedObject<'_>>(raw).ok())
        else {
            return Self::default();
        };
        let rows = object
            .0
            .iter()
            .rev()
            .find(|(key, _)| key == "rows")
            .and_then(|(_, value)| serde_json::from_str::<Vec<&RawValue>>(value.get()).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|value| {
                let Ok(row) = serde_json::from_str::<OrderedObject<'_>>(value.get()) else {
                    return (Vec::new(), Vec::new());
                };
                let changes = row.child_keys("changes");
                (row.keys(), changes)
            })
            .collect();
        Self {
            text_style: object.child_keys("text_style"),
            root: object.keys(),
            rows,
        }
    }
}

struct OrderedObject<'a>(Vec<(String, &'a RawValue)>);

impl OrderedObject<'_> {
    fn keys(self) -> Vec<String> {
        self.0.into_iter().map(|(key, _)| key).collect()
    }

    fn child_keys(&self, name: &str) -> Vec<String> {
        self.0
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .and_then(|(_, value)| serde_json::from_str::<OrderedObject<'_>>(value.get()).ok())
            .map(OrderedObject::keys)
            .unwrap_or_default()
    }
}

impl<'de> Deserialize<'de> for OrderedObject<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ObjectVisitor;

        impl<'de> Visitor<'de> for ObjectVisitor {
            type Value = OrderedObject<'de>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut fields = Vec::new();
                while let Some(field) = access.next_entry()? {
                    fields.push(field);
                }
                Ok(OrderedObject(fields))
            }
        }

        deserializer.deserialize_map(ObjectVisitor)
    }
}

pub(crate) fn ordered_fields<'a>(
    object: &'a Map<String, Value>,
    order: &[String],
) -> Vec<(&'a str, &'a Value)> {
    if order.is_empty() {
        return object
            .iter()
            .map(|(key, value)| (key.as_str(), value))
            .collect();
    }
    let mut seen = HashSet::new();
    let mut fields = Vec::with_capacity(object.len());
    for key in order {
        if let Some((stored, value)) = object.get_key_value(key) {
            if seen.insert(stored.as_str()) {
                fields.push((stored.as_str(), value));
            }
        }
    }
    fields.extend(
        object
            .iter()
            .filter_map(|(key, value)| seen.insert(key.as_str()).then_some((key.as_str(), value))),
    );
    fields
}
