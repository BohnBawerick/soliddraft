# The kernel bridge (phase 0)

SolidDraft talks to a geometry kernel only through the `Kernel` trait in
`soliddraft-kernel-api`. This page covers that trait, the first backend on the C++ SMLib
kernel, the recipe format and the exit test.

## Crates

| Crate | What it holds |
|---|---|
| `soliddraft-kernel-api` | The `Kernel` trait, handles, profiles, history and query types, and the `contract` checks every backend must pass (feature `contract`). No dependency on any kernel. |
| `soliddraft-smlib-sys` | Compiles SMLib and SM_API from the `third_party/usd-brep` submodule into a static library. |
| `soliddraft-kernel-smlib` | The SMLib backend: a cxx bridge to a small C++ engine, the kernel thread, result checks and history. |
| `soliddraft-recipe` | The recipe format, replay on any `Kernel`, and the geometric digest. No dependency on any kernel. |
| `soliddraft-cli` | The `soliddraft` binary: `soliddraft replay [--json] [--steps] <recipe.json>...` and `soliddraft view <recipe.json> --dir front --svg out.svg`. |
| `soliddraft-drawing` | Drawing documents: sheets, views, annotations, templates and commands. Phase D0 has the types, a fixture drawing and stubs. No dependency on any kernel backend. |

## The trait contract

- Handles are opaque. A backend owns every body; `BodyId`, `FaceId`, `EdgeId` and
  `VertexId` are numbers only that backend can interpret. No kernel type crosses the
  trait.
- Operations do not mutate. `extrude`, `boolean`, `fillet` and the others read their
  inputs and return a new body. A failed operation therefore always leaves the previous
  body as it was, and the document layer can cache one body per feature.
- Every operation returns `Result<OpResult, KernelError>`.
- A kernel's success status is not trusted on its own. Each result is checked before it
  is returned: SMLib's `AssertValid`, a closed manifold solid for solid operations, a
  positive finite volume, and a volume check per operation (an extrusion's volume against
  profile area times distance, a revolution's against Pappus, a union between the larger
  input and the sum, a subtract or intersect no larger than its input, a fillet or
  chamfer that changes the volume and creates at least one face). A result that fails is
  freed and reported as `KernelError::InvalidResult`. This is the guard against the
  false successes in usd-brep's `KNOWN_ISSUES.md`.
- `history(body)` returns one `HistoryRecord` per operation that produced the body,
  oldest first. Each record carries the caller's `OpTag` and lists the faces, edges and
  vertices the operation created, modified, deleted or split. Entities that an
  operation leaves alone keep their ids. This is the foundation for persistent naming
  (see [modelling.md](modelling.md)).
- Each record also gives the role of every face the operation created (`FaceRole`):
  the region of a profile; the start cap, end cap, or the side swept from profile
  segment `(loop, segment)` of an extrusion or revolution; the blend of a given input
  edge for a fillet or chamfer; the offset of a given input face for a shell's inner
  wall; `Other` for anything else, such as a patch a boolean adds.
- `topology(body)` lists faces, edges and vertices. A planar face carries its unit
  normal pointing out of the material, also on the inside faces of holes and shells;
  an edge carries its curve type and its start and end vertex; a vertex carries its
  position and edges.
- `tessellate(body, params)` triangulates a body for display, each triangle labelled by
  its face. `TessellationParams` carries all five of SMLib's controls: chord height,
  curve and surface angle, edge length and `max_aspect_ratio` (the longest edge over the
  height; zero, the default, disables it, and requests written before it existed leave
  it out). SMLib 1.0.0 applies the aspect ratio while subdividing spline surfaces; on
  planes, cylinders and fillet blends it changed no mesh in our checks, so the
  automatic policy leaves it off and only advanced settings set it.
  `tessellate_levels(body, levels)` makes one mesh per entry in one call: the
  SMLib backend looks the body up once and reads each face's tag once for all levels,
  and the remote backend sends one request. Its default runs `tessellate` per entry, so
  a new backend need not implement it. The levels and when each is drawn are in
  [tessellation.md](tessellation.md).
- `tessellate_edges(body, params)` samples every edge along its exact curve, from its
  start vertex to its end vertex, until the tangent turns at most
  `params.curve_angle_deg` between samples and, when `params.chord_height` is set, the
  chord stays that close to the curve. The viewport draws and picks edges from it.

