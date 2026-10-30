//! Review immutable STEP source bytes before committing the imported feature.

use std::path::Path;

use serde_json::{Value, json};
use soliddraft_assistant::tools::schema::{array, optional, string};
use soliddraft_kernel_api::{Approval, Disposition, ImportOptions, ProposalSet};
use soliddraft_model::ImportStepFeature;

use crate::document::{Command, Document, FeatureId, FeatureKind};
use crate::state::{AppState, NoteKind};

pub fn params() -> Vec<(&'static str, Value)> {
    let mut params = preview_params();
    params.push(("decline", optional(array(string("Gap id to decline, such as g0."), "Repairs to leave open."))));
    params
}

pub fn preview_params() -> Vec<(&'static str, Value)> {
    vec![("path", string("A STEP source file path."))]
}

pub fn source_path(input: &Value) -> Result<std::path::PathBuf, String> {
    let path = input["path"].as_str().filter(|p| !p.trim().is_empty()).ok_or("missing STEP source path")?;
    Ok(path.into())
}

#[derive(Clone, Debug)]
pub struct Preview {
    feature: ImportStepFeature,
    proposals: ProposalSet,
    revision: u64,
    document: soliddraft_document::DocumentId,
}

impl Preview {
    pub fn read(doc: &Document, path: &Path) -> Result<Self, String> {
        let source = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Self::from_bytes(doc, &path.file_name().unwrap_or_default().to_string_lossy(), source)
    }

    pub fn from_bytes(doc: &Document, name: &str, source: Vec<u8>) -> Result<Self, String> {
        let feature = ImportStepFeature::new(name, source);
        let proposals =
            doc.kernel().propose_import(&feature.source, &ImportOptions::default()).map_err(|e| e.to_string())?;
        if proposals.source_sha256 != feature.sha256 {
            return Err("the repair preview does not match the source file".into());
        }
        Ok(Self { feature, proposals, revision: doc.revision(), document: doc.model().id() })
    }

    pub fn summary(&self) -> Value {
        json!({"source_name": self.feature.source_name, "source_sha256": self.feature.sha256,
               "proposals": self.proposals.proposals})
    }
}

pub fn commit(doc: &mut Document, preview: &Preview, decline: &[String]) -> Result<FeatureId, String> {
    if doc.revision() != preview.revision || doc.model().id() != preview.document || doc.proof_pending() {
        return Err("the part changed during the preview; review the import again".into());
    }
    let mut feature = preview.feature.clone();
    for id in decline {
        let proposal = preview
            .proposals
            .proposals
            .iter()
            .find(|p| &p.id == id)
            .ok_or_else(|| format!("no repair named {id} in this file"))?;
        feature = feature.declined(&proposal.question);
    }
    if preview.proposals.proposals.iter().any(|p| p.disposition == Disposition::Ask && !decline.contains(&p.id)) {
        return Err("this repair needs an explicit tolerance approval before it can be imported".into());
    }
    let command = Command::AddFeature(FeatureKind::ImportStep(feature));
    // Keep a refused multibody file out of the part and reuse the accepted trial's build.
    let mut trial = doc.scratch();
    let trial_result = trial.apply(command.clone());
    let imported = trial.features.last().ok_or("the import made no feature")?;
    let record = trial.model().import_record(soliddraft_model::FeatureId(imported.id as u32));
    let declined = trial_result.is_err()
        && record.is_some_and(|r| {
            !r.changes.is_empty()
                && r.derivative.is_none()
                && r.changes.iter().all(|c| c.approval == Approval::Declined)
        });
    if !declined {
        trial_result?;
    }
    let declined_record = declined.then(|| record.cloned()).flatten();
    doc.reuse_builds_from(trial.model());
    match doc.apply(command) {
        Ok(Some(id)) => Ok(id),
        Err(error) => {
            let imported = doc.features.last().ok_or("the import made no feature")?;
            let record = doc.model().import_record(soliddraft_model::FeatureId(imported.id as u32));
            if declined && record == declined_record.as_ref() { Ok(imported.id) } else { Err(error) }
        }
        Ok(None) => Err("the import made no feature".into()),
    }
}

pub fn window(ctx: &egui::Context, state: &mut AppState) {
    let Some(preview) = state.import_dialog.take() else { return };
    let mut open = true;
    let mut choice = None;
    let mut cancelled = false;
    egui::Window::new("Import STEP").collapsible(false).open(&mut open).show(ctx, |ui| {
        ui.label(&preview.feature.source_name);
        ui.label("Review repairs before importing. The original file stays unchanged.");
        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
            for proposal in &preview.proposals.proposals {
                ui.group(|ui| {
                    ui.label(format!("{}: {}", proposal.id, proposal.kind));
                    ui.label(format!(
                        "{} to {} mm seam; tolerance {} mm",
                        proposal.distance_min_mm, proposal.distance_max_mm, proposal.tolerance_mm
                    ));
                    ui.label(match proposal.disposition {
                        Disposition::Auto => "automatic",
                        Disposition::Ask => "approval required",
                    });
                    ui.label(&proposal.reason);
                    ui.label(proposal.entities.join(", "));
                });
            }
            if preview.proposals.proposals.is_empty() {
                ui.label("No repairs proposed.");
            }
        });
        ui.horizontal(|ui| {
            let automatic = preview.proposals.proposals.iter().all(|p| p.disposition == Disposition::Auto);
            if ui.add_enabled(automatic, egui::Button::new("Accept")).clicked() {
                choice = Some(false);
            }
            if ui.add_enabled(!preview.proposals.proposals.is_empty(), egui::Button::new("Decline")).clicked() {
                choice = Some(true);
            }
            if ui.button("Cancel").clicked() {
                cancelled = true;
            }
        });
    });
    if let Some(decline) = choice {
        let ids = if decline { preview.proposals.proposals.iter().map(|p| p.id.clone()).collect() } else { Vec::new() };
        match commit(&mut state.doc, &preview, &ids) {
            Ok(_) => state.note(
                if decline { "Import saved with declined repairs" } else { "Imported STEP body" },
                NoteKind::Info,
            ),
            Err(error) => state.note(error, NoteKind::Error),
        }
    } else if open && !cancelled {
        state.import_dialog = Some(preview);
    }
}
