//! The eframe application: menu bar, ribbon, docked panels, status bar,
//! keyboard shortcuts and the action loop.

use crate::commands;
use crate::document::{
    ChamferParams, Command, CutParams, Document, ExtrudeParams, FeatureKind, FeatureStatus, FilletParams, Selection,
    SketchPlane,
};
use crate::icons::{self, Icon};
use crate::panels::{self, Layout, Side};
use crate::state::{AppAction, AppState, EditorParams, ExitInto, FeatureEditor, NoteKind, SketchSession};
use crate::theme;
use crate::viewport::Overlay;
use crate::viewport::look::{Preset, Tier};
use crate::viewport::render::{DisplayMode, Picked, ViewportRenderer};
use crate::{assistant, file, properties, ribbon, tree};
use egui::{Align2, CornerRadius, Key, Rect, Sense, Stroke, pos2, vec2};
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};
use std::path::PathBuf;
use std::time::Duration;
use web_time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Tree,
    Properties,
    Viewport,
    Assistant,
    /// The drawing document, opened beside the part by Make drawing.
    Drawing,
}

impl Tab {
    fn title(&self) -> &'static str {
        match self {
            Tab::Tree => "Feature tree",
            Tab::Properties => "Properties",
            Tab::Viewport => "Part",
            Tab::Assistant => "Assistant",
            Tab::Drawing => "Drawing",
        }
    }
}

/// The middle: the part, and the drawing tab beside it once one is open.
fn center_dock() -> DockState<Tab> {
    DockState::new(vec![Tab::Viewport])
}

/// The left side area: the feature tree with the property panel under it.
fn tree_dock() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Tree]);
    dock.main_surface_mut().split_below(NodeIndex::root(), 0.46, vec![Tab::Properties]);
    dock
}

/// The right side area: the assistant.
fn assistant_dock() -> DockState<Tab> {
    DockState::new(vec![Tab::Assistant])
}

/// A dock area in the app's style, without the buttons the shell does not use.
fn show_dock(ui: &mut egui::Ui, dock: &mut DockState<Tab>, id: egui::Id, panels: &mut Panels<'_>) {
    let style = theme::dock_style(ui.ctx());
    DockArea::new(dock)
        .id(id)
        .style(style)
        .show_close_buttons(false)
        .show_add_buttons(false)
        .show_leaf_close_all_buttons(false)
        .show_leaf_collapse_buttons(false)
        .show_inside(ui, panels);
}

/// The strip a hidden side area leaves at its edge: a chevron pointing into the
/// window and the title set on end. Returns true when clicked.
fn strip(ui: &mut egui::Ui, side: Side, title: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click());
    let label = format!("Show the {}", side.name());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label.clone()));
    let resp = resp.on_hover_text(format!("{label} ({})", side.shortcut()));
    let painter = ui.painter();
    if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::ZERO, theme::HOVER);
    }
    let color = if resp.hovered() { theme::INK } else { theme::MUTED };
    let icon = match side {
        Side::Tree => Icon::ChevronRight,
        Side::Assistant => Icon::ChevronLeft,
    };
    let icon_rect = Rect::from_center_size(pos2(rect.center().x, rect.top() + 16.0), vec2(14.0, 14.0));
    icons::paint(painter, icon, icon_rect, color);
    let galley = painter.layout_no_wrap(title.to_owned(), theme::font_medium(12.0), color);
    let (w, h) = (galley.size().x, galley.size().y);
    // The text turns about its top-left corner: the left strip reads bottom to top,
    // the right one top to bottom, like the tabs of a collapsed task pane.
    let (pos, angle) = match side {
        Side::Tree => (pos2(rect.center().x - h / 2.0, rect.top() + 34.0 + w), -std::f32::consts::FRAC_PI_2),
        Side::Assistant => (pos2(rect.center().x + h / 2.0, rect.top() + 34.0), std::f32::consts::FRAC_PI_2),
    };
    painter.add(egui::epaint::TextShape::new(pos, galley, color).with_angle(angle));
    resp.clicked()
}

struct Panels<'a> {
    state: &'a mut AppState,
}

impl TabViewer for Panels<'_> {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(("tab", tab.title()))
    }

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        let text = match tab {
            Tab::Viewport => self.state.doc.name.clone(),
            Tab::Tree if self.state.drawing.is_active() => "Drawing tree".to_owned(),
            Tab::Drawing => match self.state.drawing.session.as_ref() {
                // A dot marks a drawing with stale views or unresolved annotations.
                Some(s) if s.has_issues() => format!("{} •", s.title()),
                Some(s) => s.title(),
                None => "Drawing".to_owned(),
            },
            other => other.title().to_owned(),
        };
        egui::RichText::new(text).family(theme::medium()).size(12.0).into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
        match tab {
            Tab::Tree if self.state.drawing.is_active() => crate::drawing::tree_drawing::show(ui, self.state),
            Tab::Tree => tree::show(ui, self.state),
            Tab::Properties if self.state.drawing.new.is_some() && !self.state.drawing.is_active() => {
                crate::drawing::properties_drawing::new_drawing(ui, self.state)
            }
            Tab::Properties if self.state.drawing.is_active() => {
                crate::drawing::properties_drawing::show(ui, self.state)
            }
            Tab::Properties => properties::show(ui, self.state),
            Tab::Assistant => assistant::show(ui, self.state),
            Tab::Drawing => crate::drawing::canvas::show(ui, self.state),
            Tab::Viewport => {
                let st = &mut *self.state;
                let ghost = st.ghost();
                let preview = st.preview_body();
                let selection = st.doc.selection;
                let hover_label = st.hover_info.as_ref().map(|i| i.title.clone());
                // A selected sketch shows quietly on its plane while no sketch is open.
                let passive = if st.sketch.is_none() {
                    st.selected_sketch().and_then(|id| {
                        let f = st.doc.feature(id)?;
                        Some((f.plane?, &f.sketch()?.sketch))
                    })
                } else {
                    None
                };
                let preview_label = st.drag_preview.as_ref().map(|d| d.label());
                let overlay =
                    Overlay { ghost, preview, sketch: st.sketch.as_mut(), passive_sketch: passive, preview_label };
                let events = st.viewport.show(ui, &st.doc, overlay, selection, hover_label.as_deref());
                let (dt, ppp) = ui.input(|i| (i.stable_dt.min(0.1), i.pixels_per_point));
                if st.doc.update_display(&st.viewport.projection(ppp), dt) {
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
                }
                for a in events.actions {
                    st.push(a);
                }
                if let Some(picked) = events.clicked {
                    let sel = picked.and_then(|p| selection_of(&st.doc, p));
                    st.push(AppAction::Select(sel));
                }
                if let Some(v) = events.set_view {
                    st.push(AppAction::SetView(v));
                }
                if events.fit {
                    st.push(AppAction::ZoomFit);
                }
                if let Some(d) = events.set_display {
                    st.push(AppAction::SetDisplay(d));
                }
                if events.toggle_projection {
                    st.push(AppAction::ToggleProjection);
                }
            }
        }
    }

    fn is_closeable(&self, tab: &Tab) -> bool {
        matches!(tab, Tab::Drawing)
    }

    fn on_close(&mut self, tab: &mut Tab) -> egui_dock::tab_viewer::OnCloseResponse {
        if matches!(tab, Tab::Drawing) {
            self.state.drawing.session = None;
            self.state.drawing.active = false;
        }
        egui_dock::tab_viewer::OnCloseResponse::Close
    }

    fn clear_background(&self, _tab: &Tab) -> bool {
        true
    }

    fn scroll_bars(&self, _tab: &Tab) -> [bool; 2] {
        [false, false]
    }
}

/// Options for automated runs (benchmarks and screenshots).
#[derive(Clone, Debug, Default)]
pub struct Automation {
    pub screenshot: Option<std::path::PathBuf>,
    pub screenshot_after_frames: u64,
    pub exit_after_screenshot: bool,
    pub bench: bool,
    pub bench_seconds: f32,
    /// Connect the assistant with the key from the environment or the keychain.
    pub connect_assistant: bool,
    /// Serve MCP on this loopback port (0 picks one).
    pub mcp_port: Option<u16>,
    /// Start in this render look instead of the default from the settings.
    pub preset: Option<Preset>,
}

struct Bench {
    reported: bool,
    started: Instant,
    warmup: Duration,
    frames: Vec<f32>,
    last: Instant,
}

pub struct SolidDraftApp {
    pub state: AppState,
    /// The middle dock: the part and the drawing.
    dock: DockState<Tab>,
    tree_dock: DockState<Tab>,
    assistant_dock: DockState<Tab>,
    /// The panel layout as last written to disk.
    saved_layout: Layout,
    saved_commands: crate::commands::History,
    automation: Automation,
    frame: u64,
    bench: Option<Bench>,
    screenshot_requested: bool,
    title: String,
    /// The GPU the window draws with, for part thumbnails. Desktop only.
    render_state: Option<egui_wgpu::RenderState>,
    /// The renderer part thumbnails are drawn with, made at the first save.
    offscreen: Option<crate::viewport::offscreen::Offscreen>,
}

