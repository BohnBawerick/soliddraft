//! The agent loop: one user message in, streamed text and tool calls out, until the
//! model stops asking for tools.
//!
//! Requests run on a worker thread; the caller polls for events, so an immediate-mode
//! interface can call [`Conversation::poll`] once per frame and run the tools on its own
//! thread, where the document lives. [`Conversation::wait`] blocks instead, for the
//! command line and tests.
//!
//! The history only grows. The system prompt and tools never change, so they stay
//! cached, and thinking blocks go back exactly as they came, which the model requires.
//! The one exception is a turn that does not complete (an error, a refusal or a stop):
//! it is removed whole, which leaves every earlier message as it was.

use crate::provider::{Delivery, Provider, ProviderError, ProviderResponse, StreamEvent};
use crate::tools::{Caller, ToolHost, ToolOutput, ToolSpec, call_checked};
use crate::wire::{ChatRequest, Effort, Message, Role, block_type, echo_content, text_block, tool_result_block};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// Settings fixed for a conversation.
#[derive(Clone, Debug)]
pub struct ConversationConfig {
    /// The system prompt.
    pub system: String,
    /// The effort level.
    pub effort: Effort,
    /// Output limit per request, thinking included.
    pub max_tokens: u32,
    /// Most requests one user message may cause, tool round trips included.
    pub max_requests_per_turn: usize,
}

impl ConversationConfig {
    /// The defaults: medium effort, 64k output tokens, 24 requests per turn.
    pub fn new(system: impl Into<String>) -> Self {
        Self { system: system.into(), effort: Effort::Medium, max_tokens: 64_000, max_requests_per_turn: 24 }
    }
}

/// What happened, for the interface.
#[derive(Clone, Debug, PartialEq)]
pub enum ChatEvent {
    /// A request went out.
    Requesting,
    /// More reply text.
    Text(String),
    /// The model started a tool call.
    ToolStarted {
        /// The tool-use id.
        id: String,
        /// The tool.
        name: String,
    },
    /// More of a tool call's input streamed in.
    ToolInput {
        /// The tool-use id.
        id: String,
        /// Bytes so far.
        bytes: usize,
    },
    /// A tool ran.
    ToolDone {
        /// The tool-use id.
        id: String,
        /// The tool.
        name: String,
        /// Its input.
        input: Value,
        /// What it returned.
        output: ToolOutput,
    },
    /// A refusal moved the reply to another model.
    Fallback {
        /// The model that declined.
        from: String,
        /// The model that continued.
        to: String,
    },
    /// The turn is complete.
    Finished {
        /// The model that wrote the last reply.
        model: String,
    },
    /// The model declined; the turn was removed.
    Refused {
        /// The refusal category, when the API gave one.
        category: Option<String>,
    },
    /// The turn failed and was removed.
    Failed(String),
}

enum WorkerMsg {
    Event(StreamEvent),
    Done(Result<ProviderResponse, ProviderError>),
}

struct Pending {
    rx: Receiver<WorkerMsg>,
    cancel: Arc<AtomicBool>,
    /// Tool-use ids by block index, for the streaming events.
    ids: Vec<(usize, String)>,
}

