//! Mate commands. Every one runs the same way: copy the truth, change the copy, prove and
//! solve it, and only then publish truth, pose and undo as one step. A refusal changes
//! nothing, and Update solves its candidate through the same [`AssemblyDocument::solve_truth`].

use std::collections::BTreeMap;

use num_bigint::BigInt;
use num_rational::BigRational;
use serde::Serialize;
use soliddraft_document::{ContentHash, DocRef, DocumentId};
use soliddraft_kernel_api::Certainty;
use soliddraft_model::{FaceRef, ValueTable};

use crate::document::{AssemblyDocument, EditError, key_of};
use crate::mates::bodies::{Gathered, Problem, Scope};
use crate::mates::reference::check_endpoint;
use crate::mates::solver::{self, Body, Constraint, FreeMotion, LocalPlane, SolverState, SolverStatus};
use crate::mates::{
    Alignment, EndpointSelector, FaceEndpoint, Mate, MateEntry, MateId, MateRefusal, PlaneProof, RefusalCode,
};
use crate::path::{ComponentId, OccurrencePath, Step};
use crate::placement::Placement;
use crate::schema::Assembly;
use crate::store::Freshness;

/// One change to a mate.
#[derive(Clone, Debug, PartialEq)]
pub enum MateEdit {
    /// A new literal distance.
    Distance(String),
    /// A new relation between the normals.
    Alignment(Alignment),
    /// Whether an Origin mate aligns the axes too.
    AlignAxes(bool),
}

/// What a solve found.
pub(crate) struct Solved {
    /// The parts it moved, in the root's frame.
    pub placements: BTreeMap<Body, Placement>,
    pub status: SolverStatus,
}

/// One side of a measurement.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProofIdentity {
    /// The occurrence.
    pub path: OccurrencePath,
    /// The part.
    pub document: DocumentId,
    /// The accepted revision the proof is on.
    pub revision: ContentHash,
    /// That revision's geometry hash.
    pub geometry: ContentHash,
    /// The face's label at that revision.
    pub label: String,
    /// True when the source has moved on, so this proof is of the accepted revision only.
    pub retained: bool,
}

/// The signed distance between two parallel placed planes, from `a` along its outward
/// normal, on the placements in the assembly's root frame.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PlaneMeasurement {
    /// What is measured.
    pub quantity: &'static str,
    /// The unit.
    pub unit: &'static str,
    /// The frame.
    pub frame: &'static str,
    /// The first face.
    pub a: ProofIdentity,
    /// The second face.
    pub b: ProofIdentity,
    /// The placements it was measured on.
    pub placements: [Placement; 2],
    /// Exact or bounded.
    pub certainty: Certainty,
    /// The nearest double.
    pub value: f64,
    /// Lower bound, rounded outward.
    pub lo: f64,
    /// Upper bound, rounded outward.
    pub hi: f64,
    /// The exact value as a fraction.
    pub rational: String,
}

pub(crate) fn value_refusal(what: &str, message: String) -> MateRefusal {
    let code = match message.as_str() {
        "value cycle" | "value graph is too deep" => RefusalCode::ValueCycle,
        "incompatible units in sum" | "the input needs a length" => RefusalCode::UnitMismatch,
        _ => RefusalCode::InvalidValue,
    };
    MateRefusal::new(code, format!("{what}: {message}"))
}

fn component_path(c: ComponentId) -> OccurrencePath {
    OccurrencePath::root().child(Step::component(c))
}

fn mate_named<'a>(truth: &'a mut Assembly, name: &str) -> Result<&'a mut Mate, MateRefusal> {
    truth
        .mates
        .iter_mut()
        .find_map(|m| match m {
            MateEntry::Known(m) if m.name() == name => Some(m),
            _ => None,
        })
        .ok_or_else(|| MateRefusal::invalid(format!("no mate named {name}")))
}

fn check_new_name(truth: &Assembly, name: &str) -> Result<(), MateRefusal> {
    if name.trim().is_empty() {
        return Err(MateRefusal::invalid("a mate needs a name"));
    }
    if truth.known_mates().any(|m| m.name() == name) {
        return Err(MateRefusal::invalid(format!("the name {name} is already taken")));
    }
    Ok(())
}

