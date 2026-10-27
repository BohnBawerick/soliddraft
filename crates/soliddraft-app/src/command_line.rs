//! The command line along the bottom of the window, and the assistant session that opens
//! above it.
//!
//! The bar holds a row of tool chips and a prompt where a typed command runs, as on
//! AutoCAD's command line: a command's id or label runs it, Tab completes the name, Up and
//! Down walk back through what was typed, and Enter on an empty line repeats the last one.
//! The names come from the command registry ([`crate::commands`]), so a typed command runs
//! exactly what its ribbon button runs.
//!
//! [`AI_COMMAND`] opens the assistant session: a terminal-like pane that grows upward from
//! the bar. A line there that is a command's name, or starts with `/`, runs the command;
//! anything else goes to the assistant, which answers and proposes changes as it did in the
//! old chat panel (`chat.rs`). Escape or `exit` collapses it back to the bar.
//!
//! The chips start as the selection tools. Dragging a chip moves it, dragging a ribbon
//! button onto the row adds it, + adds one by name and right-click removes one. The row and
//! the session's height are kept in the settings file.

use crate::chat::{ChatMessage, Role, ToolActivity, ToolState};
use crate::command_tools::{Proposal, ProposalStatus};
use crate::commands::{self, CommandSpec, Context, History};
use crate::icons::{self, Icon};
use crate::state::{AppAction, AppState, NoteKind};
use crate::theme;
use egui::{Align2, Color32, CornerRadius, Key, Modifiers, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde::{Deserialize, Serialize};

/// The typed command that opens the assistant session. The name lives here alone.
pub const AI_COMMAND: &str = "ai";

/// Words that close the session when typed in it, with or without a leading `/`.
pub const CLOSE_WORDS: [&str; 2] = ["exit", "close"];

/// The chips a new bar starts with: the selection tools.
pub const DEFAULT_TOOLS: [&str; 5] = ["sketch_select", "cancel", "measure", "normal_to", "zoom_fit"];

pub const SESSION_MIN_HEIGHT: f32 = 160.0;
pub const SESSION_DEFAULT_HEIGHT: f32 = 320.0;
/// The tallest a saved session height may be; the window caps it further.
const SESSION_MAX_HEIGHT: f32 = 2000.0;
/// How many typed lines Up and Down go back through.
const HISTORY: usize = 100;

/// The egui id of the bar while the session is closed.
pub fn bar_id() -> egui::Id {
    egui::Id::new("command_line")
}

/// The egui id of the panel while the session is open; its height lives under this id.
pub fn session_id() -> egui::Id {
    egui::Id::new("command_session")
}

/// The prompt's text field, in the bar and in the session alike, so focus stays on it
/// when the session opens or closes.
pub fn input_id() -> egui::Id {
    egui::Id::new("command_line_input")
}

/// What the user arranged: the chips and the session's height. The `command_line` key of
/// the settings file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BarSettings {
    /// The chips, by command id, left to right.
    pub tools: Vec<String>,
    /// The session pane's height in logical pixels.
    pub session_height: f32,
}

impl Default for BarSettings {
    fn default() -> Self {
        Self { tools: DEFAULT_TOOLS.map(str::to_owned).to_vec(), session_height: SESSION_DEFAULT_HEIGHT }
    }
}

impl BarSettings {
    /// Drops ids no command of the app has and repeats, and brings the height into range,
    /// so a stale or hand-edited file cannot break the bar.
    pub fn cleaned(mut self) -> Self {
        let mut seen = Vec::new();
        self.tools.retain(|id| {
            let keep = commands::get(id).is_some_and(CommandSpec::in_app) && !seen.contains(id);
            seen.push(id.clone());
            keep
        });
        if !self.session_height.is_finite() {
            self.session_height = SESSION_DEFAULT_HEIGHT;
        }
        self.session_height = self.session_height.clamp(SESSION_MIN_HEIGHT, SESSION_MAX_HEIGHT);
        self
    }

    pub fn position(&self, id: &str) -> Option<usize> {
        self.tools.iter().position(|t| t == id)
    }

    /// Puts a command's chip before the chip at `at`, or last; a chip already on the bar
    /// moves there.
    pub fn place(&mut self, id: &str, at: Option<usize>) {
        let mut at = at.unwrap_or(self.tools.len());
        if let Some(from) = self.position(id) {
            self.tools.remove(from);
            if from < at {
                at -= 1;
            }
        }
        self.tools.insert(at.min(self.tools.len()), id.to_owned());
    }

    pub fn remove(&mut self, id: &str) {
        self.tools.retain(|t| t != id);
    }
}

/// What is dragged onto the chip row: a command, from the ribbon or from the row itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolDrag {
    pub id: &'static str,
}

/// The names Tab offers after one Tab, and the one shown now.
#[derive(Clone, Debug, PartialEq)]
struct Completion {
    names: Vec<&'static str>,
    shown: Option<usize>,
    /// A `/` typed before the name, kept in front of it.
    slash: bool,
}

/// The bar's and the session's state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommandLine {
    /// What is typed at the prompt.
    pub input: String,
    pub session_open: bool,
    /// Put the keyboard on the prompt next frame.
    pub focus_requested: bool,
    pub settings: BarSettings,
    /// Lines typed, the oldest first.
    pub history: Vec<String>,
    /// While Up and Down walk the history: the place, and what was typed before.
    browsing: Option<(usize, String)>,
    completion: Option<Completion>,
    /// Move the prompt's cursor to the end next frame, after the text was replaced.
    cursor_to_end: bool,
    /// The search in the + menu.
    adding: String,
}

