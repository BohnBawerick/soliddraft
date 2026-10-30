//! The command layer: every change to a document is a [`Command`], whether the ribbon,
//! the assistant or a script issues it. Each executed command is one undo entry.
//!
//! Undo is a command stack over the document, not over kernel state: applying a command
//! yields its exact inverse, and undo applies that inverse and rebuilds.

use serde::{Deserialize, Serialize};
use soliddraft_document::ConfigScope;
use soliddraft_materials::{Appearance, AppearanceId, Catalog, Material, MaterialId};
use soliddraft_sketch::{ConstraintId, SolveStatus};

use crate::document::Document;
use crate::feature::{DatumPlane, Feature, FeatureId, FeatureKind, PatternSpec};
use crate::materials::{AppearanceAssignment, AppearanceTarget, MaterialAssignment, PartMaterials};

/// A change to a document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    /// Adds a new feature. Without a position it goes at the rollback bar, or at the end.
    /// Without a name it gets the next free one for its kind, such as "Fillet2".
    AddFeature {
        /// What the feature does.
        kind: FeatureKind,
        /// The tree name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// Tree position.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<usize>,
        /// Marks the feature as the assistant's.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        via_assistant: bool,
    },
    /// Puts back a feature with a known id, as redo of an add and undo of a delete do.
    InsertFeature {
        /// The feature, id included.
        feature: Feature,
        /// Tree position.
        position: usize,
    },
    /// Replaces what a feature does. A sketch is stored as given: solve it first.
    EditFeature {
        /// The feature.
        id: FeatureId,
        /// The new definition.
        kind: FeatureKind,
    },
    /// Sets a dimension of a sketch and solves the sketch.
    SetSketchDimension {
        /// The sketch feature.
        id: FeatureId,
        /// The dimension constraint.
        constraint: ConstraintId,
        /// The new value.
        value: f64,
    },
    /// Creates or replaces a named expression and propagates its drivers atomically.
    SetValue {
        /// The value's identifier.
        name: String,
        /// Authored expression text.
        expression: String,
    },
    /// Binds a feature input to an expression.
    Drive {
        /// The input to drive.
        input: crate::values::Input,
        /// Authored driver text.
        expression: String,
    },
    /// Renames a value and the text of its references.
    RenameValue {
        /// Existing identifier.
        name: String,
        /// New identifier.
        to: String,
    },
    /// Restores value truth and resolved feature definitions for undo.
    RestoreValues {
        /// Authored values and bindings.
        state: crate::values::Values,
        /// Resolved definitions to restore.
        features: Vec<Feature>,
    },
    /// Renames a feature.
    RenameFeature {
        /// The feature.
        id: FeatureId,
        /// The new name, not empty.
        name: String,
    },
    /// Renames a reference plane without changing the datum sketches reference.
    RenameDatumPlane {
        /// The plane's stable identity.
        datum: DatumPlane,
        /// The new display name, not empty or used by another datum.
        name: String,
    },
    /// Removes a feature. Features that used it report what broke.
    DeleteFeature {
        /// The feature.
        id: FeatureId,
    },
    /// Moves a feature to a new tree position. Refused when a feature would come before
    /// one it reads.
    MoveFeature {
        /// The feature.
        id: FeatureId,
        /// Its position after the move.
        position: usize,
    },
    /// Suppresses or unsuppresses a feature.
    SetSuppressed {
        /// The feature.
        id: FeatureId,
        /// Whether it is suppressed.
        suppressed: bool,
    },
    /// Moves the rollback bar above a feature, or to the end with `None`.
    RollTo {
        /// The first feature below the bar.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<FeatureId>,
    },
    /// Sets the part's material in some configurations, or removes it with `None`. The
    /// part keeps a copy of the material as given, so later library edits do not
    /// change it.
    SetMaterial {
        /// The material, by value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        material: Option<Material>,
        /// The material's default appearance, by value, when the caller has it: a part
        /// with no appearance of its own shows it. Without it the part shows SolidDraft's
        /// default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default_appearance: Option<Appearance>,
        /// In which configurations; all of them by default.
        #[serde(default, skip_serializing_if = "is_all")]
        scope: ConfigScope,
    },
    /// Applies an appearance to the part, a feature's faces or a face, or removes the
    /// one there with `None`. The part keeps a copy of the appearance as given.
    SetAppearance {
        /// Where.
        target: AppearanceTarget,
        /// The appearance, by value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        appearance: Option<Appearance>,
        /// In which configurations; all of them by default.
        #[serde(default, skip_serializing_if = "is_all")]
        scope: ConfigScope,
    },
    /// Replaces the part's copies of materials and appearances it uses with these
    /// values, as updating from a library does. Refused for one the part does not use.
    UpdateCopies {
        /// New values for materials the part uses.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        materials: Vec<Material>,
        /// New values for appearances the part uses.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        appearances: Vec<Appearance>,
    },
    /// Puts back the part's whole materials state, as undo of the commands above does.
    RestoreMaterials {
        /// The state.
        state: Box<PartMaterials>,
    },
    /// Several commands as one undo entry, such as an assistant proposal.
    Batch {
        /// What the batch does, for the undo menu.
        label: String,
        /// The commands, in order.
        commands: Vec<Command>,
    },
}

