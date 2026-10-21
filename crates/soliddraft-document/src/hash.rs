//! Canonical hashes of document content.
//!
//! A document's semantic hash is BLAKE3 over the canonical JSON of its truth entries:
//! what the user authored, without caches, thumbnails, timestamps, the writing
//! application or the format tag. Canonical JSON sorts object keys, has no whitespace and
//! writes numbers the way `serde_json` round-trips them, so two files that hold the same
//! design hash the same however they were formatted. The geometry hash is the same over
//! the truth with names, colours and materials removed; the document kind decides what
//! it removes.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A content hash, written "blake3:" and 64 hex digits.
///
/// Drawings migrated from before phase 0 can hold an older digest ("fnv64:..."), which
/// never equals a BLAKE3 hash, so they read as older than their part until a Rebuild.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentHash(String);

impl ContentHash {
    /// The hash of raw bytes.
    pub fn of_bytes(bytes: &[u8]) -> ContentHash {
        ContentHash(format!("blake3:{}", blake3::hash(bytes).to_hex()))
    }

    /// The hash of a JSON value in canonical form.
    pub fn of_json(value: &Value) -> ContentHash {
        ContentHash::of_bytes(canonical_json(value).as_bytes())
    }

    /// The hash of several named JSON entries, in name order, each in canonical form.
    pub fn of_entries<'a>(entries: impl IntoIterator<Item = (&'a str, &'a Value)>) -> ContentHash {
        let mut entries: Vec<(&str, &Value)> = entries.into_iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        let mut hasher = blake3::Hasher::new();
        for (name, value) in entries {
            hasher.update(name.as_bytes());
            hasher.update(b"\0");
            hasher.update(canonical_json(value).as_bytes());
            hasher.update(b"\0");
        }
        ContentHash(format!("blake3:{}", hasher.finalize().to_hex()))
    }

    /// A hash kept from an older file, such as a pre-phase-0 drawing's part digest.
    pub fn foreign(scheme: &str, digest: &str) -> ContentHash {
        ContentHash(format!("{scheme}:{digest}"))
    }

    /// The hash as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The scheme before the colon, such as "blake3".
    pub fn scheme(&self) -> &str {
        self.0.split_once(':').map(|(s, _)| s).unwrap_or("")
    }

    /// The first eight digits, for display: "3f2a9c1b".
    pub fn short(&self) -> &str {
        let digits = self.0.split_once(':').map(|(_, d)| d).unwrap_or(&self.0);
        &digits[..digits.len().min(8)]
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The canonical text of a JSON value: object keys sorted by their UTF-8 bytes, no
/// insignificant whitespace, strings escaped as `serde_json` escapes them, numbers as
/// `serde_json` writes them (shortest round-trip form for floats).
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).expect("strings serialise"));
                out.push(':');
                write_canonical(&map[k], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, v) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(v, out);
            }
            out.push(']');
        }
        other => out.push_str(&serde_json::to_string(other).expect("scalars serialise")),
    }
}

/// `value` without its top-level `format` key: the format tag says how the content is
/// written, not what it is, so a migration does not change the hash.
pub fn without_format(value: &Value) -> Value {
    let mut v = value.clone();
    if let Value::Object(map) = &mut v {
        map.remove("format");
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_json_ignores_key_order_and_whitespace() {
        let a: Value = serde_json::from_str(r#"{ "b": [1, 2.5, {"y": true, "x": null}], "a": "é\n" }"#).unwrap();
        let b: Value = serde_json::from_str(r#"{"a":"é\n","b":[1,2.5,{"x":null,"y":true}]}"#).unwrap();
        assert_eq!(canonical_json(&a), r#"{"a":"é\n","b":[1,2.5,{"x":null,"y":true}]}"#);
        assert_eq!(ContentHash::of_json(&a), ContentHash::of_json(&b));
        assert_ne!(ContentHash::of_json(&a), ContentHash::of_json(&json!({"a": "é\n"})));
    }

    #[test]
    fn floats_hash_by_value() {
        let x = 0.1_f64 + 0.2;
        let v = json!({ "d": x });
        let back: Value = serde_json::from_str(&serde_json::to_string_pretty(&v).unwrap()).unwrap();
        assert_eq!(ContentHash::of_json(&v), ContentHash::of_json(&back));
    }

    #[test]
    fn entries_hash_in_name_order() {
        let (a, b) = (json!({"k": 1}), json!([2]));
        let h = ContentHash::of_entries([("b.json", &b), ("a.json", &a)]);
        assert_eq!(h, ContentHash::of_entries([("a.json", &a), ("b.json", &b)]));
        assert_ne!(h, ContentHash::of_entries([("a.json", &b), ("b.json", &a)]));
        assert_eq!(h.scheme(), "blake3");
        assert_eq!(h.as_str().len(), "blake3:".len() + 64);
        assert_eq!(h.short().len(), 8);
    }

    #[test]
    fn the_format_tag_is_not_content() {
        let a = json!({"format": "soliddraft-part/1", "features": []});
        let b = json!({"format": "soliddraft-part/2", "features": []});
        assert_eq!(ContentHash::of_json(&without_format(&a)), ContentHash::of_json(&without_format(&b)));
        assert_eq!(ContentHash::foreign("fnv64", "00ff").short(), "00ff");
    }
}
