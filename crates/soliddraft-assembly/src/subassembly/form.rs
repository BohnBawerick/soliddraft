//! Preparing and committing a formation.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;
use soliddraft_document::resolve::{Location, Resolution};
use soliddraft_document::{ContentHash, DocKind, DocRef, DocumentId};

use crate::document::AssemblyDocument;
use crate::file::AssemblyPackage;
use crate::load::LoadError;
use crate::mates::bodies::Scope;
use crate::mates::{Mate, MateEntry};
use crate::occurrence::FlattenError;
use crate::path::{ComponentId, OccurrencePath, SourceId, Step};
use crate::schema::Assembly;
use crate::subassembly::{FormCode, FormRefusal, FormRequest, FormStatus, Migration, MovedMate, MovedOccurrence};

/// A formation ready to commit: both candidate truths, checked and solved, with what they
/// were prepared on. Committing checks that nothing it was prepared on has changed.
pub struct FormPlan {
    /// The root's revision it was prepared on.
    root_revision: ContentHash,
    /// Each selected source's reference, and the revision its file had then.
    sources: Vec<(DocRef, Option<ContentHash>)>,
    destination: PathBuf,
    child_bytes: Vec<u8>,
    candidate: AssemblyDocument,
    /// The components and sources the root's undo step restores.
    components: Vec<ComponentId>,
    changed_sources: Vec<SourceId>,
    migration: Migration,
    status: FormStatus,
}

impl FormPlan {
    /// Where every reference goes.
    pub fn migration(&self) -> &Migration {
        &self.migration
    }

    /// How the root and the new file solve.
    pub fn status(&self) -> &FormStatus {
        &self.status
    }

    /// The new file.
    pub fn destination(&self) -> &Path {
        &self.destination
    }

    /// The root's truth once formed.
    pub fn root(&self) -> &Assembly {
        self.candidate.assembly()
    }
}

/// A committed formation.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Formed {
    /// The new file.
    pub path: PathBuf,
    /// Where every reference went.
    pub migration: Migration,
    /// How the root and the new file solve.
    pub status: FormStatus,
}

fn invalid(message: impl Into<String>) -> FormRefusal {
    FormRefusal::new(FormCode::Invalid, message)
}

fn unsupported(message: impl Into<String>) -> FormRefusal {
    FormRefusal::new(FormCode::Unsupported, message)
}