impl CommandLine {
    /// The bar as the settings file left it.
    pub fn load() -> Self {
        Self { settings: crate::settings::command_line(), ..Default::default() }
    }

    pub fn open_session(&mut self) {
        self.session_open = true;
        self.focus_requested = true;
    }

    pub fn close_session(&mut self) {
        self.session_open = false;
    }

    pub fn toggle_session(&mut self) {
        if self.session_open { self.close_session() } else { self.open_session() }
    }

    fn remember(&mut self, line: &str) {
        let line = line.trim();
        if !line.is_empty() && self.history.last().map(String::as_str) != Some(line) {
            self.history.push(line.to_owned());
            if self.history.len() > HISTORY {
                self.history.remove(0);
            }
        }
        self.browsing = None;
        self.completion = None;
    }

    /// Up (`back`) and Down at the prompt: the line typed before or after the one shown.
    /// Down past the latest brings back what was being typed.
    pub fn browse(&mut self, back: bool) {
        let len = self.history.len();
        let at = match (&self.browsing, back) {
            (_, _) if len == 0 => return,
            (None, true) => len - 1,
            (None, false) => return,
            (Some((i, _)), true) => i.saturating_sub(1),
            (Some((i, _)), false) if i + 1 < len => i + 1,
            (Some(_), false) => {
                if let Some((_, typed)) = self.browsing.take() {
                    self.input = typed;
                }
                self.cursor_to_end = true;
                return;
            }
        };
        let typed = match self.browsing.take() {
            Some((_, typed)) => typed,
            None => std::mem::take(&mut self.input),
        };
        self.browsing = Some((at, typed));
        self.input = self.history[at].clone();
        self.completion = None;
        self.cursor_to_end = true;
    }

    /// Tab at the prompt. One name that fits completes it; several complete as far as
    /// they agree, and the next Tab steps through them. Only the first word completes.
    pub fn complete(&mut self, context: Context) {
        if let Some(c) = self.completion.as_mut().filter(|c| c.names.len() > 1) {
            let next = c.shown.map_or(0, |i| (i + 1) % c.names.len());
            c.shown = Some(next);
            self.input = format!("{}{}", if c.slash { "/" } else { "" }, c.names[next]);
            self.cursor_to_end = true;
            return;
        }
        if self.input.trim().contains(char::is_whitespace) {
            return;
        }
        let slash = self.input.trim_start().starts_with('/');
        let names = completions(&self.input, context, self.session_open);
        let prefix = if slash { "/" } else { "" };
        match names.as_slice() {
            [] => {}
            [one] => {
                self.input = format!("{prefix}{one}");
                self.completion = None;
            }
            [first, rest @ ..] => {
                let common = rest.iter().fold(first.len(), |n, name| {
                    first.bytes().zip(name.bytes()).take(n).take_while(|(a, b)| a == b).count()
                });
                let typed = self.input.trim().trim_start_matches('/').len();
                if common > typed {
                    self.input = format!("{prefix}{}", &first[..common]);
                }
                self.completion = Some(Completion { names, shown: None, slash });
            }
        }
        self.cursor_to_end = true;
    }
}

/// The command a name stands for: its id, or its label, either case. Where two commands
/// share a label, the one the context offers wins.
pub fn named(name: &str, context: Context) -> Option<&'static CommandSpec> {
    let name = name.trim().to_lowercase();
    let found: Vec<&'static CommandSpec> =
        commands::all().iter().filter(|c| c.in_app() && (c.id == name || c.label.to_lowercase() == name)).collect();
    found.iter().find(|c| c.available(context)).or(found.first()).copied()
}

/// The names Tab offers for the start of one: the ids of the commands the context offers,
/// with the session's own words while it is open, in alphabetical order.
pub fn completions(start: &str, context: Context, session: bool) -> Vec<&'static str> {
    let start = start.trim().trim_start_matches('/').to_lowercase();
    let mut names: Vec<&'static str> =
        commands::all().iter().filter(|c| c.available(context) && c.id.starts_with(&start)).map(|c| c.id).collect();
    if session {
        names.extend(CLOSE_WORDS.iter().filter(|w| w.starts_with(&start)));
    }
    names.sort_unstable();
    names.dedup();
    names
}

/// What a typed line asks for.
#[derive(Debug)]
pub enum Typed<'a> {
    Nothing,
    /// A command, and what was typed after its name.
    Command(&'static CommandSpec, &'a str),
    /// A word that closes the session; only while it is open.
    Close,
    /// No command has that name. The closest commands, by search.
    Unknown(Vec<&'static CommandSpec>),
}

/// Reads a typed line. The whole line is tried as a name first, so a label of two words
/// works, then its first word, with the rest kept for the command.
pub fn resolve(line: &str, context: Context, session: bool) -> Typed<'_> {
    let line = line.trim();
    let bare = line.strip_prefix('/').unwrap_or(line).trim_start();
    if bare.is_empty() {
        return Typed::Nothing;
    }
    if session && CLOSE_WORDS.iter().any(|w| bare.eq_ignore_ascii_case(w)) {
        return Typed::Close;
    }
    if let Some(c) = named(bare, context) {
        return Typed::Command(c, "");
    }
    let (word, rest) = bare.split_once(char::is_whitespace).map_or((bare, ""), |(w, r)| (w, r.trim()));
    if let Some(c) = named(word, context) {
        return Typed::Command(c, rest);
    }
    Typed::Unknown(closest(word, context))
}

