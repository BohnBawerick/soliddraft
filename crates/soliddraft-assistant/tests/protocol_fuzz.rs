//! Fuzzing the MCP server's inputs below the commands: the JSON-RPC layer, the schema
//! check every tool input passes, the stdio line loop and the HTTP endpoint.
//!
//! Whatever arrives, the server must not panic, must answer every request with a
//! well-formed JSON-RPC reply that names an error when the input was bad, and must stay
//! up for the next message. The commands behind the tools are fuzzed in
//! `soliddraft-app`'s `command_fuzz`.
//!
//! Runs a fixed number of cases from a fixed seed, so a failure reproduces. To search
//! further: `PROPTEST_CASES=20000 PROPTEST_RNG_SEED=7 cargo test -p soliddraft-assistant
//! --test protocol_fuzz`.

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::OnceLock;
use std::time::Duration;

use proptest::prelude::*;
use proptest::test_runner::RngSeed;
use serde_json::{Map, Number, Value, json};
use soliddraft_assistant::Caller;
use soliddraft_assistant::mcp::{HttpServer, McpSession, serve_lines};
use soliddraft_assistant::tools::{ToolHost, ToolOutput, ToolSpec, schema::*, validate};

/// Cases per property, and the seed they come from, unless the environment says otherwise.
fn config() -> ProptestConfig {
    // A failing input is printed with the test's seed; nothing is written beside the sources.
    let mut config = ProptestConfig { failure_persistence: None, ..ProptestConfig::default() };
    if std::env::var_os("PROPTEST_CASES").is_none() {
        config.cases = 400;
    }
    if std::env::var_os("PROPTEST_RNG_SEED").is_none() {
        config.rng_seed = RngSeed::Fixed(0x50_4c_49_44);
    }
    config
}

// ----- inputs -----

fn numbers() -> impl Strategy<Value = Value> {
    prop_oneof![
        any::<i64>().prop_map(Value::from),
        any::<u64>().prop_map(Value::from),
        prop_oneof![
            Just(0.0),
            Just(-0.0),
            Just(f64::MAX),
            Just(f64::MIN),
            Just(f64::MIN_POSITIVE),
            Just(5e-324),
            Just(1e300),
            Just(-1e300),
            Just(0.1),
            Just(9007199254740993.0),
            any::<f64>(),
        ]
        .prop_filter_map("finite", |f| Number::from_f64(f).map(Value::Number)),
    ]
}

fn texts() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        Just("\u{0}".to_owned()),
        Just("Boss-Extrude1 · End cap".to_owned()),
        Just("previous".to_owned()),
        Just("\u{202e}rtl\u{200b}".to_owned()),
        Just("😀".repeat(500)),
        Just("a".repeat(100_000)),
        "\\PC{0,24}",
        any::<String>(),
    ]
}

/// Any JSON value, nested a few levels deep.
fn json_value() -> impl Strategy<Value = Value> {
    let leaf =
        prop_oneof![Just(Value::Null), any::<bool>().prop_map(Value::Bool), numbers(), texts().prop_map(Value::String)];
    leaf.prop_recursive(5, 64, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            prop::collection::vec((texts(), inner), 0..6)
                .prop_map(|kv| Value::Object(kv.into_iter().collect::<Map<_, _>>())),
        ]
    })
}

/// A JSON-RPC id of any type, including the ones the specification does not allow.
fn id() -> impl Strategy<Value = Value> {
    prop_oneof![
        3 => any::<u32>().prop_map(Value::from),
        1 => texts().prop_map(Value::String),
        1 => Just(Value::Null),
        1 => json_value(),
    ]
}

fn method() -> impl Strategy<Value = Value> {
    prop_oneof![
        4 => prop::sample::select(vec!["initialize", "ping", "tools/list", "tools/call", "notifications/initialized"])
            .prop_map(Value::from),
        1 => texts().prop_map(Value::String),
        1 => json_value(),
    ]
}

/// A message that is mostly request-shaped, with fields dropped, retyped or added.
fn message() -> impl Strategy<Value = Value> {
    let params = prop_oneof![
        2 => json_value(),
        2 => (json_value(), json_value(), json_value()).prop_map(|(name, arguments, extra)| {
            json!({ "name": name, "arguments": arguments, "extra": extra })
        }),
        2 => (texts(), json_value()).prop_map(|(name, arguments)| json!({ "name": name, "arguments": arguments })),
        1 => (json_value(), json_value()).prop_map(|(version, info)| {
            json!({ "protocolVersion": version, "clientInfo": info, "capabilities": {} })
        }),
        1 => Just(json!({ "name": "shape", "arguments": { "name": "x", "values": [1, 2], "kind": "a" } })),
    ];
    (
        prop::option::weighted(0.85, id()),
        prop::option::weighted(0.9, method()),
        prop::option::weighted(0.8, params),
        json_value(),
    )
        .prop_map(|(id, method, params, junk)| {
            let mut m = Map::new();
            m.insert("jsonrpc".into(), json!("2.0"));
            if let Some(id) = id {
                m.insert("id".into(), id);
            }
            if let Some(method) = method {
                m.insert("method".into(), method);
            }
            if let Some(params) = params {
                m.insert("params".into(), params);
            }
            if junk.is_object() {
                m.extend(junk.as_object().unwrap().clone());
            }
            Value::Object(m)
        })
}

