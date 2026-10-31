//! The command registry: one row per command a person or an agent can run.
//!
//! The ribbon, the keyboard shortcuts, the menus, the shortcuts window, the command palette,
//! the command search, the assistant's recipe verbs, the MCP server and the CLI all read
//! this table, so a command has one id, one label, one description and one set of shortcuts
//! everywhere.
//!
//! A row describes and dispatches a command; it does not replace the typed commands
//! underneath. In the app a row yields a typed [`AppAction`]; for an agent, a verb's
//! input parses into a typed recipe [`Line`](crate::command_tools::Line) that runs as
//! `soliddraft_model` commands, and the other tools run the same typed commands through
//! [`AppTools`](crate::app_tools::AppTools), which the MCP servers and `soliddraft run`
//! serve. A command that needs the window says why in its row ([`Agents::App`]).
//!
//! A tab's panels come in the order [`panels`] gives, and each panel lists its commands
//! in table order.

#[path = "assembly/registry.rs"]
mod assembly_registry;

use crate::app_tools as app;
use crate::assembly::AssemblyAction;
use crate::command_tools as tools;
use crate::document::{Command, Selection};
use crate::drawing::{DrawingAction, DrawingSel, DrawingSession, DrawingTab, DrawingTool, LinkState, PlaceKind};
use crate::icons::Icon;
use crate::panels::Side;
use crate::state::{AppAction, AppState, DragStep, EditorParams, ExitInto, RibbonTab, Tool};
use crate::viewport::camera::StandardView;
use crate::viewport::look::Preset;
use crate::viewport::render::DisplayMode;
use egui::{Key, Modifiers};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use soliddraft_assistant::ToolSpec;
use soliddraft_assistant::tools::schema::object;
use soliddraft_drawing::export::Format;
use soliddraft_drawing::{
    Command as DrawingCommand, HiddenLines, SetKind, TangentEdges, View as SheetView, ViewDisplay,
};
use soliddraft_exchange::Format as ExportFormat;
use std::sync::LazyLock;

/// A ribbon tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    /// A tab of the part's ribbon. The part's Assembly tab is [`Tab::Assembly`].
    Part(RibbonTab),
    /// A tab of the drawing's ribbon.
    Drawing(DrawingTab),
    /// The assembly's commands, on the assembly's one tab and on the part's Assembly tab.
    Assembly,
}

impl Tab {
    /// Every tab with commands, the part's, then the drawing's. The assembly's is among
    /// the part's.
    pub fn all() -> Vec<Tab> {
        RibbonTab::ALL
            .into_iter()
            .map(Tab::from)
            .filter(|t| !panels(*t).is_empty())
            .chain(DrawingTab::ALL.into_iter().map(Tab::Drawing))
            .collect()
    }

    pub fn label(self) -> &'static str {
        match self {
            Tab::Part(t) => t.label(),
            Tab::Drawing(t) => t.label(),
            Tab::Assembly => "Assembly",
        }
    }
}

impl From<RibbonTab> for Tab {
    fn from(t: RibbonTab) -> Self {
        match t {
            RibbonTab::Assembly => Tab::Assembly,
            t => Tab::Part(t),
        }
    }
}

impl From<DrawingTab> for Tab {
    fn from(t: DrawingTab) -> Self {
        Tab::Drawing(t)
    }
}

/// A tab's panels, left to right.
pub fn panels(tab: Tab) -> &'static [&'static str] {
    use DrawingTab as D;
    match tab {
        Tab::Part(RibbonTab::Solid) => &["Sketch", "Create", "Remove", "Modify", "Pattern", "Reference"],
        Tab::Part(RibbonTab::Sketch) => &["Sketch", "Draw", "Constrain", "Use sketch", "View"],
        Tab::Part(RibbonTab::Inspect) => &["Measure", "Check", "Surface quality"],
        Tab::Part(RibbonTab::View) => &["Orientation", "Display", "Projection", "Panels"],
        Tab::Part(RibbonTab::Assembly | RibbonTab::Surface | RibbonTab::PointCloud) => &[],
        Tab::Drawing(D::Views) => &["Edit", "Standard", "Derived", "Arrange", "Display", "Model", "Assistant"],
        Tab::Drawing(D::Annotate) => &["Edit", "Dimension", "Notes", "Centre", "Symbols", "Tables", "Assistant"],
        Tab::Drawing(D::Sheet) => &["Edit", "Sheet", "Format", "Export"],
        Tab::Drawing(D::View) => &["Zoom", "Sheet", "Documents", "Panels"],
        Tab::Drawing(D::Assistant) => &["Chat", "Drawing"],
        Tab::Assembly => &["Edit", "Component", "File", "View"],
    }
}

/// Which workspace a command works in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Workspace {
    Part,
    Drawing,
    Assembly,
    /// Every workspace.
    Both,
}

impl Workspace {
    /// Whether a command of this workspace works in a context.
    pub fn applies(self, c: Context) -> bool {
        match self {
            Workspace::Part => !c.drawing && !c.assembly,
            Workspace::Drawing => c.drawing,
            Workspace::Assembly => c.assembly,
            Workspace::Both => true,
        }
    }
}

/// How big a ribbon tile's icon is: large for a main verb, small for its variants. Either
/// way the label sits under the icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Large,
    Small,
}

/// Where a command sits on the ribbon. A command can sit in several places.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    pub tab: Tab,
    /// The panel's label, such as "Create".
    pub panel: &'static str,
    pub size: Size,
}

/// Where a shortcut works. Two commands may share a key only where their scopes never
/// apply together, or where one scope is narrower and wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    /// Both workspaces.
    App,
    /// The part workspace, in a sketch or not.
    Part,
    /// The part workspace outside a sketch.
    Model,
    /// The part workspace outside a sketch, with an edge of the body selected. Wins over
    /// [`Scope::Model`].
    Edge,
    /// Inside a sketch.
    Sketch,
    /// The drawing workspace.
    Drawing,
    /// The drawing workspace while a section or auxiliary view is being placed. Wins over
    /// [`Scope::Drawing`].
    Placing,
    /// The assembly workspace.
    Assembly,
}

impl Scope {
    pub const ALL: [Scope; 8] = [
        Scope::App,
        Scope::Part,
        Scope::Model,
        Scope::Edge,
        Scope::Sketch,
        Scope::Drawing,
        Scope::Placing,
        Scope::Assembly,
    ];

    /// Whether the scope applies in a context.
    pub fn applies(self, c: Context) -> bool {
        match self {
            Scope::App => true,
            Scope::Part => c.part(),
            Scope::Model => c.part() && !c.sketch,
            Scope::Edge => c.part() && !c.sketch && c.edge,
            Scope::Sketch => c.part() && c.sketch,
            Scope::Drawing => c.drawing,
            Scope::Placing => c.drawing && c.placing,
            Scope::Assembly => c.assembly,
        }
    }

    /// Whether this scope's shortcuts win over `other`'s where both apply.
    pub fn narrower_than(self, other: Scope) -> bool {
        matches!((self, other), (Scope::Edge, Scope::Model) | (Scope::Placing, Scope::Drawing))
    }

    /// Whether some wider scope gives way to this one.
    pub fn is_narrow(self) -> bool {
        Scope::ALL.iter().any(|other| self.narrower_than(*other))
    }

    /// Where the scope applies, for the shortcuts window.
    pub fn label(self) -> &'static str {
        match self {
            Scope::App => "Everywhere",
            Scope::Part => "In the part",
            Scope::Model => "Outside a sketch",
            Scope::Edge => "With an edge selected",
            Scope::Sketch => "In a sketch",
            Scope::Drawing => "In a drawing",
            Scope::Placing => "Placing a section or auxiliary view",
            Scope::Assembly => "In an assembly",
        }
    }
}

/// What decides which shortcuts apply.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Context {
    /// The drawing workspace is active.
    pub drawing: bool,
    /// A sketch is open.
    pub sketch: bool,
    /// An edge of the body is selected.
    pub edge: bool,
    /// A section or auxiliary view is being placed in the drawing.
    pub placing: bool,
    /// The assembly workspace is active.
    pub assembly: bool,
}

impl Context {
    /// Every context, for checks over all of them.
    pub fn all() -> Vec<Context> {
        let mut all = Vec::new();
        for drawing in [false, true] {
            for sketch in [false, true] {
                for edge in [false, true] {
                    for placing in [false, true] {
                        all.push(Context { drawing, sketch, edge, placing, assembly: false });
                    }
                }
            }
        }
        // One workspace is active at a time.
        all.push(Context { assembly: true, ..Context::default() });
        all
    }

    /// The part workspace is active.
    pub fn part(self) -> bool {
        !self.drawing && !self.assembly
    }

    pub fn of(state: &AppState) -> Self {
        Context {
            drawing: state.drawing.is_active(),
            sketch: state.in_sketch(),
            edge: has_body(state) && matches!(state.doc.selection, Some(Selection::Edge(_))),
            placing: session(state)
                .is_some_and(|d| matches!(d.tool, DrawingTool::Section { .. } | DrawingTool::Auxiliary)),
            assembly: state.assembly.is_active(),
        }
    }
}

/// A key with modifiers, in a scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub scope: Scope,
    pub modifiers: Modifiers,
    pub key: Key,
    /// It fires even while a text field has focus. Chords with Ctrl do; a plain key only
    /// when its row says so, as F9 does.
    pub while_typing: bool,
}

impl Shortcut {
    pub fn fires_while_typing(&self) -> bool {
        self.while_typing
    }

    /// The same key and modifiers.
    pub fn same_chord(&self, other: &Shortcut) -> bool {
        self.key == other.key && self.modifiers == other.modifiers
    }

    /// How many modifiers it holds. egui matches a plain chord even with Shift held, so
    /// the chords with more modifiers are tried first.
    pub fn modifier_count(&self) -> usize {
        let m = self.modifiers;
        [m.command || m.ctrl, m.shift, m.alt].into_iter().filter(|b| *b).count()
    }

    /// As the interface writes it: "Ctrl+Shift+S", "F2", "Del".
    pub fn text(&self) -> String {
        let mut parts = Vec::new();
        if self.modifiers.command || self.modifiers.ctrl {
            parts.push("Ctrl".to_owned());
        }
        if self.modifiers.shift {
            parts.push("Shift".to_owned());
        }
        if self.modifiers.alt {
            parts.push("Alt".to_owned());
        }
        parts.push(
            match self.key {
                Key::Delete => "Del",
                Key::Escape => "Esc",
                Key::Backspace => "Backspace",
                Key::Enter => "Enter",
                k => k.symbol_or_name(),
            }
            .to_owned(),
        );
        parts.join("+")
    }
}

/// Which agents can reach a command, and how. MCP clients and `soliddraft run` reach
/// every command with a tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agents {
    /// No agent tool, because nothing runs the command yet: it has no action.
    None,
    /// No agent tool, because the command works on the window: the camera, the panels,
    /// a dialog or an interactive tool. The text says why, as a doc line does.
    App(&'static str),
    /// A tool the assistant and MCP clients both get.
    Tool,
    /// A recipe verb: a line of `propose` for both, and a tool of its own that runs at
    /// once for MCP clients.
    Verb,
    /// A tool MCP clients get and the assistant does not.
    Mcp,
}

impl Agents {
    /// Whether agents get a tool for the command.
    pub fn has_tool(self) -> bool {
        matches!(self, Agents::Tool | Agents::Verb | Agents::Mcp)
    }
}

/// What a command does to the undo stacks of the part and the drawing, wherever it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Undo {
    /// Leaves them as they are: it reads, writes a file or changes only the session.
    None,
    /// One step on the undo stack of the document it changes.
    Step,
    /// One step for each thing it adds.
    Steps,
    /// Moves along the undo stack: undo and redo.
    Moves,
    /// Puts a new or opened document in place, or closes one; the new one's undo stack
    /// starts empty.
    Replaces,
    /// Does what the journal command it carries does: one of the others.
    Carried,
}

impl Undo {
    /// As the CLI lists it.
    pub fn label(self) -> &'static str {
        match self {
            Undo::None => "none",
            Undo::Step => "step",
            Undo::Steps => "steps",
            Undo::Moves => "moves",
            Undo::Replaces => "replaces",
            Undo::Carried => "carried",
        }
    }

    /// What an agent tool's description says about it, if anything.
    fn sentence(self) -> Option<&'static str> {
        match self {
            Undo::None | Undo::Moves | Undo::Carried => None,
            Undo::Step => Some("One undo step."),
            Undo::Steps => Some("One undo step for each thing it adds."),
            Undo::Replaces => Some("The document it puts in place starts with an empty undo stack."),
        }
    }
}

