//! Sparse named-value overrides, resolved from one persisted feature base.

use crate::command::Applied;
use crate::{Command, CommandError, Document, Feature, values::Values};
use serde::{Deserialize, Serialize};
use soliddraft_document::{ConfigurationId, DocumentId};
use std::collections::{BTreeMap, BTreeSet};

/// Configuration rows and their common sketch starting state.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configurations {
    rows: Vec<Row>,
    active: Option<ConfigurationId>,
    base: Vec<Feature>,
    #[serde(skip)]
    resolved: Option<Values>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    id: ConfigurationId,
    name: String,
    overrides: BTreeMap<String, String>,
}

impl Configurations {
    /// Whether the document has only its implicit Default row.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub(crate) fn lookup<'a>(&'a self, defaults: &'a Values) -> &'a Values {
        self.resolved.as_ref().unwrap_or(defaults)
    }

    fn row(&self, name: &str) -> Result<usize, String> {
        self.rows.iter().position(|r| r.name == name).ok_or_else(|| format!("unknown configuration {name}"))
    }

    pub(crate) fn validate(&mut self, defaults: &Values, features: &[Feature]) -> Result<Vec<Feature>, String> {
        if self.is_empty() {
            if self.active.is_some() || !self.base.is_empty() {
                return Err("configuration state has no rows".into());
            }
            self.resolved = None;
            return defaults.validate(features);
        }
        if self.rows.len() > 256 {
            return Err("at most 256 configurations are supported".into());
        }
        if self.base.len() != features.len()
            || self.base.iter().zip(features).any(|(a, b)| {
                a.id != b.id || a.name != b.name || a.suppressed != b.suppressed || a.via_assistant != b.via_assistant
            })
        {
            return Err("configuration base disagrees with the feature tree".into());
        }
        for feature in &self.base {
            Document::check_kind(&feature.kind).map_err(|e| e.to_string())?;
        }
        Document::check_order(&self.base).map_err(|e| e.to_string())?;
        Document::check_field_last(&self.base).map_err(|e| e.to_string())?;
        defaults.validate(&self.base)?;
        let mut ids = BTreeSet::new();
        let mut names = BTreeSet::new();
        let mut active_values = None;
        for row in &self.rows {
            check_name(&row.name)?;
            if row.id.0.is_empty() || !ids.insert(&row.id) || !names.insert(&row.name) {
                return Err("duplicate configuration id or name".into());
            }
            let values = defaults.overlay(&row.overrides).map_err(|e| format!("{}: {e}", row.name))?;
            values.validate(&self.base).map_err(|e| format!("{}: {e}", row.name))?;
            if self.active.as_ref() == Some(&row.id) {
                active_values = Some(values);
            }
        }
        if self.active.is_some() && active_values.is_none() {
            return Err("unknown active configuration".into());
        }
        let features = active_values.as_ref().unwrap_or(defaults).validate(&self.base)?;
        self.resolved = active_values;
        Ok(features)
    }

    pub(crate) fn capture_edits(&mut self, before: &[Feature], after: &[Feature]) -> bool {
        if self.is_empty() {
            return false;
        }
        let next: Vec<_> = after
            .iter()
            .map(|f| {
                let mut base = f.clone();
                if before.iter().find(|b| b.id == f.id).is_some_and(|b| b.kind == f.kind)
                    && let Some(old) = self.base.iter().find(|b| b.id == f.id)
                {
                    base.kind = old.kind.clone();
                }
                base
            })
            .collect();
        let changed = next != self.base;
        self.base = next;
        changed
    }
}

fn check_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() || name.len() > 128 || name == "Default" {
        return Err("a configuration needs a nonempty name other than reserved Default".into());
    }
    Ok(())
}

pub(crate) fn handles(command: &Command, has_rows: bool) -> bool {
    matches!(
        command,
        Command::AddConfiguration { .. }
            | Command::SetConfigurationValue { .. }
            | Command::ActivateConfiguration { .. }
            | Command::RestoreConfigurations { .. }
    ) || has_rows && matches!(command, Command::SetValue { .. } | Command::Drive { .. } | Command::RenameValue { .. })
}

impl Document {
    /// The selected row's stable identity; None denotes Default.
    pub fn active_configuration(&self) -> Option<&ConfigurationId> {
        self.configurations.active.as_ref()
    }