The checks in `soliddraft_kernel_api::contract` state the contract as code. The SMLib
backend runs all of them in `crates/soliddraft-kernel-smlib/tests/contract.rs`.
Additional backends can run the same functions.

## The SMLib backend

### Threading

SM_API keeps one process-global context that is not synchronised (`SmApiGeneral.h`).
The backend therefore runs every kernel call on one dedicated thread. That thread calls
`SmApiCreateContext()` once, before any other SM_API call, and then serves commands sent
over a channel. Every `SmlibKernel` value in a process talks to that one thread and
shares its bodies, so the trait is `Send + Sync` and calls from several threads are
serialised. A crash inside the kernel still ends the process; moving the thread into a
child process later would not change the trait.

### The bridge

`cxx` bridges Rust to a small C++ class, `Engine` (`cpp/engine.cpp`), which holds every
`SmBrep` in a table keyed by handle. The engine calls SM_API for every kernel operation
(`SmApiCreatePlanarFaces`, `SmApiCreateLinearSweep`, `SmApiCreateRotationalSweep`,
`SmApiBooleanUnion`/`Difference`/`Intersection`, `SmApiFilletEdges`,
`SmApiShellBrepFull`, `SmApiTessellate`, the `SmApiBrep*`, `SmApiFace*` and `SmApiEdge*`
queries). It uses SMLib classes directly for three things SM_API has no call for:
reading topology, keeping entity tags in attributes, and walking tessellation output.
Operations that SM_API runs in place (booleans, fillets) run on a private copy made with
`SmApiBrepCopy`, so inputs are never touched. C++ exceptions carry the SM_API status
code and the name of the failing call back to Rust.

### History

Each face and edge carries an `SmLongAttribute` with a tag. SMLib copies attributes when
it copies, splits or merges an entity (`SmAttribute.h`, "Attribute propagation"), so a
tag on a result entity names the input entity it came from. After each operation the
backend reads the raw tags and makes them unique:

- an entity with no tag is new and gets a fresh tag (created);
- a tag found on one entity stays (unchanged, or modified when its area or length,
  centroid or midpoint, or bounding box moved beyond round-off);
