//! Assistant panel: message list with proposal cards and a composer. Text and
//! tool activity stream in from the conversation in `chat`; proposals come from
//! the session's proposal book and apply through the same command layer as the
//! ribbon.

use crate::chat::{ToolActivity, ToolState};
use crate::command_tools::{Proposal, ProposalStatus};
use crate::icons::{self, Icon};
use crate::state::{AppAction, AppState, ChatMessage, Role};
use crate::theme;
use egui::{Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

pub fn show(ui: &mut egui::Ui, state: &mut AppState) {
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    let tag = state.chat.status.as_str();
    let galley = ui.painter().layout_no_wrap(tag.to_owned(), theme::font(10.5), theme::MUTED);
    let pill = Rect::from_min_size(
        pos2(head.left() + 10.0, head.center().y - 9.0),
        galley.size() + vec2(16.0, 18.0 - galley.size().y),
    );
    ui.painter().rect(pill, CornerRadius::same(99), theme::PANEL, Stroke::new(1.0, theme::LINE), StrokeKind::Inside);
    ui.painter().galley(pill.center() - galley.size() / 2.0, galley, theme::MUTED);

    let composer_h = 104.0;
    let list_h = (ui.available_height() - composer_h).max(60.0);
    let mut actions = Vec::new();
    egui::ScrollArea::vertical()
        .id_salt("chat")
        .auto_shrink([false, false])
        .max_height(list_h)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            ui.set_min_height(list_h);
            ui.add_space(10.0);
            let width = ui.available_width();
            let messages = state.chat.messages.clone();
            let previewing = state.chat.preview.as_ref().map(|(id, _)| id.clone());
            for msg in &messages {
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
                message(ui, msg, &cards, width, previewing.as_deref(), &mut actions);
                ui.add_space(8.0);
            }
        });

    // Composer.
    egui::Frame::new().inner_margin(egui::Margin { left: 10, right: 10, top: 6, bottom: 10 }).show(ui, |ui| {
        ui.painter().hline(ui.max_rect().x_range(), ui.max_rect().top() - 5.0, Stroke::new(1.0, theme::LINE));
        ui.horizontal(|ui| {
            for chip in ["@selection", "@model", "@sketch", "@errors"] {
                let galley = ui.painter().layout_no_wrap(chip.to_owned(), theme::font(10.5), theme::MUTED);
                let (r, resp) = ui.allocate_exact_size(galley.size() + vec2(14.0, 6.0), Sense::click());
                let bg = if resp.hovered() { theme::HOVER } else { theme::PANEL };
                ui.painter().rect(r, CornerRadius::same(99), bg, Stroke::new(1.0, theme::LINE), StrokeKind::Inside);
                ui.painter().galley(r.center() - galley.size() / 2.0, galley, theme::MUTED);
                if resp.clicked() {
                    if !state.chat.draft.is_empty() && !state.chat.draft.ends_with(' ') {
                        state.chat.draft.push(' ');
                    }
                    state.chat.draft.push_str(chip);
                    state.chat.draft.push(' ');
                    state.chat.focus_requested = true;
                }
            }
        });
        ui.add_space(4.0);
        let box_rect = egui::Frame::new()
            .fill(theme::PANEL)
            .stroke(Stroke::new(1.0, theme::LINE))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(egui::Margin { left: 8, right: 6, top: 6, bottom: 6 })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let edit = egui::TextEdit::multiline(&mut state.chat.draft)
                        .hint_text("Ask or describe a feature…")
                        .desired_rows(2)
                        .desired_width(ui.available_width() - 40.0)
                        .frame(egui::Frame::NONE)
                        .return_key(Some(egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, egui::Key::Enter)));
                    let resp = ui.add(edit);
                    if state.chat.focus_requested {
                        resp.request_focus();
                        state.chat.focus_requested = false;
                    }
                    let submit =
                        resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::BOTTOM), |ui| {
                        let (r, send) = ui.allocate_exact_size(vec2(30.0, 26.0), Sense::click());
                        let busy = state.chat.busy();
                        send.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                true,
                                if busy { "Stop" } else { "Send" },
                            )
                        });
                        let enabled = busy || !state.chat.draft.trim().is_empty();
                        let bg = if !enabled {
                            theme::LINE
                        } else if send.hovered() {
                            theme::ACCENT.gamma_multiply(0.9)
                        } else {
                            theme::ACCENT
                        };
                        ui.painter().rect_filled(r, CornerRadius::same(7), bg);
                        if busy {
                            ui.painter().rect_filled(
                                Rect::from_center_size(r.center(), vec2(9.0, 9.0)),
                                CornerRadius::same(2),
                                Color32::WHITE,
                            );
                            if send.clicked() {
                                actions.push(AppAction::StopChat);
                            }
                        } else {
                            icons::paint(ui.painter(), Icon::Send, r.shrink(6.0), Color32::WHITE);
                            if (send.clicked() || submit) && enabled {
                                actions.push(AppAction::SendChat);
                            }
                        }
                    });
                });
            })
            .response
            .rect;
        let _ = box_rect;
    });
    for a in actions {
        state.push(a);
    }
}

