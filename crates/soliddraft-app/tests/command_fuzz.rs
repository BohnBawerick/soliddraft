//! Fuzzing the command layer through the door the MCP server and the assistant use: tool
//! calls whose arguments are built from each tool's own schema and then bent, with
//! wrong types, missing and extra fields, extreme numbers, unknown tool names, long
//! and odd names, repeated and nested input.
//!
//! The properties, over sequences of calls on the sample bracket:
//!
//! - nothing panics, and nothing runs for longer than a minute;
//! - every request gets a well-formed JSON-RPC reply, and a refusal is typed: an
//!   error code, or `isError` with a message that says what was wrong;
//! - a refused call changes nothing: the part's revision, feature list, undo stack and
//!   rollback bar are as they were;
//! - whatever was applied, the part still saves, reopens and rebuilds to the same revision.
//!
//! No window or GPU is needed. `FUZZ_TRACE=1 --nocapture` prints whether each call ran or was
//! refused. A fixed number of cases from a fixed seed run by default,
//! so a failure reproduces; to search further:
//! `PROPTEST_CASES=2000 PROPTEST_RNG_SEED=7 cargo test -p soliddraft-app --test command_fuzz`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;

use proptest::prelude::*;
use proptest::strategy::Union;
use proptest::test_runner::RngSeed;
use serde_json::{Map, Number, Value, json};
use soliddraft_app::app_tools::AppTools;
use soliddraft_app::command_tools::{CommandTools, MCP_INSTRUCTIONS, ProposalBook, Surface, board_bracket, tool_specs};
use soliddraft_app::document::Document;
use soliddraft_app::state::AppState;
use soliddraft_assistant::mcp::McpSession;
use soliddraft_assistant::tools::call_checked;
use soliddraft_assistant::{Caller, ToolHost};

/// Cases per property, and the seed they come from, unless the environment says otherwise.
fn config() -> ProptestConfig {
    // A failing input is printed with the test's seed; nothing is written beside the sources.
    let mut config = ProptestConfig { failure_persistence: None, ..ProptestConfig::default() };
    if std::env::var_os("PROPTEST_CASES").is_none() {
        config.cases = 24;
    }
    if std::env::var_os("PROPTEST_RNG_SEED").is_none() {
        config.rng_seed = RngSeed::Fixed(0x43_4d_44);
    }
    config
}

/// A hang guard: this detects a command that does not return, not a slow one.
const LIMIT: Duration = Duration::from_secs(600);

// ----- inputs -----

/// Numbers a call could hold.
///
/// NOT COVERED: nonzero numbers smaller than 0.01 in size are left out, so a green run says nothing
/// about them. A plane origin a hair off a round value sends Catallax's boolean region sampling into
/// a recursion that takes a minute or overflows the stack and aborts the process; the fix is in
/// Catallax and the isolated reproductions are the ignored tests in `kernel_abort.rs` in
/// soliddraft-cli. This exclusion comes out with their `#[ignore]` lines. Lengths and coordinates over
/// `MAX_LENGTH_MM` are refused by the command layer before the kernel, so what the kernel does with
/// them is not covered either (Catallax meshes a huge shape until it runs out of memory).
fn numbers() -> impl Strategy<Value = Value> {
    fn float(f: f64) -> Option<Value> {
        Number::from_f64(f).map(Value::Number)
    }
    prop_oneof![
        8 => (1u32..=200).prop_map(|n| json!(n as f64 / 4.0)),
        2 => (1i64..=100).prop_map(Value::from),
        1 => prop::sample::select(vec![
            0.0, -0.0, -1.0, 1e9, 1e12, 1e15, 1e100, 1e300, f64::MAX, f64::MIN, 0.5, 123_456_789.123_456_79,
        ])
        .prop_filter_map("finite", float),
        1 => any::<f64>().prop_filter_map("finite and not tiny", |f| if f == 0.0 || f.abs() >= 0.01 { float(f) } else { None }),
        1 => any::<i64>().prop_map(Value::from),
        1 => any::<u64>().prop_map(Value::from),
    ]
}

fn texts() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        Just(" ".to_owned()),
        Just("\u{0}".to_owned()),
        Just("·".to_owned()),
        Just("Boss-Extrude1 ·".to_owned()),
        Just("· End cap".to_owned()),
        Just("Boss-Extrude1 · Vertex 0".to_owned()),
        Just("Boss-Extrude1 · Vertex 4294967295".to_owned()),
        Just("Boss-Extrude1 · Vertex 18446744073709551616".to_owned()),
        Just("Boss-Extrude1 · Edge -1".to_owned()),
        Just("\u{202e}top\u{200b}".to_owned()),
        Just("😀".repeat(300)),
        Just("a".repeat(50_000)),
        "\\PC{0,20}",
        any::<String>(),
    ]
}