/// One command.
pub struct CommandSpec {
    /// Unique, lower snake case. Also the MCP tool's name and the recipe line's `op`.
    pub id: &'static str,
    /// The ribbon's label.
    pub label: &'static str,
    /// The label now, for a button whose label follows the state, such as "Auto update
    /// on". The ribbon shows it; search and the CLI use `label`.
    pub label_now: fn(&AppState) -> Option<String>,
    pub workspace: Workspace,
    /// One line on what it does: the tooltip, the search result and the CLI listing.
    pub doc: &'static str,
    /// More for agents, after `doc`: how the parameters work, what to call first.
    pub details: &'static str,
    pub icon: Icon,
    pub ribbon: Vec<Place>,
    /// The first one is the one hints show.
    pub keys: Vec<Shortcut>,
    /// The agent tool's parameters as `(name, JSON schema)`; each schema's description
    /// documents its parameter. Empty for commands without parameters.
    pub params: fn() -> Vec<(&'static str, Value)>,
    pub agents: Agents,
    /// What it does to the undo stacks, in the app and as a tool alike.
    pub undo: Undo,
    /// What the app does. `None` for a command of a later phase, or one only agents run.
    pub action: Option<fn(&AppState) -> AppAction>,
    /// Whether the action can run now.
    pub enabled: fn(&AppState) -> bool,
    /// Whether its ribbon button is there at all, for a button that appears only when
    /// it applies, such as Repair reference.
    pub shown: fn(&AppState) -> bool,
    /// Whether its ribbon button shows as on: the open tool, the shown panel.
    pub active: fn(&AppState) -> bool,
    /// Said in place of `doc` now, such as what undo would undo or why the command is
    /// disabled. A shortcut pressed while the command is disabled shows it as a note.
    pub hint: fn(&AppState) -> Option<String>,
}

const LATER: &str = "Arrives in a later phase";

// Why commands run only in the app, for their rows.
const CAMERA: &str = "It moves the viewport camera, which only the window has";
const LOOK: &str = "It changes how the viewport draws the part, which only the window has";
const PANELS: &str = "It shows, hides or resets the window's panels";
const WINDOW: &str = "It opens a window of the app";
const EDITOR: &str = "It answers the open PropertyManager, sketch or drag, which only the window has; agents and scripts run each \
     command whole";
const SKETCHER: &str = "It picks a tool of the interactive sketcher; agents and scripts draw a whole sketch with sketch or \
     sketch_geometry";
const CHAT: &str =
    "It asks the app's own assistant; an MCP client reads the part with model_summary and feature and answers itself";
const CANVAS: &str = "It picks a tool of the drawing canvas, which places views and annotations where the sheet and \
                      its projected edges are clicked; there is no form without the window yet";
const CANVAS_STEP: &str = "It steers the drawing canvas tool in use, which only the window has";
const SHEET_VIEW: &str = "It changes how the drawing canvas shows the sheet, which only the window has";
const PANEL: &str = "It selects what the property panel edits; the panel's fields have no form without the window yet";
impl CommandSpec {
    pub(crate) fn new(id: &'static str, label: &'static str, icon: Icon, doc: &'static str) -> Self {
        CommandSpec {
            id,
            label,
            label_now: |_| None,
            workspace: Workspace::Part,
            doc,
            details: "",
            icon,
            ribbon: Vec::new(),
            keys: Vec::new(),
            params: Vec::new,
            agents: Agents::None,
            undo: Undo::None,
            action: None,
            enabled: |_| true,
            shown: |_| true,
            active: |_| false,
            hint: |_| None,
        }
    }

    pub(crate) fn on(mut self, tab: impl Into<Tab>, panel: &'static str, size: Size) -> Self {
        self.ribbon.push(Place { tab: tab.into(), panel, size });
        self
    }

    fn key(mut self, scope: Scope, modifiers: Modifiers, key: Key) -> Self {
        let while_typing = modifiers.command || modifiers.ctrl;
        self.keys.push(Shortcut { scope, modifiers, key, while_typing });
        self
    }

    /// A plain key that fires even while a text field has focus.
    fn key_anytime(mut self, scope: Scope, modifiers: Modifiers, key: Key) -> Self {
        self.keys.push(Shortcut { scope, modifiers, key, while_typing: true });
        self
    }

    fn both(mut self) -> Self {
        self.workspace = Workspace::Both;
        self
    }

    /// An assembly command: enabled while an assembly is open and active.
    pub(crate) fn assembly(mut self) -> Self {
        self.workspace = Workspace::Assembly;
        self.enabled = |s| s.assembly.is_active();
        self.hint = |s| (!s.assembly.is_active()).then(|| "Needs an assembly: File, New assembly".into());
        self
    }

    /// A drawing command: enabled while a drawing is open, unless `when` says more.
    fn drawing(mut self) -> Self {
        self.workspace = Workspace::Drawing;
        self.enabled = |s| session(s).is_some();
        self
    }

    fn label_now(mut self, label: fn(&AppState) -> Option<String>) -> Self {
        self.label_now = label;
        self
    }

    pub(crate) fn run(mut self, action: fn(&AppState) -> AppAction) -> Self {
        self.action = Some(action);
        self
    }

    fn when(mut self, enabled: fn(&AppState) -> bool) -> Self {
        self.enabled = enabled;
        self
    }

    fn shown(mut self, shown: fn(&AppState) -> bool) -> Self {
        self.shown = shown;
        self
    }

    pub(crate) fn lit(mut self, active: fn(&AppState) -> bool) -> Self {
        self.active = active;
        self
    }

    fn hint(mut self, hint: fn(&AppState) -> Option<String>) -> Self {
        self.hint = hint;
        self
    }

    pub(crate) fn agents(
        mut self,
        agents: Agents,
        details: &'static str,
        params: fn() -> Vec<(&'static str, Value)>,
    ) -> Self {
        self.agents = agents;
        self.details = details;
        self.params = params;
        self
    }

    /// A command that runs only in the app, and why.
    pub(crate) fn app_only(mut self, why: &'static str) -> Self {
        self.agents = Agents::App(why);
        self
    }

    pub(crate) fn undo(mut self, undo: Undo) -> Self {
        self.undo = undo;
        self
    }

    /// Whether the app runs it: it has an action, or it holds a place on the ribbon.
    pub fn in_app(&self) -> bool {
        self.action.is_some() || !self.ribbon.is_empty()
    }

    /// Whether the app offers it in a context: in the app, and in the context's workspace.
    pub fn available(&self, context: Context) -> bool {
        self.in_app() && self.workspace.applies(context)
    }

    /// The label as the ribbon shows it now.
    pub fn label_in(&self, state: &AppState) -> String {
        (self.label_now)(state).unwrap_or_else(|| self.label.to_owned())
    }

    /// Commands whose geometry is waiting for Catallax, in roadmap section 6 order.
    pub fn catallax_wait(&self) -> Option<&'static str> {
        match self.id {
            "sweep" | "loft" => Some("Arrives with Catallax stage 5"),
            _ => None,
        }
    }

    pub fn is_enabled(&self, state: &AppState) -> bool {
        self.catallax_wait().is_none() && self.action.is_some() && (self.enabled)(state)
    }

    /// The action to run now, or `None` when the command is disabled.
    pub fn action_now(&self, state: &AppState) -> Option<AppAction> {
        self.is_enabled(state).then(|| (self.action.expect("an enabled command has an action"))(state))
    }

    /// What its hint says now, or why it is disabled.
    pub fn hint_now(&self, state: &AppState) -> Option<String> {
        self.catallax_wait()
            .map(str::to_owned)
            .or_else(|| (self.hint)(state))
            .or_else(|| self.action.is_none().then(|| LATER.to_owned()))
    }

    /// The first shortcut, as hints show it.
    pub fn shortcut_text(&self) -> Option<String> {
        self.keys.first().map(Shortcut::text)
    }

    /// The tooltip: the hint or the doc, with the shortcut that works here when the
    /// command is enabled.
    pub fn tip(&self, state: &AppState) -> String {
        let text = self.hint_now(state).unwrap_or_else(|| self.doc.to_owned());
        let context = Context::of(state);
        match self.keys.iter().find(|k| k.scope.applies(context)) {
            Some(k) if self.is_enabled(state) => format!("{text} ({})", k.text()),
            _ => text,
        }
    }

    /// The first place on the ribbon as a path, such as "Solid › Create".
    pub fn path(&self) -> Option<String> {
        self.ribbon.first().map(|p| format!("{} › {}", p.tab.label(), p.panel))
    }

    /// The agent tool's description: the doc, then the details.
    pub fn description(&self) -> String {
        if self.details.is_empty() { format!("{}.", self.doc) } else { format!("{}. {}", self.doc, self.details) }
    }

    /// The command as an agent tool.
    pub fn tool_spec(&self) -> ToolSpec {
        let mut description = self.description();
        if self.agents == Agents::Verb {
            description.push_str(" Runs at once, as one undo step.");
        } else if let Some(undo) = self.undo.sentence() {
            description.push(' ');
            description.push_str(undo);
        }
        ToolSpec { name: self.id.into(), description, input_schema: object(&(self.params)()) }
    }
}

/// Every command, in ribbon order.
pub fn all() -> &'static [CommandSpec] {
    static REGISTRY: LazyLock<Vec<CommandSpec>> = LazyLock::new(registry);
    &REGISTRY
}

/// The command with this id.
pub fn get(id: &str) -> Option<&'static CommandSpec> {
    all().iter().find(|c| c.id == id)
}

/// The commands with an agent tool.
pub fn for_agents() -> impl Iterator<Item = &'static CommandSpec> {
    all().iter().filter(|c| c.agents.has_tool())
}

/// The recipe verbs.
pub fn verbs() -> impl Iterator<Item = &'static CommandSpec> {
    all().iter().filter(|c| c.agents == Agents::Verb)
}

/// A tab's ribbon: its panels in order, each with its commands and their sizes.
pub fn ribbon(tab: impl Into<Tab>) -> Vec<(&'static str, Vec<(&'static CommandSpec, Size)>)> {
    let tab = tab.into();
    panels(tab)
        .iter()
        .map(|panel| {
            let items = all()
                .iter()
                .flat_map(|c| c.ribbon.iter().filter(|p| p.tab == tab && p.panel == *panel).map(move |p| (c, p.size)))
                .collect();
            (*panel, items)
        })
        .collect()
}

/// The shortcuts that apply in a context, in the order to try them: more modifiers
/// first, then narrower scopes first.
pub fn bindings(context: Context) -> Vec<(&'static CommandSpec, Shortcut)> {
    let mut out: Vec<(&'static CommandSpec, Shortcut)> = all()
        .iter()
        .flat_map(|c| c.keys.iter().map(move |k| (c, *k)))
        .filter(|(_, k)| k.scope.applies(context))
        .collect();
    out.sort_by_key(|(_, k)| (std::cmp::Reverse(k.modifier_count()), !k.scope.is_narrow()));
    out
}

/// The commands a person ran lately and the ones they pinned, by id. The palette shows
/// them before anything is typed; the settings file keeps them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    /// Pinned commands, in the order they were pinned.
    pub pinned: Vec<String>,
    /// Recently run commands, the latest first.
    pub recent: Vec<String>,
}

impl History {
    /// How many recent commands are kept.
    pub const RECENT: usize = 12;

    /// Notes that a command ran.
    pub fn record(&mut self, id: &str) {
        self.recent.retain(|r| r != id);
        self.recent.insert(0, id.to_owned());
        self.recent.truncate(Self::RECENT);
    }

    pub fn is_pinned(&self, id: &str) -> bool {
        self.pinned.iter().any(|p| p == id)
    }

    pub fn pin(&mut self, id: &str) {
        if !self.is_pinned(id) {
            self.pinned.push(id.to_owned());
        }
    }

    pub fn unpin(&mut self, id: &str) {
        self.pinned.retain(|p| p != id);
    }

    /// The place in the recent list, the latest at 0.
    pub fn recency(&self, id: &str) -> Option<usize> {
        self.recent.iter().position(|r| r == id)
    }

    /// Drops ids no command has, such as one a newer build added.
    pub fn retain_known(&mut self) {
        self.pinned.retain(|id| get(id).is_some());
        self.recent.retain(|id| get(id).is_some());
    }
}

/// A search result.
#[derive(Clone, Copy, Debug)]
pub struct Match {
    pub command: &'static CommandSpec,
    pub pinned: bool,
    /// The place in the recent list, the latest at 0.
    pub recent: Option<usize>,
    /// Higher is better.
    pub score: u32,
}

impl std::fmt::Debug for CommandSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandSpec").field("id", &self.id).finish_non_exhaustive()
    }
}

/// Searches the app's commands. An empty query lists the pinned commands, then the
/// recent ones. Otherwise every word must match the label, the id, the ribbon path or
/// the start of a word in the doc; label matches rank first, then pinned and recent commands, then table order.
pub fn search(query: &str, history: &History) -> Vec<Match> {
    search_in(all().iter().filter(|c| c.in_app()), query, history)
}

/// [`search`] over the commands the app offers in a context, as the palette searches.
pub fn search_here(query: &str, history: &History, context: Context) -> Vec<Match> {
    search_in(all().iter().filter(move |c| c.available(context)), query, history)
}

/// [`search`] over any set of commands, such as all of them for the CLI.
pub fn search_in<'a>(
    commands: impl Iterator<Item = &'static CommandSpec> + 'a,
    query: &str,
    history: &History,
) -> Vec<Match> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut found: Vec<Match> = commands
        .filter_map(|c| {
            let pinned = history.is_pinned(c.id);
            let recent = history.recency(c.id);
            let score = if words.is_empty() {
                if pinned || recent.is_some() { Some(0) } else { None }
            } else {
                words.iter().map(|w| word_score(c, w)).sum::<Option<u32>>()
            }?;
            Some(Match { command: c, pinned, recent, score })
        })
        .collect();
    let order = |m: &Match| all().iter().position(|c| c.id == m.command.id).unwrap_or(usize::MAX);
    found.sort_by_key(|m| {
        (std::cmp::Reverse(m.score), !m.pinned, m.recent.unwrap_or(usize::MAX), history_pin_order(history, m), order(m))
    });
    found
}

fn history_pin_order(history: &History, m: &Match) -> usize {
    history.pinned.iter().position(|p| p == m.command.id).unwrap_or(usize::MAX)
}

/// How well one lowercase word matches a command, or `None` when it does not.
fn word_score(c: &CommandSpec, word: &str) -> Option<u32> {
    let label = c.label.to_lowercase();
    if label == word {
        return Some(100);
    }
    if label.starts_with(word) {
        return Some(80);
    }
    if label.split(|ch: char| !ch.is_alphanumeric()).any(|w| w.starts_with(word)) {
        return Some(60);
    }
    if label.contains(word) {
        return Some(40);
    }
    if c.id.contains(word) {
        return Some(30);
    }
    if c.path().is_some_and(|p| p.to_lowercase().contains(word)) {
        return Some(20);
    }
    c.doc.to_lowercase().split(|ch: char| !ch.is_alphanumeric()).any(|w| w.starts_with(word)).then_some(10)
}

fn session(s: &AppState) -> Option<&DrawingSession> {
    s.drawing.session.as_ref()
}