/// A conversation with a model over a fixed set of tools.
pub struct Conversation {
    provider: Arc<dyn Provider>,
    config: ConversationConfig,
    tools: Vec<ToolSpec>,
    history: Vec<Message>,
    turn_start: usize,
    prompt: String,
    requests_this_turn: usize,
    pending: Option<Pending>,
    waker: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Conversation {
    /// A new conversation. `tools` is fixed for its lifetime.
    pub fn new(provider: Arc<dyn Provider>, tools: Vec<ToolSpec>, config: ConversationConfig) -> Self {
        Self {
            provider,
            config,
            tools,
            history: Vec::new(),
            turn_start: 0,
            prompt: String::new(),
            requests_this_turn: 0,
            pending: None,
            waker: None,
        }
    }

    /// Called from the worker thread whenever an event is ready, so an idle interface
    /// can wake up and poll.
    pub fn set_waker(&mut self, waker: Arc<dyn Fn() + Send + Sync>) {
        self.waker = Some(waker);
    }

    /// The model id requests use.
    pub fn model(&self) -> String {
        self.provider.model()
    }

    /// The provider's name.
    pub fn provider_name(&self) -> String {
        self.provider.name()
    }

    /// The messages so far.
    pub fn history(&self) -> &[Message] {
        &self.history
    }

    /// Whether a turn is running.
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Starts a turn. `context` (the model summary, the selection) goes before the
    /// user's text in the same message, after the cached prefix.
    pub fn send(&mut self, text: &str, context: Option<&str>) -> Result<(), String> {
        if self.busy() {
            return Err("the assistant is still answering".into());
        }
        self.turn_start = self.history.len();
        self.prompt = text.to_owned();
        self.requests_this_turn = 0;
        let mut blocks = Vec::new();
        if let Some(c) = context {
            blocks.push(text_block(c));
        }
        blocks.push(text_block(text));
        self.push_user(blocks);
        self.request();
        Ok(())
    }

    /// Stops the running turn at once and removes it from the history. The worker
    /// thread notices on its next event and ends; nothing it sends is read. Returns
    /// whether a turn was running.
    pub fn cancel(&mut self) -> bool {
        match self.pending.take() {
            Some(p) => {
                p.cancel.store(true, Ordering::SeqCst);
                self.history.truncate(self.turn_start);
                true
            }
            None => false,
        }
    }

    /// Takes whatever happened since the last call, without blocking. Tool calls run
    /// here, on the caller's thread.
    pub fn poll(&mut self, host: &mut dyn ToolHost) -> Vec<ChatEvent> {
        let mut out = Vec::new();
        while let Some(p) = &self.pending {
            match p.rx.try_recv() {
                Ok(msg) => self.handle(msg, host, &mut out),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.fail("the assistant worker stopped unexpectedly".into(), &mut out);
                }
            }
        }
        out
    }

    /// Runs the current turn to its end and returns everything that happened.
    pub fn wait(&mut self, host: &mut dyn ToolHost) -> Vec<ChatEvent> {
        let mut out = Vec::new();
        while let Some(p) = &self.pending {
            match p.rx.recv() {
                Ok(msg) => self.handle(msg, host, &mut out),
                Err(_) => self.fail("the assistant worker stopped unexpectedly".into(), &mut out),
            }
        }
        out
    }

    fn push_user(&mut self, blocks: Vec<Value>) {
        match self.history.last_mut() {
            // Two user messages in a row would be merged by the API anyway; merging here
            // keeps the history in the shape the API echoes.
            Some(m) if m.role == Role::User => m.content.extend(blocks),
            _ => self.history.push(Message { role: Role::User, content: blocks }),
        }
    }

    fn request(&mut self) {
        self.requests_this_turn += 1;
        let request = ChatRequest {
            system: self.config.system.clone(),
            tools: self.tools.clone(),
            messages: self.history.clone(),
            effort: self.config.effort,
            max_tokens: self.config.max_tokens,
        };
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let wake = {
            let waker = self.waker.clone();
            move || {
                if let Some(w) = &waker {
                    w();
                }
            }
        };
        let (done_tx, done_wake) = (tx.clone(), wake.clone());
        self.provider.clone().start(
            request,
            Delivery {
                on_event: Box::new(move |e| {
                    let open = tx.send(WorkerMsg::Event(e)).is_ok();
                    wake();
                    open && !stop.load(Ordering::SeqCst)
                }),
                on_done: Box::new(move |result| {
                    let _ = done_tx.send(WorkerMsg::Done(result));
                    done_wake();
                }),
            },
        );
        self.pending = Some(Pending { rx, cancel, ids: Vec::new() });
    }

