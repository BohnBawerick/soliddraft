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

Both sketch tools accept an optional signed `offset` in millimetres along a
reference plane's normal. Omitted offsets are zero; nonzero offsets on body
faces are refused. For example, `front` has normal -Y, so `offset: -6` places
the sketch at y=6. World-coordinate shapes must lie on that displaced plane;
`sketch_geometry` operations still use its local u/v coordinates.

`datum-offset.json` exercises this control at the normal 20M allowance. It
extrudes from the displaced plane, saves and reopens the part, then edits the
extrusion and sketch dimension. Volume and world-space bounds must survive;
every displayed step retains its viewport PNG, SVG and click-budget verdict.

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
Optional `checks` use JSON pointers into `command.json`, or `/measurement/...` for an
explicit live-body measurement requested after the edit and display have been timed.
Each check has exactly one of `equals`, `approx`, `minimum` or `contains`. `approx`
accepts an absolute `tolerance`, default 0.001. Missing values fail. A step can request `drawing: true`
for drawing output, `export` with a list of extensions (`step`, `stl`, `3mf`) for
`soliddraft export` after the click is timed, or `refusal` with an expected stable kernel code. A `label`
names the step in the result's `timings`, such as a feature's build or edit. Every step
with a displayed solid captures the live viewport and SVG, including sketches
and history changes. Unexpected tool errors fail immediately.
Volume checks request full-precision results rather than triggering measurement through
the model summary; the existing 0.005 mm³ bracket allowance is unchanged.
A step's `repair` lists the features expected
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
- `kernel.json`: the command's actual kernel calls, work counts, errors and Catallax's
  `catallax-operation-log/1` entries. Entries retain decision settlement counts,
  marks, refusal codes and timings. Catallax currently reports local tolerances
  as null. Cached queries can have no numeric operations. Nested drawing queries
  share the outer call's log and work allowance, identified by `log_scope`; their
  own `work` is null rather than a second charge. `on_capture_thread` distinguishes
  the command thread from concurrent display refinement, whose time must not be
  added to the caller's external wait. `evidence.kernel.json` holds the queries
  that collecting this evidence made, such as reference topology, after the click.
- `question-*.json`: portable `catallax-question/1` inputs for kernel builds,
  including scratch builds that rejected a proposal. These contain Catallax
  recipes, not numeric body handles or paths into this machine.
- `view.svg` and `viewport.png`, when a solid exists. The PNG comes from the
  viewport's `ViewportRenderer`, shader and camera, without reopening saved input.
  `viewport.json` records the retained GPU adapter. `display.json` identifies the
  live viewport and backend; `display.kernel.json` retains only its capture calls.
  The separate bundle-only `scenario_view` request writes `view.svg`, `view.json`
  and `view.kernel.json` from the same live body, without reopening saved input.
  Its external wait, phase split and work are in `view.timing.json` and the step's
  `drawing_view` result. A drawing step also writes `drawing.dwdrw`, `drawing.svg`
  and `drawing.kernel.json`, collected by `soliddraft draw --kernel-log`. An export
  step writes `part.<extension>` and keeps the export report in its check.
- `measurement.json` and `measurement.kernel.json`, when a check requests
  `/measurement/volume_mm3` or another quantity. The bundle-only `scenario_measure`
  request runs after the complete click interval, so it cannot pre-warm that display.
  It neither advances the command sequence nor changes the part. Its own elapsed time,
  work, unchanged 1000 ms ceiling and 2,000 ms gate are retained as `measurement` in the step result.
  Any measurement still performed by an edit is charged to that edit, not subtracted.
- `waits.json` and `timing.json`: external command and viewport waits, their
  disjoint phase split, deterministic work and unchanged click-budget verdict.
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
`soliddraft mcp --bundle /tmp/new-agent-run`. A command's files appear after its
reply: the bundle writes them on `scenario_evidence`, before the next request other
than `scenario_display`, or when the session ends. `--max-work N` sets its per-call
kernel allowance. View, drawing and screenshot logging impose no work allowance.
Normal MCP sessions without `--bundle` keep their existing
kernel behavior and do not collect kernel diagnostic data.

## Command journals

Desktop and MCP document sessions write an always-on command journal under the
configured recovery folder's `journals/` directory. Browser sessions write the
same records to localStorage, one item per record. Native file locks and browser
Web Locks keep live sessions out of crash-report offers. No heartbeat is used.

A journal is `soliddraft-journal/1` JSON Lines. Each document instance starts with
its authored state, identity and settings, without part display caches. Part
baselines also keep any existing undo and redo commands. Subsequent records hold
the command intent and its returned refusal or success. The intent is written
synchronously before entering the handler, so a process crash leaves the failing
command readable. Native writes are unbuffered, not a guarantee against power
loss. An incomplete final record is ignored, including a torn UTF-8 character.
A malformed complete record is an error.

