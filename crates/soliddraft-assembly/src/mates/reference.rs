//! Finding faces and proving their planes at a pinned revision.
//!
//! A proof belongs to a document, a revision and a face. It comes from the part rebuilt on
//! the kernel from that revision's bytes, never from a display copy, so a cached picture
//! can be shown but never proves a mate. Selectors are for commands: what is stored is the
//! part's persistent [`FaceRef`].

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use soliddraft_document::{ConfigurationId, ContentHash, DocKind, DocRef, DocumentId};
use soliddraft_kernel_api::{ExactAxisPlane, FaceId, KernelError};
use soliddraft_model::{Document as PartDocument, FaceRef, FeatureId, Generator, RepairReason};

use crate::document::AssemblyDocument;
use crate::mates::{FaceEndpoint, Mate, MateRefusal, RefusalCode};
use crate::path::ComponentId;
use crate::schema::Assembly;

/// How many rebuilt parts a document keeps for proofs.
const PARTS_KEPT: usize = 16;

/// Which kind of face a selector asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeneratorChoice {
    /// An extrusion's cap in the sketch plane.
    StartCap,
    /// An extrusion's cap at its far end.
    EndCap,
    /// A side swept from a sketch entity.
    Side,
}

impl GeneratorChoice {
    fn matches(self, generator: Generator) -> bool {
        match self {
            GeneratorChoice::StartCap => generator == Generator::StartCap,
            GeneratorChoice::EndCap => generator == Generator::EndCap,
            GeneratorChoice::Side => matches!(generator, Generator::Side { .. }),
        }
    }
}

/// How a command names a face. Each finds one face at command time or refuses.
#[derive(Clone, Debug, PartialEq)]
pub enum FaceSelector {
    /// The persistent name a pick carries.
    Picked(FaceRef),
    /// The face's current label, such as "Boss-Extrude1 · End cap".
    Label(String),
    /// The face a feature's generator made; a side also says which way it looks.
    Generator {
        /// The feature's current name.
        feature: String,
        /// What made the face.
        generator: GeneratorChoice,
        /// The proven outward normal, to tell the sides apart.
        normal: Option<[f64; 3]>,
    },
}

/// A face of one of the root's components.
#[derive(Clone, Debug, PartialEq)]
pub struct EndpointSelector {
    /// The component.
    pub component: ComponentId,
    /// The face.
    pub face: FaceSelector,
}

/// A plane proven on one revision of a part.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PlaneProof {
    /// The part.
    pub document: DocumentId,
    /// The revision it was proven on.
    pub revision: ContentHash,
    /// That revision's geometry hash.
    pub geometry: ContentHash,
    /// The plane in the part's frame.
    pub plane: ExactAxisPlane,
    /// The face's label at that revision.
    pub label: String,
}

type PlaneKey = (DocumentId, ContentHash, FeatureId, Generator, u32);

/// Proofs and the rebuilt parts behind them. All of it is derived: dropping it changes
/// nothing but how soon the next proof comes.
#[derive(Default)]
pub(crate) struct Proofs {
    parts: HashMap<(DocumentId, ContentHash), Arc<Mutex<PartDocument>>>,
    planes: HashMap<PlaneKey, PlaneProof>,
    /// How many parts were rebuilt for proofs.
    pub built: usize,
}

fn unavailable(message: String) -> MateRefusal {
    MateRefusal::new(RefusalCode::ProofUnavailable, message)
}

fn configuration_refusal(what: fmt::Arguments, configuration: &ConfigurationId) -> MateRefusal {
    MateRefusal::new(
        RefusalCode::UnsupportedConfiguration,
        format!("{what} names configuration {}; this version mates default configurations only", configuration.0),
    )
}

/// Refuses a mate that would have to ignore a configuration. A file keeps the field as
/// read, so this runs before anything is proved or changed.
pub(crate) fn check_configurations(truth: &Assembly) -> Result<(), MateRefusal> {
    for mate in truth.known_mates() {
        let ends = match mate {
            Mate::Distance { a, b, .. } => vec![a, b],
            Mate::Origin { .. } => Vec::new(),
        };
        for end in ends {
            check_endpoint(end).map_err(|r| MateRefusal { mates: vec![mate.name().to_owned()], ..r })?;
        }
        for c in mate.components() {
            let source = truth.component(c).and_then(|c| truth.source(c.source));
            if let Some(configuration) = source.and_then(|s| s.document.configuration.as_ref()) {
                let refused = configuration_refusal(format_args!("component {c}"), configuration);
                return Err(MateRefusal { mates: vec![mate.name().to_owned()], ..refused });
            }
        }
    }
    Ok(())
}

/// Refuses a stored face of a configuration other than the default.
pub(crate) fn check_endpoint(end: &FaceEndpoint) -> Result<(), MateRefusal> {
    match &end.configuration {
        Some(configuration) => Err(configuration_refusal(format_args!("occurrence {}", end.path), configuration)),
        None => Ok(()),
    }
}

