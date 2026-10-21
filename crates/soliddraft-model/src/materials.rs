//! A part's material and appearances.
//!
//! A part has a material (none until one is assigned) and appearances on the whole part,
//! on the faces a feature made, or on single faces. The most specific wins: a face's
//! own appearance, then its feature's, then the part's, then the material's default
//! appearance, then SolidDraft's default grey. Faces are named persistently (see
//! [`reference`](crate::reference)), so an appearance follows its face through edits;
//! a face that splits keeps the appearance on every piece, and an assignment whose
//! face is gone is reported as lost rather than moved to another face.
//!
//! Every assignment carries a configuration scope. Until configurations exist the
//! scope is always "all", and the part is shown in its default configuration.
//!
//! The part keeps a copy of every material and appearance it uses, so its look and mass
//! never depend on a library (see [`soliddraft_materials::Snapshots`]).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use soliddraft_document::{ConfigScope, ConfigurationId};
use soliddraft_kernel_api::FaceId;
use soliddraft_materials::{Appearance, AppearanceId, Mass, Material, MaterialId, Snapshots, Unknown, scope};

use crate::document::Document;
use crate::feature::FeatureId;
use crate::reference::{self, Candidate, FaceRef, RepairReason, View};

/// Where an appearance is applied.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "on", rename_all = "snake_case")]
pub enum AppearanceTarget {
    /// The whole part.
    Part,
    /// Every face a feature made, through later splits.
    Feature {
        /// The feature.
        feature: FeatureId,
    },
    /// One face, by its persistent name.
    Face {
        /// The face.
        face: FaceRef,
    },
}

impl AppearanceTarget {
    /// True when both name the same place, however their fingerprints were taken: two
    /// references to a face are the same place when they name the same feature,
    /// generator and ordinal.
    pub fn same_place(&self, other: &AppearanceTarget) -> bool {
        match (self, other) {
            (AppearanceTarget::Part, AppearanceTarget::Part) => true,
            (AppearanceTarget::Feature { feature: a }, AppearanceTarget::Feature { feature: b }) => a == b,
            (AppearanceTarget::Face { face: a }, AppearanceTarget::Face { face: b }) => {
                (a.feature, a.generator, a.ordinal) == (b.feature, b.generator, b.ordinal)
            }
            _ => false,
        }
    }

    /// How specific the target is: faces beat features, features beat the part.
    fn level(&self) -> u8 {
        match self {
            AppearanceTarget::Part => 0,
            AppearanceTarget::Feature { .. } => 1,
            AppearanceTarget::Face { .. } => 2,
        }
    }
}

/// An appearance applied somewhere in the part, in some configurations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppearanceAssignment {
    /// Where.
    #[serde(flatten)]
    pub target: AppearanceTarget,
    /// Which appearance; the part holds its copy.
    pub appearance: AppearanceId,
    /// In which configurations.
    #[serde(flatten)]
    pub scope: ConfigScope,
}

/// The part's material, in some configurations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaterialAssignment {
    /// Which material; the part holds its copy.
    pub material: MaterialId,
    /// In which configurations.
    #[serde(flatten)]
    pub scope: ConfigScope,
}

/// Everything about a part's materials and appearances that its file holds, as the
/// `material`, `appearances` and `snapshots` keys of `part.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PartMaterials {
    /// The material assignments; at most one per scope.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub material: Vec<MaterialAssignment>,
    /// The appearance assignments, at most one per place and scope, in the order they
    /// were made.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub appearances: Vec<AppearanceAssignment>,
    /// The copies of every material and appearance the assignments name.
    #[serde(default, skip_serializing_if = "Snapshots::is_empty")]
    pub snapshots: Snapshots,
}

impl PartMaterials {
    /// True when the part has no material and no appearance.
    pub fn is_empty(&self) -> bool {
        self.material.is_empty() && self.appearances.is_empty() && self.snapshots.is_empty()
    }

    /// Drops the copies no assignment uses.
    pub(crate) fn prune(&mut self) {
        let materials: Vec<MaterialId> = self.material.iter().map(|m| m.material.clone()).collect();
        let appearances: Vec<AppearanceId> = self.appearances.iter().map(|a| a.appearance.clone()).collect();
        self.snapshots.retain_used(&materials, &appearances);
    }

    /// Why the file's materials section is inconsistent, if it is: an assignment
    /// naming a material or appearance the file holds no copy of.
    pub(crate) fn check(&self) -> Result<(), String> {
        for m in &self.material {
            if self.snapshots.material(&m.material).is_none() {
                return Err(format!("the material {} has no copy in the file", m.material));
            }
        }
        for a in &self.appearances {
            if self.snapshots.appearance(&a.appearance).is_none() {
                return Err(format!("the appearance {} has no copy in the file", a.appearance));
            }
        }
        Ok(())
    }
}

/// An appearance assignment whose place is not in the part any more, or not as it was.
/// It stays in the part, and applies again if its face comes back (a feature is
/// unsuppressed, an edit is undone); the properties panel offers to re-point it or
/// remove it.
#[derive(Clone, Debug, PartialEq)]
pub struct LostAppearance {
    /// Its index in [`PartMaterials::appearances`].
    pub index: usize,
    /// Why.
    pub reason: RepairReason,
    /// The problem, for people.
    pub message: String,
    /// Faces it could be moved to, best match first.
    pub candidates: Vec<Candidate>,
}