fn sheet_view(s: &AppState) -> Option<&SheetView> {
    session(s).and_then(DrawingSession::selected_view)
}

fn drawing_tool_is(s: &AppState, f: fn(&DrawingTool) -> bool) -> bool {
    session(s).is_some_and(|d| f(&d.tool))
}

/// The active document's undo and redo labels.
fn undo_labels(s: &AppState) -> (Option<String>, Option<String>) {
    if let Some(a) = s.assembly.session.as_ref().filter(|_| s.assembly.is_active()) {
        return (a.doc.undo_label().map(str::to_owned), a.doc.redo_label().map(str::to_owned));
    }
    match session(s).filter(|_| s.drawing.is_active()) {
        Some(d) => (d.doc.undo_label().map(str::to_owned), d.doc.redo_label().map(str::to_owned)),
        None => (s.doc.undo_label().map(str::to_owned), s.doc.redo_label().map(str::to_owned)),
    }
}

/// A drawing tool's row: picks the tool, and shows as on while it is the tool.
macro_rules! drawing_tool {
    ($id:expr, $label:expr, $icon:expr, $doc:expr, $tool:pat => $make:expr) => {
        CommandSpec::new($id, $label, $icon, $doc)
            .drawing()
            .run(|_| AppAction::Drawing(DrawingAction::SetTool($make)))
            .lit(|s| drawing_tool_is(s, |t| matches!(t, $tool)))
    };
}

fn has_body(s: &AppState) -> bool {
    s.doc.display_body().is_some()
}

/// Why no CAD feature may be added now: an active lattice or gyroid is the last feature.
/// The same typed policy refuses the add in the document; the kernel's is
/// `lattice.cad-after-lattice`.
fn after_field(s: &AppState) -> Option<String> {
    s.doc.model().cad_after_field().map(|e| e.to_string())
}

/// No terminal field stands in the way of adding a CAD feature.
fn field_open(s: &AppState) -> bool {
    after_field(s).is_none()
}

fn can_export(s: &AppState) -> bool {
    s.drag_preview.is_none()
        && !s.doc.proof_pending()
        && !s.doc.proof_message().is_some_and(|(_, failed)| failed)
        && s.doc.error_count() == 0
        && s.doc.repair_count() == 0
        && has_body(s)
}

fn editing(s: &AppState, f: fn(&EditorParams) -> bool) -> bool {
    s.editor.as_ref().is_some_and(|e| f(&e.params))
}

fn tool_is(s: &AppState, t: Tool) -> bool {
    s.sketch.as_ref().is_some_and(|k| k.tool == t)
}

fn selected_feature_is(s: &AppState, f: fn(&crate::document::Feature) -> bool) -> bool {
    s.selected_feature().and_then(|id| s.doc.feature(id)).is_some_and(f)
}

/// The text of another command's first shortcut, for hints that name it.
fn key_of(id: &str) -> String {
    get(id).and_then(CommandSpec::shortcut_text).unwrap_or_default()
}