impl Command {
    /// Sets the part's material, for all configurations, to a material from the
    /// catalog, with its default appearance when a library has it. `None` when no
    /// library has the id.
    pub fn set_material_from(catalog: &Catalog, id: &MaterialId) -> Option<Command> {
        let (_, m) = catalog.material(id)?;
        Some(Command::SetMaterial {
            material: Some(m.clone()),
            default_appearance: catalog.default_appearance_of(m).cloned(),
            scope: ConfigScope::All,
        })
    }

    /// Applies an appearance from the catalog to `target`, for all configurations.
    /// `None` when no library has the id.
    pub fn set_appearance_from(catalog: &Catalog, target: AppearanceTarget, id: &AppearanceId) -> Option<Command> {
        let (_, a) = catalog.appearance(id)?;
        Some(Command::SetAppearance { target, appearance: Some(a.clone()), scope: ConfigScope::All })
    }
}

fn is_all(scope: &ConfigScope) -> bool {
    *scope == ConfigScope::All
}

/// Why a command was refused. A refused command changes nothing.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum CommandError {
    /// No feature has this id.
    #[error("no feature {0}")]
    UnknownFeature(FeatureId),
    /// The feature exists but is the wrong kind for the command.
    #[error("{0} is not a {1}")]
    WrongKind(FeatureId, &'static str),
    /// A feature would come before a feature it reads.
    #[error("{0} must come after {1}, which it reads")]
    Order(FeatureId, FeatureId),
    /// A position is past the end of the tree.
    #[error("position {0} is past the end of the tree")]
    Position(usize),
    /// An argument is out of range.
    #[error("{0}")]
    Invalid(String),
    /// The sketch rejected an edit.
    #[error("sketch: {0}")]
    Sketch(String),
    /// A CAD feature would come after the lattice, which is always last.
    #[error("{0} is the lattice stage and must stay last; return before the lattice to change the part")]
    AfterLattice(FeatureId),
    /// A lattice would not be the single last feature.
    #[error("the lattice must be the only lattice and the last feature")]
    LatticeNotLast,
    /// Nothing to undo or redo.
    #[error("nothing to {0}")]
    Empty(&'static str),
}

/// What an executed command did.
#[derive(Clone, Debug, PartialEq)]
pub struct Executed {
    /// The feature it created, if any.
    pub created: Option<FeatureId>,
    /// The undo entry's label.
    pub label: String,
}

/// One entry of the undo or redo stack.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct UndoEntry {
    pub label: String,
    pub undo: Command,
    pub redo: Command,
}

/// What applying a command did.
struct Applied {
    inverse: Command,
    redo: Command,
    created: Option<FeatureId>,
    label: String,
}

impl Document {
    pub(crate) fn rename_datum(&mut self, datum: DatumPlane, name: &str) -> Result<String, CommandError> {
        if name.trim().is_empty() {
            return Err(CommandError::Invalid("a plane name cannot be empty".into()));
        }
        let key = name.trim().to_ascii_lowercase();
        let key = key.trim_end_matches(" plane");
        for other in [DatumPlane::Top, DatumPlane::Front, DatumPlane::Right] {
            if other != datum
                && [self.datum_name(other), other.label()]
                    .iter()
                    .any(|label| label.to_ascii_lowercase().trim_end_matches(" plane") == key)
            {
                return Err(CommandError::Invalid(format!("another plane is already named {name}")));
            }
        }
        let old = self.datum_name(datum).to_owned();
        if name == datum.label() {
            self.datum_names.remove(&datum);
        } else {
            self.datum_names.insert(datum, name.to_owned());
        }
        Ok(old)
    }

    /// Runs a command, records it for undo and rebuilds from the first feature it
    /// changed. A rebuild problem does not refuse the command: it shows up in the
    /// features' statuses.
    pub fn execute(&mut self, command: Command) -> Result<Executed, CommandError> {
        let journal = self.journal_command(|| serde_json::json!({"op":"execute", "command":command}));
        journal.finish((|| {
            let executed = self.execute_deferred(command)?;
            self.rebuild();
            Ok(executed)
        })())
    }

