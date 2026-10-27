# P2 collar application slice

The public P2 meridian is a rectangle at radii 6–12 mm, height 12 mm,
revolved 360 degrees about Y. Whole-ring chamfers on the two outer rims
start at 1 mm and edit to 0.75 mm. `ci/scenarios/collar.json` records these
operations, undo/redo, drawing requests and STEP/STL output requests.

The saved model now has a Revolve feature with a sketch, axis origin,
axis direction and angle. The ribbon and sketch exit open its editor;
`revolve` and `edit_revolve` expose the same model operation to commands.
The existing Catallax adapter already implements revolution, so this
slice requires no adapter, lifecycle, dependency-pin or export changes.
The build dispatch includes Revolve's existing sketch profile version
in the existing dependency calculation.

## Evidence and defect ledger

- Historical: Revolve was an advertised placeholder without command or
  model dispatch. Current: command and real ribbon/editor tests pass.
- Original and edited collars expose six named faces and six named
  edges with display pick entries. Outer rims are `Revolve1 · Edge 1`
  and `Revolve1 · Edge 3`. Edit, undo, redo and native save/reopen pass.
- The rendered app regression writes screenshots and exports both STEP
  files through the existing application output path. Clicking Make drawing
  then Create drawing opens four populated views in the active drawing
  tab (verified with the wgpu screenshot). Independent OCP
  readback reports valid six-face solids, volumes 3998.200250468612 and
  4029.9761511627325 cubic millimetres respectively.
- Four model unit tests and both dedicated collar regressions pass.
  Model/app Clippy passes with warnings denied. The dedicated application
  regression exercises the real wgpu renderer; a machine without a GPU
  explicitly reports the UI portion as not run.
- Individual GUI harness command-through-render samples were 209 ms
  original and 255 ms edited. These are not controlled performance
  comparisons or qualification results.
- Resolved integration blocker: CLI evidence now handles Revolve in its
  exhaustive feature match. Revolve contains no stored FaceRef slots.
  Release CLI build and CLI Clippy now pass.
- The separate Catallax cap triangulation fix is commit
  `260dc026645f20d70fd8a81c300608b31ea1b718`. SolidDraft retains its existing
  Catallax pin; these app checks do not claim integrated acceptance of
  that fix. L14 owns dependency integration.

## Full public scenario follow-up

All six steps in `ci/scenarios/collar.json` pass with the checked-in
Catallax pin `878bb7fa78d036508564173d7ed425fc13385a37` and a verifier built
from that same revision. No temporary dependency override was used.
The unchanged click and drawing-view budgets pass, together with live
viewport PNGs, hidden-line SVGs, original/edited drawing outputs,
STEP/STL exports, volume checks and undo/redo. Command plus live display
times in this run were 499.998 ms for revolve, 145.218 ms for the original
chamfer and 211.482 ms for its edit. These are single-run observations.

Reproduce after building the release CLI and matching verifier:

```sh
SOLIDDRAFT_CONFIG_DIR=/tmp/soliddraft-collar-config python3 ci/run-scenarios.py ci/scenarios/collar.json --out /tmp/soliddraft-collar-evidence --soliddraft target/release/soliddraft --catallax-verify /path/to/pinned/catallax-verify
```

The first launch could not create its journal in the environment's default
read-only configuration directory; the supported `SOLIDDRAFT_CONFIG_DIR`
override resolved this without source changes. The focused CLI schema
refusal regression also passes.

Functional app checks are distinct from complete acceptance. Main was
still `a743000060cafd9304beb87e65b3b496dfd6cd4d` at reconciliation; L03 lifecycle
work had not landed, so failure snap-back/red-state acceptance remains
pending L03. Integrated mesh-fix acceptance remains pending the separately
owned dependency update. No qualification tier is claimed by this
public-only slice.
