//! Menus, status, dialogs and shortcuts.

pub(super) mod docks;

use crate::command_line::{Status, StatusNote};
use crate::commands;
use crate::document::Selection;
use crate::file;
use crate::icons::{self, Icon};
use crate::panels::{self, Side};
use crate::state::{AppAction, AppState, EditorParams, NoteKind};
use crate::theme;
use crate::viewport::look::Preset;
use crate::viewport::render::DisplayMode;
use egui::{Align2, CornerRadius, Key, Rect, Sense, Stroke, pos2, vec2};

use super::workspace::SolidDraftApp;
use docks::{Panels, show_dock, strip};

impl SolidDraftApp {
    /// Runs the commands whose shortcuts were pressed, as the command registry binds
    /// them for the current context. A key whose command is disabled and has nothing to
    /// say is left for the widgets.
    pub(super) fn shortcuts(&mut self, ctx: &egui::Context) {
        // The open palette takes the plain keys, as a focused text field does.
        let typing = ctx.egui_wants_keyboard_input() || self.state.palette.is_some();
        let state = &self.state;
        let context = commands::Context::of(state);
        // The view-key profile's rows run first. A row that took over a command's key
        // (Onshape's N) leaves that command off the key.
        let view_keys = (context.part() && !typing).then(|| crate::navigation::view_keys::in_use(&state.navigation));
        let superseded: Vec<(&str, Key)> = view_keys.iter().flat_map(|v| v.superseded()).collect();
        // Escape closes an open menu or ribbon row, not what is underneath it.
        let popup = egui::Popup::is_any_open(ctx);
        let bindings: Vec<_> = commands::bindings(context)
            .into_iter()
            .filter(|(_, k)| !(popup && k.key == Key::Escape))
            .filter(|(c, k)| (!typing || k.fires_while_typing()) && (c.is_enabled(state) || (c.hint)(state).is_some()))
            .filter(|(c, k)| !superseded.contains(&(c.id, k.key)))
            .collect();
        // A key the navigation preset holds to navigate, as Inventor's F2, is left to the
        // viewport; its own command runs when it comes up without a drag having used it.
        let mac = ctx.os() == egui::os::OperatingSystem::Mac;
        let router = &mut self.state.viewport.router;
        router.configure(&self.state.navigation, mac);
        let hold = |k: Key| router.is_hold_key(crate::navigation::NavKey::main(k));
        let mut fired = Vec::new();
        let mut views = Vec::new();
        ctx.input_mut(|i| {
            if let Some(v) = &view_keys {
                i.events.retain(|e| {
                    let egui::Event::Key { key, physical_key, pressed: true, repeat: false, modifiers } = e else {
                        return true;
                    };
                    let key = physical_key.unwrap_or(*key);
                    let nav = crate::navigation::NavKey { key, location: crate::navigation::key_location(key) };
                    match v.row_for(nav, *modifiers, context.sketch, mac) {
                        Some(row) => {
                            views.push(row.action);
                            false
                        }
                        None => true,
                    }
                });
            }
            let mut taps: Vec<(Key, egui::Modifiers)> = i
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key { key, physical_key, pressed: false, modifiers, .. } => {
                        Some((physical_key.unwrap_or(*key), *modifiers))
                    }
                    _ => None,
                })
                .filter(|(k, _)| !typing && hold(*k) && !router.held_key_used(crate::navigation::NavKey::main(*k)))
                .collect();
            for (command, key) in &bindings {
                if !typing && hold(key.key) {
                    if let Some(t) = taps.iter().position(|(k, m)| *k == key.key && m.matches_logically(key.modifiers))
                    {
                        taps.remove(t);
                        fired.push(*command);
                    }
                } else if i.consume_key(key.modifiers, key.key) {
                    fired.push(*command);
                }
            }
        });
        for action in views {
            crate::navigation::view_keys::run(&mut self.state, action);
        }
        for command in fired {
            self.state.run_command(command);
        }
    }

    pub(super) fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let st = &mut self.state;
        let preferences = &mut self.preferences;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let (brand, _) = ui.allocate_exact_size(vec2(96.0, theme::MENU_HEIGHT), Sense::hover());
            let sq = Rect::from_center_size(pos2(brand.left() + 14.0, brand.center().y), vec2(15.0, 15.0));
            ui.painter().rect_filled(sq, CornerRadius::same(4), theme::accent());
            ui.painter().rect_filled(
                Rect::from_min_max(sq.min + vec2(5.0, 5.0), sq.max),
                CornerRadius { nw: 0, ne: 0, sw: 0, se: 4 },
                theme::accent().lerp_to_gamma(egui::Color32::from_rgb(0x49, 0xb3, 0xff), 0.7),
            );
            ui.painter().text(
                pos2(brand.left() + 27.0, brand.center().y),
                Align2::LEFT_CENTER,
                "SolidDraft",
                theme::font_semibold(13.0),
                theme::ink(),
            );
            ui.add_space(6.0);

            ui.menu_button("File", |ui| {
                menu_command(ui, st, "new_part", "New part");
                menu_command(ui, st, "open", "Open…");
                menu_command(ui, st, "import_step", "Import STEP…");
                menu_command(ui, st, "make_drawing", "New drawing from part…");
                menu_command(ui, st, "new_assembly", "New assembly");
                menu_command(ui, st, "make_assembly", "Make assembly from part");
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
                if st.assembly.is_active() {
                    menu_command(ui, st, "assembly_save", "Save assembly");
                    menu_command(ui, st, "assembly_save_as", "Save assembly as…");
                    menu_command(ui, st, "insert_component", "Insert component…");
                } else {
                    menu_command(ui, st, "save", "Save");
                    menu_command(ui, st, "save_as", "Save as…");
                }
                ui.menu_button("Export", |ui| {
                    menu_command(ui, st, "export_step", "STEP AP242…");
                    menu_command(ui, st, "export_stl", "STL…");
                    menu_command(ui, st, "export_3mf", "3MF…");
                    menu_command(ui, st, "export_obj", "OBJ…");
                });
                #[cfg(target_arch = "wasm32")]
                if crate::journal_browser::has_offers() && ui.button("Crash report journals…").clicked() {
                    st.push(AppAction::ShowRecovery);
                    ui.close();
                }
                if st.recovery.as_ref().is_some_and(|r| !r.offers.is_empty() || !r.journals.is_empty())
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
                let labels = match st.assembly.session.as_ref().filter(|_| st.assembly.is_active()) {
                    Some(a) => (a.doc.undo_label(), a.doc.redo_label()),
                    None => (st.doc.undo_label(), st.doc.redo_label()),
                };
                let undo = labels.0.map(|l| format!("Undo {l}")).unwrap_or("Undo".into());
                let redo = labels.1.map(|l| format!("Redo {l}")).unwrap_or("Redo".into());
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
                for m in theme::Mode::ALL {
                    if ui.radio(theme::mode() == m, format!("{} theme", m.label())).clicked() {
                        st.push(AppAction::SetTheme(m));
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
                if !st.drawing.is_active() && !st.assembly.is_active() {
                    ui.menu_button("Assistant", |ui| {
                        menu_command(ui, st, "ask", "Ask");
                        menu_command(ui, st, "check_model", "Check model");
                        menu_command(ui, st, "explain", "Explain");
                        menu_command(ui, st, "summarise", "Summarise");
                        menu_command(ui, st, "find_issues", "Find issues");
                        ui.separator();
                        menu_command(ui, st, "provider_settings", "Provider…");
                        menu_command(ui, st, "auto_apply", "Auto-apply");
                    });
                }
                ui.separator();
                menu_command(ui, st, "file_locations", "File locations…");
                if ui.button("Preferences…").clicked() {
                    *preferences = Some(crate::preferences::Preferences::new(&st.navigation, st.viewport.references));
                    ui.close();
                }
            });
            ui.menu_button("Window", |ui| {
                menu_command(ui, st, "reset_layout", "Reset panel layout");
                menu_command(ui, st, "show_drawing", "Open drawings");
                menu_command(ui, st, "ask", "Assistant");
                menu_command(ui, st, "command_palette", "Command palette");
            });
            ui.menu_button("Help", |ui| {
                menu_command(ui, st, "shortcuts", "Keyboard shortcuts");
                menu_command(ui, st, "about", "About SolidDraft");
            });

            ui.add_space(8.0);
            if !st.drawing.is_active() && !st.assembly.is_active() {
                quick_access(ui, st);
            }
            ui.add_space(6.0);
            // Command search.
            let search_w = 270.0_f32.min(ui.available_width() - 180.0).max(120.0);
            let frame = egui::Frame::new()
                .fill(theme::panel())
                .stroke(Stroke::new(1.0, theme::line()))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(6, 1));
            let inner = frame.show(ui, |ui| {
                ui.set_width(search_w);
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(13.0, 13.0), Sense::hover());
                    icons::paint(ui.painter(), Icon::Search, r, theme::muted());
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
                ui.label(egui::RichText::new(text).size(12.0).color(theme::muted())).on_hover_text(
                    st.doc.path.as_ref().map(|p| p.display().to_string()).unwrap_or("Not saved yet".into()),
                );
            });
        });
    }

    /// What the bottom bar says about the active document.
    pub(super) fn status(&self) -> Status {
        let st = &self.state;
        let other = if st.assembly.is_active() {
            crate::assembly::status(st)
        } else if st.drawing.is_active() {
            crate::drawing::status(st)
        } else {
            None
        };
        other.unwrap_or_else(|| part_status(st))
    }

    /// The side area: a resizable panel while shown, a strip with its title while
    /// hidden. Dragging the edge past the minimum hides it; clicking the strip,
    /// dragging it outward or double-clicking the edge shows it again.
    pub(super) fn side_panel(&mut self, root: &mut egui::Ui, side: Side, window_width: f32) {
        let frame = egui::Frame::new().fill(theme::panel()).inner_margin(egui::Margin::ZERO);
        let make = |id: egui::Id| match side {
            Side::Tree => egui::Panel::left(id),
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

    pub(super) fn dialogs(&mut self, ctx: &egui::Context) {
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
                        .color(theme::muted()),
                    );
                },
            );
        }
        self.assistant_settings(ctx);
        crate::drawing::reference::window(ctx, &mut self.state);
        crate::locations::window(ctx, &mut self.state);
        crate::import::window(ctx, &mut self.state);
        crate::recovery::window(ctx, &mut self.state);
        #[cfg(target_arch = "wasm32")]
        crate::journal_browser::window(ctx, &mut self.state);
    }

    /// The assistant settings dialog: the provider, an API key per provider and the models.
    pub(super) fn assistant_settings(&mut self, ctx: &egui::Context) {
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
                            ui.label(egui::RichText::new(text).color(theme::muted()));
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
                                    .color(theme::muted()),
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
}