/// The commands closest to a name no command has: the ones whose id or label is a few
/// edits away, so a misspelling such as "filet" offers fillet, or else what the search
/// finds for it.
fn closest(word: &str, context: Context) -> Vec<&'static CommandSpec> {
    let word = word.to_lowercase();
    let most = (word.chars().count() / 3).max(1);
    let mut near: Vec<(usize, &'static CommandSpec)> = commands::all()
        .iter()
        .filter(|c| c.available(context))
        .filter_map(|c| {
            let d = edits(&word, c.id).min(edits(&word, &c.label.to_lowercase()));
            (d <= most).then_some((d, c))
        })
        .collect();
    near.sort_by_key(|(d, _)| *d);
    if near.is_empty() {
        return commands::search_here(&word, &History::default(), context)
            .into_iter()
            .take(3)
            .map(|m| m.command)
            .collect();
    }
    near.into_iter().take(3).map(|(_, c)| c).collect()
}

/// How many single-character insertions, deletions or substitutions turn `a` into `b`.
fn edits(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (above + 1).min(row[j] + 1).min(diagonal + usize::from(ca != *cb));
            diagonal = above;
        }
    }
    row[b.len()]
}

fn unknown_text(line: &str, closest: &[&'static CommandSpec]) -> String {
    let word = line.trim().trim_start_matches('/').split_whitespace().next().unwrap_or_default();
    if closest.is_empty() {
        format!("No command is called \"{word}\"")
    } else {
        let names: Vec<&str> = closest.iter().map(|c| c.id).collect();
        format!("No command is called \"{word}\". Closest: {}", names.join(", "))
    }
}

/// Runs a typed command as its ribbon button would, and says what came of it: its label,
/// or why it did not run.
fn run(state: &mut AppState, command: &'static CommandSpec, rest: &str) -> String {
    let label = command.label_in(state);
    if command.action_now(state).is_none() {
        let why = command.hint_now(state).unwrap_or_else(|| "not available here".to_owned());
        return format!("{label}: {why}");
    }
    state.run_command(command);
    if rest.is_empty() {
        label
    } else {
        format!("{label} · typed values are not read yet, so \"{rest}\" was left out")
    }
}

/// Enter at the bar's prompt.
fn submit_bar(state: &mut AppState, line: String) {
    let line = match line.trim() {
        "" => match state.command_line.history.last() {
            Some(last) => last.clone(),
            None => return,
        },
        _ => line,
    };
    state.command_line.remember(&line);
    match resolve(&line, Context::of(state), false) {
        Typed::Nothing | Typed::Close => {}
        Typed::Command(c, rest) if c.id == AI_COMMAND => {
            state.command_line.open_session();
            if !rest.is_empty() {
                state.chat.draft = rest.to_owned();
                state.push(AppAction::SendChat);
            }
        }
        Typed::Command(c, rest) => {
            let enabled = c.action_now(state).is_some();
            let said = run(state, c, rest);
            if !enabled || !rest.is_empty() {
                state.note(said, NoteKind::Info);
            }
        }
        Typed::Unknown(closest) => state.note(unknown_text(&line, &closest), NoteKind::Error),
    }
}

/// Enter at the session's prompt: a command's name, or `/` and a command, runs it there;
/// anything else is for the assistant.
fn submit_session(state: &mut AppState, line: String) {
    if line.trim().is_empty() {
        return;
    }
    state.command_line.remember(&line);
    let slash = line.trim_start().starts_with('/');
    let echo = |state: &mut AppState, said: String| {
        state.chat.messages.push(ChatMessage::new(Role::Command, line.trim()));
        state.chat.messages.push(ChatMessage::new(Role::System, said));
    };
    match resolve(&line, Context::of(state), true) {
        Typed::Nothing => {}
        Typed::Close => state.command_line.close_session(),
        Typed::Command(c, rest) if slash || rest.is_empty() => {
            if c.id == AI_COMMAND {
                state.command_line.close_session();
                return;
            }
            let said = run(state, c, rest);
            echo(state, said);
        }
        Typed::Unknown(closest) if slash => echo(state, unknown_text(&line, &closest)),
        _ => {
            state.chat.draft = line.trim().to_owned();
            state.send_chat();
        }
    }
}

/// The bottom of the window: the bar, with the session above it while it is open. Dragging
/// the bar's top edge up opens the session; dragging the session down past its minimum
/// closes it.
pub fn panel(root: &mut egui::Ui, state: &mut AppState) {
    let max = (root.available_height() - 220.0).max(SESSION_MIN_HEIGHT);
    let frame = egui::Frame::new().fill(theme::command_line()).inner_margin(egui::Margin::ZERO);
    let collapsed = egui::Panel::bottom(bar_id()).resizable(true).exact_size(theme::COMMAND_LINE_HEIGHT).frame(frame);
    let expanded = egui::Panel::bottom(session_id())
        .resizable(true)
        .default_size(state.command_line.settings.session_height.min(max))
        .size_range(SESSION_MIN_HEIGHT..=max)
        .frame(frame);
    let before = state.command_line.session_open;
    let mut open = before;
    egui::Panel::show_switched(root, &mut open, collapsed, expanded, |ui, expanded| {
        if expanded {
            let full = ui.max_rect();
            let split = full.bottom() - theme::COMMAND_LINE_HEIGHT;
            let top = Rect::from_min_max(full.min, pos2(full.right(), split));
            let bottom = Rect::from_min_max(pos2(full.left(), split), full.max);
            ui.scope_builder(egui::UiBuilder::new().max_rect(top), |ui| session(ui, state));
            ui.scope_builder(egui::UiBuilder::new().max_rect(bottom), |ui| bar(ui, state, true));
        } else {
            bar(ui, state, false);
        }
    });
    if open != before {
        state.command_line.session_open = open;
        state.command_line.focus_requested |= open;
    }
    if state.command_line.session_open
        && let Some(s) = egui::PanelState::load(root.ctx(), session_id())
    {
        state.command_line.settings.session_height = s.outer_rect.height().max(SESSION_MIN_HEIGHT);
    }
}

/// The bar: the chips, then the prompt, or while the session is open a line saying the
/// typing happens above.
fn bar(ui: &mut egui::Ui, state: &mut AppState, session_open: bool) {
    let rect = ui.max_rect();
    ui.painter().hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, theme::LINE));
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.add_space(8.0);
        chips(ui, state);
        ui.add_space(6.0);
        let x = ui.cursor().left();
        ui.painter().vline(x, rect.y_range().shrink(7.0), Stroke::new(1.0, theme::LINE));
        ui.add_space(10.0);
        if session_open {
            let text = format!("Typing goes to the session above · Esc or {} closes it", CLOSE_WORDS[0]);
            ui.label(egui::RichText::new(text).font(theme::mono(11.5)).color(theme::MUTED));
        } else {
            ui.label(egui::RichText::new("Command:").font(theme::mono(12.5)).color(theme::command_prompt()));
            prompt(ui, state, false);
        }
    });
}

