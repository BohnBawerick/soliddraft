//! The API keys: each provider's key is read from its environment variable
//! (`ANTHROPIC_API_KEY`, `OPENROUTER_API_KEY`), or else from its own OS keychain entry
//! (Secret Service on Linux, Keychain on macOS, Credential Manager on Windows).
//!
//! A key is never logged: [`ApiKey`] prints as `ApiKey(<redacted>)` and only
//! [`ApiKey::expose`] gives the text, for the request header.

use crate::provider::ProviderKind;
use std::fmt;
use std::sync::Arc;

/// Set to `1` to skip the keychain, for tests and headless runs that must not prompt.
pub const NO_KEYCHAIN_VAR: &str = "SOLIDDRAFT_NO_KEYCHAIN";
/// The keychain entries' service name.
pub const KEYCHAIN_SERVICE: &str = "soliddraft";
const LEGACY_SERVICE: &str = "draftwell";

/// The keychain entry's user name for a provider's key.
pub fn keychain_user(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Anthropic => "anthropic-api-key",
        ProviderKind::OpenRouter => "openrouter-api-key",
    }
}

/// An API key. Debug and Display never show it.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    /// Wraps a key. Surrounding whitespace is dropped; an empty key is `None`.
    pub fn new(key: impl Into<String>) -> Option<Self> {
        let key = key.into();
        let trimmed = key.trim();
        if trimmed.is_empty() { None } else { Some(Self(trimmed.to_owned())) }
    }

    /// The key itself, for the request header only.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// The key masked for the interface: dots and the last four characters, or only dots
    /// for a key too short to show any of it.
    pub fn masked(&self) -> String {
        let chars: Vec<char> = self.0.chars().collect();
        if chars.len() < 16 {
            return "••••••••".into();
        }
        format!("••••••••{}", chars[chars.len() - 4..].iter().collect::<String>())
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

impl fmt::Display for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Where a key came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeySource {
    /// The provider's environment variable, such as `OPENROUTER_API_KEY`.
    Environment(ProviderKind),
    /// The provider's OS keychain entry, such as `soliddraft` / `openrouter-api-key`.
    Keychain(ProviderKind),
}

impl KeySource {
    /// A short phrase for the interface.
    pub fn describe(self) -> String {
        match self {
            KeySource::Environment(kind) => format!("key from {}", kind.key_var()),
            KeySource::Keychain(_) => "key from the OS keychain".into(),
        }
    }
}

/// Finds a provider's key: the environment first, then the keychain. A keychain that cannot
/// be reached (no Secret Service, locked) counts as no key; the reason is logged without the key.
pub fn find(kind: ProviderKind) -> Option<(ApiKey, KeySource)> {
    if let Some(key) = std::env::var(kind.key_var()).ok().and_then(ApiKey::new) {
        return Some((key, KeySource::Environment(kind)));
    }
    if keychain_off() {
        return None;
    }
    match read_keychain(kind) {
        Ok(key) => key.map(|k| (k, KeySource::Keychain(kind))),
        Err(e) => {
            log::debug!("no {kind} key from the keychain: {e}");
            None
        }
    }
}

/// Whether the keychain holds a key for a provider, whatever the environment says.
pub fn in_keychain(kind: ProviderKind) -> bool {
    !keychain_off() && matches!(read_keychain(kind), Ok(Some(_)))
}

fn keychain_off() -> bool {
    crate::environment::var(NO_KEYCHAIN_VAR).is_some_and(|v| v == "1")
}

/// Reads a provider's keychain entry. `Ok(None)` when there is no entry.
pub fn read_keychain(kind: ProviderKind) -> Result<Option<ApiKey>, String> {
    read_with(|service| match entry_for(service, kind)?.get_password() {
        Ok(text) => Ok(Some(text)),
        Err(keyring_core::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.to_string()),
    })
}

fn read_with(get: impl Fn(&str) -> Result<Option<String>, String>) -> Result<Option<ApiKey>, String> {
    let text = match get(KEYCHAIN_SERVICE)? {
        Some(text) => Some(text),
        None => get(LEGACY_SERVICE)?,
    };
    Ok(text.and_then(ApiKey::new))
}

/// Stores a provider's key in the keychain, replacing any earlier one.
pub fn store_in_keychain(kind: ProviderKind, key: &ApiKey) -> Result<(), String> {
    entry(kind)?.set_password(key.expose()).map_err(|e| e.to_string())
}

/// Removes a provider's keychain entry. Removing a missing entry is not an error.
pub fn delete_from_keychain(kind: ProviderKind) -> Result<(), String> {
    // Remove both entries so a deleted key cannot reappear through the fallback.
    for service in [KEYCHAIN_SERVICE, LEGACY_SERVICE] {
        match entry_for(service, kind)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

fn entry(kind: ProviderKind) -> Result<keyring_core::Entry, String> {
    entry_for(KEYCHAIN_SERVICE, kind)
}

fn entry_for(service: &str, kind: ProviderKind) -> Result<keyring_core::Entry, String> {
    store().map_err(|e| e.to_string())?.build(service, keychain_user(kind), None).map_err(|e| e.to_string())
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android"))))]
fn store() -> keyring_core::Result<Arc<keyring_core::CredentialStore>> {
    Ok(zbus_secret_service_keyring_store::Store::new()?)
}

#[cfg(target_os = "macos")]
fn store() -> keyring_core::Result<Arc<keyring_core::CredentialStore>> {
    Ok(apple_native_keyring_store::keychain::Store::new()?)
}

#[cfg(target_os = "windows")]
fn store() -> keyring_core::Result<Arc<keyring_core::CredentialStore>> {
    Ok(windows_native_keyring_store::Store::new()?)
}

#[cfg(not(any(unix, target_os = "windows")))]
fn store() -> keyring_core::Result<Arc<keyring_core::CredentialStore>> {
    Err(keyring_core::Error::NotSupportedByStore("no keychain on this platform".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_keys_are_read_only_when_the_current_entry_is_absent() {
        let old = |service: &str| Ok((service == LEGACY_SERVICE).then(|| "old-key".into()));
        assert_eq!(read_with(old).unwrap().unwrap().expose(), "old-key");
        assert_eq!(read_with(|_| Ok(None)).unwrap(), None);
        for value in ["new-key", ""] {
            let key = read_with(|service| {
                assert_eq!(service, KEYCHAIN_SERVICE, "a current entry blocks the fallback");
                Ok(Some(value.into()))
            })
            .unwrap();
            assert_eq!(key, ApiKey::new(value));
        }
        assert_eq!(read_with(|_| Err("locked".into())), Err("locked".into()));
    }

    #[test]
    fn key_never_prints() {
        let key = ApiKey::new("  sk-ant-secret ").unwrap();
        assert_eq!(key.expose(), "sk-ant-secret");
        assert!(!format!("{key:?} {key}").contains("secret"));
        assert!(ApiKey::new("   ").is_none());
        assert_eq!(key.masked(), "••••••••");
        assert_eq!(ApiKey::new("sk-or-v1-0123456789abcdef").unwrap().masked(), "••••••••cdef");
    }
}