    /// Records a command and invalidates its suffix without building it.
    /// Call [`Self::rebuild`] or adopt a completed fork before exporting.
    pub fn execute_deferred(&mut self, command: Command) -> Result<Executed, CommandError> {
        let journal = self.journal_command(|| serde_json::json!({"op":"execute_deferred", "command":command}));
        journal.finish((|| {
            let applied = self.apply(command)?;
            self.log.push(applied.label.clone());
            self.undo.push(UndoEntry { label: applied.label.clone(), undo: applied.inverse, redo: applied.redo });
            self.redo.clear();
            Ok(Executed { created: applied.created, label: applied.label })
        })())
    }

    /// The terminal lattice feature, if the part has one.
    pub fn lattice(&self) -> Option<FeatureId> {
        self.features.iter().find(|f| f.kind.is_lattice()).map(|f| f.id)
    }

    /// The enabled lattice above the rollback bar, including a failed build.
    pub fn active_lattice(&self) -> Option<&Feature> {
        self.features.iter().take(self.active_count()).find(|f| f.kind.is_lattice() && !f.suppressed)
    }

    /// Puts the rollback bar above the lattice so the user is back at the last CAD
    /// feature. The lattice and its settings stay in the tree and are not built until
    /// the bar rolls forward.
    pub fn return_before_lattice(&mut self) -> Result<Executed, CommandError> {
        let id = self.lattice().ok_or_else(|| CommandError::Invalid("the part has no lattice".into()))?;
        self.execute(Command::RollTo { before: Some(id) })
    }

    /// Reverts the last command. Returns its label.
    pub fn undo(&mut self) -> Result<String, CommandError> {
        let journal = self.journal_command(|| serde_json::json!({"op":"undo"}));
        journal.finish((|| {
            let label = self.undo_deferred()?;
            self.rebuild();
            Ok(label)
        })())
    }

    /// Reverts the last command and invalidates its suffix without building it.
    pub fn undo_deferred(&mut self) -> Result<String, CommandError> {
        let journal = self.journal_command(|| serde_json::json!({"op":"undo_deferred"}));
        journal.finish((|| {
            let entry = self.undo.pop().ok_or(CommandError::Empty("undo"))?;
            if let Err(e) = self.apply(entry.undo.clone()) {
                self.undo.push(entry);
                return Err(e);
            }
            self.log.push(format!("undo {}", entry.label));
            let label = entry.label.clone();
            self.redo.push(entry);
            Ok(label)
        })())
    }

    /// Repeats the last undone command. Returns its label.
    pub fn redo(&mut self) -> Result<String, CommandError> {
        let journal = self.journal_command(|| serde_json::json!({"op":"redo"}));
        journal.finish((|| {
            let label = self.redo_deferred()?;
            self.rebuild();
            Ok(label)
        })())
    }

    /// Repeats the last undone command and invalidates its suffix without building it.
    pub fn redo_deferred(&mut self) -> Result<String, CommandError> {
        let journal = self.journal_command(|| serde_json::json!({"op":"redo_deferred"}));
        journal.finish((|| {
            let entry = self.redo.pop().ok_or(CommandError::Empty("redo"))?;
            if let Err(e) = self.apply(entry.redo.clone()) {
                self.redo.push(entry);
                return Err(e);
            }
            self.log.push(format!("redo {}", entry.label));
            let label = entry.label.clone();
            self.undo.push(entry);
            Ok(label)
        })())
    }

    /// How many entries the undo stack holds.
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// Joins the undo entries recorded since the stack was `depth` deep into one entry
    /// called `label`, so commands run one at a time undo and redo as one. An assistant
    /// proposal needs this: its later commands read features its earlier ones made, so
    /// it cannot be a [`Command::Batch`] written in advance.
    pub fn group_undo(&mut self, depth: usize, label: impl Into<String>) {
        let label = label.into();
        let journal = self.journal_command(|| serde_json::json!({"op":"group_undo", "depth":depth, "label":label}));
        let _ = journal.finish((|| {
            if self.undo.len() <= depth {
                return Ok::<_, String>(());
            }
            let entries = self.undo.split_off(depth);
            let undo = Command::Batch {
                label: label.clone(),
                commands: entries.iter().rev().map(|e| e.undo.clone()).collect(),
            };
            let redo = Command::Batch { label: label.clone(), commands: entries.into_iter().map(|e| e.redo).collect() };
            self.undo.push(UndoEntry { label, undo, redo });
            Ok(())
        })());
    }

