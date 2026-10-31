//! Text properties: file defaults, then sparse configuration overrides.

use super::Configurations;
use crate::Document;
use std::collections::BTreeMap;

/// Text properties by name.
pub type Properties = BTreeMap<String, String>;

/// The longest property value, in bytes.
const MAX_TEXT: usize = 4096;

/// Checks a property name. The name sits inside a table header, so it cannot hold brackets.
pub(crate) fn check_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() || name.trim() != name || name.len() > 128 {
        return Err("a property needs a name of 1 to 128 bytes without outer spaces".into());
    }
    if name.chars().any(|c| c.is_control() || matches!(c, '[' | ']')) {
        return Err(format!("property name {name} cannot hold brackets or control characters"));
    }
    Ok(())
}

/// Checks a stored value. Blank means inherit, so it is never stored; text is otherwise exact.
pub(crate) fn check_text(name: &str, text: &str) -> Result<(), String> {
    if text.is_empty() || text.len() > MAX_TEXT {
        return Err(format!("property {name} needs text of 1 to {MAX_TEXT} bytes"));
    }
    if text.chars().any(char::is_control) {
        return Err(format!("property {name} cannot hold control characters"));
    }
    Ok(())
}

/// Checks a persisted or candidate property layer.
pub(crate) fn check(properties: &Properties) -> Result<(), String> {
    properties.iter().try_for_each(|(name, text)| {
        check_name(name)?;
        check_text(name, text)
    })
}

impl Configurations {
    /// The selected row's overrides over the file's defaults.
    fn resolved_properties(&self, defaults: &Properties) -> Properties {
        let mut out = defaults.clone();
        if let Some(row) = self.rows.iter().find(|r| Some(&r.id) == self.active.as_ref()) {
            out.extend(row.properties.iter().map(|(name, text)| (name.clone(), text.clone())));
        }
        out
    }
}

impl Document {
    /// The file's own text properties.
    pub fn file_properties(&self) -> &Properties {
        &self.properties
    }

    /// The text properties of the selected configuration, overrides over file defaults.
    pub fn resolved_properties(&self) -> Properties {
        self.configurations.resolved_properties(&self.properties)
    }
}
