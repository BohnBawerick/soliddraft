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
- Current integration blocker: CLI evidence's exhaustive feature match
  needs `FeatureKind::Revolve(_) => Vec::new()`. Revolve contains no
  stored FaceRef slots. This file belongs to L12 and is left untouched
  pending coordination. The full scenario has not run.
- The separate Catallax cap triangulation fix is commit
  `260dc026645f20d70fd8a81c300608b31ea1b718`. SolidDraft retains its existing
  Catallax pin; these app checks do not claim integrated acceptance of
  that fix. L14 owns dependency integration.

Functional app checks are distinct from complete acceptance: scenario
budgets, certified drawing export, failure snap-back/red-state behavior,
and integrated mesh-fix acceptance remain to be demonstrated. No
qualification tier is claimed by this public-only slice.
