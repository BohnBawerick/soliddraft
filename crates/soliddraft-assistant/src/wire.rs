//! Messages API wire shapes: conversation messages, the request body, and the
//! accumulator that turns the streamed events back into a message.
//!
//! Content blocks stay as JSON values. Thinking blocks carry signatures and must go back
//! to the API byte for byte, and new block types (such as `fallback`) appear without
//! notice; keeping the JSON is the only way to echo every one of them faithfully.

use crate::tools::ToolSpec;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Who wrote a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// The person, or tool results going back to the model.
    User,
    /// The model.
    Assistant,
}

/// One conversation message, as sent to and received from the API.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// Who wrote it.
    pub role: Role,
    /// Its content blocks, verbatim.
    pub content: Vec<Value>,
}

impl Message {
    /// A user message of text blocks.
    pub fn user_text(parts: &[&str]) -> Self {
        Self { role: Role::User, content: parts.iter().map(|t| text_block(t)).collect() }
    }
}

/// A `text` block.
pub fn text_block(text: &str) -> Value {
    json!({ "type": "text", "text": text })
}

/// A `tool_result` block.
pub fn tool_result_block(tool_use_id: &str, content: &str, is_error: bool) -> Value {
    let mut b = json!({ "type": "tool_result", "tool_use_id": tool_use_id, "content": content });
    if is_error {
        b["is_error"] = Value::Bool(true);
    }
    b
}

/// The type of a content block.
pub fn block_type(block: &Value) -> &str {
    block.get("type").and_then(Value::as_str).unwrap_or("")
}

/// How hard the model works on a reply.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    /// Least thinking.
    Low,
    /// The default for `claude-opus-5-5`.
    #[default]
    Medium,
    /// More thinking.
    High,
    /// More again, for long agentic work.
    Xhigh,
    /// No limit.
    Max,
}

/// Everything a provider needs for one request.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatRequest {
    /// The system prompt: stable for the whole conversation, so it caches.
    pub system: String,
    /// The tools: also stable for the whole conversation.
    pub tools: Vec<ToolSpec>,
    /// The conversation so far, ending with a user message.
    pub messages: Vec<Message>,
    /// The effort level.
    pub effort: Effort,
    /// Output limit, thinking included.
    pub max_tokens: u32,
}

/// The beta that turns on server-side refusal fallbacks in their `"default"` form.
pub const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

/// Builds the Messages API body for a streamed request.
///
/// The system prompt and tools render first and carry the cache breakpoint, so every
/// request of the conversation reads them from the cache; the top-level `cache_control`
/// adds the automatic breakpoint at the end of the messages. There is no `thinking`
/// field (thinking is always on for `claude-opus-5-5`) and no `tool_choice` (forced tool
/// use is rejected on that model; the system prompt steers instead).
pub fn request_body(model: &str, req: &ChatRequest, fallbacks: bool) -> Value {
    let tools: Vec<Value> = req
        .tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.input_schema,
                "strict": true,
                "eager_input_streaming": true,
            })
        })
        .collect();
    let mut body = json!({
        "model": model,
        "max_tokens": req.max_tokens,
        "stream": true,
        "system": [{ "type": "text", "text": req.system, "cache_control": { "type": "ephemeral" } }],
        "tools": tools,
        "output_config": { "effort": req.effort },
        "cache_control": { "type": "ephemeral" },
        "messages": req.messages,
    });
    if fallbacks {
        body["fallbacks"] = json!("default");
    }
    body
}

/// Token counts from a response.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Uncached input tokens.
    pub input_tokens: u64,
    /// Output tokens, thinking included.
    pub output_tokens: u64,
    /// Input tokens written to the cache.
    pub cache_creation_input_tokens: u64,
    /// Input tokens read from the cache.
    pub cache_read_input_tokens: u64,
}

impl Usage {
    fn merge(&mut self, v: &Value) {
        let get = |k: &str| v.get(k).and_then(Value::as_u64);
        if let Some(n) = get("input_tokens") {
            self.input_tokens = n;
        }
        if let Some(n) = get("output_tokens") {
            self.output_tokens = n;
        }
        if let Some(n) = get("cache_creation_input_tokens") {
            self.cache_creation_input_tokens = n;
        }
        if let Some(n) = get("cache_read_input_tokens") {
            self.cache_read_input_tokens = n;
        }
    }
}