fn registry() -> Vec<CommandSpec> {
    use RibbonTab::{Inspect, Sketch, Solid, View};
    use Size::{Large, Small};
    let none = Modifiers::NONE;
    let ctrl = Modifiers::COMMAND;
    let ctrl_shift = Modifiers::COMMAND | Modifiers::SHIFT;
    let cmd = CommandSpec::new;
    vec![
        cmd("document_command", "Document command", Icon::Tree, "Run a canonical part journal command").agents(
            Agents::Mcp,
            "Uses the application's model command loop. command_json holds the journal command.",
            tools::document_command_params,
        )
        .undo(Undo::Carried),
        cmd("drawing_command", "Drawing command", Icon::Tree, "Run a canonical drawing journal command").agents(
            Agents::Mcp,
            "Requires an open drawing. Accepts execute, undo and redo.",
            tools::document_command_params,
        )
        .undo(Undo::Carried),
        // Reading the part, for agents.
        cmd("model_summary", "Model summary", Icon::Tree, "The part as it is now").agents(
            Agents::Tool,
            "Every feature in tree order with its parameters and status, the body's bounding box, volume and \
             area, values and tracked checks, the selection, pending proposals and what undo would undo. Tracked rows request an entity proof after each accepted rebuild.",
            Vec::new,
        ),
        cmd("feature", "Feature details", Icon::Props, "One feature in detail").agents(
            Agents::Tool,
            concat!(
                "Its parameters, the faces it made (surface type, area, centroid, and for planar faces the outward \
                 normal and the sketch frame) and the edges it made (length, midpoint, the two faces it joins, and \
                 whether it is a convex outside edge or a concave inside corner). ",
                names_help!()
            ),
            tools::feature_params,
        ),
        cmd("selection", "Selection details", Icon::Search, "What the user has selected in the viewport or the tree")
            .agents(Agents::Tool, "By name, with its details. Use it when the user says this, that or here.", Vec::new),
        cmd("rollback", "Rollback", Icon::Rollback, "Move the rollback bar before a feature, or to the end").agents(
            Agents::Mcp,
            "Uses the same document rebuild as the tree rollback bar.",
            tools::rollback_params,
        ),
        // Edit.
        cmd("undo", "Undo", Icon::Glyph("undo"), "Undo the last change to the active document")
            .both()
            .on(DrawingTab::Views, "Edit", Small)
            .on(DrawingTab::Annotate, "Edit", Small)
            .on(DrawingTab::Sheet, "Edit", Small)
            .on(Tab::Assembly, "Edit", Small)
            .key(Scope::App, ctrl, Key::Z)
            .run(|_| AppAction::Undo)
            .when(|s| undo_labels(s).0.is_some())
            .hint(|s| Some(undo_labels(s).0.map(|l| format!("Undo {l}")).unwrap_or("Nothing to undo".into())))
            .agents(
                Agents::Mcp,
                "Whatever made it, as the Edit menu does: the drawing's step while the drawing is the active \
                 document, the assembly's while the assembly is, the part's otherwise.",
                Vec::new,
            )
.undo(Undo::Moves),
        cmd("redo", "Redo", Icon::Glyph("redo"), "Redo the last undone change")
            .both()
            .on(DrawingTab::Views, "Edit", Small)
            .on(DrawingTab::Annotate, "Edit", Small)
            .on(DrawingTab::Sheet, "Edit", Small)
            .on(Tab::Assembly, "Edit", Small)
            .key(Scope::App, ctrl, Key::Y)
            .key(Scope::App, ctrl_shift, Key::Z)
            .run(|_| AppAction::Redo)
            .when(|s| undo_labels(s).1.is_some())
            .hint(|s| Some(undo_labels(s).1.map(|l| format!("Redo {l}")).unwrap_or("Nothing to redo".into())))
            .agents(Agents::Mcp, "", Vec::new)
.undo(Undo::Moves),
        // Sketch.
        cmd("sketch", "Sketch", Icon::Cad("sketch"), "Sketch on a plane or a planar face")
            .on(Solid, "Sketch", Large)
            .on(Sketch, "Sketch", Large)
            .key(Scope::Model, none, Key::S)
            .run(|s| if s.in_sketch() { AppAction::ExitSketch(None) } else { AppAction::StartSketch })
            .when(|s| s.in_sketch() || field_open(s))
            .lit(AppState::in_sketch)
            .hint(|s| if s.in_sketch() { Some(format!("Exit the sketch ({})", key_of("confirm"))) } else { after_field(s) })
            .agents(
                Agents::Verb,
                "The sketch holds closed shapes (rectangles, circles, straight slots). Shapes may not overlap. \
                 Every shape becomes one profile for a later extrude or cut.",
                tools::sketch_params,
            )
.undo(Undo::Step),
        cmd("sketch_geometry", "Sketch geometry", Icon::Sketch, "Create a sketch using polygon and line/arc tools")
            .agents(
                Agents::Verb,
                "Build a sketch from analytic entities and polygon, trim, extend, offset, mirror or pattern operations. \
                 Coordinates are u/v on the sketch plane. Sketch arithmetic is tolerance-based; Catallax converts the profiles. \
                 Entity ids start at 1 in insertion order. Construction entities do not enter profiles. \
                 Copies are independent and instance counts include the seed. Extrude the resulting closed profiles with extrude.",
                tools::sketch_geometry_params,
            )
.undo(Undo::Step),
        cmd("exit_sketch", "Exit", Icon::Cad("exit_sketch"), "Exit the sketch")
            .on(Sketch, "Sketch", Large)
            .run(|_| AppAction::ExitSketch(None))
            .when(AppState::in_sketch)
.app_only(EDITOR),
        cmd("sketch_line", "Line", Icon::Cad("line"), "Line: click points; right-click ends the chain")
            .on(Sketch, "Draw", Small)
            .key(Scope::Sketch, none, Key::L)
            .run(|_| AppAction::SketchTool(Tool::Line))
            .when(AppState::in_sketch)
            .lit(|s| tool_is(s, Tool::Line))
.app_only(SKETCHER),
        cmd("sketch_circle", "Circle", Icon::Cad("circle"), "Circle: centre, then a point on it")
            .on(Sketch, "Draw", Small)
            .key(Scope::Sketch, none, Key::C)
            .run(|_| AppAction::SketchTool(Tool::Circle))
            .when(AppState::in_sketch)
            .lit(|s| tool_is(s, Tool::Circle))
.app_only(SKETCHER),
        cmd("sketch_rectangle", "Rectangle", Icon::Cad("rect"), "Corner rectangle")
            .on(Sketch, "Draw", Small)
            .key(Scope::Sketch, none, Key::R)
            .run(|_| AppAction::SketchTool(Tool::Rectangle))
            .when(AppState::in_sketch)
            .lit(|s| tool_is(s, Tool::Rectangle))
.app_only(SKETCHER),
        cmd("sketch_arc", "Arc", Icon::Cad("arc"), "Centre-point arc: centre, start, end")
            .on(Sketch, "Draw", Small)
            .key(Scope::Sketch, none, Key::A)
            .run(|_| AppAction::SketchTool(Tool::Arc))
            .when(AppState::in_sketch)
            .lit(|s| tool_is(s, Tool::Arc))
.app_only(SKETCHER),
        cmd("sketch_slot", "Slot", Icon::Cad("slot_sketch"), "Straight slot: two centres, then the width")
            .on(Sketch, "Draw", Small)
            .key(Scope::Sketch, none, Key::S)
            .run(|_| AppAction::SketchTool(Tool::Slot))
            .when(AppState::in_sketch)
            .lit(|s| tool_is(s, Tool::Slot))
.app_only(SKETCHER),
        cmd("sketch_point", "Point", Icon::Cad("sk_point"), "Point")
            .on(Sketch, "Draw", Small)
            .key(Scope::Sketch, none, Key::P)
            .run(|_| AppAction::SketchTool(Tool::Point))
            .when(AppState::in_sketch)
            .lit(|s| tool_is(s, Tool::Point))
.app_only(SKETCHER),
        cmd("sketch_select", "Select", Icon::Sketch, "Select and drag sketch geometry")
            .key(Scope::Sketch, none, Key::V)
            .run(|_| AppAction::SketchTool(Tool::Select))
            .when(AppState::in_sketch)
            .lit(|s| tool_is(s, Tool::Select))
.app_only(SKETCHER),
        cmd("smart_dimension", "Smart dim", Icon::Cad("smart_dim"), "Dimension a line, arc, circle, point or pair")
            .on(Sketch, "Constrain", Large)
            .key(Scope::Sketch, none, Key::D)
            .run(|_| AppAction::SketchTool(Tool::Dimension))
            .when(AppState::in_sketch)
            .lit(|s| tool_is(s, Tool::Dimension))
.app_only(SKETCHER),
        cmd("construction", "Construction", Icon::Cad("construction"), "Toggle construction geometry on the selection")
            .on(Sketch, "Constrain", Small)
            .key(Scope::Sketch, none, Key::X)
            .run(|_| AppAction::ToggleConstruction)
            .when(AppState::in_sketch)
.app_only(SKETCHER),
        cmd("relations", "Relations", Icon::Cad("relations"), "Relations between sketch entities")
            .on(Sketch, "Constrain", Small)
            .hint(|_| Some("Relations for the selection are in the property panel".into())),
        // Boss.
        cmd("extrude", "Extrude", Icon::Cad("extrude"), "Add material: extrude a sketch and join it to the part")
            .on(Solid, "Create", Large)
            .on(Sketch, "Use sketch", Large)
            .key(Scope::Model, none, Key::E)
            .run(
                |s| {
                    if s.in_sketch() { AppAction::ExitSketch(Some(ExitInto::Extrude)) } else { AppAction::StartExtrude }
                },
            )
            .when(field_open)
            .lit(|s| editing(s, |p| matches!(p, EditorParams::Extrude(_))))
            .hint(after_field)
            .agents(Agents::Verb, "Every profile of the sketch extrudes along its normal.", tools::extrude_params)
.undo(Undo::Step),
        cmd("revolve", "Revolve", Icon::Cad("revolve"), "Add material: revolve a sketch about an axis")
            .on(Solid, "Create", Large)
            .on(Sketch, "Use sketch", Large)
            .run(|s| if s.in_sketch() { AppAction::ExitSketch(Some(ExitInto::Revolve)) } else { AppAction::StartRevolve })
            .when(|s| field_open(s) && (s.in_sketch() || s.selected_sketch().is_some()))
            .lit(|s| editing(s, |p| matches!(p, EditorParams::Revolve(_))))
            .hint(|s| {
                after_field(s).or_else(|| {
                    (!s.in_sketch() && s.selected_sketch().is_none()).then(|| "Select a sketch to revolve".into())
                })
            })
            .agents(Agents::Verb, "Revolve a closed meridian sketch about a model-space axis.", tools::revolve_params)
            .undo(Undo::Step),
        cmd("edit_revolve", "Edit revolve", Icon::Revolve, "Change a revolution angle")
            .agents(Agents::Verb, "Preserves the sketch and axis.", tools::edit_revolve_params)
            .undo(Undo::Step),
        cmd("sweep", "Sweep", Icon::Cad("sweep"), "Add material: sweep a profile along a path").on(Solid, "Create", Small),
        cmd("loft", "Loft", Icon::Cad("loft"), "Add material: loft between profiles").on(Solid, "Create", Small),
        // Cut.
        cmd("cut", "Cut", Icon::Cad("cut"), "Remove material: cut a sketch into the part")
            .on(Solid, "Remove", Large)
            .on(Sketch, "Use sketch", Large)
            .key(Scope::Model, none, Key::X)
            .run(|s| if s.in_sketch() { AppAction::ExitSketch(Some(ExitInto::Cut)) } else { AppAction::StartCut })
            .when(|s| field_open(s) && (s.in_sketch() || (has_body(s) && s.selected_sketch().is_some())))
            .lit(|s| editing(s, |p| matches!(p, EditorParams::Cut(_))))
            .hint(|s| {
                if let Some(why) = after_field(s) {
                    Some(why)
                } else if s.in_sketch() || (has_body(s) && s.selected_sketch().is_some()) {
                    None
                } else if has_body(s) {
                    Some("Select a sketch to cut with, or sketch on a face first".into())
                } else {
                    Some("Cut needs a body and a sketch".into())
                }
            })
            .agents(
                Agents::Verb,
                "Every profile cuts against the sketch normal (into the face it sits on) unless flipped. Several \
                 circles in one sketch cut several holes at once.",
                tools::cut_params,
            )
            .undo(Undo::Step),
        cmd("hole", "Hole", Icon::Cad("hole"), "Drill holes into a planar face, with a counterbore or countersink")
            .on(Solid, "Remove", Large)
            .run(|_| AppAction::StartHole)
            .when(|s| has_body(s) && field_open(s))
            .lit(|s| editing(s, |p| matches!(p, EditorParams::Hole(_))))
            .hint(|s| after_field(s).or_else(|| (!has_body(s)).then(|| "Hole needs a body".into())))
            .agents(
                Agents::Verb,
                "Each point is a hole centre on the face. Holes drill against the face's outward normal. Standard \
                 angles: 118 for drill tips; 82, 90 and 100 for countersinks.",
                tools::hole_params,
            )
            .undo(Undo::Step),
        cmd("slot", "Slot", Icon::Cad("slot"), "Cut a slot").on(Solid, "Remove", Large),
        // Finish.
        cmd("fillet", "Fillet", Icon::Cad("fillet"), "Round edges with a circular fillet of one radius")
            .on(Solid, "Modify", Large)
            .key(Scope::Edge, none, Key::F)
            .run(|_| AppAction::StartFillet)
            .when(|s| has_body(s) && field_open(s))
            .lit(|s| editing(s, |p| matches!(p, EditorParams::Fillet(_))))
            .hint(|s| after_field(s).or_else(|| (!has_body(s)).then(|| "Fillet needs a body".into())))
            .agents(Agents::Verb, "", tools::fillet_params)
.undo(Undo::Step),
        cmd("chamfer", "Chamfer", Icon::Cad("chamfer"), "Bevel edges with a flat chamfer of one distance")
            .on(Solid, "Modify", Large)
            .key(Scope::Model, none, Key::C)
            .run(|_| AppAction::StartChamfer)
            .when(|s| has_body(s) && field_open(s))
            .lit(|s| editing(s, |p| matches!(p, EditorParams::Chamfer(_))))
            .hint(|s| after_field(s).or_else(|| (!has_body(s)).then(|| "Chamfer needs a body".into())))
            .agents(Agents::Verb, "", tools::chamfer_params)
.undo(Undo::Step),
        cmd("lattice", "Lattice", Icon::Cad("lattice"), "Make a terminal cubic lattice")
            .on(Solid, "Modify", Small)
            .run(|_| AppAction::StartLattice)
            .when(|s| has_body(s) && field_open(s))
            .lit(|s| editing(s, |p| matches!(p, EditorParams::Lattice(_))))
            .hint(|s| after_field(s).or_else(|| (!has_body(s)).then(|| "Lattice needs a body".into())))
            .agents(Agents::Verb, "Density is the constant beam radius. The preview is unproved; export certifies the mesh.", tools::lattice_params)
            .undo(Undo::Step),
        cmd("edit_lattice", "Edit lattice", Icon::Cad("lattice"), "Change lattice density and output settings")
            .agents(Agents::Verb, "Only supplied settings change.", tools::edit_lattice_params)
            .undo(Undo::Step),
        cmd("gyroid", "Gyroid", Icon::Cad("lattice"), "Make a terminal network gyroid")
            .on(Solid, "Modify", Small)
            .run(|_| AppAction::StartGyroid)
            .when(|s| has_body(s) && field_open(s))
            .lit(|s| editing(s, |p| matches!(p, EditorParams::Gyroid(_))))
            .hint(|s| after_field(s).or_else(|| (!has_body(s)).then(|| "Gyroid needs a body".into())))
            .agents(Agents::Verb, "The level is dimensionless, not a thickness, and may be zero or negative. The preview is unproved; export certifies the mesh.", tools::gyroid_params)
            .undo(Undo::Step),
        cmd("edit_gyroid", "Edit gyroid", Icon::Cad("lattice"), "Change gyroid period, level and output settings")
            .agents(Agents::Verb, "Only supplied settings change.", tools::edit_gyroid_params)
            .undo(Undo::Step),
        cmd("grade_field", "Grade field", Icon::Cad("lattice"), "Grade a lattice's density or a gyroid's level along a part axis")
            .agents(
                Agents::Mcp,
                "A lattice's beam radius in millimetres, which controls density, or a gyroid's dimensionless network \
                 level, not a thickness, is start at part coordinate from on the axis and end at to, linear between \
                 and held beyond them. The anchors stay put when the part's size changes. edit_lattice radius or \
                 edit_gyroid level makes it one value again. Grades the kernel cannot certify fail with its typed \
                 coverage code.",
                tools::grade_field_params,
            )
            .undo(Undo::Step),
        cmd("field_mass", "Field mass", Icon::Cad("mass"), "Prove the active lattice or gyroid's volume, centre of gravity and inertia")
            .agents(
                Agents::Tool,
                "Unit-density moments of the field's own definition, as exact rational enclosures: volume within \
                 0.01 mm³, each centroid coordinate within 0.001 mm and each centroidal inertia component within \
                 0.1 mm⁵. Never computed from the preview or a mesh, and only when asked for.",
                Vec::new,
            ),
        cmd("shell", "Shell", Icon::Cad("shell"), "Hollow the part, leaving walls of one thickness")
            .on(Solid, "Modify", Small)
            .run(|_| AppAction::StartShell)
            .when(|s| has_body(s) && field_open(s))
            .lit(|s| editing(s, |p| matches!(p, EditorParams::Shell(_))))
            .hint(|s| after_field(s).or_else(|| (!has_body(s)).then(|| "Shell needs a body".into())))
            .agents(Agents::Verb, "Selected faces are left open. An empty list makes a closed shell.", tools::shell_params)
            .undo(Undo::Step),
        cmd("draft", "Draft", Icon::Cad("draft"), "Taper faces by an angle about a neutral plane")
            .on(Solid, "Modify", Small)
            .run(|_| AppAction::StartDraft)
            .when(|s| has_body(s) && field_open(s))
            .lit(|s| editing(s, |p| matches!(p, EditorParams::Draft(_))))
            .hint(|s| after_field(s).or_else(|| (!has_body(s)).then(|| "Draft needs a body".into())))
            .agents(
                Agents::Verb,
                "The pull direction is the neutral plane's normal, or its reverse. Catallax drafts planar faces and \
                 cylinders parallel to the pull.",
                tools::draft_params,
            )
            .undo(Undo::Step),
        // Pattern.
        cmd("linear_pattern", "Linear", Icon::Cad("linear"), "Copy the part's body along a direction")
            .on(Solid, "Pattern", Large)
            .run(|_| AppAction::StartPattern { circular: false })
            .when(|s| has_body(s) && field_open(s))
            .lit(|s| editing(s, |p| matches!(p, EditorParams::Pattern(f) if matches!(f.layout, soliddraft_model::PatternSpec::Linear { .. }))))
            .hint(|s| after_field(s).or_else(|| (!has_body(s)).then(|| "Pattern needs a body".into())))
            .agents(Agents::Verb, "The copies unite with the body.", tools::linear_pattern_params)
            .undo(Undo::Step),
        cmd("circular_pattern", "Circular", Icon::Cad("circular"), "Copy the part's body around an axis")
            .on(Solid, "Pattern", Large)
            .run(|_| AppAction::StartPattern { circular: true })
            .when(|s| has_body(s) && field_open(s))
            .lit(|s| editing(s, |p| matches!(p, EditorParams::Pattern(f) if matches!(f.layout, soliddraft_model::PatternSpec::Circular { .. }))))
            .hint(|s| after_field(s).or_else(|| (!has_body(s)).then(|| "Pattern needs a body".into())))
            .agents(Agents::Verb, "The copies unite with the body.", tools::circular_pattern_params)
            .undo(Undo::Step),
        cmd("mirror", "Mirror", Icon::Cad("mirror"), "Mirror the part's body in a plane")
            .on(Solid, "Pattern", Large)
            .run(|_| AppAction::StartMirror)
            .when(|s| has_body(s) && field_open(s))
            .lit(|s| editing(s, |p| matches!(p, EditorParams::Mirror(_))))
            .hint(|s| after_field(s).or_else(|| (!has_body(s)).then(|| "Mirror needs a body".into())))
            .agents(Agents::Verb, "", tools::mirror_params)
            .undo(Undo::Step),
        // Reference geometry.
        cmd("reference_plane", "Plane", Icon::Cad("plane"), "Add a reference plane").on(Solid, "Reference", Small),
        cmd("reference_axis", "Axis", Icon::Cad("axis"), "Add a reference axis").on(Solid, "Reference", Small),
        cmd("reference_point", "Point", Icon::Cad("refpoint"), "Add a reference point").on(Solid, "Reference", Small),
        // Drawing.
        cmd(
            "make_drawing",
            "Make drawing",
            Icon::Cad("drawing"),
            "Make a 2D drawing of this part: template, scale and views",
        )
        .key(Scope::Part, ctrl_shift, Key::D)
        .run(|_| AppAction::Drawing(DrawingAction::MakeDrawing))
        .when(has_body)
        .lit(|s| s.drawing.new.is_some())
        .hint(|s| (!has_body(s)).then(|| "Make drawing needs a body".into()))
.agents(
            Agents::Mcp,
            "With the template's default views (Front, Top, Left and Isometric) fitted to the sheet and centre marks \
             on the orthographic views, in place of an open drawing. The drawing becomes the active document: undo \
             and redo work on it until drawing_show_part.",
            app::make_drawing_params,
        ).undo(Undo::Replaces),
        cmd("show_drawing", "Open drawings", Icon::Cad("open_drawings"), "Bring the open drawing forward")
            .run(|_| AppAction::Drawing(DrawingAction::ShowDrawing))
            .when(|s| s.drawing.session.is_some())
.agents(Agents::Mcp, "Undo and redo then work on the drawing.", Vec::new),
        // The assistant.
        cmd("ask", "Ask", Icon::Spark, "Ask the assistant")
            .both()
            .on(DrawingTab::Views, "Assistant", Large)
            .on(DrawingTab::Annotate, "Assistant", Large)
            .on(DrawingTab::Assistant, "Chat", Large)
            .key(Scope::Model, none, Key::Slash)
            .key(Scope::Drawing, none, Key::Slash)
            .run(|_| AppAction::FocusChat)
.app_only(CHAT),
        cmd("check_model", "Check model", Icon::Check, "Ask the assistant to check the part")
            .run(|_| {
                AppAction::Ask("Check the part: does every feature rebuild, and is anything likely to be wrong?".into())
            })
.app_only(CHAT),
        cmd("explain", "Explain", Icon::Chat, "Ask the assistant to explain the selection")
            .run(|_| AppAction::Ask("Explain what is selected and how it was made.".into()))
.app_only(CHAT),
        cmd("summarise", "Summarise", Icon::Mass, "Ask the assistant to summarise the part")
            .run(|_| AppAction::Ask("Summarise the part in a few lines.".into()))
.app_only(CHAT),
        cmd("find_issues", "Find issues", Icon::Warning, "Ask the assistant for features that fail or look wrong")
            .run(|_| AppAction::Ask("List features that fail or look wrong, and why.".into()))
.app_only(CHAT),
        cmd(
            "provider_settings",
            "Provider",
            Icon::Config,
            "Choose the assistant's provider and model, and add API keys",
        )
        .run(|_| AppAction::ShowProviderSettings)
.app_only("It opens the Provider dialog; `soliddraft key` manages the assistant's keys without a window"),
        cmd("auto_apply", "Auto-apply", Icon::Eye, "Let the assistant apply its own proposals for this session")
            .run(|_| AppAction::ToggleAutoApply)
            .lit(|s| s.chat.book.auto_apply)
.app_only("It sets what the app's own assistant may do this session; MCP clients and scripts run commands directly"),
        // Inspect.
        cmd("measure", "Measure", Icon::Cad("measure"), "Prove a distance or angle between two edges or faces")
            .on(Inspect, "Measure", Large)
            .run(|_| AppAction::MeasureEntity)
            .agents(
                Agents::Tool,
                concat!(
                    "Proves one scalar between two entities of the part, in the part frame, with its bounds and \
                 certainty, without changing the part. minimum_distance (the default) is the finite minimum \
                 between two straight edges or two parallel planar faces; point_distance joins two straight \
                 edges' midpoints (a_point and b_point: midpoint); direction_angle is 0 or 90 degrees between \
                 parallel or perpendicular straight edges. Other pairs are refused by name, never estimated. \
                 distance_mm repeats a minimum distance and gap_mm a face gap. ",
                    names_help!()
                ),
                tools::measure_params,
            ),
        cmd(
            "propose",
            "Propose",
            Icon::Spark,
            "Propose a change as recipe lines that run in order, with a one-line summary",
        )
        .agents(
            Agents::Tool,
            "The lines are tried on a copy of the part first: a line whose reference does not resolve, or a result \
             that breaks the rebuild, is rejected with the reason. An accepted proposal appears to the user as a \
             card with Preview, Apply and Discard; nothing changes until it is applied, and it applies as one undo \
             step.",
            tools::propose_params,
        ),
        cmd("apply", "Apply proposal", Icon::Check, "Apply a pending proposal through the command layer").agents(
            Agents::Tool,
            "As the user's Apply button does. The assistant may call this only when the user turned on auto-apply \
             for the session.",
            tools::apply_params,
        )
.undo(Undo::Step),
        cmd(
            "mass_properties",
            "Mass props",
            Icon::Cad("mass"),
            "Calculate mass properties of the selected feature",
        )
        .on(Inspect, "Measure", Large)
        .run(|_| AppAction::MeasureMassProperties)
.agents(
            Agents::Tool,
            "Calculate volume in mm³, surface area in mm² and centre of mass after the named feature. \
             Editing and model_summary report previously requested body properties. Tracked rows are standing requests for their own entity proofs.",
            tools::feature_params,
        ),
        cmd("check", "Check", Icon::Cad("check"), "Check the body for geometry faults").on(Inspect, "Check", Large),
        cmd("section", "Section", Icon::Cad("section"), "Cut the view open on a plane").on(Inspect, "Check", Small),
        cmd("draft_check", "Draft check", Icon::Cad("draft_check"), "Show which faces have enough draft")
            .on(Inspect, "Surface quality", Small),
        // View.
        cmd("view_isometric", "Isometric", Icon::Glyph("iso"), "Isometric view")
            .on(View, "Orientation", Large)
            .key(Scope::Model, none, Key::I)
            .run(|_| AppAction::SetView(StandardView::Isometric))
.app_only(CAMERA),
        cmd("zoom_fit", "Fit", Icon::Glyph("fit"), "Zoom to fit")
            .both()
            .on(View, "Orientation", Large)
            .on(DrawingTab::View, "Zoom", Large)
            .on(Tab::Assembly, "View", Large)
            .key(Scope::Part, none, Key::Z)
            .key(Scope::Drawing, none, Key::Z)
            .key(Scope::Drawing, none, Key::F)
            .key(Scope::Assembly, none, Key::Z)
            .run(|s| {
                if s.assembly.is_active() {
                    AppAction::Assembly(AssemblyAction::ZoomFit)
                } else if s.drawing.is_active() {
                    AppAction::Drawing(DrawingAction::ZoomFit)
                } else {
                    AppAction::ZoomFit
                }
            })
.app_only(CAMERA),
        cmd("view_front", "Front", Icon::Glyph("iso"), "Front view")
            .on(View, "Orientation", Small)
            .key(Scope::Model, none, Key::F)
            .run(|_| AppAction::SetView(StandardView::Front))
.app_only(CAMERA),
        cmd("view_top", "Top", Icon::Glyph("iso"), "Top view")
            .on(View, "Orientation", Small)
            .key(Scope::Model, none, Key::T)
            .run(|_| AppAction::SetView(StandardView::Top))
.app_only(CAMERA),
        cmd("view_right", "Right", Icon::Glyph("iso"), "Right view")
            .on(View, "Orientation", Small)
            .key(Scope::Model, none, Key::R)
            .run(|_| AppAction::SetView(StandardView::Right))
.app_only(CAMERA),
        cmd("normal_to", "Normal to", Icon::Cad("normal_to"), "Look straight at the sketch plane or the selected face")
            .on(Sketch, "View", Large)
            .on(View, "Orientation", Small)
            .key(Scope::Part, none, Key::N)
            .run(|_| AppAction::NormalTo)
.app_only(CAMERA),
        cmd("view_back", "Back", Icon::Glyph("iso"), "Back view").run(|_| AppAction::SetView(StandardView::Back))
.app_only(CAMERA),
        cmd("view_left", "Left", Icon::Glyph("iso"), "Left view").run(|_| AppAction::SetView(StandardView::Left))
.app_only(CAMERA),
        cmd("view_bottom", "Bottom", Icon::Glyph("iso"), "Bottom view").run(|_| AppAction::SetView(StandardView::Bottom))
.app_only(CAMERA),
        cmd("display_shaded_edges", "Shaded, edges", Icon::Views, "Shaded with edges")
            .on(View, "Display", Large)
            .run(|_| AppAction::SetDisplay(DisplayMode::ShadedWithEdges))
            .lit(|s| s.viewport.display == DisplayMode::ShadedWithEdges)
.app_only(LOOK),
        cmd("display_shaded", "Shaded", Icon::Display, "Shaded")
            .on(View, "Display", Large)
            .run(|_| AppAction::SetDisplay(DisplayMode::Shaded))
            .lit(|s| s.viewport.display == DisplayMode::Shaded)
.app_only(LOOK),
        cmd("display_wireframe", "Wireframe", Icon::Wireframe, "Wireframe")
            .on(View, "Display", Large)
            .run(|_| AppAction::SetDisplay(DisplayMode::Wireframe))
            .lit(|s| s.viewport.display == DisplayMode::Wireframe)
.app_only(LOOK),
        cmd("look_fast", "Fast look", Icon::Display, "Colour, lighting and edges, without shadows or occlusion")
            .run(|_| AppAction::SetPreset(Preset::Fast))
            .lit(|s| s.viewport.preset == Preset::Fast)
.app_only(LOOK),
        cmd("look_studio", "Studio look", Icon::Display, "Soft shadows from a fixed light and light occlusion")
            .run(|_| AppAction::SetPreset(Preset::Studio))
            .lit(|s| s.viewport.preset == Preset::Studio)
.app_only(LOOK),
        cmd("look_realistic", "Realistic look", Icon::Display, "Stronger shadows and reflections, without edges")
            .run(|_| AppAction::SetPreset(Preset::Realistic))
            .lit(|s| s.viewport.preset == Preset::Realistic)
.app_only(LOOK),
        cmd("theme_light", "Light theme", Icon::Display, "Light chrome and panels; saved in the settings")
            .run(|_| AppAction::SetTheme(crate::theme::Mode::Light))
            .lit(|_| crate::theme::mode() == crate::theme::Mode::Light)
.app_only(LOOK),
        cmd("theme_dark", "Dark theme", Icon::Display, "Dark chrome and panels; saved in the settings")
            .run(|_| AppAction::SetTheme(crate::theme::Mode::Dark))
            .lit(|_| crate::theme::mode() == crate::theme::Mode::Dark)
.app_only(LOOK),
        cmd("orthographic", "Orthographic", Icon::OnPlane, "Toggle perspective and orthographic projection")
            .on(View, "Projection", Large)
            .run(|_| AppAction::ToggleProjection)
            .lit(|s| s.viewport.camera.orthographic)
.app_only(CAMERA),
        cmd("toggle_tree", "Feature tree", Icon::Tree, "Show or hide the feature tree")
            .both()
            .on(View, "Panels", Large)
            .on(DrawingTab::View, "Panels", Large)
            .key_anytime(Scope::App, none, Key::F9)
            .run(|_| AppAction::TogglePanel(Side::Tree))
            .lit(|s| s.layout.tree.shown)
            .label_now(|s| s.drawing.is_active().then(|| "Drawing tree".to_owned()))
.app_only(PANELS),
        cmd(
            crate::command_line::AI_COMMAND,
            "AI session",
            Icon::Chat,
            "Open or close the assistant session above the command line",
        )
        .both()
        .on(View, "Panels", Large)
        .on(DrawingTab::View, "Panels", Large)
        .key_anytime(Scope::App, Modifiers::SHIFT, Key::F9)
        .run(|_| AppAction::ToggleSession)
        .lit(|s| s.command_line.session_open)
.app_only(PANELS),
        cmd("reset_layout", "Reset layout", Icon::Config, "Restore the default panel layout")
            .both()
            .on(View, "Panels", Small)
            .run(|_| AppAction::ResetLayout)
.app_only(PANELS),
        // Files.
        cmd("new_part", "New part", Icon::Glyph("new"), "Start a new, empty part")
            .key(Scope::Part, ctrl, Key::N)
            .run(|_| AppAction::NewPart)
.agents(Agents::Mcp, "In place of the open part, which is not saved first.", Vec::new).undo(Undo::Replaces),
        cmd("open", "Open", Icon::Glyph("open"), "Open a part, drawing or assembly; a drawing opens its part too")
            .key(Scope::Part, ctrl, Key::O)
            .run(|_| AppAction::Open(None))
            .agents(
                Agents::Mcp,
                "A part file opens in place of the current part. A drawing file opens in place of an open drawing, \
                 with its part when the reference finds it, and becomes the active document. An assembly opens \
                 beside the part and becomes active.",
                app::open_params,
            )
.undo(Undo::Replaces),
        cmd("preview_import", "Preview STEP import", Icon::Import, "Read a STEP file's proposed repairs")
            .agents(Agents::Mcp, "Reads the source without changing the part.", crate::import::preview_params),
        cmd("import_step", "Import STEP", Icon::Import, "Import one STEP body after reviewing its repairs")
            .run(|_| AppAction::ImportStep(None))
            .when(|s| !s.doc.proof_pending() && !cfg!(target_arch = "wasm32"))
            .agents(Agents::Mcp, "Automatic repairs take no answer. Declined gap ids are saved with the source.", crate::import::params)
            .undo(Undo::Step),
        cmd("save", "Save", Icon::Glyph("save"), "Save the part")
            .key(Scope::Part, ctrl, Key::S)
            .run(|_| AppAction::Save)
            .agents(Agents::Mcp, "Saves the part to the file.", tools::part_path_params),
        cmd("save_as", "Save as", Icon::Glyph("save"), "Save the part under a new name")
            .key(Scope::Part, ctrl_shift, Key::S)
            .run(|_| AppAction::SaveAs(None))
.agents(Agents::Mcp, "Saves the part to the file, as save does.", tools::part_path_params),
        cmd("export_step", "STEP", Icon::Save, "Export the part as STEP AP242, with its proven deviation")
            .run(|_| AppAction::Export { format: ExportFormat::Step, path: None })
            .when(can_export)
.agents(Agents::Mcp, "The result carries the deviation report.", app::step_path_params),
        cmd("export_stl", "STL", Icon::Save, "Export the part's mesh as binary STL at the export tolerance")
            .run(|_| AppAction::Export { format: ExportFormat::Stl, path: None })
            .when(can_export)
.agents(Agents::Mcp, "The result carries the mesh report.", app::stl_path_params),
        cmd("export_3mf", "3MF", Icon::Save, "Export the part's mesh as 3MF at the export tolerance")
            .run(|_| AppAction::Export { format: ExportFormat::ThreeMf, path: None })
            .when(can_export)
.agents(Agents::Mcp, "The result carries the mesh report.", app::three_mf_path_params),
        cmd("export_obj", "OBJ", Icon::Save, "Export the part's mesh as OBJ at the export tolerance")
            .run(|_| AppAction::Export { format: ExportFormat::Obj, path: None })
            .when(can_export)
.agents(Agents::Mcp, "The result carries the mesh report.", app::obj_path_params),
        cmd("export_stand_in", "Mass stand-in", Icon::Save, "Export the field's mass stand-in as STEP, not its boundary")
            .run(|_| AppAction::Export { format: ExportFormat::StandIn, path: None })
            .when(|s| can_export(s) && s.doc.model().active_field().is_some())
            .hint(|s| s.doc.model().active_field().is_none().then(|| "A mass stand-in needs a lattice or gyroid".into()))
            .agents(
                Agents::Mcp,
                "A massless surface envelope of the field's source region and unit-density proxy solids whose \
                 proven moments lie inside the field's. The result carries the moment report.",
                app::step_path_params,
            ),
        cmd("quit", "Quit", Icon::Close, "Quit SolidDraft")
            .both()
            .key(Scope::App, ctrl, Key::Q)
            .run(|_| AppAction::Quit)
.app_only("It closes the window"),
        // Features in the tree.
        cmd("edit_feature", "Edit feature", Icon::Props, "Edit the selected feature or sketch")
            .run(|s| {
                let id = s.selected_feature().expect("enabled with a selected feature");
                if selected_feature_is(s, |f| f.is_sketch()) {
                    AppAction::EditSketch(id)
                } else {
                    AppAction::EditFeature(id)
                }
            })
            .when(|s| s.selected_feature().is_some())
.app_only("It opens the feature's PropertyManager; agents and scripts change a feature with edit_extrude, edit_revolve, edit_cut, edit_fillet, edit_chamfer and edit_dimension"),
        cmd("rename", "Rename", Icon::Props, "Rename the selected feature")
            .key(Scope::Part, none, Key::F2)
            .run(|_| AppAction::RenameSelected)
            .agents(Agents::Mcp, "References to the feature's faces keep working.", tools::rename_params)
.undo(Undo::Step),
        cmd("rename_plane", "Rename plane", Icon::Plane, "Rename a reference plane").agents(
            Agents::Mcp,
            "Sketches keep their datum identity when its display name changes.",
            tools::rename_plane_params,
        )
.undo(Undo::Step),
        cmd("set_value", "Set value", Icon::Props, "Create or change a named value").agents(
            Agents::Verb, "Values drive bound dimensions; cycles and incompatible units are refused.", tools::set_value_params,
        ).undo(Undo::Step),
        cmd("drive", "Drive input", Icon::Props, "Drive a dimension or extrusion depth with an expression").agents(
            Agents::Verb, "Choose a sketch dimension or an extrusion depth.", tools::drive_params,
        ).undo(Undo::Step),
        cmd("rename_value", "Rename value", Icon::Props, "Rename a value and its references").agents(
            Agents::Verb, "Expressions keep their references when a value is renamed.", tools::rename_value_params,
        ).undo(Undo::Step),
        cmd("edit_dimension", "Edit dimension", Icon::Props, "Change a dimension of a sketch").agents(
            Agents::Mcp,
            "The sketch solves again and everything after it rebuilds.",
            tools::edit_dimension_params,
        )
.undo(Undo::Step),
        cmd(
            "suppress",
            "Suppress",
            Icon::Suppress,
            "Suppress a feature, so the part rebuilds without it, or restore it",
        )
        .run(|s| {
            let id = s.selected_feature().expect("enabled with a selected feature");
            let suppressed = selected_feature_is(s, |f| f.suppressed);
            AppAction::Command(Command::Suppress { id, suppressed: !suppressed })
        })
        .when(|s| s.selected_feature().is_some())
        .hint(|s| s.selected_feature().is_none().then(|| "Select a feature first".into()))
        .agents(Agents::Verb, "", tools::suppress_params)
.undo(Undo::Step),
        cmd(
            "delete_feature",
            "Delete feature",
            Icon::Close,
            "Remove a feature from the tree, with the sketch it consumes",
        )
        .run(|s| AppAction::Command(Command::DeleteFeature(s.selected_feature().expect("enabled with a selection"))))
        .when(|s| s.selected_feature().is_some())
        .hint(|s| s.selected_feature().is_none().then(|| "Select a feature first".into()))
        .agents(Agents::Verb, "", tools::delete_feature_params)
.undo(Undo::Step),
        cmd(
            "delete",
            "Delete",
            Icon::Close,
            "Delete the selected feature, or the selected sketch entities in a sketch",
        )
        .key(Scope::Part, none, Key::Delete)
        .key(Scope::Part, none, Key::Backspace)
        .run(|_| AppAction::DeleteSelection)
.app_only("It deletes what is selected in the viewport or the sketch; agents and scripts name the feature with delete_feature"),
        cmd("edit_extrude", "Edit extrude", Icon::Extrude, "Change an extrude's depth or direction").agents(
            Agents::Verb,
            "Everything after it rebuilds.",
            tools::edit_extrude_params,
        )
.undo(Undo::Step),
        cmd("edit_cut", "Edit cut", Icon::Cut, "Change a cut's end condition").agents(
            Agents::Verb,
            "Everything after it rebuilds.",
            tools::edit_cut_params,
        )
.undo(Undo::Step),
        cmd("edit_fillet", "Edit fillet", Icon::Fillet, "Change a fillet's radius").agents(
            Agents::Verb,
            "",
            tools::edit_fillet_params,
        )
.undo(Undo::Step),
        cmd("edit_chamfer", "Edit chamfer", Icon::Chamfer, "Change a chamfer's distance").agents(
            Agents::Verb,
            "",
            tools::edit_chamfer_params,
        )
        .undo(Undo::Step),
        cmd("edit_hole", "Edit hole", Icon::Hole, "Change a hole's diameter, depth and countersink").agents(
            Agents::Verb,
            "Everything after it rebuilds.",
            tools::edit_hole_params,
        )
        .undo(Undo::Step),
        cmd("edit_pattern", "Edit pattern", Icon::LinearPattern, "Change a pattern's copies and spacing or angle")
            .agents(Agents::Verb, "Everything after it rebuilds.", tools::edit_pattern_params)
            .undo(Undo::Step),
        cmd("edit_mirror", "Edit mirror", Icon::Mirror, "Change whether a mirror keeps the original").agents(
            Agents::Verb,
            "",
            tools::edit_mirror_params,
        )
        .undo(Undo::Step),
        cmd("edit_shell", "Edit shell", Icon::Shell, "Change a shell's wall thickness")
            .agents(Agents::Verb, "The openings are retained; everything after it rebuilds.", tools::edit_shell_params)
            .undo(Undo::Step),
        cmd("edit_draft", "Edit draft", Icon::Draft, "Change a draft's angle").agents(
            Agents::Verb,
            "Everything after it rebuilds.",
            tools::edit_draft_params,
        )
        .undo(Undo::Step),
        cmd("roll_to_end", "Roll to end", Icon::Rollback, "Move the rollback bar to the end of the tree")
            .run(|_| AppAction::Command(Command::Rollback(None)))
            .when(|s| s.doc.rollback.is_some())
.agents(Agents::Mcp, "The undoable form of rollback to the end.", Vec::new).undo(Undo::Step),
        // Keys that do what the moment needs.
        cmd(
            "cancel",
            "Cancel",
            Icon::Close,
            "Cancel a typed value, a drag or the open feature, leave the sketch, or clear the selection",
        )
        .key(Scope::Part, none, Key::Escape)
        .run(|s| {
            if s.dimension_edit.is_some() {
                // Escape takes the keyboard from a text field before the shortcuts run.
                AppAction::Dimension(crate::dimension_edit::DimensionAction::Cancel)
            } else if s.drawing.new.is_some() {
                AppAction::Drawing(DrawingAction::CancelNewDrawing)
            } else if s.editor.is_some() {
                AppAction::CancelEditor
            } else if s.in_sketch() {
                AppAction::ExitSketch(None)
            } else if s.drag_preview.is_some() {
                AppAction::DragDimension(DragStep::Cancel)
            } else {
                AppAction::Select(None)
            }
        })
.app_only(EDITOR),
        cmd("confirm", "OK", Icon::Check, "Apply the open feature or new drawing, or leave the sketch")
            .key(Scope::Part, none, Key::Enter)
            .run(|s| {
                if s.drawing.new.is_some() {
                    AppAction::Drawing(DrawingAction::CreateDrawing)
                } else if s.editor.is_some() {
                    AppAction::CommitEditor
                } else {
                    AppAction::ExitSketch(None)
                }
            })
            .when(|s| s.drawing.new.is_some() || s.editor.is_some() || s.in_sketch())
.app_only(EDITOR),
        // Help.
        cmd("command_palette", "Command palette", Icon::Search, "Search the commands and run one")
            .both()
            .key(Scope::App, ctrl_shift, Key::P)
            .run(|_| AppAction::TogglePalette)
.app_only(WINDOW),
        cmd("shortcuts", "Keyboard shortcuts", Icon::Config, "List the keyboard shortcuts")
            .key(Scope::Model, none, Key::Questionmark)
            .run(|_| AppAction::ShowShortcuts)
.app_only(WINDOW),
        cmd(
            "file_locations",
            "File locations",
            Icon::Open,
            "Where libraries, templates, sheet formats, styles and moved references are found",
        )
        .both()
        .run(|_| AppAction::ShowFileLocations(None))
.app_only("It opens the File locations page; `soliddraft locations` lists and edits them without a window"),
        cmd("about", "About SolidDraft", Icon::Config, "The version and the kernel")
            .both()
            .run(|_| AppAction::ShowAbout)
.app_only(WINDOW),
    ]
    .into_iter()
    .chain(assembly_registry::registry())
    .chain(drawing_registry())
    .chain(crate::measure::registry::commands())
    .chain(crate::grid::registry::commands())
    .chain(crate::mates::registry::commands())
    .chain(crate::subassembly::registry::commands())
    .collect()
}