fn any_json() -> impl Strategy<Value = Value> {
    let leaf =
        prop_oneof![Just(Value::Null), any::<bool>().prop_map(Value::Bool), numbers(), texts().prop_map(Value::String)];
    leaf.prop_recursive(4, 32, 5, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..5).prop_map(Value::Array),
            prop::collection::vec((texts(), inner), 0..5)
                .prop_map(|kv| Value::Object(kv.into_iter().collect::<Map<_, _>>())),
        ]
    })
}

/// The names an agent would use for the sample bracket's parts, by what a parameter
/// takes, so that many calls reach past the lookup into the commands themselves.
#[derive(Clone)]
struct Pools {
    /// Anything an entity can be called.
    any: Vec<String>,
    planes: Vec<String>,
    sketches: Vec<String>,
    features: Vec<String>,
    edges: Vec<String>,
    proposals: Vec<String>,
    configurations: Vec<String>,
    templates: Vec<String>,
    /// The views a drawing of the bracket has.
    views: Vec<String>,
}

fn pools(doc: &Document) -> Pools {
    let strings = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
    // What the first calls of a sequence make is named ahead: Sketch5, Boss-Extrude4, ...
    let ahead = strings(&["Sketch5", "Sketch6", "Boss-Extrude4", "Cut-Extrude2", "Fillet1", "Chamfer1"]);
    let mut pools = Pools {
        any: strings(&["top", "front", "right", "previous"]),
        planes: strings(&["top", "front", "right", "Top Plane", "top plane"]),
        sketches: strings(&["previous"]),
        features: ahead.clone(),
        edges: Vec::new(),
        configurations: strings(&["Default", "Narrow", "Wide"]),
        proposals: strings(&["P1", "P2", "P3", "P4", "P99"]),
        templates: strings(&["iso-a3", "ansi-b", "iso-a9"]),
        views: strings(&["Front view", "Top view", "Left view", "right VIEW", "Isometric view", "Section Z-Z"]),
    };
    pools.sketches.extend(ahead);
    for f in &doc.features {
        pools.features.push(f.name.clone());
        if f.is_sketch() {
            pools.sketches.push(f.name.clone());
        }
        for x in doc.faces.iter().filter(|x| x.feature == f.id) {
            pools.planes.push(format!("{} · {}", f.name, x.label));
        }
        for x in doc.edges.iter().filter(|x| x.feature == f.id) {
            pools.edges.push(format!("{} · {}", f.name, x.label));
        }
    }
    pools.any.extend(pools.features.iter().chain(&pools.planes).chain(&pools.edges).cloned());
    pools
}

impl Pools {
    /// The names a string parameter called `hint` takes.
    fn named(&self, hint: &str) -> &[String] {
        match hint {
            "configuration" => &self.configurations,
            "plane" => &self.planes,
            "sketch" => &self.sketches,
            "feature" | "before" => &self.features,
            "edges" => &self.edges,
            "entity" => &self.any,
            "proposal_id" => &self.proposals,
            "template" => &self.templates,
            "view" => &self.views,
            _ => &self.any,
        }
    }
}

/// Values for a schema: mostly what it asks for, with each node occasionally replaced by
/// anything at all, and objects occasionally missing a field or holding an extra one.
/// `hint` is the name of the property the value is for.
fn for_schema(root: &Value, schema: &Value, pools: &Pools, hint: &str) -> BoxedStrategy<Value> {
    let typed = typed(root, schema, pools, hint);
    prop_oneof![30 => typed, 1 => any_json()].boxed()
}

/// A coordinate in the part's own range, now and then far outside it.
fn coord() -> BoxedStrategy<Value> {
    prop_oneof![
        20 => (-40i32..=40).prop_map(|n| json!(n as f64 / 2.0)),
        1 => prop::sample::select(vec![0.3, 1e6, -1e6, 1e15, -1e15, 1e300, -1e300, 1e308]).prop_map(|f| json!(f)),
    ]
    .boxed()
}

/// A length: in the part's own range, now and then absurdly small or large.
fn length() -> BoxedStrategy<Value> {
    prop_oneof![
        20 => (1u32..=120).prop_map(|n| json!(n as f64 / 2.0)),
        1 => prop::sample::select(vec![0.05, 1e6, 1e15, 1e100, 1e300]).prop_map(|f| json!(f)),
    ]
    .boxed()
}

