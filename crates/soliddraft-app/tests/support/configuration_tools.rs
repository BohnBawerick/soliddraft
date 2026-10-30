use super::Session;
use serde_json::json;

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
}