fn message(
    ui: &mut egui::Ui,
    msg: &ChatMessage,
    cards: &[(Proposal, bool)],
    width: f32,
    previewing: Option<&str>,
    actions: &mut Vec<AppAction>,
) {
    match msg.role {
        Role::System => {
            egui::Frame::new().inner_margin(egui::Margin::symmetric(14, 0)).show(ui, |ui| {
                ui.label(egui::RichText::new(&msg.text).size(11.5).color(theme::MUTED));
            });
        }
        Role::User => {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                ui.add_space(10.0);
                egui::Frame::new()
                    .fill(theme::ACCENT)
                    .corner_radius(CornerRadius { nw: 12, ne: 12, sw: 12, se: 4 })
                    .inner_margin(egui::Margin::symmetric(11, 7))
                    .show(ui, |ui| {
                        ui.set_max_width(width * 0.82);
                        // Right-to-left layouts do not wrap on their own.
                        ui.add(
                            egui::Label::new(egui::RichText::new(&msg.text).size(12.5).color(Color32::WHITE)).wrap(),
                        );
                    });
            });
        }
        Role::Assistant => {
            egui::Frame::new().inner_margin(egui::Margin { left: 10, right: 10, top: 0, bottom: 0 }).show(ui, |ui| {
                egui::Frame::new()
                    .fill(theme::PANEL)
                    .stroke(Stroke::new(1.0, theme::LINE))
                    .corner_radius(CornerRadius { nw: 12, ne: 12, sw: 4, se: 12 })
                    .inner_margin(egui::Margin::symmetric(11, 9))
                    .show(ui, |ui| {
                        ui.set_width(width - 20.0);
                        if !msg.text.is_empty() {
                            ui.label(egui::RichText::new(&msg.text).size(12.5).color(theme::INK));
                        } else if msg.streaming && msg.tools.is_empty() {
                            ui.label(egui::RichText::new("Thinking…").size(12.0).color(theme::MUTED));
                        }
                        for t in &msg.tools {
                            tool_line(ui, t);
                        }
                        for (p, can_undo) in cards {
                            ui.add_space(8.0);
                            proposal_card(ui, p, *can_undo, previewing == Some(p.id.as_str()), actions);
                        }
                    });
            });
        }
    }
}

fn tool_line(ui: &mut egui::Ui, t: &ToolActivity) {
    let (text, color) = match &t.state {
        ToolState::Running(0) => (format!("{} …", t.name), theme::MUTED),
        ToolState::Running(bytes) => (format!("{} … {bytes} bytes", t.name), theme::MUTED),
        ToolState::Done(s) => (format!("{} · {s}", t.name), theme::MUTED),
        ToolState::Failed(e) => (format!("{} failed: {e}", t.name), theme::ERROR),
    };
    ui.label(egui::RichText::new(text).size(11.0).family(theme::medium()).color(color));
}