    fn handle(&mut self, msg: WorkerMsg, host: &mut dyn ToolHost, out: &mut Vec<ChatEvent>) {
        match msg {
            WorkerMsg::Event(e) => {
                let Some(p) = &mut self.pending else { return };
                match e {
                    StreamEvent::TextDelta(t) => out.push(ChatEvent::Text(t)),
                    StreamEvent::ToolUseStarted { index, id, name } => {
                        p.ids.push((index, id.clone()));
                        out.push(ChatEvent::ToolStarted { id, name });
                    }
                    StreamEvent::ToolInputDelta { index, bytes } => {
                        if let Some((_, id)) = p.ids.iter().find(|(i, _)| *i == index) {
                            out.push(ChatEvent::ToolInput { id: id.clone(), bytes });
                        }
                    }
                    StreamEvent::Fallback { from, to } => out.push(ChatEvent::Fallback { from, to }),
                }
            }
            WorkerMsg::Done(Ok(resp)) => {
                self.pending = None;
                self.respond(resp, host, out);
            }
            WorkerMsg::Done(Err(ProviderError::Cancelled)) => self.fail("stopped".into(), out),
            WorkerMsg::Done(Err(e)) => self.fail(e.to_string(), out),
        }
    }

    fn fail(&mut self, reason: String, out: &mut Vec<ChatEvent>) {
        self.pending = None;
        self.history.truncate(self.turn_start);
        out.push(ChatEvent::Failed(reason));
    }

