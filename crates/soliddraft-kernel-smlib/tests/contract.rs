//! The kernel trait's contract, checked on the SMLib backend.

use soliddraft_kernel_api::contract;
use soliddraft_kernel_smlib::SmlibKernel;

#[test]
fn operations_build_what_they_say() {
    contract::operations_build_what_they_say(&SmlibKernel::new());
}

#[test]
fn operations_leave_inputs_untouched() {
    contract::operations_leave_inputs_untouched(&SmlibKernel::new());
}

#[test]
fn failed_operations_return_errors() {
    contract::failed_operations_return_errors(&SmlibKernel::new());
}

#[test]
fn history_tracks_entities() {
    contract::history_tracks_entities(&SmlibKernel::new());
}

#[test]
fn tessellation_is_labelled() {
    contract::tessellation_is_labelled(&SmlibKernel::new());
}

#[test]
fn tessellation_controls_refine() {
    contract::tessellation_controls_refine(&SmlibKernel::new());
}

#[test]
fn tessellation_levels_match_single_calls() {
    contract::tessellation_levels_match_single_calls(&SmlibKernel::new());
}

#[test]
fn mass_properties_match_closed_form() {
    contract::mass_properties_match_closed_form(&SmlibKernel::new());
}

#[test]
fn concurrent_calls_are_serialised() {
    contract::concurrent_calls_are_serialised(&SmlibKernel::new());
}

#[test]
fn view_curves_are_tagged() {
    contract::view_curves_are_tagged(&SmlibKernel::new());
}

#[test]
fn view_curves_are_repeatable() {
    contract::view_curves_are_repeatable(&SmlibKernel::new());
}

#[test]
fn sheet_views_are_refused() {
    contract::sheet_views_are_refused(&SmlibKernel::new());
}

#[test]
fn box_section_is_a_closed_loop() {
    contract::box_section_is_a_closed_loop(&SmlibKernel::new());
}

#[test]
fn edge_curves_are_exact() {
    contract::edge_curves_are_exact(&SmlibKernel::new());
}

#[test]
fn silhouettes_outline_curved_faces() {
    contract::silhouettes_outline_curved_faces(&SmlibKernel::new());
}

#[test]
fn lines_and_arcs_are_lines_and_arcs() {
    contract::lines_and_arcs_are_lines_and_arcs(&SmlibKernel::new());
}

#[test]
fn view_curves_match_mesh_hlr() {
    contract::view_curves_match_mesh_hlr(&SmlibKernel::new());
}

#[test]
fn vertices_have_ids_that_survive() {
    contract::vertices_have_ids_that_survive(&SmlibKernel::new());
}

#[test]
fn edges_sample_along_their_curves() {
    contract::edges_sample_along_their_curves(&SmlibKernel::new());
}

#[test]
fn planar_faces_have_outward_normals() {
    contract::planar_faces_have_outward_normals(&SmlibKernel::new());
}

#[test]
fn created_faces_have_roles() {
    contract::created_faces_have_roles(&SmlibKernel::new());
}

#[test]
fn box_section_view_hatches_the_cut() {
    contract::box_section_view_hatches_the_cut(&SmlibKernel::new());
}

#[test]
fn bracket_section_view_has_three_regions() {
    contract::bracket_section_view_has_three_regions(&SmlibKernel::new());
}

#[test]
fn section_planes_off_the_body() {
    contract::section_planes_off_the_body(&SmlibKernel::new());
}