fn load_refusal(e: LoadError) -> FormRefusal {
    match e {
        LoadError::Flatten(FlattenError::Cycle(at)) => {
            FormRefusal::new(FormCode::Cycle, format!("the assembly would contain itself through {at}"))
                .caused_by(at.to_string())
        }
        e => FormRefusal::new(FormCode::MissingReference, e.to_string()),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

impl AssemblyDocument {
    /// The revision `r`'s file has now and where it was found, or a refusal naming it
    /// when it cannot be found.
    fn resolved(&self, r: &DocRef) -> Result<(Option<ContentHash>, Option<PathBuf>), FormRefusal> {
        match self.resolver().resolve(r) {
            Resolution::Found(t) => {
                let file = match &t.location {
                    Location::File(p) => Some(p.clone()),
                    Location::Open(d) => d.path.clone(),
                };
                Ok((t.revision().cloned(), file))
            }
            Resolution::Missing(_) => Err(FormRefusal::new(
                FormCode::MissingReference,
                format!("{} cannot be found; the copy the assembly shows cannot stand in for it", r.path),
            )
            .caused_by(r.path.clone())),
        }
    }

    /// Prepares forming the parts `request` names into a new file: makes and checks both
    /// candidate truths, reads their structures and solves them. Changes nothing.
    pub fn prepare_form(&mut self, request: &FormRequest) -> Result<FormPlan, FormRefusal> {
        self.finish_loading();
        let root_path = self
            .path()
            .map(Path::to_path_buf)
            .ok_or_else(|| invalid("save the assembly first: the new file's references are relative to it"))?;
        let truth = self.assembly().clone();
        let selected = selection(&truth, request)?;
        let destination = soliddraft_document::kind::saved_path(&request.path, DocKind::Assembly);
        if DocKind::of_path(&request.path) != Some(DocKind::Assembly) {
            return Err(invalid("the new file's name must end in .dwasm"));
        }
        let taken = || {
            FormRefusal::new(FormCode::DestinationExists, format!("{} already exists", destination.display()))
                .caused_by(file_name(&destination))
        };
        if destination.exists() || std::path::absolute(&destination).ok() == std::path::absolute(&root_path).ok() {
            return Err(taken());
        }
        let chosen: BTreeSet<ComponentId> = selected.iter().copied().collect();
        let mut source_ids: Vec<SourceId> = Vec::new();
        for c in &selected {
            let source = truth.component(*c).expect("checked").source;
            if !source_ids.contains(&source) {
                source_ids.push(source);
            }
        }
        source_ids.sort();
        let mut sources = Vec::new();
        let mut found = Vec::new();
        for s in &source_ids {
            let r = truth.source(*s).expect("checked").document.clone();
            let (now, file) = self.resolved(&r)?;
            found.push(file.unwrap_or_else(|| r.stored_path(&root_path)));
            sources.push((r, now));
        }
        check_carried(self, &truth, &chosen, &request.paths)?;

        // The new file's frame and the parts' placements in it.
        let origin = Self::owner(&request.origin).map_err(|_| invalid("the origin is one of the selected parts"))?;
        let frame = truth.component(origin).expect("checked").placement;
        let child_id = DocumentId::new();
        let mut child =
            Assembly::new(destination.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
        // Each reference keeps its id, configuration and pin, and points where the root
        // found the document, which may not be where its stored path says.
        for (s, target) in source_ids.iter().zip(&found) {
            let mut source = truth.source(*s).expect("checked").clone();
            source.document.set_path(&destination, target);
            child.sources.push(source);
        }
        for c in &selected {
            let mut component = truth.component(*c).expect("checked").clone();
            component.placement = frame.inverse().compose(&component.placement);
            child.components.push(component);
        }

        // The root: the parts go, and the new file comes in their place under ids from the
        // root's own counter. Its revision is known once its mates are in.
        let mut root = truth.clone();
        root.components.retain(|c| !chosen.contains(&c.id));
        root.sources.retain(|s| root.components.iter().any(|c| c.source == s.id));
        let source = SourceId(root.take_id());
        let component = ComponentId(root.take_id());
        let holder = OccurrencePath::root().child(Step::component(component));

        // Mates: inside, across, or untouched.
        let mut moved_mates = Vec::new();
        let mut kept_mates = Vec::new();
        for entry in &truth.mates {
            let MateEntry::Known(mate) = entry else {
                return Err(unsupported("the assembly holds a mate this version cannot carry into a new file"));
            };
            let ends: Vec<OccurrencePath> = match mate {
                Mate::Origin { component, .. } => vec![OccurrencePath::root().child(Step::component(*component))],
                Mate::Distance { a, b, .. } => vec![a.path.clone(), b.path.clone()],
            };
            let inside =
                ends.iter().filter(|p| p.steps().first().is_some_and(|s| chosen.contains(&s.component))).count();
            let mut record = MovedMate {
                id: mate.id(),
                name: mate.name().to_owned(),
                owner_before: self.id(),
                owner_after: self.id(),
                endpoints_before: if matches!(mate, Mate::Origin { .. }) { Vec::new() } else { ends.clone() },
                endpoints_after: Vec::new(),
            };
            match (mate, inside) {
                (_, 0) => {
                    record.endpoints_after = record.endpoints_before.clone();
                    kept_mates.push(entry.clone());
                }
                (Mate::Origin { name, .. }, _) => {
                    return Err(unsupported(format!(
                        "{name} holds a selected part on the root's origin; it cannot move into a new file"
                    ))
                    .caused_by(name.clone()));
                }
                (Mate::Distance { name, distance, .. }, 2) => {
                    if !distance.references().is_empty() {
                        return Err(unsupported(format!(
                            "{name} reads {}, a value of the root; the new file could not read it",
                            distance.text()
                        ))
                        .caused_by(name.clone()));
                    }
                    record.owner_after = child_id;
                    record.endpoints_after = ends.clone();
                    child.mates.push(entry.clone());
                }
                (Mate::Distance { .. }, _) => {
                    let mut crossing = mate.clone();
                    if let Mate::Distance { a, b, .. } = &mut crossing {
                        for end in [&mut *a, &mut *b] {
                            if end.path.steps().first().is_some_and(|s| chosen.contains(&s.component)) {
                                end.path = OccurrencePath([holder.steps(), end.path.steps()].concat());
                            }
                        }
                        record.endpoints_after = vec![a.path.clone(), b.path.clone()];
                    }
                    kept_mates.push(MateEntry::Known(crossing));
                }
            }
            moved_mates.push(record);
        }
        child.next_id = child
            .sources
            .iter()
            .map(|s| s.id.0)
            .chain(child.components.iter().map(|c| c.id.0))
            .chain(child.known_mates().map(|m| m.id().0))
            .max()
            .unwrap_or(0)
            + 1;
        child.normalize();
        child.validate().map_err(|e| invalid(e.0))?;
        let child_revision = child.semantic_hash();

        let mut child_ref = DocRef::new(child_id, DocKind::Assembly, "", child_revision.clone());
        child_ref.set_path(&root_path, &destination);
        root.sources.push(crate::schema::Source { id: source, document: child_ref });
        root.components.push(crate::schema::Component::new(component, source, frame));
        root.mates = kept_mates;
        root.normalize();
        root.validate().map_err(|e| invalid(e.0))?;

        // Both structures, read and solved before anything is written.
        let copies: Vec<_> = self
            .copies()
            .into_iter()
            .filter(|c| child.sources.iter().any(|s| s.document.id == Some(c.document)))
            .collect();
        let child_bytes = child.to_bytes(child_id, None, &copies).map_err(|e| invalid(e.to_string()))?;
        let package = AssemblyPackage::from_bytes(child_bytes.clone()).map_err(|e| invalid(e.to_string()))?;
        let candidate = self
            .candidate(self.path().map(Path::to_path_buf), self.id(), root, vec![package.clone()])
            .map_err(load_refusal)?;
        let mut alone =
            self.candidate(Some(destination.clone()), child_id, child.clone(), vec![package]).map_err(load_refusal)?;
        let root_status = {
            let problem = self
                .gather(&Scope::new(candidate.assembly(), candidate.table(), candidate.store()))
                .map_err(|r| unsupported(r.to_string()))?;
            let solved = self.solve_problem(problem).map_err(|r| unsupported(r.to_string()))?;
            if let Some(body) = solved.placements.keys().next() {
                return Err(unsupported(format!("the root's mates would move {body} once formed")));
            }
            solved.status
        };
        let child_status = {
            std::mem::swap(&mut alone.proofs, &mut self.proofs);
            let solved = alone.solve_truth(&child);
            std::mem::swap(&mut alone.proofs, &mut self.proofs);
            let solved = solved.map_err(|r| unsupported(r.to_string()))?;
            if let Some(body) = solved.placements.keys().next() {
                return Err(unsupported(format!("the new file's mates would move {body}")));
            }
            solved.status
        };

        let occurrences = request
            .paths
            .iter()
            .map(|p| MovedOccurrence {
                from: p.clone(),
                to: OccurrencePath([holder.steps(), p.steps()].concat()),
                child: p.clone(),
            })
            .collect();
        let mut changed_sources = source_ids.clone();
        changed_sources.push(source);
        let mut components = selected.clone();
        components.push(component);
        Ok(FormPlan {
            root_revision: self.revision(),
            sources,
            destination,
            child_bytes,
            candidate,
            components,
            changed_sources,
            migration: Migration {
                root: self.id(),
                child: child_id,
                child_revision,
                component,
                source,
                sources: source_ids,
                occurrences,
                mates: moved_mates,
            },
            status: FormStatus { root: root_status, child: child_status },
        })
    }

    /// Commits a prepared formation: checks nothing it was prepared on has changed,
    /// publishes the new file without replacing any, then makes the formed root the
    /// accepted one in one undo step labelled "Form subassembly". The root's own file is
    /// not written until it is saved.
    pub fn commit_form(&mut self, plan: FormPlan) -> Result<Formed, FormRefusal> {
        self.finish_loading();
        let now = self.revision();
        if now != plan.root_revision {
            return Err(FormRefusal {
                expected: Some(plan.root_revision.to_string()),
                actual: Some(now.to_string()),
                ..FormRefusal::new(FormCode::StaleRevision, "the assembly changed after the formation was prepared")
                    .caused_by(self.id().to_string())
            });
        }
        for (r, then) in &plan.sources {
            let (current, _) = self.resolved(r)?;
            if current != *then {
                let show = |h: &Option<ContentHash>| h.as_ref().map(ToString::to_string).unwrap_or_default();
                return Err(FormRefusal {
                    expected: Some(show(then)),
                    actual: Some(show(&current)),
                    ..FormRefusal::new(
                        FormCode::StaleRevision,
                        format!("{} changed after the formation was prepared", r.path),
                    )
                    .caused_by(r.path.clone())
                });
            }
        }
        publish(&plan.destination, &plan.child_bytes)?;
        let before = self.structural_image(&plan.components, &plan.changed_sources);
        self.adopt("Form subassembly", plan.candidate, before);
        let open = self.options.open.clone();
        self.refresh_references(open);
        Ok(Formed { path: plan.destination, migration: plan.migration, status: plan.status })
    }

    /// Prepares and commits a formation.
    pub fn form_subassembly(&mut self, request: &FormRequest) -> Result<Formed, FormRefusal> {
        let plan = self.prepare_form(request)?;
        self.commit_form(plan)
    }
}

/// Writes the new file, never over another.
fn publish(path: &Path, bytes: &[u8]) -> Result<(), FormRefusal> {
    soliddraft_document::save_new(path, bytes).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            FormRefusal::new(FormCode::DestinationExists, format!("{} already exists", path.display()))
                .caused_by(file_name(path))
        } else {
            FormRefusal::new(FormCode::Io, format!("cannot write {}: {e}", path.display())).caused_by(file_name(path))
        }
    })
}

