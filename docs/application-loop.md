# Application scenarios

The scenario runner drives `soliddraft mcp` over its stdio protocol and captures
the live document through a retained offscreen viewport and Catallax SVG view.
It runs `soliddraft draw` only for requested drawings and the pinned verifier for
refusal replay. MCP uses the command registry and document commands the assistant
and ribbon use. The runner needs Python 3 and the two release executables. It opens
no window.

```sh
cargo build --release --locked -p soliddraft-cli
python3 ci/run-scenarios.py ci/scenarios/bracket.json \
  --soliddraft target/release/soliddraft \
  --catallax-verify /path/to/pinned/catallax-verify \
  --out /tmp/bracket-run
```

Build `catallax-verify` from `CATALLAX_REVISION` in
`crates/soliddraft-kernel-catallax/src/lib.rs`. Hosted CI checks out that exact
revision and builds its `catallax-cli` package. A verifier from a different
revision is not evidence of matching behavior. The workflow uploads both
successful and failed bundles as `application-loop`.

`bracket.json` builds the plate, flange, bored boss, four mounting holes and flange
slot. It checks volumes, changes the flange depth, undoes and redoes the edit,
makes the slot shallower and undoes that, rolls back to the plate and returns to the
end. The slot edit must rebuild only the slot, and the undo and the return to the end
must run no kernel operation. Its final step makes a drawing.
Model commands, viewport screenshots, SVG views and drawing output use Catallax.
The view and drawing checks require that backend. Views use the same polygon
hidden-line method as the drawing workspace.

`references.json` is the reference survival scenario. It builds the bracket with
sketches on the plate's and the boss's top faces, splits the plate's top with a groove,
sketches on one piece, and merges the top with a flush tab. It then changes the plate
thickness and the boss diameter, renames features and the Top reference plane, suppresses
and restores the groove, undoes and redoes, and saves and reopens the part. Every
reference must survive these steps, including the datum references. Deleting the boss
then raises the repair prompt for the bore's missing plane. Split and merge checks
inspect the actual origin sets, not just feature statuses.

`refusal.json` sets the deterministic per-call work allowance to zero. The
extrusion refuses with `numbers.operation-budget`, leaves the part unchanged,
and exports its actual Catallax recipe and allowance. The runner calls
`catallax-verify replay` and requires the same refusal code and a complete
response. A crash, timeout, invalid input or missing response fails the scenario.

The seven `sketch-*.json` scenarios create profiles using `sketch_geometry`, then
extrude them through Catallax and check volume and extrusion height. They cover a
regular polygon, line and arc trim, line and arc extend, a mixed line/arc offset,
mirror, linear pattern and circular pattern. Rendered steps retain the SVG view
and viewport PNG through the same runner as the bracket.

`sketch_geometry` accepts `plane` and an ordered `operations` array. Operations
use the same `soliddraft_sketch::tools::Operation` implementation as the sketch
properties panel. `add` supplies seed line, arc, circle or point entities;
`polygon`, `trim`, `extend`, `offset`, `mirror`, `linear_pattern` and
`circular_pattern` operate on those entities. Coordinates are `{ "u": ..., "v": ... }`
in the sketch plane, not world coordinates. Entity ids start at 1 in insertion
order and are never reused. Trim keeps the target id for the first surviving
piece and allocates another id only when it splits a curve. Copies receive new
ids. See the scenarios for complete requests, including construction boundaries.

Sketch arithmetic remains tolerance-based. These scenarios prove the resulting
profiles work through Catallax's existing exact conversion, not that sketch
intersections, offsets or rotations use rational arithmetic.

`progressive.json` creates a 320 × 20 × 10 mm plate, then 30 separate sketches
and cuts of the same 2 × 4 mm hole at (10, 10), each 12 mm deep. Each cut is undone
and redone before the next sketch. The surface stays at ten faces after the first
cut, so the run separates recipe length from live geometry size. Volume and cached
history work are checked throughout. The four sketch/cut/undo/redo series must not
develop a feature-count latency trend.

## Files and checks

A scenario is `soliddraft-scenario/1` with a name and ordered `steps`. Each step
has `tool` and `arguments`, using the schemas printed by `soliddraft mcp --help`.
Optional `checks` use JSON pointers into `command.json`, with exactly one of
`equals`, `approx`, `minimum` or `contains`. `approx` accepts an absolute
`tolerance`, default 0.001. Missing values fail. A step can request `drawing: true`
for drawing output or `refusal` with an expected stable kernel code. Every step
with a displayed solid captures the live viewport and SVG, including sketches
and history changes. Unexpected tool errors fail immediately.
MCP rounds reported volumes to two decimals, so the bracket checks allow
0.005 mm³ around the analytical volume. A step's `repair` lists the features expected
to raise the repair prompt; features that wait for them may report it, and every other
feature must build. `{bundle}` in a string argument is replaced by the run's bundle
directory, for `save` and `open`.

Every step also checks references: each reference in `command.json` must keep its
original identity and Catallax operation tag and role. A built face sketch must lie on
the plane of the input body's faces carrying that origin. The reference must resolve
unless its feature is in `repair`. A reference may disappear only when the step deletes
a feature.