/// A length with no reference to any value.
fn literal(text: &str) -> Result<soliddraft_model::Expression, MateRefusal> {
    let none = ValueTable::default();
    let expression = none.parse(text).map_err(|e| value_refusal("the distance", e))?;
    none.length(&expression).map_err(|e| value_refusal("the distance", e))?;
    Ok(expression)
}

impl AssemblyDocument {
    /// Runs `edit` on a copy of the truth, proves and solves the copy, and publishes it
    /// as one undo step. With `strict` a candidate that leaves any freedom is refused.
    fn transact<T>(
        &mut self,
        label: &str,
        strict: bool,
        edit: impl FnOnce(&mut AssemblyDocument, &mut Assembly) -> Result<T, MateRefusal>,
    ) -> Result<T, MateRefusal> {
        self.finish_loading();
        let mut candidate = self.assembly.clone();
        let out = edit(self, &mut candidate)?;
        let solved = self.solve_truth(&candidate)?;
        if strict && solved.status.remaining_dof > 0 {
            let free: Vec<String> = solved.status.free.iter().map(solver::describe).collect();
            return Err(MateRefusal {
                remaining_dof: Some(solved.status.remaining_dof),
                free: solved.status.free,
                ..MateRefusal::new(
                    RefusalCode::UnderConstrained,
                    format!("{} freedoms would remain: {}", solved.status.remaining_dof, free.join(", ")),
                )
            });
        }
        let kept = crate::mates::bodies::locals(&Scope::new(&candidate, &self.table, &self.store), &solved.placements);
        crate::mates::bodies::keep(&mut candidate, kept);
        if candidate != self.assembly {
            self.publish(label, candidate);
        }
        Ok(out)
    }

    /// Makes `candidate`, which differs in mates, values, placements and the overrides
    /// that place parts inside sub-assemblies only, the truth.
    fn publish(&mut self, label: &str, candidate: Assembly) {
        let moved: Vec<(ComponentId, Placement)> = candidate
            .components
            .iter()
            .filter(|c| self.assembly.component(c.id).is_none_or(|old| old.placement != c.placement))
            .map(|c| (c.id, c.placement))
            .collect();
        let ids: Vec<ComponentId> = moved.iter().map(|(c, _)| *c).collect();
        let mut overrides: Vec<OccurrencePath> = self
            .assembly
            .overrides
            .iter()
            .chain(&candidate.overrides)
            .map(|o| o.path.clone())
            .filter(|p| self.assembly.override_at(p) != candidate.override_at(p))
            .collect();
        overrides.sort();
        overrides.dedup();
        let rows: Vec<_> = ids
            .iter()
            .map(|c| component_path(*c))
            .chain(overrides.iter().cloned())
            .filter_map(|p| self.table.find(&p))
            .collect();
        let rows_before = rows.iter().map(|r| self.row_state(*r)).collect();
        let mut before = self.image(&ids, &overrides);
        before.design = Some(crate::document::Design::of(&self.assembly));
        crate::document::Design::of(&candidate).restore(&mut self.assembly);
        self.assembly.next_id = candidate.next_id;
        for (id, placement) in &moved {
            if let Some(c) = self.assembly.component_mut(*id) {
                c.placement = *placement;
            }
        }
        for p in &overrides {
            self.assembly.overrides.retain(|o| o.path != *p);
            self.assembly.overrides.extend(candidate.override_at(p).cloned());
        }
        self.assembly.overrides.sort_by(|a, b| a.path.cmp(&b.path));
        let mut placed: Vec<(OccurrencePath, Placement)> =
            moved.iter().map(|(id, placement)| (component_path(*id), *placement)).collect();
        let scope = Scope::new(&self.assembly, &self.table, &self.store);
        placed.extend(overrides.iter().filter_map(|p| Some((p.clone(), scope.local(p)?))));
        for (path, placement) in &placed {
            if let Some(row) = self.table.find(path) {
                let shifted = self.table.set_local(row, *placement, &self.store);
                self.bvh_moved.extend(shifted.iter().copied());
                self.changes.moved.extend(shifted);
            }
        }
        self.record(label, before, rows_before);
        // A pattern's members follow their seed, and the table holds them as rows of their own.
        if self.assembly.patterns.iter().any(|p| p.seeds.iter().any(|s| ids.contains(s))) {
            self.reflatten();
        }
    }