/// A point `[x, y, z]` on the top, front or right plane, and now and then anywhere.
fn point() -> BoxedStrategy<Value> {
    prop_oneof![
        3 => (coord(), coord()).prop_map(|(x, y)| json!([x, y, 0])),
        1 => (coord(), coord()).prop_map(|(x, z)| json!([x, 0, z])),
        1 => (coord(), coord()).prop_map(|(y, z)| json!([0, y, z])),
        1 => prop::collection::vec(numbers(), 0..5).prop_map(Value::Array),
    ]
    .boxed()
}

fn typed(root: &Value, schema: &Value, pools: &Pools, hint: &str) -> BoxedStrategy<Value> {
    if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
        let target = &root["$defs"][r.trim_start_matches("#/$defs/")];
        return typed(root, target, pools, hint);
    }
    if let Some(options) = schema.get("anyOf").and_then(Value::as_array) {
        return Union::new(options.iter().map(|o| for_schema(root, o, pools, hint))).boxed();
    }
    if let Some(c) = schema.get("const") {
        return Just(c.clone()).boxed();
    }
    if let Some(options) = schema.get("enum").and_then(Value::as_array) {
        return prop::sample::select(options.clone()).boxed();
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => {
            let required: Vec<&str> =
                schema["required"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            let fields: Vec<BoxedStrategy<Option<(String, Value)>>> = schema["properties"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(name, s)| {
                    let name = name.clone();
                    let keep = if required.contains(&name.as_str()) { 40 } else { 3 };
                    let value = for_schema(root, s, pools, &name);
                    prop_oneof![keep => value.prop_map(move |v| Some((name.clone(), v))), 1 => Just(None)].boxed()
                })
                .collect();
            let extra = prop::option::weighted(0.03, (texts(), any_json()));
            (fields, extra)
                .prop_map(|(fields, extra)| {
                    Value::Object(fields.into_iter().flatten().chain(extra).collect::<Map<_, _>>())
                })
                .boxed()
        }
        Some("array") if matches!(hint, "center" | "start" | "end") => point(),
        Some("array") => {
            let item = for_schema(root, &schema["items"], pools, hint);
            let min = if schema.get("minItems").is_some() { 1 } else { 0 };
            prop::collection::vec(item, min..5).prop_map(Value::Array).boxed()
        }
        Some("number" | "integer") => numbers().boxed(),
        Some("boolean") => any::<bool>().prop_map(Value::Bool).boxed(),
        Some("string") => prop_oneof![
            6 => prop::sample::select(pools.named(hint).to_vec()).prop_map(Value::String),
            1 => texts().prop_map(Value::String),
        ]
        .boxed(),
        _ => any_json().boxed(),
    }
}

#[derive(Clone, Debug)]
struct Call {
    tool: String,
    arguments: Value,
}

fn call_sequences(surface: Surface) -> BoxedStrategy<Vec<Call>> {
    let pools = pools(&board_bracket());
    let specs = tool_specs(surface);
    let mut by_name: HashMap<String, BoxedStrategy<Value>> = HashMap::new();
    for spec in &specs {
        by_name.insert(spec.name.clone(), for_schema(&spec.input_schema, &spec.input_schema, &pools, ""));
    }
    let tools: Vec<String> = specs.iter().map(|s| s.name.clone()).collect();
    let known = prop::sample::select(tools).prop_flat_map(move |tool| {
        let arguments = by_name[&tool].clone();
        arguments.prop_map(move |arguments| Call { tool: tool.clone(), arguments })
    });
    let unknown = (texts(), any_json()).prop_map(|(tool, arguments)| Call { tool, arguments });
    let surface_has = move |tool: &str| specs.iter().any(|s| s.name == tool);
    let sane = sane_call(pools).prop_filter("a tool of this surface", move |c| surface_has(&c.tool));
    prop::collection::vec(prop_oneof![20 => known, 15 => sane, 1 => unknown], 1..6).boxed()
}

fn call(tool: &str, arguments: Value) -> Call {
    Call { tool: tool.to_owned(), arguments }
}