impl SolidDraftApp {
    pub fn new(cc: &eframe::CreationContext<'_>, doc: Document, automation: Automation) -> Self {
        theme::install(&cc.egui_ctx);
        if let Some(rs) = cc.wgpu_render_state.as_ref() {
            let tier = Tier::of(rs.adapter.get_info().backend);
            let renderer = ViewportRenderer::new(&rs.device, &rs.queue, rs.target_format, tier);
            rs.renderer.write().callback_resources.insert(renderer);
        }
        log::debug!("renderer installed");
        #[cfg(target_arch = "wasm32")]
        crate::web::set_context(&cc.egui_ctx);
        let mut state = AppState::new(doc);
        state.viewport.camera.fit(state.doc.bounds());
        let preset = automation.preset.unwrap_or_else(crate::settings::default_preset);
        state.viewport.preset = preset;
        if !preset.shows_edges() {
            state.viewport.display = DisplayMode::Shaded;
        }
        let ctx = cc.egui_ctx.clone();
        let waker: std::sync::Arc<dyn Fn() + Send + Sync> = std::sync::Arc::new(move || ctx.request_repaint());
        if automation.connect_assistant {
            state.chat = crate::chat::ChatState::from_environment();
        }
        state.chat.set_waker(waker.clone());
        if let Some(port) = automation.mcp_port {
            match state.start_mcp(port, Some(waker)) {
                Ok(url) => {
                    log::info!("MCP server at {url}");
                    eprintln!("SolidDraft MCP server: {url}");
                    state.chat.status.push_str(&format!(" · MCP {url}"));
                }
                Err(e) => state.note(e, NoteKind::Error),
            }
        }
        let bench = automation.bench.then(|| Bench {
            reported: false,
            started: Instant::now(),
            warmup: Duration::from_secs(1),
            frames: Vec::new(),
            last: Instant::now(),
        });
        Self {
            saved_layout: state.layout,
            saved_commands: state.commands.clone(),
            state,
            dock: center_dock(),
            tree_dock: tree_dock(),
            assistant_dock: assistant_dock(),
            automation,
            frame: 0,
            bench,
            screenshot_requested: false,
            title: String::new(),
            #[cfg(not(target_arch = "wasm32"))]
            render_state: cc.wgpu_render_state.clone(),
            #[cfg(target_arch = "wasm32")]
            render_state: None,
            offscreen: None,
        }
    }

    /// Starts autosave with its session folder under `root`, and opens the Recover
    /// documents window when a session that ended without quitting left copies. The
    /// desktop binary calls it; tests and automated runs leave autosave off.
    pub fn start_recovery(&mut self, root: &std::path::Path, policy: soliddraft_document::AutosavePolicy) {
        match crate::recovery::Recovery::start(root, policy) {
            Ok(recovery) => self.state.recovery = Some(recovery),
            Err(e) => self.state.note(format!("Autosave is off: {e}"), NoteKind::Error),
        }
    }

    /// A PNG of the part as the viewport would first show it, for its file. `None` when
    /// there is no body or no GPU, or the picture failed; the save goes on without it.
    fn thumbnail(&mut self) -> Option<Vec<u8>> {
        let rs = self.render_state.as_ref()?;
        let doc = &self.state.doc;
        if doc.display_body().is_none() || doc.meshes.is_empty() {
            return None;
        }
        let offscreen = self.offscreen.get_or_insert_with(|| {
            let tier = Tier::of(rs.adapter.get_info().backend);
            crate::viewport::offscreen::Offscreen::new(&rs.device, &rs.queue, tier)
        });
        let picture = offscreen.capture(&rs.device, &rs.queue, doc, crate::viewport::offscreen::THUMBNAIL);
        match picture.and_then(|p| p.png()) {
            Ok(png) => Some(png),
            Err(e) => {
                log::warn!("no thumbnail for the part: {e}");
                None
            }
        }
    }

    /// Opens the document in a recovery copy in place of the open one of its kind.
    fn restore(&mut self, file: &std::path::Path) {
        use soliddraft_document::DocKind;
        let Some(recovery) = self.state.recovery.as_mut() else { return };
        let (record, bytes) = match recovery.restore(file) {
            Ok(taken) => taken,
            Err(e) => return self.state.note(e, NoteKind::Error),
        };
        match record.kind {
            DocKind::Drawing => match crate::drawing::document::DrawingDoc::from_bytes(&bytes) {
                Ok(mut doc) => {
                    doc.path = record.path.clone();
                    doc.touch();
                    let edits = doc.edits;
                    crate::drawing::restore(&mut self.state, &mut self.dock, doc);
                    if let Some(r) = self.state.recovery.as_mut() {
                        r.restored(record.id, DocKind::Drawing, edits);
                    }
                }
                Err(e) => self.state.note(format!("cannot restore {}: {e}", record.name), NoteKind::Error),
            },
            _ => match file::open_bytes(&record.name, &bytes, self.state.doc.kernel().clone()) {
                Ok(mut doc) => {
                    doc.name = record.name.clone();
                    doc.path = record.path.clone();
                    doc.touch();
                    let edits = doc.edits;
                    self.opened(Ok(doc));
                    self.state.note(format!("Restored {} · not saved yet", record.name), NoteKind::Ok);
                    if let Some(r) = self.state.recovery.as_mut() {
                        r.restored(record.id, DocKind::Part, edits);
                    }
                }
                Err(e) => self.state.note(format!("cannot restore {}: {e}", record.name), NoteKind::Error),
            },
        }
    }

    /// Shows a part that was just opened, or why it could not be.
    fn opened(&mut self, opened: Result<Document, String>) {
        match opened {
            Ok(doc) => {
                self.close_editor();
                self.state.sketch = None;
                self.state.chat.preview = None;
                let n = doc.features.len();
                self.state.doc = doc;
                self.state.remember_revision();
                self.state.viewport.camera.fit(self.state.doc.bounds());
                self.state.recent = file::recent();
                let problems = self.state.doc.error_count() + self.state.doc.repair_count();
                if problems == 0 {
                    self.state.note(format!("Opened {} · {n} features rebuilt", self.state.doc.name), NoteKind::Ok);
                } else {
                    self.state.note(
                        format!(
                            "Opened {} · {problems} feature{} need attention",
                            self.state.doc.name,
                            if problems == 1 { "" } else { "s" }
                        ),
                        NoteKind::Error,
                    );
                }
            }
            Err(e) => self.state.note(e, NoteKind::Error),
        }
    }

