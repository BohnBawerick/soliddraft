//! Assembly commands shared by MCP and the scenario runner.

use super::*;
use crate::mates::tools::refused;
use serde_json::{Value, json};
use soliddraft_assembly::Step;

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
        let (status, now) = freshness(def);
        json!({
            "path": table.path(row),
            "document": def.key.document,
            "shown": def.key.revision,
            "pinned": reference.map(|r| &r.pinned),
            "now": now, "status": status, "warning": warning(def), "floating": true,
            "translation": placement.translation.to_array(), "rotation": placement.rotation.to_array(),
            "bounds": if bounds.is_empty() { Value::Null } else { json!({ "min": bounds.min.to_array(), "max": bounds.max.to_array() }) },
            "display": def.state.as_str(),
        })
    }).collect();
    let (mates, values, solver) = crate::mates::tools::rows(s);
    let occurrences = occurrences(s);
    Some(json!({
        "active": state.assembly.active, "name": s.doc.assembly().name, "path": s.doc.path(),
        "revision": s.doc.revision(), "geometry_hash": s.doc.assembly().geometry_hash(), "dirty": s.doc.is_dirty(), "undo": s.doc.undo_label(),
        "components": components, "occurrences": occurrences, "mates": mates, "values": values, "solver": solver,
        "proof_rebuilds": s.doc.parts_rebuilt_for_proofs(),
        "accepted_state": accepted_state(s),
    }))
}

/// How a definition's pinned revision compares with its document now, and the revision
/// now when it is known.
fn freshness(def: &soliddraft_assembly::Definition) -> (&'static str, Option<&soliddraft_document::ContentHash>) {
    match &def.freshness {
        Freshness::Current => ("current", Some(&def.key.revision)),
        Freshness::NewerAvailable { now } => ("newer_available", Some(now)),
        Freshness::Missing | Freshness::Unchecked => ("missing", None),
    }
}

/// The occurrences inside sub-assemblies whose documents have moved on. The root's Update
/// does not reach them: each sub-assembly is updated on its own, then the root accepts it.
fn nested_newer(s: &AssemblySession) -> Value {
    let table = s.doc.table();
    let rows: Vec<Value> = (1..table.len() as u32)
        .map(OccurrenceIndex)
        .filter(|r| !table.flags(*r).has(Flags::REMOVED) && table.parent(*r) != Some(OccurrenceIndex::ROOT))
        .filter_map(|r| {
            let def = s.doc.store().get(table.definition(r));
            let Freshness::NewerAvailable { now } = &def.freshness else { return None };
            let holder = table.parent(r).map(|p| table.path(p)).unwrap_or_default();
            Some(json!({ "path": table.path(r), "holder": holder, "document": def.key.document, "pinned": def.key.revision, "now": now }))
        })
        .collect();
    Value::Array(rows)
}

/// The motions of a rigid body, as the solver names them.
const MOTIONS: [&str; 6] = ["translate_x", "translate_y", "translate_z", "spin_x", "spin_y", "spin_z"];