/// The prompt's text field, with Tab, Up, Down, Enter and Escape taken before it sees them.
fn prompt(ui: &mut egui::Ui, state: &mut AppState, session: bool) {
    let id = input_id();
    let focused = ui.memory(|m| m.has_focus(id));
    let none = Modifiers::NONE;
    let (tab, up, down, enter, escape) = if focused {
        ui.input_mut(|i| {
            (
                i.consume_key(none, Key::Tab),
                i.consume_key(none, Key::ArrowUp),
                i.consume_key(none, Key::ArrowDown),
                i.consume_key(none, Key::Enter),
                i.consume_key(none, Key::Escape),
            )
        })
    } else {
        Default::default()
    };
    let context = Context::of(state);
    if up || down {
        state.command_line.browse(up);
    }
    if tab {
        state.command_line.complete(context);
    }
    let (ink, hint) = if session {
        (
            theme::terminal_ink(),
            egui::RichText::new("Ask the assistant, or type a command").color(theme::terminal_muted()),
        )
    } else {
        let hint = format!("Type a command, Tab completes · {AI_COMMAND} opens the assistant");
        (theme::INK, egui::RichText::new(hint).color(theme::MUTED))
    };
    let cl = &mut state.command_line;
    let edit = egui::TextEdit::singleline(&mut cl.input)
        .id(id)
        .font(theme::mono(12.5))
        .text_color(ink)
        .hint_text(hint)
        .frame(egui::Frame::NONE)
        .desired_width(ui.available_width() - 12.0)
        .return_key(None)
        .event_filter(egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true });
    let resp = ui.add(edit);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Command line"));
    if resp.changed() {
        cl.completion = None;
        cl.browsing = None;
    }
    if cl.focus_requested {
        resp.request_focus();
        cl.focus_requested = false;
    }
    if std::mem::take(&mut cl.cursor_to_end)
        && let Some(mut s) = egui::text_edit::TextEditState::load(ui.ctx(), id)
    {
        let end = egui::text::CCursor::new(cl.input.chars().count());
        s.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
        s.store(ui.ctx(), id);
    }
    completion_list(ui, cl, &resp);
    if enter {
        let line = std::mem::take(&mut state.command_line.input);
        if session { submit_session(state, line) } else { submit_bar(state, line) }
    }
    if escape {
        let cl = &mut state.command_line;
        if !cl.input.is_empty() {
            cl.input.clear();
        } else if session && state.chat.busy() {
            state.stop_chat();
        } else if session {
            cl.close_session();
        } else {
            ui.memory_mut(|m| m.surrender_focus(id));
        }
    }
}

/// The names Tab found, over the prompt, while there are several.
fn completion_list(ui: &egui::Ui, cl: &mut CommandLine, field: &egui::Response) {
    let Some(c) = cl.completion.as_ref().filter(|c| c.names.len() > 1) else { return };
    let slash = if c.slash { "/" } else { "" };
    let mut picked = None;
    egui::Area::new(egui::Id::new("command_completions"))
        .order(egui::Order::Foreground)
        .fixed_pos(field.rect.left_top() - vec2(4.0, 6.0))
        .pivot(Align2::LEFT_BOTTOM)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                for (i, name) in c.names.iter().enumerate().take(14) {
                    let label = commands::get(name).map(|c| c.label).unwrap_or("closes the session");
                    let text = egui::RichText::new(format!("{name:<24}{label}")).font(theme::mono(11.5));
                    if ui.add(egui::Button::selectable(c.shown == Some(i), text)).clicked() {
                        picked = Some(*name);
                    }
                }
                if c.names.len() > 14 {
                    ui.label(egui::RichText::new(format!("{} more", c.names.len() - 14)).color(theme::MUTED));
                }
            });
        });
    if let Some(name) = picked {
        cl.input = format!("{slash}{name}");
        cl.completion = None;
        cl.cursor_to_end = true;
        cl.focus_requested = true;
    }
}

