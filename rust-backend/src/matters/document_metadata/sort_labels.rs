use crate::matters::reading::{decimal_digit, folded, python_whitespace, UNICODE_DECIMAL_ZEROS};
use regex::Regex;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
    sync::LazyLock,
};

static NUMBERED: LazyLock<Regex> = LazyLock::new(|| {
    let mut digits = String::new();
    for zero in UNICODE_DECIMAL_ZEROS {
        write!(digits, "\\u{{{zero:x}}}-\\u{{{:x}}}", zero + 9).expect("writing to a string");
    }
    Regex::new(&format!(r"\A(.+?)[- ]?([{digits}]+)\z")).expect("document label pattern")
});

// Left padding makes lexicographic order equal to numeric order. Document
// labels allow 60 Unicode scalars and need up to 59 decimal digits.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
struct Number([u8; 60]);

impl Number {
    fn parse(text: &str) -> Option<Self> {
        let mut digits = [0; 60];
        for (index, ch) in text.chars().rev().enumerate() {
            *digits.iter_mut().rev().nth(index)? = decimal_digit(ch)?;
        }
        Some(Self(digits))
    }

    fn increment(&mut self) {
        for digit in self.0.iter_mut().rev() {
            if *digit < 9 {
                *digit += 1;
                return;
            }
            *digit = 0;
        }
    }

    fn decimal(&self) -> String {
        let first = self
            .0
            .iter()
            .position(|&digit| digit > 0)
            .unwrap_or(self.0.len() - 1);
        self.0[first..]
            .iter()
            .map(|&digit| char::from(b'0' + digit))
            .collect()
    }
}

pub(super) fn review(documents: &[Value]) -> Value {
    let mut labels = BTreeMap::<String, (&str, usize)>::new();
    let mut numbers = BTreeMap::<String, BTreeSet<Number>>::new();
    for document in documents {
        let label = document["label"]
            .as_str()
            .unwrap_or_default()
            .trim_matches(python_whitespace);
        if label.is_empty() {
            continue;
        }
        labels.entry(folded(label)).or_insert((label, 0)).1 += 1;
        if let Some(captures) = NUMBERED.captures(label) {
            if let Some(number) = Number::parse(&captures[2]) {
                numbers
                    .entry(captures[1].to_uppercase())
                    .or_default()
                    .insert(number);
            }
        }
    }
    let duplicates = labels
        .into_values()
        .filter_map(|(label, count)| (count > 1).then_some(label))
        .collect::<Vec<_>>();
    let mut gaps = Vec::new();
    'prefixes: for (prefix, numbers) in numbers {
        for (&before, &after) in numbers.iter().zip(numbers.iter().skip(1)) {
            let mut missing = before;
            missing.increment();
            while missing < after {
                gaps.push(format!("{prefix}-{}", missing.decimal()));
                if gaps.len() == 100 {
                    break 'prefixes;
                }
                missing.increment();
            }
        }
    }
    json!({"duplicates": duplicates, "gaps": gaps})
}
