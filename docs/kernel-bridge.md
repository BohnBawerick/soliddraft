# The Catallax application entry

SolidDraft runs only on Catallax. `soliddraft-kernel-catallax` implements the existing `soliddraft-kernel-api::Kernel` trait through Catallax's `catallax-app` entry. The application, CLI and tests use that implementation. Desktop and browser builds compile it in process with Rust. There is no kernel thread, C++ bridge, separate WebAssembly kernel or server fallback.

The adapter pins its Catallax revision in `Cargo.toml` and `CATALLAX_REVISION`. No Catallax source is copied here. See [the remaining gaps](needs-catallax.md) and [the roadmap](roadmap.md).

## Operations and body ownership

Profiles are deferred definitions rather than sheet geometry. Recipe replay reports their loop count, closure and tags without asking for sheet topology, mass or validity. Extrusion and revolution consume those definitions to build solids. Each solid owns an immutable Catallax application build. Bodies have arena handles; operations leave their inputs unchanged and callers release handles they no longer need.

Extrudes, cuts, booleans and rational-frame revolves use Catallax stage 1. A refused construction remains an error with Catallax's stable refusal code. Fillets between planes and cylinders wait for stage 3, other fillets for stage 4. Chamfers and shell wait for stage 3. Waiting ribbon commands are disabled and name their stages. Existing saved features rebuild with an error and pass the preceding body through unchanged.

## Display and drawings

Catallax supplies shared-edge display meshes with chord bounds, face handles and edge polylines. The adapter reserves part of the chord bound for conversion to float display positions and refuses a request that cannot survive that conversion. The adapter does not yet forward Catallax's angular and edge-length controls or convert its aspect-ratio report. It lists those requested controls in `Mesh::unavailable_controls`.

Topology contains exact definitions and incidence. Face centroids, edge lengths, midpoints and bounds still use the display mesh until the adapter wires Catallax's proven queries. `Topology::display_chord_height` identifies those estimates. They never decide geometry or operation success.

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

The recipe format also stays unchanged. The model still names faces by generators and follows origin-derived history when resolving saved references.

## Checks

Catallax acceptance tests exercise repeated recipe replay, input preservation, display labels, selection and edits. Waiting-feature tests check stage messages and unchanged bodies after failure and reopening. Catallax view, SVG and DXF goldens remain in `recipes/golden/`; SMLib-only goldens have been removed. Hosted CI builds and tests the full workspace on Linux, macOS and Windows, builds the browser module and drives the interactive Catallax bracket.