Part editors, undo, redo, rollback, drawing edits and MCP tools use these hooks.
An MCP call is recorded once, including a refused proposal. Its speculative
model calls are not separate edits in the journal. A scratch fork never writes
its own document journal. Document replacement starts a new instance in the
same session rather than losing the preceding commands.

MCP `open` and `save` also retain the filesystem inputs their commands observed.
An opening part's authored state is recorded before its rebuild, without the
archive's display caches. Replay rebuilds that state through the model loader;
it does not require the source file. Writes still serialize the document, then
consume the recorded filesystem outcome without touching the original
destination. Missing or mismatched input records fail closed. Source-file
refusals and proposal ids survive a document replacement. Path tokens keep
`.dwprt`, `.dwasm` and `.dwdrw` suffixes so exported commands remain valid.

The journal contains commands and their outcomes, not durations, rebuild counts,
survival checks or per-edit state hashes. Normal close removes only the current
session's journal. A crash leaves it for the next start, even with autosave off.
The recovery window offers **Save crash report journal** and **Discard journal**.
Browser reports download; desktop reports use a save dialog. Later keeps the
offer. Browser storage requires a secure origin with Web Locks and writable
localStorage. Storage failures are shown, and a failed writer never resumes with
a silent gap.

Browser startup discovers saved reports and installs read-only export and discard
controls before writing a new journal header. If storage cannot accept that
header, the editor does not start. The controls remain available through failed
retries and reloads. Export uses the same path-redacted download as the recovery
window and keeps the saved report. Discard removes only the selected report,
after checking that no live tab owns it. Free space, then choose **Retry journal
startup** to start editing with a new persisted header.

The report copy replaces POSIX, Windows, UNC and relative reference paths with
stable report-local tokens, including paths inside command JSON and error text.
The raw local journal is never offered or changed by export. Geometry, feature
names, notes and other authored content remain in the report. Review the copy
before sharing it.

```sh
soliddraft journal export /tmp/local-session.jsonl /tmp/crash-report.jsonl
soliddraft journal replay /tmp/crash-report.jsonl --out /tmp/replayed.json
```

Replay uses the application's document and tool handlers with Catallax, without
a window or access to the original part files. Use the same application revision
and kernel version as the crashed run. An explicitly configured deterministic
work allowance from `mcp --bundle --max-work` is restored automatically.
`replayed.json` contains the reconstructed
parts and drawings, command outcomes, the interrupted command number and any
reproduced panic. A changed refusal or reproduced panic exits unsuccessfully.
An unmatched final intent is executed, never replaced by a synthetic crash. If
it returns normally, the report keeps `interrupted` set and does not claim that
the original process failure was reproduced. Replay reports are developer
output, not automatic crash-report attachments. Existing output files are not
overwritten.

MCP's `document_command` accepts canonical part commands in `command_json`,
including `app_command`, `app_undo`, `app_redo`, `app_rebuild` and the model's
`execute`, `undo`, `redo`, `rollback`, `settings`, `group_undo` and `abandon`.
`drawing_command` accepts drawing `execute`, `undo` and `redo`.
`soliddraft mcp --open part.dwprt --drawing sheet.dwdrw` opens both documents.
Commands in the journal are the same data these tools accept.

`journal_replay` deliberately panics a child session during an extrusion edit,
then replays it through the application loop. It compares the actual mutated
part, edited drawing, earlier refusal and panic message. Storage tests cover
live-session exclusion, torn writes, path replacement and clean-close retention.
`ci/browser-journal.js` fills real Chromium localStorage until even a header is
refused, reloads the compiled application, exports or discards the saved report,
and starts the editor after storage becomes writable. It checks retained records,
path redaction and unrelated storage, including another retry while still full.

## Latency and feature-count trends

The runner's monotonic clock measures each click as the command round trip plus
the live viewport capture. Hidden-line SVG, explicit measurement and requested
drawing exports run afterward, outside the click interval. SVG and measurement
have their own checks; drawing export keeps its artifact check and elapsed time.
GPU preparation runs once, before the first step. Writing a command's evidence is
not part of the click either: the command replies, the runner times the viewport
capture, then asks `scenario_evidence` to write `command.json`, `kernel.json`, the
part files and questions. The evidence's own kernel queries go to
`evidence.kernel.json`, outside every interval. Kernel logs partition the
external waits into construction, edit, display, measurement and refusal; nested
calls are not charged twice. Concurrent background calls keep their own time/work
record rather than inflating the caller's wait. Verifier replay is also outside
the click interval.

### Intervals, replies and verdicts

Each step's `intervals` record what the user waited for, with its own verdict:

- `preview` starts when the command request is sent and ends only at a visible
  selectable edit: the `scenario_display` capture of the same request and document
  revision, with a pickable face. A step with no solid ends at the command
  reply; its display request answers `solid: false` and is not timed. This is the
  click and keeps its class budget.