/// What the accumulator reports as a stream arrives.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamEvent {
    /// More reply text.
    TextDelta(String),
    /// The model started a tool call.
    ToolUseStarted {
        /// The block index.
        index: usize,
        /// The tool-use id.
        id: String,
        /// The tool's name.
        name: String,
    },
    /// More of a tool call's input arrived; `bytes` is the length so far.
    ToolInputDelta {
        /// The block index.
        index: usize,
        /// Input bytes received so far.
        bytes: usize,
    },
    /// The request moved to a fallback model after a refusal.
    Fallback {
        /// The model that declined.
        from: String,
        /// The model that continued.
        to: String,
    },
}

/// A finished response.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderResponse {
    /// The message id.
    pub id: String,
    /// The model that produced the message.
    pub model: String,
    /// The content blocks, ready to echo back.
    pub content: Vec<Value>,
    /// Why the model stopped.
    pub stop_reason: Option<String>,
    /// Details for a refusal.
    pub stop_details: Option<Value>,
    /// Token counts.
    pub usage: Usage,
    /// Tool inputs that were not valid JSON, by block index, with the raw text.
    pub invalid_inputs: Vec<(usize, String)>,
}

/// Turns streamed Messages API events into a [`ProviderResponse`].
#[derive(Default)]
pub struct Accumulator {
    response: ProviderResponse,
    /// Raw `partial_json` per tool-use block index.
    partial: Vec<(usize, String)>,
    done: bool,
}

/// An `error` event in the stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamError {
    /// The error type, such as `overloaded_error`.
    pub kind: String,
    /// The message.
    pub message: String,
}

impl Accumulator {
    /// Whether `message_stop` arrived.
    pub fn done(&self) -> bool {
        self.done
    }

    /// Whether any content arrived yet.
    pub fn has_content(&self) -> bool {
        !self.response.content.is_empty()
    }

