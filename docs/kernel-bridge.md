# The Catallax application entry

SolidDraft runs only on Catallax. `soliddraft-kernel-catallax` implements `soliddraft-kernel-api::Kernel` through Catallax's `catallax-app` entry. The application, CLI and tests use that implementation. Desktop and browser builds compile it in process with Rust. There is no C++ bridge, separate WebAssembly kernel or server fallback. Desktop edit construction and exact proof run on separate application workers; the browser currently runs both on its interface thread.

The adapter pins its Catallax revision in `Cargo.toml` and `CATALLAX_REVISION`. No Catallax source is copied here. See [the remaining gaps](needs-catallax.md) and [the roadmap](roadmap.md).

## Operations and body ownership

Profiles are deferred definitions rather than sheet geometry. Recipe replay reports their loop count, closure and tags without asking for sheet topology, mass or validity. Extrusion and revolution consume those definitions to build solids. Each solid owns an immutable Catallax application build. Bodies have arena handles; operations leave their inputs unchanged and callers release handles they no longer need.

Extrudes, cuts, booleans and rational-frame revolves use Catallax stage 1. Holes, patterns, mirrors, fillets and chamfers between planes and cylinders, and extrusions up to the next boundary or a planar face use Catallax stage 3 operations; drafts of planar faces and pull-parallel cylinders use the stage 4 operation. `soliddraft-recipe/1` cannot spell these, so the adapter stores them as native Catallax steps and names their faces and edges by Catallax origins. A refused construction remains an error with Catallax's stable refusal code. Shell waits for stage 3, other fillets for stage 4. Waiting ribbon commands are disabled and name their stages. Existing saved features rebuild with an error and pass the preceding body through unchanged.

## Display and drawings

Catallax supplies shared-edge display meshes with chord bounds, face handles and edge polylines. The adapter reserves part of the chord bound for conversion to float display positions and refuses a request that cannot survive that conversion. It forwards the surface angle, edge length and aspect ratio as Catallax controls. A maximum edge length is enforced on the returned float triangle edges and edge polylines, or the request is refused. Catallax bounds facet normals rather than the turn along an edge, so a requested curve angle, and any control Catallax could not certify on a face, appears in `Mesh::unavailable_controls`.

The screen does not use those certified meshes. A proved body's viewport and feature-editor preview call `Kernel::preview`, `preview_levels` and `preview_edges`, which the adapter answers with Catallax's preview mesh. It samples toward the same chord and angle targets, keeps face and edge handles, claims no bound and is marked `Mesh::unproved`. Drawing views on screen ask for it with `ViewSpec::preview`; their curves and stored `ViewGeometry` are then marked unproved. Display normals are averaged from the facets in `f64`.

Exports, the quality report and anything published as proven keep `tessellate`. The PDF, DXF and SVG drawing writers refuse unproved line work: the drawing tab exports a copy whose views are drawn again from the certified mesh, and `soliddraft draw` only ever projects from it. A backend without a preview answers the preview calls with `tessellate`.

A drawing context takes the current proved model body with its matching definition hash, never the viewport's retained fallback. During a dimension drag, pending proof or a failed edit, automatic following and Rebuild keep the last views and explain why they wait. Already-certified views deliberately pinned to an older revision can still be exported unchanged; unproved views cannot certify an old body under a newer definition hash.

Topology contains exact definitions and incidence with Catallax's proven measures: face areas, centroids and boxes, edge lengths and half-arc-length points, and the body's box. Mass properties include the proven centroidal inertia.

Drawings use polygon hidden-line removal on the meshes. A section view for the screen (`ViewSpec::preview`) is cut from the preview mesh with no boolean (`drawing::section::by_mesh_slice`): the crossings chain into the hatch regions and the cut faces hide what lies behind them. A certified section, as exports and `soliddraft draw` make, subtracts a half-space through a Catallax boolean (`by_half_space`). Whole lines and untrimmed circles keep their geometry after polygon visibility. Other curves retain polylines until conversion queries arrive. Each curve of a view is labelled accordingly, `exact` or `polygon`: `model_summary` counts the two per view, a drawing export reports the counts of the curves it wrote, and the SVG marks every view curve with `data-accuracy`. A drawing saved before the label reads its curves as `polygon`. Drawing documents and SVG, PDF and DXF exports keep their formats.

## Construction before exact proof