impl AssemblyDocument {
    fn proof_part(&mut self, r: &DocRef) -> Result<Arc<Mutex<PartDocument>>, MateRefusal> {
        let id = r.id.ok_or_else(|| unavailable("the reference has no document id".into()))?;
        let key = (id, r.pinned.clone());
        if let Some(part) = self.proofs.parts.get(&key) {
            return Ok(part.clone());
        }
        let bytes = self.revision_bytes(r).map_err(unavailable)?;
        let display = self.options.display.clone();
        let source = display
            .as_deref()
            .and_then(|d| d.kernel_source())
            .ok_or_else(|| unavailable("this session has no kernel to prove planes on".into()))?;
        let part = Arc::new(Mutex::new(source.exact(&bytes).map_err(unavailable)?));
        if self.proofs.parts.len() == PARTS_KEPT {
            self.proofs.parts.clear();
        }
        self.proofs.parts.insert(key, part.clone());
        self.proofs.built += 1;
        Ok(part)
    }

    /// The proof of `face`'s plane at the revision `r` pins.
    pub(crate) fn prove(&mut self, r: &DocRef, face: &FaceRef) -> Result<PlaneProof, MateRefusal> {
        let id = r.id.ok_or_else(|| unavailable("the reference has no document id".into()))?;
        let key = (id, r.pinned.clone(), face.feature, face.generator, face.ordinal);
        if let Some(proof) = self.proofs.planes.get(&key) {
            return Ok(proof.clone());
        }
        let part = self.proof_part(r)?;
        let part = part.lock().expect("the part lock");
        let body = part.body().ok_or_else(|| unavailable(format!("{} has no body", r.path)))?;
        let found = part.resolve_face(face, body).map_err(|repair| {
            let code = match repair.reason {
                RepairReason::Ambiguous => RefusalCode::AmbiguousReference,
                RepairReason::Missing | RepairReason::Mismatch { .. } => RefusalCode::DanglingReference,
            };
            MateRefusal::new(code, repair.message)
        })?;
        let proof = plane_of(&part, r, id, found)?;
        drop(part);
        self.proofs.planes.insert(key, proof.clone());
        Ok(proof)
    }

    /// The persistent name of the face `selector` finds on the part `r` pins.
    pub(crate) fn select(&mut self, r: &DocRef, selector: &FaceSelector) -> Result<FaceRef, MateRefusal> {
        let id = r.id.ok_or_else(|| unavailable("the reference has no document id".into()))?;
        let part = self.proof_part(r)?;
        let part = part.lock().expect("the part lock");
        let body = part.body().ok_or_else(|| unavailable(format!("{} has no body", r.path)))?;
        let labels = part.face_labels(body);
        let found: Vec<FaceId> = match selector {
            FaceSelector::Picked(face) => {
                let repaired = part
                    .resolve_face(face, body)
                    .map_err(|repair| MateRefusal::new(RefusalCode::DanglingReference, repair.message))?;
                vec![repaired]
            }
            FaceSelector::Label(text) => labels
                .iter()
                .filter(|(_, l)| l.as_ref().is_some_and(|l| l.text.eq_ignore_ascii_case(text.trim())))
                .map(|(f, _)| *f)
                .collect(),
            FaceSelector::Generator { feature, generator, normal } => {
                let wanted = part.features().iter().find(|f| f.name == *feature).map(|f| f.id);
                labels
                    .iter()
                    .filter(|(_, l)| {
                        l.as_ref().is_some_and(|l| Some(l.feature) == wanted && generator.matches(l.generator))
                    })
                    .map(|(f, _)| *f)
                    .filter(|f| {
                        normal.is_none_or(|n| {
                            plane_of(&part, r, id, *f).is_ok_and(|p| p.plane.normal().map(f64::from) == n)
                        })
                    })
                    .collect()
            }
        };
        let [face] = found[..] else {
            let (code, what) = if found.is_empty() {
                (RefusalCode::NoSuchFace, "finds no face")
            } else {
                (RefusalCode::AmbiguousReference, "finds several faces")
            };
            return Err(MateRefusal::new(code, format!("the selector {selector:?} {what} on {}", r.path)));
        };
        part.face_ref(face).map_err(|e| MateRefusal::new(RefusalCode::AmbiguousReference, e.to_string()))
    }

    /// The reference of a root component that places a part.
    pub(crate) fn part_source(truth: &Assembly, component: ComponentId) -> Result<DocRef, MateRefusal> {
        let c = truth
            .component(component)
            .ok_or_else(|| MateRefusal::invalid(format!("the assembly has no component {component}")))?;
        let source = truth
            .source(c.source)
            .ok_or_else(|| MateRefusal::invalid(format!("component {component} has no source")))?;
        if let Some(configuration) = &source.document.configuration {
            return Err(configuration_refusal(format_args!("component {component}"), configuration));
        }
        if source.document.kind != DocKind::Part {
            return Err(MateRefusal::invalid(format!(
                "component {component} is a sub-assembly; mates reach the parts placed directly for now"
            )));
        }
        Ok(source.document.clone())
    }
}

fn plane_of(part: &PartDocument, r: &DocRef, document: DocumentId, face: FaceId) -> Result<PlaneProof, MateRefusal> {
    let body = part.body().ok_or_else(|| unavailable(format!("{} has no body", r.path)))?;
    let plane = part.kernel().face_support_plane(body, face).map_err(|e| {
        let code = match e {
            KernelError::Unsupported(_) => RefusalCode::UnsupportedPlane,
            _ => RefusalCode::ProofUnavailable,
        };
        MateRefusal::new(code, format!("{}: {e}", r.path))
    })?;
    Ok(PlaneProof {
        document,
        revision: r.pinned.clone(),
        geometry: part.geometry_hash(),
        plane,
        label: part.face_label(face).map(|l| l.text).unwrap_or_default(),
    })
}