    /// Proves every face `truth`'s mates name at the revisions it pins, and solves it.
    /// `truth` has the structure the document shows; its sub-assemblies bring their own
    /// mates.
    pub(crate) fn solve_truth(&mut self, truth: &Assembly) -> Result<Solved, MateRefusal> {
        let problem = self.gather(&Scope::new(truth, &self.table, &self.store))?;
        self.solve_problem(problem)
    }

    /// Proves the faces `problem` reads and solves it.
    pub(crate) fn solve_problem(&mut self, problem: Problem) -> Result<Solved, MateRefusal> {
        let mut constraints = Vec::new();
        let mut dangling: Vec<(String, String)> = Vec::new();
        for mate in problem.mates {
            match mate {
                Gathered::Origin { mate, body, axes, at } => {
                    constraints.push(Constraint::Origin { mate, body, axes, at })
                }
                Gathered::Distance { mate, ends, sign, distance } => {
                    let mut planes = Vec::new();
                    for end in &ends {
                        let source = self.rebased(end);
                        match self.prove(&source, &end.face) {
                            Ok(proof) => planes.push((end.body.clone(), local_plane(&mate, &proof)?)),
                            Err(r) if r.code == RefusalCode::DanglingReference => {
                                let label = &end.face.label;
                                let feature = end.face.feature;
                                let at = format!(
                                    "{}: occurrence {}, {label} of feature {feature}: {}",
                                    end.source.path, end.body, r.message
                                );
                                dangling.push((mate.clone(), at));
                            }
                            Err(r) => return Err(MateRefusal { mates: vec![mate.clone()], ..r }),
                        }
                    }
                    if let [(a, plane_a), (b, plane_b)] = &planes[..] {
                        constraints.push(Constraint::Distance {
                            mate,
                            a: a.clone(),
                            b: b.clone(),
                            plane_a: plane_a.clone(),
                            plane_b: plane_b.clone(),
                            sign,
                            distance,
                        });
                    }
                }
            }
        }
        if !dangling.is_empty() {
            let mut mates: Vec<String> = dangling.iter().map(|(m, _)| m.clone()).collect();
            mates.dedup();
            let list: Vec<&str> = dangling.iter().map(|(_, at)| at.as_str()).collect();
            let message = format!("{}; mates {}", list.join("; "), mates.join(", "));
            return Err(MateRefusal { mates, ..MateRefusal::new(RefusalCode::DanglingReference, message) });
        }
        let placements = solver::solve(&problem.bodies, &constraints)?;
        let (dof, free) = solver::freedoms(&placements, &constraints)?;
        let unmated = problem.parts.saturating_sub(problem.bodies.len());
        let moved = placements.into_iter().filter(|(b, p)| problem.bodies[b] != *p).collect();
        Ok(Solved { placements: moved, status: status_of(dof + 6 * unmated, free) })
    }

    /// The accepted assembly's status, from its definitions: reads no part and proves nothing.
    pub fn solver(&mut self) -> SolverStatus {
        if let Some(s) = &self.solver_status {
            return s.clone();
        }
        let status = self.read_status().unwrap_or_else(SolverStatus::error);
        self.solver_status = Some(status.clone());
        status
    }

    fn read_status(&self) -> Result<SolverStatus, String> {
        let problem = self.gather(&Scope::new(&self.assembly, &self.table, &self.store)).map_err(|r| r.to_string())?;
        let stored = |face: &soliddraft_model::FaceRef| {
            let normal = face.fingerprint.normal.ok_or("a mate's face has no stored normal")?;
            let n = [normal.x, normal.y, normal.z].map(|c| c.round() as i8);
            Ok::<_, String>(LocalPlane { normal: n, offset: num_traits::Zero::zero() })
        };
        let mut constraints = Vec::new();
        for mate in problem.mates {
            constraints.push(match mate {
                Gathered::Origin { mate, body, axes, at } => Constraint::Origin { mate, body, axes, at },
                Gathered::Distance { mate, ends: [a, b], sign, .. } => Constraint::Distance {
                    mate,
                    plane_a: stored(&a.face)?,
                    plane_b: stored(&b.face)?,
                    a: a.body,
                    b: b.body,
                    sign,
                    distance: 0.0,
                },
            });
        }
        let (dof, free) = solver::freedoms(&problem.bodies, &constraints).map_err(|r| r.to_string())?;
        Ok(status_of(dof + 6 * problem.parts.saturating_sub(problem.bodies.len()), free))
    }

