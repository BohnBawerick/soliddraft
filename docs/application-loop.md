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
thickness and the boss diameter, renames features, suppresses and restores the boss,
undoes and redoes, saves and reopens the part, and deletes the boss. Every step checks
that each reference keeps the origin it named and resolves, or raises the repair prompt
where the step expects it. Reference planes are fixed datums today, so the scenario
renames features only.

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
`tolerance`, default 0.001. Missing values fail. A step can set `render: false`
for a sketch without a body, `drawing: true` for drawing output, or `refusal`
with an expected stable kernel code. Unexpected tool errors fail immediately.
MCP rounds reported volumes to two decimals, so the bracket checks allow
0.005 mm³ around the analytical volume. A step's `repair` lists the features expected
to raise the repair prompt; features that wait for them may report it, and every other
feature must build. `{bundle}` in a string argument is replaced by the run's bundle
directory, for `save` and `open`.

Every step also checks references: each reference in `command.json` must name the same
origin as at the previous step, and must resolve unless its feature is in `repair`. A
reference may disappear only when the step deletes a feature.

Every run has `scenario.json`, `protocol.json` and `result.json`. Each executed
MCP tool call has a numbered directory under `commands/` containing:

- `command.json`: input, output, backend, the current model summary and every
  persistent reference: the feature holding it, its slot, the origin it names and
  whether the last rebuild resolved it (null when that feature was not built). The
  bundle writer reads these from the rebuild; it resolves nothing itself.
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
kernel behavior and do not collect diagnostic data.