/// The chip row, then + to add a chip. Takes a dropped command anywhere on the row.
fn chips(ui: &mut egui::Ui, state: &mut AppState) {
    let dragging = egui::DragAndDrop::payload::<ToolDrag>(ui.ctx());
    let start = ui.cursor().left();
    let tools = state.command_line.settings.tools.clone();
    let mut run = None;
    let mut drop: Option<(usize, &'static str)> = None;
    let mut remove = None;
    let mut reset = false;
    for (i, id) in tools.iter().enumerate() {
        let Some(command) = commands::get(id) else { continue };
        let resp = chip(ui, state, command, dragging.as_ref().is_some_and(|d| d.id == command.id));
        resp.dnd_set_drag_payload(ToolDrag { id: command.id });
        let pointer_x = ui.ctx().pointer_interact_pos().map_or(resp.rect.center().x, |p| p.x);
        let at = if pointer_x < resp.rect.center().x { i } else { i + 1 };
        if resp.dnd_hover_payload::<ToolDrag>().is_some() {
            let x = if at == i { resp.rect.left() - 2.5 } else { resp.rect.right() + 2.5 };
            ui.painter().vline(x, resp.rect.y_range().expand(2.0), Stroke::new(2.0, theme::command_prompt()));
        }
        if let Some(d) = resp.dnd_release_payload::<ToolDrag>() {
            drop = Some((at, d.id));
        }
        if resp.clicked() {
            run = Some(command);
        }
        resp.context_menu(|ui| {
            if ui.button(format!("Remove {} from the bar", command.label)).clicked() {
                remove = Some(command.id);
            }
            if ui.button("Reset the bar").clicked() {
                reset = true;
            }
        });
    }
    let plus = add_button(ui, state);
    if let Some(d) = plus.dnd_release_payload::<ToolDrag>() {
        drop = Some((tools.len(), d.id));
    }
    let row = Rect::from_x_y_ranges(start..=ui.cursor().left(), ui.max_rect().y_range());
    let area = ui.interact(row, ui.id().with("chip_row"), Sense::hover());
    if let Some(d) = area.dnd_release_payload::<ToolDrag>() {
        drop = Some((tools.len(), d.id));
    }
    if let Some(d) = dragging {
        if area.contains_pointer() {
            ui.painter().rect_stroke(
                row.shrink2(vec2(-3.0, 4.0)),
                CornerRadius::same(theme::RADIUS),
                Stroke::new(1.0, theme::command_prompt()),
                StrokeKind::Inside,
            );
        }
        // The dragged command follows the pointer.
        if let Some(p) = ui.ctx().pointer_interact_pos() {
            let label = commands::get(d.id).map_or(d.id, |c| c.label);
            let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, ui.id().with("chip_ghost")));
            let galley = painter.layout_no_wrap(label.to_owned(), theme::font_medium(11.5), theme::SELECTION_INK);
            let r = Rect::from_min_size(p + vec2(12.0, 6.0), galley.size() + vec2(16.0, 8.0));
            painter.rect(
                r,
                CornerRadius::same(99),
                theme::SELECTION_BG,
                Stroke::new(1.0, theme::command_prompt()),
                StrokeKind::Inside,
            );
            painter.galley(r.center() - galley.size() / 2.0, galley, theme::SELECTION_INK);
        }
    }
    let bar = &mut state.command_line.settings;
    if let Some((at, id)) = drop {
        bar.place(id, Some(at));
    }
    if let Some(id) = remove {
        bar.remove(id);
    }
    if reset {
        bar.tools = BarSettings::default().tools;
    }
    if let Some(command) = run {
        state.run_command(command);
    }
}

/// One chip: the command's icon and label in a pill, lit while the command is on.
fn chip(ui: &mut egui::Ui, state: &AppState, command: &'static CommandSpec, lifted: bool) -> egui::Response {
    let label = command.label_in(state);
    let enabled = command.is_enabled(state);
    let active = (command.active)(state);
    let galley = ui.painter().layout_no_wrap(label.clone(), theme::font_medium(11.5), theme::INK);
    let size = vec2(galley.size().x + 36.0, 22.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let name = format!("{label} on the command bar");
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, active, name.clone()));
    let (fill, stroke, ink) = if active {
        (theme::SELECTION_BG, theme::command_prompt(), theme::SELECTION_INK)
    } else if resp.hovered() && enabled {
        (theme::HOVER, theme::LINE, theme::INK)
    } else {
        (theme::chip(), theme::LINE, if enabled { theme::INK } else { theme::MUTED.gamma_multiply(0.7) })
    };
    let fill = if lifted { fill.gamma_multiply(0.4) } else { fill };
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(99), fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
    let icon = Rect::from_center_size(pos2(rect.left() + 14.0, rect.center().y), vec2(13.0, 13.0));
    icons::paint(painter, command.icon, icon, ink);
    painter.galley(pos2(rect.left() + 25.0, rect.center().y - galley.size().y / 2.0), galley, ink);
    let tip = format!("{}\nDrag to move · right-click to remove", command.tip(state));
    resp.on_hover_text(tip)
}

