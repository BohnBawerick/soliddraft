//! The assistant session behind the session pane at the bottom (`command_line.rs`): the
//! conversation with the model, the proposals, and the MCP endpoint, all driven once per frame on the interface thread,
//! where the document lives.

use crate::app_tools::AppTools;
use crate::command_tools::{CommandTools, ProposalBook, ProposalStatus, Surface};
use crate::document::Preview;
use crate::state::{AppState, NoteKind};
use soliddraft_assistant::key::{self, ApiKey, KeySource};
use soliddraft_assistant::mcp::{HttpCall, HttpServer, McpSession};
use soliddraft_assistant::{AssistantSettings, ChatEvent, Conversation, ConversationConfig, Provider, ProviderKind};
use std::sync::Arc;
use std::sync::mpsc::Receiver;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    /// A line typed in the session that ran as an app command. Shown, never sent.
    Command,
}

/// What a tool call is doing, for the line under the assistant's text.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolState {
    /// Streaming its input: bytes so far.
    Running(usize),
    /// Finished with a short result.
    Done(String),
    /// Failed with the reason.
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolActivity {
    pub id: String,
    pub name: String,
    pub state: ToolState,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatMessage {
    pub role: Role,
    pub text: String,
    /// Ids of the proposals this message made.
    pub proposals: Vec<String>,
    pub tools: Vec<ToolActivity>,
    /// Still streaming.
    pub streaming: bool,
}

impl ChatMessage {
    pub fn new(role: Role, text: impl Into<String>) -> Self {
        Self { role, text: text.into(), proposals: Vec::new(), tools: Vec::new(), streaming: false }
    }
}

/// Whether the model is reachable.
pub enum Link {
    Connected(Box<Conversation>),
    /// No model; the reason says how to connect one.
    Offline(String),
}

/// The MCP endpoint of the running app.
pub struct McpEndpoint {
    pub server: HttpServer,
    calls: Receiver<HttpCall>,
    session: McpSession,
}

pub struct ChatState {
    pub messages: Vec<ChatMessage>,
    pub draft: String,
    /// The proposal being previewed, with the part as it would be after it.
    pub preview: Option<(String, Preview)>,
    pub book: ProposalBook,
    pub link: Link,
    /// One line on the connection, for the session's header.
    pub status: String,
    pub mcp: Option<McpEndpoint>,
    /// The assistant message being streamed into.
    streaming: Option<usize>,
    /// Wakes the interface from the worker threads; kept so a new connection gets it too.
    waker: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// The assistant settings dialog (Assistant tab, Provider): the provider, the models and
/// the API keys. The provider and models are saved on Save and reconnect; a key goes to the
/// OS keychain as soon as it is saved.
pub struct ProviderDialog {
    /// `None` picks the provider that has a key.
    pub provider: Option<ProviderKind>,
    /// The model typed for each provider; empty means its default.
    pub models: Vec<(ProviderKind, String)>,
    /// One row per provider's key.
    pub keys: Vec<KeyRow>,
}

/// One provider's API key in the settings dialog.
pub struct KeyRow {
    pub kind: ProviderKind,
    /// Where the key comes from and its masked form; `None` when there is no key.
    pub found: Option<(KeySource, String)>,
    /// Whether the OS keychain holds a key, which an environment key may hide.
    pub in_keychain: bool,
    /// A new key being typed. Cleared once it is stored.
    pub entry: String,
}

impl KeyRow {
    /// The row for a provider as its key stands now.
    pub fn read(kind: ProviderKind) -> Self {
        let found = key::find(kind).map(|(k, source)| (source, k.masked()));
        let in_keychain = match &found {
            Some((KeySource::Keychain(_), _)) => true,
            Some((KeySource::Environment(_), _)) => key::in_keychain(kind),
            None => false,
        };
        Self { kind, found, in_keychain, entry: String::new() }
    }
}

impl ProviderDialog {
    pub fn new(settings: &AssistantSettings) -> Self {
        Self {
            provider: settings.provider,
            models: ProviderKind::ALL
                .into_iter()
                .map(|k| (k, settings.model(k).unwrap_or_default().to_owned()))
                .collect(),
            keys: ProviderKind::ALL.into_iter().map(KeyRow::read).collect(),
        }
    }

    /// The settings as edited.
    pub fn settings(&self) -> AssistantSettings {
        let mut s = AssistantSettings { provider: self.provider, ..Default::default() };
        for (kind, model) in &self.models {
            s.set_model(*kind, model);
        }
        s
    }
}

/// The system prompt. It never changes during a session, so it stays cached.
pub const SYSTEM_PROMPT: &str = "\
You are the modelling assistant inside SolidDraft, a parametric CAD application for mechanical parts. \
You work beside the user: they have a feature tree on the left, a 3D viewport in the middle and you in a session \
pane along the bottom, where they also type app commands.

How you change the part:
- You never change the part directly. To change it, call `propose` with recipe lines and a one-line summary. \
The user sees the proposal as a card with Preview, Apply and Discard. Make one proposal per request unless the \
user asks for alternatives.
- Do not call `apply` unless the model summary says auto_apply is true. When it is false, end your turn after \
proposing and tell the user, in one or two sentences, what the proposal does and why.
- If `propose` rejects the lines, read the reason, fix the lines and propose again. Do not guess names: read them \
from the model summary or the `feature` tool.

The part:
- Units are millimetres. Z is up. The Top plane is XY (normal +Z), the Front plane is XZ with its normal toward \
the viewer (-Y), the Right plane is YZ (normal +X). The part is one body built feature by feature, in tree order.
- Every message from the user starts with a <model_summary> block: the part as it was when they wrote. After \
something is applied, call `model_summary` again if you need the current state.
- Entities are named as the interface shows them: a feature by its tree name (Boss-Extrude1), a face or edge as \
`<feature> · <label>` (Boss-Extrude1 · End cap, Boss-Extrude2 · Edge 5). `feature` lists the faces and edges a \
feature made, with their positions; it marks an edge where two faces meet in an inside corner as concave. \
Face and edge labels can change when the part changes, so read them again after anything is applied.

Building geometry:
- New geometry is a `sketch` line followed by an `extrude` (adds material) or `cut` (removes material) line whose \
sketch is `previous`. Sketch points are world coordinates on the sketch plane: on a face, take a point with the \
face's coordinate across it (a face at z = 10 needs points with z = 10).
- Several holes of one size are one sketch with several circles and one cut. A slot is one `slot` shape. A hole \
through the whole part is a cut with through_all, which also cuts anything behind the sketch. A hole or \
slot through one wall is a cut as deep as the wall, so it stops there.
- Holes and slots need clearance from edges and other features; check positions against the bounding box and the \
faces before proposing, and say where you placed them.
- A fillet or chamfer names edges. The inside corner is a concave edge; outside edges are convex.

How you answer:
- Be brief and concrete: name the feature, the size and where it goes. Use mm.
- If no available command can make what the user asks for, say so plainly instead of approximating it with \
other features, and say what you can do instead.
- Ask a short question when the request is ambiguous in a way that changes the geometry (which edge, which face, \
which side).";

impl ChatState {
    /// A chat connected as the settings file and the environment say, or offline with the reason.
    pub fn from_environment() -> Self {
        Self::from_settings(&crate::settings::assistant())
    }

    /// A chat connected as `settings` and the environment say, or offline with the reason.
    /// The web build's model and key are on its server, so there the settings do not apply.
    pub fn from_settings(settings: &AssistantSettings) -> Self {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = settings;
            return match crate::web::assistant() {
                Ok((provider, how)) => Self::connected(provider, how),
                Err(reason) => Self::offline(reason),
            };
        }
        #[cfg(not(target_arch = "wasm32"))]
        match soliddraft_assistant::choice::choose(settings) {
            Ok(choice) => {
                let how = choice.source.describe();
                match choice.connect() {
                    Ok(p) => Self::connected(p, &how),
                    Err(e) => Self::offline(format!("The assistant could not start: {e}")),
                }
            }
            Err(why) => Self::offline(why.to_string()),
        }
    }

    /// A chat on a given provider.
    pub fn connected(provider: Arc<dyn Provider>, how: &str) -> Self {
        let status = format!("{} · {} · {how}", provider.name(), provider.model());
        let conversation = Conversation::new(
            provider,
            crate::command_tools::tool_specs(Surface::Assistant),
            ConversationConfig::new(SYSTEM_PROMPT),
        );
        let mut chat = Self::empty(Link::Connected(Box::new(conversation)), status);
        chat.messages.push(ChatMessage::new(
            Role::System,
            "Reads the model and proposes features. Nothing changes until you apply.",
        ));
        chat
    }

    /// A chat with no model.
    pub fn offline(reason: String) -> Self {
        let mut chat = Self::empty(Link::Offline(reason.clone()), "Not connected".into());
        chat.messages.push(ChatMessage::new(Role::System, reason));
        chat
    }

    fn empty(link: Link, status: String) -> Self {
        Self {
            messages: Vec::new(),
            draft: String::new(),
            preview: None,
            book: ProposalBook::default(),
            link,
            status,
            mcp: None,
            streaming: None,
            waker: None,
        }
    }

    /// Moves to the connection in `fresh`, keeping the messages, the proposals and the MCP
    /// endpoint. The new conversation starts without the old one's history.
    pub fn relink(&mut self, fresh: ChatState) {
        self.end_stream();
        let ChatState { link, status, mut messages, .. } = fresh;
        self.link = link;
        self.status = status;
        if let Some(mcp) = &self.mcp {
            self.status.push_str(&format!(" · MCP {}", mcp.server.url()));
        }
        if let Some(waker) = self.waker.clone() {
            self.set_waker(waker);
        }
        let note = match &self.link {
            Link::Connected(_) => format!("Connected to {}.", self.status),
            Link::Offline(_) => messages.pop().map(|m| m.text).unwrap_or_default(),
        };
        self.messages.push(ChatMessage::new(Role::System, note));
    }

    /// Whether the model is answering.
    pub fn busy(&self) -> bool {
        matches!(&self.link, Link::Connected(c) if c.busy())
    }

    /// Lets the worker threads wake an idle interface.
    pub fn set_waker(&mut self, waker: Arc<dyn Fn() + Send + Sync>) {
        if let Link::Connected(c) = &mut self.link {
            c.set_waker(waker.clone());
        }
        self.waker = Some(waker);
    }

    fn current(&mut self) -> &mut ChatMessage {
        let i = match self.streaming {
            Some(i) => i,
            None => {
                let mut m = ChatMessage::new(Role::Assistant, "");
                m.streaming = true;
                self.messages.push(m);
                self.messages.len() - 1
            }
        };
        self.streaming = Some(i);
        &mut self.messages[i]
    }

    fn end_stream(&mut self) {
        if let Some(i) = self.streaming.take() {
            self.messages[i].streaming = false;
            let m = &self.messages[i];
            if m.text.is_empty() && m.tools.is_empty() && m.proposals.is_empty() {
                self.messages.remove(i);
            }
        }
    }
}

fn short(output: &str) -> String {
    let line = output.lines().next().unwrap_or_default();
    if line.chars().count() > 120 {
        format!("{}…", line.chars().take(120).collect::<String>())
    } else {
        line.to_owned()
    }
}

impl AppState {
    /// Sends the composer's text to the assistant.
    pub fn send_chat(&mut self) {
        let text = self.chat.draft.trim().to_owned();
        if text.is_empty() {
            return;
        }
        if let Link::Offline(reason) = &self.chat.link {
            let reason = reason.clone();
            self.chat.draft.clear();
            self.chat.messages.push(ChatMessage::new(Role::User, text));
            self.chat.messages.push(ChatMessage::new(Role::System, reason));
            return;
        }
        let summary =
            CommandTools { doc: &mut self.doc, book: &mut self.chat.book, surface: Surface::Assistant, drawing: None }
                .summary();
        let context = format!("<model_summary>\n{summary}\n</model_summary>");
        let Link::Connected(conversation) = &mut self.chat.link else { return };
        match conversation.send(&text, Some(&context)) {
            Ok(()) => {
                self.chat.draft.clear();
                self.chat.messages.push(ChatMessage::new(Role::User, text));
                self.chat.end_stream();
                self.chat.current();
            }
            Err(e) => self.note(e, NoteKind::Info),
        }
    }

    /// Stops the answer in progress.
    pub fn stop_chat(&mut self) {
        if let Link::Connected(c) = &mut self.chat.link
            && c.cancel()
        {
            self.chat.end_stream();
            self.chat.messages.push(ChatMessage::new(Role::System, "Stopped."));
        }
    }

    /// Takes the assistant's progress since the last frame and runs its tool calls.
    pub fn poll_assistant(&mut self) {
        let crate::chat::ChatState { link, book, .. } = &mut self.chat;
        let Link::Connected(conversation) = link else { return };
        let mut host = CommandTools { doc: &mut self.doc, book, surface: Surface::Assistant, drawing: None };
        let events = conversation.poll(&mut host);
        for e in events {
            self.chat_event(e);
        }
    }

    /// Blocks until the current answer is complete. For tests and scripts.
    pub fn wait_assistant(&mut self) {
        let crate::chat::ChatState { link, book, .. } = &mut self.chat;
        let Link::Connected(conversation) = link else { return };
        let mut host = CommandTools { doc: &mut self.doc, book, surface: Surface::Assistant, drawing: None };
        let events = conversation.wait(&mut host);
        for e in events {
            self.chat_event(e);
        }
    }

    fn chat_event(&mut self, e: ChatEvent) {
        let chat = &mut self.chat;
        match e {
            ChatEvent::Requesting => {
                chat.current();
            }
            ChatEvent::Text(t) => chat.current().text.push_str(&t),
            ChatEvent::ToolStarted { id, name } => {
                chat.current().tools.push(ToolActivity { id, name, state: ToolState::Running(0) })
            }
            ChatEvent::ToolInput { id, bytes } => {
                if let Some(t) = chat.current().tools.iter_mut().find(|t| t.id == id) {
                    t.state = ToolState::Running(bytes);
                }
            }
            ChatEvent::ToolDone { id, name, output, .. } => {
                let parsed: serde_json::Value = serde_json::from_str(&output.content).unwrap_or_default();
                let m = chat.current();
                if let Some(t) = m.tools.iter_mut().find(|t| t.id == id) {
                    t.state = if output.is_error {
                        ToolState::Failed(short(&output.content))
                    } else {
                        ToolState::Done(match name.as_str() {
                            "propose" | "apply" => {
                                format!(
                                    "{} {}",
                                    parsed["proposal_id"].as_str().unwrap_or(""),
                                    parsed["status"].as_str().unwrap_or("")
                                )
                            }
                            _ => "done".into(),
                        })
                    };
                }
                if !output.is_error
                    && (name == "propose" || name == "apply")
                    && let Some(pid) = parsed["proposal_id"].as_str()
                    && !m.proposals.iter().any(|p| p == pid)
                {
                    m.proposals.push(pid.to_owned());
                }
                if name == "apply" || parsed["status"] == "applied" {
                    self.after_change();
                }
            }
            ChatEvent::Fallback { from, to } => {
                chat.messages.push(ChatMessage::new(Role::System, format!("{from} declined; {to} answered instead.")));
            }
            ChatEvent::Finished { .. } => chat.end_stream(),
            ChatEvent::Refused { category } => {
                chat.end_stream();
                let why = category.map(|c| format!(" ({c})")).unwrap_or_default();
                chat.messages.push(ChatMessage::new(Role::System, format!("The model declined this request{why}.")));
            }
            ChatEvent::Failed(reason) => {
                chat.end_stream();
                chat.messages.push(ChatMessage::new(Role::System, format!("The assistant stopped: {reason}")));
            }
        }
    }

    /// Clears what a change to the part made stale.
    fn after_change(&mut self) {
        self.chat.preview = None;
    }

    /// Shows or hides a proposal's ghost geometry.
    pub fn preview_proposal(&mut self, id: &str) {
        if self.chat.preview.as_ref().is_some_and(|(p, _)| p == id) {
            self.chat.preview = None;
            return;
        }
        match self.chat.book.get(id).map(|p| p.preview.clone()) {
            Some(preview) if preview.result.is_some() => self.chat.preview = Some((id.to_owned(), preview)),
            _ => self.note("This proposal leaves no body to preview", NoteKind::Info),
        }
    }

    /// Applies a proposal for the user.
    pub fn apply_proposal(&mut self, id: &str) {
        self.after_change();
        match self.chat.book.apply(&mut self.doc, id) {
            Ok(added) => {
                let names: Vec<String> =
                    added.iter().filter_map(|f| self.doc.feature(*f)).map(|f| f.name.clone()).collect();
                let text = if names.is_empty() {
                    format!("{id} applied")
                } else {
                    format!("{} applied from the assistant", names.join(", "))
                };
                self.note(text, NoteKind::Ok);
            }
            Err(e) => self.note(e, NoteKind::Error),
        }
    }

    /// Undoes an applied proposal.
    pub fn undo_proposal(&mut self, id: &str) {
        self.after_change();
        match self.chat.book.undo(&mut self.doc, id) {
            Ok(()) => self.note(format!("{id} undone"), NoteKind::Ok),
            Err(e) => self.note(e, NoteKind::Error),
        }
    }

    pub fn discard_proposal(&mut self, id: &str) {
        if self.chat.preview.as_ref().is_some_and(|(p, _)| p == id) {
            self.chat.preview = None;
        }
        if let Err(e) = self.chat.book.discard(id) {
            self.note(e, NoteKind::Error);
        }
    }

    /// Stores the key typed in the settings dialog in the OS keychain.
    pub fn store_key(&mut self, kind: ProviderKind) {
        let Some(row) = self.provider_dialog.as_mut().and_then(|d| d.keys.iter_mut().find(|r| r.kind == kind)) else {
            return;
        };
        let typed = ApiKey::new(std::mem::take(&mut row.entry));
        let result = match &typed {
            Some(k) => key::store_in_keychain(kind, k),
            None => return,
        };
        *row = KeyRow::read(kind);
        let shadowed = matches!(row.found, Some((KeySource::Environment(_), _)));
        match result {
            Ok(()) if shadowed => self.note(
                format!("Saved the {kind} key in the OS keychain; {} still takes precedence", kind.key_var()),
                NoteKind::Info,
            ),
            Ok(()) => self.note(format!("Saved the {kind} key in the OS keychain"), NoteKind::Ok),
            Err(e) => self.note(format!("The keychain refused the {kind} key: {e}"), NoteKind::Error),
        }
    }

    /// Removes a provider's key from the OS keychain.
    pub fn delete_key(&mut self, kind: ProviderKind) {
        let result = key::delete_from_keychain(kind);
        if let Some(row) = self.provider_dialog.as_mut().and_then(|d| d.keys.iter_mut().find(|r| r.kind == kind)) {
            *row = KeyRow::read(kind);
        }
        match result {
            Ok(()) => self.note(format!("Removed the {kind} key from the OS keychain"), NoteKind::Ok),
            Err(e) => self.note(format!("The keychain did not remove the {kind} key: {e}"), NoteKind::Error),
        }
    }

    /// Saves the Provider dialog's settings and reconnects the assistant with them.
    pub fn save_provider_settings(&mut self) {
        let Some(dialog) = self.provider_dialog.take() else { return };
        let settings = dialog.settings();
        if let Err(e) = crate::settings::save_assistant(&settings) {
            self.note(format!("The provider setting was not saved: {e}"), NoteKind::Error);
        }
        self.chat.relink(ChatState::from_settings(&settings));
    }

    pub fn toggle_auto_apply(&mut self) {
        self.chat.book.auto_apply = !self.chat.book.auto_apply;
        let text = if self.chat.book.auto_apply {
            "Auto-apply on: the assistant applies its proposals for this session"
        } else {
            "Auto-apply off: proposals wait for you"
        };
        self.note(text, NoteKind::Info);
    }

    /// Serves MCP on `127.0.0.1:port` (0 picks a port) for this session.
    pub fn start_mcp(&mut self, port: u16, waker: Option<Arc<dyn Fn() + Send + Sync>>) -> Result<String, String> {
        let (server, calls) = HttpServer::start(port, waker).map_err(|e| format!("MCP server: {e}"))?;
        let url = server.url();
        self.chat.mcp = Some(McpEndpoint {
            server,
            calls,
            session: McpSession::new("soliddraft", env!("CARGO_PKG_VERSION"), crate::command_tools::MCP_INSTRUCTIONS),
        });
        Ok(url)
    }

    /// Answers waiting MCP calls against the open part and drawing.
    pub fn service_mcp(&mut self) {
        let Some(mut mcp) = self.chat.mcp.take() else { return };
        let mut changed = false;
        while let Ok(call) = mcp.calls.try_recv() {
            let before = self.doc.mesh_version;
            let reply = mcp.session.handle(&mut AppTools { state: self }, call.message.clone());
            call.respond(reply);
            changed |= self.doc.mesh_version != before;
        }
        self.chat.mcp = Some(mcp);
        if changed {
            self.after_change();
        }
    }

    /// Whether a proposal can be undone right now.
    pub fn can_undo(&self, id: &str) -> bool {
        self.chat.book.can_undo(&self.doc, id)
    }

    /// A proposal's status as the part stands now.
    pub fn proposal_status(&self, id: &str) -> Option<ProposalStatus> {
        self.chat.book.status(&self.doc, id)
    }
}
