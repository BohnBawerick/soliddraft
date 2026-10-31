use super::Session;
use serde_json::{Value, json};

pub(super) fn exercise(s: &mut Session) {
    assert!(!s.call("add_configuration", json!({"name":"ToolVariant"})).is_null());
    assert!(
        !s.call(
            "set_configuration_value",
            json!({"configuration":"ToolVariant","name":"plate_depth","expression":"14"})
        )
        .is_null()
    );
    assert!(!s.call("activate_configuration", json!({"name":"ToolVariant"})).is_null());
    let summary = s.call("model_summary", json!({}));
    assert_eq!(summary["configurations"]["active"], "ToolVariant");
    assert_eq!(summary["values"][0]["value"], 14.0);
    assert_eq!(summary["features"][1]["depth"], 14.0);
    assert!(!s.call("activate_configuration", json!({"name":"Default"})).is_null());
    assert_eq!(s.call("model_summary", json!({}))["values"][0]["value"], 12.0);
    table(s);
}

/// The whole table's tools: nothing moves until Apply, which is one undo step.
fn table(s: &mut Session) {
    let live = s.call("configuration_grid", json!({}));
    let column = live["columns"].as_array().unwrap().iter().position(|c| c == "value:plate_depth [mm]").unwrap();
    let mut rows: Vec<Value> = live["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            let cells = row["cells"].as_array().unwrap();
            Value::Array(std::iter::once(row["name"].clone()).chain(cells.iter().cloned()).collect())
        })
        .collect();
    let row = live["rows"].as_array().unwrap().iter().position(|r| r["name"] == "ToolVariant").unwrap();
    rows[row][column] = json!("16");
    let candidate = json!({ "columns": live["columns"], "rows": rows });

    let cancelled = s.call("preview_configuration_grid", candidate.clone());
    assert!(!s.call("cancel_configuration_grid", json!({ "preview_id": cancelled["preview_id"] })).is_null());
    let exported = s.path("configurations.csv");
    assert!(!s.call("export_configuration_grid", json!({ "path": exported, "format": "csv" })).is_null());
    let text = std::fs::read_to_string(s.dir.join("configurations.csv")).unwrap();
    assert!(text.contains("ToolVariant,14"), "{text}");

    let preview = s.call("preview_configuration_grid", candidate);
    assert!(!s.call("apply_configuration_grid", json!({ "preview_id": preview["preview_id"] })).is_null());
    let applied = s.call("configuration_grid", json!({}));
    assert_eq!(applied["rows"][row]["cells"][column - 1], "16");
    assert!(!s.call("undo", json!({})).is_null());
    assert_eq!(s.call("configuration_grid", json!({}))["rows"], live["rows"]);
}