/// + at the end of the row: a menu of the commands to add as chips.
fn add_button(ui: &mut egui::Ui, state: &mut AppState) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Add a tool to the bar"));
    let bg = if resp.hovered() { theme::HOVER } else { theme::PANEL };
    ui.painter().rect(rect, CornerRadius::same(99), bg, Stroke::new(1.0, theme::LINE), StrokeKind::Inside);
    let c = rect.center();
    let s = Stroke::new(1.4, theme::MUTED);
    ui.painter().hline((c.x - 4.5)..=(c.x + 4.5), c.y, s);
    ui.painter().vline(c.x, (c.y - 4.5)..=(c.y + 4.5), s);
    let resp = resp.on_hover_text("Add a tool to the bar, or drag a ribbon button here");
    egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.set_min_width(240.0);
        let field = ui.add(
            egui::TextEdit::singleline(&mut state.command_line.adding)
                .hint_text("Find a command")
                .desired_width(f32::INFINITY),
        );
        field.request_focus();
        let context = Context::of(state);
        let query = state.command_line.adding.trim().to_owned();
        let found: Vec<&'static CommandSpec> = if query.is_empty() {
            commands::all().iter().filter(|c| c.available(context)).take(12).collect()
        } else {
            commands::search_here(&query, &state.commands, context).into_iter().take(12).map(|m| m.command).collect()
        };
        for command in found {
            let on_bar = state.command_line.settings.position(command.id).is_some();
            let text = if on_bar { format!("{} (on the bar)", command.label) } else { command.label.to_owned() };
            if ui.add_enabled(!on_bar, egui::Button::new(text)).on_hover_text(command.doc).clicked() {
                state.command_line.settings.place(command.id, None);
                state.command_line.adding.clear();
                ui.close();
            }
        }
    });
    resp
}

/// The session pane: a header, the scrollback and the prompt, on the terminal's dark ground.
fn session(ui: &mut egui::Ui, state: &mut AppState) {
    let full = ui.max_rect();
    ui.painter().rect_filled(full, CornerRadius::ZERO, theme::terminal_bg());
    // Text egui colours itself, such as the prompt's hint and cursor, follows the dark ground.
    let visuals = ui.visuals_mut();
    visuals.widgets.noninteractive.fg_stroke.color = theme::terminal_ink();
    visuals.text_cursor.stroke.color = theme::terminal_accent();
    visuals.selection.bg_fill = theme::terminal_accent().gamma_multiply(0.35);
    let header = Rect::from_min_size(full.min, vec2(full.width(), 26.0));
    ui.painter().hline(full.x_range(), header.bottom(), Stroke::new(1.0, theme::terminal_line()));
    ui.painter().text(
        pos2(header.left() + 12.0, header.center().y),
        Align2::LEFT_CENTER,
        format!("assistant session · {}", state.chat.status),
        theme::mono(11.0),
        theme::terminal_muted(),
    );
    let close = Rect::from_center_size(pos2(header.right() - 16.0, header.center().y), vec2(20.0, 20.0));
    let close_resp = ui.interact(close, ui.id().with("session_close"), Sense::click());
    close_resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Close the session"));
    if close_resp.hovered() {
        ui.painter().rect_filled(close, CornerRadius::same(4), theme::terminal_line());
    }
    icons::paint(ui.painter(), Icon::Close, close.shrink(5.0), theme::terminal_ink());
    if close_resp.on_hover_text("Close the session (Esc)").clicked() {
        state.command_line.close_session();
    }

    let prompt_h = 30.0;
    let body =
        Rect::from_min_max(pos2(full.left(), header.bottom() + 1.0), pos2(full.right(), full.bottom() - prompt_h));
    let mut actions = Vec::new();
    ui.scope_builder(egui::UiBuilder::new().max_rect(body.shrink2(vec2(12.0, 6.0))), |ui| {
        // The frame gives the scroll area's edge fades the dark ground to fade into.
        egui::Frame::new().fill(theme::terminal_bg()).show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("session_scrollback")
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    banner(ui);
                    let previewing = state.chat.preview.as_ref().map(|(id, _)| id.clone());
                    for msg in state.chat.messages.clone() {
                        let cards: Vec<(Proposal, bool)> = msg
                            .proposals
                            .iter()
                            .filter_map(|id| state.chat.book.get(id))
                            .map(|p| {
                                let mut p = p.clone();
                                p.status = state.proposal_status(&p.id).unwrap_or(p.status);
                                let undo = state.can_undo(&p.id);
                                (p, undo)
                            })
                            .collect();
                        line(ui, &msg, &cards, previewing.as_deref(), &mut actions);
                    }
                });
        });
    });
    for a in actions {
        state.push(a);
    }

    let row = Rect::from_min_max(pos2(full.left(), full.bottom() - prompt_h), full.max);
    ui.painter().hline(full.x_range(), row.top(), Stroke::new(1.0, theme::terminal_line()));
    ui.scope_builder(egui::UiBuilder::new().max_rect(row.shrink2(vec2(12.0, 0.0))), |ui| {
        ui.horizontal_centered(|ui| {
            ui.label(egui::RichText::new("›").font(theme::mono(14.0)).color(theme::terminal_accent()));
            if state.chat.busy() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let stop = egui::Button::new(egui::RichText::new("Stop").font(theme::mono(11.5)));
                    if ui.add(stop).on_hover_text("Stop the answer (Esc)").clicked() {
                        state.push(AppAction::StopChat);
                    }
                    ui.label(egui::RichText::new("working…").font(theme::mono(11.5)).color(theme::terminal_muted()));
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| prompt(ui, state, true));
                });
            } else {
                prompt(ui, state, true);
            }
        });
    });
}