    /// Feeds one event. Returns the events worth showing, or the stream's error.
    pub fn feed(&mut self, event: &str, data: &str) -> Result<Vec<StreamEvent>, StreamError> {
        let protocol = |m: String| StreamError { kind: "protocol_error".into(), message: m };
        if event == "ping" {
            return Ok(vec![]);
        }
        let v: Value = serde_json::from_str(data).map_err(|e| protocol(format!("bad event data: {e}")))?;
        let kind = v.get("type").and_then(Value::as_str).unwrap_or(event);
        let mut out = Vec::new();
        match kind {
            "message_start" => {
                let m = &v["message"];
                self.response.id = m["id"].as_str().unwrap_or_default().to_owned();
                self.response.model = m["model"].as_str().unwrap_or_default().to_owned();
                self.response.usage.merge(&m["usage"]);
            }
            "content_block_start" => {
                let index = v["index"].as_u64().ok_or_else(|| protocol("block without index".into()))? as usize;
                let mut block = v["content_block"].clone();
                match block_type(&block) {
                    "tool_use" => {
                        out.push(StreamEvent::ToolUseStarted {
                            index,
                            id: block["id"].as_str().unwrap_or_default().to_owned(),
                            name: block["name"].as_str().unwrap_or_default().to_owned(),
                        });
                        self.partial.push((index, String::new()));
                    }
                    "fallback" => out.push(StreamEvent::Fallback {
                        from: block["from"]["model"].as_str().unwrap_or_default().to_owned(),
                        to: block["to"]["model"].as_str().unwrap_or_default().to_owned(),
                    }),
                    "text" => {
                        if let Some(t) = block["text"].as_str().filter(|t| !t.is_empty()) {
                            out.push(StreamEvent::TextDelta(t.to_owned()));
                        }
                    }
                    _ => {}
                }
                if block_type(&block) == "tool_use" {
                    block["input"] = json!({});
                }
                if self.response.content.len() <= index {
                    self.response.content.resize(index + 1, Value::Null);
                }
                self.response.content[index] = block;
            }
            "content_block_delta" => {
                let index = v["index"].as_u64().ok_or_else(|| protocol("delta without index".into()))? as usize;
                let delta = &v["delta"];
                let block = self
                    .response
                    .content
                    .get_mut(index)
                    .ok_or_else(|| protocol(format!("delta for unknown block {index}")))?;
                match delta["type"].as_str().unwrap_or_default() {
                    "text_delta" => {
                        let t = delta["text"].as_str().unwrap_or_default();
                        append(block, "text", t);
                        out.push(StreamEvent::TextDelta(t.to_owned()));
                    }
                    "input_json_delta" => {
                        let t = delta["partial_json"].as_str().unwrap_or_default();
                        if let Some((_, buf)) = self.partial.iter_mut().find(|(i, _)| *i == index) {
                            buf.push_str(t);
                            out.push(StreamEvent::ToolInputDelta { index, bytes: buf.len() });
                        }
                    }
                    "thinking_delta" => append(block, "thinking", delta["thinking"].as_str().unwrap_or_default()),
                    "signature_delta" => {
                        block["signature"] = Value::String(delta["signature"].as_str().unwrap_or_default().to_owned())
                    }
                    "citations_delta" => {
                        if let Some(list) = block.get_mut("citations").and_then(Value::as_array_mut) {
                            list.push(delta["citation"].clone());
                        } else {
                            block["citations"] = json!([delta["citation"].clone()]);
                        }
                    }
                    other => log::debug!("ignoring delta type {other}"),
                }
            }
            "content_block_stop" => {
                let index = v["index"].as_u64().unwrap_or(u64::MAX) as usize;
                if let Some(pos) = self.partial.iter().position(|(i, _)| *i == index) {
                    let (_, raw) = self.partial.remove(pos);
                    // Eager input streaming means the server no longer checks the input:
                    // parse strictly here, and keep the raw text when it is not JSON.
                    let parsed =
                        if raw.trim().is_empty() { Ok(json!({})) } else { serde_json::from_str::<Value>(&raw) };
                    match parsed {
                        Ok(input) => self.response.content[index]["input"] = input,
                        Err(_) => self.response.invalid_inputs.push((index, raw)),
                    }
                }
            }
            "message_delta" => {
                let d = &v["delta"];
                if let Some(r) = d["stop_reason"].as_str() {
                    self.response.stop_reason = Some(r.to_owned());
                }
                if !d["stop_details"].is_null() {
                    self.response.stop_details = Some(d["stop_details"].clone());
                }
                self.response.usage.merge(&v["usage"]);
            }
            "message_stop" => self.done = true,
            "error" => {
                return Err(StreamError {
                    kind: v["error"]["type"].as_str().unwrap_or("error").to_owned(),
                    message: v["error"]["message"].as_str().unwrap_or_default().to_owned(),
                });
            }
            other => log::debug!("ignoring stream event {other}"),
        }
        Ok(out)
    }

    /// The response so far. Blocks that never started (gaps) are dropped.
    pub fn finish(mut self) -> ProviderResponse {
        self.response.content.retain(|b| !b.is_null());
        self.response
    }
}

fn append(block: &mut Value, field: &str, text: &str) {
    let mut s = block[field].as_str().unwrap_or_default().to_owned();
    s.push_str(text);
    block[field] = Value::String(s);
}

