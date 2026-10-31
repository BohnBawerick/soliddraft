//! The Form subassembly command over MCP.

use crate::assembly::{AssemblySession, tools::summary};
use crate::state::AppState;
use serde_json::{Value, json};
use soliddraft_assembly::{
    AssemblyPackage, ComponentId, FormCode, FormRefusal, FormRequest, Formed, OccurrencePath, Step,
};
use std::path::PathBuf;

/// A refusal as a tool's error: typed JSON with its cause.
fn refused(r: &FormRefusal) -> String {
    serde_json::to_string(r).unwrap_or_else(|_| r.to_string())
}

fn invalid(message: impl Into<String>) -> String {
    let r = FormRefusal { code: FormCode::Invalid, message: message.into(), cause: None, expected: None, actual: None };
    refused(&r)
}

/// An occurrence as a tool names it: its path as text, such as "4" or "16/4", or a root
/// component's id.
fn occurrence(v: &Value) -> Result<OccurrencePath, String> {
    match v {
        Value::String(text) => text.parse().map_err(|e: soliddraft_assembly::path::PathError| invalid(e.to_string())),
        Value::Number(n) => n
            .as_u64()
            .and_then(|c| u32::try_from(c).ok())
            .map(|c| OccurrencePath::root().child(Step::component(ComponentId(c))))
            .ok_or_else(|| invalid("a component id is a whole number")),
        _ => Err(invalid("an occurrence is a path such as \"4\", or a component id")),
    }
}

fn request(input: &Value) -> Result<FormRequest, String> {
    let paths = input["paths"].as_array().ok_or_else(|| invalid("paths lists the parts to form"))?;
    let paths = paths.iter().map(occurrence).collect::<Result<Vec<_>, _>>()?;
    let path =
        input["path"].as_str().filter(|p| !p.trim().is_empty()).ok_or_else(|| invalid("path names the new file"))?;
    let origin = occurrence(&input["origin"])?;
    Ok(FormRequest { paths, path: PathBuf::from(path), origin })
}

/// The selection follows the parts into the new file, and the tree opens on it.
fn follow(s: &mut AssemblySession, formed: &Formed) {
    s.selected = s.selected.take().map(|p| formed.migration.map(&p));
    s.expanded.insert(OccurrencePath::root().child(Step::component(formed.migration.component)));
}

fn form(state: &mut AppState, input: &Value) -> Result<Value, String> {
    let request = request(input)?;
    let s = state.assembly.session.as_mut().ok_or("no assembly is open")?;
    let formed = s.doc.form_subassembly(&request).map_err(|r| refused(&r))?;
    follow(s, &formed);
    // The new file's bytes, so this session can show and prove its pinned revision later
    // whatever happens to the file.
    let bytes = std::fs::read(&formed.path).map_err(|e| e.to_string())?;
    state.revisions.record(formed.migration.child, &formed.migration.child_revision, &bytes);
    let child = AssemblyPackage::from_bytes(bytes).map_err(|e| e.to_string())?;
    let selected = s.selected.clone();
    Ok(json!({
        "formed": formed,
        "child": {
            "path": formed.path, "id": child.id(), "revision": child.manifest().semantic_hash,
            "truth": &**child.assembly(), "solver": formed.status.child,
        },
        "selected": selected,
        "assembly": summary(state),
    }))
}

/// Runs the subassembly tools. `None` hands the call on.
pub(crate) fn run(state: &mut AppState, name: &str, input: &Value) -> Option<Result<Value, String>> {
    (name == "assembly_form_subassembly").then(|| form(state, input))
}