    fn respond(&mut self, resp: ProviderResponse, host: &mut dyn ToolHost, out: &mut Vec<ChatEvent>) {
        let stop = resp.stop_reason.clone().unwrap_or_default();
        if stop == "refusal" {
            // A refusal may cut a tool call off mid-input: run nothing, keep nothing.
            self.history.truncate(self.turn_start);
            let category = resp.stop_details.as_ref().and_then(|d| d["category"].as_str()).map(str::to_owned);
            out.push(ChatEvent::Refused { category });
            return;
        }
        let invalid: Vec<(String, String)> = resp
            .invalid_inputs
            .iter()
            .filter_map(|(i, raw)| Some((resp.content.get(*i)?["id"].as_str()?.to_owned(), raw.clone())))
            .collect();
        let content = echo_content(&resp.content);
        let calls: Vec<(String, String, Value)> = content
            .iter()
            .filter(|b| block_type(b) == "tool_use")
            .map(|b| {
                (
                    b["id"].as_str().unwrap_or_default().to_owned(),
                    b["name"].as_str().unwrap_or_default().to_owned(),
                    b["input"].clone(),
                )
            })
            .collect();
        self.history.push(Message { role: Role::Assistant, content });
        if stop == "pause_turn" {
            self.request();
            return;
        }
        if calls.is_empty() {
            if stop == "max_tokens" {
                out.push(ChatEvent::Failed("the reply reached the output limit and was cut short".into()));
            }
            out.push(ChatEvent::Finished { model: resp.model });
            return;
        }
        let caller = Caller::Assistant { prompt: self.prompt.clone(), model: resp.model.clone() };
        let mut results = Vec::new();
        for (id, name, input) in calls {
            let output = if stop == "max_tokens" {
                ToolOutput::error(
                    "the input was cut off at the output limit, so the call did not run; send a shorter call",
                )
            } else if let Some((_, raw)) = invalid.iter().find(|(i, _)| *i == id) {
                ToolOutput::error(json!({ "INVALID_JSON": raw }).to_string())
            } else {
                call_checked(host, &name, input.clone(), &caller)
            };
            results.push(tool_result_block(&id, &output.content, output.is_error));
            out.push(ChatEvent::ToolDone { id, name, input, output });
        }
        self.push_user(results);
        if self.requests_this_turn >= self.config.max_requests_per_turn {
            // The history ends with the tool results; the next message joins them.
            out.push(ChatEvent::Failed(format!(
                "stopped after {} requests in one turn",
                self.config.max_requests_per_turn
            )));
            out.push(ChatEvent::Finished { model: resp.model });
            return;
        }
        out.push(ChatEvent::Requesting);
        self.request();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{MockProvider, MockTurn};
    use crate::tools::schema::*;

    struct Counter {
        calls: Vec<(String, Value, Caller)>,
    }

    impl ToolHost for Counter {
        fn tools(&self) -> Vec<ToolSpec> {
            vec![ToolSpec {
                name: "add".into(),
                description: "Adds".into(),
                input_schema: object(&[("n", integer("how many"))]),
            }]
        }

        fn call(&mut self, name: &str, input: Value, caller: &Caller) -> ToolOutput {
            self.calls.push((name.into(), input.clone(), caller.clone()));
            ToolOutput::json(&json!({ "total": input["n"] }))
        }
    }

    fn conversation(turns: Vec<MockTurn>) -> (Conversation, Arc<MockProvider>, Counter) {
        let provider = Arc::new(MockProvider::new(turns));
        let host = Counter { calls: vec![] };
        let c = Conversation::new(provider.clone(), host.tools(), ConversationConfig::new("You count."));
        (c, provider, host)
    }

    #[test]
    fn tool_loop_runs_tools_and_echoes_history() {
        let (mut c, provider, mut host) =
            conversation(vec![MockTurn::tools("Adding.", vec![("add", json!({"n": 2}))]), MockTurn::text("Done: 2.")]);
        c.send("add two", Some("<model_summary>empty</model_summary>")).unwrap();
        let events = c.wait(&mut host);
        assert!(matches!(events.last(), Some(ChatEvent::Finished { .. })), "{events:?}");
        let text: String =
            events.iter().filter_map(|e| if let ChatEvent::Text(t) = e { Some(t.as_str()) } else { None }).collect();
        assert_eq!(text, "Adding.Done: 2.");
        assert_eq!(host.calls.len(), 1);
        assert_eq!(host.calls[0].2, Caller::Assistant { prompt: "add two".into(), model: crate::DEFAULT_MODEL.into() });
        // user, assistant (thinking, text, tool_use), user (tool_result), assistant.
        let h = c.history();
        assert_eq!(h.len(), 4);
        assert_eq!(h[0].content.len(), 2, "context then text");
        assert_eq!(block_type(&h[1].content[0]), "thinking");
        assert_eq!(h[1].content[0]["signature"], "mock-signature-1");
        assert_eq!(block_type(&h[2].content[0]), "tool_result");
        // The second request echoed the first reply byte for byte.
        let second = &provider.requests()[1];
        assert_eq!(second.messages[1], h[1]);
        assert_eq!(second.system, "You count.");
    }

    #[test]
    fn invalid_input_is_reported_not_run() {
        let (mut c, _, mut host) =
            conversation(vec![MockTurn::tools("", vec![("add", json!({"n": "two"}))]), MockTurn::text("Sorry.")]);
        c.send("add", None).unwrap();
        let events = c.wait(&mut host);
        assert!(host.calls.is_empty());
        let out =
            events.iter().find_map(|e| if let ChatEvent::ToolDone { output, .. } = e { Some(output) } else { None });
        assert!(out.unwrap().is_error);
    }

    #[test]
    fn failed_turn_is_removed_whole() {
        let (mut c, _, mut host) = conversation(vec![
            MockTurn::text("first"),
            MockTurn::tools("", vec![("add", json!({"n": 1}))]),
            MockTurn::Error { status: 400, kind: "invalid_request_error".into() },
        ]);
        c.send("one", None).unwrap();
        c.wait(&mut host);
        assert_eq!(c.history().len(), 2);
        c.send("two", None).unwrap();
        let events = c.wait(&mut host);
        assert!(matches!(events.last(), Some(ChatEvent::Failed(_))), "{events:?}");
        assert_eq!(c.history().len(), 2, "the failed turn left nothing behind");
    }

    #[test]
    fn cancel_ends_the_turn_at_once() {
        let (mut c, _, mut host) = conversation(vec![MockTurn::text("first")]);
        c.send("one", None).unwrap();
        assert!(c.cancel());
        assert!(!c.busy());
        assert!(c.history().is_empty());
        assert!(c.poll(&mut host).is_empty());
        assert!(!c.cancel());
    }

    #[test]
    fn refusal_runs_nothing() {
        let refusal = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"model\":\"claude-opus-5-5\",\"usage\":{}}}\n\n\
event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"refusal\",\"stop_details\":{\"type\":\"refusal\",\"category\":\"cyber\"}},\"usage\":{\"output_tokens\":0}}\n\n\
event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
        let (mut c, _, mut host) = conversation(vec![MockTurn::Recorded(refusal.into())]);
        c.send("x", None).unwrap();
        let events = c.wait(&mut host);
        assert_eq!(events.last(), Some(&ChatEvent::Refused { category: Some("cyber".into()) }));
        assert!(c.history().is_empty());
    }
}