/// The content to echo back after a response. After a mid-output fallback, only text
/// blocks from before the last `fallback` marker may go back: the declined model's
/// thinking and tool calls are not part of the conversation the fallback model continued.
pub fn echo_content(content: &[Value]) -> Vec<Value> {
    let boundary = content.iter().rposition(|b| block_type(b) == "fallback");
    content
        .iter()
        .enumerate()
        .filter(|(i, b)| match boundary {
            Some(at) if *i < at => block_type(b) == "text",
            _ => true,
        })
        .map(|(_, b)| b.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(acc: &mut Accumulator, events: &[(&str, Value)]) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        for (e, d) in events {
            out.extend(acc.feed(e, &d.to_string()).unwrap());
        }
        out
    }

    #[test]
    fn accumulates_thinking_text_and_tool_use() {
        let mut acc = Accumulator::default();
        let events = feed_all(
            &mut acc,
            &[
                (
                    "message_start",
                    json!({"type":"message_start","message":{"id":"msg_1","model":"claude-opus-5-5","usage":{"input_tokens":10,"cache_read_input_tokens":900}}}),
                ),
                (
                    "content_block_start",
                    json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
                ),
                (
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":""}}),
                ),
                (
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}}),
                ),
                ("content_block_stop", json!({"type":"content_block_stop","index":0})),
                (
                    "content_block_start",
                    json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
                ),
                (
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Adding "}}),
                ),
                (
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"it."}}),
                ),
                ("content_block_stop", json!({"type":"content_block_stop","index":1})),
                (
                    "content_block_start",
                    json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"propose","input":{}}}),
                ),
                (
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"summary\": \"R8"}}),
                ),
                (
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":" fillet\"}"}}),
                ),
                ("content_block_stop", json!({"type":"content_block_stop","index":2})),
                (
                    "message_delta",
                    json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":42}}),
                ),
                ("message_stop", json!({"type":"message_stop"})),
            ],
        );
        assert!(acc.done());
        let r = acc.finish();
        assert_eq!(r.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(r.usage.output_tokens, 42);
        assert_eq!(r.usage.cache_read_input_tokens, 900);
        assert_eq!(r.content[0], json!({"type":"thinking","thinking":"","signature":"sig"}));
        assert_eq!(r.content[1]["text"], "Adding it.");
        assert_eq!(r.content[2]["input"], json!({"summary": "R8 fillet"}));
        assert!(events.contains(&StreamEvent::TextDelta("it.".into())));
        assert!(matches!(&events[2], StreamEvent::ToolUseStarted { name, .. } if name == "propose"));
    }

    #[test]
    fn invalid_tool_json_is_kept_raw() {
        let mut acc = Accumulator::default();
        feed_all(
            &mut acc,
            &[
                (
                    "content_block_start",
                    json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"x","input":{}}}),
                ),
                (
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"a\": \"b"}}),
                ),
                ("content_block_stop", json!({"type":"content_block_stop","index":0})),
            ],
        );
        let r = acc.finish();
        assert_eq!(r.invalid_inputs, vec![(0, "{\"a\": \"b".to_owned())]);
        assert_eq!(r.content[0]["input"], json!({}));
    }

    #[test]
    fn stream_error_event_surfaces() {
        let mut acc = Accumulator::default();
        let e = acc.feed("error", r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#);
        assert_eq!(e.unwrap_err().kind, "overloaded_error");
    }

    #[test]
    fn echo_drops_declined_blocks_before_fallback() {
        let content = vec![
            json!({"type":"thinking","thinking":"","signature":"s"}),
            json!({"type":"text","text":"partial"}),
            json!({"type":"tool_use","id":"a","name":"x","input":{}}),
            json!({"type":"fallback","from":{"model":"claude-opus-5-5"},"to":{"model":"claude-opus-5"}}),
            json!({"type":"text","text":"rest"}),
        ];
        let echoed = echo_content(&content);
        let types: Vec<&str> = echoed.iter().map(block_type).collect();
        assert_eq!(types, ["text", "fallback", "text"]);
    }

    #[test]
    fn request_body_follows_the_api_rules() {
        let req = ChatRequest {
            system: "sys".into(),
            tools: vec![ToolSpec {
                name: "t".into(),
                description: "d".into(),
                input_schema: json!({"type":"object","properties":{},"required":[],"additionalProperties":false}),
            }],
            messages: vec![Message::user_text(&["hi"])],
            effort: Effort::Medium,
            max_tokens: 64000,
        };
        let b = request_body("claude-opus-5-5", &req, true);
        assert_eq!(b["model"], "claude-opus-5-5");
        assert!(b.get("thinking").is_none());
        assert!(b.get("tool_choice").is_none());
        assert_eq!(b["output_config"]["effort"], "medium");
        assert_eq!(b["tools"][0]["strict"], true);
        assert_eq!(b["tools"][0]["eager_input_streaming"], true);
        assert_eq!(b["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(b["fallbacks"], "default");
        assert_eq!(b["stream"], true);
    }
}
