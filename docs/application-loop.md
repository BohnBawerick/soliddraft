# Application scenarios

The scenario runner drives `soliddraft mcp` over its stdio protocol, then runs
`soliddraft view`, `soliddraft screenshot` and `soliddraft draw` as child commands.
MCP uses the command registry and document commands the assistant and ribbon use.
The runner needs Python 3 and the two built executables. It opens no window.

```sh
cargo build -p soliddraft-cli
python3 ci/run-scenarios.py ci/scenarios/bracket.json \
  --soliddraft target/debug/soliddraft \
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


## Files and checks

A scenario is `soliddraft-scenario/1` with a name and ordered `steps`. Each step
has `tool` and `arguments`, using the schemas printed by `soliddraft mcp --help`.
Optional `checks` use JSON pointers into `command.json`, with exactly one of
`equals`, `approx`, `minimum` or `contains`. `approx` accepts an absolute
`tolerance`, default 0.001. Missing values fail. A step can set `render: false`
for a sketch without a body, `drawing: true` for drawing output, or `refusal`
with an expected stable kernel code. Unexpected tool errors fail immediately.
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
- `recipe.json`, when a solid exists: the recipe that constructed the displayed
  body, for the existing view command.
- `kernel.json`: the actual kernel calls, work counts, errors and Catallax's
  `catallax-operation-log/1` entries. Entries retain decision settlement counts,
  marks, refusal codes and timings. Catallax currently reports local tolerances
  as null. Cached queries can have no numeric operations. Nested drawing queries
  share the outer call's log and work allowance, identified by `log_scope`; their
  own `work` is null rather than a second charge.
- `question-*.json`: portable `catallax-question/1` inputs for kernel builds,
  including scratch builds that rejected a proposal. These contain Catallax
  recipes, not numeric body handles or paths into this machine.
- `view.svg` and `viewport.png`, for rendered steps. The PNG comes from the
  viewport's `ViewportRenderer`, shader and camera. The adjacent `viewport.json`
  records the adapter and kernel calls made while reopening the saved part.
  `view.svg.kernel.json` retains the view command's kernel calls. A drawing step
  also writes `drawing.dwdrw`, `drawing.svg` and `drawing.kernel.json`, collected
  by `soliddraft draw --kernel-log`.
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
run. Commands have a timeout, and the runner stops at the first unexpected failure.
To collect the same evidence from another agent, start
`soliddraft mcp --bundle /tmp/new-agent-run`. `--max-work N` sets its per-call
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