/// The selected components, in order, each a part placed directly in the root.
fn selection(truth: &Assembly, request: &FormRequest) -> Result<Vec<ComponentId>, FormRefusal> {
    if request.paths.is_empty() {
        return Err(invalid("select the parts to form into a sub-assembly"));
    }
    let mut out = Vec::new();
    for path in &request.paths {
        let c = match path.steps() {
            [step] if step.member.is_none() => step.component,
            _ => {
                return Err(
                    invalid(format!("{path} is not one of the root's own components")).caused_by(path.to_string())
                );
            }
        };
        if out.contains(&c) {
            return Err(invalid(format!("{path} is selected twice")).caused_by(path.to_string()));
        }
        let component = truth
            .component(c)
            .ok_or_else(|| invalid(format!("the assembly has no component {c}")).caused_by(path.to_string()))?;
        let source = truth.source(component.source).ok_or_else(|| invalid(format!("component {c} has no source")))?;
        if source.document.kind != DocKind::Part {
            return Err(unsupported(format!("{path} is a sub-assembly; this version forms parts placed in the root"))
                .caused_by(path.to_string()));
        }
        out.push(c);
    }
    if !request.paths.contains(&request.origin) {
        return Err(invalid("the origin is one of the selected parts").caused_by(request.origin.to_string()));
    }
    Ok(out)
}