    /// Configuration names and authored overrides in creation order.
    pub fn configuration_summary(&self) -> serde_json::Value {
        let state = &self.configurations;
        let active =
            state.rows.iter().find(|r| Some(&r.id) == state.active.as_ref()).map_or("Default", |r| r.name.as_str());
        let mut rows = vec![serde_json::json!({"name":"Default", "overrides":{}})];
        rows.extend(state.rows.iter().map(|r| serde_json::json!({"name":r.name,"overrides":r.overrides})));
        serde_json::json!({"active":active,"rows":rows})
    }

    pub(crate) fn apply_configuration(&mut self, command: Command) -> Result<Applied, CommandError> {
        let old = Command::RestoreConfigurations {
            state: Box::new(self.configurations.clone()),
            values: self.values.clone(),
            features: self.features.clone(),
        };
        let mut state = self.configurations.clone();
        let mut defaults = self.values.clone();
        let mut features = self.features.clone();
        let result = (|| -> Result<String, String> {
            let label = match &command {
                Command::AddConfiguration { name } => {
                    check_name(name)?;
                    if state.rows.iter().any(|r| r.name == *name) {
                        return Err(format!("configuration {name} already exists"));
                    }
                    if state.is_empty() {
                        state.base = features.clone();
                    }
                    state.rows.push(Row {
                        id: ConfigurationId(DocumentId::new().to_string()),
                        name: name.clone(),
                        overrides: BTreeMap::new(),
                    });
                    format!("add configuration {name}")
                }
                Command::SetConfigurationValue { configuration, name, expression } => {
                    let i = state.row(configuration)?;
                    state.rows[i].overrides.insert(name.clone(), expression.clone());
                    let layer = defaults.overlay(&state.rows[i].overrides)?;
                    state.rows[i].overrides.insert(name.clone(), layer.expression(name).ok_or("unknown value")?.into());
                    format!("set {configuration} value {name}")
                }
                Command::ActivateConfiguration { name } => {
                    state.active = if name == "Default" { None } else { Some(state.rows[state.row(name)?].id.clone()) };
                    format!("activate configuration {name}")
                }
                Command::SetValue { name, expression } => {
                    if let Some(row) = state
                        .rows
                        .iter_mut()
                        .find(|r| Some(&r.id) == state.active.as_ref() && r.overrides.contains_key(name))
                    {
                        row.overrides.insert(name.clone(), expression.clone());
                        let layer = defaults.overlay(&row.overrides)?;
                        row.overrides.insert(name.clone(), layer.expression(name).ok_or("unknown value")?.into());
                    } else {
                        defaults.set(name, expression)?;
                    }
                    format!("set value {name}")
                }
                Command::Drive { input, expression } => {
                    defaults.drive(input.clone(), expression)?;
                    "drive input".into()
                }
                Command::RenameValue { name, to } => {
                    defaults.rename(name, to)?;
                    for row in &mut state.rows {
                        if let Some(text) = row.overrides.remove(name) {
                            row.overrides.insert(to.clone(), text);
                        }
                        for text in row.overrides.values_mut() {
                            *text = crate::value_expr::rename_tokens(text, name, to);
                        }
                    }
                    format!("rename value {name}")
                }
                Command::RestoreConfigurations { state: saved, values, features: saved_features } => {
                    state = *saved.clone();
                    defaults = values.clone();
                    features = saved_features.clone();
                    "restore configurations".into()
                }
                _ => unreachable!(),
            };
            features = state.validate(&defaults, &features)?;
            Ok(label)
        })();
        let label = result.map_err(CommandError::Invalid)?;
        if features.len() != self.features.len()
            || features.iter().zip(&self.features).any(|(a, b)| {
                a.id != b.id
                    || a.name != b.name
                    || a.suppressed != b.suppressed
                    || a.via_assistant != b.via_assistant
                    || std::mem::discriminant(&a.kind) != std::mem::discriminant(&b.kind)
            })
        {
            return Err(CommandError::Invalid("a configuration command cannot replace the feature tree".into()));
        }
        let first = self.features.iter().zip(&features).position(|(a, b)| a.kind != b.kind);
        self.configurations = state;
        self.values = defaults;
        self.features = features;
        if let Some(i) = first {
            self.touch(i);
        }
        let redo = Command::RestoreConfigurations {
            state: Box::new(self.configurations.clone()),
            values: self.values.clone(),
            features: self.features.clone(),
        };
        Ok(Applied { inverse: old, redo, created: None, label })
    }
}
