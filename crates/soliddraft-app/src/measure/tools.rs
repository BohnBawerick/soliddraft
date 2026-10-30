//! MCP entry points for picked measurement and Track this.

use crate::{
    command_tools::{NAMES_HELP, find_entity},
    document::{Command, Document, Selection},
};
use serde_json::{Value, json};
use soliddraft_assistant::tools::schema::{one_of, optional, string};
use soliddraft_kernel_api::PairQuantity;

const QUANTITIES: [&str; 3] = ["minimum_distance", "point_distance", "direction_angle"];

fn point_param(which: &str) -> Value {
    optional(one_of(&["midpoint"], &format!("The proven midpoint of edge {which}; point_distance needs both")))
}

pub fn measure_params() -> Vec<(&'static str, Value)> {
    vec![
        ("entity", json!({"type":"string", "description":"An edge or face interface name"})),
        ("quantity", optional(one_of(&["midpoint"], "midpoint: the proven midpoint coordinates of a straight edge"))),
    ]
}

/// The two-entity `measure` tool's parameters.
pub fn pair_params() -> Vec<(&'static str, Value)> {
    vec![
        ("a", string("First edge or face interface name")),
        ("b", string("Second edge or face interface name")),
        ("quantity", optional(one_of(&QUANTITIES, "The proven scalar; minimum_distance by default"))),
        ("a_point", point_param("a")),
        ("b_point", point_param("b")),
    ]
}

pub fn track_params() -> Vec<(&'static str, Value)> {
    vec![
        (
            "entity",
            json!({"type":"string", "description":"An edge or face interface name; the first source of a pair"}),
        ),
        ("name", json!({"type":"string", "description":"A new value identifier"})),
        ("check", optional(string("Optional <, <=, > or >= followed by one limit"))),
        ("entity_b", optional(string("The second edge or face of a pair"))),
        ("quantity", optional(one_of(&QUANTITIES, "The pair's scalar; minimum_distance by default"))),
        ("a_point", point_param("entity")),
        ("b_point", point_param("entity_b")),
    ]
}

fn entity(doc: &Document, name: &str) -> Result<Selection, String> {
    find_entity(doc, name).ok_or_else(|| format!("no entity named {name}; select an edge or a face"))
}

/// Resolves interface labels once, into a persistent pair request.
fn pair(doc: &Document, input: &Value, a: &str, b: &str) -> Result<soliddraft_model::tracked::Source, String> {
    let quantity = match input.get("quantity") {
        Some(q) => serde_json::from_value(q.clone()).map_err(|_| format!("unknown quantity {q}"))?,
        None => PairQuantity::MinimumDistance,
    };
    let points = ["a_point", "b_point"].map(|k| input.get(k).is_some_and(|p| p == "midpoint"));
    match (quantity, points) {
        (PairQuantity::PointDistance, [true, true]) => {}
        (PairQuantity::PointDistance, _) => return Err("point_distance needs a_point and b_point: midpoint".into()),
        (_, [false, false]) => {}
        _ => return Err(format!("{} measures whole entities and takes no midpoint", quantity.name())),
    }
    let find = |key: &str| {
        let name = input[key].as_str().ok_or(format!("missing {key}"))?;
        find_entity(doc, name).ok_or_else(|| format!("no entity named `{name}`. {NAMES_HELP}"))
    };
    super::pair(doc, find(a)?, find(b)?, quantity)
}

pub fn call(doc: &mut Document, name: &str, input: &Value) -> Result<Value, String> {
    if name == "measure" {
        let source = pair(doc, input, "a", "b")?;
        return super::measure_pair(doc, &source);
    }
    let first = input["entity"].as_str().ok_or("missing entity")?;
    if name == "measure_entity" {
        let selection = entity(doc, first)?;
        return match input.get("quantity") {
            Some(q) if q == "midpoint" => super::midpoint(doc, selection),
            Some(q) => Err(format!("unknown quantity {q}")),
            None => super::measure(doc, selection),
        };
    }
    let source = if input.get("entity_b").is_some() {
        pair(doc, input, "entity", "entity_b")?
    } else if ["quantity", "a_point", "b_point"].iter().any(|k| input.get(*k).is_some()) {
        return Err("quantity and points need a second source, entity_b".into());
    } else {
        super::source(doc, entity(doc, first)?)?
    };
    doc.apply(Command::Track {
        name: input["name"].as_str().ok_or("missing name")?.into(),
        source,
        check: input.get("check").map(|v| v.as_str().map(str::to_owned).ok_or("check must be text")).transpose()?,
    })?;
    Ok(json!({"tracked": input["name"]}))
}
