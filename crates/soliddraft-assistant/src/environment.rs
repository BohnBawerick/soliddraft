//! SolidDraft environment variables, with Draftwell names as read-only fallbacks.

use std::ffi::OsString;

/// Reads `name`, using its `DRAFTWELL_*` alias only when it is absent.
pub fn var_os(name: &str) -> Option<OsString> {
    lookup(name, |key| std::env::var_os(key))
}

/// Reads a Unicode value. A present but empty or non-Unicode value blocks its alias.
pub fn var(name: &str) -> Option<String> {
    var_os(name)?.into_string().ok()
}

fn lookup(name: &str, get: impl Fn(&str) -> Option<OsString>) -> Option<OsString> {
    get(name).or_else(|| get(&format!("DRAFTWELL_{}", name.strip_prefix("SOLIDDRAFT_")?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_values_override_legacy_values_even_when_empty() {
        let old = |key: &str| (key == "DRAFTWELL_MODEL").then(|| OsString::from("old-model"));
        assert_eq!(lookup("SOLIDDRAFT_MODEL", old), Some("old-model".into()));
        for value in ["new-model", ""] {
            let get = |key: &str| match key {
                "SOLIDDRAFT_MODEL" => Some(value.into()),
                _ => old(key),
            };
            assert_eq!(lookup("SOLIDDRAFT_MODEL", get), Some(value.into()));
        }
        assert_eq!(lookup("SOLIDDRAFT_PROVIDER", old), None);
        assert_eq!(lookup("OTHER_MODEL", old), None);
    }
}