    /// How many parts were rebuilt so far to prove planes.
    pub fn parts_rebuilt_for_proofs(&self) -> usize {
        self.proofs.built
    }

    fn endpoint(&mut self, truth: &Assembly, selector: &EndpointSelector) -> Result<FaceEndpoint, MateRefusal> {
        let source = Self::part_source(truth, selector.component)?;
        let face: FaceRef = self.select(&source, &selector.face)?;
        let document = source.id.ok_or_else(|| MateRefusal::invalid("the source has no document id"))?;
        Ok(FaceEndpoint {
            path: component_path(selector.component),
            document,
            configuration: source.configuration,
            face,
        })
    }

    fn identity(
        &self,
        path: &OccurrencePath,
        source: &DocRef,
        proof: &PlaneProof,
    ) -> Result<ProofIdentity, MateRefusal> {
        let retained = key_of(source)
            .and_then(|k| self.store.find(&k))
            .is_some_and(|d| !matches!(self.store.get(d).freshness, Freshness::Current | Freshness::Unchecked));
        Ok(ProofIdentity {
            path: path.clone(),
            document: proof.document,
            revision: proof.revision.clone(),
            geometry: proof.geometry.clone(),
            label: proof.label.clone(),
            retained,
        })
    }

    fn measure(&mut self, a: &FaceEndpoint, b: &FaceEndpoint) -> Result<PlaneMeasurement, MateRefusal> {
        check_endpoint(a)?;
        check_endpoint(b)?;
        let (ea, eb, placements) = {
            let scope = Scope::new(&self.assembly, &self.table, &self.store);
            let world = |e: &FaceEndpoint| {
                scope.world(&e.path).ok_or_else(|| MateRefusal::invalid(format!("no occurrence at {}", e.path)))
            };
            let placements = [world(a)?, world(b)?];
            (self.end_of(&scope, a.path.clone(), a, "")?, self.end_of(&scope, b.path.clone(), b, "")?, placements)
        };
        let (ra, rb) = (self.rebased(&ea), self.rebased(&eb));
        let (pa, pb) = (self.prove(&ra, &a.face)?, self.prove(&rb, &b.face)?);
        let gap = signed_gap(&pa, &pb, &placements)?;
        let value = solver::nearest(&gap);
        let (mut lo, mut hi) = (value, value);
        if solver::exact(lo).is_some_and(|r| r > gap) {
            lo = lo.next_down();
        }
        if solver::exact(hi).is_some_and(|r| r < gap) {
            hi = hi.next_up();
        }
        Ok(PlaneMeasurement {
            quantity: "signed_support_plane_distance",
            unit: "mm",
            frame: "root",
            a: self.identity(&a.path, &ra, &pa)?,
            b: self.identity(&b.path, &rb, &pb)?,
            placements,
            certainty: Certainty::Exact,
            value,
            lo,
            hi,
            rational: solver::text(&gap),
        })
    }

    /// Measures the signed distance between two faces' planes. Changes no truth.
    pub fn measure_planes(
        &mut self,
        a: &EndpointSelector,
        b: &EndpointSelector,
    ) -> Result<PlaneMeasurement, MateRefusal> {
        self.finish_loading();
        let truth = self.assembly.clone();
        let (a, b) = (self.endpoint(&truth, a)?, self.endpoint(&truth, b)?);
        self.measure(&a, &b)
    }

    /// What each distance mate measures on the accepted placements, by proof. A mate
    /// whose proof cannot be had says why; the accepted pose stands.
    pub fn mate_measurements(&mut self) -> Vec<(MateId, Result<PlaneMeasurement, MateRefusal>)> {
        let mates: Vec<Mate> = self.assembly.known_mates().cloned().collect();
        mates
            .iter()
            .filter_map(|m| match m {
                Mate::Distance { id, a, b, .. } => Some((*id, self.measure(a, b))),
                Mate::Origin { .. } => None,
            })
            .collect()
    }

