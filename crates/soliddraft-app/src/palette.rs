//! The command palette: a text field over the command registry, opened with a shortcut.
//! Typing filters the commands the current workspace offers, Up and Down move the
//! choice, and Enter or a click runs it as a ribbon button does.
//!
//! The ranking is [`commands::search_here`], so the palette, the menu bar's search and
//! `soliddraft commands` order a query the same way. With nothing typed it lists the
//! pinned and recent commands.

use crate::commands::{self, CommandSpec, Context, History, Match};
use crate::state::AppState;
use crate::theme;
use egui::{Align2, Key, Modifiers, vec2};

/// How many results the palette lists.
pub const SHOWN: usize = 10;

/// The commands a query finds in a context, best first, as many as the palette lists.
pub fn find(query: &str, history: &History, context: Context) -> Vec<Match> {
    let mut found = commands::search_here(query, history, context);
    found.truncate(SHOWN);
    found
}

/// An open palette.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Palette {
    /// What has been typed.
    pub query: String,
    /// The chosen result, counted from the top.
    pub selected: usize,
}

impl Palette {
    /// The commands the query finds in the context the app is in, best first.
    pub fn results(&self, state: &AppState) -> Vec<Match> {
        find(&self.query, &state.commands, Context::of(state))
    }

    /// Moves the choice up or down, stopping at the ends.
    pub fn step(&mut self, down: bool, len: usize) {
        self.selected =
            if down { (self.selected + 1).min(len.saturating_sub(1)) } else { self.selected.saturating_sub(1) };
    }

    /// The chosen command.
    pub fn chosen(&self, state: &AppState) -> Option<&'static CommandSpec> {
        self.results(state).get(self.selected).map(|m| m.command)
    }
}

/// Opens the palette, or closes it when it is open.
pub fn toggle(state: &mut AppState) {
    state.palette = match state.palette {
        Some(_) => None,
        None => Some(Palette::default()),
    };
}

/// Draws the palette when it is open, and runs the command chosen with Enter or a click.
pub fn show(ctx: &egui::Context, state: &mut AppState) {
    let Some(mut palette) = state.palette.take() else { return };
    let (escape, down, up, enter) = ctx.input_mut(|i| {
        (
            i.consume_key(Modifiers::NONE, Key::Escape),
            i.consume_key(Modifiers::NONE, Key::ArrowDown),
            i.consume_key(Modifiers::NONE, Key::ArrowUp),
            i.consume_key(Modifiers::NONE, Key::Enter),
        )
    });
    if escape {
        return;
    }
    let query = palette.query.clone();
    let len = palette.results(state).len();
    if down || up {
        palette.step(down, len);
    }
    let mut run = enter.then(|| palette.chosen(state)).flatten();
    egui::Area::new(egui::Id::new("command_palette"))
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_TOP, vec2(0.0, 80.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(460.0);
                let field = ui.add(
                    egui::TextEdit::singleline(&mut palette.query)
                        .hint_text("Type a command")
                        .desired_width(f32::INFINITY),
                );
                field.request_focus();
                if palette.query != query {
                    palette.selected = 0;
                }
                let results = palette.results(state);
                if results.is_empty() && !palette.query.trim().is_empty() {
                    ui.label(egui::RichText::new("No matching command").color(theme::muted()));
                }
                for (i, m) in results.iter().enumerate() {
                    let c = m.command;
                    let button = egui::Button::selectable(i == palette.selected, c.label_in(state))
                        .shortcut_text(c.shortcut_text().unwrap_or_default());
                    let r = ui.add_enabled(c.is_enabled(state), button);
                    let r = match c.path() {
                        Some(path) => r.on_hover_text(path),
                        None => r,
                    };
                    if r.clicked() {
                        run = Some(c);
                    }
                }
            });
        });
    match run {
        Some(command) => state.run_command(command),
        None => state.palette = Some(palette),
    }
}