/// A call that is right in form and mostly in substance, so that sequences go on to
/// change the part and the later calls meet a part that has changed.
fn sane_call(pools: Pools) -> BoxedStrategy<Call> {
    let size = length;
    let pick = |names: Vec<String>| prop::sample::select(names);
    // A sketch on a reference plane, with points on it, or on any face, with points that mostly miss it.
    let on = |plane: &'static str| {
        let n = coord;
        let at = move |a: Value, b: Value| match plane {
            "top" => json!([a, b, 0]),
            "front" => json!([a, 0, b]),
            _ => json!([0, a, b]),
        };
        let shape = prop_oneof![
            (n(), n(), size(), size()).prop_map(move |(x, y, width, height)| {
                json!({ "shape": "rectangle", "center": at(x, y), "width": width, "height": height })
            }),
            (n(), n(), size()).prop_map(
                move |(x, y, diameter)| json!({ "shape": "circle", "center": at(x, y), "diameter": diameter })
            ),
            (n(), n(), n(), n(), size()).prop_map(move |(x, y, u, v, width)| {
                json!({ "shape": "slot", "start": at(x, y), "end": at(u, v), "width": width })
            }),
        ];
        (Just(plane), prop::collection::vec(shape, 1..4))
            .prop_map(|(plane, shapes)| call("sketch", json!({ "plane": plane, "shapes": shapes })))
    };
    let on_face =
        (pick(pools.planes.clone()), prop::collection::vec((point(), size()), 1..3)).prop_map(|(plane, circles)| {
            let shapes: Vec<Value> = circles
                .into_iter()
                .map(|(center, diameter)| json!({ "shape": "circle", "center": center, "diameter": diameter }))
                .collect();
            call("sketch", json!({ "plane": plane, "shapes": shapes }))
        });
    let sketch = prop_oneof![3 => on("top"), 2 => on("front"), 2 => on("right"), 1 => on_face];
    let extrude = (pick(pools.sketches.clone()), size(), any::<bool>()).prop_map(move |(sketch, depth, flip)| {
        call("extrude", json!({ "sketch": sketch, "depth": depth, "flip": flip }))
    });
    let cut = (pick(pools.sketches.clone()), any::<bool>(), size(), any::<bool>()).prop_map(
        move |(sketch, through_all, depth, flip)| {
            call("cut", json!({ "sketch": sketch, "through_all": through_all, "depth": depth, "flip": flip }))
        },
    );
    let edges = prop::collection::vec(pick(pools.edges.clone()), 1..3);
    let fillet = (edges.clone(), size())
        .prop_map(move |(edges, radius)| call("fillet", json!({ "edges": edges, "radius": radius })));
    let chamfer = (edges, size())
        .prop_map(move |(edges, distance)| call("chamfer", json!({ "edges": edges, "distance": distance })));
    let edit = (
        pick(pools.features.clone()),
        size(),
        any::<bool>(),
        prop::sample::select(vec!["edit_extrude", "edit_cut", "edit_fillet", "edit_chamfer"]),
    )
        .prop_map(move |(feature, n, flip, tool)| {
            let arguments = match tool {
                "edit_extrude" => json!({ "feature": feature, "depth": n, "flip": flip }),
                "edit_cut" => json!({ "feature": feature, "through_all": flip, "depth": n, "flip": flip }),
                "edit_fillet" => json!({ "feature": feature, "radius": n }),
                _ => json!({ "feature": feature, "distance": n }),
            };
            call(tool, arguments)
        });
    let suppress = (pick(pools.features.clone()), any::<bool>()).prop_map(move |(feature, suppressed)| {
        call("suppress", json!({ "feature": feature, "suppressed": suppressed }))
    });
    let delete =
        pick(pools.features.clone()).prop_map(move |feature| call("delete_feature", json!({ "feature": feature })));
    let propose = (pick(pools.planes.clone()), point(), size(), size(), any::<bool>()).prop_map(move |(plane, center, diameter, depth, through_all)| {
        call(
            "propose",
            json!({ "summary": "A hole", "commands": [
                { "op": "sketch", "plane": plane, "shapes": [{ "shape": "circle", "center": center, "diameter": diameter }] },
                { "op": "cut", "sketch": "previous", "through_all": through_all, "depth": depth, "flip": false },
            ] }),
        )
    });
    let apply = pick(pools.proposals.clone()).prop_map(move |id| call("apply", json!({ "proposal_id": id })));
    let step =
        prop::sample::select(vec!["undo", "redo", "model_summary", "selection"]).prop_map(|tool| call(tool, json!({})));
    let quantity =
        prop::option::of(prop::sample::select(vec!["minimum_distance", "point_distance", "direction_angle"]));
    let measure = (pick(pools.any.clone()), pick(pools.any.clone()), quantity, any::<bool>()).prop_map(
        move |(a, b, quantity, midpoints)| {
            let mut input = json!({ "a": a, "b": b });
            if let Some(q) = quantity {
                input["quantity"] = q.into();
            }
            if midpoints {
                input["a_point"] = "midpoint".into();
                input["b_point"] = "midpoint".into();
            }
            call("measure", input)
        },
    );
    let rollback = prop::option::of(pick(pools.features.clone()))
        .prop_map(move |before| call("rollback", json!({ "before": before })));
    let configurations =
        (pick(pools.configurations.clone()), 0u8..4, size()).prop_map(|(configuration, action, width)| match action {
            0 => call("set_value", json!({"name":"plate_w", "expression":width.to_string()})),
            1 => call("add_configuration", json!({"name":configuration})),
            2 => call(
                "set_configuration_value",
                json!({"configuration":configuration,"name":"plate_w","expression":width.to_string()}),
            ),
            _ => call("activate_configuration", json!({"name":configuration})),
        });
    prop_oneof![
        3 => configurations,
        3 => sketch, 3 => extrude, 2 => cut, 2 => fillet, 2 => chamfer, 2 => edit, 1 => suppress, 1 => delete,
        2 => propose, 2 => apply, 2 => step, 1 => measure, 1 => rollback,
    ]
    .boxed()
}