fn banner(ui: &mut egui::Ui) {
    let shortcut = commands::get(AI_COMMAND).and_then(CommandSpec::shortcut_text).unwrap_or_default();
    ui.label(
        egui::RichText::new("Welcome to the SolidDraft session.")
            .font(theme::mono(12.5))
            .color(theme::terminal_accent()),
    );
    for text in [
        "Type a command's name to run it (Tab completes, a leading / always means a command),".to_owned(),
        "or write to the assistant in plain words: it reads the part and proposes changes you apply.".to_owned(),
        format!("Esc or {} closes the session; {AI_COMMAND} or {shortcut} opens it again.", CLOSE_WORDS[0]),
    ] {
        ui.label(egui::RichText::new(text).font(theme::mono(11.5)).color(theme::terminal_muted()));
    }
    ui.add_space(6.0);
}

fn text(ui: &mut egui::Ui, text: &str, color: Color32) {
    ui.add(egui::Label::new(egui::RichText::new(text).font(theme::mono(12.0)).color(color)).wrap());
}

/// A line typed at the prompt: the prompt mark, then the text.
fn typed(ui: &mut egui::Ui, line: &str) {
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(egui::RichText::new("›").font(theme::mono(12.0)).color(theme::terminal_accent()));
        text(ui, line, theme::terminal_ink());
    });
}

fn line(
    ui: &mut egui::Ui,
    msg: &ChatMessage,
    cards: &[(Proposal, bool)],
    previewing: Option<&str>,
    actions: &mut Vec<AppAction>,
) {
    match msg.role {
        Role::System => text(ui, &msg.text, theme::terminal_muted()),
        Role::User | Role::Command => typed(ui, &msg.text),
        Role::Assistant => {
            if !msg.text.is_empty() {
                text(ui, &msg.text, theme::terminal_ink());
            } else if msg.streaming && msg.tools.is_empty() {
                text(ui, "Thinking…", theme::terminal_muted());
            }
            for t in &msg.tools {
                tool_line(ui, t);
            }
            for (p, can_undo) in cards {
                proposal_card(ui, p, *can_undo, previewing == Some(p.id.as_str()), actions);
            }
        }
    }
}

fn tool_line(ui: &mut egui::Ui, t: &ToolActivity) {
    let (said, color) = match &t.state {
        ToolState::Running(0) => (format!("  └ {} …", t.name), theme::terminal_muted()),
        ToolState::Running(bytes) => (format!("  └ {} … {bytes} bytes", t.name), theme::terminal_muted()),
        ToolState::Done(s) => (format!("  └ {} · {s}", t.name), theme::terminal_muted()),
        ToolState::Failed(e) => (format!("  └ {} failed: {e}", t.name), theme::ERROR),
    };
    text(ui, &said, color);
}

