# Tessellation quality and the display cache

How finely SolidDraft tessellates bodies for the screen, how that adapts to what is on
screen, and how a million placements share one definition's meshes. The code is the
`soliddraft-tessellation` crate; the kernel calls it uses are in
[kernel-bridge.md](kernel-bridge.md). The policy is automatic by default, with a quality slider saved in each
document and a per-component override in assemblies.

## The automatic policy

`Levels::new(quality, diagonal)` turns a quality setting and a body's bounding diagonal
into the kernel parameters of three levels:

| Level | Chord height | Used for |
|---|---|---|
| Coarse | 4 x standard | far away, lightweight opening |
| Standard | 0.2 percent of the diagonal under Auto | the setting itself |
| Fine | 1/4 x standard | close up |

- The chord height follows the body's size, so a washer and an engine block look equally
  smooth at the size they fill the screen. 0.2 percent is FreeCAD's default.
- Curve and surface angles stay at 15 degrees on every level. At SMLib's own 25,
  triangles beside circular fillets can wind against their face's normal.
- No chord height goes below 0.0001 mm, so a degenerate body never asks the kernel for
  an unbounded mesh.
- The chord height is the deviation the app reports: the mesh is within that distance of
  the exact surface.

Small bodies whose curves the 15 degree angle bounds before the chord does can come out
the same at two levels. The display cache keeps one mesh for both.

## The setting

`Quality` is what a document saves:

| Mode | JSON | Standard chord height |
|---|---|---|
| Auto | `{"mode": "auto"}` | 0.2 percent of the diagonal |
| Slider | `{"mode": "slider", "value": 7}` | Auto times sqrt(2) to the power (5 - value); 1 is 0.8 percent, 10 is about 0.035 percent |
| Custom | `{"mode": "custom", "chord_height": 0.05, "angle_deg": 15, "max_edge_length": 0, "max_aspect_ratio": 0}` | the raw values; levels scale the chord height and edge length |

Slider position 5 gives what Auto gives. `"auto"`, `"7"` and `"chord=0.05,angle=10"`
parse into the three modes for command-line tools.

An assembly can show every part at one quality, and a component with everything under
it at its own (`Settings::overrides`); the nearest override wins. A part shown at
another quality than its own becomes a definition of its own, so it gets its own mesh
while other placements keep theirs. See [assemblies.md](assemblies.md#display-quality).

## Choosing a level per occurrence

`Thresholds::update` moves one occurrence's `Choice` (its level and how long it has
been quiet, eight bytes) on by a frame:

- **Promote at once** when the current level's error on screen (deviation times pixels
  per millimetre at the near side of the occurrence's bounding sphere) covers more than
  1 pixel, to the coarsest level that is within it.
- **Demote one level** after the error has stayed under 0.1 pixel for 2 seconds.
- Levels are a factor of four apart and the thresholds a factor of ten, so a demoted
  occurrence lands at 0.4 pixel or less and does not promote straight back.
- **The ceiling** from the device budget caps the level: an occurrence above it drops
  at once.

`Thresholds::initial` gives a newly shown occurrence the coarsest sharp level, so an
assembly opened from coarse display copies only asks for finer meshes where the camera
needs them.

The assembly viewport makes the same choice on the GPU, for every placement at once, in
its cull pass; see "Rendering and picking" in [assemblies.md](assemblies.md).

## The device budget

The app owns the budget (`DeviceBudget`), not the document, so a file opens the same on
every machine and a smaller device shows it coarser. The desktop default is 40 million
triangles in 2 GiB; the browser default is 8 million in 512 MiB. With usage under 80
percent of the budget the ceiling is fine, under 100 percent standard, beyond it coarse.

## The display cache

`DisplayCache<K, M>` holds meshes by definition and parameters. `K` is whatever the
caller keys a body by: an assembly's definition key (document, configuration, revision)
or an open part's geometry hash. `M` is any mesh that can say how many triangles and
bytes it holds.

- Every placement of a definition draws from the same entries.
- `best(definition, levels, wanted)` returns the wanted level, or while that one loads
  the nearest held level, coarser first.
- Levels whose meshes are equal share one mesh and count once.
- `trim()` drops the least recently used standard and fine meshes until usage fits the
  budget. Coarse meshes stay, so every loaded definition can be drawn, and nothing used
  since the last `tick()` is dropped.

## Tessellation off the interface thread

