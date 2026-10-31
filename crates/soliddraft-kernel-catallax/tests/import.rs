//! A1: the independent V9 contract on the real adapter and exact fixture bytes.

use soliddraft_kernel_api::{Decision, Disposition, ImportOptions, ImportStepRequest, Kernel, OpTag};
use soliddraft_kernel_catallax::CatallaxKernel;
use soliddraft_model::ImportStepFeature;

const BLOCK: &[u8] = include_bytes!("../../../ci/fixtures/import/block-seam-4um.step");
const TWO: &[u8] = include_bytes!("../../../ci/fixtures/import/two-blocks-5um.step");

#[test]
fn reviewed_import_round_trips_the_kernel_question_and_builds_one_exact_body() {
    let kernel = CatallaxKernel::new();
    let options = ImportOptions::default();
    let preview = kernel.propose_import(BLOCK, &options).unwrap();
    assert_eq!(preview.source_sha256, "1a602addaa4b534ef9427427d5231ba689a811a11159a5f9e7ebc17b3365c32b");
    assert_eq!(preview.proposals.len(), 1);
    let proposal = &preview.proposals[0];
    assert_eq!(proposal.id, "g0");
    assert_eq!(proposal.question, "repair:1a602addaa4b:g0");
    assert_eq!(proposal.kind, "seam_close");
    assert_eq!(proposal.distance_min_mm, "0.004");
    assert_eq!(proposal.distance_max_mm, "0.004");
    assert_eq!(proposal.tolerance_mm, "0.01");
    assert_eq!(proposal.disposition, Disposition::Auto);
    assert!(proposal.entities.iter().any(|e| e == "#176"));
    let feature = ImportStepFeature::new("block-seam-4um.step", BLOCK.to_vec()).declined(&proposal.question);
    let decisions = feature.kernel_decisions().unwrap();
    assert_eq!(decisions, [Decision::Decline { id: "g0".into() }]);
    let request = ImportStepRequest {
        name: &feature.source_name,
        source: &feature.source,
        sha256: &feature.sha256,
        options: &options,
        decisions: &decisions,
    };
    let declined = kernel.import_step(&OpTag::new("declined"), &request).unwrap();
    assert_eq!(declined.left_open, ["#178"]);
    assert!(declined.bodies.is_empty());
    let accepted =
        kernel.import_step(&OpTag::new("imported"), &ImportStepRequest { decisions: &[], ..request }).unwrap();
    assert!(accepted.left_open.is_empty());
    assert_eq!(accepted.bodies.len(), 1);
    let body = accepted.bodies[0].result.body;
    let topology = kernel.topology(body).unwrap();
    assert_eq!(topology.faces.len(), 6);
    assert_eq!(topology.edges.len(), 12);
    assert!((kernel.mass_properties(body).unwrap().volume - 10004.0).abs() <= 1e-6);
    kernel.release(body).unwrap();
    let two = ImportStepFeature::new("two-blocks-5um.step", TWO.to_vec());
    let refused = kernel
        .import_step(
            &OpTag::new("two"),
            &ImportStepRequest {
                name: &two.source_name,
                source: &two.source,
                sha256: &two.sha256,
                options: &options,
                decisions: &[],
            },
        )
        .unwrap_err();
    assert!(refused.to_string().contains("2 solids"), "{refused}");
    assert!(refused.to_string().contains("a part holds one body"), "{refused}");
}