Every run has `scenario.json`, `protocol.json` and `result.json`. Each executed
MCP tool call has a numbered directory under `commands/` containing:

- `command.json`: input, output, backend, the current model summary and every
  persistent reference: the feature holding it, its slot, its identity, the Catallax
  origins, carrier counts and whether the built sketch's plane matches those carriers.
  `resolved` is null when that feature was not built. These observations run only after
  execution when `--bundle` was requested; the scenario runner makes the assertions.
  The `rebuild` report's `features_recomputed` lists the features whose kernel
  operations the command ran, in tree order. `operations` counts those operations and
  `meshes_rebuilt` the meshes made. The bundle counts by wrapping the kernel, and the
  application does no counting. `meshes_rebuilt` includes display levels the background
  worker finished during the command, so scenarios do not check it.
- `part.json` and `part.dwprt`: the application's complete feature recipe and
  saved part after the command, including undo, rejected proposals and rollback.
  The archive omits optional display caches and thumbnails, as recovery snapshots do.
  Collecting command evidence must not request more meshes of an unchanged body.
- `recipe.json`, when a solid exists: the recipe that constructed the displayed
  body, for the existing view command.
- `kernel.json`: the actual kernel calls, work counts, errors and Catallax's
  `catallax-operation-log/1` entries. Entries retain decision settlement counts,
  marks, refusal codes and timings. Catallax currently reports local tolerances
  as null. Cached queries can have no numeric operations. Nested drawing queries
  share the outer call's log and work allowance, identified by `log_scope`; their
  own `work` is null rather than a second charge. `on_capture_thread` distinguishes
  the command thread from concurrent display refinement, whose time must not be
  added to the caller's external wait.
- `question-*.json`: portable `catallax-question/1` inputs for kernel builds,
  including scratch builds that rejected a proposal. These contain Catallax
  recipes, not numeric body handles or paths into this machine.
- `view.svg` and `viewport.png`, when a solid exists. The PNG comes from the
  viewport's `ViewportRenderer`, shader and camera, without reopening saved input.
  `viewport.json` records the retained GPU adapter. `display.json` identifies the
  live document and backend; `display.kernel.json` retains the SVG/display calls.
  A drawing step also writes `drawing.dwdrw`, `drawing.svg` and `drawing.kernel.json`,
  collected by `soliddraft draw --kernel-log`.
- `waits.json` and `timing.json`: external command/display/drawing waits, their
  disjoint phase split, deterministic work and strict budget verdict.
- `checks.json`: the observed value and pass/fail for each check. A refusal step
  also retains the verifier response, manifest and operation log under `replay/`.

A failed application check retains its feature recipe and the kernel questions
already captured. Application-only errors, such as a missing feature name, have
no equivalent kernel refusal; the recipe is evidence for application diagnosis.
Kernel refusals can be replayed directly:

```sh
catallax-verify replay /tmp/bracket-run/commands/0002/question-0000.json \
  --out /tmp/kernel-replay
```

Directories must be new. Neither MCP capture nor the runner overwrites an older
run. Commands have a timeout. An unexpected behavioral failure stops the run;
latency-only failures do not, so later steps retain their measurements.
To collect the same evidence from another agent, start
`soliddraft mcp --bundle /tmp/new-agent-run`. `--max-work N` sets its per-call
kernel allowance. View, drawing and screenshot logging impose no work allowance.
Normal MCP sessions without `--bundle` keep their existing
kernel behavior and do not collect diagnostic data.

## Latency and feature-count trends

The runner's monotonic clock measures the command round trip, live viewport/SVG
capture and any requested drawing. GPU preparation runs once outside the click
interval. Kernel logs partition that external wait into construction, edit,
display, measurement and refusal; nested calls are not charged twice. Concurrent
background calls keep their own time/work record rather than inflating the caller's
wait. Verifier replay is a survival check outside that interval.

The strict command-class limits remain 100 ms for stock primitives, 500 ms for
booleans, 1,000 ms for features and 2,000 ms for comparisons. All bracket and
progressive steps use the 1,000 ms feature limit. `ci/known-over-budget.json`
retains only the owner-approved bracket step 6 exception. An exception that meets
its budget fails as stale and must be removed; no trend rule exempts a budget
failure. Hosted CI publishes every step and uploads the raw records on failure.

An optional `trends` list selects comparable tool series with `tool` and
`from_step`. Each sample records its current feature count. After the scenario,
the runner fits the median pairwise slope (Theil–Sen), requiring at least ten
samples with increasing feature counts. Fitted positive growth over the full
feature-count range must not exceed the fixed 25 ms noise allowance. This catches
gradual accumulation even when every individual click stays below 1,000 ms, while
resisting isolated shared-runner stalls. The progressive scenario excludes only
the initial empty-document sketch from its sketch trend; its click budget still
applies. Missing timing or feature-count evidence fails closed.

Timing, rebuild reports and reference-survival assertions remain in the opt-in
command bundle, scenario runner and CI, not in the model build or normal edit path.