`Tessellator<K>` answers requests on a worker thread, one `tessellate_levels` call per
body for all its levels. The interface polls once a frame and never waits on the kernel.
A new request for a key supersedes the old one: the worker skips it if it has not
started, and its answer is dropped if it has. `cancel(key)` withdraws a request. The
caller keeps the body alive until the answer arrives; a body released early answers
with `UnknownBody`. A WebAssembly build has no threads, so it answers in place and the
answer waits for the next poll.

## The four tolerances

Four tolerances decide how closely SolidDraft follows exact geometry. Each is its own
setting, and changing one never redoes another's work.

| Tolerance | Controls | Lives in | Changing it redoes |
|---|---|---|---|
| Viewport quality | display meshes | the document's `settings.json` | display meshes only |
| Drawing projection | how closely drawing views follow curves | the drawing, which caches its views | that drawing's views |
| Export | meshes written to mesh formats (0.01 mm, 10 degrees by default) | the document's `settings.json` | the next export |
| Modelling | when the kernel treats two points as one | the kernel | the geometry and everything made from it |

`settings.json` (`soliddraft-settings/1`) sits outside both document hashes, so changing
a setting never gives the document a new revision: assemblies and drawings that pin it
see no change, and no cache keyed by geometry goes stale. It also holds the "save display
data" switch. The modelling tolerance is the other way round: it changes geometry, so it
belongs in the part's truth and its hashes. SMLib fixes its own and takes none through
SM_API, so no document carries one yet.

The drawing tolerance (`DrawingTolerance`, auto or a distance in millimetres, sent as the
view request's `tolerance`) belongs to the drawing, because the drawing caches its views
against the revisions they came from. A tolerance kept in the model would change what a
view shows without the drawing knowing. Drawings use auto until the drawing file stores
one.

## Reporting

`report(kernel, body, quality)` tessellates a body at every level in one call and gives
each level's deviation in millimetres, its triangle count and the parameters used. The
quality slider shows these next to the setting.

`soliddraft_model::Document` exposes the part's side: `settings()`, `set_settings(s)`
(checked, not an undo step, no new revision) and `quality_report()`.

## The part view

The app's part document (`soliddraft-app`, `display_quality.rs`) shows one of the part's
three levels:

- A rebuild tessellates the level the camera last asked for at once, so the viewport
  never waits for the worker to show a new shape. The worker then makes the other two.
- Each frame, `Document::update_display(projection, dt)` moves the level choice on with
  the viewport's pixels per millimetre (`ViewportState::projection`) and swaps in a finer
  or coarser mesh when it is ready. It returns true while the worker still owes a level,
  so the app keeps repainting until it arrives.
- `Document::set_viewport_quality(q)` and `Document::set_settings(s)` save the setting in
  the part and ask the worker for every level. The old mesh stays on screen until the
  new one arrives, and nothing rebuilds.
- `Document::display_quality()` says which level is shown and wanted, and each level's
  parameters, for the quality slider.

## Display data in part files

A part file keeps its coarse and standard levels in `cache/mesh/coarse.bin` and
`cache/mesh/standard.bin` when its settings save display data (the default) and it has a
body (`soliddraft_model::display`). Assemblies, drawing previews and the browser can show
the part from them without rebuilding it.

- Each is a `DisplayMesh` (`soliddraft_tessellation::mesh`, the same format as an
  assembly's `display.bin` copies): faces named by the part's persistent references,
  never kernel ids, and edges sampled along their curves.
- Its header records the kernel and its version, the level, the exact parameters and
  the part's geometry hash. `display::read` returns a mesh only when that geometry hash
  is the file's, so an edited part never shows an old shape. A mesh from another kernel
  or quality is still a true picture of the same geometry; the reader decides.
- Meshes are saved in canonical order (`DisplayMesh::canonical`): faces by name,
  triangles sorted, vertices numbered by first use. SMLib lists faces in an order that
  can change from run to run, and without this a reopened part would save to different
  bytes.
- The section is optional and outside both hashes. Turning "save display data" off
  leaves it out; the part's revision is the same either way.

For assemblies, `FileSource` shows parts from this data with no kernel at all, and
`KernelSource` reads it before rebuilding anything. `KernelSource::with_quality` shows
every part at one quality instead of each part's own, rebuilding them.

## Command line

`soliddraft quality <part>` prints the part's settings and each level's deviation and
triangle count; `--json` prints the same as one JSON object. `--set auto`, `--set 7` or
`--set chord=0.05,angle=10` changes the viewport quality, `--export 0.01,10` the export
accuracy and `--display-data on|off` the switch, and the part is saved in place. The
printed `revision` is the same before and after.
