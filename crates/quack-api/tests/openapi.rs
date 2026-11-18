//! The OpenAPI document: committed copy is current, it is OpenAPI 3.1,
//! every reference resolves, and routes use glossary words only.

use std::collections::BTreeSet;
use std::path::PathBuf;

use quack_api::openapi::{DOCUMENT_PATH, document_json};

fn committed_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(DOCUMENT_PATH)
}

/// `UPDATE_OPENAPI=1 cargo nextest run -p quack-api openapi_is_current`
/// rewrites the committed document.
#[test]
fn openapi_is_current() {
    let generated = document_json();
    let path = committed_path();
    if std::env::var("UPDATE_OPENAPI").is_ok_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &generated).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed == generated,
        "{DOCUMENT_PATH} is stale; run UPDATE_OPENAPI=1 cargo nextest run -p quack-api openapi_is_current"
    );
}

fn refs(value: &serde_json::Value, out: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                if k == "$ref" {
                    if let Some(s) = v.as_str() {
                        out.insert(s.to_owned());
                    }
                } else {
                    refs(v, out);
                }
            }
        }
        serde_json::Value::Array(items) => items.iter().for_each(|v| refs(v, out)),
        _ => {}
    }
}

#[test]
fn openapi_is_valid_3_1() {
    let doc: serde_json::Value = serde_json::from_str(&document_json()).unwrap();
    assert!(doc["openapi"].as_str().unwrap().starts_with("3.1"));
    let schemas = doc["components"]["schemas"].as_object().unwrap();
    let mut found = BTreeSet::new();
    refs(&doc, &mut found);
    assert!(!found.is_empty());
    for r in found {
        let name = r
            .strip_prefix("#/components/schemas/")
            .unwrap_or_else(|| panic!("{r}"));
        assert!(schemas.contains_key(name), "unresolved {r}");
    }
    let mut operation_ids = BTreeSet::new();
    for (path, item) in doc["paths"].as_object().unwrap() {
        for word in ["template", "audit", "notification", "infraction"] {
            assert!(!path.contains(word), "{path} uses {word}");
        }
        for (method, op) in item.as_object().unwrap() {
            assert!(
                op["responses"].as_object().is_some_and(|r| !r.is_empty()),
                "{method} {path}"
            );
            let id = op["operationId"].as_str().unwrap_or_default().to_owned();
            assert!(
                operation_ids.insert(id),
                "duplicate operationId at {method} {path}"
            );
        }
    }
    let envelope = &schemas["ErrorEnvelope"];
    assert!(envelope["properties"]["error"].is_object());
}
