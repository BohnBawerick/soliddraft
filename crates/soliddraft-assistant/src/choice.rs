//! Which provider and model the assistant uses.
//!
//! In order: `SOLIDDRAFT_PROVIDER`, then the provider setting, then whichever provider has a
//! key (Anthropic when both do). The model is `SOLIDDRAFT_MODEL`, then the setting for the
//! chosen provider, then that provider's default.

use crate::key::{self, ApiKey, KeySource};
use crate::provider::ProviderKind;
use serde::{Deserialize, Serialize};
use std::fmt;
#[cfg(not(target_arch = "wasm32"))]
use {
    crate::provider::{Provider, ProviderError},
    std::sync::Arc,
};

/// Overrides the provider setting: `anthropic` or `openrouter`.
pub const PROVIDER_VAR: &str = "SOLIDDRAFT_PROVIDER";
/// Overrides the model setting.
pub const MODEL_VAR: &str = "SOLIDDRAFT_MODEL";

/// The assistant's settings, as the app saves them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AssistantSettings {
    /// The provider. `None` picks by the keys present.
    pub provider: Option<ProviderKind>,
    /// The model on the Anthropic API. `None` uses its default.
    pub anthropic_model: Option<String>,
    /// The model on OpenRouter. `None` uses its default.
    pub openrouter_model: Option<String>,
}

impl AssistantSettings {
    /// The model set for a provider, if any.
    pub fn model(&self, kind: ProviderKind) -> Option<&str> {
        match kind {
            ProviderKind::Anthropic => self.anthropic_model.as_deref(),
            ProviderKind::OpenRouter => self.openrouter_model.as_deref(),
        }
        .filter(|m| !m.trim().is_empty())
    }

    /// Sets a provider's model; an empty one goes back to the default.
    pub fn set_model(&mut self, kind: ProviderKind, model: &str) {
        let model = Some(model.trim().to_owned()).filter(|m| !m.is_empty());
        match kind {
            ProviderKind::Anthropic => self.anthropic_model = model,
            ProviderKind::OpenRouter => self.openrouter_model = model,
        }
    }
}

/// A provider with its key and model, ready to connect.
#[derive(Debug)]
pub struct Choice {
    /// The provider.
    pub kind: ProviderKind,
    /// The model id, in the provider's form.
    pub model: String,
    /// The key.
    pub key: ApiKey,
    /// Where the key came from.
    pub source: KeySource,
}

impl Choice {
    /// Builds the provider. The web build has no HTTP providers of its own; its model is
    /// on its server.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn connect(self) -> Result<Arc<dyn Provider>, ProviderError> {
        self.kind.connect(self.key, &self.model)
    }
}

/// Why no provider could be chosen. Displays as a sentence for the chat panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoProvider {
    /// No provider has a key.
    NoKey,
    /// The provider asked for, by setting or by `SOLIDDRAFT_PROVIDER`, has no key.
    MissingKey(ProviderKind),
    /// `SOLIDDRAFT_PROVIDER` names no provider.
    UnknownProvider(String),
}

impl fmt::Display for NoProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NoProvider::NoKey => write!(
                f,
                "No API key, so the assistant is off. Add an OpenRouter or Anthropic key under Provider on the \
                 Assistant tab. Or set {} or {}, or run `soliddraft key set openrouter` or \
                 `soliddraft key set anthropic`, and restart.",
                ProviderKind::OpenRouter.key_var(),
                ProviderKind::Anthropic.key_var(),
            ),
            NoProvider::MissingKey(kind) => {
                let other = ProviderKind::ALL.into_iter().find(|k| k != kind).unwrap_or(*kind);
                write!(
                    f,
                    "The assistant is set to {kind}, which has no API key. Add one under Provider on the Assistant \
                     tab, or choose {other} there. Or set {}, or run `soliddraft key set {}`, and restart.",
                    kind.key_var(),
                    kind.id(),
                )
            }
            NoProvider::UnknownProvider(name) => {
                write!(f, "{PROVIDER_VAR} is {name:?}, which is not a provider. Use openrouter or anthropic.")
            }
        }
    }
}

/// Chooses the provider, its key and its model from the environment, the settings and the
/// keys present.
pub fn choose(settings: &AssistantSettings) -> Result<Choice, NoProvider> {
    choose_with(settings, crate::environment::var(PROVIDER_VAR), crate::environment::var(MODEL_VAR), key::find)
}