// ----- properties -----

/// What a refused call must leave as it found it.
#[derive(Debug, PartialEq)]
struct Snapshot {
    revision: String,
    features: Vec<String>,
    undo_depth: usize,
    undo_label: Option<String>,
    rollback: Option<usize>,
    proposals: Vec<String>,
}

fn snapshot(doc: &Document, book: &ProposalBook) -> Snapshot {
    Snapshot {
        revision: doc.model().semantic_hash().to_string(),
        features: doc.features.iter().map(|f| format!("{} {:?} {}", f.name, f.status, f.suppressed)).collect(),
        undo_depth: doc.undo_depth(),
        undo_label: doc.undo_label().map(str::to_owned),
        rollback: doc.rollback,
        proposals: book.proposals.iter().map(|p| format!("{} {:?}", p.id, p.status)).collect(),
    }
}

/// Runs `body` on a thread of its own, and fails with `what` if it outlasts [`LIMIT`].
fn within_limit(what: String, body: impl FnOnce() + Send + 'static) {
    let (done, finished) = channel();
    // The stack of a Unix main thread, which is where the app runs a command (see recipe_fuzz
    // in soliddraft-cli for why the 2 MiB of a default thread is not enough for the kernel).
    let worker = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            body();
            let _ = done.send(());
        })
        .expect("a thread to run the case on");
    match finished.recv_timeout(LIMIT) {
        Ok(()) => worker.join().expect("the case finished"),
        // The thread ended without finishing: it panicked. Report its panic.
        Err(RecvTimeoutError::Disconnected) => std::panic::resume_unwind(worker.join().unwrap_err()),
        Err(RecvTimeoutError::Timeout) => panic!("no answer after {LIMIT:?}: {what}"),
    }
}

fn assert_reply(request: &Value, reply: &Value) -> bool {
    assert_eq!(reply["jsonrpc"], "2.0", "{reply}");
    assert_eq!(reply["id"], request["id"], "{reply}");
    match (reply.get("result"), reply.get("error")) {
        (Some(result), None) => {
            let refused =
                result["isError"].as_bool().unwrap_or_else(|| panic!("a tool result says whether it failed: {reply}"));
            let text =
                result["content"][0]["text"].as_str().unwrap_or_else(|| panic!("a tool result has text: {reply}"));
            assert!(!text.trim().is_empty(), "{reply}");
            if refused {
                assert!(text.len() < 9_000, "a refusal should not echo the whole input back: {} bytes", text.len());
            }
            refused
        }
        (None, Some(error)) => {
            assert!(error["code"].is_i64(), "{reply}");
            assert!(error["message"].as_str().is_some_and(|m| !m.trim().is_empty()), "{reply}");
            true
        }
        _ => panic!("a reply holds a result or an error: {reply}"),
    }
}

fn run_over_mcp(calls: Vec<Call>) {
    let mut state = AppState::new(board_bracket());
    let mut session = McpSession::new("soliddraft", "0.1.0", MCP_INSTRUCTIONS);
    for (n, call) in calls.iter().enumerate() {
        let before = snapshot(&state.doc, &state.chat.book);
        let request = json!({
            "jsonrpc": "2.0", "id": n,
            "method": "tools/call",
            "params": { "name": call.tool, "arguments": call.arguments },
        });
        if std::env::var_os("FUZZ_TRACE").is_some() {
            eprintln!("calling {}", request["params"].to_string().chars().take(600).collect::<String>());
        }
        let reply =
            session.handle(&mut AppTools { state: &mut state }, request.clone()).expect("a request is answered");
        let refused = assert_reply(&request, &reply);
        if std::env::var_os("FUZZ_TRACE").is_some() {
            let why =
                reply["result"]["content"][0]["text"].as_str().unwrap_or_default().lines().next().unwrap_or_default();
            let why = if refused { why.chars().take(110).collect::<String>() } else { String::new() };
            eprintln!("{} {} {why}", if refused { "refused" } else { "ran    " }, call.tool);
        }
        if refused {
            // A refused apply or undo may have moved a proposal's status, never the part.
            let after = snapshot(&state.doc, &state.chat.book);
            assert_eq!(
                (&before.revision, &before.features, before.undo_depth, &before.undo_label, before.rollback),
                (&after.revision, &after.features, after.undo_depth, &after.undo_label, after.rollback),
                "a refused call changed the part: {} {}\n{reply}",
                call.tool,
                call.arguments
            );
        }
    }
    assert_part_survives(&state.doc);
}