/// The part's status: how the last rebuild went, what is being edited, how big the model
/// is and where the pointer is.
fn part_status(st: &AppState) -> Status {
    let errors = st.doc.error_count();
    let repairs = st.doc.repair_count();
    let headline = if let Some((message, kind)) = st.edit_status() {
        (message.to_owned(), if matches!(kind, NoteKind::Error) { theme::error() } else { theme::muted() })
    } else if errors > 0 {
        (format!("{errors} rebuild error{}", if errors == 1 { "" } else { "s" }), theme::error())
    } else if repairs > 0 {
        (format!("{repairs} feature{} to repair", if repairs == 1 { "" } else { "s" }), theme::warn())
    } else if st.doc.display_failure().is_some() {
        ("Display failed".to_owned(), theme::error())
    } else {
        ("Rebuild OK".to_owned(), theme::ok())
    };
    let mode = match (&st.sketch, &st.editor) {
        (Some(s), _) => {
            let name = st.doc.feature(s.feature).map(|f| f.name.clone()).unwrap_or("sketch".into());
            format!("Editing {name} · {}", s.sketch.status.label())
        }
        (None, Some(e)) => match e.params {
            EditorParams::Extrude(_) => "Editing Extrude".to_owned(),
            EditorParams::Cut(_) => "Editing Cut".to_owned(),
            _ => format!("Editing {}", e.title().trim_start_matches("Edit ")),
        },
        _ => "Editing part".to_owned(),
    };
    let n = st.doc.features.len();
    let position = match (st.sketch.as_ref().and_then(|s| s.cursor), st.hover_point) {
        (Some(c), _) => format!("u {:.1}   v {:.1} mm", c.u, c.v),
        (None, Some(p)) => format!("x {:.1}   y {:.1}   z {:.1} mm", p.x, p.y, p.z),
        _ => "mm".to_owned(),
    };
    Status {
        headline,
        note: st
            .doc
            .display_failure()
            .map(|text| StatusNote { text: text.to_owned(), color: theme::error(), fresh: true })
            .or_else(|| st.note.as_ref().map(StatusNote::of)),
        details: vec![
            mode,
            format!("{n} feature{}", if n == 1 { "" } else { "s" }),
            format!("{} triangles", st.doc.triangle_count()),
            format!("{:.1} ms", st.frame_ms),
        ],
        position,
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

/// Undo and Redo beside the menus. The part's ribbon has no Edit group; the drawing's and
/// the assembly's do.
fn quick_access(ui: &mut egui::Ui, st: &mut AppState) {
    for (id, icon) in [("undo", Icon::Undo), ("redo", Icon::Redo)] {
        let Some(command) = commands::get(id) else {
            debug_assert!(false, "no command {id}");
            continue;
        };
        let enabled = command.is_enabled(st);
        let sense = if enabled { Sense::click() } else { Sense::hover() };
        let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 22.0), sense);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, command.label));
        if enabled && resp.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(theme::RADIUS), theme::hover());
        }
        let color = if enabled { theme::ink() } else { theme::muted().gamma_multiply(0.55) };
        icons::paint(ui.painter(), icon, Rect::from_center_size(rect.center(), vec2(15.0, 15.0)), color);
        if resp.on_hover_text(command.tip(st)).clicked() {
            st.run_command(command);
        }
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
            ui.label(egui::RichText::new(scope.label()).family(theme::semibold()).color(theme::muted()));
            ui.end_row();
            for (keys, doc) in rows {
                ui.label(egui::RichText::new(keys).family(theme::semibold()));
                ui.label(doc);
                ui.end_row();
            }
        }
        ui.label(egui::RichText::new("Mouse").family(theme::semibold()).color(theme::muted()));
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
            ui.label(egui::RichText::new("No matching command").color(theme::muted()));
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
