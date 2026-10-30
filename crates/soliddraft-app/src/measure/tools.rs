//! MCP entry points for picked measurement and Track this.

use crate::{
    command_tools::find_entity,
    document::{Command, Document},
};
use serde_json::{Value, json};
use soliddraft_assistant::tools::schema::{optional, string};

pub fn measure_params() -> Vec<(&'static str, Value)> {
    vec![("entity", json!({"type":"string", "description":"An edge or face interface name"}))]
}

pub fn track_params() -> Vec<(&'static str, Value)> {
    let mut params = measure_params();
    params.push(("name", json!({"type":"string", "description":"A new value identifier"})));
    params.push(("check", optional(string("Optional <, <=, > or >= followed by one limit"))));
    params
}

pub fn call(doc: &mut Document, name: &str, input: &Value) -> Result<Value, String> {
    let entity = input["entity"].as_str().ok_or("missing entity")?;
    let selection =
        find_entity(doc, entity).ok_or_else(|| format!("no entity named {entity}; select an edge or a face"))?;
    if name == "measure_entity" {
        return super::measure(doc, selection);
    }
    let source = super::source(doc, selection)?;
    doc.apply(Command::Track {
        name: input["name"].as_str().ok_or("missing name")?.into(),
        source,
        check: input.get("check").map(|v| v.as_str().map(str::to_owned).ok_or("check must be text")).transpose()?,
    })?;
    Ok(json!({"tracked": input["name"]}))
}