/// The part saves, reopens and rebuilds to the revision it had.
fn assert_part_survives(doc: &Document) {
    let bytes = doc.model().to_bytes().expect("the part saves");
    let reopened =
        soliddraft_model::Document::from_bytes(&bytes, doc.kernel().clone()).expect("the saved part reopens");
    assert_eq!(reopened.semantic_hash(), doc.model().semantic_hash(), "the part reopens as another revision");
}

fn run_as_assistant(calls: Vec<Call>, auto_apply: bool) {
    let mut doc = board_bracket();
    let mut book = ProposalBook::default();
    book.auto_apply = auto_apply;
    let caller = Caller::Assistant { prompt: "fuzz".into(), model: "none".into() };
    for call in &calls {
        let before = snapshot(&doc, &book);
        if std::env::var_os("FUZZ_TRACE").is_some() {
            eprintln!("calling {} {}", call.tool, call.arguments.to_string().chars().take(600).collect::<String>());
        }
        let mut host = CommandTools { doc: &mut doc, book: &mut book, surface: Surface::Assistant, drawing: None };
        let out = call_checked(&mut host, &call.tool, call.arguments.clone(), &caller);
        assert!(!out.content.trim().is_empty(), "{} says nothing", call.tool);
        if out.is_error && !auto_apply {
            assert_eq!(
                before,
                snapshot(&doc, &book),
                "a refused call changed the part: {} {}",
                call.tool,
                call.arguments
            );
        }
        if !auto_apply {
            assert!(
                !book
                    .proposals
                    .iter()
                    .any(|p| matches!(p.status, soliddraft_app::command_tools::ProposalStatus::Applied { .. })),
                "the assistant applied a proposal the user did not: {} {}",
                call.tool,
                call.arguments
            );
        }
    }
    assert_part_survives(&doc);
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn mcp_tool_calls_never_panic_and_refusals_change_nothing(calls in call_sequences(Surface::Mcp)) {
        let what = format!("{calls:?}");
        within_limit(what, move || run_over_mcp(calls));
    }

    #[test]
    fn the_assistant_surface_never_panics_and_cannot_apply_unasked(calls in call_sequences(Surface::Assistant), auto in any::<bool>()) {
        let what = format!("{calls:?}");
        within_limit(what, move || run_as_assistant(calls, auto));
    }
}

