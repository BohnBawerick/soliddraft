# Application scenarios

The scenario runner drives `soliddraft mcp` over its stdio protocol and captures
the live document through bundle-only display tools. MCP uses the command
registry and document commands the assistant and ribbon use. Drawings and refusal
replays run as child commands. The runner needs Python 3 and the two built
executables. It opens no window.

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

`refusal.json` sets the deterministic per-call work allowance to zero. The
extrusion refuses with `numbers.operation-budget`, leaves the part unchanged,
and exports its actual Catallax recipe and allowance. The runner calls
`catallax-verify replay` and requires the same refusal code and a complete
response. A crash, timeout, invalid input or missing response fails the scenario.

## Files and checks

A scenario is `soliddraft-scenario/1` with a name and ordered `steps`. Each step
has `tool` and `arguments`, using the schemas printed by `soliddraft mcp --help`.
Optional `checks` use JSON pointers into `command.json`, with exactly one of
`equals`, `approx`, `minimum` or `contains`. `approx` accepts an absolute
`tolerance`, default 0.001. Missing values fail. `render: false` describes a sketch
without a body; it never skips display timing when a body exists. A step can set
`drawing: true` for drawing output, or `refusal` with an expected stable kernel
code. Unexpected tool errors fail immediately.
MCP rounds reported volumes to two decimals, so the bracket checks allow
0.005 mm³ around the analytical volume.

Every run has `scenario.json`, `protocol.json` and `result.json`. Each executed
MCP tool call has a numbered directory under `commands/` containing:

- `command.json`: input, output, backend, the current model summary and the
  `rebuild` report. `features_recomputed` lists the features whose kernel operations
  the command ran, in tree order. `operations` counts those operations and
  `meshes_rebuilt` the meshes made. The bundle counts by wrapping the kernel, and the
  application does no counting. `meshes_rebuilt` includes display levels the
  background worker finished during the command, so scenarios do not check it.
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
- `view.svg` and `viewport.png`, for steps with a displayed body. Both use the
  live MCP document, never a reopened recipe or saved part. The PNG comes from
  the viewport's `ViewportRenderer`, shader and camera. `viewport.json` records
  the adapter, `display.json` the model backend, and `display.kernel.json` the
  view's actual kernel calls. `scenario_prepare` retains one offscreen GPU and
  renderer for the run; `scenario_display` captures without editing or advancing
  the command number. These tools exist only with `mcp --bundle`.
  A drawing step also writes `drawing.dwdrw`, `drawing.svg` and
  `drawing.kernel.json`, collected by `soliddraft draw --kernel-log`.
- `timing.json`: external command, live display and requested drawing waits,
  their total, five phase totals, deterministic work counts and the click-budget
  verdict. The same record appears in `checks.json` and `result.json`.
- `waits.json`: external segment durations, also kept if phase attribution fails.
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
run. Commands have a timeout. Semantic, protocol and artifact failures stop the
scenario; latency failures remain recorded while later steps run.
To collect the same evidence from another agent, start
`soliddraft mcp --bundle /tmp/new-agent-run`. `--max-work N` sets its per-call
kernel allowance. View, drawing and screenshot logging impose no work allowance.
Normal MCP sessions without `--bundle` keep their existing
kernel behavior and do not collect diagnostic data.

## Click budgets

The runner uses a monotonic nanosecond clock outside the application. It sums
the command response, live viewport and SVG capture, and requested drawing
command. A command response alone cannot pass a step with a slow display.
Protocol initialization, GPU setup, Python checks and refusal replay are outside
the click interval. Capture includes pixel readback and artifact writing.
The command interval also includes existing evidence snapshot overhead, so this
is a conservative headless wait, not an isolated kernel benchmark.

Catallax's [PLAN section 7.3](https://github.com/BohnBawerick/catallax/blob/main/docs/PLAN.md#73-useful-latency-kept)
sets the fixed limits:

| Command class | Budget |
|---|---:|
| Stock solid | 100 ms |
| Boolean | 500 ms |
| Recipe feature or edit | 1,000 ms |
| Complete background comparison | 2,000 ms |

Application `extrude` and `cut` tools are recipe features, even when they perform
a boolean internally. Requested measurement and display share the command's
budget. No scenario or CLI option can raise it. The 300-second timeout only
contains hung subprocesses; it is not a latency allowance.

The split uses `kernel.json` and Catallax's per-operation logs. Top-level display,
measurement and refused calls own their whole interval. Other calls extract
logged display and measurement operations; their remainder belongs to construction
or edit according to the command. Nested `log_scope` calls are already included
by their parent and are never charged twice. Work counts follow the same
ownership rule. External time outside kernel calls belongs to the command phase,
or to display for capture. Missing or invalid timing evidence fails the run.
Opt-in diagnostic calls identify whether they ran on the kernel capture thread.
Only that thread's outer calls partition the command wait. Background calls keep
their phase work and aggregate call durations under `background`; these durations
can overlap and are never added to, or subtracted from, the external wait.
Nested calls are still owned by their enclosing log scope on either thread.
The ownership marker adds no clock and is absent from the normal kernel path.
Unexplained capture-thread overlap still fails attribution and preserves
`waits.json`.
The pinned kernel does not log initial meshing inside its application build as
a separate operation. That inseparable cost remains in construction or edit,
not in the display subtotal; no timing is invented to separate it.

An over-budget step fails but the runner continues collecting later steps when
all semantic and artifact checks passed. Other failures stop the scenario.
Hosted CI uploads the complete bundle and publishes per-step waits and budgets
in its job summary. Shared-hosted-runner numbers are not quiet pinned-core
performance baselines. Over-budget results remain failures unless the owner
explicitly approves a known-failure policy; budgets are never increased to pass.
All timers and budget decisions live in the Python runner and CI. No clocks or
reports are added to the application's model build or edit path.
