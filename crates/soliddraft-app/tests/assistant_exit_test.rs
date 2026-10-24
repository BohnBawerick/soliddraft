//! Phase 3 exit test: the sample three exchanges work for real. A fillet, a hole
//! pattern and a slot are proposed from text, previewed, applied and undone, through
//! the conversation loop, the command tools and the same command layer as the ribbon;
//! and an outside agent drives the same commands over MCP.
//!
//! The exchanges run on the mock provider always, and on the real model when an API key
//! is available: OPENROUTER_API_KEY or ANTHROPIC_API_KEY, or either in the OS keychain.
//! The provider is chosen as the app chooses it, so SOLIDDRAFT_PROVIDER and SOLIDDRAFT_MODEL
//! pick another one. Without a key the real-model test says it was skipped. Its transcript
//! goes to `target/exit-test/assistant-real.txt`.

use serde_json::{Value, json};
use soliddraft_app::chat::{ChatState, Role};
use soliddraft_app::command_tools::{Line, ProposalStatus, Shape, board_bracket};
use soliddraft_app::document::{Document, FeatureStatus, Selection};
use soliddraft_app::state::AppState;
use soliddraft_assistant::mcp::HttpClient;
use soliddraft_assistant::{MockProvider, MockTurn};
use std::f64::consts::PI;
use std::sync::Arc;

const FILLET: &str = "Add a fillet to the inside corner, R8";
const HOLES: &str = "Add four 9 mm holes on a 90 x 44 pattern";
const SLOT: &str = "Cut a 40 x 12 slot through the flange";

/// Material each exchange adds (positive) or removes (negative), mm³.
const HOLES_REMOVE: f64 = 4.0 * PI * 4.5 * 4.5 * 10.0;
const SLOT_REMOVES: f64 = (28.0 * 12.0 + PI * 36.0) * 10.0;

fn volume(doc: &Document) -> f64 {
    doc.kernel().mass_properties(doc.display_body().expect("a body")).expect("mass properties").volume
}

/// The edge where the flange meets the base plate.
fn inside_corner(doc: &Document) -> String {
    let edge = doc
        .edges
        .iter()
        .find(|e| (e.midpoint - glam::Vec3::new(60.0, 10.0, 10.0)).length() < 0.01 && (e.length - 120.0).abs() < 0.01)
        .expect("the inside corner edge");
    doc.describe(Selection::Edge(edge.id)).unwrap().title
}

fn holes() -> Value {
    json!({
        "summary": "Four Ø9 holes on 90 × 44",
        "commands": [
            { "op": "sketch", "plane": "Boss-Extrude1 · End cap", "shapes": [
                { "shape": "circle", "center": [15, 28, 10], "diameter": 9 },
                { "shape": "circle", "center": [105, 28, 10], "diameter": 9 },
                { "shape": "circle", "center": [15, 72, 10], "diameter": 9 },
                { "shape": "circle", "center": [105, 72, 10], "diameter": 9 }
            ]},
            { "op": "cut", "sketch": "previous", "through_all": true, "depth": 10, "flip": false }
        ]
    })
}

fn slot() -> Value {
    json!({
        "summary": "40 × 12 slot through the flange",
        "commands": [
            { "op": "sketch", "plane": "front", "shapes": [
                { "shape": "slot", "start": [46, 0, 35], "end": [74, 0, 35], "width": 12 }
            ]},
            { "op": "cut", "sketch": "previous", "through_all": false, "depth": 10, "flip": false }
        ]
    })
}

/// Sends `prompt` and waits for the answer; returns the id of the proposal it made.
fn exchange(st: &mut AppState, prompt: &str, transcript: &mut String) -> Option<String> {
    let before = st.chat.messages.len();
    st.chat.draft = prompt.into();
    st.send_chat();
    st.wait_assistant();
    transcript.push_str(&format!("\n> {prompt}\n"));
    for m in &st.chat.messages[before..] {
        match m.role {
            Role::User => {}
            _ => transcript.push_str(&format!("{:?}: {}\n", m.role, m.text)),
        }
        for t in &m.tools {
            transcript.push_str(&format!("  tool {} {:?}\n", t.name, t.state));
        }
        for p in &m.proposals {
            let p = st.chat.book.get(p).unwrap();
            transcript.push_str(&format!("  proposal {} \"{}\"\n", p.id, p.summary));
            for (k, v) in &p.rows {
                transcript.push_str(&format!("    {k}: {v}\n"));
            }
        }
    }
    st.chat.messages[before..].iter().rev().flat_map(|m| m.proposals.iter().rev()).next().cloned()
}