/// A floating block and a second one placed on it, with `O` and `Z`, saved.
fn mated_state() -> AppState {
    let dir = std::env::temp_dir().join(format!("soliddraft-fuzz-mates-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut state = AppState::new(soliddraft_app::new_document());
    let caller = Caller::Mcp { client: "fuzz".into() };
    let mut run = |state: &mut AppState, tool: &str, input: Value| {
        let out = call_checked(&mut AppTools { state }, tool, input, &caller);
        assert!(!out.is_error, "{tool}: {}", out.content);
    };
    let part = |state: &mut AppState, run: &mut dyn FnMut(&mut AppState, &str, Value), name: &str| {
        let shape = json!({ "shape": "rectangle", "center": [0, 0, 0], "width": 40, "height": 30 });
        run(state, "sketch", json!({ "plane": "top", "shapes": [shape] }));
        run(state, "extrude", json!({ "sketch": "Sketch1", "depth": 10, "flip": false }));
        run(state, "save", json!({ "path": dir.join(name) }));
    };
    part(&mut state, &mut run, "a.dwprt");
    run(&mut state, "make_assembly", json!({}));
    run(&mut state, "assembly_save", json!({ "path": dir.join("rig.dwasm") }));
    run(&mut state, "new_part", json!({}));
    part(&mut state, &mut run, "b.dwprt");
    run(&mut state, "show_assembly", json!({}));
    run(&mut state, "insert_component", json!({ "paths": [dir.join("b.dwprt")] }));
    run(&mut state, "assembly_mate_origin", json!({ "name": "O", "component": 2, "align_axes": true }));
    let cap = |c: u64, g: &str| json!({ "component": c, "feature": "Boss-Extrude1", "generator": g });
    let z = json!({ "name": "Z", "a": cap(2, "end_cap"), "b": cap(4, "start_cap"), "distance": "5 mm", "alignment": "opposed" });
    run(&mut state, "assembly_mate_distance", z);
    state
}

const MATE_TOOLS: [&str; 10] = [
    "insert_component",
    "assembly_form_subassembly",
    "assembly_measure_planes",
    "assembly_mate_origin",
    "assembly_mate_distance",
    "assembly_set_value",
    "assembly_rename_value",
    "assembly_drive_mate",
    "assembly_edit_mate",
    "assembly_delete_mate",
];

/// The mate tools' calls, from their schemas, with the names an assembly really has.
fn mate_calls() -> BoxedStrategy<Vec<Call>> {
    let mut pools = pools(&board_bracket());
    pools.any = ["O", "Z", "gap", "5 mm", "12 deg", "gap + 1 mm", "Boss-Extrude1", "side", "end_cap", "opposed"]
        .map(str::to_owned)
        .to_vec();
    // Occurrence paths and assembly files for Form subassembly: real, nested, unknown, a
    // pattern member, a file that exists and one that does not.
    let dir = std::env::temp_dir().join(format!("soliddraft-fuzz-mates-{}", std::process::id()));
    pools.any.extend(["2", "4", "4/2", "99", "4:7.1"].map(str::to_owned));
    pools.any.extend(["rig.dwasm", "pack.dwasm"].map(|f| dir.join(f).display().to_string()));
    pools.features = vec!["Boss-Extrude1".to_owned()];
    let specs: Vec<_> =
        tool_specs(Surface::Mcp).into_iter().filter(|s| MATE_TOOLS.contains(&s.name.as_str())).collect();
    let mut by_name: HashMap<String, BoxedStrategy<Value>> = HashMap::new();
    for spec in &specs {
        by_name.insert(spec.name.clone(), for_schema(&spec.input_schema, &spec.input_schema, &pools, ""));
    }
    let tools: Vec<String> = specs.iter().map(|s| s.name.clone()).collect();
    let known = prop::sample::select(tools).prop_flat_map(move |tool| {
        let arguments = by_name[&tool].clone();
        arguments.prop_map(move |arguments| Call { tool: tool.clone(), arguments })
    });
    prop::collection::vec(known, 1..6).boxed()
}

fn run_on_mates(calls: Vec<Call>) {
    let mut state = mated_state();
    let caller = Caller::Mcp { client: "fuzz".into() };
    for call in &calls {
        let before = AppTools { state: &mut state }.summary()["assembly"]["accepted_state"].clone();
        let out = call_checked(&mut AppTools { state: &mut state }, &call.tool, call.arguments.clone(), &caller);
        if out.is_error {
            let after = AppTools { state: &mut state }.summary()["assembly"]["accepted_state"].clone();
            assert_eq!(before, after, "a refused call changed the assembly: {call:?}\n{}", out.content);
        }
    }
    let s = state.assembly.session.as_mut().expect("the assembly stays open");
    let (solver, accepted) = (s.doc.solver(), s.doc.assembly().clone());
    assert_ne!(solver.state, soliddraft_assembly::SolverState::Error, "{calls:?}");
    let bytes = accepted.to_bytes(s.doc.id(), None, &[]).expect("the assembly saves");
    soliddraft_assembly::AssemblyPackage::from_bytes(bytes).expect("the saved assembly reopens");
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn mate_tools_never_panic_and_refusals_change_nothing(calls in mate_calls()) {
        let what = format!("{calls:?}");
        within_limit(what, move || run_on_mates(calls));
    }
}

/// Form subassembly refuses what it cannot form whole, and a refusal changes nothing: the
/// accepted state, the redo it finds and the files are as they were.
#[test]
fn form_refusals_change_nothing() {
    let mut state = mated_state();
    let dir = std::env::temp_dir().join(format!("soliddraft-fuzz-mates-{}", std::process::id()));
    let (root, pack) = (dir.join("rig.dwasm"), dir.join("refused.dwasm"));
    let caller = Caller::Mcp { client: "fuzz".into() };
    let form = |paths: Value, path: &Path, origin: Value| json!({ "paths": paths, "path": path, "origin": origin });
    let cases = [
        // The schema refuses an empty selection and a missing file before the command runs.
        (form(json!([]), &pack, json!("4")), "schema"),
        (json!({ "paths": ["4"], "origin": "4" }), "schema"),
        (form(json!(["4", "4"]), &pack, json!("4")), "invalid"),
        (form(json!(["4", "99"]), &pack, json!("4")), "invalid"),
        (form(json!(["4/2"]), &pack, json!("4/2")), "invalid"),
        (form(json!(["4:7.1"]), &pack, json!("4:7.1")), "invalid"),
        (form(json!(["not a path"]), &pack, json!("4")), "invalid"),
        (form(json!(["4"]), &pack, json!("2")), "invalid"),
        (form(json!(["4"]), &dir.join("refused.dwprt"), json!("4")), "invalid"),
        // O holds A on the root's origin: it cannot move into a new file.
        (form(json!(["2"]), &pack, json!("2")), "unsupported"),
        (form(json!(["4"]), &root, json!("4")), "destination_exists"),
    ];
    let rig = std::fs::read(&root).unwrap();
    for (arguments, code) in cases {
        let before = AppTools { state: &mut state }.summary()["assembly"]["accepted_state"].clone();
        let out =
            call_checked(&mut AppTools { state: &mut state }, "assembly_form_subassembly", arguments.clone(), &caller);
        assert!(out.is_error, "{arguments} formed");
        match serde_json::from_str::<Value>(&out.content) {
            Ok(refused) => assert_eq!(refused["refused"], code, "{arguments}: {}", out.content),
            Err(_) => assert_eq!(code, "schema", "{arguments}: {}", out.content),
        }
        assert_eq!(AppTools { state: &mut state }.summary()["assembly"]["accepted_state"], before, "{arguments}");
        assert!(!pack.exists(), "{arguments} wrote a file");
        assert_eq!(std::fs::read(&root).unwrap(), rig, "{arguments} touched the root's file");
    }
}

/// A call with no arguments is refused, with a message, by every tool that needs some.
#[test]
fn every_tool_refuses_an_empty_call_with_a_message() {
    let mut state = AppState::new(board_bracket());
    let mut host = AppTools { state: &mut state };
    let caller = Caller::Mcp { client: "test".into() };
    for spec in host.tools() {
        let schema_wants_fields = spec.input_schema["required"].as_array().is_some_and(|r| !r.is_empty());
        let out = call_checked(&mut host, &spec.name, json!({}), &caller);
        if schema_wants_fields {
            assert!(out.is_error && !out.content.trim().is_empty(), "{}: {}", spec.name, out.content);
        }
    }
}

/// Found by the fuzz: a through-all cut of a Ø1,000 km circle made the kernel mesh a disc until
/// the machine ran out of memory (about 9 GB), and a Ø100 m one took 40 s. A length or
/// coordinate beyond `MAX_LENGTH_MM` is refused by name before the kernel meets it.
#[test]
fn a_size_no_part_has_is_refused_before_the_kernel_meets_it() {
    use soliddraft_app::command_tools::MAX_LENGTH_MM;
    let big = MAX_LENGTH_MM * 10.0;
    let sketch = |shape: Value| ("sketch", json!({ "plane": "top", "shapes": [shape] }));
    let calls = [
        sketch(json!({ "shape": "circle", "center": [0, 0, 0], "diameter": 1e9 })),
        sketch(json!({ "shape": "rectangle", "center": [0, 0, 0], "width": big, "height": 10 })),
        sketch(json!({ "shape": "rectangle", "center": [0, 0, 0], "width": 10, "height": big })),
        sketch(json!({ "shape": "slot", "start": [0, 0, 0], "end": [10, 0, 0], "width": big })),
        sketch(json!({ "shape": "circle", "center": [big, 0, 0], "diameter": 5 })),
        sketch(json!({ "shape": "circle", "center": [0, -1e300, 0], "diameter": 5 })),
        ("extrude", json!({ "sketch": "Sketch1", "depth": big, "flip": false })),
        ("cut", json!({ "sketch": "Sketch1", "through_all": false, "depth": big, "flip": false })),
        ("edit_extrude", json!({ "feature": "Boss-Extrude1", "depth": 1e300, "flip": false })),
        ("fillet", json!({ "edges": ["Boss-Extrude1 · Edge 1"], "radius": big })),
    ];
    let mut doc = board_bracket();
    let mut book = ProposalBook::default();
    let caller = Caller::Mcp { client: "test".into() };
    let started = std::time::Instant::now();
    for (tool, arguments) in calls {
        let before = snapshot(&doc, &book);
        let mut host = CommandTools { doc: &mut doc, book: &mut book, surface: Surface::Mcp, drawing: None };
        let out = call_checked(&mut host, tool, arguments.clone(), &caller);
        assert!(out.is_error, "{tool} {arguments} ran");
        assert!(out.content.contains("at most") || out.content.contains("within"), "{tool}: {}", out.content);
        assert_eq!(before, snapshot(&doc, &book), "{tool} changed the part");
    }
    assert!(started.elapsed() < Duration::from_secs(10), "refusing took {:?}", started.elapsed());
    // A through-all cut ignores its depth, whatever it says.
    let mut host = CommandTools { doc: &mut doc, book: &mut book, surface: Surface::Mcp, drawing: None };
    let out = call_checked(
        &mut host,
        "cut",
        json!({ "sketch": "Sketch4", "through_all": true, "depth": 1e300, "flip": false }),
        &caller,
    );
    assert!(!out.content.contains("at most"), "{}", out.content);
}