fn choose_with(
    settings: &AssistantSettings,
    provider_var: Option<String>,
    model_var: Option<String>,
    find: impl Fn(ProviderKind) -> Option<(ApiKey, KeySource)>,
) -> Result<Choice, NoProvider> {
    let asked = match provider_var.filter(|v| !v.trim().is_empty()) {
        Some(v) => Some(ProviderKind::parse(&v).ok_or(NoProvider::UnknownProvider(v))?),
        None => settings.provider,
    };
    let (kind, (key, source)) = match asked {
        Some(kind) => (kind, find(kind).ok_or(NoProvider::MissingKey(kind))?),
        None => [ProviderKind::Anthropic, ProviderKind::OpenRouter]
            .into_iter()
            .find_map(|k| find(k).map(|found| (k, found)))
            .ok_or(NoProvider::NoKey)?,
    };
    let model = model_var
        .filter(|m| !m.trim().is_empty())
        .or_else(|| settings.model(kind).map(str::to_owned))
        .unwrap_or_else(|| kind.default_model().to_owned());
    Ok(Choice { kind, model, key, source })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(present: &[ProviderKind]) -> impl Fn(ProviderKind) -> Option<(ApiKey, KeySource)> + '_ {
        move |k| present.contains(&k).then(|| (ApiKey::new("k").unwrap(), KeySource::Environment(k)))
    }

    #[test]
    fn the_key_present_picks_the_provider() {
        let s = AssistantSettings::default();
        let c = choose_with(&s, None, None, keys(&[ProviderKind::OpenRouter])).unwrap();
        assert_eq!((c.kind, c.model.as_str()), (ProviderKind::OpenRouter, crate::OPENROUTER_DEFAULT_MODEL));
        let c = choose_with(&s, None, None, keys(&[ProviderKind::Anthropic])).unwrap();
        assert_eq!((c.kind, c.model.as_str()), (ProviderKind::Anthropic, crate::DEFAULT_MODEL));
        let c = choose_with(&s, None, None, keys(&ProviderKind::ALL)).unwrap();
        assert_eq!(c.kind, ProviderKind::Anthropic);
        assert_eq!(choose_with(&s, None, None, keys(&[])).unwrap_err(), NoProvider::NoKey);
    }

    #[test]
    fn the_setting_and_the_environment_choose_over_the_keys() {
        let mut s = AssistantSettings { provider: Some(ProviderKind::OpenRouter), ..Default::default() };
        s.set_model(ProviderKind::OpenRouter, " anthropic/claude-fable-5.1 ");
        let c = choose_with(&s, None, None, keys(&ProviderKind::ALL)).unwrap();
        assert_eq!((c.kind, c.model.as_str()), (ProviderKind::OpenRouter, "anthropic/claude-fable-5.1"));
        let c = choose_with(&s, Some("Anthropic".into()), Some("claude-x".into()), keys(&ProviderKind::ALL)).unwrap();
        assert_eq!((c.kind, c.model.as_str()), (ProviderKind::Anthropic, "claude-x"));
        // A provider asked for by name does not fall back to the other one.
        let e = choose_with(&s, None, None, keys(&[ProviderKind::Anthropic])).unwrap_err();
        assert_eq!(e, NoProvider::MissingKey(ProviderKind::OpenRouter));
        assert!(e.to_string().contains("OPENROUTER_API_KEY"));
        let e = choose_with(&s, Some("bedrock".into()), None, keys(&ProviderKind::ALL)).unwrap_err();
        assert!(matches!(e, NoProvider::UnknownProvider(_)));
    }

    #[test]
    fn the_no_key_message_names_both_providers() {
        let text = NoProvider::NoKey.to_string();
        for needle in ["OpenRouter", "Anthropic", "OPENROUTER_API_KEY", "ANTHROPIC_API_KEY", "key set openrouter"] {
            assert!(text.contains(needle), "{text}");
        }
    }

    #[test]
    fn settings_round_trip_and_tolerate_missing_fields() {
        let s: AssistantSettings = serde_json::from_str(r#"{"provider":"openrouter"}"#).unwrap();
        assert_eq!(s.provider, Some(ProviderKind::OpenRouter));
        assert_eq!(s.model(ProviderKind::OpenRouter), None);
        let back: AssistantSettings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }
}
