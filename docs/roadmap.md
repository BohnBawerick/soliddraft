# SolidDraft roadmap

This is a list of planned work in dependency order, not a release schedule. Progress is recorded by completed work, never by dates.

The companion document is [Catallax's roadmap](https://github.com/BohnBawerick/catallax/blob/main/docs/PLAN.md). SolidDraft and Catallax are one project in two repositories: SolidDraft is the application, Catallax is its kernel. This file orders the application work; Catallax's roadmap orders the kernel work. Where an item here needs a kernel item, it names the Catallax stage.

## 1. Premise

SolidDraft runs only on Catallax. Catallax is designed for the features SolidDraft's users drive, and SolidDraft grows step by step toward the most usable parts of SolidWorks and nTopology: parametric parts, drawings and assemblies on one side; implicit modelling, lattices, field-driven thickness, topology optimisation and simulation-led shapes on the other. The most usable features come first.

SolidDraft runs on Catallax. The former SMLib C++ kernel, its toolchain and its separate WebAssembly module have been removed. It is not kept as a fallback. A kernel kept in reserve is the half-commitment that Hanno Braun's Fornjot retrospective names as its project's biggest mistake, and keeping it would also keep the C++ toolchain, the second WebAssembly module and an interface shaped around a kernel we are not building.

## 2. What removing SMLib means for users

Features the application has today only through SMLib wait for Catallax to land them. Until then:

- the ribbon shows the command disabled, with a tooltip naming the kernel stage it arrives with, following the existing rule that unfinished commands stay visible and say why;
- an existing part that uses such a feature rebuilds up to it, marks it with a rebuild error, and passes the body through unchanged, which is how the application already treats a feature the kernel cannot build;
- the works-today list in README.md says which features wait, and for which stage.

On the day SMLib goes, sketches, extrudes, cuts, revolves and booleans keep working on Catallax. Fillets between planes and cylinders and chamfers arrive with Catallax stage 3. Other fillets wait for stage 4. Drawings keep working through the polygon-mode hidden-line pass (section 7). STEP and mesh export arrive at the same time, because Catallax already writes STEP AP242 with a proven deviation and shared-edge meshes with proven chord bounds.

## 3. The kernel boundary

The application keeps one boundary between itself and the kernel. Undo, rebuild and caching stay on the application side. What goes is the promise that any kernel can sit behind it.

1. Implement a Catallax backend behind the existing `soliddraft_kernel_api::Kernel` trait, so the application runs unchanged.
2. Remove the SMLib backend and every part of the trait that exists only because of SMLib: `f64` values with a tolerance, history as tag propagation, `FaceRole::Other`, "modified" as a geometric-change guess, and `validate()` as a guard against false successes.
3. Widen the trait toward Catallax's shapes one method at a time: exact rationals accepted, recipe origins as the face identity, refusals as typed outcomes with stable codes.
4. Rename the trait as Catallax's application interface and delete the backend indirection. The trait's contract checks become the application's acceptance tests for Catallax.

Catallax keeps no global state, so the dedicated kernel thread and the plan to move the kernel into a child process both go. A document holds its own model. Safe background threads for rebuilds come from Catallax's own plan.

Rebuild stops replaying the whole feature list on every change. Catallax keeps every intermediate body valid, so a rebuild starts at the first changed feature and reuses the bodies before it.

A rebuild also skips work that is not geometry. Renaming a feature or a body recomputes nothing. Restoring an earlier parameter value reuses the bodies built for it. Display meshes of bodies that did not change stay in place. Cache keys come from exact parameter values, recorded settings and body versions, never from rounded coordinates. A coordinate grid and Catallax's recorded sameness decisions mean different things. Caches are budgeted by the bytes they keep. Tests in the development build count the features recomputed, the meshes rebuilt and the bodies placed for each edit, and compare the result with a full replay. The counting lives in the test harness, not in the rebuild.

## 4. One recipe format

New files are written as `catallax-recipe`, following RECIPE.md section 6.2 in the Catallax repository. That section already maps `soliddraft-recipe/1`, including how `nearest` and `within` selectors become names resolved once against a built part. Old files and the existing corpus stay readable through a converter. The converter lives on the Catallax side, so both projects test it; switching the document model's file format is SolidDraft's work. The geometric digest stays as a test tool during the change and goes once agreement on point sets and names replaces it.

Persistent references get simpler. Catallax names faces by the operation that made them and reports origin sets through splits and merges, so the origin becomes the identity and the fingerprint becomes a check. The repair prompt for a genuinely lost reference stays. The face-role heuristic (caps by centroid, a wall if cylindrical, sides in topology order) is deleted.

Reference survival is tested as a scenario, not argued. The scenario that edits the bracket's plate thickness and boss radius also renames features and reference planes, splits and merges faces with a later cut, suppresses and restores a feature, undoes and redoes, and saves and reopens the `.dwprt` file. After each step every reference resolves to the same origin or raises the repair prompt. The application never recovers a lost reference by picking the nearest face or by guessing an identity from geometry.

## 5. The application loop for agents

Agents write most of the code, so the application needs a loop that checks it end to end, fast, with no person at the keyboard. Catallax's roadmap calls it the application loop; the kernel loop is Catallax's `catallax-verify` command.

Headless SolidDraft on Catallax is driven by agents through `soliddraft mcp` and the command line, the same commands the assistant and the ribbon use. A scenario builds and edits parts the way a user does: sketches, features, parameter edits, undo, rollback, drawings. Each run saves a bundle:

- the recipe after every command;
- rendered views: hidden-line SVG from the existing `view` command, and raster screenshots made without a window by the same renderer the viewport uses;
- Catallax's per-operation log: how each decision was settled, refusals with their codes, marks, local tolerances and timings;
- elapsed time for each command, taken by the runner from outside the application and split into construction, edit, display (tessellation and rendering), measurement and refusal, next to Catallax's deterministic work counts;
- a pass or fail result for each check in the scenario.

A command that returns before its body is displayed or its requested measurement is made has not met the click budget. The scenario times the whole path, from the command to the displayed result, and warns on a step that exceeds the latency budget Catallax's roadmap sets for that kind of command. Timings sit beside the deterministic counts and do not replace them.

Measurement is test and development tooling. Timings, rebuild reports, survival checks, comparison runs and the scoreboard live in the scenario runner, the benchmarks, CI and the development build. None of it runs inside the application at runtime or slows a user edit, and measurement stays separate from the code path that builds the part. Checks that already run at runtime by design stay as they are; no new runtime work is added to them.

Any failure exports the recipe, or the failing step, as a Catallax kernel-loop question, so the fix is checked in seconds with `catallax-verify` and the scenario then runs again through the application. Because it is the same application users run, what the agents test is what users get. Full scenario suites run on GitHub's runners.

## 6. Features in the kernel's order

Application work for a feature, such as its PropertyManager page, ribbon command, tree entry and document-model plumbing, can be built ahead of the kernel. The feature switches on when Catallax lands its operation, and the application loop builds and edits the matching target part.

| Feature | Catallax stage | State when SMLib goes |
|---|---|---|
| Sketch on a plane or a planar face; lines, rectangles, circles, arcs, slots | 1 | Works |
| Extrude, cut, two-sided through all | 1 | Works |
| Revolve | 1 | Works for rational frames, widening with the foundation |
| Booleans of bodies | 1 | Works |
| Mass properties | 1 | Works, computed on request |
| STEP AP242, STL and 3MF export | 1 | Done |
| OBJ export | 1 | Done with this change |
| IGES export | After STEP export, before STEP and IGES import | Planned; needs Catallax's exact-surface IGES writer |
| One-sided through all, up to face, up to next | 3 | Works for planar limits on targets without curved walls; curved limits wait for general targets |
| Hole and countersink | 3 | Works |
| Chamfer | 3 | Works between planes and cylinders, one distance |
| Pattern and mirror | 3 | Works on the whole body |
| Shell, offset, thicken | 3 | Waits |
| Fillet between planes and cylinders | 3 | Works |
| Other fillets | 4 | Waits |
| Draft | 4 | Works for planar faces and cylinders parallel to the pull |
| Sweep, loft; spline, ellipse and text profiles | 5 | Waits |
| STEP import, then IGES import | 6 | Waits for Catallax's importers |
| Lattice, gyroid, graded density, field-driven thickness | Field-only track, after stage 3 | One cubic lattice or one network gyroid works as the last feature, its radius or level constant or graded linearly along a part axis, with proven field moments and an explicit mass stand-in; other grades and field-driven thickness wait |

Reference geometry (planes, axes, points) is application-side work and can come at any time.

The export set is STEP, IGES, STL, 3MF and OBJ. STEP AP242, STL and 3MF are complete; OBJ is complete with this change. IGES export comes next in the exchange work, once Catallax provides an exact-surface writer like its STEP writer. It does not depend on STEP import or IGES import.

## 7. Drawings on Catallax

Drawings use the polygon-mode pass (`mesh_hlr`) over Catallax's display triangles and edge polylines, with face origins preserved for associativity. Plane sections subtract a half-space through Catallax booleans and hatch the cut-face loops. Whole lines and untrimmed circles retain their geometry after polygon visibility; other curves and silhouettes keep the polygon result. Exact silhouettes and the remaining curve conversions are Catallax requests. The drawing document, its associativity and its exporters keep their formats.

Every curve in a drawing says how accurate it is. Whole lines and untrimmed circles are exact. Silhouettes and other curves are fitted from polygons at the mesh's chord tolerance, and visibility is decided on polygons. The drawing document and its exports record which. A requested fit tolerance is never reported as a proven bound, and zero deviation measured at sampled points stays sampled. The labels tighten as exact silhouettes arrive from Catallax.

## 8. Work that does not depend on the kernel

These items from the previous roadmap continue in parallel.

- Modelling and documents: autosave, recovery, thumbnails and a start page for recent documents; assembly editing, drawings of assemblies and Pack and Go; configurations and in-context references after assembly references are stable. Exchange work follows section 6's kernel dependencies.
- Interface: a shortcut editor and right-click selection tools (the shared command registry and a minimal command palette, opened with Ctrl+Shift+P, are in; every command the app runs is also an MCP tool and runs with `soliddraft run`, except those that need the window, whose rows say why; `crates/soliddraft-cli/tests/entry_points.rs` checks every entry point against the registry); on-canvas feature handles, display presets, dark theme and interface scaling; material property sources and conditions.
- Reliability: command inputs are fuzzed and damaged document files are tested (docs/ci.md); [replayable command journals for crash reports](application-loop.md#command-journals) are implemented; interface scenarios and layout checks at several window sizes; kernel work off the interface thread in the browser build; installers once the document formats and modelling tools are ready.

On-canvas handles drag with previews. While a handle moves, the application shows a preview computed off the interface thread, from the field kernel or from a cached mesh of the last exact result, and delivers intermediate values as the drag goes on. When the drag settles, the exact B-rep result at the requested value replaces the preview. The viewport and the tree label a preview as a preview. Only the exact result is saved, exported or measured; a preview carries no proof, and a stale preview never stands in for an edit that failed. One drag is one undo step. The interface scenarios drive continuous input, cancellation, a change of selection during a drag and a result that arrives after its drag was superseded.

Some of these items were informed by public work on other open-source CAD projects, including [SolidCraft](https://github.com/tangency-labs/solidcraft); ideas only, with no code, screenshots, fonts or icons copied, and any future code reuse keeps the upstream licence and attribution in `NOTICE`.

## 9. Verdicts

### 9.1 Architecture

| Item | Verdict | Reason and what happens |
|---|---|---|
| The SMLib bridge: `soliddraft-kernel-smlib`, `soliddraft-smlib-sys`, the cxx engine, the kernel thread, the `third_party/usd-brep` submodule | Dropped | Removed once the Catallax backend displays the first target part, in the same change. Not kept as a fallback. The building instructions lose the C++ compiler. |
| The swappable-kernel trait | Changes | Becomes Catallax's application interface in the four steps of section 3. |
| `soliddraft-recipe/1` | Changes | Converges on `catallax-recipe`; old files read through the converter. |
| The remote kernel backend | Changes | The JSON protocol stays as a developer tool for the command line, MCP and tests. The server kernel stops being a product path, because Catallax compiles into the application's own WebAssembly module. |
| The WebAssembly kernel backend: SMLib under Emscripten as a second module | Dropped | Goes with SMLib, with its Emscripten CI job. Catallax is safe Rust and builds for `wasm32-unknown-unknown` inside the application module. Its findings stay as evidence that the browser side of the application is sound. |
| Golden digests and the conformance runner | Changes | SMLib-only golden files are removed. The runner replays recipes on Catallax; Catallax view and drawing baselines stay. |
| Kernel thread and process isolation | Dropped | Needed only for SMLib's process-wide state. |
| `validate()`, per-operation volume checks, `KernelError::InvalidResult` | Changes | Volume checks stay as tests. The runtime gate goes: Catallax never returns a body it has not proven valid, and a refusal is a typed outcome. |
| Persistent references | Changes | Simplify as section 4 says. |
| Face-role heuristics | Dropped | Catallax gives every face a defined role. |
| Tessellation through the kernel | Changes | Catallax's shared-edge meshes with proven chord bounds, face ids and edge polylines. SMLib's five controls collapse to a chord tolerance and a vertex limit. Levels of detail stay an application concern. |
| Drawing queries | Changes | Polygon mode first, exact silhouettes and sections as Catallax requests, as section 7 says. |

### 9.2 Phases of the previous plan

| Phase | Verdict | Reason |
|---|---|---|
| 0, kernel bridge | Done; its premise is retired | Its exit test, identical digests across runs and a failed operation leaving the previous body, becomes the acceptance test for the Catallax backend. |
| 1, shell and viewport | Stays | Kernel-independent. |
| 2, modelling | Stays | Fillets and chamfers between planes and cylinders work on Catallax stage 3; other fillets wait for stage 4. References simplify. Sketch coordinates and the solver remain floating-point; point joining feeds Catallax's rational-profile converter. |
| 3, assistant | Stays | Kernel-independent. MCP and the command line drive the application loop. |
| 4, second kernel and exchange | Changes | Becomes the Catallax backend and the SMLib removal. STEP comes from Catallax, not from an OpenCASCADE bridge, which also avoids its LGPL dependency. STL, 3MF and OBJ come from Catallax's meshes. IGES export waits for Catallax's exact-surface writer, not for import. The recipe runner and Catallax drawing baselines remain. |

### 9.3 Queued work

| Work | Verdict | Reason |
|---|---|---|
| Release machinery: tagged self-contained builds for three platforms | Stays | Release notes carry the works-today list and name Catallax as the kernel. |
| Phase 4 as written: a Rust backend, cross-kernel runner, STEP and mesh export | Changes | Rewritten as next items 1, 2 and 6 below. |
| Cancel superseded CI runs | Done | A push cancels its branch's older runs and never a run on main (docs/ci.md). |
| Feature plumbing: revolve, shell, hole, patterns, mirror, reference geometry | Changes | Ordered by Catallax's stages, as section 6 says. |
| Sketch depth: polygon, ellipse, spline, text, offset, mirror, trim and extend, patterns, automatic relations | Changes, split | Tools that make lines and arcs come now. Ellipse, spline and text profiles come with Catallax stage 5, so the sketcher and the kernel gain them together. |
| Settings, display presets, shortcut map, theme and interface scale | Stays | Kernel-independent. |
| Autosave, recovery, backup files, thumbnails | Stays | Kernel-independent. |
| Material property sources and conditions | Stays | Kernel-independent. |
| Depth-ordered picking, click cycling, handles mapped to parameters | Stays, after the Catallax backend | Handles mapped to parameters should use recipe origins from the start. |
| Command palette and the phase 1 interface halves | Stays | Kernel-independent. |
| The SolidWorks comparison corpus: recipe generator, comparator, SolidWorks and Onshape runners | Changes | Its comparator targets Catallax. The SMLib column goes with SMLib. The SolidWorks run waits until Catallax builds the first three target parts, so it measures the kernel that ships. |
| Committing the corpus tooling | Stays | Without it the corpus cannot be rebuilt or extended toward the target parts. |
| Making the repository public | Done | Before it opened, README.md's "the kernel can be swapped without changing the app" line was replaced by what is true. |

## 10. Next items

Numbered in priority order.

1. **Catallax backend behind the existing interface.** Build. Pairs with Catallax next item 6. Deliverable: a backend for `soliddraft_kernel_api::Kernel` on Catallax's application entry, chosen in `soliddraft_app::kernel()`. Exit: the bracket recipe builds, displays, picks named faces, edits and rebuilds through Catallax inside Catallax's interactive targets; phase 0's exit test passes on Catallax. Depends on Catallax milestone M2.
2. **Remove SMLib.** Build. Deliverable: the SMLib crates, the `third_party/usd-brep` submodule, the WebAssembly kernel module and its Emscripten job deleted; waiting features marked in the ribbon with their stage; README.md's works-today list rewritten. Exit: a fresh clone builds with Rust alone on all three platforms and in the browser build; CI passes; every waiting command names its stage. Ships with the Catallax backend in item 1.
3. **The application loop.** Build. Deliverable: section 5, with raster screenshots made without a window, Catallax's per-operation log and the split timings the runner takes in each run's bundle. Exit: an agent scenario builds and edits the bracket headlessly and writes the bundle; the bundle reports each command's time split into construction, edit, display, measurement and refusal, and a step over its budget fails the scenario; a deliberately failing scenario exports a question that `catallax-verify` replays with the same refusal; the suite runs on GitHub's runners. Depends on Catallax milestone M2.
4. **Drawings on Catallax.** Build. Deliverable: the polygon-mode hidden-line pass over Catallax's meshes, plane sections, and section 7's accuracy labels on every curve. Exit: the bracket drawing with four views and a section regenerates on Catallax; an application-loop scenario makes that drawing, edits a parameter, regenerates it and writes SVG, PDF and DXF, with each curve's accuracy label in the bundle; SVG, PDF and DXF output keep their formats. Ships with the SMLib removal in item 2.
5. **One recipe format.** Build. Deliverable: section 4's write and read paths. Exit: every recipe in `recipes/` converts and replays on Catallax, and existing part files open. Depends on Catallax milestone M3.
6. **STEP and mesh export.** Done for STEP AP242, STL, 3MF and, with this change, OBJ. STEP carries Catallax's deviation report; meshes carry their chord tolerance, and OBJ retains the object name, face groups and normals. Exit: the bracket's STEP opens in OpenCASCADE with the same face count and a volume within the reported deviation; its mesh exports retain the triangles within their recorded chord tolerance. Depends on Catallax milestone M3.

   **Next exchange work: IGES export.** Planned after STEP export and before STEP and IGES import. Deliverable: exact surfaces through a Catallax IGES writer, like the STEP writer. Depends on that Catallax writer, not on an importer. Exit: the exported bracket opens in an independent IGES reader with its exact surfaces and reported deviation preserved.
7. **References on Catallax origins.** Build. Deliverable: section 4's simplified references, with the face-role heuristic deleted, and section 4's survival scenario in the application loop. Exit: plate-thickness and boss-radius edits on the bracket keep every reference; the survival scenario keeps every reference across rename, split, merge, suppress and restore, undo and redo, and save and reopen; a genuinely lost reference still raises the repair prompt. Depends on Catallax milestone M3.
8. **Rebuild from the first changed feature.** Build. Deliverable: section 3's rebuild, and a rebuild report, written by the development build into each application-loop command's bundle, that counts features recomputed, meshes rebuilt and bodies placed; the counting stays out of the rebuild path. Exit: editing the last feature of the bracket rebuilds only that feature and rebuilds no unrelated mesh or placement; renaming a feature recomputes nothing; restoring the previous value reuses the earlier bodies; in each case the result is identical to a full replay in bodies, face names and refusals. Depends on Catallax milestone M4.
9. **Feature plumbing in the kernel's order.** Build, recurring. Deliverable: each feature in section 6 switched on when its Catallax operation lands. Exit, per feature: the application loop builds and edits the matching target part. Switched on so far: holes and countersinks, patterns, mirrors, fillets and chamfers between planes and cylinders, drafts, and planar up-to limits. Next: curved limits, shell, other fillets.
10. **Sketch depth, first half.** Build. Deliverable: polygon, trim, extend, offset, mirror and patterns of lines and arcs. The shared geometry tools, numeric sketch controls and seven `ci/scenarios/sketch-*.json` application-loop scenarios are implemented. Exit: profiles made with the new tools build through Catallax, with each scenario extruding its profile and checking volume and height. Sketch arithmetic remains tolerance-based; Catallax is exact after conversion. Pointer workflows and previews belong to the interface-design lane. Ellipse, spline and text profiles remain out of scope until Catallax stage 5.
11. **The SolidWorks comparison corpus on Catallax.** Build. Waits for a machine with SolidWorks. Deliverable: the comparator pointed at Catallax. Exit: the first comparison report for the first three target parts.

Kernel-independent items from section 8 run alongside, as people and machines allow.

## 11. Open questions

1. **Drawings.** The polygon-mode hidden-line pass is implemented as proposed. The question remains open for the project owner. Exact silhouettes remain Catallax work.
2. **Waiting features.** Confirm the behaviour in section 2: disabled ribbon commands that name their stage, and existing parts that stop at a waiting feature with a rebuild error.
3. **Shared with Catallax's roadmap. Answered.** Target parts P1 to P10 are confirmed. The B-rep carries the stand-in for field-only lattices. Fillets between planes and cylinders come forward into Catallax stage 3.