/// Every occurrence at any depth, keyed by its path: where it comes from, where it sits in
/// its own assembly and in the root, what it shows, the mates it takes part in and the
/// motions it keeps on its own.
fn occurrences(s: &mut AssemblySession) -> Value {
    let participation = s.doc.mate_participation();
    let free = s.doc.solver().free;
    let table = s.doc.table();
    let mut out = serde_json::Map::new();
    for row in (1..table.len() as u32).map(OccurrenceIndex) {
        let flags = table.flags(row);
        if flags.has(Flags::REMOVED) {
            continue;
        }
        let path = table.path(row);
        let def = s.doc.store().get(table.definition(row));
        let reference = s.doc.reference(table.definition(row));
        let parent = table.parent(row).map(|p| table.path(p)).unwrap_or_default();
        let (local, world, bounds) = (table.local(row), table.world(row), table.bounds(row));
        let mates = participation.get(&path).cloned().unwrap_or_default();
        // A part no mate reads keeps all six; a pattern member follows its seed.
        let alone = mates.is_empty()
            && !flags.has(Flags::ASSEMBLY)
            && !flags.has(Flags::SUPPRESSED)
            && path.steps().iter().all(|s| s.member.is_none());
        let motions: Vec<&str> = if alone {
            MOTIONS.to_vec()
        } else {
            free.iter()
                .filter(|m| {
                    m.path.clone().unwrap_or_else(|| OccurrencePath::root().child(Step::component(m.component))) == path
                })
                .map(|m| m.motion.as_str())
                .collect()
        };
        out.insert(path.to_string(), json!({
            "path": path, "parent": parent, "kind": if flags.has(Flags::ASSEMBLY) { "assembly" } else { "part" },
            "document": def.key.document, "configuration": def.key.configuration, "pinned": reference.map(|r| &r.pinned),
            "definition": { "document": def.key.document, "revision": def.key.revision, "quality": def.key.quality },
            "display": def.state.as_str(), "warning": warning(def), "status": freshness(def).0,
            "local": { "translation": local.translation.to_array(), "rotation": local.rotation.to_array() },
            "world": { "translation": world.translation.to_array(), "rotation": world.rotation.to_array() },
            "bounds": if bounds.is_empty() { Value::Null } else { json!({ "min": bounds.min.to_array(), "max": bounds.max.to_array() }) },
            "mates": mates, "free": motions,
        }));
    }
    Value::Object(out)
}

/// The accepted state, read without saving: what a refusal must leave exactly as it was.
/// Freshness, repair messages and timing are not part of it.
fn accepted_state(s: &AssemblySession) -> Value {
    let table = s.doc.table();
    let rows: Vec<_> = (1..table.len() as u32)
        .map(OccurrenceIndex)
        .filter(|r| !table.flags(*r).has(Flags::REMOVED))
        .map(|r| {
            let key = &s.doc.store().get(table.definition(r)).key;
            json!({
                "path": table.path(r), "document": key.document, "revision": key.revision,
                "local": table.local(r), "world": table.world(r),
            })
        })
        .collect();
    let mut definitions: Vec<_> = s
        .doc
        .store()
        .iter()
        .filter(|(i, _)| *i != s.doc.root())
        .map(|(_, d)| json!({ "document": d.key.document, "revision": d.key.revision, "quality": d.key.quality }))
        .collect();
    definitions.sort_by_key(Value::to_string);
    json!({
        "truth": s.doc.assembly(), "revision": s.doc.revision(), "geometry": s.doc.assembly().geometry_hash(),
        "rows": rows, "definitions": definitions, "dirty": s.doc.is_dirty(), "next_id": s.doc.assembly().next_id,
        "undo": { "label": s.doc.undo_label(), "depth": s.doc.undo_depth() },
        "redo": { "label": s.doc.redo_label(), "depth": s.doc.redo_depth() },
    })
}

/// Places one file, as Insert component does, at `placement` when one is given.
fn insert_one(state: &mut AppState, input: &Value) -> Result<Value, String> {
    let paths: Vec<&str> = input["paths"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let [file] = paths[..] else { return Err("paths names one file to insert".into()) };
    let placement = match &input["placement"] {
        Value::Null => None,
        p => Some(serde_json::from_value::<Placement>(p.clone()).map_err(|e| format!("the placement: {e}"))?),
    };
    if placement.is_some_and(|p| !(p.translation.is_finite() && p.rotation.is_finite() && p.rotation.length() > 0.5)) {
        return Err("a placement needs a finite translation and a unit rotation".into());
    }
    let revisions = state.revisions.clone();
    let s = state.assembly.session.as_mut().ok_or("no assembly is open")?;
    let path = place(s, &*revisions, Path::new(file), placement)?;
    s.doc.finish_loading();
    s.selected = Some(path.clone());
    Ok(json!({ "inserted": path, "assembly": summary(state) }))
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
        "insert_component" => insert_one(state, input),
        "assembly_update" => update(state)
            .map(|report| {
                let nested = state.assembly.session.as_ref().map(nested_newer).unwrap_or_default();
                json!({
                    "updated": report.updated(), "unchanged": report.sources.len() - report.updated(),
                    "nested_newer": nested,
                    "assembly": summary(state),
                })
            })
            .map_err(|report| match report.mate_refusal {
                Some(r) => refused(r),
                None => report.refusal.unwrap_or_default(),
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