/// The appearance of every face of the part's body, as a renderer reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct AppearanceTable {
    /// The appearances in use. Index 0 is the part's own: what a face with no
    /// appearance of its own or its feature's shows.
    pub appearances: Vec<Appearance>,
    /// The faces that show another appearance than index 0, and its index.
    pub faces: HashMap<FaceId, u32>,
    /// Assignments that did not apply.
    pub lost: Vec<LostAppearance>,
}

impl AppearanceTable {
    /// The appearance a face shows.
    pub fn of(&self, face: FaceId) -> &Appearance {
        &self.appearances[self.faces.get(&face).copied().unwrap_or(0) as usize]
    }
}

/// Why a part has no mass to show.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum MassError {
    /// The part has no material, or its material no density.
    #[error("{0}")]
    Unknown(Unknown),
    /// The part has no body yet.
    #[error("the part has no body")]
    NoBody,
    /// The kernel could not measure the body.
    #[error("the kernel could not measure the body: {0}")]
    Kernel(String),
}

impl Document {
    /// The part's material and appearance assignments and the copies they use.
    pub fn materials(&self) -> &PartMaterials {
        &self.materials
    }

    /// The part's material in a configuration (`None` for the default one), from the
    /// copy the part holds.
    pub fn material(&self, configuration: Option<&ConfigurationId>) -> Option<&Material> {
        let m = &self.materials;
        scope::pick(&m.material, |a| &a.scope, configuration).and_then(|a| m.snapshots.material(&a.material))
    }

    /// What a face with no appearance of its own or its feature's shows: the part's
    /// appearance, else the material's default appearance, else SolidDraft's default.
    pub fn part_appearance(&self, configuration: Option<&ConfigurationId>) -> Appearance {
        let m = &self.materials;
        let part = m.appearances.iter().filter(|a| a.target == AppearanceTarget::Part);
        scope::pick(part, |a| &a.scope, configuration)
            .map(|a| &a.appearance)
            .or_else(|| self.material(configuration).and_then(|mat| mat.default_appearance.as_ref()))
            .and_then(|id| m.snapshots.appearance(id))
            .cloned()
            .unwrap_or_else(Appearance::default_part)
    }

    /// The appearance of every face of the part's body in a configuration, with the
    /// assignments that did not apply.
    pub fn appearance_table(&self, configuration: Option<&ConfigurationId>) -> AppearanceTable {
        let mut table = AppearanceTable {
            appearances: vec![self.part_appearance(configuration)],
            faces: HashMap::new(),
            lost: Vec::new(),
        };
        let m = &self.materials;
        // The assignment that holds at each place, most specific place last so it wins.
        let mut chosen: Vec<(usize, &AppearanceAssignment)> = Vec::new();
        for (i, a) in m.appearances.iter().enumerate() {
            if a.target == AppearanceTarget::Part || !scope::applies(&a.scope, configuration) {
                continue;
            }
            match chosen.iter_mut().find(|(_, c)| c.target.same_place(&a.target)) {
                Some(slot) if scope::rank(&a.scope) >= scope::rank(&slot.1.scope) => *slot = (i, a),
                Some(_) => {}
                None => chosen.push((i, a)),
            }
        }
        chosen.sort_by_key(|(i, a)| (a.target.level(), *i));
        if chosen.is_empty() {
            return table;
        }
        let Some(body) = self.body() else { return table };
        let Ok((topology, history)) = self.view_of(body) else { return table };
        let view = View { topology: &topology, history: &history };
        let names = self.name_map();
        let mut index: HashMap<&AppearanceId, u32> = HashMap::new();
        for (i, a) in chosen {
            let faces = match &a.target {
                AppearanceTarget::Part => continue,
                AppearanceTarget::Feature { feature } => {
                    if self.feature(*feature).is_none() {
                        table.lost.push(LostAppearance {
                            index: i,
                            reason: RepairReason::Missing,
                            message: format!("{feature} is not in the part any more"),
                            candidates: Vec::new(),
                        });
                        continue;
                    }
                    reference::faces_of_feature(*feature, &view, &names)
                }
                AppearanceTarget::Face { face } => match reference::resolve_faces(face, &view, &names) {
                    Ok(faces) => faces,
                    Err(u) => {
                        let message =
                            if face.label.is_empty() { u.message } else { format!("{}: {}", face.label, u.message) };
                        table.lost.push(LostAppearance {
                            index: i,
                            reason: u.reason,
                            message,
                            candidates: u.candidates,
                        });
                        continue;
                    }
                },
            };
            let Some(appearance) = m.snapshots.appearance(&a.appearance) else { continue };
            let next = table.appearances.len() as u32;
            let slot = *index.entry(&a.appearance).or_insert_with(|| {
                table.appearances.push(appearance.clone());
                next
            });
            for f in faces {
                table.faces.insert(f, slot);
            }
        }
        table
    }

    /// The part's mass properties in a configuration, from its body and the density of
    /// its material. Unknown without a material or a density; never guessed.
    pub fn mass(&self, configuration: Option<&ConfigurationId>) -> Result<Mass, MassError> {
        let body = self.body().ok_or(MassError::NoBody)?;
        let props = self.kernel.mass_properties(body).map_err(|e| MassError::Kernel(e.to_string()))?;
        soliddraft_materials::mass::of_body(&props, self.material(configuration)).map_err(MassError::Unknown)
    }
}