    /// Reverts the commands recorded since the undo stack was `depth` deep and forgets
    /// them, leaving nothing to redo: for a group of commands that failed partway.
    pub fn abandon(&mut self, depth: usize) -> Result<(), CommandError> {
        let journal = self.journal_command(|| serde_json::json!({"op":"abandon", "depth":depth}));
        journal.finish((|| {
            while self.undo.len() > depth {
                let entry = self.undo.pop().expect("the stack is deeper than depth");
                self.apply(entry.undo)?;
                self.log.push(format!("abandon {}", entry.label));
            }
            self.rebuild();
            Ok(())
        })())
    }

    /// Adds a line to the command log, such as who asked for the commands that follow.
    pub fn log_note(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
    }

    /// The label of the command undo would revert.
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|e| e.label.as_str())
    }

    /// The label of the command redo would repeat.
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|e| e.label.as_str())
    }

    /// What a command that changed only the materials state did: its inverse puts the
    /// old state back.
    fn materials_changed(&self, old: PartMaterials, redo: Command, label: String) -> Applied {
        Applied { inverse: Command::RestoreMaterials { state: Box::new(old) }, redo, created: None, label }
    }

    fn index(&self, id: FeatureId) -> Result<usize, CommandError> {
        self.index_of(id).ok_or(CommandError::UnknownFeature(id))
    }

    /// Checks that every feature reads only features above it, in this order.
    fn check_order(features: &[Feature]) -> Result<(), CommandError> {
        for (i, f) in features.iter().enumerate() {
            for dep in f.kind.dependencies() {
                match features.iter().position(|g| g.id == dep) {
                    Some(j) if j < i => {}
                    Some(_) => return Err(CommandError::Order(f.id, dep)),
                    // A missing input is allowed: deleting a feature leaves its readers
                    // to report what broke.
                    None => {}
                }
            }
        }
        Ok(())
    }

    /// Checks that a lattice is the only one and the last feature, so no CAD follows it.
    pub(crate) fn check_lattice_last(features: &[Feature]) -> Result<(), CommandError> {
        let Some(first) = features.iter().position(|f| f.kind.is_lattice()) else { return Ok(()) };
        if features.iter().filter(|f| f.kind.is_lattice()).count() > 1 {
            return Err(CommandError::LatticeNotLast);
        }
        if first + 1 < features.len() {
            return Err(CommandError::AfterLattice(features[first].id));
        }
        Ok(())
    }

    fn check_kind(kind: &FeatureKind) -> Result<(), CommandError> {
        match kind {
            FeatureKind::Fillet(b) | FeatureKind::Chamfer(b) if !(b.size.is_finite() && b.size > 0.0) => {
                Err(CommandError::Invalid(format!("size must be positive, got {}", b.size)))
            }
            FeatureKind::Fillet(b) | FeatureKind::Chamfer(b) if b.edges.is_empty() => {
                Err(CommandError::Invalid("a fillet or chamfer needs at least one edge".into()))
            }
            FeatureKind::Hole(h) if !(h.diameter.is_finite() && h.diameter > 0.0) => {
                Err(CommandError::Invalid(format!("hole diameter must be positive, got {}", h.diameter)))
            }
            FeatureKind::Hole(h) if h.points.is_empty() => {
                Err(CommandError::Invalid("a hole needs at least one point".into()))
            }
            FeatureKind::Pattern(p) => match p.layout {
                PatternSpec::Linear { count, .. } | PatternSpec::Circular { count, .. } if count < 2 => {
                    Err(CommandError::Invalid(format!("a pattern needs at least 2 copies, got {count}")))
                }
                PatternSpec::Linear { spacing, .. } if !(spacing.is_finite() && spacing > 0.0) => {
                    Err(CommandError::Invalid(format!("pattern spacing must be positive, got {spacing}")))
                }
                _ => Ok(()),
            },
            FeatureKind::Shell(s) if !(s.thickness.is_finite() && s.thickness > 0.0) => {
                Err(CommandError::Invalid("shell thickness must be finite and positive".into()))
            }
            FeatureKind::Draft(d) if d.faces.is_empty() => {
                Err(CommandError::Invalid("a draft needs at least one face".into()))
            }
            FeatureKind::Draft(d) if !(d.angle.is_finite() && d.angle.abs() < 90.0 && d.angle != 0.0) => {
                Err(CommandError::Invalid(format!("draft angle must be between -90 and 90 and not 0, got {}", d.angle)))
            }
            FeatureKind::Lattice(l)
                if l.cell_size_mm.is_some_and(|v| !v.is_finite() || v <= 0.0)
                    || l.radius_mm.is_some_and(|v| !v.is_finite() || v <= 0.0) =>
            {
                Err(CommandError::Invalid("lattice cell size and density must be finite and positive".into()))
            }
            FeatureKind::Lattice(l)
                if !(l.accuracy_mm.is_finite()
                    && l.accuracy_mm > 0.0
                    && l.min_detail_mm.is_finite()
                    && l.min_detail_mm > 0.0) =>
            {
                Err(CommandError::Invalid("lattice accuracy and minimum detail must be positive".into()))
            }
            FeatureKind::ImportStep(i) if i.source.is_empty() => {
                Err(CommandError::Invalid("the imported file is empty".into()))
            }
            FeatureKind::ImportStep(i) if !i.hash_matches() => {
                Err(CommandError::Invalid("the imported file does not match its sha256".into()))
            }
            FeatureKind::ImportStep(i) => i.kernel_decisions().map(drop).map_err(CommandError::Invalid),
            _ => Ok(()),
        }
    }

    /// Checks that every feature a new definition reads exists.
    fn check_inputs(&self, kind: &FeatureKind) -> Result<(), CommandError> {
        kind.dependencies().into_iter().try_for_each(|dep| self.index(dep).map(|_| ()))
    }

    /// The next free name for a kind: "Sketch1", "Sketch2", ...
    fn fresh_name(&self, base: &str) -> String {
        (1..).map(|n| format!("{base}{n}")).find(|name| self.features.iter().all(|f| &f.name != name)).unwrap()
    }

    fn insert(&mut self, feature: Feature, position: usize) -> Result<(), CommandError> {
        if position > self.features.len() {
            return Err(CommandError::Position(position));
        }
        if self.features.iter().any(|f| f.id == feature.id) {
            return Err(CommandError::Invalid(format!("feature {} already exists", feature.id)));
        }
        let mut next = self.features.clone();
        next.insert(position, feature.clone());
        Self::check_order(&next)?;
        Self::check_lattice_last(&next)?;
        self.next_id = self.next_id.max(feature.id.0 + 1);
        self.features = next;
        self.touch(position);
        Ok(())
    }

    fn check_drivers(&self, features: &[Feature]) -> Result<(), CommandError> {
        let resolved = self.values.validate(features).map_err(CommandError::Invalid)?;
        if resolved != features {
            return Err(CommandError::Invalid("edit the driving value instead of its driven input".into()));
        }
        Ok(())
    }

    fn apply_values(&mut self, command: Command) -> Result<Applied, CommandError> {
        let old = Command::RestoreValues { state: self.values.clone(), features: self.features.clone() };
        let mut state = self.values.clone();
        let mut features = self.features.clone();
        let label = match &command {
            Command::SetValue { name, expression } => {
                state.set(name, expression).map_err(CommandError::Invalid)?;
                format!("set value {name}")
            }
            Command::Drive { input, expression } => {
                state.drive(input.clone(), expression).map_err(CommandError::Invalid)?;
                "drive input".into()
            }
            Command::RenameValue { name, to } => {
                state.rename(name, to).map_err(CommandError::Invalid)?;
                format!("rename value {name}")
            }
            Command::RestoreValues { state: restored, features: restored_features } => {
                state = restored.clone();
                features = restored_features.clone();
                "restore values".into()
            }
            _ => unreachable!(),
        };
        if features.len() != self.features.len()
            || features.iter().zip(&self.features).any(|(a, b)| {
                a.id != b.id
                    || a.name != b.name
                    || a.suppressed != b.suppressed
                    || a.via_assistant != b.via_assistant
                    || a.kind.is_lattice() != b.kind.is_lattice()
            })
        {
            return Err(CommandError::Invalid("a value command cannot replace the feature tree".into()));
        }
        features = state.validate(&features).map_err(CommandError::Invalid)?;
        for f in &features {
            Self::check_kind(&f.kind)?;
        }
        Self::check_order(&features)?;
        Self::check_lattice_last(&features)?;
        let first = self.features.iter().zip(&features).position(|(a, b)| a.kind != b.kind);
        self.values = state;
        self.features = features;
        if let Some(i) = first {
            self.touch(i);
        }
        Ok(Applied { inverse: old, redo: command, created: None, label })
    }

    fn apply(&mut self, command: Command) -> Result<Applied, CommandError> {
        match command {
            command @ (Command::SetValue { .. }
            | Command::Drive { .. }
            | Command::RenameValue { .. }
            | Command::RestoreValues { .. }) => self.apply_values(command),
            Command::AddFeature { kind, name, position, via_assistant } => {
                Self::check_kind(&kind)?;
                self.check_inputs(&kind)?;
                let position = position.unwrap_or_else(|| self.active_count());
                let name = name.unwrap_or_else(|| self.fresh_name(kind.base_name()));
                let feature = Feature { id: FeatureId(self.next_id), name, kind, suppressed: false, via_assistant };
                let id = feature.id;
                let label = format!("add {}", feature.name);
                self.insert(feature.clone(), position)?;
                Ok(Applied {
                    inverse: Command::DeleteFeature { id },
                    redo: Command::InsertFeature { feature, position },
                    created: Some(id),
                    label,
                })
            }
            Command::InsertFeature { feature, position } => {
                Self::check_kind(&feature.kind)?;
                let id = feature.id;
                let label = format!("insert {}", feature.name);
                self.insert(feature.clone(), position)?;
                Ok(Applied {
                    inverse: Command::DeleteFeature { id },
                    redo: Command::InsertFeature { feature, position },
                    created: Some(id),
                    label,
                })
            }
            Command::EditFeature { id, kind } => {
                Self::check_kind(&kind)?;
                self.check_inputs(&kind)?;
                let i = self.index(id)?;
                let mut next = self.features.clone();
                let old = std::mem::replace(&mut next[i].kind, kind.clone());
                if old.is_lattice() != kind.is_lattice() {
                    return Err(CommandError::Invalid("a feature cannot change into or out of a lattice".into()));
                }
                Self::check_order(&next)?;
                self.check_drivers(&next)?;
                let label = format!("edit {}", next[i].name);
                self.features = next;
                self.touch(i);
                Ok(Applied {
                    inverse: Command::EditFeature { id, kind: old },
                    redo: Command::EditFeature { id, kind },
                    created: None,
                    label,
                })
            }
            Command::SetSketchDimension { id, constraint, value } => {
                let i = self.index(id)?;
                let FeatureKind::Sketch(old) = &self.features[i].kind else {
                    return Err(CommandError::WrongKind(id, "sketch"));
                };
                let mut sketch = old.clone();
                let target =
                    sketch.sketch.constraint_mut(constraint).filter(|c| c.value().is_some()).ok_or_else(|| {
                        CommandError::Sketch(format!("{constraint} is not a dimension of this sketch"))
                    })?;
                target.set_value(value);
                soliddraft_sketch::solve::solve(&mut sketch.sketch, None, true);
                let status: SolveStatus = sketch.sketch.status.clone();
                if let SolveStatus::Failed(error) = &status {
                    return Err(CommandError::Sketch(error.clone()));
                }
                let old = self.features[i].kind.clone();
                let kind = FeatureKind::Sketch(sketch);
                let mut next = self.features.clone();
                next[i].kind = kind.clone();
                self.check_drivers(&next)?;
                let label = format!("set {} {constraint} to {value}", self.features[i].name);
                self.features[i].kind = kind.clone();
                self.last_solve = Some(status);
                self.touch(i);
                Ok(Applied {
                    inverse: Command::EditFeature { id, kind: old },
                    redo: Command::EditFeature { id, kind },
                    created: None,
                    label,
                })
            }
            Command::RenameFeature { id, name } => {
                if name.trim().is_empty() {
                    return Err(CommandError::Invalid("a feature name cannot be empty".into()));
                }
                let i = self.index(id)?;
                let old = std::mem::replace(&mut self.features[i].name, name.clone());
                // Messages that name the feature are made again; every build is reused.
                self.touch(i);
                Ok(Applied {
                    inverse: Command::RenameFeature { id, name: old.clone() },
                    redo: Command::RenameFeature { id, name: name.clone() },
                    created: None,
                    label: format!("rename {old} to {name}"),
                })
            }
            Command::RenameDatumPlane { datum, name } => {
                let name = name.trim().to_owned();
                let old = self.rename_datum(datum, &name)?;
                Ok(Applied {
                    inverse: Command::RenameDatumPlane { datum, name: old.clone() },
                    redo: Command::RenameDatumPlane { datum, name: name.clone() },
                    created: None,
                    label: format!("rename {old} to {name}"),
                })
            }
            Command::DeleteFeature { id } => {
                let i = self.index(id)?;
                let mut next = self.features.clone();
                next.remove(i);
                self.check_drivers(&next)?;
                let feature = self.features.remove(i);
                let label = format!("delete {}", feature.name);
                let mut inverse = Command::InsertFeature { feature, position: i };
                if self.rollback == Some(id) {
                    // The bar stays where it was: above the feature that followed.
                    self.rollback = self.features.get(i).map(|f| f.id);
                    inverse = Command::Batch {
                        label: label.clone(),
                        commands: vec![inverse, Command::RollTo { before: Some(id) }],
                    };
                }
                self.touch(i);
                Ok(Applied { inverse, redo: Command::DeleteFeature { id }, created: None, label })
            }
            Command::MoveFeature { id, position } => {
                let from = self.index(id)?;
                if position >= self.features.len() {
                    return Err(CommandError::Position(position));
                }
                let mut next = self.features.clone();
                let f = next.remove(from);
                let label = format!("move {} to position {}", f.name, position + 1);
                next.insert(position, f);
                Self::check_order(&next)?;
                Self::check_lattice_last(&next)?;
                self.features = next;
                self.touch(from.min(position));
                Ok(Applied {
                    inverse: Command::MoveFeature { id, position: from },
                    redo: Command::MoveFeature { id, position },
                    created: None,
                    label,
                })
            }
            Command::SetSuppressed { id, suppressed } => {
                let i = self.index(id)?;
                let old = std::mem::replace(&mut self.features[i].suppressed, suppressed);
                let label = format!("{} {}", if suppressed { "suppress" } else { "unsuppress" }, self.features[i].name);
                if old != suppressed {
                    self.touch(i);
                }
                Ok(Applied {
                    inverse: Command::SetSuppressed { id, suppressed: old },
                    redo: Command::SetSuppressed { id, suppressed },
                    created: None,
                    label,
                })
            }
            Command::RollTo { before } => {
                if let Some(id) = before {
                    self.index(id)?;
                }
                let old = self.rollback;
                let (a, b) =
                    (self.active_count(), before.and_then(|id| self.index_of(id)).unwrap_or(self.features.len()));
                self.rollback = before;
                self.touch(a.min(b));
                let label = match before.and_then(|id| self.feature(id)) {
                    Some(f) => format!("roll back to before {}", f.name),
                    None => "roll forward to the end".to_string(),
                };
                Ok(Applied {
                    inverse: Command::RollTo { before: old },
                    redo: Command::RollTo { before },
                    created: None,
                    label,
                })
            }
            Command::SetMaterial { material, default_appearance, scope } => {
                if let Some(m) = &material {
                    check_entry(m.problems())?;
                }
                if let Some(a) = &default_appearance {
                    if material.as_ref().and_then(|m| m.default_appearance.as_ref()) != Some(&a.id) {
                        return Err(CommandError::Invalid(format!(
                            "{} is not the material's default appearance",
                            a.id
                        )));
                    }
                    check_entry(a.problems())?;
                }
                let old = self.materials.clone();
                let label = match &material {
                    Some(m) => format!("set material {}", m.name),
                    None => "remove material".to_owned(),
                };
                let state = &mut self.materials;
                state.material.retain(|a| a.scope != scope);
                if let Some(m) = &material {
                    state.material.push(MaterialAssignment { material: m.id.clone(), scope: scope.clone() });
                    state.snapshots.put_material(m.clone());
                    if let Some(a) = &default_appearance {
                        state.snapshots.put_appearance(a.clone());
                    }
                }
                state.prune();
                Ok(self.materials_changed(old, Command::SetMaterial { material, default_appearance, scope }, label))
            }
            Command::SetAppearance { target, appearance, scope } => {
                if let AppearanceTarget::Feature { feature } = &target {
                    self.index(*feature)?;
                }
                if let Some(a) = &appearance {
                    check_entry(a.problems())?;
                }
                let old = self.materials.clone();
                let place = match &target {
                    AppearanceTarget::Part => "the part".to_owned(),
                    AppearanceTarget::Feature { feature } => {
                        self.feature(*feature).map(|f| f.name.clone()).unwrap_or_else(|| feature.to_string())
                    }
                    AppearanceTarget::Face { face } if !face.label.is_empty() => face.label.clone(),
                    AppearanceTarget::Face { face } => format!("a face of {}", face.feature),
                };
                let label = match &appearance {
                    Some(a) => format!("set appearance {} on {place}", a.name),
                    None => format!("remove appearance from {place}"),
                };
                let state = &mut self.materials;
                state.appearances.retain(|a| !(a.target.same_place(&target) && a.scope == scope));
                if let Some(a) = &appearance {
                    state.appearances.push(AppearanceAssignment {
                        target: target.clone(),
                        appearance: a.id.clone(),
                        scope: scope.clone(),
                    });
                    state.snapshots.put_appearance(a.clone());
                }
                state.prune();
                Ok(self.materials_changed(old, Command::SetAppearance { target, appearance, scope }, label))
            }
            Command::UpdateCopies { materials, appearances } => {
                let old = self.materials.clone();
                let state = &mut self.materials;
                for m in &materials {
                    if state.snapshots.material(&m.id).is_none() {
                        return Err(CommandError::Invalid(format!("the part does not use the material {}", m.id)));
                    }
                    check_entry(m.problems())?;
                }
                for a in &appearances {
                    if state.snapshots.appearance(&a.id).is_none() {
                        return Err(CommandError::Invalid(format!("the part does not use the appearance {}", a.id)));
                    }
                    check_entry(a.problems())?;
                }
                for m in &materials {
                    state.snapshots.put_material(m.clone());
                }
                for a in &appearances {
                    state.snapshots.put_appearance(a.clone());
                }
                let label = "update material and appearance copies".to_owned();
                Ok(self.materials_changed(old, Command::UpdateCopies { materials, appearances }, label))
            }
            Command::RestoreMaterials { state } => {
                state.check().map_err(CommandError::Invalid)?;
                let old = std::mem::replace(&mut self.materials, *state.clone());
                let label = "restore materials".to_owned();
                Ok(self.materials_changed(old, Command::RestoreMaterials { state }, label))
            }
            Command::Batch { label, commands } => {
                let dirty = self.dirty;
                let mut done: Vec<Applied> = Vec::new();
                for c in commands {
                    match self.apply(c) {
                        Ok(a) => done.push(a),
                        Err(e) => {
                            for a in done.into_iter().rev() {
                                self.apply(a.inverse).expect("the inverse of an applied command applies");
                            }
                            self.dirty = dirty;
                            return Err(e);
                        }
                    }
                }
                let created = done.iter().rev().find_map(|a| a.created);
                let redo =
                    Command::Batch { label: label.clone(), commands: done.iter().map(|a| a.redo.clone()).collect() };
                let inverse = Command::Batch {
                    label: label.clone(),
                    commands: done.into_iter().rev().map(|a| a.inverse).collect(),
                };
                Ok(Applied { inverse, redo, created, label })
            }
        }
    }
}