/// Refuses what the new file could not carry whole: anything but default visibility,
/// colour and quality, patterns, overrides and configurations on the selected parts.
fn check_carried(
    doc: &AssemblyDocument,
    truth: &Assembly,
    chosen: &BTreeSet<ComponentId>,
    paths: &[OccurrencePath],
) -> Result<(), FormRefusal> {
    let refuse = |c: &ComponentId, what: &str| {
        Err(unsupported(format!("component {c} {what}; this version forms parts in their default state"))
            .caused_by(c.to_string()))
    };
    for c in chosen {
        let component = truth.component(*c).expect("checked");
        if component.hidden || component.suppressed {
            return refuse(c, "is hidden or suppressed");
        }
        let source = truth.source(component.source).expect("checked");
        if source.document.configuration.is_some() {
            return refuse(c, "places a configuration");
        }
        if truth.patterns.iter().any(|p| p.seeds.contains(c)) {
            return refuse(c, "seeds a pattern");
        }
        let under = |p: &OccurrencePath| p.steps().first().is_some_and(|s| s.component == *c);
        if truth.overrides.iter().any(|o| under(&o.path)) {
            return refuse(c, "has overrides");
        }
        if truth.appearances.iter().any(|a| under(&a.path)) {
            return refuse(c, "has a colour of the assembly's");
        }
    }
    for p in paths {
        if doc
            .settings()
            .overrides
            .iter()
            .any(|o| o.path.parse::<OccurrencePath>().is_ok_and(|q| q.steps().starts_with(p.steps())))
        {
            return Err(unsupported(format!(
                "{p} is shown at a quality of its own; this version forms parts in their default state"
            ))
            .caused_by(p.to_string()));
        }
    }
    Ok(())
}
