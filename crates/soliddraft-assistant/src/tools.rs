//! Tools: their definitions, the host that runs them, and the schema check every input
//! passes before it runs.
//!
//! Schemas follow the subset strict tool use accepts: every object closes with
//! `additionalProperties: false`, and there are no numeric or string bounds (the API
//! rejects them). Bounds such as "radius greater than zero" are the host's to check.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// A tool the model or an MCP client can call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// The tool's name.
    pub name: String,
    /// What it does and when to use it.
    pub description: String,
    /// JSON schema of its input, in the strict subset.
    pub input_schema: Value,
}

/// What a tool returned.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolOutput {
    /// The result text, usually JSON.
    pub content: String,
    /// Whether the call failed.
    pub is_error: bool,
}

impl ToolOutput {
    /// A successful result holding a JSON value.
    pub fn json(v: &Value) -> Self {
        Self { content: serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string()), is_error: false }
    }

    /// A failed call with a reason the model or client can act on.
    pub fn error(msg: impl Into<String>) -> Self {
        Self { content: msg.into(), is_error: true }
    }
}

/// Who is calling a tool. The host decides what each caller may do: the assistant may
/// only propose unless the user turned on auto-apply; an MCP client the user connected
/// runs commands directly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Caller {
    /// The in-app assistant, answering `prompt` with `model`.
    Assistant {
        /// The user's message this turn answers.
        prompt: String,
        /// The model id.
        model: String,
    },
    /// An MCP client.
    Mcp {
        /// The client's name from `initialize`, or `unknown`.
        client: String,
    },
}

/// Runs tools. The app implements this over its command layer.
pub trait ToolHost {
    /// The tools, in a fixed order. The list must not change during a conversation: it
    /// is part of the cached prompt prefix.
    fn tools(&self) -> Vec<ToolSpec>;

    /// Runs a tool. The input already passed [`validate`] against the tool's schema.
    fn call(&mut self, name: &str, input: Value, caller: &Caller) -> ToolOutput;
}

/// Checks the input against the named tool's schema, then runs it.
pub fn call_checked(host: &mut dyn ToolHost, name: &str, input: Value, caller: &Caller) -> ToolOutput {
    let Some(spec) = host.tools().into_iter().find(|t| t.name == name) else {
        return ToolOutput::error(format!("unknown tool `{name}`"));
    };
    if let Err(e) = validate(&spec.input_schema, &input) {
        return ToolOutput::error(format!("input does not match the `{name}` schema: {e}"));
    }
    host.call(name, input, caller)
}

/// Validates a value against a schema in the strict subset: `type` (one or several),
/// `properties`, `required`, `additionalProperties: false`, `items`, `enum`, `const`,
/// `anyOf` and local `$ref`s into `$defs`.
pub fn validate(schema: &Value, value: &Value) -> Result<(), String> {
    check(schema, schema, value, "$")
}

fn check(root: &Value, schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
        let name = r.strip_prefix("#/$defs/").ok_or_else(|| format!("{path}: unsupported $ref {r}"))?;
        let target =
            root.get("$defs").and_then(|d| d.get(name)).ok_or_else(|| format!("{path}: missing $defs/{name}"))?;
        return check(root, target, value, path);
    }
    if let Some(options) = schema.get("anyOf").and_then(Value::as_array) {
        let mut errors = Vec::new();
        for o in options {
            match check(root, o, value, path) {
                Ok(()) => return Ok(()),
                Err(e) => errors.push(e),
            }
        }
        return Err(format!("{path}: matches none of the alternatives ({})", errors.join("; ")));
    }
    if let Some(c) = schema.get("const")
        && c != value
    {
        return Err(format!("{path}: must be {c}"));
    }
    if let Some(options) = schema.get("enum").and_then(Value::as_array)
        && !options.contains(value)
    {
        return Err(format!("{path}: must be one of {}", Value::Array(options.clone())));
    }
    if let Some(t) = schema.get("type") {
        let types: Vec<&str> = match t {
            Value::String(s) => vec![s.as_str()],
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        if !types.iter().any(|t| type_matches(t, value)) {
            return Err(format!("{path}: expected {}, got {}", types.join(" or "), kind(value)));
        }
    }
    if let Value::Object(obj) = value {
        let props = schema.get("properties").and_then(Value::as_object);
        for req in schema.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
            if !obj.contains_key(req) {
                return Err(format!("{path}: missing `{req}`"));
            }
        }
        for (k, v) in obj {
            match props.and_then(|p| p.get(k)) {
                Some(s) => check(root, s, v, &format!("{path}.{k}"))?,
                None if schema.get("additionalProperties") == Some(&Value::Bool(false)) => {
                    return Err(format!("{path}: unexpected `{k}`"));
                }
                None => {}
            }
        }
    }
    if let (Value::Array(items), Some(item_schema)) = (value, schema.get("items")) {
        for (i, v) in items.iter().enumerate() {
            check(root, item_schema, v, &format!("{path}[{i}]"))?;
        }
        if schema.get("minItems").and_then(Value::as_u64).is_some_and(|n| (items.len() as u64) < n) {
            return Err(format!("{path}: must not be empty"));
        }
    }
    Ok(())
}