/// Previews, applies and undoes a proposal, and checks what it does to the part.
fn apply_and_undo(st: &mut AppState, id: &str, prompt: &str, volume_change: f64, tolerance: f64) {
    let names_before: Vec<String> = st.doc.features.iter().map(|f| f.name.clone()).collect();
    let v_before = volume(&st.doc);
    assert!(st.chat.book.get(id).unwrap().pending(), "{id} waits for the user");

    st.preview_proposal(id);
    assert!(st.preview_body().is_some(), "{id}: Preview shows the part after the proposal");
    assert_eq!(st.doc.features.len(), names_before.len(), "{id}: previewing changes nothing");

    st.apply_proposal(id);
    assert!(st.preview_body().is_none());
    assert!(st.doc.features.len() > names_before.len(), "{id} added features");
    for f in &st.doc.features {
        assert_eq!(f.status, FeatureStatus::Ok, "{id}: {} after applying", f.name);
    }
    let added: Vec<_> = st.doc.features.iter().filter(|f| !names_before.contains(&f.name)).collect();
    assert!(added.iter().all(|f| f.via_assistant), "{id}: new features carry the assistant mark");
    let change = volume(&st.doc) - v_before;
    assert!(
        (change - volume_change).abs() < tolerance,
        "{id}: volume changed by {change:.1} mm³, expected {volume_change:.1}"
    );
    let log = st.doc.command_log().join("\n");
    assert!(log.contains(&format!("apply {id}")) && log.contains(prompt), "{log}");
    assert!(log.contains("model claude-"), "the command log names the model: {log}");

    assert!(st.can_undo(id));
    st.undo_proposal(id);
    let names_after: Vec<String> = st.doc.features.iter().map(|f| f.name.clone()).collect();
    assert_eq!(names_after, names_before, "{id}: undo restores the tree");
    assert!((volume(&st.doc) - v_before).abs() < 1e-6, "{id}: undo restores the body");
    assert_eq!(st.proposal_status(id), Some(ProposalStatus::Undone));
}

#[test]
fn the_three_board_exchanges_with_the_mock_provider() {
    let doc = board_bracket();
    let corner = inside_corner(&doc);
    let provider = Arc::new(MockProvider::new(vec![
        // Fillet: the model looks at the flange, then proposes on the concave edge.
        MockTurn::tools("", vec![("feature", json!({ "name": "Boss-Extrude2" }))]),
        MockTurn::tools(
            "",
            vec![(
                "propose",
                json!({ "summary": "R8 fillet on the inside corner", "commands": [{ "op": "fillet", "edges": [corner], "radius": 8 }] }),
            )],
        ),
        MockTurn::text(
            "Fillets between planes and cylinders wait for Catallax stage 3; other fillets for Catallax stage 4.",
        ),
        // Hole pattern.
        MockTurn::tools("", vec![("propose", holes())]),
        MockTurn::text("Four Ø9 holes through the base on a 90 × 44 pattern centred on the free plate."),
        // Slot.
        MockTurn::tools("", vec![("propose", slot())]),
        MockTurn::text("A 40 × 12 slot through the flange, centred 35 mm up."),
    ]));
    let mut st = AppState::new(doc);
    st.chat = ChatState::connected(provider.clone(), "mock provider");
    let mut transcript = String::new();

    let before = st.doc.features.len();
    assert!(exchange(&mut st, FILLET, &mut transcript).is_none(), "{transcript}");
    assert!(transcript.contains("Catallax stage 3") && transcript.contains("Catallax stage 4"), "{transcript}");
    assert_eq!(st.doc.features.len(), before);

    let p = exchange(&mut st, HOLES, &mut transcript).expect("a hole proposal");
    apply_and_undo(&mut st, &p, HOLES, -HOLES_REMOVE, 30.0);

    let p = exchange(&mut st, SLOT, &mut transcript).expect("a slot proposal");
    apply_and_undo(&mut st, &p, SLOT, -SLOT_REMOVES, 30.0);

    assert_eq!(provider.remaining(), 0, "every scripted turn was used");
    // Every request after the first replays the history it was given, unchanged.
    let requests = provider.requests();
    for pair in requests.windows(2) {
        let (a, b) = (&pair[0].messages, &pair[1].messages);
        assert_eq!(&b[..a.len()], &a[..], "the history only grows");
        assert_eq!(pair[0].system, pair[1].system, "the cached system prompt never changes");
    }
    let names: Vec<&str> = requests[0].tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["model_summary", "feature", "selection", "measure", "propose", "apply"]);
    eprintln!("{transcript}");
}

