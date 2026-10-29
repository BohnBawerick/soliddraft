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

## Acceptance status

**The center-section correction is merged in Catallax. Final pinned-application
and manufacturing acceptance remain expected-pending.**

[The torus-section correction](https://github.com/BohnBawerick/catallax/pull/160)
merged as `3c0a4ff3536ae193cee6d85bf83182dd4e439940`. An independent SolidDraft
build at `ad00c04073b4ec02b6059011acc7935aca503ef4`, with a local path override
to the correction's exact source `74d20c36263f73c626c0815eaa5834c8a0822b1b`,
passed all 14 material/reference/history steps and all six section commands.
Both center drawings showed one connected block/boss/junction cut region.

| Cut plane | Original region area, mm² | Edited region area, mm² |
| --- | ---: | ---: |
| Center, y=15 | 593.289 | 594.255 |
| Front boundary, y=0 | 360 | 360 |
| Outside, y=-10 | No cut regions | No cut regions |

These are mesh-derived results, not exact-area or manufacturing certificates.
The y=0 cut polygons span x=[0,40], z=[0,9], excluding the disputed point
`(80/3,0,22)`; the y=-10 sections contain no cut material at `(40,-10,88/3)`.

SolidDraft still declares kernel pin `878bb7fa78d036508564173d7ed425fc13385a37`,
whose baseline center sections refused at `op_117` with
`boolean.torus-chart-required`. The override build retains that declared backend
label; the candidate source above identifies the kernel actually tested.
The section commands must be repeated on the final application pin. A merged
kernel correction does not establish that the pinned application contains it.

### Expected-pending, not waived

The initial local run at SolidDraft `a743000060cafd9304beb87e65b3b496dfd6cd4d`
passed all 14 commands, material volumes, reference checks and history restoration.
Its timing failures remain recorded: junction edit 1317.802 ms, reopen
2107.888 ms, failed hidden-line views 1210.652–2423.167 ms, and separately
requested measurement after reopen 1010.604 ms, against unchanged 1000 ms limits.

[The hosted scenario run](https://github.com/BohnBawerick/soliddraft/actions/runs/40450607631)
passed all 14 P3 steps, including timing gates. That result does not erase the
local measurements or introduce new budgets, performance baselines or entries in
`known-over-budget.json`.

“Expected-pending” documents missing acceptance. It does not suppress a failure,
convert it to success, or change a runner/checker assertion. Hosted CI discovers
this JSON file through its existing scenario glob and retains failing evidence.

This fixture makes no manufacturing export request. STEP trim validity and
original/edit export acceptance remain separate unresolved gates. Neither a
correct viewport nor the successful candidate sections establish full
rounded-boss acceptance.