    /// Adds an Origin mate on a component. One undo step.
    pub fn mate_origin(
        &mut self,
        name: &str,
        component: ComponentId,
        align_axes: bool,
        strict: bool,
    ) -> Result<MateId, MateRefusal> {
        self.transact("Origin mate", strict, |_, truth| {
            check_new_name(truth, name)?;
            if truth.component(component).is_none() {
                return Err(MateRefusal::invalid(format!("the assembly has no component {component}")));
            }
            let id = MateId(truth.take_id());
            truth.mates.push(MateEntry::Known(Mate::Origin { id, name: name.into(), component, align_axes }));
            Ok(id)
        })
    }

    /// Adds a distance mate between two faces. One undo step. Partial mates are accepted
    /// unless `strict` asks for a fully constrained result.
    pub fn mate_distance(
        &mut self,
        name: &str,
        a: &EndpointSelector,
        b: &EndpointSelector,
        distance: &str,
        alignment: Alignment,
        strict: bool,
    ) -> Result<MateId, MateRefusal> {
        self.transact("Distance mate", strict, |doc, truth| {
            check_new_name(truth, name)?;
            if a.component == b.component {
                return Err(MateRefusal::invalid("a mate ties two different components"));
            }
            let distance = literal(distance)?;
            let (a, b) = (doc.endpoint(truth, a)?, doc.endpoint(truth, b)?);
            let id = MateId(truth.take_id());
            truth.mates.push(MateEntry::Known(Mate::Distance { id, name: name.into(), a, b, alignment, distance }));
            Ok(id)
        })
    }

    /// Changes mates together, as one candidate and one undo step.
    pub fn edit_mates(&mut self, edits: &[(String, MateEdit)]) -> Result<(), MateRefusal> {
        self.transact("Edit mate", false, |_, truth| {
            for (name, edit) in edits {
                match (mate_named(truth, name)?, edit) {
                    (Mate::Distance { distance, .. }, MateEdit::Distance(_)) if !distance.references().is_empty() => {
                        let driver = distance.text().to_owned();
                        return Err(MateRefusal {
                            mates: vec![name.clone()],
                            ..MateRefusal::new(
                                RefusalCode::DrivenInput,
                                format!("{name} is driven by {driver}; edit {driver} instead"),
                            )
                        });
                    }
                    (Mate::Distance { distance, .. }, MateEdit::Distance(text)) => *distance = literal(text)?,
                    (Mate::Distance { alignment, .. }, MateEdit::Alignment(a)) => *alignment = *a,
                    (Mate::Origin { align_axes, .. }, MateEdit::AlignAxes(on)) => *align_axes = *on,
                    (_, _) => return Err(MateRefusal::invalid(format!("{name} has no such setting"))),
                }
            }
            Ok(())
        })
    }

    /// Removes a mate. One undo step; the components stay where they are.
    pub fn delete_mate(&mut self, name: &str) -> Result<(), MateRefusal> {
        self.transact("Delete mate", false, |_, truth| {
            let id = mate_named(truth, name)?.id();
            truth.mates.retain(|m| m.known().is_none_or(|m| m.id() != id));
            Ok(())
        })
    }

    /// Adds or changes an assembly value, solving everything it drives in the same step.
    pub fn set_value(&mut self, name: &str, expression: &str) -> Result<(), MateRefusal> {
        self.transact("Set value", false, |_, truth| {
            let id = truth.next_id;
            let mut taken = false;
            truth
                .values
                .set(name, expression, || {
                    taken = true;
                    id
                })
                .map_err(|e| value_refusal(name, e))?;
            if taken {
                truth.next_id += 1;
            }
            Ok(())
        })
    }

    /// Renames a value. What reads it holds its id, so only its label changes.
    pub fn rename_value(&mut self, name: &str, to: &str) -> Result<(), MateRefusal> {
        self.transact("Rename value", false, |_, truth| {
            let Assembly { values, mates, .. } = truth;
            let expressions = mates.iter_mut().filter_map(|m| match m {
                MateEntry::Known(Mate::Distance { distance, .. }) => Some(distance),
                _ => None,
            });
            values.rename(name, to, expressions).map_err(|e| value_refusal(name, e))
        })
    }