fn input() -> impl Strategy<Value = Value> {
    prop_oneof![
        6 => message(),
        2 => prop::collection::vec(message(), 0..5).prop_map(Value::Array),
        2 => json_value(),
    ]
}

// ----- the host -----

/// A tool with every schema keyword the strict subset uses.
struct Host;

fn shape_schema() -> Value {
    let mut schema = object(&[
        ("name", string("n")),
        ("values", array(number_schema(), "v")),
        ("kind", one_of(&["a", "b"], "k")),
        ("pick", any_of(vec![object(&[("shape", constant("p")), ("size", number_schema())]), string("s")])),
        ("flag", optional(boolean("f"))),
        ("depth", optional(json!({ "$ref": "#/$defs/depth" }))),
        ("count", optional(integer("c"))),
        ("list", optional(non_empty_array(string("i"), "l"))),
    ]);
    schema["$defs"] = json!({ "depth": { "type": "object", "properties": { "inner": { "type": "number" }, "again": { "$ref": "#/$defs/depth" } }, "required": ["inner"], "additionalProperties": false } });
    schema
}

fn number_schema() -> Value {
    number("n")
}

impl ToolHost for Host {
    fn tools(&self) -> Vec<ToolSpec> {
        vec![ToolSpec { name: "shape".into(), description: "A tool".into(), input_schema: shape_schema() }]
    }

    fn call(&mut self, _: &str, input: Value, _: &Caller) -> ToolOutput {
        assert!(validate(&shape_schema(), &input).is_ok(), "a tool ran on input that fails its schema: {input}");
        ToolOutput::json(&json!({ "got": input }))
    }
}

// ----- what a reply must look like -----

fn assert_reply(request: &Value, reply: &Value) {
    assert_eq!(reply["jsonrpc"], "2.0", "{reply}");
    let object = reply.as_object().unwrap_or_else(|| panic!("a reply is an object: {reply}"));
    assert!(object.contains_key("id"), "{reply}");
    match (object.get("result"), object.get("error")) {
        (Some(result), None) => {
            if let Some(is_error) = result.get("isError") {
                assert!(is_error.is_boolean(), "{reply}");
                let content =
                    result["content"][0]["text"].as_str().unwrap_or_else(|| panic!("a tool reply has text: {reply}"));
                assert!(!content.trim().is_empty(), "{reply}");
                assert!(
                    !is_error.as_bool().unwrap() || content.len() < 9_000,
                    "a refusal does not echo the whole input back"
                );
            }
        }
        (None, Some(error)) => {
            assert!(error["code"].is_i64(), "an error has a numeric code: {reply}");
            assert!(error["message"].as_str().is_some_and(|m| !m.trim().is_empty()), "an error says why: {reply}");
            assert!(
                error["message"].as_str().is_some_and(|m| m.len() < 9_000),
                "an error does not echo the whole input back"
            );
        }
        _ => panic!("a reply holds a result or an error, not both or neither: {reply}"),
    }
    if let Some(id) = request.get("id") {
        assert_eq!(&reply["id"], id, "the reply carries the request's id");
    }
}

fn assert_replies(request: &Value, reply: Option<Value>) {
    match (request, reply) {
        (Value::Array(requests), Some(Value::Array(replies))) => {
            assert!(!replies.is_empty());
            assert!(replies.len() <= requests.len());
            for reply in &replies {
                assert_reply(&json!({}), reply);
            }
        }
        (Value::Array(_), Some(single)) => assert_reply(&json!({}), &single),
        (_, Some(reply)) => assert_reply(request, &reply),
        (_, None) => {}
    }
}