- `view` is the hidden-line SVG request, with its own 1,000 ms ceiling.
- `measurement` is the explicit measurement request. It keeps its unchanged 1,000 ms
  budget and also records a 2,000 ms gate, so a 4,245 ms measurement fails both.
- `export` records each export's elapsed time. It runs after the click, as a separate request.

The bundle tags `command.json`, `display.json`, `view.json` and `measurement.json`
with the `request` number and document `revision` they describe, and `display.json`
with `selectable`, true when some vertex of the displayed mesh carries a face pick id (an early-exit
check, so the timed capture does no per-face counting). The runner accepts a reply only if it names this step's request
and the command's revision (check `answers`). A reply with no tags, an older request or
revision, a pending marker or no pickable face does not end the interval: the step fails
as `click ends at a visible selectable edit`, `view answers this request` or `measurement
answers this request`. A proof for an earlier edit therefore cannot satisfy a newer one,
and a command reply that returns before the viewport shows the edit cannot pass. A
timeout or closed pipe records `incomplete` on the step with the request that did not
answer and fails the run; it is never counted as a pass.

Every click carries a `verdict`: `strict-pass` met its budget, `known-gap` was accepted
only under `ci/known-over-budget.json` and is not a strict pass, `fail` was not accepted.
The result lists `known_gaps` and sets `strict_pass` only when the run passed with none.
`pass` keeps its meaning of CI acceptance.

`--strict` selects the release-path verdict. The exit status is zero only when `strict_pass`
is true, so an approved known gap exits one and is printed as an error. Without the flag the
exit status keeps its meaning of CI acceptance, where a listed gap is a warning. The hosted
workflow runs without `--strict`; a release gate runs the same scenarios with it.

### Pinned evidence

`result.json` records `catallax_revision`, the full commit sha in `CATALLAX_REVISION`,
and every command must report that same sha as its backend version
(`Catallax backend is the pinned revision`). Evidence from another revision, a branch name
or a missing version fails the step. The runner cannot yet check the replay verifier the
same way: `catallax-verify about` does not report its source commit, so hosted CI builds it
from the pinned sha and the workflow remains the evidence for that binary.

The baseline-reuse ledger holds no measured real-corpus time saving, so this runner
claims none and does not consume it. A reused baseline would have to arrive as pinned,
qualified evidence of its own before it could excuse a measurement.

### Independent drawing-view gate

Every step with a solid requests `scenario_display` first and records the click
before asking `scenario_view` for the hidden-line SVG. A slow SVG cannot consume
the user's 1,000 ms edit-to-viewport allowance or escape CI: `drawing_view` has an
independent strict 1,000 ms ceiling, no known-gap exemptions, error annotations
and its own per-step CI summary table. A drawing-view timing failure fails the
scenario while later steps still run, retaining the cost distribution.

The initial ceiling is calibrated from the
[current-main baseline run](https://github.com/BohnBawerick/soliddraft/actions/runs/40229375814)
at `cfedbd7`: 183 capture-thread `view_curves` samples across ten scenarios,
with median 0.565 ms, nearest-rank p95 417.182 ms, p99 731.791 ms and maximum
769.365 ms. The 1,000 ms ceiling leaves about 231 ms above the observed maximum
for protocol, SVG serialization and run-to-run variation. The new check measures
that entire external request, not just the kernel call. The click budget and
existing approved-gap records are unchanged.

Option 3 remains kernel follow-up: faster fine meshing of curved faces inside
`view_curves`. Measure that improvement, then tighten the drawing-view ceiling;
this change does not alter geometric proofs or mesh accuracy to make SVG pass.

### Click budgets and trends

The strict command-class limits remain 100 ms for stock primitives, 500 ms for
booleans, 1,000 ms for features and 2,000 ms for comparisons. All bracket and
progressive steps use the unchanged 1,000 ms feature limit. The owner-approved
eleven known gaps in `ci/known-over-budget.json` record the measured times for
reference steps 16, 17, 18, 25 and 30, and bracket steps 14, 16, 18, 19, 22 and 25.
The records identify the local work-PC Linux runner and the calibration run;
hosted-VM times are not substituted for that machine's baselines.
Bracket step 16 records 1,189.068388 ms, with a 1,426.8820656 ms regression limit.
An over-budget listed step is accepted only up to 120% of its recorded time;
anything slower fails as a known-gap regression. Its `budget_ms` stays 1,000
and `within_budget` stays false. An improved listed step can pass the strict
budget without changing its baseline. Every unlisted step stays strict, and
the feature-count trend guard is unchanged. Remaining work:
https://github.com/BohnBawerick/soliddraft/issues/27 — faster booleans when
rebuilding after an edit. CI prints known gaps as warnings, regressions as errors,
and uploads every raw step record; an accepted gap is not a claim of meeting
the original budget.

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
