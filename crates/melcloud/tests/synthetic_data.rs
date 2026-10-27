//! Guards for the synthetic test data in `tests/data`.
//!
//! The data is hand-written to follow the API schema and holds no captured values. When
//! `MELCLOUD_CAPTURED_DIR` points to a directory of captured server responses, every field in
//! the synthetic data must also appear in the matching captured response with the same JSON
//! type, so the test data cannot drift from what the server actually sends. Without it,
//! including in CI, that check is skipped.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Synthetic file and the captured response it mirrors.
const PAIRS: [(&str, &str); 4] = [
    ("login.json", "login-response.json"),
    ("list-devices.json", "list-devices-response.json"),
    ("device-get.json", "device-get-response.json"),
    ("set-ata.json", "setata-response.json"),
];

fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn read_json(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Collects every place where `synthetic` has a field that `captured` lacks or types differ.
/// `null` on either side matches anything, because nullable fields carry no type.
/// An empty captured array says nothing about its elements, so those are not checked.
fn schema_drift(synthetic: &Value, captured: &Value, path: &str, drift: &mut Vec<String>) {
    match (synthetic, captured) {
        (Value::Null, _)
        | (_, Value::Null)
        | (Value::Bool(_), Value::Bool(_))
        | (Value::Number(_), Value::Number(_))
        | (Value::String(_), Value::String(_)) => {}
        (Value::Object(fields), Value::Object(known)) => {
            for (key, value) in fields {
                let child = format!("{path}.{key}");
                match known.get(key) {
                    Some(known_value) => schema_drift(value, known_value, &child, drift),
                    None => drift.push(format!("{child} is not in the captured response")),
                }
            }
        }
        (Value::Array(items), Value::Array(known)) => {
            if let Some(example) = known.first() {
                for (i, item) in items.iter().enumerate() {
                    schema_drift(item, example, &format!("{path}[{i}]"), drift);
                }
            }
        }
        _ => drift.push(format!("{path} has a different JSON type than captured")),
    }
}

#[test]
fn synthetic_data_is_valid_json() {
    for (synthetic, _) in PAIRS {
        read_json(&manifest_dir().join("tests/data").join(synthetic));
    }
}

#[test]
fn synthetic_data_matches_captured_schema() {
    let Some(captured_dir) = std::env::var_os("MELCLOUD_CAPTURED_DIR").map(PathBuf::from) else {
        eprintln!("skipped: MELCLOUD_CAPTURED_DIR is not set");
        return;
    };
    let mut drift = Vec::new();
    for (synthetic, captured) in PAIRS {
        let synthetic_value = read_json(&manifest_dir().join("tests/data").join(synthetic));
        let captured_value = read_json(&captured_dir.join(captured));
        schema_drift(&synthetic_value, &captured_value, synthetic, &mut drift);
    }
    assert!(
        drift.is_empty(),
        "synthetic data drifted from the API schema:\n{}",
        drift.join("\n")
    );
}
