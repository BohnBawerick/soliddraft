use serde_json::Value;
use soliddraft_app::{command_tools::line_schema, commands};
use std::collections::BTreeSet;

/// Compare the frozen proposal contract after checking its three new alternatives.
pub(super) fn before_configurations(id: &str, current: &Value, plain: fn(&Value) -> Value) -> Value {
    let mut current = current.clone();
    if id != "propose" {
        return current;
    }
    let expected = BTreeSet::from(["activate_configuration", "add_configuration", "set_configuration_value"]);
    let mut found = BTreeSet::new();
    let lines = current.pointer_mut("/tool/input/properties/commands/items/anyOf").unwrap().as_array_mut().unwrap();
    lines.retain(|line| {
        let op = line["properties"]["op"]["const"].as_str().unwrap();
        if !expected.contains(op) {
            return true;
        }
        assert!(found.insert(op.to_owned()), "duplicate proposal alternative {op}");
        let verb = commands::get(op).unwrap();
        assert!(matches!(verb.agents, commands::Agents::Verb));
        assert_eq!(line, &plain(&line_schema(verb)), "{op} proposal differs from its verb");
        false
    });
    assert_eq!(found, expected.into_iter().map(str::to_owned).collect());
    current
}
