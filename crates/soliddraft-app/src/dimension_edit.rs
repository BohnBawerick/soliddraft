//! Editing a feature's dimension where the viewport shows it. A double-click on a solid
//! or on its feature's tree row shows the dimensions that build it; a double-click on a
//! value of a blind extrude types over that one value, and Enter rebuilds the same
//! feature as one undo step. Escape, or a click elsewhere, leaves the part as it was.
//!
//! The typed text stays here until it is a positive finite length, so a refused value
//! never reaches the document. Other features keep their side editor and sketch
//! dimensions keep the sketch's own value editor.

use crate::document::{EndKind, FeatureId, FeatureKind, FeatureStatus, ProfileKind, Selection, fmt_mm};
use crate::state::{AppAction, AppState, NoteKind};

/// The lengths the side panel's fields take, in millimetres.
const SMALLEST: f64 = 0.01;
const LARGEST: f64 = 100_000.0;

/// One value of a feature. The feature's id alone cannot say which: a built-in profile's
/// sizes and the depth all belong to the same extrude.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FeatureField {
    Width,
    Height,
    Diameter,
    Depth,
}

impl FeatureField {
    pub fn label(self) -> &'static str {
        match self {
            FeatureField::Width => "Width",
            FeatureField::Height => "Height",
            FeatureField::Diameter => "Diameter",
            FeatureField::Depth => "Depth",
        }
    }

    /// The field's value in `kind`, when `kind` drives it.
    pub fn get(self, kind: &FeatureKind) -> Option<f64> {
        let FeatureKind::Extrude(p) = kind else { return None };
        match (self, p.profile, p.sketch) {
            (FeatureField::Depth, _, _) => (p.end == EndKind::Blind).then_some(p.depth),
            (FeatureField::Width, ProfileKind::Rectangle { width, .. }, None) => Some(width),
            (FeatureField::Height, ProfileKind::Rectangle { height, .. }, None) => Some(height),
            (FeatureField::Diameter, ProfileKind::Circle { diameter }, None) => Some(diameter),
            _ => None,
        }
    }

    /// `kind` with the field set to `value`.
    pub fn with(self, kind: &FeatureKind, value: f64) -> FeatureKind {
        let mut kind = kind.clone();
        if let FeatureKind::Extrude(p) = &mut kind {
            match (self, &mut p.profile) {
                (FeatureField::Depth, _) => p.depth = value,
                (FeatureField::Width, ProfileKind::Rectangle { width, .. }) => *width = value,
                (FeatureField::Height, ProfileKind::Rectangle { height, .. }) => *height = value,
                (FeatureField::Diameter, ProfileKind::Circle { diameter }) => *diameter = value,
                _ => {}
            }
        }
        kind
    }
}

/// A value being typed at its dimension in the viewport.
#[derive(Clone, Debug, PartialEq)]
pub struct DimensionEdit {
    pub feature: FeatureId,
    pub field: FeatureField,
    pub text: String,
    /// The text the field opened with: Enter on it changes nothing.
    pub opened_with: String,
    /// Why the last typed value was refused.
    pub error: Option<String>,
    /// The field takes the keyboard and selects its text on the next frame.
    pub focus: bool,
    /// The document revision the value was read from.
    pub revision: u64,
}

impl DimensionEdit {
    /// The text as a length to apply, why it cannot be one, or `None` when Enter has
    /// nothing to do.
    pub fn typed(&self) -> Result<Option<f64>, String> {
        if self.text.trim() == self.opened_with {
            return Ok(None);
        }
        parse_length(&self.text).map(Some)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DimensionAction {
    /// Shows the dimensions that build the body.
    Show,
    /// Opens a value for typing at its dimension.
    Open { feature: FeatureId, field: FeatureField },
    /// Applies a typed value, as one undo step.
    Commit(f64),
    /// Closes the value with nothing applied.
    Cancel,
}

/// What a double-click on a feature's tree row does.
pub fn row_double_click(state: &AppState, id: FeatureId) -> AppAction {
    let Some(f) = state.doc.feature(id) else { return AppAction::EditFeature(id) };
    if f.is_sketch() {
        AppAction::EditSketch(id)
    } else if edits_in_place(state, id) {
        AppAction::Dimension(DimensionAction::Show)
    } else {
        AppAction::EditFeature(id)
    }
}

/// Whether the viewport can show the feature's dimensions and take a value for it: the
/// feature is built, and no sketch or feature editor holds the viewport.
fn edits_in_place(state: &AppState, id: FeatureId) -> bool {
    let end = state.doc.rollback.unwrap_or(state.doc.features.len());
    let built = state.doc.index_of(id).is_some_and(|i| i < end);
    built
        && state.editor.is_none()
        && state.sketch.is_none()
        && state.doc.display_body().is_some()
        && state.doc.feature(id).is_some_and(|f| {
            !f.suppressed && f.status != FeatureStatus::Inactive && FeatureField::Depth.get(&f.kind).is_some()
        })
}

/// A typed length: a positive finite number of millimetres, with or without its unit.
fn parse_length(text: &str) -> Result<f64, String> {
    let number = text.trim().trim_end_matches("mm").trim();
    let value = number.parse::<f64>().ok().filter(|v| v.is_finite()).ok_or("Enter a number in mm")?;
    if value <= 0.0 {
        return Err("Enter a length above 0 mm".into());
    }
    if !(SMALLEST..=LARGEST).contains(&value) {
        return Err(format!("Enter a length from {SMALLEST} to {LARGEST} mm"));
    }
    Ok(value)
}

impl AppState {
    pub fn dimension(&mut self, action: DimensionAction) {
        match action {
            DimensionAction::Show => {
                self.rename = None;
                self.doc.selection = Some(Selection::Body);
            }
            DimensionAction::Open { feature, field } => {
                let Some(value) = self.doc.feature(feature).and_then(|f| field.get(&f.kind)) else { return };
                let text = fmt_mm(value);
                self.dimension_edit = Some(DimensionEdit {
                    feature,
                    field,
                    opened_with: text.clone(),
                    text,
                    error: None,
                    focus: true,
                    revision: self.doc.revision(),
                });
            }
            DimensionAction::Commit(value) => self.commit_dimension(value),
            DimensionAction::Cancel => self.dimension_edit = None,
        }
    }

    /// Applies the value through the same command the side editor uses. A refusal stays
    /// at the field, which keeps its text. The body stays selected, so the rebuilt
    /// dimensions stay on screen.
    fn commit_dimension(&mut self, value: f64) {
        let Some(edit) = &self.dimension_edit else { return };
        let (id, field) = (edit.feature, edit.field);
        let command = self
            .doc
            .feature(id)
            .filter(|f| field.get(&f.kind).is_some())
            .and_then(|f| crate::drag::edit_command(id, field.with(&f.kind, value)));
        let Some(command) = command else {
            self.dimension_edit = None;
            return;
        };
        if let Some(drag) = self.drag_preview.take() {
            drag.cancel(&mut self.doc);
        }
        match self.doc.apply_interactive(command) {
            Ok(_) => {
                self.dimension_edit = None;
                self.doc.selection = Some(Selection::Body);
                let (kind, ending) =
                    if self.doc.proof_pending() { (NoteKind::Info, " requested") } else { (NoteKind::Ok, "") };
                self.note(format!("{} {} mm{ending}", field.label(), fmt_mm(value)), kind);
            }
            Err(message) => {
                if let Some(edit) = self.dimension_edit.as_mut() {
                    edit.error = Some(message);
                    edit.focus = true;
                    edit.revision = self.doc.revision();
                }
            }
        }
    }
}