- a tag found on several entities means the input entity was copied or split. If exactly
  one piece still has the input's geometry, it keeps the tag and the others count as
  created (a shell's inner skin is the example). Otherwise every piece gets a fresh tag
  and the record lists a split.

Input entities whose tag no longer appears are deleted. Extrusions and revolutions
build new bodies, so everything in them counts as created.

### Vertices, normals, roles and edge samples

Vertices carry a tag attribute like faces and edges, and go through the same tag and
history logic, so a block's corners keep their ids through a union that does not touch
them.

A face's surface normal says nothing about which side the material is on: a subtraction
keeps the tool's surfaces as they were, so a hole's floor still has the tool cylinder's
downward normal. SMLib's radial-edge topology gives each face two faceuses, one per
side, and a faceuse's normal points into the region its shell bounds. The outward normal
is therefore the reverse of the faceuse on the solid (non-void) side, the same
orientation SMLib's own mass properties use. A sheet has no solid side and keeps its
surface normal.

SMLib does not say which part of a sweep made a face, so roles come from the result's
geometry against the operation's own inputs: caps are the planar faces parallel to the
profile plane at the start and at the far end (for a revolution, the far plane rotated
back); a side face has an edge whose midpoint and ends lie on one profile segment in the
profile plane; a blend face rounds the selected edge whose bounding box centre is
nearest its own; a shell's inner wall is a copy of its outer face, which the tag
attribute records as the piece's parent.

Edge samples are computed in the engine by halving parameter intervals until the
tangents at both ends and the chord deviation meet the controls, then reversed if needed
so they run from the edge's start vertex. SMLib arcs run counter-clockwise only, so a
clockwise profile arc is built from its end to its start.

### Building the kernel

`soliddraft-smlib-sys` compiles the kernel from source with the `cc` crate, from the
submodule pinned at usd-brep commit `c9c5979f04cea8c38213004d4f04df0dcfb40d05`
(release 1.0.0). Prebuilt static libraries were the alternative; building from source
was chosen because it needs nothing but a C++17 compiler, keeps the pinned commit the
single source of truth, and works the same on every platform `cc` supports. The cost is
time: SMLib is about 240 C++ files and takes several minutes to compile once. It lives in
its own crate so that changes to the bridge do not recompile it, and dev builds compile
it with optimisation (`opt-level = 2`) so tests do not run an unoptimised kernel.

Upstream's premake and `repo.sh` build was not used: it fetches a toolchain and OpenUSD
through packman, and SMLib and SM_API need neither. The flags follow upstream's release
configuration in `premake5.lua`: C++17, `NDEBUG`, `NOMINMAX`, `SM_GRAPHICS_CALLBACKS=1`,
and `-Wno-write-strings` for SM_API. `SM_USE_TBB` stays undefined, as upstream leaves
it, so the kernel runs single-threaded and needs no TBB.

### GCC 16 and -Werror

Upstream builds with `-Werror` (`FatalCompileWarnings`). GCC 16 turns on three
diagnostics that fire in the unmodified sources, so the upstream build fails on GCC 16:

| Diagnostic | Reports (repeats from shared headers included) | Cause |
|---|---|---|
| `-Woverloaded-virtual` | 258 | Derived classes declare a method with the name of a base virtual and a different signature, such as `SmAssemblyInstance::GetBrep()` hiding `SmTopology::GetBrep() const` |
| `-Wtemplate-id-cdtor` | 236 | Destructors spelled `~SmProbArray<TYPE>()`, which C++20 disallows |
| `-Wunused-but-set-variable` | 55 | Variables assigned and never read |

All three are diagnostics about well-defined code; none changes what the compiler
generates. The fix is in the build, not the kernel: SolidDraft compiles SMLib and SM_API
without `-Werror` and with warnings silenced (`-w`). No kernel source file is patched,
so SMLib's semantics are upstream's. The counts above come from compiling every file of
both libraries with upstream's flags and GCC 16.2.1; with `-Werror` removed, the same
sources compile with no errors.

## Drawing queries

The trait's drawing queries (`soliddraft_kernel_api::drawing`) give the 2D drawing code
their curves. Each has a default that returns `KernelError::Unsupported`, so a backend
can add them one at a time, except `section_view`, whose default is built from the other
operations.

| Query | What it returns | SMLib |
|---|---|---|
| `view_curves(body, &ViewSpec)` | Visible and hidden curves of a parallel view, each tagged with the edge, seam or silhouetted face it comes from | `SmHCR`, then the crossing pass below |
| `silhouette_curves(body, direction)` | The outline seen from a direction: face silhouettes and outline edges | `SmApiCreateSilhouetteCurves` |
| `section_curves(body, &Plane)` | Where a plane cuts the body, tagged with the cut face | `SmApiIntersectBrepWithPlane` |
| `section_view(body, &SectionSpec)` | A section view: the cut body's visible and hidden curves, and the regions where the plane meets material, to hatch | The default: subtract a half-space box, `view_curves` (so `SmHCR`) on the cut body, the cut faces' `face_loops` |
| `edge_curve(body, edge)`, `face_loops(body, face)` | Exact edge geometry, and face boundaries in loop order | `SmApiFaceGetBoundaryLoops` and the edge curves |
| `lines_and_arcs(&Curve3, tolerance)` | A spline as lines and circular arcs | `SmApiConvertToLinesAndArcs`, checked and halved until within tolerance |

Curves come back as `Curve3`: lines, arcs and circles where the kernel's B-spline is
one, otherwise the exact NURBS. The SMLib bridge (`cpp/hlr.cpp`) checks every exported
spline against the kernel's own points before using it.

`SmHCR` returns copies of curve intervals, not entities. The bridge tags each copy by
geometry: three interior points on one edge make it a piece of that edge (or of a seam),
otherwise three points on one face make it that face's silhouette. Seams and smooth
edges that the view does not show, which `SmHCR` reports as hidden, are left out.

`SmHCR` decides each piece of a curve with one ray cast, after splitting the curve where
it crosses the projected outline. It misses some crossings: the floor edge of a slot seen
through the slot, or the bottom rim of a hole seen through the hole, comes back hidden or
visible as a whole. `drawing::visibility::refine_visibility` therefore finds every place
where two curves cross in the view, cuts the exact curves there, and asks `SmHCR`'s own
ray caster about each new piece (`Engine::hidden_points`).

`drawing::mesh_hlr` is a hidden-line pass over a tessellation, after FreeCAD TechDraw's
polygon mode. It works on any backend with `tessellate` and `edge_curve`, gives
polylines, and is the yardstick for exact backends: the contract compares visible length
per edge.

A section view maps every id in its result back to the uncut body through the
subtraction's history: an edge the cut trimmed or split keeps its original id, and each
boundary of a cut region is tagged with the face of the uncut body it lies on. A drawing
therefore names section curves with the same references as the body's other views.

The bracket views are pinned in `recipes/golden/smlib-views.json`, written by
`SOLIDDRAFT_BLESS=1 cargo test -p soliddraft-cli --test view_exit_test bracket_views`, and
section A-A of the bracket in `recipes/golden/smlib-sections.json`, written by
`SOLIDDRAFT_BLESS=1 cargo test -p soliddraft-kernel-smlib --test sections`.

## Recipes

A recipe (`soliddraft-recipe/1`) is an ordered list of steps. Each step has an id, runs
one command (`profile`, `extrude`, `revolve`, `boolean`, `fillet`, `chamfer`, `shell`)
and names its inputs by the ids of earlier steps. Faces and edges are chosen by
selectors evaluated when the step runs, never by kernel ids:

| Selector | Chooses |
|---|---|
| `"all"` | every face or edge |
| `{ "created_by": "<step>" }` | what that step generated, followed through later splits and deletions in the body's history |
| `{ "nearest": [x, y, z] }` | the one entity whose centroid (faces) or midpoint (edges) is nearest |
| `{ "within": { "min": [...], "max": [...] } }` | entities whose bounding box lies inside the box |
| `{ "all_of": [ ... ] }` | entities every listed selector chooses |

A step marked `"expect": "error"` must fail, and replay checks that its input bodies
keep their digests. A selector that matches nothing fails its step.

The geometric digest hashes the body's kind, face, edge and vertex counts, volume, area,
bounding box, and every face's surface type, area and centroid and every edge's length
and midpoint, each rounded to a grid of a millionth of the body's scale and sorted, so it
does not depend on ids or on the order a kernel lists entities in. `canonical_text`
gives the hashed text for comparing two digests line by line.

The corpus is in `recipes/`; `recipes/golden/smlib.json` holds the expected results on
the SMLib backend. It is generated: run
`SOLIDDRAFT_BLESS=1 cargo test -p soliddraft-cli --test exit_test corpus_matches_golden_digests`
after a deliberate change, and review the diff.

## Exit test

Phase 0 exits when box, cylinder, union, subtract and a circular fillet replay from
recipes with identical digests across repeated runs, and a deliberately failing
operation returns an error and leaves the previous body.

`crates/soliddraft-cli/tests/exit_test.rs` checks this through the `soliddraft` binary,
each run in a separate process:

- `box`, `cylinder`, `union`, `subtract`, `fillet` and `failing-fillet` replay three
  times; every report, including every step's counts, volume, area and digest, is
  identical across the runs.
- In `failing-fillet`, a 50 mm fillet on a 10 mm block fails in the kernel
  (`SmApiFilletEdges`, status 1001); the step reports the error, and the block afterwards
  equals a block that never saw the failure.
- Every recipe in the corpus matches `recipes/golden/smlib.json`.

The contract tests add a second deliberate failure: subtracting a block from an
identical block, which would leave nothing, returns an error. Phase 2 added four
contract checks for the new queries: vertices keep their ids, edge samples follow the
exact curves and honour both controls, planar faces carry outward normals on outer,
hole and shell faces, and every created face has exactly one role.

### Platforms

| Platform | Kernel built and exit test run | Rust-only crates (`kernel-api`, `recipe`, `drawing`) tested |
|---|---|---|
| Linux x86_64, GCC 16.2 (development machine) | yes | yes |
| Linux x86_64, CI | yes | yes |
| macOS, CI (`macos-latest`) | no | yes |
| Windows, CI (`windows-latest`) | no | yes |

What is not proven yet: that SMLib compiles with Apple Clang and MSVC through
`soliddraft-smlib-sys`, that the bridge links there, and that digests on macOS and
Windows match the Linux golden file. Upstream builds SMLib on both platforms with its
own scripts, so the sources are known to compile there; the open risks are this crate's
flags (in particular the MSVC export macros in a static library) and floating-point
differences between platforms that move a value across a rounding boundary of the
digest. Building the kernel on macOS and Windows runners cost several times the Linux
minutes while the repository was private, so CI runs only the Rust-only crates there
for now.