#[test]
fn the_three_board_exchanges_with_the_real_model() {
    let choice = match soliddraft_assistant::choice::choose(&Default::default()) {
        Ok(c) => c,
        Err(why) => {
            eprintln!("skipped the real-model exit test: {why}");
            return;
        }
    };
    let how = choice.source.describe();
    let provider = choice.connect().expect("provider");
    eprintln!("real-model exit test on {} {} with the {how}", provider.name(), provider.model());
    let mut st = AppState::new(board_bracket());
    st.chat = ChatState::connected(provider, &how);
    let mut transcript = String::new();
    let check = |st: &mut AppState, prompt: &str, change: f64, tolerance: f64, transcript: &mut String| {
        let id =
            exchange(st, prompt, transcript).unwrap_or_else(|| panic!("no proposal for {prompt:?}:\n{transcript}"));
        apply_and_undo(st, &id, prompt, change, tolerance);
        id
    };

    let p = check(&mut st, HOLES, -HOLES_REMOVE, 60.0, &mut transcript);
    let lines = st.chat.book.get(&p).unwrap().lines.clone();
    let circles: usize = lines
        .iter()
        .map(|l| match l {
            Line::Sketch { shapes, .. } => {
                shapes.iter().filter(|s| matches!(s, Shape::Circle { diameter, .. } if *diameter == 9.0)).count()
            }
            _ => 0,
        })
        .sum();
    assert_eq!(circles, 4, "{lines:?}");

    let p = check(&mut st, SLOT, -SLOT_REMOVES, 120.0, &mut transcript);
    let lines = st.chat.book.get(&p).unwrap().lines.clone();
    assert!(
        lines.iter().any(|l| matches!(l, Line::Sketch { shapes, .. } if shapes.iter().any(|s| matches!(s, Shape::Slot { width, .. } if *width == 12.0)))),
        "{lines:?}"
    );

    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/exit-test");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("assistant-real.txt"), &transcript).unwrap();
    eprintln!("{transcript}");
}

#[test]
fn an_outside_agent_drives_the_same_commands_over_mcp() {
    let doc = board_bracket();
    let corner = inside_corner(&doc);
    let features = doc.features.len();
    let v0 = volume(&doc);
    let mut st = AppState::new(doc);
    let url = st.start_mcp(0, None).unwrap();
    let agent = std::thread::spawn(move || {
        let (mut c, init) = HttpClient::connect(&url, "exit-test-agent").unwrap();
        assert_eq!(init["serverInfo"]["name"], "soliddraft");
        let call = |c: &mut HttpClient, name: &str, args: Value| {
            let r = c.call_tool(name, args).unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(r["isError"], false, "{name}: {r}");
            r["structuredContent"].clone()
        };
        let mut volumes = Vec::new();
        let body_volume = |c: &mut HttpClient| {
            let summary = call(c, "model_summary", json!({}));
            let feature = summary["features"]
                .as_array()
                .unwrap()
                .iter()
                .rev()
                .find(|f| f["type"] != "sketch" && f["status"] == "ok")
                .unwrap()["name"]
                .as_str()
                .unwrap();
            call(c, "mass_properties", json!({"name":feature}))["volume_mm3"].as_f64().unwrap()
        };
        volumes.push(body_volume(&mut c));
        let waiting = c.call_tool("fillet", json!({ "edges": [corner], "radius": 8 })).unwrap();
        assert_eq!(waiting["isError"], true);
        let message = waiting.to_string();
        assert!(message.contains("Catallax stage 3") && message.contains("Catallax stage 4"), "{message}");
        volumes.push(body_volume(&mut c));
        let h = holes();
        call(
            &mut c,
            "sketch",
            h["commands"][0]
                .as_object()
                .unwrap()
                .iter()
                .filter(|(k, _)| *k != "op")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        );
        call(&mut c, "cut", json!({ "sketch": "Sketch5", "through_all": true, "depth": 10, "flip": false }));
        volumes.push(body_volume(&mut c));
        let s = slot();
        call(
            &mut c,
            "sketch",
            s["commands"][0]
                .as_object()
                .unwrap()
                .iter()
                .filter(|(k, _)| *k != "op")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        );
        call(&mut c, "cut", json!({ "sketch": "Sketch6", "through_all": false, "depth": 10, "flip": false }));
        volumes.push(body_volume(&mut c));
        // A reference that does not resolve is refused before anything happens.
        let bad = c.call_tool("fillet", json!({ "edges": ["Fillet1 · Edge 999"], "radius": 2 })).unwrap();
        assert_eq!(bad["isError"], true);
        for _ in 0..4 {
            call(&mut c, "undo", json!({}));
        }
        volumes.push(body_volume(&mut c));
        volumes
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
    while !agent.is_finished() && std::time::Instant::now() < deadline {
        st.service_mcp();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let v = agent.join().unwrap();
    assert!((v[0] - v0).abs() < 0.1);
    assert_eq!(v[1], v[0], "a waiting fillet leaves the body unchanged");
    assert!((v[2] - v[1] + HOLES_REMOVE).abs() < 30.0, "holes {v:?}");
    assert!((v[3] - v[2] + SLOT_REMOVES).abs() < 30.0, "slot {v:?}");
    assert!((v[4] - v0).abs() < 0.1, "undone {v:?}");
    assert_eq!(st.doc.features.len(), features);
    assert!(st.doc.command_log().iter().any(|l| l.contains("MCP client exit-test-agent")));
}

#[test]
fn without_a_key_the_chat_says_why() {
    let mut st = AppState::new(soliddraft_app::document::Document::new(
        "Part1",
        std::sync::Arc::new(soliddraft_kernel_catallax::CatallaxKernel::new()),
    ));
    st.chat = ChatState::offline("No API key, so the assistant is off.".into());
    st.chat.draft = "hello".into();
    st.send_chat();
    let last = st.chat.messages.last().unwrap();
    assert_eq!(last.role, Role::System);
    assert!(last.text.contains("No API key"));
}