fn session() -> McpSession {
    McpSession::new("soliddraft", "0.1.0", "CAD")
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn any_json_gets_a_wellformed_reply_or_none(messages in prop::collection::vec(input(), 1..6)) {
        let mut host = Host;
        let mut session = session();
        for message in messages {
            let reply = session.handle(&mut host, message.clone());
            assert_replies(&message, reply);
        }
    }

    #[test]
    fn any_text_gets_a_parse_error_or_a_reply(text in prop_oneof![any::<String>(), "[\\[\\]{}\",:0-9a-z. -]{0,80}"]) {
        let mut host = Host;
        let reply = session().handle_text(&mut host, &text);
        if let Some(reply) = reply {
            assert_reply(&json!({}), &reply);
        }
    }

    #[test]
    fn deeply_nested_input_is_refused_not_overflowed(depth in 1usize..20_000, open in prop::sample::select(vec!['[', '{'])) {
        let text = match open {
            '[' => format!("{}{}", "[".repeat(depth), "]".repeat(depth)),
            _ => format!("{}1{}", "{\"a\":".repeat(depth), "}".repeat(depth)),
        };
        let mut host = Host;
        let reply = session().handle_text(&mut host, &text);
        if let Some(reply) = reply {
            assert_reply(&json!({}), &reply);
        }
    }

    /// The schema check against an oracle written separately for one small schema.
    #[test]
    fn validate_agrees_with_an_oracle(value in json_value(), keys in prop::collection::vec(prop::sample::select(vec!["n", "extra", "s"]), 0..4), values in prop::collection::vec(json_value(), 4)) {
        let schema = object(&[("n", number("n")), ("s", optional(string("s")))]);
        let mut candidates = vec![value];
        let mut map = Map::new();
        for (key, v) in keys.iter().zip(values) {
            map.insert((*key).to_owned(), v);
        }
        candidates.push(Value::Object(map));
        for candidate in candidates {
            let expected = candidate.as_object().is_some_and(|o| {
                o.get("n").is_some_and(Value::is_number)
                    && o.get("s").is_none_or(Value::is_string)
                    && o.keys().all(|k| k == "n" || k == "s")
            });
            prop_assert_eq!(validate(&schema, &candidate).is_ok(), expected, "{}", candidate);
        }
    }

    /// Garbage on standard input, then a good request: the server answers the good one.
    #[test]
    fn the_stdio_loop_survives_garbage_lines(garbage in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..60), 0..6)) {
        let mut input = Vec::new();
        for line in &garbage {
            input.extend(line.iter().copied().filter(|b| *b != b'\n'));
            input.push(b'\n');
        }
        input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":77,\"method\":\"ping\"}\n");
        let mut output = Vec::new();
        let mut host = Host;
        let served = serve_lines(&mut session(), &mut host, input.as_slice(), &mut output);
        prop_assert!(served.is_ok(), "the loop ended early: {:?}", served);
        let text = String::from_utf8(output).expect("replies are text");
        let replies: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).expect("each reply line is JSON")).collect();
        for reply in &replies {
            assert_reply(&json!({}), reply);
        }
        prop_assert!(replies.iter().any(|r| r["id"] == 77 && r["result"] == json!({})), "the ping after the garbage was not answered: {text}");
    }
}

#[test]
fn a_huge_tool_or_method_name_is_not_echoed_back_whole() {
    let name = "n".repeat(2 * 1024 * 1024);
    for request in [
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name } }),
        json!({ "jsonrpc": "2.0", "id": 1, "method": name }),
    ] {
        let reply = session().handle(&mut Host, request.clone()).expect("answered");
        assert_reply(&request, &reply);
        assert_eq!(reply["error"]["code"].as_i64().map(|c| c < 0), Some(true), "{}", &reply.to_string()[..200]);
    }
}

#[test]
fn an_empty_batch_is_an_invalid_request() {
    let mut host = Host;
    let reply = session().handle(&mut host, json!([])).expect("an empty batch is answered");
    assert_eq!(reply["error"]["code"], -32600, "{reply}");
}

#[test]
fn a_line_that_is_not_utf8_is_a_parse_error_and_the_loop_goes_on() {
    let mut input = vec![0xff, 0xfe, b'\n'];
    input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n");
    let mut output = Vec::new();
    let mut host = Host;
    serve_lines(&mut session(), &mut host, input.as_slice(), &mut output).expect("the loop reads on");
    let replies: Vec<Value> =
        String::from_utf8(output).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(replies.len(), 2, "{replies:?}");
    assert_eq!(replies[0]["error"]["code"], -32700);
    assert_eq!(replies[1]["id"], 1);
}

#[test]
fn a_line_longer_than_any_message_is_refused_whole_and_the_loop_goes_on() {
    let mut input = vec![b'a'; 17 * 1024 * 1024];
    input.extend_from_slice(b"\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n");
    let mut output = Vec::new();
    serve_lines(&mut session(), &mut Host, input.as_slice(), &mut output).expect("the loop reads on");
    let replies: Vec<Value> =
        String::from_utf8(output).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(replies.len(), 2, "{replies:?}");
    assert_eq!(replies[0]["error"]["code"], -32600);
    assert_eq!(replies[1]["id"], 1);
}

