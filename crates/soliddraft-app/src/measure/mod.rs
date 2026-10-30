//! Proven picked quantities and tracked checks.

pub mod guides;
pub mod hover;
pub mod panel;
pub(crate) mod registry;
pub(crate) mod tools;

use crate::document::{Document, Selection};
use serde_json::{Value, json};
use soliddraft_kernel_api::{PairEndpoint, PairQuantity};
use soliddraft_model::tracked::{Endpoint, Entity, Source};

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
        "geometry": doc.model().geometry_hash(), "source_revision": doc.revision()}))
}

/// One end of a pair request, from a picked edge or face.
fn endpoint(doc: &Document, selection: Selection, midpoint: bool) -> Result<Endpoint, String> {
    match (source(doc, selection)?, midpoint) {
        (Source::Edge { reference }, true) => Ok(Endpoint::EdgeMidpoint { reference }),
        (Source::Edge { reference }, false) => Ok(Endpoint::Edge { reference }),
        (Source::Face { reference }, false) => Ok(Endpoint::Face { reference }),
        (other, _) => Err(format!("measure.unsupported-pair: {} has no midpoint", other.label())),
    }
}

/// The persistent request for `quantity` between two picks. A point distance joins
/// their midpoints; the other quantities measure the whole entities.
pub fn pair(doc: &Document, a: Selection, b: Selection, quantity: PairQuantity) -> Result<Source, String> {
    let midpoint = quantity == PairQuantity::PointDistance;
    let source =
        Source::Pair { quantity, a: Box::new(endpoint(doc, a, midpoint)?), b: Box::new(endpoint(doc, b, midpoint)?) };
    source.validate()?;
    Ok(source)
}

/// A pair proof with both persistent sources. `distance_mm` and `gap_mm` keep the
/// earlier two-entity tool's fields, now carrying the proven minimum distance.
pub fn measure_pair(doc: &Document, source: &Source) -> Result<Value, String> {
    let Source::Pair { quantity, a, b } = source else { return Err("a pair needs two sources".into()) };
    let m = doc.model().measure_source(source)?;
    let sources = [a, b].map(|e| json!({"source": e.label(), "point": e.point(), "reference": e}));
    let mut v = json!({"quantity": source.quantity(), "unit": source.unit(), "value": m.value, "lo": m.lo, "hi": m.hi,
        "certainty": m.certainty, "frame": "part", "geometry": doc.model().geometry_hash(),
        "source_revision": doc.revision(), "sources": sources});
    if *quantity == PairQuantity::MinimumDistance {
        v["distance_mm"] = m.value.into();
        if matches!((&**a, &**b), (Endpoint::Face { .. }, Endpoint::Face { .. })) {
            v["gap_mm"] = m.value.into();
        }
    }
    Ok(v)
}

/// The proven midpoint of a straight edge: one enclosure per part-frame coordinate.
pub fn midpoint(doc: &Document, selection: Selection) -> Result<Value, String> {
    let end = endpoint(doc, selection, true)?;
    let Endpoint::EdgeMidpoint { reference } = &end else { return Err("only an edge has a midpoint".into()) };
    let p = doc.model().measure_midpoint(reference)?;
    Ok(json!({"quantity": "midpoint", "unit": "mm",
        "coordinates": p.coordinates.map(|c| json!({"value": c.value, "lo": c.lo, "hi": c.hi, "certainty": c.certainty})),
        "frame": "part", "geometry": doc.model().geometry_hash(), "source_revision": doc.revision(),
        "sources": [{"source": end.label(), "point": "midpoint", "reference": end}]}))
}

/// App labels follow resolved references through feature renames and reordered edges.
pub fn values(doc: &Document) -> Value {
    let mut values = doc.model().values_summary();
    if let Some(body) = doc.model().body() {
        let title = |s: Selection| doc.describe(s).map(|i| i.title);
        for row in doc.model().tracked() {
            let Some(value) = values.as_array_mut().and_then(|rows| rows.iter_mut().find(|v| v["name"] == row.name))
            else {
                continue;
            };
            match &row.source {
                Source::Pair { a, b, .. } => {
                    // A stale pair names both ends as they were defined: the end that
                    // still resolves may carry another interface number by now.
                    let current = [a, b].map(|e| match e.resolve(doc.model(), body) {
                        Ok(PairEndpoint::Edge(edge) | PairEndpoint::EdgeMidpoint(edge)) => title(Selection::Edge(edge)),
                        Ok(PairEndpoint::Face(face)) => title(Selection::Face(face)),
                        Err(_) => None,
                    });
                    let titles = match current {
                        [Some(a), Some(b)] => [a, b],
                        _ => [a, b].map(|e| saved_title(doc, e)),
                    };
                    value["tracked"]["source"] = titles.join(" and ").into();
                    for (i, t) in titles.into_iter().enumerate() {
                        value["tracked"]["sources"][i]["source"] = t.into();
                    }
                }
                source => {
                    let shown = match source.resolve(doc.model(), body) {
                        Ok(Entity::Edge(edge)) => title(Selection::Edge(edge)),
                        Ok(Entity::Face(face)) => title(Selection::Face(face)),
                        _ => None,
                    };
                    if let Some(t) = shown {
                        value["tracked"]["source"] = t.into();
                    }
                }
            }
        }
    }
    values
}

/// A pair end's saved label, under its feature's current name when that feature has
/// been renamed since.
fn saved_title(doc: &Document, end: &Endpoint) -> String {
    let features = match end {
        Endpoint::Edge { reference } | Endpoint::EdgeMidpoint { reference } => {
            reference.faces.iter().map(|f| f.feature).collect()
        }
        Endpoint::Face { reference } => vec![reference.feature],
    };
    let names: std::collections::BTreeSet<&str> =
        features.into_iter().filter_map(|id| doc.model().feature(id)).map(|f| f.name.as_str()).collect();
    match (end.label().split_once(" · "), names.first()) {
        (Some((saved, rest)), Some(name)) if names.len() == 1 && !names.contains(saved) => format!("{name} · {rest}"),
        _ => end.label().to_owned(),
    }
}