    /// Binds a mate's distance to an expression over assembly values. A distance has one
    /// driver: `replace` swaps it, and without it a second driver is refused.
    pub fn drive_mate(&mut self, mate: &str, expression: &str, replace: bool) -> Result<(), MateRefusal> {
        self.transact("Drive mate", false, |_, truth| {
            let driver = truth.values.parse(expression).map_err(|e| value_refusal(mate, e))?;
            if driver.references().is_empty() {
                return Err(MateRefusal::new(
                    RefusalCode::InvalidValue,
                    format!("{mate}: a driver names a value; a literal distance is an edit"),
                ));
            }
            let Mate::Distance { distance, .. } = mate_named(truth, mate)? else {
                return Err(MateRefusal::invalid(format!("{mate} has no distance to drive")));
            };
            if !distance.references().is_empty() && !replace {
                return Err(MateRefusal {
                    mates: vec![mate.to_owned()],
                    ..MateRefusal::new(
                        RefusalCode::AlreadyDriven,
                        format!("{mate} is driven by {}; a distance has one driver", distance.text()),
                    )
                });
            }
            *distance = driver;
            Ok(())
        })
    }

    /// Moves mated components: each move proposes a pose, and the mates decide what of
    /// it stands. Refused when they leave nothing of it.
    pub(crate) fn move_mated(&mut self, moves: &[(OccurrencePath, Placement)]) -> Result<(), EditError> {
        let before = self.assembly.clone();
        self.transact("Move", false, |_, truth| {
            for (path, placement) in moves {
                match Self::owner(path) {
                    Ok(c) => {
                        if let Some(component) = truth.component_mut(c) {
                            component.placement = *placement;
                        }
                    }
                    Err(p) => truth.override_mut(&p).placement = Some(*placement),
                }
            }
            Ok(())
        })
        .map_err(|r| EditError::Invalid(r.to_string()))?;
        if self.assembly == before {
            return Err(EditError::Invalid("the mates hold this component where it is".into()));
        }
        Ok(())
    }
}

fn status_of(remaining: usize, free: Vec<FreeMotion>) -> SolverStatus {
    let state = if remaining == 0 { SolverState::Solved } else { SolverState::Underconstrained };
    SolverStatus { state, remaining_dof: remaining, free, diagnostics: Vec::new() }
}

fn local_plane(mate: &str, proof: &PlaneProof) -> Result<LocalPlane, MateRefusal> {
    let offset = solver::rational(&proof.plane.offset).ok_or_else(|| {
        MateRefusal::new(RefusalCode::UnsupportedPlane, format!("{mate}: the plane has no exact offset"))
    })?;
    Ok(LocalPlane { normal: proof.plane.normal(), offset })
}

fn signed_gap(a: &PlaneProof, b: &PlaneProof, placements: &[Placement; 2]) -> Result<BigRational, MateRefusal> {
    let off = |_| MateRefusal::new(RefusalCode::UnsupportedPlacement, "a face is placed off the world axes");
    let normal = |p: &PlaneProof, at: &Placement| {
        let n = p.plane.normal().map(f64::from);
        let v = at.rotate(glam::DVec3::from_array(n));
        let rounded = v.to_array().map(f64::round);
        (v.distance(glam::DVec3::from_array(rounded)) <= 1e-12 && rounded.iter().map(|c| c.abs()).sum::<f64>() == 1.0)
            .then_some(rounded)
            .ok_or(())
    };
    let (na, nb) = (normal(a, &placements[0]).map_err(off)?, normal(b, &placements[1]).map_err(off)?);
    let axis = na.iter().position(|c| *c != 0.0).expect("a unit axis");
    if nb[axis] == 0.0 {
        return Err(MateRefusal::new(RefusalCode::NotParallel, "the two faces' planes are not parallel"));
    }
    let whole = |v: f64| BigRational::from_integer(BigInt::from(v as i64));
    let side = whole(na[axis]);
    let relation = whole(if na[axis] == nb[axis] { 1.0 } else { -1.0 });
    let exact = |v: f64| solver::exact(v).ok_or_else(|| MateRefusal::invalid("a placement is not finite"));
    let (ta, tb) = (exact(placements[0].translation[axis])?, exact(placements[1].translation[axis])?);
    let offset = |p: &PlaneProof| {
        solver::rational(&p.plane.offset)
            .ok_or_else(|| MateRefusal::new(RefusalCode::UnsupportedPlane, "the plane has no exact offset"))
    };
    Ok(side * (tb - ta) + relation * offset(b)? - offset(a)?)
}