    /// Runs the commands whose shortcuts were pressed, as the command registry binds
    /// them for the current context. A key whose command is disabled and has nothing to
    /// say is left for the widgets.
    fn shortcuts(&mut self, ctx: &egui::Context) {
        // The open palette takes the plain keys, as a focused text field does.
        let typing = ctx.egui_wants_keyboard_input() || self.state.palette.is_some();
        let state = &self.state;
        let bindings: Vec<_> = commands::bindings(commands::Context::of(state))
            .into_iter()
            .filter(|(c, k)| (!typing || k.fires_while_typing()) && (c.is_enabled(state) || (c.hint)(state).is_some()))
            .collect();
        let mut fired = Vec::new();
        ctx.input_mut(|i| {
            for (command, key) in bindings {
                if i.consume_key(key.modifiers, key.key) {
                    fired.push(command);
                }
            }
        });
        for command in fired {
            self.state.run_command(command);
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let st = &mut self.state;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let (brand, _) = ui.allocate_exact_size(vec2(96.0, theme::MENU_HEIGHT), Sense::hover());
            let sq = Rect::from_center_size(pos2(brand.left() + 14.0, brand.center().y), vec2(15.0, 15.0));
            ui.painter().rect_filled(sq, CornerRadius::same(4), theme::ACCENT);
            ui.painter().rect_filled(
                Rect::from_min_max(sq.min + vec2(5.0, 5.0), sq.max),
                CornerRadius { nw: 0, ne: 0, sw: 0, se: 4 },
                theme::ACCENT.lerp_to_gamma(egui::Color32::from_rgb(0x49, 0xb3, 0xff), 0.7),
            );
            ui.painter().text(
                pos2(brand.left() + 27.0, brand.center().y),
                Align2::LEFT_CENTER,
                "SolidDraft",
                theme::font_semibold(13.0),
                theme::INK,
            );
            ui.add_space(6.0);

            ui.menu_button("File", |ui| {
                menu_command(ui, st, "new_part", "New part");
                menu_command(ui, st, "open", "Open…");
                menu_command(ui, st, "make_drawing", "New drawing from part…");
                ui.menu_button("Open recent", |ui| {
                    if st.recent.is_empty() {
                        ui.add_enabled(false, egui::Button::new("No recent files"));
                    }
                    for path in st.recent.clone() {
                        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        if ui.button(name).on_hover_text(path.display().to_string()).clicked() {
                            st.push(AppAction::Open(Some(path)));
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.add_enabled(!st.recent.is_empty(), egui::Button::new("Clear list")).clicked() {
                        st.push(AppAction::ClearRecent);
                        ui.close();
                    }
                });
                ui.separator();
                menu_command(ui, st, "save", "Save");
                menu_command(ui, st, "save_as", "Save as…");
                ui.menu_button("Export", |ui| {
                    menu_command(ui, st, "export_step", "STEP AP242…");
                    menu_command(ui, st, "export_stl", "STL…");
                    menu_command(ui, st, "export_3mf", "3MF…");
                    menu_command(ui, st, "export_obj", "OBJ…");
                });
                if st.recovery.as_ref().is_some_and(|r| !r.offers.is_empty())
                    && ui.button("Recover documents…").clicked()
                {
                    st.push(AppAction::ShowRecovery);
                    ui.close();
                }
                ui.separator();
                menu_command(ui, st, "quit", "Quit");
            });
            ui.menu_button("Edit", |ui| {
                // Rebuild sits where SolidWorks keeps it: Edit, Ctrl+B.
                let drawing = st.drawing.session.as_ref();
                let rebuild = match drawing.map(|s| s.marker()) {
                    Some(Some((marker, _))) => format!("Rebuild drawing · {}", marker.to_lowercase()),
                    _ => "Rebuild drawing".to_owned(),
                };
                menu_command(ui, st, "drawing_rebuild", &rebuild);
                ui.separator();
                let undo = st.doc.undo_label().map(|l| format!("Undo {l}")).unwrap_or("Undo".into());
                let redo = st.doc.redo_label().map(|l| format!("Redo {l}")).unwrap_or("Redo".into());
                menu_command(ui, st, "undo", &undo);
                menu_command(ui, st, "redo", &redo);
                ui.separator();
                let feature = st.selected_feature().and_then(|id| st.doc.feature(id));
                let is_sketch = feature.is_some_and(|f| f.is_sketch());
                let suppressed = feature.is_some_and(|f| f.suppressed);
                menu_command(ui, st, "edit_feature", if is_sketch { "Edit sketch" } else { "Edit feature" });
                menu_command(ui, st, "rename", "Rename feature");
                menu_command(ui, st, "suppress", if suppressed { "Unsuppress feature" } else { "Suppress feature" });
                menu_command(ui, st, "delete_feature", "Delete feature");
                ui.separator();
                menu_command(ui, st, "roll_to_end", "Roll to end");
            });
            ui.menu_button("View", |ui| {
                for id in
                    ["view_isometric", "view_front", "view_top", "view_right", "view_back", "view_left", "view_bottom"]
                {
                    menu_command(ui, st, id, commands::get(id).map(|c| c.label).unwrap_or(id));
                }
                menu_command(ui, st, "normal_to", "Normal to");
                ui.separator();
                menu_command(ui, st, "zoom_fit", "Zoom to fit");
                menu_command(
                    ui,
                    st,
                    "orthographic",
                    if st.viewport.camera.orthographic { "Perspective" } else { "Orthographic" },
                );
                ui.separator();
                for m in DisplayMode::ALL {
                    if ui.radio(st.viewport.display == m, m.label()).clicked() {
                        st.push(AppAction::SetDisplay(m));
                        ui.close();
                    }
                }
                ui.separator();
                for p in Preset::ALL {
                    if ui.radio(st.viewport.preset == p, p.label()).clicked() {
                        st.push(AppAction::SetPreset(p));
                        ui.close();
                    }
                }
                ui.separator();
                for side in Side::ALL {
                    let shown = st.layout.panel(side).shown;
                    if ui.add(egui::Button::selectable(shown, side.title()).shortcut_text(side.shortcut())).clicked() {
                        st.push(AppAction::TogglePanel(side));
                        ui.close();
                    }
                }
            });
            ui.menu_button("Insert", |ui| {
                menu_command(ui, st, "sketch", "Sketch");
                menu_command(ui, st, "extrude", "Boss extrude");
                menu_command(ui, st, "cut", "Cut extrude");
                menu_command(ui, st, "fillet", "Fillet");
                menu_command(ui, st, "chamfer", "Chamfer");
            });
            ui.menu_button("Tools", |ui| {
                menu_command(ui, st, "measure", "Measure");
                menu_command(ui, st, "check", "Check");
                ui.separator();
                menu_command(ui, st, "file_locations", "File locations…");
            });
            ui.menu_button("Window", |ui| {
                menu_command(ui, st, "reset_layout", "Reset panel layout");
                menu_command(ui, st, "ask", "Assistant");
                menu_command(ui, st, "command_palette", "Command palette");
            });
            ui.menu_button("Help", |ui| {
                menu_command(ui, st, "shortcuts", "Keyboard shortcuts");
                menu_command(ui, st, "about", "About SolidDraft");
            });

            ui.add_space(14.0);
            // Command search.
            let search_w = 270.0_f32.min(ui.available_width() - 180.0).max(120.0);
            let frame = egui::Frame::new()
                .fill(theme::PANEL)
                .stroke(Stroke::new(1.0, theme::LINE))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(6, 1));
            let inner = frame.show(ui, |ui| {
                ui.set_width(search_w);
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(13.0, 13.0), Sense::hover());
                    icons::paint(ui.painter(), Icon::Search, r, theme::MUTED);
                    ui.add(
                        egui::TextEdit::singleline(&mut st.search)
                            .hint_text("Search commands and features")
                            .frame(egui::Frame::NONE)
                            .desired_width(search_w - 26.0),
                    )
                })
                .inner
            });
            let resp = inner.inner;
            if resp.has_focus() || resp.changed() {
                search_popup(ui, st, &resp);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(10.0);
                let name = match &st.doc.path {
                    Some(p) => p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    None => format!("{}.{}", st.doc.name, file::EXTENSION),
                };
                let text = if st.doc.dirty { format!("{name} •") } else { name };
                ui.label(egui::RichText::new(text).size(12.0).color(theme::MUTED)).on_hover_text(
                    st.doc.path.as_ref().map(|p| p.display().to_string()).unwrap_or("Not saved yet".into()),
                );
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        if self.state.drawing.is_active() {
            crate::drawing::status_bar(ui, &mut self.state);
            return;
        }
        let st = &mut self.state;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 18.0;
            ui.add_space(4.0);
            let errors = st.doc.error_count();
            let repairs = st.doc.repair_count();
            if errors == 0 && repairs == 0 {
                ui.label(egui::RichText::new("Rebuild OK").family(theme::semibold()).size(11.5).color(theme::OK));
            } else if errors > 0 {
                ui.label(
                    egui::RichText::new(format!("{errors} rebuild error{}", if errors == 1 { "" } else { "s" }))
                        .family(theme::semibold())
                        .size(11.5)
                        .color(theme::ERROR),
                );
            } else {
                ui.label(
                    egui::RichText::new(format!("{repairs} feature{} to repair", if repairs == 1 { "" } else { "s" }))
                        .family(theme::semibold())
                        .size(11.5)
                        .color(theme::WARN),
                );
            }
            let n = st.doc.features.len();
            ui.label(
                egui::RichText::new(format!("{n} feature{}", if n == 1 { "" } else { "s" }))
                    .size(11.5)
                    .color(theme::MUTED),
            );
            ui.label(
                egui::RichText::new(format!("{} triangles", st.doc.triangle_count())).size(11.5).color(theme::MUTED),
            );
            let mode = match (&st.sketch, &st.editor) {
                (Some(s), _) => {
                    let name = st.doc.feature(s.feature).map(|f| f.name.clone()).unwrap_or("sketch".into());
                    format!("Editing {name} · {}", s.sketch.status.label())
                }
                (None, Some(e)) => format!("Editing {}", e.title()),
                _ => "Editing part".to_owned(),
            };
            ui.label(egui::RichText::new(mode).size(11.5).color(theme::MUTED));
            if let Some(note) = &st.note
                && note.at.elapsed() < Duration::from_secs(6)
            {
                let color = match note.kind {
                    NoteKind::Info => theme::MUTED,
                    NoteKind::Ok => theme::OK,
                    NoteKind::Error => theme::ERROR,
                };
                ui.label(egui::RichText::new(&note.text).size(11.5).color(color));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(10.0);
                let coords = match (st.sketch.as_ref().and_then(|s| s.cursor), st.hover_point) {
                    (Some(c), _) => format!("u {:.1}   v {:.1} mm", c.u, c.v),
                    (None, Some(p)) => format!("x {:.1}   y {:.1}   z {:.1} mm", p.x, p.y, p.z),
                    _ => "mm".to_owned(),
                };
                ui.label(egui::RichText::new(coords).size(11.5).color(theme::MUTED));
                ui.label(egui::RichText::new(format!("{:.1} ms", st.frame_ms)).size(11.5).color(theme::MUTED))
                    .on_hover_text("Frame time, smoothed");
            });
        });
    }

    fn apply_actions(&mut self, ctx: &egui::Context) {
        for action in self.state.take_actions() {
            self.apply(ctx, action);
        }
    }

    /// Closes the open editor, restoring the display the editor rolled back.
    fn close_editor(&mut self) -> Option<FeatureEditor> {
        let e = self.state.editor.take()?;
        if e.target.is_some() && self.state.doc.rollback != e.saved_rollback {
            self.state.doc.rollback = e.saved_rollback;
            let _ = self.state.doc.rebuild();
        }
        Some(e)
    }

    /// Opens an editor; an existing feature is shown as the body before it.
    fn open_editor(&mut self, target: Option<crate::document::FeatureId>, params: EditorParams) {
        self.close_editor();
        self.state.chat.preview = None;
        let saved = self.state.doc.rollback;
        if let Some(id) = target
            && let Some(i) = self.state.doc.index_of(id)
            && self.state.doc.rollback.is_none_or(|r| r > i)
        {
            self.state.doc.rollback = Some(i);
            let _ = self.state.doc.rebuild();
        }
        self.state.editor = Some(FeatureEditor::new(target, params, saved));
        self.state.ribbon_tab = crate::state::RibbonTab::Features;
    }

    fn open_sketch(&mut self, id: crate::document::FeatureId) {
        self.close_editor();
        let Some(f) = self.state.doc.feature(id) else { return };
        let (Some(plane), Some(sk)) = (f.plane, f.sketch()) else {
            self.state.note("The sketch plane could not be resolved; repair it first", NoteKind::Error);
            return;
        };
        let was_orthographic = self.state.viewport.camera.orthographic;
        let session = SketchSession::new(id, plane, sk.sketch.clone(), was_orthographic);
        self.state.sketch = Some(session);
        self.state.doc.selection = Some(Selection::Feature(id));
        self.state.ribbon_tab = crate::state::RibbonTab::Sketch;
        self.state.viewport.camera.orthographic = true;
        self.state.viewport.camera.look_along(glam::Vec3::new(
            plane.normal.x as f32,
            plane.normal.y as f32,
            plane.normal.z as f32,
        ));
        self.state.viewport.camera.fit(self.fit_bounds());
    }

    /// What "zoom to fit" frames: the body, the open sketch, or with nothing
    /// built yet a 200 mm workspace around the sketch origin.
    fn fit_bounds(&self) -> Option<(glam::Vec3, glam::Vec3)> {
        let mut acc = self.state.doc.bounds();
        if let Some(s) = &self.state.sketch {
            let f = crate::sketcher::Frame {
                plane: s.plane,
                camera: &self.state.viewport.camera,
                rect: self.state.viewport.rect,
            };
            let (lo, hi) = s
                .sketch
                .bounds()
                .unwrap_or((crate::sketch::P2::new(-100.0, -100.0), crate::sketch::P2::new(100.0, 100.0)));
            let corners = [
                f.world(lo),
                f.world(hi),
                f.world(crate::sketch::P2::new(lo.u, hi.v)),
                f.world(crate::sketch::P2::new(hi.u, lo.v)),
            ];
            for c in corners {
                acc = Some(match acc {
                    Some((a, b)) => (a.min(c), b.max(c)),
                    None => (c, c),
                });
            }
        }
        acc
    }

    fn apply(&mut self, ctx: &egui::Context, action: AppAction) {
        match action {
            AppAction::Command(cmd) => {
                // A sketch edit from the sketcher must not close the editor state.
                let is_sketch_edit = matches!(cmd, Command::EditSketch { .. });
                match self.state.doc.apply(cmd) {
                    Ok(Some(id)) => {
                        self.state.doc.selection = Some(Selection::Feature(id));
                        self.state.note("Feature added", NoteKind::Ok);
                    }
                    Ok(None) => {
                        if !is_sketch_edit {
                            self.state.note("Rebuilt", NoteKind::Ok);
                        }
                    }
                    Err(e) => self.state.note(e, NoteKind::Error),
                }
                self.sync_sketch_session();
            }
            AppAction::Undo if self.state.drawing.is_active() => {
                self.apply(ctx, AppAction::Drawing(crate::drawing::DrawingAction::Undo));
            }
            AppAction::Redo if self.state.drawing.is_active() => {
                self.apply(ctx, AppAction::Drawing(crate::drawing::DrawingAction::Redo));
            }
            AppAction::Drawing(a) => crate::drawing::apply(&mut self.state, &mut self.dock, ctx, a),
            AppAction::Undo => {
                self.close_editor();
                match self.state.doc.undo() {
                    Ok(l) => self.state.note(format!("Undid {l}"), NoteKind::Info),
                    Err(e) => self.state.note(e, NoteKind::Error),
                }
                self.sync_sketch_session();
            }
            AppAction::Redo => {
                self.close_editor();
                match self.state.doc.redo() {
                    Ok(l) => self.state.note(format!("Redid {l}"), NoteKind::Info),
                    Err(e) => self.state.note(e, NoteKind::Error),
                }
                self.sync_sketch_session();
            }
            AppAction::StartSketch => {
                if self.state.in_sketch() {
                    return;
                }
                let face = match self.state.doc.selection {
                    Some(Selection::Face(f)) => self.state.doc.face_ref(f),
                    _ => None,
                };
                match face {
                    Some(r) => self.apply(ctx, AppAction::StartSketchOn(SketchPlane::Face { face: r })),
                    None => {
                        self.open_editor(None, EditorParams::PickPlane);
                        self.state.origin_open = true;
                        self.state.note("Choose a plane in the tree or click a planar face", NoteKind::Info);
                    }
                }
            }
            AppAction::StartSketchOn(plane) => {
                self.close_editor();
                match self.state.doc.apply(Command::AddSketch { plane }) {
                    Ok(Some(id)) => self.open_sketch(id),
                    Ok(None) => {}
                    Err(e) => self.state.note(e, NoteKind::Error),
                }
            }
            AppAction::EditSketch(id) => {
                if self.state.sketch.as_ref().is_some_and(|s| s.feature == id) {
                    return;
                }
                self.apply(ctx, AppAction::ExitSketch(None));
                self.open_sketch(id);
            }
            AppAction::ExitSketch(into) => {
                let Some(session) = self.state.sketch.take() else { return };
                self.state.viewport.camera.orthographic = session.was_orthographic;
                self.state.ribbon_tab = crate::state::RibbonTab::Features;
                let id = session.feature;
                let empty = self.state.doc.feature(id).and_then(|f| f.sketch()).is_some_and(|s| s.sketch.is_empty());
                if empty && into.is_none() && self.state.doc.consumer_of(id).is_none() {
                    // An empty new sketch leaves nothing behind.
                    let _ = self.state.doc.apply(Command::DeleteFeature(id));
                    self.state.doc.selection = None;
                    return;
                }
                self.state.doc.selection = Some(Selection::Feature(id));
                match into {
                    Some(ExitInto::Extrude) => {
                        self.open_editor(
                            None,
                            EditorParams::Extrude(ExtrudeParams {
                                sketch: Some(id),
                                depth: 10.0,
                                ..Default::default()
                            }),
                        );
                    }
                    Some(ExitInto::Cut) => {
                        self.open_editor(None, EditorParams::Cut(CutParams::new(id)));
                    }
                    None => {}
                }
            }
            AppAction::StartExtrude => {
                if self.state.in_sketch() {
                    self.apply(ctx, AppAction::ExitSketch(Some(ExitInto::Extrude)));
                    return;
                }
                let sketch = self.state.selected_sketch().filter(|s| self.state.doc.consumer_of(*s).is_none());
                let params = match sketch {
                    Some(id) => ExtrudeParams { sketch: Some(id), depth: 10.0, ..Default::default() },
                    None => ExtrudeParams::default(),
                };
                self.open_editor(None, EditorParams::Extrude(params));
            }
            AppAction::StartCut => {
                if self.state.in_sketch() {
                    self.apply(ctx, AppAction::ExitSketch(Some(ExitInto::Cut)));
                    return;
                }
                match self.state.selected_sketch() {
                    Some(id) if self.state.doc.consumer_of(id).is_none() => {
                        self.open_editor(None, EditorParams::Cut(CutParams::new(id)));
                    }
                    _ => self.state.note("Select a sketch to cut with, or sketch on a face first", NoteKind::Info),
                }
            }
            AppAction::StartFillet | AppAction::StartChamfer => {
                let chamfer = matches!(action, AppAction::StartChamfer);
                let edges = match self.state.doc.selection {
                    Some(Selection::Edge(e)) => self.state.doc.edge_ref(e).into_iter().collect(),
                    _ => Vec::new(),
                };
                let params = if chamfer {
                    EditorParams::Chamfer(ChamferParams { edges, distance: 2.0 })
                } else {
                    EditorParams::Fillet(FilletParams { edges, radius: 4.0 })
                };
                self.open_editor(None, params);
                self.state.doc.selection = None;
            }
            AppAction::StartRepair(id) => {
                let Some(FeatureStatus::NeedsRepair(items)) = self.state.doc.feature(id).map(|f| f.status.clone())
                else {
                    return;
                };
                let n = items.len();
                self.open_editor(Some(id), EditorParams::Repair { items, chosen: vec![None; n], active: 0 });
                self.state.note("Click the replacement edge or face in the viewport", NoteKind::Info);
            }
            AppAction::EditFeature(id) => {
                let Some(f) = self.state.doc.feature(id) else { return };
                let params = match &f.kind {
                    FeatureKind::Sketch(_) => {
                        self.apply(ctx, AppAction::EditSketch(id));
                        return;
                    }
                    FeatureKind::Extrude(p) => EditorParams::Extrude(*p),
                    FeatureKind::Cut(p) => EditorParams::Cut(*p),
                    FeatureKind::Fillet(p) => EditorParams::Fillet(p.clone()),
                    FeatureKind::Chamfer(p) => EditorParams::Chamfer(p.clone()),
                };
                if self.state.in_sketch() {
                    self.apply(ctx, AppAction::ExitSketch(None));
                }
                self.open_editor(Some(id), params);
                self.state.doc.selection = Some(Selection::Feature(id));
            }
            AppAction::CommitEditor => {
                let Some(e) = self.close_editor() else { return };
                let cmd = match (e.target, e.params) {
                    (Some(id), EditorParams::Extrude(p)) => Command::EditExtrude { id, params: p },
                    (None, EditorParams::Extrude(p)) => Command::AddExtrude { params: p, via_assistant: false },
                    (Some(id), EditorParams::Cut(p)) => Command::EditCut { id, params: p },
                    (None, EditorParams::Cut(p)) => Command::AddCut { params: p },
                    (Some(id), EditorParams::Fillet(p)) => Command::EditFillet { id, params: p },
                    (None, EditorParams::Fillet(p)) => Command::AddFillet { params: p },
                    (Some(id), EditorParams::Chamfer(p)) => Command::EditChamfer { id, params: p },
                    (None, EditorParams::Chamfer(p)) => Command::AddChamfer { params: p },
                    (_, EditorParams::PickPlane) => return,
                    (Some(id), EditorParams::Repair { items, chosen, .. }) => {
                        let mut applied = 0;
                        for (item, choice) in items.iter().zip(chosen) {
                            if let Some(r) = choice {
                                match self.state.doc.apply(Command::Repair { id, index: item.index, replacement: r }) {
                                    Ok(_) => applied += 1,
                                    Err(e) => self.state.note(e, NoteKind::Error),
                                }
                            }
                        }
                        let status = self.state.doc.feature(id).map(|f| f.status.clone());
                        match status {
                            Some(FeatureStatus::Ok) => self.state.note(
                                format!("Repaired {applied} reference{}", if applied == 1 { "" } else { "s" }),
                                NoteKind::Ok,
                            ),
                            Some(FeatureStatus::NeedsRepair(_)) => {
                                self.state.note("Some references are still missing", NoteKind::Error)
                            }
                            Some(FeatureStatus::Error(e)) => self.state.note(e, NoteKind::Error),
                            _ => {}
                        }
                        return;
                    }
                    (None, EditorParams::Repair { .. }) => return,
                };
                self.apply(ctx, AppAction::Command(cmd));
            }
            AppAction::CancelEditor => {
                self.close_editor();
            }
            AppAction::DragDimension(step) => self.state.drag_dimension(step),
            AppAction::Select(sel) => {
                // Editors that collect picks take the click instead of selecting.
                if let Some(e) = self.state.editor.as_mut() {
                    match (&mut e.params, sel) {
                        (EditorParams::Fillet(_) | EditorParams::Chamfer(_), Some(Selection::Edge(edge))) => {
                            if let Some(r) = self.state.doc.edge_ref(edge)
                                && let Some(list) = e.edges_mut()
                            {
                                if let Some(i) = list.iter().position(|x| x.label == r.label) {
                                    list.remove(i);
                                } else {
                                    list.push(r);
                                }
                            }
                            return;
                        }
                        (EditorParams::PickPlane, Some(Selection::Face(f))) => {
                            match self.state.doc.face_ref(f) {
                                Some(r) => self.apply(ctx, AppAction::StartSketchOn(SketchPlane::Face { face: r })),
                                None => self.state.note("Sketches need a planar face", NoteKind::Error),
                            }
                            return;
                        }
                        (EditorParams::Repair { items, chosen, active }, Some(s)) => {
                            let wanted = items.get(*active).map(|i| i.kind);
                            let r = match (wanted, s) {
                                (Some(crate::document::RefKind::Edge), Selection::Edge(edge)) => {
                                    self.state.doc.edge_ref(edge).map(crate::document::EntityRef::Edge)
                                }
                                (Some(crate::document::RefKind::Face), Selection::Face(face)) => {
                                    self.state.doc.face_ref(face).map(crate::document::EntityRef::Face)
                                }
                                _ => None,
                            };
                            match r {
                                Some(r) => {
                                    if let Some(slot) = chosen.get_mut(*active) {
                                        *slot = Some(r);
                                    }
                                    if let Some(next) = chosen.iter().position(|c| c.is_none()) {
                                        *active = next;
                                    }
                                    e.error = None;
                                }
                                None => {
                                    e.error = Some(match wanted {
                                        Some(crate::document::RefKind::Edge) => {
                                            "Pick an edge for this reference".into()
                                        }
                                        _ => "Pick a planar face for this reference".into(),
                                    })
                                }
                            }
                            return;
                        }
                        _ => {}
                    }
                }
                self.state.doc.selection = sel;
                self.state.rename = None;
            }
            AppAction::SketchTool(tool) => {
                if let Some(s) = self.state.sketch.as_mut() {
                    s.set_tool(tool);
                }
            }
            AppAction::ToggleConstruction => {
                if let Some(s) = self.state.sketch.as_mut() {
                    let ids = s.selected_entities();
                    if ids.is_empty() {
                        s.say("Select entities to toggle construction");
                        return;
                    }
                    let all = ids.iter().all(|e| s.sketch.entity(*e).is_some_and(|e| e.construction));
                    for id in ids {
                        if let Some(e) = s.sketch.entity_mut(id) {
                            e.construction = !all;
                        }
                    }
                    crate::sketch::solve::solve(&mut s.sketch, None, true);
                    let cmd = Command::EditSketch { id: s.feature, sketch: s.sketch.clone() };
                    self.apply(ctx, AppAction::Command(cmd));
                }
            }
            AppAction::DeleteSelection => {
                if let Some(s) = self.state.sketch.as_mut() {
                    if s.selection.is_empty() {
                        return;
                    }
                    for sel in s.selection.clone() {
                        match sel {
                            crate::state::SketchSel::Entity(e) => s.sketch.remove_entity(e),
                            crate::state::SketchSel::Constraint(c) => s.sketch.remove_constraint(c),
                            crate::state::SketchSel::Point(_) => {}
                        }
                    }
                    s.selection.clear();
                    crate::sketch::solve::solve(&mut s.sketch, None, true);
                    let cmd = Command::EditSketch { id: s.feature, sketch: s.sketch.clone() };
                    self.apply(ctx, AppAction::Command(cmd));
                } else if self.state.editor.is_none()
                    && let Some(id) = self.state.selected_feature()
                {
                    self.apply(ctx, AppAction::Command(Command::DeleteFeature(id)));
                }
            }
            AppAction::RenameSelected => {
                if let Some(id) = self.state.selected_feature()
                    && let Some(f) = self.state.doc.feature(id)
                {
                    self.state.rename = Some(crate::state::Rename { feature: id, text: f.name.clone(), focus: true });
                }
            }
            AppAction::SetView(v) => self.state.viewport.camera.set_view(v),
            AppAction::NormalTo => {
                let normal = match (&self.state.sketch, self.state.doc.selection) {
                    (Some(s), _) => Some(s.plane.normal),
                    (None, Some(Selection::Face(f))) => self.state.doc.face_plane(f).map(|p| p.normal),
                    (None, _) => {
                        self.state.selected_sketch().and_then(|id| self.state.doc.feature(id)?.plane).map(|p| p.normal)
                    }
                };
                match normal {
                    Some(n) => {
                        self.state.viewport.camera.look_along(glam::Vec3::new(n.x as f32, n.y as f32, n.z as f32))
                    }
                    None => self.state.note("Select a planar face or a sketch first", NoteKind::Info),
                }
            }
            AppAction::ZoomFit => {
                let bounds = self.fit_bounds();
                self.state.viewport.camera.fit(bounds);
            }
            AppAction::SetDisplay(d) => self.state.viewport.display = d,
            AppAction::SetPreset(p) => {
                let viewport = &mut self.state.viewport;
                // Realistic leaves edges off; switching to or from it moves the shaded
                // modes along, and a wireframe stays a wireframe.
                if p.shows_edges() != viewport.preset.shows_edges() && viewport.display != DisplayMode::Wireframe {
                    viewport.display = if p.shows_edges() { DisplayMode::ShadedWithEdges } else { DisplayMode::Shaded };
                }
                viewport.preset = p;
            }
            AppAction::ToggleProjection => {
                self.state.viewport.camera.orthographic = !self.state.viewport.camera.orthographic
            }
            AppAction::PreviewProposal(id) => {
                self.close_editor();
                self.state.preview_proposal(&id);
            }
            AppAction::ApplyProposal(id) => {
                self.close_editor();
                self.state.apply_proposal(&id);
            }
            AppAction::UndoProposal(id) => self.state.undo_proposal(&id),
            AppAction::DiscardProposal(id) => self.state.discard_proposal(&id),
            AppAction::FocusChat => {
                self.state.layout.assistant.shown = true;
                self.state.chat.focus_requested = true;
            }
            AppAction::SendChat => self.state.send_chat(),
            AppAction::StopChat => self.state.stop_chat(),
            AppAction::Ask(text) => {
                self.state.chat.draft = text;
                self.state.send_chat();
            }
            AppAction::ToggleAutoApply => self.state.toggle_auto_apply(),
            // The web build's provider, model and key are set on its server.
            #[cfg(target_arch = "wasm32")]
            AppAction::ShowProviderSettings => {
                self.state.note("The assistant's provider and key are set on the SolidDraft server", NoteKind::Info)
            }
            #[cfg(not(target_arch = "wasm32"))]
            AppAction::ShowProviderSettings => {
                self.state.provider_dialog = Some(crate::chat::ProviderDialog::new(&crate::settings::assistant()))
            }
            AppAction::NewPart => {
                self.close_editor();
                self.state.sketch = None;
                self.state.chat.preview = None;
                self.state.doc = crate::new_document();
                self.state.viewport.camera.fit(None);
                self.state.note("New part", NoteKind::Info);
            }
            AppAction::Open(path) => {
                let path =
                    path.or_else(|| crate::platform::open_path("Open", "SolidDraft documents", &file::OPEN_EXTENSIONS));
                let Some(path) = path else { return };
                match soliddraft_document::DocKind::of_path(&path) {
                    Some(soliddraft_document::DocKind::Drawing) => {
                        self.apply(ctx, AppAction::Drawing(crate::drawing::DrawingAction::OpenFile(path)));
                    }
                    Some(soliddraft_document::DocKind::Assembly) => {
                        self.state.note("Assemblies arrive in a later phase", NoteKind::Error);
                    }
                    _ => {
                        let opened = file::open(&path, self.state.doc.kernel().clone());
                        self.opened(opened);
                    }
                }
            }
            AppAction::Save => match self.state.doc.path.clone() {
                Some(p) => self.apply(ctx, AppAction::SaveAs(Some(p))),
                None => self.apply(ctx, AppAction::SaveAs(None)),
            },
            AppAction::SaveAs(path) => {
                let path = path.or_else(|| {
                    let name = format!("{}.{}", self.state.doc.name, file::EXTENSION);
                    crate::platform::save_path("Save part", "SolidDraft part", file::EXTENSION, &name)
                });
                let Some(path) = path else { return };
                let path = file::with_extension(&path);
                let thumbnail = self.thumbnail();
                let extras = file::Extras { thumbnail: thumbnail.as_deref(), ..Default::default() };
                match file::save_with(&mut self.state.doc, &path, &extras) {
                    Ok(warning) => {
                        self.state.remember_revision();
                        self.state.recent = file::recent();
                        match warning {
                            Some(w) => self.state.note(format!("Saved {} · {w}", path.display()), NoteKind::Error),
                            None => self.state.note(format!("Saved {}", path.display()), NoteKind::Ok),
                        }
                    }
                    Err(e) => self.state.note(e, NoteKind::Error),
                }
            }
            AppAction::Export { format, path } => {
                let path = path.or_else(|| {
                    let name = format!("{}.{}", self.state.doc.name, format.extension());
                    let title = format!("Export {}", format.name());
                    crate::platform::save_path(&title, format.name(), format.extension(), &name)
                });
                let Some(path) = path else { return };
                let exported = self.state.doc.model().export(format);
                match exported.and_then(|out| crate::platform::write_file(&path, &out.bytes).map(|()| out.report)) {
                    Ok(report) => {
                        let kind = match &report {
                            soliddraft_exchange::Report::Step(r) if !r.within_tolerance => NoteKind::Error,
                            soliddraft_exchange::Report::Mesh(r) if !r.closed => NoteKind::Error,
                            _ => NoteKind::Ok,
                        };
                        self.state.note(format!("Exported {} · {}", path.display(), report.summary()), kind);
                    }
                    Err(e) => self.state.note(format!("Export: {e}"), NoteKind::Error),
                }
            }
            AppAction::ClearRecent => {
                file::forget_all();
                self.state.recent.clear();
            }
            AppAction::ResetLayout => {
                let drawing_open = self.dock.find_tab(&Tab::Drawing).is_some();
                self.dock = center_dock();
                if drawing_open {
                    self.dock.push_to_focused_leaf(Tab::Drawing);
                }
                self.tree_dock = tree_dock();
                self.assistant_dock = assistant_dock();
                self.state.layout = Layout::default();
                // Forget the widths egui kept, so the defaults apply again.
                ctx.data_mut(|d| {
                    for side in Side::ALL {
                        d.remove::<egui::PanelState>(side.panel_id());
                    }
                });
            }
            AppAction::TogglePanel(side) => self.state.layout.toggle(side),
            AppAction::ShowFileLocations(kind) => {
                self.state.locations_dialog = Some(crate::locations::LocationsDialog::new(kind));
            }
            AppAction::ShowShortcuts => self.state.show_shortcuts = true,
            AppAction::TogglePalette => crate::palette::toggle(&mut self.state),
            AppAction::ShowAbout => self.state.show_about = true,
            AppAction::ShowRecovery => {
                if let Some(r) = self.state.recovery.as_mut() {
                    r.show = true;
                }
            }
            AppAction::RestoreRecovery(file) => self.restore(&file),
            AppAction::DiscardRecovery(file) => {
                if let Some(Err(e)) = self.state.recovery.as_mut().map(|r| r.discard(&file)) {
                    self.state.note(e, NoteKind::Error);
                }
            }
            AppAction::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
    }

    /// After the document changed under an open sketch (undo, redo, a command),
    /// the session takes the document's copy so both agree.
    fn sync_sketch_session(&mut self) {
        let Some(s) = self.state.sketch.as_mut() else { return };
        match self.state.doc.feature(s.feature).and_then(|f| f.sketch()) {
            Some(sk) => {
                if sk.sketch != s.sketch {
                    s.sketch = sk.sketch.clone();
                    s.selection.retain(|sel| match sel {
                        crate::state::SketchSel::Entity(e) => s.sketch.entity(*e).is_some(),
                        crate::state::SketchSel::Point(p) => s.sketch.point(*p).is_some(),
                        crate::state::SketchSel::Constraint(c) => s.sketch.constraint(*c).is_some(),
                    });
                }
                if let Some(plane) = self.state.doc.feature(s.feature).and_then(|f| f.plane) {
                    s.plane = plane;
                }
            }
            None => {
                // The sketch is gone (undone past its creation).
                self.state.sketch = None;
                self.state.ribbon_tab = crate::state::RibbonTab::Features;
            }
        }
    }

    /// One side area: a resizable panel while shown, a strip with its title while
    /// hidden. Dragging the edge past the minimum hides it; clicking the strip,
    /// dragging it outward or double-clicking the edge shows it again.
    fn side_panel(&mut self, root: &mut egui::Ui, side: Side, window_width: f32) {
        let frame = egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::ZERO);
        let make = |id: egui::Id| match side {
            Side::Tree => egui::Panel::left(id),
            Side::Assistant => egui::Panel::right(id),
        };
        let collapsed = make(side.strip_id()).resizable(true).exact_size(panels::STRIP_WIDTH).frame(frame);
        let expanded = make(side.panel_id())
            .resizable(true)
            .default_size(self.state.layout.panel(side).width)
            .size_range(side.min_width()..=self.state.layout.max_width(side, window_width))
            .frame(frame);
        let title = match side {
            Side::Tree if self.state.drawing.is_active() => "Drawing tree",
            _ => side.title(),
        };
        let mut shown = self.state.layout.panel(side).shown;
        let mut open_requested = false;
        let dock = match side {
            Side::Tree => &mut self.tree_dock,
            Side::Assistant => &mut self.assistant_dock,
        };
        let mut panels = Panels { state: &mut self.state };
        egui::Panel::show_switched(root, &mut shown, collapsed, expanded, |ui, expanded| {
            if expanded {
                show_dock(ui, dock, side.panel_id().with("dock"), &mut panels);
            } else if strip(ui, side, title) {
                open_requested = true;
            }
        });
        let panel = self.state.layout.panel_mut(side);
        panel.shown = shown || open_requested;
        if let Some(s) = egui::PanelState::load(root.ctx(), side.panel_id()) {
            panel.width = s.outer_rect.width();
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        crate::palette::show(ctx, &mut self.state);
        let st = &mut self.state;
        if st.show_shortcuts {
            egui::Window::new("Keyboard shortcuts")
                .collapsible(false)
                .resizable(false)
                .open(&mut st.show_shortcuts)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().min_scrolled_height(600.0).max_height(640.0).show(ui, shortcuts_grid);
                });
        }
        if st.show_about {
            egui::Window::new("About SolidDraft").collapsible(false).resizable(false).open(&mut st.show_about).show(
                ctx,
                |ui| {
                    ui.label(egui::RichText::new("SolidDraft").family(theme::semibold()).size(16.0));
                    ui.label("A parametric CAD application written in Rust.");
                    ui.label(
                        egui::RichText::new(format!(
                            "Version {} · phase 2 modelling · kernel: {}",
                            env!("CARGO_PKG_VERSION"),
                            st.doc.backend_name()
                        ))
                        .color(theme::MUTED),
                    );
                },
            );
        }
        self.assistant_settings(ctx);
        crate::drawing::reference::window(ctx, &mut self.state);
        crate::locations::window(ctx, &mut self.state);
        crate::recovery::window(ctx, &mut self.state);
    }

    /// The assistant settings dialog: the provider, an API key per provider and the models.
    fn assistant_settings(&mut self, ctx: &egui::Context) {
        use soliddraft_assistant::key::KeySource;
        use soliddraft_assistant::{ProviderKind, choice};
        enum Act {
            StoreKey(ProviderKind),
            DeleteKey(ProviderKind),
            Save,
        }
        let st = &mut self.state;
        let busy = st.chat.busy();
        let mut open = st.provider_dialog.is_some();
        let mut acts = Vec::new();
        if let Some(dialog) = st.provider_dialog.as_mut() {
            egui::Window::new("Assistant settings").collapsible(false).resizable(false).open(&mut open).show(
                ctx,
                |ui| {
                    ui.label(egui::RichText::new("Provider").family(theme::semibold()));
                    ui.radio_value(&mut dialog.provider, None, "Automatic: the provider with a key, Anthropic if both");
                    for kind in ProviderKind::ALL {
                        ui.radio_value(&mut dialog.provider, Some(kind), kind.label());
                    }
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("API keys").family(theme::semibold()));
                    egui::Grid::new("assistant_keys").num_columns(3).spacing([12.0, 6.0]).show(ui, |ui| {
                        for row in &mut dialog.keys {
                            let kind = row.kind;
                            ui.label(kind.label());
                            let text = match &row.found {
                                Some((KeySource::Environment(_), masked)) => {
                                    format!("{masked} from {}, which overrides the keychain", kind.key_var())
                                }
                                Some((KeySource::Keychain(_), masked)) => format!("{masked} in the OS keychain"),
                                None => "No key".into(),
                            };
                            ui.label(egui::RichText::new(text).color(theme::MUTED));
                            if ui
                                .add_enabled(row.in_keychain, egui::Button::new("Delete"))
                                .on_hover_text("Remove the key from the OS keychain")
                                .clicked()
                            {
                                acts.push(Act::DeleteKey(kind));
                            }
                            ui.end_row();
                            ui.label("");
                            ui.add(
                                egui::TextEdit::singleline(&mut row.entry)
                                    .password(true)
                                    .hint_text(format!("Paste a new {kind} key"))
                                    .desired_width(260.0),
                            );
                            let label = if row.found.is_some() { "Replace" } else { "Save" };
                            if ui.add_enabled(!row.entry.trim().is_empty(), egui::Button::new(label)).clicked() {
                                acts.push(Act::StoreKey(kind));
                            }
                            ui.end_row();
                        }
                    });
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("Models").family(theme::semibold()));
                    egui::Grid::new("assistant_models").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                        for (kind, model) in &mut dialog.models {
                            ui.label(kind.label());
                            ui.add(
                                egui::TextEdit::singleline(model).hint_text(kind.default_model()).desired_width(260.0),
                            );
                            ui.end_row();
                        }
                    });
                    for var in [choice::PROVIDER_VAR, choice::MODEL_VAR] {
                        if soliddraft_assistant::environment::var(var).is_some_and(|v| !v.trim().is_empty()) {
                            ui.label(
                                egui::RichText::new(format!("{var} or its legacy alias overrides this setting."))
                                    .color(theme::MUTED),
                            );
                        }
                    }
                    ui.add_space(8.0);
                    if ui
                        .add_enabled(!busy, egui::Button::new("Save and reconnect"))
                        .on_disabled_hover_text("Wait for the assistant to finish its answer")
                        .clicked()
                    {
                        acts.push(Act::Save);
                    }
                },
            );
        }
        if !open {
            st.provider_dialog = None;
        }
        for act in acts {
            match act {
                Act::StoreKey(kind) => st.store_key(kind),
                Act::DeleteKey(kind) => st.delete_key(kind),
                Act::Save => st.save_provider_settings(),
            }
        }
    }

    fn automation_step(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.frame += 1;
        if let Some(bench) = self.bench.as_mut() {
            let now = Instant::now();
            if now.duration_since(bench.started) > bench.warmup {
                bench.frames.push(now.duration_since(bench.last).as_secs_f32() * 1000.0);
            }
            bench.last = now;
            // Orbit continuously so every frame renders a new view.
            self.state.viewport.camera.orbit(3.0, 0.6);
            ctx.request_repaint();
            if !bench.reported
                && now.duration_since(bench.started).as_secs_f32()
                    > bench.warmup.as_secs_f32() + self.automation.bench_seconds
            {
                bench.reported = true;
                let mut f = bench.frames.clone();
                f.sort_by(|a, b| a.total_cmp(b));
                let n = f.len().max(1);
                let mean: f32 = f.iter().sum::<f32>() / n as f32;
                let pct = |p: f32| f[((n as f32 - 1.0) * p) as usize];
                let info = frame.wgpu_render_state().map(|rs| rs.adapter.get_info());
                println!("soliddraft bench");
                println!(
                    "  adapter: {}",
                    info.as_ref()
                        .map(|i| format!("{} ({:?}, {:?})", i.name, i.backend, i.device_type))
                        .unwrap_or_default()
                );
                println!("  triangles: {}", self.state.doc.triangle_count());
                println!("  frames measured: {n} over {:.1} s", bench.frames.iter().sum::<f32>() / 1000.0);
                println!(
                    "  frame time ms: mean {mean:.2}  median {:.2}  p95 {:.2}  p99 {:.2}  max {:.2}",
                    pct(0.5),
                    pct(0.95),
                    pct(0.99),
                    f[n - 1]
                );
                println!("  fps: mean {:.0}  at p95 {:.0}", 1000.0 / mean, 1000.0 / pct(0.95));
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        if let Some(path) = self.automation.screenshot.clone() {
            if self.frame == self.automation.screenshot_after_frames && !self.screenshot_requested {
                self.screenshot_requested = true;
                log::info!("screenshot requested at frame {}", self.frame);
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            }
            if self.frame < self.automation.screenshot_after_frames + 2 {
                ctx.request_repaint();
            }
            let image = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(image) = image {
                let (w, h) = (image.width() as u32, image.height() as u32);
                let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
                match image::save_buffer(&path, &bytes, w, h, image::ColorType::Rgba8) {
                    Ok(()) => println!("screenshot saved to {} ({w}x{h})", path.display()),
                    Err(e) => eprintln!("screenshot failed: {e}"),
                }
                if self.automation.exit_after_screenshot {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }
}

/// Maps an id-buffer pick to the document entity it stands for.
fn selection_of(doc: &Document, picked: Picked) -> Option<Selection> {
    match doc.pick(picked.pick())? {
        crate::document::PickEntry::Face(_, f) => Some(Selection::Face(f)),
        crate::document::PickEntry::Edge(_, e) => Some(Selection::Edge(e)),
        crate::document::PickEntry::Vertex(k, _) => Some(Selection::Vertex(k)),
    }
}

/// A menu entry for a registry command, in the menu's own wording, with the command's
/// shortcut and enabled check.
fn menu_command(ui: &mut egui::Ui, st: &mut AppState, id: &str, label: &str) {
    let Some(command) = commands::get(id) else {
        debug_assert!(false, "no command {id}");
        return;
    };
    let button = egui::Button::new(label).shortcut_text(command.shortcut_text().unwrap_or_default());
    if ui.add_enabled(command.is_enabled(st), button).on_hover_text(command.tip(st)).clicked() {
        st.run_command(command);
        ui.close();
    }
}

/// Every shortcut in the registry, by where it works, then the mouse.
fn shortcuts_grid(ui: &mut egui::Ui) {
    egui::Grid::new("shortcuts").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
        for scope in commands::Scope::ALL {
            let rows: Vec<(String, &str)> = commands::all()
                .iter()
                .filter(|c| c.in_app())
                .filter_map(|c| {
                    let keys: Vec<String> = c.keys.iter().filter(|k| k.scope == scope).map(|k| k.text()).collect();
                    (!keys.is_empty()).then(|| (keys.join(" / "), c.doc))
                })
                .collect();
            if rows.is_empty() {
                continue;
            }
            ui.label(egui::RichText::new(scope.label()).family(theme::semibold()).color(theme::MUTED));
            ui.end_row();
            for (keys, doc) in rows {
                ui.label(egui::RichText::new(keys).family(theme::semibold()));
                ui.label(doc);
                ui.end_row();
            }
        }
        ui.label(egui::RichText::new("Mouse").family(theme::semibold()).color(theme::MUTED));
        ui.end_row();
        for (k, v) in [
            ("Left drag", "Orbit (outside a sketch)"),
            ("Right or middle drag", "Pan (also Shift + left drag)"),
            ("Wheel", "Zoom toward the cursor"),
            ("Click", "Select vertex, edge or face"),
        ] {
            ui.label(egui::RichText::new(k).family(theme::semibold()));
            ui.label(v);
            ui.end_row();
        }
    });
}

/// The command search under the menu bar: registry commands through
/// [`commands::search`], then features by name.
fn search_popup(ui: &mut egui::Ui, st: &mut AppState, anchor: &egui::Response) {
    enum Pick {
        Command(&'static commands::CommandSpec),
        Feature(crate::document::FeatureId),
    }
    let query = st.search.trim().to_lowercase();
    if query.is_empty() {
        return;
    }
    let mut entries: Vec<(String, String, bool, Pick)> = commands::search(&query, &st.commands)
        .into_iter()
        .map(|m| {
            let c = m.command;
            (c.label.to_owned(), c.shortcut_text().unwrap_or_default(), c.is_enabled(st), Pick::Command(c))
        })
        .collect();
    for f in &st.doc.features {
        if f.name.to_lowercase().contains(&query) {
            entries.push((format!("{} (feature)", f.name), String::new(), true, Pick::Feature(f.id)));
        }
    }
    entries.truncate(8);
    let enter = ui.input(|i| i.key_pressed(Key::Enter));
    let mut chosen = None;
    egui::Popup::from_response(anchor).id(egui::Id::new("search_popup")).open(true).show(|ui| {
        ui.set_min_width(anchor.rect.width());
        if entries.is_empty() {
            ui.label(egui::RichText::new("No matching command").color(theme::MUTED));
        }
        for (i, (name, keys, enabled, _)) in entries.iter().enumerate() {
            let r =
                ui.add_enabled(*enabled, egui::Button::selectable(i == 0, name.as_str()).shortcut_text(keys.as_str()));
            if r.clicked() || (enter && i == 0 && *enabled) {
                chosen = Some(i);
            }
        }
    });
    if let Some(i) = chosen {
        st.search.clear();
        match entries.swap_remove(i).3 {
            Pick::Command(c) => st.run_command(c),
            Pick::Feature(id) => st.push(AppAction::Select(Some(Selection::Feature(id)))),
        }
    }
}

/// A file to open at startup, for `--open`.
pub fn open_at_start(app: &mut SolidDraftApp, path: PathBuf) {
    app.state.push(AppAction::Open(Some(path)));
}

impl SolidDraftApp {
    /// Hands autosave the documents as this frame left them, notes any copy that failed,
    /// and wakes the app when the next copy is due.
    fn autosave(&mut self, ctx: &egui::Context) {
        let Some(recovery) = self.state.recovery.as_mut() else { return };
        let backend = self.state.doc.kernel().backend();
        let kernel = format!("{} {}", backend.name, backend.version);
        let drawing = self.state.drawing.session.as_ref().map(|s| &s.doc);
        let wait = recovery.tick(&self.state.doc, drawing, &kernel, std::time::Instant::now());
        let failures = std::mem::take(&mut recovery.failures);
        if let Some(wait) = wait {
            ctx.request_repaint_after(wait);
        }
        if let Some(failure) = failures.into_iter().last() {
            self.state.note(failure, NoteKind::Error);
        }
    }
}

impl eframe::App for SolidDraftApp {
    /// A clean exit: autosave finishes and deletes this session's recovery copies.
    fn on_exit(&mut self) {
        if let Some(recovery) = self.state.recovery.take() {
            recovery.close();
        }
    }

    fn ui(&mut self, root: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = &root.ctx().clone();
        log::debug!("ui frame {}", self.frame);
        let dt = ctx.input(|i| i.unstable_dt) * 1000.0;
        self.state.frame_ms = if self.state.frame_ms == 0.0 { dt } else { self.state.frame_ms * 0.9 + dt * 0.1 };
        self.shortcuts(ctx);
        crate::drawing::frame(&mut self.state, &mut self.dock);
        self.state.poll_assistant();
        self.state.service_mcp();
        // In the browser an opened file arrives as an upload, frames after the request.
        if let Some((name, bytes)) = crate::platform::take_upload() {
            let opened = file::open_bytes(&name, &bytes, self.state.doc.kernel().clone());
            self.opened(opened);
        }

        if let Some(e) = self.state.editor.as_mut() {
            e.refresh(&self.state.doc);
        }
        // The answers arrive between frames; look again soon while one is owed.
        if self.state.poll_drag() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        // Hover info is derived from the viewport's last pick so every panel sees the same name.
        self.state.hover_info = self
            .state
            .viewport
            .hover
            .and_then(|p| selection_of(&self.state.doc, p))
            .and_then(|sel| self.state.doc.describe(sel));
        self.state.hover_point = self.state.viewport.hover_point;

        let panel_frame = egui::Frame::new().fill(theme::PANEL).inner_margin(egui::Margin::ZERO);
        egui::Panel::top("menu")
            .exact_size(theme::MENU_HEIGHT)
            .frame(panel_frame)
            .show_separator_line(true)
            .show(root, |ui| self.menu_bar(ui));
        egui::Panel::top("ribbon")
            .exact_size(theme::RIBBON_TABS_HEIGHT + theme::RIBBON_BODY_HEIGHT)
            .frame(panel_frame)
            .show_separator_line(false)
            .show(root, |ui| ribbon::show(ui, &mut self.state));
        egui::Panel::bottom("status")
            .exact_size(theme::STATUS_HEIGHT)
            .frame(panel_frame)
            .show_separator_line(true)
            .show(root, |ui| self.status_bar(ui));
        let window_width = root.available_width();
        self.side_panel(root, Side::Tree, window_width);
        self.side_panel(root, Side::Assistant, window_width);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::BG)).show(root, |ui| {
            let mut panels = Panels { state: &mut self.state };
            show_dock(ui, &mut self.dock, egui::Id::new("center_dock"), &mut panels);
        });
        self.dialogs(ctx);
        self.apply_actions(ctx);
        // Panel choices are written once they settle, never in the middle of a drag.
        if self.state.commands != self.saved_commands {
            if let Err(e) = crate::settings::save_command_history(&self.state.commands) {
                log::warn!("cannot save the command history: {e}");
            }
            self.saved_commands = self.state.commands.clone();
        }
        if self.state.layout != self.saved_layout && !ctx.input(|i| i.pointer.any_down()) {
            self.state.layout.save();
            self.saved_layout = self.state.layout;
        }
        self.autosave(ctx);
        self.automation_step(ctx, frame);
        if self.state.note.as_ref().is_some_and(|n| n.at.elapsed() < Duration::from_secs(6)) {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        let title = format!("{}{} - SolidDraft", self.state.doc.name, if self.state.doc.dirty { " •" } else { "" });
        if title != self.title {
            self.title = title.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
    }
}
