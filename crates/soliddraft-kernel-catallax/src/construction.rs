//! Early display of retained scalar edits, then exact proof with the ordinary build allowances.

use std::collections::BTreeMap;

use catallax_app::{ConstructionOptions, lifecycle::WorkAllocation};
use catallax_numbers::{Decider, OperationAllowance, Refusal, operation_budget_active};
use catallax_recipe::Operation;
use serde_json::json;
use soliddraft_kernel_api::{
    BodyId, ConstructionEdit, ConstructionFace, ConstructionMesh, FaceId, KernelError, KernelResult, OpKind, OpTag,
    ProofWork, ScalarEdit, ScalarParameter, TessellationParams,
};

use super::{CatallaxKernel, build_error, convert, converted_recipe, native, options, query_error};

/// Work for an early construction display, separate from the exact rebuild.
const CONSTRUCTION_WORK: u64 = 2_000_000;

enum ReservedProof {
    /// No caller allowance: each kernel build in the rebuild takes its own
    /// default allowance, exactly as the synchronous rebuild does.
    PerBuild,
    /// Work already taken from a caller-owned allowance. Nested builds share it.
    Caller(OperationAllowance),
}

impl ProofWork for ReservedProof {
    fn run(self: Box<Self>, rebuild: &mut dyn FnMut() -> KernelResult<()>) -> KernelResult<()> {
        let Self::Caller(allowance) = *self else {
            return rebuild();
        };
        let (answer, work) = allowance.run(rebuild);
        if work.exhausted {
            return Err(query_error(
                "edit model proof",
                Refusal::new("numbers.operation-budget", "the reserved exact-proof allowance was exhausted"),
            ));
        }
        answer
    }
}

/// Reserves `construction` units for the early display and the exact proof's work.
/// A caller-owned allowance pays for both from one existing default, so a nested
/// request never obtains fresh work.
fn reserve(construction: u64) -> KernelResult<(OperationAllowance, ReservedProof)> {
    let refused = |refusal| query_error("edit allocation", refusal);
    if !operation_budget_active() {
        return Ok((OperationAllowance::reserve(construction).map_err(refused)?, ReservedProof::PerBuild));
    }
    let allocation = WorkAllocation::application(construction).expect("constant split is within the application total");
    let mut display = OperationAllowance::reserve(catallax_app::DEFAULT_BUILD_WORK).map_err(refused)?;
    let proof = display.split_off(allocation.proof).expect("validated phase split");
    Ok((display, ReservedProof::Caller(proof)))
}

pub(super) fn reserve_proof_work() -> KernelResult<Box<dyn ProofWork>> {
    let (_, proof) = reserve(0)?;
    Ok(Box::new(proof))
}