Scalar edits have a separate entry, `Kernel::construction_edit(body, edits, params)`. `ScalarEdit` identifies an existing operation by tag and changes only its extrusion distance, fillet radius or equal-distance chamfer distance. The adapter derives the requested recipe from the retained build and asks Catallax's checked field plan for a construction mesh without evaluating the new exact boundary first. Unsupported or unresolved construction remains a typed display refusal; it is not an approximate solid accepted as valid.

`ConstructionEdit` contains two distinct results: `display`, a `ConstructionMesh` or its refusal, and a single-use `ProofWork`. Construction faces have display-local ids and semantic origins, not reusable exact handles. The mesh is unproved and has no exportable body or certificate. On the desktop the construction worker can publish it while the exact worker is still occupied by an earlier request.

Construction reserves its own 2 million work units. Exact proof gives every kernel build in the rebuild the same per-build default allowance as the synchronous rebuild, so an edit never has less proof work than an ordinary rebuild. Inside a caller-owned allowance, one edit instead reserves 2 million units for construction and 18 million for proof from that allowance, and a refused reservation does not create fresh work. If the definition cannot use the scalar construction route, `reserve_proof_work` prepares the same proof work without a construction display. `ProofWork::run` wraps the model's ordinary rebuild callback; successful proof is adopted through the existing retained model builds, without evaluating it a second time.

The application keeps the requested document revision, displayed revision and last proved revision separate. Each scheduled edit also has a request token. Only a matching revision and request may publish success or failure. A committed request temporarily hidden by a transient drag retains its own receipt and allowance so cancellation can resume it. Undo and redo use the model's deferred command history, not another history stack.

Picking is bound to the displayed document, mesh version and body. A construction face can be selected by its origins, but exact references, edges, vertices and measurements wait for proof. Only an unambiguous origin match in that same request restores a selected exact face. Failure restores the whole last proved part, keeps the failed definition and reason in the red feature row and controls, and refuses part export. Pending definitions can be saved; their construction mesh or proof receipt is never serialized as a proved body.

The model summary exposes derived `edit_state`: `document`, `requested_revision`, `displayed_revision`, `proved_revision`, the corresponding `requested_edit`, `displayed_edit` and `proved_edit` request tokens, `mesh_version`, `display_ready`, `transient`, `pending` and `failed`. A transient drag reports its outstanding request even before a construction mesh arrives. Request tokens are scoped scheduling identities, not saved document revisions. `display_ready` means a CPU display is available; neither it nor `mesh_version` certifies that a GPU frame has been presented. `current_proved_model()` requires matching requested/proved identities with no pending, failed or transient input. `current_proved_display()` additionally requires that current revision's adopted display. Diagnostic summaries omit stale cached body values during a transient edit; they do not run a measurement to populate the state.

## Export

`Kernel::export_step` writes a solid as STEP AP242 through Catallax's exporter, `catallax-step`, from the body's exact build. Its report gives Catallax's proven bound on the distance between the written and the exact geometry, rounded up to a decimal and to the next `f64`, with the counts of solids, faces, edges and vertices written and of fitted intersection curves. A bound over the requested tolerance is reported with Catallax's warning, not hidden. STL and 3MF come from `tessellate` meshes. See [part export](exchange.md).

## Interface work that follows removal

The SMLib removal kept the existing operation and recipe interfaces. Construction previews now add the scoped edit entries above. Several parts still reflect SMLib's design and belong to the next roadmap step:

- `f64` geometry with a tolerance;
- history represented as tag propagation;
- `FaceRole::Other`;
- "modified" as a geometric-change guess;
- `validate()` as a guard against false successes.

The recipe format also stays unchanged. Faces carry Catallax's origins as `FaceInfo::origins`. The adapter maps each origin's recipe step back to the document's operation tag and leaves out roles the interface cannot express. Saved references resolve by those origins, not by history (see [modelling.md](modelling.md)). Legacy history records remain for drawings, the recipe runner and edge labels.

## Checks

Catallax acceptance tests exercise repeated recipe replay, input preservation, display labels, selection and edits. Waiting-feature tests check stage messages and unchanged bodies after failure and reopening. Catallax view, SVG and DXF goldens remain in `recipes/golden/`; SMLib-only goldens have been removed. Hosted CI builds and tests the full workspace on Linux, macOS and Windows, builds the browser module and drives the interactive Catallax bracket.