fn proposal_card(ui: &mut egui::Ui, p: &Proposal, can_undo: bool, previewing: bool, actions: &mut Vec<AppAction>) {
    let inactive = matches!(p.status, ProposalStatus::Discarded | ProposalStatus::Undone);
    let stroke = if inactive { theme::LINE } else { theme::ACCENT.gamma_multiply(0.6) };
    egui::Frame::new()
        .fill(theme::FIELD_BG)
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                icons::paint(ui.painter(), Icon::Extrude, r, if inactive { theme::MUTED } else { theme::INK });
                ui.label(egui::RichText::new(&p.summary).family(theme::semibold()).size(12.5).color(if inactive {
                    theme::MUTED
                } else {
                    theme::INK
                }));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (text, bg, fg) = match &p.status {
                        ProposalStatus::Pending => ("proposal", theme::SELECTION_INK, Color32::WHITE),
                        ProposalStatus::Applied { .. } => ("applied", theme::OK, Color32::WHITE),
                        ProposalStatus::Undone => ("undone", theme::LINE, theme::MUTED),
                        ProposalStatus::Discarded => ("discarded", theme::LINE, theme::MUTED),
                    };
                    let galley = ui.painter().layout_no_wrap(text.to_owned(), theme::font_medium(10.5), fg);
                    let (r, _) = ui.allocate_exact_size(galley.size() + vec2(14.0, 5.0), Sense::hover());
                    ui.painter().rect_filled(r, CornerRadius::same(99), bg);
                    ui.painter().galley(r.center() - galley.size() / 2.0, galley, fg);
                });
            });
            ui.add_space(4.0);
            // Values wrap so a long recipe line never widens the panel.
            let value_width = (ui.available_width() - 110.0).max(80.0);
            egui::Grid::new(("proposal", &p.id)).num_columns(2).spacing([10.0, 3.0]).show(ui, |ui| {
                for (k, v) in &p.rows {
                    ui.label(egui::RichText::new(k).size(11.5).color(theme::MUTED));
                    ui.scope(|ui| {
                        ui.set_max_width(value_width);
                        ui.add(egui::Label::new(egui::RichText::new(v).size(11.5)).wrap());
                    });
                    ui.end_row();
                }
            });
            ui.add_space(6.0);
            match &p.status {
                ProposalStatus::Pending => {
                    ui.horizontal(|ui| {
                        if p.preview.result.is_some() {
                            let preview = ui.add(
                                egui::Button::new(if previewing { "Hide preview" } else { "Preview" })
                                    .selected(previewing),
                            );
                            if preview.clicked() {
                                actions.push(AppAction::PreviewProposal(p.id.clone()));
                            }
                        }
                        let apply = ui.add(
                            egui::Button::new(egui::RichText::new("Apply").color(Color32::WHITE))
                                .fill(theme::ACCENT)
                                .stroke(Stroke::NONE),
                        );
                        if apply.clicked() {
                            actions.push(AppAction::ApplyProposal(p.id.clone()));
                        }
                        if ui.button("Discard").clicked() {
                            actions.push(AppAction::DiscardProposal(p.id.clone()));
                        }
                    });
                }
                ProposalStatus::Applied { .. } => {
                    // Wrapped, so the undo button moves under the text in a narrow panel
                    // instead of being cut off past its edge.
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new("Applied to the tree as ordinary features.")
                                .size(11.0)
                                .color(theme::MUTED),
                        );
                        if ui.add_enabled(can_undo, egui::Button::new(format!("Undo {}", p.id))).clicked() {
                            actions.push(AppAction::UndoProposal(p.id.clone()));
                        }
                    });
                }
                ProposalStatus::Undone | ProposalStatus::Discarded => {}
            }
        });
}
