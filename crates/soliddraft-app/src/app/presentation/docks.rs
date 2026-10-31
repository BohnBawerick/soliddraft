//! Part, drawing and assembly docks.

use crate::icons::{self, Icon};
use crate::panels::Side;
use crate::state::{AppAction, AppState};
use crate::theme;
use crate::viewport::Overlay;
use crate::{properties, tree};
use egui::{CornerRadius, Rect, Sense, pos2, vec2};
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};

use crate::app::dispatch::selection_of;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Tree,
    Properties,
    Viewport,
    /// The drawing document, opened beside the part by Make drawing.
    Drawing,
    /// The assembly document, opened beside the part by New assembly or Open.
    Assembly,
}

impl Tab {
    fn title(&self) -> &'static str {
        match self {
            Tab::Tree => "Feature tree",
            Tab::Properties => "Properties",
            Tab::Viewport => "Part",
            Tab::Drawing => "Drawing",
            Tab::Assembly => "Assembly",
        }
    }
}

/// The middle: the part, and the drawing and assembly tabs beside it once open.
pub(in crate::app) fn center_dock() -> DockState<Tab> {
    DockState::new(vec![Tab::Viewport])
}

/// The left side area: the feature tree with the property panel under it.
pub(in crate::app) fn tree_dock() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Tree]);
    dock.main_surface_mut().split_below(NodeIndex::root(), 0.46, vec![Tab::Properties]);
    dock
}

/// A dock area in the app's style, without the buttons the shell does not use.
pub(in crate::app) fn show_dock(ui: &mut egui::Ui, dock: &mut DockState<Tab>, id: egui::Id, panels: &mut Panels<'_>) {
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
/// window and the title set on end, reading bottom to top like the tab of a collapsed
/// task pane. Returns true when clicked.
pub(in crate::app) fn strip(ui: &mut egui::Ui, side: Side, title: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click());
    let label = format!("Show the {}", side.name());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label.clone()));
    let resp = resp.on_hover_text(format!("{label} ({})", side.shortcut()));
    let painter = ui.painter();
    if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::ZERO, theme::hover());
    }
    let color = if resp.hovered() { theme::ink() } else { theme::muted() };
    let icon_rect = Rect::from_center_size(pos2(rect.center().x, rect.top() + 16.0), vec2(14.0, 14.0));
    icons::paint(painter, Icon::ChevronRight, icon_rect, color);
    let galley = painter.layout_no_wrap(title.to_owned(), theme::font_medium(12.0), color);
    let (w, h) = (galley.size().x, galley.size().y);
    // The text turns about its top-left corner.
    let pos = pos2(rect.center().x - h / 2.0, rect.top() + 34.0 + w);
    painter.add(egui::epaint::TextShape::new(pos, galley, color).with_angle(-std::f32::consts::FRAC_PI_2));
    resp.clicked()
}

pub(in crate::app) struct Panels<'a> {
    pub(in crate::app) state: &'a mut AppState,
}

impl TabViewer for Panels<'_> {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(("tab", tab.title()))
    }

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        let text = match tab {
            Tab::Viewport => self.state.doc.name.clone(),
            Tab::Tree if self.state.assembly.is_active() => "Assembly tree".to_owned(),
            Tab::Tree if self.state.drawing.is_active() => "Drawing tree".to_owned(),
            Tab::Assembly => match self.state.assembly.session.as_ref() {
                Some(s) => s.title(),
                None => "Assembly".to_owned(),
            },
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
            Tab::Tree if self.state.assembly.is_active() => crate::assembly::tree(ui, self.state),
            Tab::Properties if self.state.assembly.is_active() => crate::assembly::properties(ui, self.state),
            Tab::Assembly => crate::assembly::viewport(ui, self.state),
            Tab::Tree if self.state.drawing.is_active() => crate::drawing::tree_drawing::show(ui, self.state),
            Tab::Tree => tree::show(ui, self.state),
            Tab::Properties if self.state.drawing.new.is_some() && !self.state.drawing.is_active() => {
                crate::drawing::properties_drawing::new_drawing(ui, self.state)
            }
            Tab::Properties if self.state.drawing.is_active() => {
                crate::drawing::properties_drawing::show(ui, self.state)
            }
            Tab::Properties => properties::show(ui, self.state),
            Tab::Drawing => crate::drawing::canvas::show(ui, self.state),
            Tab::Viewport => {
                let st = &mut *self.state;
                let ghost = st.ghost();
                let preview = st.preview_body();
                let selection = st.doc.selection;
                let hover_label = st.hover_info.as_ref().map(|i| i.title.clone());
                // Shown sketches lie quietly on their planes, except the one open for editing.
                let visible = crate::tree_visibility::visible(st);
                let editing = st.sketch.as_ref().map(|s| s.feature);
                let passive = visible
                    .sketches
                    .iter()
                    .filter(|id| Some(**id) != editing)
                    .filter_map(|id| {
                        let f = st.doc.feature(*id)?;
                        Some((f.plane?, &f.sketch()?.sketch))
                    })
                    .collect();
                let overlay = Overlay {
                    ghost,
                    preview,
                    sketch: st.sketch.as_mut(),
                    passive_sketches: passive,
                    dimension_edit: st.dimension_edit.as_mut(),
                    visible,
                };
                let events = st.viewport.show(ui, &st.doc, overlay, selection, hover_label.as_deref(), &st.navigation);
                st.after_viewport(ui, &events);
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
        matches!(tab, Tab::Drawing | Tab::Assembly)
    }

    fn on_close(&mut self, tab: &mut Tab) -> egui_dock::tab_viewer::OnCloseResponse {
        if matches!(tab, Tab::Drawing) {
            self.state.drawing.session = None;
            self.state.drawing.active = false;
        }
        if matches!(tab, Tab::Assembly) {
            self.state.assembly.session = None;
            self.state.assembly.active = false;
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