// ----- HTTP -----

/// One server for the whole file, answered by a session on its own thread.
fn server() -> SocketAddr {
    static SERVER: OnceLock<(HttpServer, SocketAddr)> = OnceLock::new();
    SERVER
        .get_or_init(|| {
            let (server, calls) = HttpServer::start(0, None).expect("a loopback port");
            let addr = server.addr();
            std::thread::spawn(move || {
                let mut host = Host;
                let mut session = session();
                while let Ok(call) = calls.recv() {
                    let reply = session.handle(&mut host, call.message.clone());
                    call.respond(reply);
                }
            });
            (server, addr)
        })
        .1
}

/// Sends `bytes`, then closes the sending side and reads what the server answers.
fn exchange(bytes: &[u8]) -> String {
    let mut stream = TcpStream::connect(server()).expect("connect");
    stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    // The server may answer and close before the whole of a large request is sent.
    let _ = stream.write_all(bytes);
    let _ = stream.shutdown(Shutdown::Write);
    let mut reply = Vec::new();
    match stream.read_to_end(&mut reply) {
        Ok(_) => {}
        // The server closed on a request it refused while the rest was in flight.
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        Err(e) => panic!("the server neither answered nor closed: {e}"),
    }
    String::from_utf8_lossy(&reply).into_owned()
}

fn post(body: &str) -> String {
    format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn status(reply: &str) -> Option<u16> {
    reply.strip_prefix("HTTP/1.1 ")?.split_whitespace().next()?.parse().ok()
}

fn assert_alive() {
    let reply = exchange(post(r#"{"jsonrpc":"2.0","id":5,"method":"ping"}"#).as_bytes());
    assert_eq!(status(&reply), Some(200), "the server stopped answering: {reply:?}");
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 120, ..config() })]

    #[test]
    fn http_survives_arbitrary_bytes(bytes in prop_oneof![
        prop::collection::vec(any::<u8>(), 0..400),
        "(POST|GET|DELETE|PUT|\u{0}) [/a-z?=%0-9]{0,12} HTTP/1\\.[01]\r\n([A-Za-z-]{1,16}: [ -~]{0,30}\r\n){0,6}\r\n[ -~]{0,60}".prop_map(String::into_bytes),
    ]) {
        let reply = exchange(&bytes);
        if !reply.is_empty() {
            prop_assert!(status(&reply).is_some(), "an answer starts with a status line: {reply:?}");
        }
        assert_alive();
    }

    #[test]
    fn http_content_lengths_that_lie_are_answered_or_closed(declared in prop_oneof![
        Just("0".to_owned()), Just("-1".to_owned()), Just("18446744073709551616".to_owned()), Just("99999999999".to_owned()),
        Just("16777217".to_owned()), Just("abc".to_owned()), Just("5, 5".to_owned()), Just(" ".to_owned()), "[0-9]{1,12}",
    ], sent in "[ -~]{0,40}") {
        let request = format!("POST /mcp HTTP/1.1\r\nContent-Length: {declared}\r\nConnection: close\r\n\r\n{sent}");
        let reply = exchange(request.as_bytes());
        if !reply.is_empty() {
            prop_assert!(status(&reply).is_some(), "{reply:?}");
        }
        assert_alive();
    }
}

#[test]
fn http_refuses_an_oversized_header_line_instead_of_buffering_it() {
    // A line with no end: the server must answer and close without holding all of it.
    let request = format!("POST /mcp HTTP/1.1\r\nX-Padding: {}", "a".repeat(2 * 1024 * 1024));
    let reply = exchange(request.as_bytes());
    assert_eq!(status(&reply), Some(431), "{:?}", &reply[..reply.len().min(200)]);
    assert_alive();
}

#[test]
fn http_refuses_an_oversized_request_line() {
    let reply = exchange(format!("GET /{} HTTP/1.1\r\n\r\n", "a".repeat(2 * 1024 * 1024)).as_bytes());
    assert_eq!(status(&reply), Some(431), "{:?}", &reply[..reply.len().min(200)]);
    assert_alive();
}

#[test]
fn http_refuses_endless_headers() {
    let headers: String = (0..5000).map(|i| format!("X-{i}: v\r\n")).collect();
    let reply = exchange(format!("POST /mcp HTTP/1.1\r\n{headers}\r\n").as_bytes());
    assert_eq!(status(&reply), Some(431), "{:?}", &reply[..reply.len().min(200)]);
    assert_alive();
}

#[test]
fn http_refuses_a_request_that_is_not_text() {
    let reply = exchange(b"POST /mcp HTTP/1.1\r\nX-Bad: \xff\xfe\r\n\r\n");
    assert_eq!(status(&reply), Some(400), "{reply:?}");
    assert_alive();
}
