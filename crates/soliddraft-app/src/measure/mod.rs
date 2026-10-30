//! Proven picked quantities and tracked checks.

pub mod panel;
pub(crate) mod registry;
pub(crate) mod tools;

use crate::document::{Document, Selection};
use serde_json::{Value, json};
use soliddraft_kernel_api::PairEndpoint;
use soliddraft_model::tracked::{Entity, Source};

/// Names a persistent source using the interface label only as a fallback.
pub fn source(doc: &Document, selection: Selection) -> Result<Source, String> {
    if !doc.proof_state().current_proved_model() {
        return Err("proof pending or failed".into());
    }
    let label = doc.describe(selection).map(|i| i.title).unwrap_or_default();
    match selection {
        Selection::Edge(edge) => {
            let mut reference = doc.model().edge_ref(edge).map_err(|e| e.to_string())?;
            reference.label = label;
            Ok(Source::Edge { reference })
        }
        Selection::Face(face) => {
            let mut reference = doc.model().face_ref(face).map_err(|e| e.to_string())?;
            reference.label = label;
            Ok(Source::Face { reference })
        }
        _ => Err("select an edge or a face".into()),
    }
}

/// Shared proof path for viewport selections and MCP names.
pub fn measure(doc: &Document, selection: Selection) -> Result<Value, String> {
    let source = source(doc, selection)?;
    let m = doc.model().measure_source(&source)?;
    Ok(json!({"entity": source.label(), "kind": match selection { Selection::Edge(_) => "edge", _ => "face" },
        "quantity": source.quantity(), "value": m.value, "unit": source.unit(),
        "certainty": m.certainty, "lo": m.lo, "hi": m.hi, "frame": "part",
        "geometry": doc.model().geometry_hash()}))
}

/// App labels follow resolved references through feature renames and reordered edges.
pub fn values(doc: &Document) -> Value {
    let mut values = doc.model().values_summary();
    if let Some(body) = doc.model().body() {
        let title = |s: Selection| doc.describe(s).map(|i| i.title);
        let end = |e: PairEndpoint| match e {
            PairEndpoint::Edge(edge) | PairEndpoint::EdgeMidpoint(edge) => title(Selection::Edge(edge)),
            PairEndpoint::Face(face) => title(Selection::Face(face)),
        };
        for row in doc.model().tracked() {
            let Some(value) = values.as_array_mut().and_then(|rows| rows.iter_mut().find(|v| v["name"] == row.name))
            else {
                continue;
            };
            let titles = match row.source.resolve(doc.model(), body) {
                Ok(Entity::Edge(edge)) => vec![title(Selection::Edge(edge))],
                Ok(Entity::Face(face)) => vec![title(Selection::Face(face))],
                Ok(Entity::Pair(_, a, b)) => vec![end(a), end(b)],
                Err(_) => continue,
            };
            let Some(titles) = titles.into_iter().collect::<Option<Vec<String>>>() else { continue };
            value["tracked"]["source"] = titles.join(" and ").into();
            if titles.len() == 2 {
                for (i, t) in titles.into_iter().enumerate() {
                    value["tracked"]["sources"][i]["source"] = t.into();
                }
            }
        }
    }
    values
}
