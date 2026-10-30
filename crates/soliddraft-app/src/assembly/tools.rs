//! Assembly commands shared by MCP and the scenario runner.

use super::*;
use serde_json::{Value, json};

pub(crate) fn summary(state: &mut AppState) -> Option<Value> {
    refresh(state);
    let s = state.assembly.session.as_mut()?;
    s.doc.finish_loading();
    // A loader may have reported file freshness while the open part was edited.
    let part = state.assembly.last_part.clone().into_iter().collect();
    s.doc.refresh_references(part);
    let components: Vec<_> = s.components().into_iter().map(|row| {
        let table = s.doc.table();
        let def = s.doc.store().get(table.definition(row));
        let reference = s.doc.reference(table.definition(row));
        let placement = table.local(row);
        let bounds = table.bounds(row);
        let (status, now) = match &def.freshness {
            Freshness::Current => ("current", Some(&def.key.revision)),
            Freshness::NewerAvailable { now } => ("newer_available", Some(now)),
            Freshness::Missing | Freshness::Unchecked => ("missing", None),
        };
        json!({
            "document": def.key.document,
            "pinned": reference.map(|r| &r.pinned),
            "now": now, "status": status, "warning": warning(def), "floating": true,
            "translation": placement.translation.to_array(), "rotation": placement.rotation.to_array(),
            "bounds": if bounds.is_empty() { Value::Null } else { json!({ "min": bounds.min.to_array(), "max": bounds.max.to_array() }) },
            "display": def.state.as_str(),
        })
    }).collect();
    Some(json!({
        "active": state.assembly.active, "name": s.doc.assembly().name, "path": s.doc.path(),
        "revision": s.doc.revision(), "dirty": s.doc.is_dirty(), "undo": s.doc.undo_label(),
        "components": components,
    }))
}

pub(crate) fn open(state: &mut AppState, path: PathBuf) -> Result<Value, String> {
    let mut doc = AssemblyDocument::open(&path, options(state)).map_err(|e| e.to_string())?;
    doc.finish_loading();
    let scene = Scene::new(&mut doc);
    state.assembly.session = Some(AssemblySession::new(doc, scene));
    state.assembly.active = true;
    state.drawing.active = false;
    Ok(json!({ "opened": path, "assembly": summary(state) }))
}

pub(crate) fn step(state: &mut AppState, undo: bool) -> Result<Value, String> {
    let s = state.assembly.session.as_mut().ok_or("no assembly is open")?;
    let label = if undo { s.doc.undo_label() } else { s.doc.redo_label() }
        .ok_or(if undo { "nothing to undo in the assembly" } else { "nothing to redo in the assembly" })?
        .to_owned();
    if undo {
        s.doc.undo();
    } else {
        s.doc.redo();
    }
    s.doc.finish_loading();
    Ok(json!({ "done": label, "document": "assembly", "assembly": summary(state) }))
}

pub(crate) fn run(state: &mut AppState, name: &str, input: &Value) -> Option<Result<Value, String>> {
    Some(match name {
        "make_assembly" => from_part(state).map(|()| json!({ "assembly": summary(state) })),
        "show_assembly" => {
            if state.assembly.session.is_none() {
                return Some(Err("no assembly is open".into()));
            }
            state.assembly.active = true;
            state.drawing.active = false;
            Ok(json!({ "assembly": summary(state) }))
        }
        "assembly_show_part" => {
            state.assembly.active = false;
            state.drawing.active = false;
            Ok(json!({ "active": "part" }))
        }
        "assembly_update" => update(state).map(|report| {
            json!({
                "updated": report.updated(), "unchanged": report.sources.len() - report.updated(),
                "assembly": summary(state),
            })
        }),
        "assembly_save" => {
            let path = PathBuf::from(input["path"].as_str().unwrap_or_default());
            if DocKind::of_path(&path) != Some(DocKind::Assembly) {
                return Some(Err("the path must end in .dwasm".into()));
            }
            let Some(s) = state.assembly.session.as_mut() else {
                return Some(Err("no assembly is open".into()));
            };
            s.doc.finish_loading();
            s.doc.save(Some(&path)).map_err(|e| e.to_string()).map(|saved| {
                json!({
                    "saved": saved, "assembly": summary(state),
                })
            })
        }
        _ => return None,
    })
}
