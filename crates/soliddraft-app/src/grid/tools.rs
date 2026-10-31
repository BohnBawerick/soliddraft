//! Configuration verbs use the same document commands as other part edits.
use crate::{command_tools::Line, document::Command};
use serde_json::{Value, json};

pub(crate) fn name_params() -> Vec<(&'static str, Value)> {
    vec![("name", json!({"type":"string", "description":"Configuration name"}))]
}

pub(crate) fn value_params() -> Vec<(&'static str, Value)> {
    vec![
        ("configuration", json!({"type":"string", "description":"Non-default configuration name"})),
        ("name", json!({"type":"string", "description":"Existing named value"})),
        ("expression", json!({"type":"string", "description":"Value expression"})),
    ]
}

pub(crate) fn command(line: &Line) -> Result<Command, String> {
    Ok(match line {
        Line::AddConfiguration { name } => Command::AddConfiguration { name: name.clone() },
        Line::SetConfigurationValue { configuration, name, expression } => Command::SetConfigurationValue {
            configuration: configuration.clone(),
            name: name.clone(),
            expression: expression.clone(),
        },
        Line::ActivateConfiguration { name } => Command::ActivateConfiguration { name: name.clone() },
        _ => return Err("not a configuration command".into()),
    })
}

/// Keep tracked readouts while showing the selected row's named expressions and numbers.
pub(crate) fn values(mut all: Value, active: Value) -> Value {
    if let (Some(rows), Some(active)) = (all.as_array_mut(), active.as_array()) {
        for row in rows {
            if let Some(selected) = active.iter().find(|value| value["name"] == row["name"]) {
                *row = selected.clone();
            }
        }
    }
    all
}