fn type_matches(t: &str, v: &Value) -> bool {
    match t {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "number" => v.is_number(),
        "integer" => v.as_i64().is_some() || v.as_u64().is_some() || v.as_f64().is_some_and(|f| f.fract() == 0.0),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        _ => false,
    }
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Checks that a schema stays inside what strict tool use accepts: objects close with
/// `additionalProperties: false`, and no unsupported keyword appears.
pub fn check_strict(schema: &Value) -> Result<(), String> {
    fn walk(s: &Value, path: &str) -> Result<(), String> {
        match s {
            Value::Object(map) => {
                for bad in [
                    "minimum",
                    "maximum",
                    "exclusiveMinimum",
                    "exclusiveMaximum",
                    "multipleOf",
                    "minLength",
                    "maxLength",
                    "pattern",
                    "maxItems",
                    "oneOf",
                    "not",
                ] {
                    if map.contains_key(bad) {
                        return Err(format!("{path}: `{bad}` is not allowed in strict schemas"));
                    }
                }
                if map.get("minItems").and_then(Value::as_u64).is_some_and(|n| n > 1) {
                    return Err(format!("{path}: minItems above 1 is not allowed"));
                }
                if map.get("type") == Some(&json!("object")) && map.get("additionalProperties") != Some(&json!(false)) {
                    return Err(format!("{path}: objects need additionalProperties: false"));
                }
                for (k, v) in map {
                    if k == "enum" || k == "const" || k == "required" || k == "description" {
                        continue;
                    }
                    walk(v, &format!("{path}/{k}"))?;
                }
                Ok(())
            }
            Value::Array(a) => a.iter().enumerate().try_for_each(|(i, v)| walk(v, &format!("{path}/{i}"))),
            _ => Ok(()),
        }
    }
    walk(schema, "#")
}

/// Small builders for strict schemas, so tool definitions read like their parameters.
pub mod schema {
    use super::*;

    /// A closed object. Every listed property is required; wrap one in [`optional`] to
    /// leave it out of `required`.
    pub fn object(props: &[(&str, Value)]) -> Value {
        let mut properties = Map::new();
        let mut required = Vec::new();
        for (name, s) in props {
            let mut s = s.clone();
            let optional = s.as_object_mut().and_then(|m| m.remove("x-optional")).is_some();
            if !optional {
                required.push(Value::String((*name).to_owned()));
            }
            properties.insert((*name).to_owned(), s);
        }
        json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
    }

    /// Marks a property as optional for [`object`].
    pub fn optional(mut s: Value) -> Value {
        s["x-optional"] = Value::Bool(true);
        s
    }

    /// A number with a description.
    pub fn number(description: &str) -> Value {
        json!({ "type": "number", "description": description })
    }

    /// An integer with a description.
    pub fn integer(description: &str) -> Value {
        json!({ "type": "integer", "description": description })
    }

    /// A string with a description.
    pub fn string(description: &str) -> Value {
        json!({ "type": "string", "description": description })
    }

    /// A boolean with a description.
    pub fn boolean(description: &str) -> Value {
        json!({ "type": "boolean", "description": description })
    }

    /// One of a fixed set of strings.
    pub fn one_of(values: &[&str], description: &str) -> Value {
        json!({ "type": "string", "enum": values, "description": description })
    }

    /// A fixed string, used to tag a command with its verb.
    pub fn constant(value: &str) -> Value {
        json!({ "type": "string", "const": value })
    }

    /// An array of items.
    pub fn array(items: Value, description: &str) -> Value {
        json!({ "type": "array", "items": items, "description": description })
    }

    /// An array of items that must not be empty.
    pub fn non_empty_array(items: Value, description: &str) -> Value {
        json!({ "type": "array", "items": items, "minItems": 1, "description": description })
    }

    /// Any of several schemas.
    pub fn any_of(options: Vec<Value>) -> Value {
        json!({ "anyOf": options })
    }

    /// Adds a description to a schema.
    pub fn described(mut s: Value, description: &str) -> Value {
        s["description"] = Value::String(description.to_owned());
        s
    }
}

#[cfg(test)]
mod tests {
    use super::schema::*;
    use super::*;

    fn sample() -> Value {
        object(&[
            ("radius", number("mm")),
            ("edges", non_empty_array(string("edge"), "edges")),
            ("note", optional(string("why"))),
            ("op", any_of(vec![constant("fillet"), constant("chamfer")])),
        ])
    }

    #[test]
    fn builders_make_strict_schemas() {
        let s = sample();
        check_strict(&s).unwrap();
        assert_eq!(s["required"], json!(["radius", "edges", "op"]));
        assert!(s["properties"]["note"].get("x-optional").is_none());
        assert!(check_strict(&json!({"type":"object","properties":{}})).is_err());
        assert!(check_strict(&json!({"type":"number","minimum":0})).is_err());
    }

    #[test]
    fn validator_accepts_and_rejects() {
        let s = sample();
        validate(&s, &json!({"radius": 8, "edges": ["e1"], "op": "fillet"})).unwrap();
        validate(&s, &json!({"radius": 8, "edges": ["e1"], "op": "chamfer", "note": "x"})).unwrap();
        for bad in [
            json!({"radius": "8", "edges": ["e1"], "op": "fillet"}),
            json!({"radius": 8, "edges": [], "op": "fillet"}),
            json!({"radius": 8, "edges": ["e1"]}),
            json!({"radius": 8, "edges": ["e1"], "op": "cut"}),
            json!({"radius": 8, "edges": ["e1"], "op": "fillet", "extra": 1}),
        ] {
            assert!(validate(&s, &bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn validator_follows_refs() {
        let s = json!({
            "$defs": {"p": {"type":"object","properties":{"x":{"type":"number"}},"required":["x"],"additionalProperties":false}},
            "type": "array", "items": {"$ref": "#/$defs/p"}
        });
        validate(&s, &json!([{"x": 1}])).unwrap();
        assert!(validate(&s, &json!([{"y": 1}])).is_err());
    }
}
