# The Catallax application entry

SolidDraft runs only on Catallax. `soliddraft-kernel-catallax` implements the existing `soliddraft-kernel-api::Kernel` trait through Catallax's `catallax-app` entry. The application, CLI and tests use that implementation. Desktop and browser builds compile it in process with Rust. There is no kernel thread, C++ bridge, separate WebAssembly kernel or server fallback.

The adapter pins its Catallax revision in `Cargo.toml` and `CATALLAX_REVISION`. No Catallax source is copied here. See [the remaining gaps](needs-catallax.md) and [the roadmap](roadmap.md).

## Operations and body ownership

Profiles are deferred definitions rather than sheet geometry. Recipe replay reports their loop count, closure and tags without asking for sheet topology, mass or validity. Extrusion and revolution consume those definitions to build solids. Each solid owns an immutable Catallax application build. Bodies have arena handles; operations leave their inputs unchanged and callers release handles they no longer need.

Extrudes, cuts, booleans and rational-frame revolves use Catallax stage 1. Holes, patterns, mirrors, fillets and chamfers between planes and cylinders, and extrusions up to the next boundary or a planar face use Catallax stage 3 operations; drafts of planar faces and pull-parallel cylinders use the stage 4 operation. `soliddraft-recipe/1` cannot spell these, so the adapter stores them as native Catallax steps and names their faces and edges by Catallax origins. A refused construction remains an error with Catallax's stable refusal code. Shell waits for stage 3, other fillets for stage 4. Waiting ribbon commands are disabled and name their stages. Existing saved features rebuild with an error and pass the preceding body through unchanged.

## Display and drawings

Catallax supplies shared-edge display meshes with chord bounds, face handles and edge polylines. The adapter reserves part of the chord bound for conversion to float display positions and refuses a request that cannot survive that conversion. It forwards the surface angle, edge length and aspect ratio as Catallax controls. Catallax bounds facet normals rather than the turn along an edge, so a requested curve angle, and any control Catallax could not certify on a face, appears in `Mesh::unavailable_controls`.

Topology contains exact definitions and incidence with Catallax's proven measures: face areas, centroids and boxes, edge lengths and half-arc-length points, and the body's box. Mass properties include the proven centroidal inertia.

Drawings use polygon hidden-line removal on the meshes and Catallax booleans for plane sections. Whole lines and untrimmed circles keep their geometry after polygon visibility. Other curves retain polylines until conversion queries arrive. Drawing documents and SVG, PDF and DXF exports keep their formats.

## Export

`Kernel::export_step` writes a solid as STEP AP242 through Catallax's exporter, `catallax-step`, from the body's exact build. Its report gives Catallax's proven bound on the distance between the written and the exact geometry, rounded up to a decimal and to the next `f64`, with the counts of solids, faces, edges and vertices written and of fitted intersection curves. A bound over the requested tolerance is reported with Catallax's warning, not hidden. STL and 3MF come from `tessellate` meshes. See [part export](exchange.md).

## Interface work that follows removal

This change removes SMLib and its callers without changing the kernel trait's contract. Several parts still reflect SMLib's design and belong to the next roadmap step:

- `f64` geometry with a tolerance;
- history represented as tag propagation;
- `FaceRole::Other`;
- "modified" as a geometric-change guess;
- `validate()` as a guard against false successes.

The recipe format also stays unchanged. Faces carry Catallax's origins as `FaceInfo::origins`. The adapter maps each origin's recipe step back to the document's operation tag and leaves out roles the interface cannot express. Saved references resolve by those origins, not by history (see [modelling.md](modelling.md)). Legacy history records remain for drawings, the recipe runner and edge labels.

## Checks

Catallax acceptance tests exercise repeated recipe replay, input preservation, display labels, selection and edits. Waiting-feature tests check stage messages and unchanged bodies after failure and reopening. Catallax view, SVG and DXF goldens remain in `recipes/golden/`; SMLib-only goldens have been removed. Hosted CI builds and tests the full workspace on Linux, macOS and Windows, builds the browser module and drives the interactive Catallax bracket.
