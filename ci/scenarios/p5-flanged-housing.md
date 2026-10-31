# Canonical P5 flanged housing

`p5-flanged-housing.json` builds the unchanged Catallax `parts/P5-flanged-housing.json`
geometry through SolidDraft's application commands: a stepped profile revolved about y,
four countersunk holes on a radius-17 bolt circle and a radius-2 fillet where the
flange meets the body. The fillet rests on all four countersink mouths, so it has four
torus regions and four fitted spline regions under one blend name.

The four holes are one `hole` feature with four points. `circular_pattern` patterns the
whole solid, which would give four disconnected housings.

The 16 steps cover construction, the radius 2.5 edit, undo, redo, save and reopen. Step
15 asks for radius 3.5, where the ball would leave the mouth edge for the cone. Catallax
refuses it with `shapes.rolling-envelope`, and step 16 reads the retained radius 2.5
document. The per-call work allowance is 20,000,000.

| State | Faces and kinds | Volume, mm³ |
| --- | --- | ---: |
| Housing | 6: 3 plane, 3 cylinder | 12880.529879718 |
| Drilled | 14: 3 plane, 7 cylinder, 4 cone | 12495.161180877803 |
| Radius 2, restored by undo | 22: 3 plane, 7 cylinder, 4 cone, 4 torus, 4 spline | 12558.681699123754 |
| Radius 2.5, restored by redo and reopen | The same 22 | 12591.90560450861 |

Step 13 reopens the saved part in a fresh process, compares it with step 11 and exports
checked STEP. That export goes through the ordinary OpenCASCADE import and the kernel
pin's direct raw checker. The volume is that of the exact definition, so the checker's
allowance adds the report's `fit_mm` to its file deviation.

A fitted body has no second moments, so the mass properties report no inertia. Face and
edge values on a fitted face come from the display mesh, and `feature` says so in its
`estimates` field. A screen view of a fitted body uses Catallax's default display chord
instead of the automatic one, which is smaller than a pixel. An exported drawing keeps
the automatic chord, so it costs far more than the screen view.

Timing is advisory here, as it is for every scenario outside `--strict`. The fillet and
its edit spend most of their time in the certified display and the exact measurements.

## Clearing the mouths

Steps 17 to 41 continue from the accepted radius 2.5. At radius 0.999 the fillet's rail
stops 0.001 mm short of every mouth, so the blend is one analytic torus, and its name
carries no region index. The steps edit to it, undo, redo, save, and reopen in a new
process with checked STEP. Radius 3.5 refuses and keeps radius 0.999, then an edit returns
to radius 2.5 and the geometry of step 8.

| State | Faces and kinds | Volume, mm³ |
| --- | --- | ---: |
| Radius 0.999 | 15: 3 plane, 7 cylinder, 4 cone, 1 torus | 12511.609716045489 |

## The placed housing

`p5-placed-housing.json` builds the same housing rotated in the sketch plane by the
3-4-5 quaternion (2,0,0,1) and moved by (7,11,-13), through a sketch on the top plane at
offset -13 and a revolve about the tilted axis. It uses only existing commands, and its
28 steps cover radius 2, the radius 2.5 edit, undo, redo, a reopen in a new process with
checked STEP, and the radius 3.5 refusal. Volumes equal the canonical table above. The
housing and drilled boxes are checked tight; a fitted body's box is a display estimate.