pub(super) fn prepare(
    kernel: &CatallaxKernel,
    body: BodyId,
    edits: &[ScalarEdit],
    params: &TessellationParams,
) -> KernelResult<ConstructionEdit> {
    let (input, records, source) = {
        let state = kernel.state()?;
        let stored = state.get(body)?;
        let source = state.solid(body)?;
        let input = json!({
            "format":"soliddraft-recipe/1", "name":"SolidDraft",
            "steps":stored.steps.values().collect::<Vec<_>>(), "result":stored.result,
        });
        let records: BTreeMap<_, _> = stored.history.iter().map(|record| (record.seq, record.tag.clone())).collect();
        (input, records, source)
    };
    let (preview, proof) = reserve(CONSTRUCTION_WORK)?;
    let (mut display, work) = preview.run(|| {
        let op = match edits.first().map(|edit| edit.parameter) {
            Some(ScalarParameter::FilletRadius) => OpKind::Fillet,
            Some(ScalarParameter::ChamferDistance) => OpKind::Chamfer,
            _ => OpKind::Extrude,
        };
        let mut converted = converted_recipe(&input, op)?;
        for edit in edits {
            let mut matches = records.iter().filter(|(_, tag)| *tag == &edit.tag);
            let (&seq, _) = matches.next().ok_or_else(|| {
                KernelError::InvalidInput(format!("operation {} is not in the retained body's history", edit.tag))
            })?;
            if matches.next().is_some() {
                return Err(KernelError::InvalidInput(format!("operation tag {} is ambiguous in this body", edit.tag)));
            }
            let name = format!("op_{seq}");
            let id = converted.steps.get(&name).ok_or_else(|| {
                KernelError::InvalidInput(format!("operation {} has no retained definition", edit.tag))
            })?;
            let step = converted.recipe.steps.iter_mut().find(|step| &step.id == id).ok_or_else(|| {
                KernelError::InvalidInput(format!("operation {} has no converted definition", edit.tag))
            })?;
            let value = native::number(edit.value, "edited scalar")?;
            match (edit.parameter, &mut step.op) {
                (ScalarParameter::ExtrudeDistance, Operation::Extrude { distance, .. }) => *distance = value,
                (ScalarParameter::FilletRadius, Operation::Fillet { radius, .. }) => *radius = value,
                (ScalarParameter::ChamferDistance, Operation::Chamfer { distance, other_distance: None, .. }) => {
                    *distance = value;
                }
                _ => {
                    return Err(KernelError::Unsupported(format!(
                        "operation {} does not support this construction-preview scalar",
                        edit.tag
                    )));
                }
            }
        }
        let tags: BTreeMap<String, OpTag> = records
            .iter()
            .filter_map(|(seq, tag)| {
                let id = converted.steps.get(&format!("op_{seq}"))?;
                Some((id.as_str().to_owned(), tag.clone()))
            })
            .collect();
        let controls = options(params)?;
        let display = catallax_app::construction_preview(
            &converted.recipe,
            &ConstructionOptions {
                tolerance: controls.chord_tolerance,
                max_vertices: controls.max_vertices,
                ..Default::default()
            },
            &mut Decider::default(),
        )
        .map_err(|error| build_error(op, error))?;
        let mut ids = BTreeMap::new();
        let mut faces = Vec::with_capacity(display.origins.len());
        let mut state = kernel.state()?;
        for (&index, origin) in &display.origins {
            let named = convert::named_origin(origin, &[&source.ids], &tags).ok_or_else(|| {
                KernelError::Unsupported(format!("construction face {origin:?} has no selectable application origin"))
            })?;
            let id = FaceId::from_raw(state.id());
            ids.insert(index, id);
            faces.push(ConstructionFace { id, origins: vec![named] });
        }
        drop(state);
        let mut mesh = convert::triangles(
            &display.mesh.positions,
            display.mesh.triangles.iter().map(|triangle| (triangle.vertices, ids[&triangle.face.index()])),
        );
        mesh.unproved = true;
        Ok(ConstructionMesh { mesh, faces })
    });
    if work.exhausted {
        display = Err(query_error(
            "edit construction preview",
            Refusal::new("numbers.operation-budget", "the reserved construction-preview allowance was exhausted"),
        ));
    }
    Ok(ConstructionEdit { display, proof: Box::new(proof) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use catallax_numbers::with_operation_budget;
    use soliddraft_kernel_api::{Extrude, Kernel, Plane, Point2, Profile, contract};

    fn edit() -> ScalarEdit {
        ScalarEdit { tag: OpTag::new("block"), parameter: ScalarParameter::ExtrudeDistance, value: 20.0 }
    }

    #[test]
    fn changed_construction_and_exact_rebuild_share_one_reservation_and_face_origins() {
        let kernel = CatallaxKernel::new();
        let original = contract::block(&kernel);
        let original_topology = kernel.topology(original).unwrap();
        let (request, reserved) = with_operation_budget(catallax_app::DEFAULT_BUILD_WORK, || {
            kernel.construction_edit(original, &[edit()], &TessellationParams::default()).unwrap()
        });
        assert_eq!(reserved.used, catallax_app::DEFAULT_BUILD_WORK);
        assert!(!reserved.exhausted);
        let display = request.display.unwrap();
        let height = display.mesh.positions.iter().map(|point| point[2]).fold(f32::NEG_INFINITY, f32::max);
        assert_eq!(height, 20.0);
        assert!(display.faces.iter().all(|face| original_topology.face(face.id).is_none()));
        let mut rebuilt = None;
        request
            .proof
            .run(&mut || {
                let profile = kernel.create_body_from_profile(
                    &OpTag::new("block_sketch"),
                    &Profile::rectangle(Plane::XY, Point2::new(0.0, 0.0), 40.0, 30.0),
                )?;
                let result =
                    kernel.extrude(&OpTag::new("block"), profile.body, &Extrude { distance: 20.0, direction: None });
                kernel.release(profile.body)?;
                rebuilt = Some(result?.body);
                Ok(())
            })
            .unwrap();
        let rebuilt = rebuilt.unwrap();
        let proved = kernel.topology(rebuilt).unwrap();
        for face in &display.faces {
            assert!(
                proved.faces.iter().any(|exact| { face.origins.iter().any(|origin| exact.origins.contains(origin)) })
            );
        }
        assert_eq!(kernel.mass_properties(original).unwrap().volume, 12_000.0);
        assert_eq!(kernel.mass_properties(rebuilt).unwrap().volume, 24_000.0);
    }

    #[test]
    fn exact_proof_without_a_caller_allowance_gives_each_build_its_own_default() {
        let kernel = CatallaxKernel::new();
        let original = contract::block(&kernel);
        let request = kernel.construction_edit(original, &[edit()], &TessellationParams::default()).unwrap();
        assert!(request.display.is_ok());
        for proof in [request.proof, kernel.reserve_proof_work().unwrap()] {
            let mut scoped = None;
            proof
                .run(&mut || {
                    scoped = Some(catallax_numbers::operation_budget_active());
                    Ok(())
                })
                .unwrap();
            assert_eq!(scoped, Some(false), "a shared scope would make every build split one default");
        }
    }

    #[test]
    fn insufficient_caller_work_cannot_fall_back_to_a_fresh_edit_allowance() {
        let kernel = CatallaxKernel::new();
        let original = contract::block(&kernel);
        let (request, work) = with_operation_budget(catallax_app::DEFAULT_BUILD_WORK - 1, || {
            kernel.construction_edit(original, &[edit()], &TessellationParams::default())
        });
        assert!(matches!(request, Err(KernelError::QueryFailed { .. })));
        assert!(work.exhausted);
        assert_eq!(kernel.mass_properties(original).unwrap().volume, 12_000.0);
    }
}