/// Refuses a material or appearance with values out of range.
fn check_entry(problems: Vec<soliddraft_materials::Problem>) -> Result<(), CommandError> {
    match problems.first() {
        Some(p) => Err(CommandError::Invalid(p.to_string())),
        None => Ok(()),
    }
}

impl Document {
    /// Runs the canonical part commands accepted by MCP and journal replay.
    pub fn run_journal_command(&mut self, input: serde_json::Value) -> Result<(), String> {
        let parse = |key: &str| input.get(key).cloned().ok_or_else(|| format!("missing {key}"));
        match input["op"].as_str().unwrap_or_default() {
            "execute" => self
                .execute(serde_json::from_value(parse("command")?).map_err(|e| e.to_string())?)
                .map(|_| ())
                .map_err(|e| e.to_string()),
            "execute_deferred" => self
                .execute_deferred(serde_json::from_value(parse("command")?).map_err(|e| e.to_string())?)
                .map(|_| ())
                .map_err(|e| e.to_string()),
            "undo_deferred" => self.undo_deferred().map(|_| ()).map_err(|e| e.to_string()),
            "redo_deferred" => self.redo_deferred().map(|_| ()).map_err(|e| e.to_string()),
            "rebuild" => {
                self.rebuild();
                Ok(())
            }
            "undo" => self.undo().map(|_| ()).map_err(|e| e.to_string()),
            "redo" => self.redo().map(|_| ()).map_err(|e| e.to_string()),
            "rollback" => {
                self.set_rollback(serde_json::from_value(parse("before")?).map_err(|e| e.to_string())?);
                Ok(())
            }
            "settings" => self.set_settings(serde_json::from_value(parse("settings")?).map_err(|e| e.to_string())?),
            "group_undo" => {
                self.group_undo(
                    input["depth"].as_u64().ok_or("missing depth")? as usize,
                    input["label"].as_str().ok_or("missing label")?,
                );
                Ok(())
            }
            "abandon" => {
                self.abandon(input["depth"].as_u64().ok_or("missing depth")? as usize).map_err(|e| e.to_string())
            }
            _ => Err("unknown part journal command".into()),
        }
    }

    /// Registers this part for a drawing command that reads it.
    pub fn journal_key(&self) -> Option<u64> {
        if let Some(id) = &self.journal {
            soliddraft_document::journal::register(id, "part", || self.journal_state());
            Some(id.key())
        } else {
            None
        }
    }

    /// Registers the starting part once and records an application command before it runs.
    /// Scratch forks do not journal speculative edits.
    pub fn journal_command(
        &self,
        command: impl FnOnce() -> serde_json::Value,
    ) -> soliddraft_document::journal::CommandGuard {
        use soliddraft_document::journal;
        if let Some(id) = &self.journal {
            journal::register(id, "part", || self.journal_state());
            journal::begin(id, command)
        } else {
            journal::disabled()
        }
    }
}