fn proposal_card(ui: &mut egui::Ui, p: &Proposal, can_undo: bool, previewing: bool, actions: &mut Vec<AppAction>) {
    let inactive = matches!(p.status, ProposalStatus::Discarded | ProposalStatus::Undone);
    let (accent, ink) = if inactive {
        (theme::terminal_line(), theme::terminal_muted())
    } else {
        (theme::terminal_accent(), theme::terminal_ink())
    };
    ui.add_space(4.0);
    egui::Frame::new()
        .stroke(Stroke::new(1.0, accent))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            let status = match &p.status {
                ProposalStatus::Pending => "proposal",
                ProposalStatus::Applied { .. } => "applied",
                ProposalStatus::Undone => "undone",
                ProposalStatus::Discarded => "discarded",
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(format!("[{}] {status}", p.id)).font(theme::mono(12.0)).color(accent));
                text(ui, &p.summary, ink);
            });
            let value_width = (ui.available_width() - 140.0).max(80.0);
            egui::Grid::new(("session_proposal", &p.id)).num_columns(2).spacing([12.0, 2.0]).show(ui, |ui| {
                for (k, v) in &p.rows {
                    ui.label(egui::RichText::new(k).font(theme::mono(11.5)).color(theme::terminal_muted()));
                    ui.scope(|ui| {
                        ui.set_max_width(value_width);
                        ui.add(egui::Label::new(egui::RichText::new(v).font(theme::mono(11.5)).color(ink)).wrap());
                    });
                    ui.end_row();
                }
            });
            ui.add_space(4.0);
            match &p.status {
                ProposalStatus::Pending => {
                    ui.horizontal(|ui| {
                        if p.preview.result.is_some() {
                            let label = if previewing { "Hide preview" } else { "Preview" };
                            if ui.add(egui::Button::new(label).selected(previewing)).clicked() {
                                actions.push(AppAction::PreviewProposal(p.id.clone()));
                            }
                        }
                        let apply = egui::Button::new(egui::RichText::new("Apply").color(theme::terminal_bg()))
                            .fill(theme::terminal_accent())
                            .stroke(Stroke::NONE);
                        if ui.add(apply).clicked() {
                            actions.push(AppAction::ApplyProposal(p.id.clone()));
                        }
                        if ui.button("Discard").clicked() {
                            actions.push(AppAction::DiscardProposal(p.id.clone()));
                        }
                    });
                }
                ProposalStatus::Applied { .. } => {
                    ui.horizontal_wrapped(|ui| {
                        text(ui, "Applied to the tree as ordinary features.", theme::terminal_muted());
                        if ui.add_enabled(can_undo, egui::Button::new(format!("Undo {}", p.id))).clicked() {
                            actions.push(AppAction::UndoProposal(p.id.clone()));
                        }
                    });
                }
                ProposalStatus::Undone | ProposalStatus::Discarded => {}
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part() -> Context {
        Context::default()
    }

    #[test]
    fn names_run_by_id_or_label_and_the_rest_is_kept() {
        let id = |line: &str| match resolve(line, part(), false) {
            Typed::Command(c, rest) => Some((c.id, rest.to_owned())),
            _ => None,
        };
        assert_eq!(id("zoom_fit"), Some(("zoom_fit", String::new())));
        assert_eq!(id("Fit"), Some(("zoom_fit", String::new())), "a label works, in any case");
        assert_eq!(id("  find issues "), Some(("find_issues", String::new())), "a label of two words");
        assert_eq!(id("/extrude"), Some(("extrude", String::new())));
        assert_eq!(id("ai add a boss"), Some((AI_COMMAND, "add a boss".to_owned())));
        assert!(matches!(resolve("   ", part(), false), Typed::Nothing));
        assert!(matches!(resolve("filet", part(), false), Typed::Unknown(c) if c.iter().any(|c| c.id == "fillet")));
        assert!(matches!(resolve("model_summary", part(), false), Typed::Unknown(_)), "agent-only commands stay out");
    }

    #[test]
    fn edits_count_single_character_changes() {
        assert_eq!(edits("filet", "fillet"), 1);
        assert_eq!(edits("extrdue", "extrude"), 2);
        assert_eq!(edits("", "fit"), 3);
        assert_eq!(edits("fit", "fit"), 0);
    }

    #[test]
    fn a_shared_label_takes_the_command_the_workspace_offers() {
        let drawing = Context { drawing: true, ..Default::default() };
        assert_eq!(named("explain", part()).map(|c| c.id), Some("explain"));
        assert_eq!(named("explain", drawing).map(|c| c.id), Some("drawing_explain"));
    }

    #[test]
    fn close_words_close_only_the_session() {
        assert!(matches!(resolve("exit", part(), true), Typed::Close));
        assert!(matches!(resolve("/Close", part(), true), Typed::Close));
        assert!(matches!(resolve("exit", part(), false), Typed::Command(c, _) if c.id == "exit_sketch"));
    }

    #[test]
    fn tab_completes_then_steps_through_the_names() {
        let mut cl = CommandLine { input: "zoom".into(), ..Default::default() };
        cl.complete(part());
        assert_eq!(cl.input, "zoom_fit", "one name fits");
        cl.input = "view_b".into();
        cl.complete(part());
        assert_eq!(cl.input, "view_b", "view_back and view_bottom agree no further");
        cl.complete(part());
        assert_eq!(cl.input, "view_back");
        cl.complete(part());
        assert_eq!(cl.input, "view_bottom");
        cl.complete(part());
        assert_eq!(cl.input, "view_back", "and round again");
        let mut cl = CommandLine { input: "/ex".into(), session_open: true, ..Default::default() };
        cl.complete(part());
        assert!(cl.input.starts_with("/ex"), "the slash stays: {}", cl.input);
        assert!(completions("ex", part(), true).contains(&"exit"));
        assert!(!completions("ex", part(), false).contains(&"exit"));
    }

    #[test]
    fn up_and_down_walk_the_history_and_come_back_to_the_typing() {
        let mut cl = CommandLine::default();
        cl.browse(true);
        assert_eq!(cl.input, "", "nothing typed yet");
        for line in ["extrude", "fillet", "fillet", "zoom_fit"] {
            cl.remember(line);
        }
        assert_eq!(cl.history, ["extrude", "fillet", "zoom_fit"], "a repeat is kept once");
        cl.input = "half typed".into();
        cl.browse(true);
        assert_eq!(cl.input, "zoom_fit");
        cl.browse(true);
        cl.browse(true);
        cl.browse(true);
        assert_eq!(cl.input, "extrude", "stops at the oldest");
        cl.browse(false);
        assert_eq!(cl.input, "fillet");
        cl.browse(false);
        cl.browse(false);
        assert_eq!(cl.input, "half typed");
    }

    #[test]
    fn chips_move_add_and_clean_up() {
        let mut bar = BarSettings::default();
        assert_eq!(bar.tools, DEFAULT_TOOLS);
        bar.place("zoom_fit", Some(0));
        assert_eq!(bar.tools[0], "zoom_fit");
        bar.place("zoom_fit", Some(3));
        assert_eq!(bar.tools, ["sketch_select", "cancel", "zoom_fit", "measure", "normal_to"]);
        bar.place("fillet", None);
        assert_eq!(bar.tools.last().unwrap(), "fillet");
        bar.remove("cancel");
        assert!(bar.position("cancel").is_none());
        let stale = BarSettings {
            tools: vec!["fillet".into(), "gone".into(), "model_summary".into(), "fillet".into()],
            session_height: f32::NAN,
        };
        let clean = stale.cleaned();
        assert_eq!(clean.tools, ["fillet"]);
        assert_eq!(clean.session_height, SESSION_DEFAULT_HEIGHT);
    }
}
