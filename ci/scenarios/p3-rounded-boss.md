# Canonical P3 rounded boss — expected-pending acceptance

`p3-rounded-boss.json` builds the unchanged Catallax `parts/P3-filleted-boss.json`
geometry through SolidDraft's application commands: a 40 × 30 × 10 mm block,
a radius-8, height-12 boss centered at (20, 15, 10), a radius-2 concave junction,
and radius-1 rounds on **all four top block edges**. Only the junction radius
changes to 2.5 mm. The existing `fillet.json` remains a separate scenario; it
rounds two bottom edges and edits the junction to 3 mm.

The 14 steps cover construction, persistent references, the junction edit,
undo/redo and save/open. The runner captures the live viewport and hidden-line
view. Volume checks run separately after the click and retain the existing
0.005 mm³ allowance. The per-call work allowance is 20,000,000.

| State | Expected volume, mm³ |
| --- | ---: |
| Original; restored by undo | 14428.640126047918 |
| Edited; restored by redo and reopen | 14455.207561827563 |

## Run the application path

Build `soliddraft` and the verifier for the exact kernel pin as described in
[the application scenario guide](../../docs/application-loop.md). Use a new
output directory:

```sh
python3 ci/run-scenarios.py ci/scenarios/p3-rounded-boss.json \
  --soliddraft target/release/soliddraft \
  --catallax-verify /path/to/pinned/catallax-verify \
  --out /tmp/p3-rounded-boss-run
```

The scenario has no expected-refusal steps. An unexpected application error,
wrong volume, lost reference or failed rebuild is a failure, not a pending gate.
Read `result.json` even when the command exits nonzero. Timing failures can leave
complete evidence for all 14 steps.

## Run the required center sections

The scenario runner does not issue these section commands. They are separate,
required acceptance checks on the saved original and edited application states;
adding the JSON fixture alone does not establish section support.

Run each command independently after confirming that the application states
were constructed correctly:

```sh
target/release/soliddraft view \
  /tmp/p3-rounded-boss-run/commands/0008/part.dwprt \
  --dir front --section 20,15,10 \
  --svg /tmp/p3-original-center.svg --json --curves

target/release/soliddraft view \
  /tmp/p3-rounded-boss-run/commands/0009/part.dwprt \
  --dir front --section 20,15,10 \
  --svg /tmp/p3-edited-center.svg --json --curves
```

Each section must succeed and show the block, boss and junction in one connected
material section. For independent comparison, its analytic area is
`40*10 + 16*12 + 2*(r*r-1)*(1-pi/4)` mm². This gives 593.2876110196153 mm² at
r=2 and 594.2533192843268 mm² at r=2.5. The CLI currently reports a mesh-derived
area, rounded to 0.001 mm²; these reference values do not define a new tolerance
or permit an inaccurate section to pass.

Boundary checks use the same commands with `--section 20,0,10` and
`--section 20,-10,10`, writing distinct SVG paths. At y=0 the cut region is the
40 × 9 mm front face; at y=-10 there is no cut region. Geometry behind the plane
can still appear in the view and must not be mistaken for cut material.

## Expected-pending, not waived

The initial run used SolidDraft `a743000060cafd9304beb87e65b3b496dfd6cd4d` and
Catallax `878bb7fa78d036508564173d7ed425fc13385a37`. All 14 commands, material
volumes, reference checks and history restoration checks passed. Both viewports
showed the top-edge rounds and the changed junction.

Acceptance remains **expected-pending** for these observed failures:

- The junction edit took 1317.802 ms and reopen took 2107.888 ms against the
  unchanged 1000 ms click limit. Failed hidden-line view checks ranged from
  1210.652 to 2423.167 ms; the separately requested measurement after reopen took
  1010.604 ms, also against unchanged 1000 ms limits. These observations are not
  new budgets, performance baselines or additions to `known-over-budget.json`.
- Both center sections refused at `op_117` with `boolean.torus-chart-required`:
  non-circular torus boundaries require a certified periodic chart. The expected
  result remains a successful section, not that refusal. Both boundary sections
  returned a 360 mm² region; both outside sections returned no cut regions.

“Expected-pending” documents missing acceptance. It does not suppress a failure,
convert it to success, or change a runner/checker assertion. Hosted CI discovers
this JSON file through its existing scenario glob and retains failing evidence.
The center-section commands above must be rerun after the kernel correction.

This fixture makes no manufacturing export request. STEP trim validity and
original/edit export acceptance remain separate unresolved gates. Neither a
correct viewport nor this fixture establishes full rounded-boss acceptance.