/// The drawing workspace's commands.
fn drawing_registry() -> Vec<CommandSpec> {
    use DrawingTab::{Annotate, Assistant, Sheet, View, Views};
    use Size::{Large, Small};
    let none = Modifiers::NONE;
    let ctrl = Modifiers::COMMAND;
    let cmd = |id, label, icon, doc| CommandSpec::new(id, label, icon, doc).drawing();
    const P3: &str = "Arrives in phase 3 (assistant)";
    vec![
        // Views.
        cmd(
            "drawing_standard_views",
            "Standard views",
            Icon::Views,
            "Place a Front base view; fold the others off it with Projected",
        )
        .on(Views, "Standard", Large)
        .run(|_| AppAction::Drawing(DrawingAction::SetTool(DrawingTool::PlaceView(PlaceKind::Standard(
            soliddraft_kernel_api::drawing::StandardView::Front,
        )))))
.app_only(CANVAS),
        drawing_tool!(
            "drawing_model_view", "Model view", Icon::Display, "Place a view of the model in a standard orientation",
            DrawingTool::PlaceView(PlaceKind::Standard(_)) =>
                DrawingTool::PlaceView(PlaceKind::Standard(soliddraft_kernel_api::drawing::StandardView::Isometric))
        )
        .on(Views, "Standard", Large)
.app_only(CANVAS),
        cmd("drawing_projected", "Projected", Icon::OnPlane, "Fold a view off the selected view in one of eight directions")
            .on(Views, "Derived", Large)
            .run(|s| {
                let parent = sheet_view(s).map(|v| v.id).expect("enabled with a selected view");
                AppAction::Drawing(DrawingAction::SetTool(DrawingTool::PlaceView(PlaceKind::Projected { parent })))
            })
            .when(|s| sheet_view(s).is_some())
            .lit(|s| drawing_tool_is(s, |t| matches!(t, DrawingTool::PlaceView(PlaceKind::Projected { .. }))))
            .hint(|s| sheet_view(s).is_none().then(|| "Select a view first, then fold a new view off it".into()))
.app_only(CANVAS),
        drawing_tool!(
            "drawing_section", "Section", Icon::Section,
            "Draw a cutting line on a view, pick the direction, place the section",
            DrawingTool::Section { replace: None } => DrawingTool::Section { replace: None }
        )
        .on(Views, "Derived", Large)
        .key(Scope::Drawing, none, Key::S)
.agents(
            Agents::Mcp,
            "A cutting line through a model point across the view, horizontal or vertical on the sheet. The section \
             is placed in line with the view on the side the sheet's projection angle puts it, with the drawing's \
             next letter. The open part must be the drawing's.",
            app::section_params,
        )
        .undo(Undo::Step),
        drawing_tool!(
            "drawing_detail", "Detail", Icon::Search, "Circle or profile a region of a view and place it magnified",
            DrawingTool::Detail => DrawingTool::Detail
        )
        .on(Views, "Derived", Large)
.app_only(CANVAS),
        drawing_tool!(
            "drawing_auxiliary", "Auxiliary", Icon::Plane, "Pick an edge; the view unfolds square to it",
            DrawingTool::Auxiliary => DrawingTool::Auxiliary
        )
        .on(Views, "Derived", Small)
.app_only(CANVAS),
        drawing_tool!(
            "drawing_broken_out", "Broken-out", Icon::Cut, "Cut away a circle of a view to a depth and hatch the cut",
            DrawingTool::BrokenOut => DrawingTool::BrokenOut
        )
        .on(Views, "Derived", Small)
.app_only(CANVAS),
        drawing_tool!(
            "drawing_crop", "Crop", Icon::Rectangle, "Keep only a rectangle of a view",
            DrawingTool::Crop => DrawingTool::Crop
        )
        .on(Views, "Derived", Small)
        .run(|s| match sheet_view(s).filter(|v| v.crop.is_some()) {
            Some(v) => AppAction::Drawing(DrawingAction::Command(DrawingCommand::SetViewCrop { view: v.id, crop: None })),
            None => AppAction::Drawing(DrawingAction::SetTool(DrawingTool::Crop)),
        })
        .label_now(|s| sheet_view(s).is_some_and(|v| v.crop.is_some()).then(|| "Remove crop".to_owned()))
        .hint(|s| {
            sheet_view(s).is_some_and(|v| v.crop.is_some()).then(|| "Show the whole of the selected view again".into())
        })
.agents(Agents::Mcp, "", app::crop_params).undo(Undo::Step),
        cmd("drawing_align", "Align", Icon::Relation, "Restore the selected view's alignment with its parent")
            .on(Views, "Arrange", Small)
            .run(|s| {
                let view = sheet_view(s).map(|v| v.id).expect("enabled with a selected view");
                AppAction::Drawing(DrawingAction::Command(DrawingCommand::Align { view }))
            })
            .when(|s| sheet_view(s).is_some_and(|v| v.parent().is_some() && v.alignment.is_none()))
.agents(Agents::Mcp, "A view is named as model_summary lists it under drawing.", app::view_params).undo(Undo::Step),
        cmd("drawing_break_alignment", "Break alignment", Icon::Construction, "Free the selected view from its parent")
            .on(Views, "Arrange", Small)
            .run(|s| {
                let view = sheet_view(s).map(|v| v.id).expect("enabled with a selected view");
                AppAction::Drawing(DrawingAction::Command(DrawingCommand::BreakAlignment { view }))
            })
            .when(|s| sheet_view(s).is_some_and(|v| v.alignment.is_some()))
.agents(Agents::Mcp, "A view is named as model_summary lists it under drawing.", app::view_params).undo(Undo::Step),
        cmd("drawing_scale", "Scale", Icon::Measure, "Scale and display style are in the property panel")
            .on(Views, "Arrange", Small)
            .run(|s| {
                let view = sheet_view(s).map(|v| v.id).expect("enabled with a selected view");
                AppAction::Drawing(DrawingAction::Select(Some(DrawingSel::View(view))))
            })
            .when(|s| sheet_view(s).is_some())
.app_only(PANEL),
        cmd("drawing_hidden_lines", "Hidden lines", Icon::Eye, "Show or remove the selected view's hidden lines (dashed)")
            .on(Views, "Display", Small)
            .run(|s| {
                let v = sheet_view(s).expect("enabled with a selected view");
                let hidden =
                    if v.display.hidden == HiddenLines::Visible { HiddenLines::Removed } else { HiddenLines::Visible };
                let display = ViewDisplay { hidden, ..v.display };
                AppAction::Drawing(DrawingAction::Command(DrawingCommand::SetViewDisplay { view: v.id, display }))
            })
            .when(|s| sheet_view(s).is_some())
            .lit(|s| sheet_view(s).is_some_and(|v| v.display.hidden == HiddenLines::Visible))
            .hint(|s| sheet_view(s).is_none().then(|| "Select a view first".into()))
.agents(Agents::Mcp, "", app::hidden_lines_params).undo(Undo::Step),
        cmd(
            "drawing_tangent_edges",
            "Tangent edges",
            Icon::Line,
            "Cycle the selected view's tangent edges: removed, phantom, visible",
        )
        .on(Views, "Display", Small)
        .run(|s| {
            let v = sheet_view(s).expect("enabled with a selected view");
            let tangent = match v.display.tangent {
                TangentEdges::Removed => TangentEdges::Phantom,
                TangentEdges::Phantom => TangentEdges::Visible,
                TangentEdges::Visible => TangentEdges::Removed,
            };
            let display = ViewDisplay { tangent, ..v.display };
            AppAction::Drawing(DrawingAction::Command(DrawingCommand::SetViewDisplay { view: v.id, display }))
        })
        .when(|s| sheet_view(s).is_some())
        .label_now(|s| {
            sheet_view(s).map(|v| {
                match v.display.tangent {
                    TangentEdges::Removed => "Tangent: removed",
                    TangentEdges::Phantom => "Tangent: phantom",
                    TangentEdges::Visible => "Tangent: visible",
                }
                .to_owned()
            })
        })
        .hint(|s| sheet_view(s).is_none().then(|| "Select a view first".into()))
.agents(Agents::Mcp, "", app::tangent_edges_params).undo(Undo::Step),
        cmd("drawing_rebuild", "Rebuild", Icon::Rollback, "Bring the views to the part's current revision")
            .on(Views, "Model", Large)
            .key(Scope::Drawing, ctrl, Key::B)
            .key(Scope::Drawing, none, Key::U)
            .run(|_| AppAction::Drawing(DrawingAction::Rebuild))
            .lit(|s| session(s).is_some_and(|d| d.doc.is_stale() || matches!(d.link_state(), LinkState::Newer { .. })))
            .hint(|s| {
                session(s).map(|d| {
                    if d.link_state() == LinkState::Missing {
                        "The part was not found; repair the reference first".into()
                    } else if d.doc.is_stale() || matches!(d.link_state(), LinkState::Newer { .. }) {
                        "Newer version available: bring the views to the current part".into()
                    } else {
                        "The views are at the part's current revision; rebuild anyway".into()
                    }
                })
            })
.agents(Agents::Mcp, "Opens the drawing's part first when it is not the open part.", Vec::new).undo(Undo::Step),
        cmd(
            "drawing_replace_model",
            "Replace model",
            Icon::Open,
            "Point the drawing at a different part; the views re-project at the next Rebuild",
        )
        .on(Views, "Model", Small)
        .run(|_| AppAction::Drawing(DrawingAction::ShowRepair { replace: true }))
.agents(Agents::Mcp, "The drawing must be saved first, so the reference has a folder to be relative to.", tools::part_path_params),
        cmd("drawing_auto_update", "Auto update", Icon::Eye, "Rebuild after every change of the open part")
            .on(Views, "Model", Small)
            .run(|_| AppAction::Drawing(DrawingAction::ToggleAutomaticUpdate))
            .label_now(|s| {
                session(s).map(|d| {
                    if d.doc.drawing.settings.automatic_update { "Auto update on" } else { "Auto update off" }.to_owned()
                })
            })
.agents(Agents::Mcp, "Turning it on rebuilds a drawing older than its part.", app::auto_update_params).undo(Undo::Step),
        cmd("drawing_show_part", "Show part", Icon::Home, "Bring the part tab forward")
            .on(Views, "Model", Small)
            .on(View, "Documents", Large)
            .run(|_| AppAction::Drawing(DrawingAction::ShowPart))
.agents(Agents::Mcp, "Undo and redo then work on the part.", Vec::new),
        cmd("drawing_repair_reference", "Repair reference", Icon::Repair, "Find the part where it is now, or pick it by hand")
            .on(Views, "Model", Small)
            .run(|_| AppAction::Drawing(DrawingAction::ShowRepair { replace: false }))
            .shown(|s| session(s).is_some_and(|d| d.link_state() == LinkState::Missing))
.agents(Agents::Mcp, "The drawing must be saved first, so the reference has a folder to be relative to.", tools::part_path_params),
        cmd("drawing_explain", "Explain", Icon::Chat, "Ask the assistant to explain the selected annotation")
            .on(Views, "Assistant", Small)
            .on(Annotate, "Assistant", Small)
            .on(Assistant, "Chat", Large)
            .hint(|_| Some(P3.into())),
        // Annotate.
        drawing_tool!(
            "drawing_smart_dimension", "Smart dimension", Icon::SmartDim,
            "Dimension an edge, circle, arc, two edges or two ends",
            DrawingTool::SmartDimension => DrawingTool::SmartDimension
        )
        .on(Annotate, "Dimension", Large)
        .key(Scope::Drawing, none, Key::D)
.app_only(CANVAS),
        cmd("drawing_model_items", "Model items", Icon::Tree, "Import the part's own dimensions into the most appropriate views")
            .on(Annotate, "Dimension", Large)
            .run(|_| AppAction::Drawing(DrawingAction::ModelItems))
.agents(Agents::Mcp, "Into the views of the current sheet.", Vec::new).undo(Undo::Steps),
        drawing_tool!(
            "drawing_ordinate", "Ordinate", Icon::Line, "Distances from an origin, written at leaders",
            DrawingTool::DimensionSet(SetKind::Ordinate) => DrawingTool::DimensionSet(SetKind::Ordinate)
        )
        .on(Annotate, "Dimension", Small)
        .key(Scope::Drawing, none, Key::O)
.app_only(CANVAS),
        drawing_tool!(
            "drawing_baseline", "Baseline", Icon::Line, "Stacked dimensions from an origin",
            DrawingTool::DimensionSet(SetKind::Baseline) => DrawingTool::DimensionSet(SetKind::Baseline)
        )
        .on(Annotate, "Dimension", Small)
.app_only(CANVAS),
        drawing_tool!(
            "drawing_chain", "Chain", Icon::Relation, "Dimensions end to end along a line",
            DrawingTool::DimensionSet(SetKind::Chain) => DrawingTool::DimensionSet(SetKind::Chain)
        )
        .on(Annotate, "Dimension", Small)
.app_only(CANVAS),
        drawing_tool!(
            "drawing_note", "Note", Icon::Chat, "Text with an optional leader",
            DrawingTool::Note => DrawingTool::Note
        )
        .on(Annotate, "Notes", Large)
        .key(Scope::Drawing, none, Key::N)
.app_only(CANVAS),
        drawing_tool!(
            "drawing_hole_callout", "Hole callout", Icon::Hole, "Read a hole: count, diameter, THRU or depth",
            DrawingTool::HoleCallout => DrawingTool::HoleCallout
        )
        .on(Annotate, "Notes", Large)
        .key(Scope::Drawing, none, Key::H)
.app_only(CANVAS),
        CommandSpec::new("drawing_balloon", "Balloon", Icon::Circle, "Number an assembly component")
            .drawing()
            .on(Annotate, "Notes", Small)
            .hint(|_| Some("Arrives with assemblies".into())),
        drawing_tool!(
            "drawing_revision_symbol", "Revision symbol", Icon::Rollback, "The revision in a triangle, at a changed place",
            DrawingTool::RevisionSymbol => DrawingTool::RevisionSymbol
        )
        .on(Annotate, "Notes", Small)
.app_only(CANVAS),
        cmd("drawing_chamfer_dimension", "Chamfer", Icon::Chamfer, "Dimension a chamfer").on(Annotate, "Notes", Small),
        drawing_tool!(
            "drawing_centre_mark", "Centre mark", Icon::Point, "Mark the centre of a circle seen face-on",
            DrawingTool::CentreMark => DrawingTool::CentreMark
        )
        .on(Annotate, "Centre", Large)
.app_only(CANVAS),
        cmd("drawing_centreline", "Centreline", Icon::Axis, "Draw a centreline between two edges")
            .on(Annotate, "Centre", Large),
        cmd(
            "drawing_auto_centre_marks",
            "Auto centre marks",
            Icon::Check,
            "Add centre marks to every circle of the selected view, or of the sheet",
        )
        .on(Annotate, "Centre", Small)
        .run(|_| AppAction::Drawing(DrawingAction::AutoCentreMarks))
.agents(Agents::Mcp, "", app::centre_marks_params).undo(Undo::Steps),
        drawing_tool!(
            "drawing_surface_finish", "Surface finish", Icon::Material, "ISO 1302 symbol: roughness, process, method, lay",
            DrawingTool::SurfaceFinish => DrawingTool::SurfaceFinish
        )
        .on(Annotate, "Symbols", Large)
.app_only(CANVAS),
        drawing_tool!(
            "drawing_gdt", "GD&T frame", Icon::Rectangle, "A feature control frame: characteristic, tolerance zone, datums",
            DrawingTool::Gdt => DrawingTool::Gdt
        )
        .on(Annotate, "Symbols", Large)
.app_only(CANVAS),
        drawing_tool!(
            "drawing_datum", "Datum", Icon::Plane, "A datum feature symbol on an edge",
            DrawingTool::Datum => DrawingTool::Datum
        )
        .on(Annotate, "Symbols", Small)
.app_only(CANVAS),
        cmd("drawing_weld", "Weld", Icon::Construction, "A weld symbol").on(Annotate, "Symbols", Small),
        cmd(
            "drawing_revision_table",
            "Revision table",
            Icon::Tree,
            "The sheet's revisions: add a row, mark changes with revision symbols",
        )
        .on(Annotate, "Tables", Large)
        .run(|_| AppAction::Drawing(DrawingAction::Select(Some(DrawingSel::Revisions))))
        .lit(|s| session(s).is_some_and(|d| d.selection == Some(DrawingSel::Revisions)))
.app_only(PANEL),
        cmd("drawing_hole_table", "Hole table", Icon::Hole, "Tabulate the holes of a view").on(Annotate, "Tables", Small),
        CommandSpec::new("drawing_bom", "BOM", Icon::Mass, "A bill of materials")
            .drawing()
            .on(Annotate, "Tables", Small)
            .hint(|_| Some("Arrives with assemblies".into())),
        // Sheet.
        cmd("drawing_sheet_properties", "Sheet properties", Icon::Props, "Name, size, scale, projection angle and title block")
            .on(Sheet, "Sheet", Large)
            .run(|_| AppAction::Drawing(DrawingAction::Select(None)))
.app_only(PANEL),
        cmd(
            "drawing_add_sheet",
            "Add sheet",
            Icon::Open,
            "A sheet after the last one, from the same template; the tabs under the canvas switch, rename, reorder and delete",
        )
        .on(Sheet, "Sheet", Small)
        .run(|_| AppAction::Drawing(DrawingAction::AddSheet))
.agents(Agents::Mcp, "The new sheet becomes the current sheet.", Vec::new).undo(Undo::Step),
        cmd("drawing_zones", "Zones", Icon::Rectangle, "The zone grid comes from the template")
            .on(Sheet, "Sheet", Small)
            .run(|_| AppAction::Drawing(DrawingAction::Select(Some(DrawingSel::Format))))
.app_only(PANEL),
        cmd(
            "drawing_edit_format",
            "Edit sheet format",
            Icon::Rectangle,
            "The border and title block; fields are edited in the property panel",
        )
        .on(Sheet, "Format", Large)
        .run(|_| AppAction::Drawing(DrawingAction::Select(Some(DrawingSel::Format))))
.app_only(PANEL),
        cmd("drawing_title_block", "Title block fields", Icon::Props, "Edit the title block values")
            .on(Sheet, "Format", Small)
            .run(|_| AppAction::Drawing(DrawingAction::Select(Some(DrawingSel::Format))))
.app_only(PANEL),
        CommandSpec::new("drawing_line_styles", "Line styles", Icon::Line, "Line widths and styles")
            .drawing()
            .on(Sheet, "Format", Small)
            .hint(|_| Some("Line widths come from the template's data file".into())),
        cmd("drawing_export_pdf", "PDF", Icon::Save, "Write this sheet as a PDF at the paper size")
            .on(Sheet, "Export", Large)
            .run(|_| AppAction::Drawing(DrawingAction::Export { format: Format::Pdf, path: None }))
.agents(Agents::Mcp, "Every sheet is a page.", app::pdf_path_params),
        cmd(
            "drawing_export_dxf",
            "DXF",
            Icon::Save,
            "Write this sheet as DXF R12 on VISIBLE, HIDDEN, CENTER, DIMENSION, TEXT and FORMAT layers",
        )
        .on(Sheet, "Export", Large)
        .run(|_| AppAction::Drawing(DrawingAction::Export { format: Format::Dxf, path: None }))
.agents(Agents::Mcp, "The current sheet.", app::dxf_path_params),
        cmd("drawing_export_svg", "SVG", Icon::Save, "Write this sheet as SVG")
            .on(Sheet, "Export", Small)
            .run(|_| AppAction::Drawing(DrawingAction::Export { format: Format::Svg, path: None }))
.agents(Agents::Mcp, "The current sheet.", app::svg_path_params),
        cmd("drawing_save", "Save drawing", Icon::Save, "Save the drawing file")
            .on(Sheet, "Export", Small)
            .key(Scope::Drawing, ctrl, Key::S)
            .run(|_| AppAction::Drawing(DrawingAction::Save(None)))
.agents(Agents::Mcp, "", app::drawing_path_params),
        CommandSpec::new("drawing_print", "Print", Icon::Display, "Print the sheet")
            .drawing()
            .on(Sheet, "Export", Small)
            .hint(|_| Some("Printing goes through the PDF for now".into())),
        // View.
        cmd("drawing_zoom_in", "Zoom in", Icon::Search, "Zoom in")
            .on(View, "Zoom", Small)
            .key(Scope::Drawing, none, Key::Plus)
            .key(Scope::Drawing, none, Key::Equals)
            .run(|_| AppAction::Drawing(DrawingAction::ZoomIn))
.app_only(SHEET_VIEW),
        cmd("drawing_zoom_out", "Zoom out", Icon::Search, "Zoom out")
            .on(View, "Zoom", Small)
            .key(Scope::Drawing, none, Key::Minus)
            .run(|_| AppAction::Drawing(DrawingAction::ZoomOut))
.app_only(SHEET_VIEW),
        cmd("drawing_grid", "Grid", Icon::Rectangle, "Show a 10 mm grid")
            .on(View, "Sheet", Large)
            .key(Scope::Drawing, none, Key::G)
            .run(|_| AppAction::Drawing(DrawingAction::ToggleGrid))
            .lit(|s| session(s).is_some_and(|d| d.show_grid))
.app_only(SHEET_VIEW),
        cmd("drawing_snap", "Snap", Icon::Relation, "Snap placements to the alignment guides")
            .on(View, "Sheet", Large)
            .run(|_| AppAction::Drawing(DrawingAction::ToggleSnap))
            .lit(|s| session(s).is_some_and(|d| d.snap))
.app_only(SHEET_VIEW),
        cmd("drawing_close", "Close drawing", Icon::Close, "Close the drawing tab")
            .on(View, "Documents", Small)
            .run(|_| AppAction::Drawing(DrawingAction::CloseDrawing))
.agents(Agents::Mcp, "Unsaved changes to the drawing are dropped.", Vec::new).undo(Undo::Replaces),
        // Assistant.
        cmd("drawing_check_sheet", "Check sheet", Icon::Check, "Ask the assistant to check the sheet")
            .on(Assistant, "Drawing", Large)
            .hint(|_| Some(P3.into())),
        cmd("drawing_propose_dimensions", "Propose dimensions", Icon::SmartDim, "Ask the assistant to dimension the views")
            .on(Assistant, "Drawing", Small)
            .hint(|_| Some(P3.into())),
        cmd("drawing_find_issues", "Find issues", Icon::Warning, "Ask the assistant for problems on the sheet")
            .on(Assistant, "Drawing", Small)
            .hint(|_| Some(P3.into())),
        // Keys.
        drawing_tool!(
            "drawing_select", "Select", Icon::Sketch, "Select and drag views and annotations",
            DrawingTool::Select => DrawingTool::Select
        )
        .key(Scope::Drawing, none, Key::V)
.app_only(CANVAS),
        cmd("drawing_cancel", "Cancel", Icon::Close, "Put the tool down, or clear the selection")
            .key(Scope::Drawing, none, Key::Escape)
            .run(|s| match session(s) {
                Some(d) if d.tool != DrawingTool::Select || !d.picks.is_empty() => {
                    AppAction::Drawing(DrawingAction::SetTool(DrawingTool::Select))
                }
                _ => AppAction::Drawing(DrawingAction::Select(None)),
            })
            .when(|s| {
                session(s).is_some_and(|d| d.tool != DrawingTool::Select || !d.picks.is_empty() || d.selection.is_some())
            })
.app_only(CANVAS_STEP),
        cmd("drawing_delete", "Delete", Icon::Close, "Delete the selected view or annotation")
            .key(Scope::Drawing, none, Key::Delete)
            .key(Scope::Drawing, none, Key::Backspace)
            .run(|_| AppAction::Drawing(DrawingAction::DeleteSelection))
.agents(Agents::Mcp, "", app::drawing_delete_params).undo(Undo::Step),
        cmd("drawing_flip", "Flip direction", Icon::Rollback, "Flip the section or auxiliary view being placed")
            .key(Scope::Placing, none, Key::F)
            .run(|_| AppAction::Drawing(DrawingAction::FlipPending))
.app_only(CANVAS_STEP),
        cmd("drawing_close_profile", "Close profile", Icon::Check, "Close the detail's profile and compute the view")
            .key(Scope::Drawing, none, Key::Enter)
            .run(|_| AppAction::Drawing(DrawingAction::CloseProfile))
            .when(|s| {
                session(s).is_some_and(|d| d.tool == DrawingTool::Detail && d.draft.profile && d.draft.points.len() >= 3)
            })
.app_only(CANVAS_STEP),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn waiting_commands_stay_visible_disabled_and_name_their_stages() {
        let state = AppState::new(crate::demo_document());
        for (id, stages) in [("sweep", &[5][..]), ("loft", &[5][..])] {
            let command = get(id).unwrap();
            assert!(!command.ribbon.is_empty(), "{id}");
            assert!((command.shown)(&state), "{id}");
            assert!(!command.is_enabled(&state), "{id}");
            assert!(command.action_now(&state).is_none(), "{id}");
            for stage in stages {
                assert!(
                    command.tip(&state).contains(&format!("Catallax stage {stage}")),
                    "{id}: {}",
                    command.tip(&state)
                );
            }
        }
    }

    #[test]
    fn catallax_features_on_main_are_switched_on() {
        let state = AppState::new(crate::demo_document());
        for id in ["fillet", "chamfer", "hole", "linear_pattern", "circular_pattern", "mirror", "draft"] {
            let command = get(id).unwrap();
            assert!(command.catallax_wait().is_none(), "{id}");
            assert!(command.is_enabled(&state), "{id}: {}", command.tip(&state));
            assert!(command.agents == Agents::Verb, "{id} is an agent verb");
        }
    }

    #[test]
    fn ids_are_unique_and_valid_tool_names() {
        let mut seen = HashSet::new();
        for c in all() {
            assert!(seen.insert(c.id), "{} is registered twice", c.id);
            assert!(
                c.id.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
                    && c.id.starts_with(|ch: char| ch.is_ascii_lowercase())
                    && c.id.len() <= 64,
                "{} is not lower snake case, or too long for a tool name",
                c.id
            );
        }
    }

    #[test]
    fn every_command_has_a_label_and_a_doc() {
        for c in all() {
            assert!(!c.label.is_empty() && !c.doc.is_empty(), "{} needs a label and a doc", c.id);
            assert!(!c.doc.ends_with('.'), "{}: a doc is a line without a full stop", c.id);
            assert!(!c.agents.has_tool() || c.details.is_empty() || c.details.ends_with('.'), "{}", c.id);
            if let Agents::App(why) = c.agents {
                assert!(!why.is_empty() && !why.ends_with('.'), "{}: why it runs only in the app, as a doc line", c.id);
            }
        }
    }

    #[test]
    fn shortcuts_do_not_collide() {
        let bound: Vec<(&CommandSpec, Shortcut)> =
            all().iter().flat_map(|c| c.keys.iter().map(move |k| (c, *k))).collect();
        for context in Context::all() {
            for (i, (a, ka)) in bound.iter().enumerate() {
                for (b, kb) in &bound[i + 1..] {
                    if !ka.same_chord(kb) || !ka.scope.applies(context) || !kb.scope.applies(context) {
                        continue;
                    }
                    let resolved = ka.scope.narrower_than(kb.scope) || kb.scope.narrower_than(ka.scope);
                    assert!(resolved, "{} and {} both take {} in {context:?}", a.id, b.id, ka.text());
                }
            }
        }
    }

    #[test]
    fn only_ctrl_chords_and_the_panel_keys_fire_while_typing() {
        for c in all() {
            for k in &c.keys {
                let ctrl = k.modifiers.command || k.modifiers.ctrl;
                let panel_key = c.id == "toggle_tree" || c.id == crate::command_line::AI_COMMAND;
                assert_eq!(k.fires_while_typing(), ctrl || panel_key, "{} {}", c.id, k.text());
            }
        }
    }

    #[test]
    fn chords_with_more_modifiers_are_tried_first() {
        let order = bindings(Context::default());
        let pos = |id: &str, m: Modifiers| order.iter().position(|(c, k)| c.id == id && k.modifiers == m).unwrap();
        assert!(pos("save_as", Modifiers::COMMAND | Modifiers::SHIFT) < pos("save", Modifiers::COMMAND));
        assert!(pos("redo", Modifiers::COMMAND | Modifiers::SHIFT) < pos("undo", Modifiers::COMMAND));
        assert!(pos(crate::command_line::AI_COMMAND, Modifiers::SHIFT) < pos("toggle_tree", Modifiers::NONE));
        let edge = bindings(Context { edge: true, ..Default::default() });
        let at = |id: &str| edge.iter().position(|(c, _)| c.id == id).unwrap();
        assert!(at("fillet") < at("view_front"), "with an edge selected, F is Fillet");
    }

    #[test]
    fn the_ribbon_keeps_its_layout() {
        let panels = |tab: RibbonTab| ribbon(tab).iter().map(|(p, _)| *p).collect::<Vec<_>>();
        assert_eq!(panels(RibbonTab::Solid), ["Sketch", "Create", "Remove", "Modify", "Pattern", "Reference"]);
        assert_eq!(panels(RibbonTab::Sketch), ["Sketch", "Draw", "Constrain", "Use sketch", "View"]);
        assert_eq!(panels(RibbonTab::Assembly), ["Edit", "Component", "File", "View"]);
        assert_eq!(panels(RibbonTab::Inspect), ["Measure", "Check", "Surface quality"]);
        assert_eq!(panels(RibbonTab::View), ["Orientation", "Display", "Projection", "Panels"]);
        assert!(panels(RibbonTab::Surface).is_empty() && panels(RibbonTab::PointCloud).is_empty());
        let ids = |tab: RibbonTab, panel: &str| {
            ribbon(tab).into_iter().find(|(p, _)| *p == panel).unwrap().1.iter().map(|(c, _)| c.id).collect::<Vec<_>>()
        };
        assert_eq!(ids(RibbonTab::Solid, "Sketch"), ["sketch"]);
        assert_eq!(
            ids(RibbonTab::View, "Orientation"),
            ["view_isometric", "zoom_fit", "view_front", "view_top", "view_right", "normal_to"]
        );
        for c in all() {
            for p in &c.ribbon {
                assert!(super::panels(p.tab).contains(&p.panel), "{}: no panel {} on {}", c.id, p.panel, p.tab.label());
            }
        }
        for tab in Tab::all() {
            for (panel, items) in ribbon(tab) {
                assert!(!items.is_empty(), "{} › {panel} is empty", tab.label());
            }
        }
        let drawing = |tab: DrawingTab, panel: &str| {
            ribbon(tab).into_iter().find(|(p, _)| *p == panel).unwrap().1.iter().map(|(c, _)| c.id).collect::<Vec<_>>()
        };
        assert_eq!(drawing(DrawingTab::Views, "Edit"), ["undo", "redo"]);
        assert_eq!(drawing(DrawingTab::View, "Zoom"), ["zoom_fit", "drawing_zoom_in", "drawing_zoom_out"]);
        assert_eq!(drawing(DrawingTab::View, "Panels"), ["toggle_tree", crate::command_line::AI_COMMAND]);
        assert_eq!(drawing(DrawingTab::Assistant, "Chat"), ["ask", "drawing_explain"]);
    }

    #[test]
    fn search_here_offers_the_current_workspace() {
        let history = History::default();
        let part = Context::default();
        let drawing = Context { drawing: true, ..Default::default() };
        let ids = |c: Context, q: &str| search_here(q, &history, c).iter().map(|m| m.command.id).collect::<Vec<_>>();
        assert_eq!(ids(part, "section").first(), Some(&"section"));
        assert_eq!(ids(drawing, "section").first(), Some(&"drawing_section"));
        assert!(ids(drawing, "extrude").is_empty());
        assert!(ids(drawing, "undo").contains(&"undo"), "undo works in both workspaces");
        assert!(!ids(part, "zoom in").contains(&"drawing_zoom_in"));
    }

    #[test]
    fn search_ranks_labels_then_pins_and_recents() {
        let mut history = History::default();
        let first = |q: &str, h: &History| search(q, h).first().map(|m| m.command.id);
        assert_eq!(first("extrude", &history), Some("extrude"));
        assert_eq!(first("fil", &history), Some("fillet"));
        assert_eq!(first("front", &history), Some("view_front"));
        assert!(search("zzzz", &history).is_empty());
        assert!(!search("fil", &history).iter().any(|m| m.command.id == "sweep"), "a doc matches at word starts only");
        assert!(search("", &history).is_empty(), "nothing pinned or recent yet");
        assert!(search("model summary", &history).is_empty(), "agent-only commands stay out of the app's search");
        // Two words must both match.
        assert_eq!(first("smart dim", &history), Some("smart_dimension"));
        // "Point" is both a sketch tool and reference geometry; the recent one ranks first.
        assert_eq!(first("point", &history), Some("sketch_point"));
        history.record("reference_point");
        assert_eq!(first("point", &history), Some("reference_point"));
        history.pin("sketch_point");
        assert_eq!(first("point", &history), Some("sketch_point"));
        let listed: Vec<&str> = search("", &history).iter().map(|m| m.command.id).collect();
        assert_eq!(listed, ["sketch_point", "reference_point"], "pins first, then recents");
    }

    #[test]
    fn history_keeps_the_latest_first_and_drops_unknown_ids() {
        let mut h = History::default();
        for i in 0..20 {
            h.record(if i % 2 == 0 { "undo" } else { "redo" });
        }
        assert_eq!(h.recent, ["redo", "undo"]);
        for c in all().iter().take(History::RECENT + 3) {
            h.record(c.id);
        }
        assert_eq!(h.recent.len(), History::RECENT);
        h.pin("gone_in_a_newer_build");
        h.pin("sketch");
        h.pin("sketch");
        h.retain_known();
        assert_eq!(h.pinned, ["sketch"]);
        h.unpin("sketch");
        assert!(h.pinned.is_empty());
    }

    #[test]
    fn shortcut_text_reads_as_the_menus_write_it() {
        let text = |id: &str| get(id).unwrap().shortcut_text().unwrap();
        assert_eq!(text("save_as"), "Ctrl+Shift+S");
        assert_eq!(text("delete"), "Del");
        assert_eq!(text("rename"), "F2");
        assert_eq!(text("ask"), "/");
        assert_eq!(text(crate::command_line::AI_COMMAND), "Shift+F9");
        assert_eq!(text("shortcuts"), "?");
    }
}
