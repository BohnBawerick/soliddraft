# Assemblies

An assembly places parts and other assemblies. Each stays its own file: the assembly
references them by id, pins the revision it was last rebuilt against, and keeps display
copies of what it shows, so it opens and looks as saved without opening another file.
The code is `crates/soliddraft-assembly`; the envelope, references and hashes it builds on
are in [documents.md](documents.md).

## The file

A `.dwasm` file is a document envelope with these entries:

| Entry | Section | What |
|---|---|---|
| `manifest.json` | | id, kind `assembly`, format `soliddraft-assembly/1`, hashes, a copy of every source reference |
| `assembly.json` | required | the truth, below |
| `placements.bin` | required when present | unnamed components of a large assembly, as fixed records |
| `cache/refs/<id>/<revision>/...` | optional | display copies at the pinned revisions |

Writing is deterministic: the same assembly and copies give the same bytes.

## assembly.json

```json
{
  "format": "soliddraft-assembly/1",
  "name": "Conveyor",
  "units": "mm",
  "sources": [
    { "id": 1, "document": { "id": "3f0c...", "kind": "part", "path": "parts/bolt.dwprt", "pinned": "blake3:..." } }
  ],
  "components": [
    { "id": 2, "source": 1, "placement": { "translation": [10.0, 0.0, 0.0] } },
    { "id": 3, "name": "Drive end", "source": 1, "placement": { "rotation": [0.0, 0.0, 0.7071067811865476, 0.7071067811865476] }, "hidden": true }
  ],
  "patterns": [
    { "id": 4, "seeds": [2], "rule": { "kind": "linear", "direction": [1.0, 0.0, 0.0], "spacing": 20.0, "count": 8 } }
  ],
  "overrides": [
    { "path": "2:4.3", "suppressed": true }
  ],
  "next_id": 5
}
```

- **Ids.** Sources, components, patterns, mates and values take their ids from one
  counter, `next_id`, and an id is never reused. Ids are stable for the life of the assembly; names are
  labels. A component without a name shows as its source's file name and its id.
- **Sources** are the documents the assembly places. Each is a `DocRef` (see
  [documents.md](documents.md#references)) with its pinned revision. A thousand
  placements of one bolt share one source. A source's `configuration` picks the
  configuration placed; it is reserved and always the default for now.
- **Components** place a source. `placement` is rigid: `translation` in millimetres and
  `rotation` as a unit quaternion `[x, y, z, w]`, each left out when it is the identity's.
  There is no scale and no mirror: scale would change dimensions and mass, and a mirrored
  part is an opposite-hand part of its own. `hidden` components are not drawn but still
  count for mass and bills of materials. `suppressed` components are left out entirely.
  Components are kept in id order, which is the tree's order.
- **Patterns** store the rule, never the copies. A linear pattern has a `direction`,
  `spacing` and `count`, and optionally a `second` direction with its own; a circular
  pattern has an axis `origin` and `axis`, an `angle` between neighbours in degrees and a
  `count`. Counts include the seed. Member 0 is the seed itself; the rule generates
  members 1 and up, numbered row by row for two directions. Each member repeats every
  seed, sub-assemblies included.
- **Overrides** change one occurrence below the assembly's own components: hide a part
  inside a sub-assembly, suppress one bolt of a pattern, move one member. Unset fields
  leave the occurrence as its own assembly has it. A placement override is relative to
  the assembly that contains the occurrence. The assembly's own components are changed
  on the component, never by an override.
- **Appearances** colour occurrences (see [Colours](#colours)), with `snapshots`
  holding the appearances they use.
- **Mates and values** tie components together; see [mates.md](mates.md). A placement is
  still the truth of where a component is: a solve writes placements, and an assembly with
  no mates keeps them as authored.
- **Reserved.** `configurations` and `display_states` are kept exactly as read until those
  formats are defined.

## Occurrence paths

An occurrence is one placed instance of a part or sub-assembly, anywhere in the tree. Its
path is the steps from the root, written with `/` between them. A step is a component
id, or for a pattern member `<seed>:<pattern>.<member>`. So `4/12:9.3` is member 3 of
pattern 9 generated from component 12, inside the sub-assembly placed as component 4 of
the root. Paths never use names, so renaming a component breaks no drawing anchor or
mate. A selection is the root document, an occurrence path, and the persistent name of a
face of the part.

## placements.bin

An assembly with more than 1,024 components writes every unnamed component to
`placements.bin` and keeps named ones in `assembly.json`. Which component goes where
depends only on the component, so the bytes are deterministic; in memory they are one
list in id order. Little-endian:

| Bytes | Field |
|---|---|
| 4 | magic `DWPL` |
| 4 | version, `u32` 1 |
| 4 | record count, `u32` |
| 4 | record size, `u32` 72 |
| 72 each | component id `u32`, source id `u32`, flags `u32` (bit 0 hidden, bit 1 suppressed), zero `u32`, translation `3 x f64`, rotation `4 x f64` |

## Hashes

The semantic hash (the assembly's revision, what a parent assembly or a drawing pins) is
BLAKE3 over the canonical `assembly.json` without its format tag, and, when there is a
`placements.bin`, over a hash of its bytes, by the rule in
[documents.md](documents.md#hashes). The geometry hash leaves out the assembly's,
components' and patterns' names, hidden flags (in both entries), `appearances` and
`snapshots`, display states, mates, values and the sources' paths and locators. Hiding,
colouring or renaming a component changes the revision and not the geometry. Mates decide
placements, and the placements are hashed, so the same placements are the same geometry
however they were reached.

## Display copies

`cache/refs/<document id>/<revision>/` holds what the assembly needs to show a document
at the revision it pins. The revision is written with `-` for `:`, such as
`blake3-3f2a...`.

- A part's copy is `display.bin`, its display mesh (below), and `coarse.bin`, its coarse
  level, once the viewport has loaded one (see "Rendering and picking").
- A sub-assembly's copy is its own truth entries, `assembly.json` and `placements.bin`
  as its file holds them. The copies of its parts sit beside it in the same cache, so the
  root file alone holds everything the tree shows.

Copies are keyed by document and revision, so sources that pin the same revision share
one. The cache is optional: deleting it never changes the design, only how much has to be
read from the referenced files on open.

### display.bin

A display mesh is in the part's frame, in millimetres, with its triangles grouped by face.
Each face carries the part's persistent name for it (the model's face reference as
canonical JSON, empty when unknown), never a kernel face id. The format lives in
`soliddraft_tessellation::mesh` because part files keep the same meshes as their own
display data ([tessellation.md](tessellation.md)). Little-endian:

| Bytes | Field |
|---|---|
| 4 | magic `DWDM` |
| 4 | version, `u32` 1 |
| 4 + n | what made it (kernel and level), length-prefixed UTF-8 |
| 4 x 4 | vertex, index, face and edge counts, `u32` |
| 12 per vertex | positions, `3 x f32` |
| 12 per vertex | unit normals, `3 x f32` |
| 4 per index | indices, `u32`, three per triangle, counter-clockwise seen from outside |
| per face | first triangle `u32`, triangle count `u32`, name length-prefixed UTF-8 |
| per edge | point count `u32`, points `3 x f32` |

## The definition store

In memory, an open assembly has one definition per document, configuration and revision
it shows, however many times it is placed. Each definition has a loading state:

| State | Meaning |
|---|---|
| unresolved | known by reference only |
| metadata | kind, name and, for a sub-assembly, its structure are read; nothing to draw |
| display | its display mesh at the pinned revision is loaded: enough to draw, select, hide, show and move it |
| exact | its kernel body is built too; measure, mass, drawing projection and editing need this |
| stale | drawn from another revision's display data, because the pinned copy was missing; a Rebuild fixes it |
| failed | nothing could be loaded; the definition says why |

The display mesh never answers an engineering question. Separately, each definition
records how its pinned revision compares with the document now: unchecked (opening never
waits for this), current, newer available, or missing.

## Colours

An assembly can colour any occurrence: one of its components or anything below one.
In that assembly, every face of every part under the occurrence shows the colour,
replacing the parts' own colours; opened on its own, a part shows its own.
The nearest occurrence with a colour wins, so a part coloured inside a coloured
sub-assembly keeps its own. Like overrides, a colour set in an outer assembly wins over
one the sub-assembly's file sets on the same occurrence.

```json
"appearances": [
  { "path": "4", "appearance": "anodised-red", "scope": "all" }
],
"snapshots": {
  "appearances": [
    { "id": "anodised-red", "name": "Anodised red", "base_color": "#b3261e", "metallic": 1.0, "roughness": 0.35 }
  ]
}
```

The keys and types are the part's (see [materials.md](materials.md)): the appearance is
`soliddraft_materials::Appearance`, and `snapshots` keeps a copy of every one the assembly
uses, so its look never depends on a library. Every assignment carries a configuration
scope, picked by the same rule as in parts: an assignment for named configurations wins
over one for all of them. Until configurations exist only `all` is written and shown.
Colours are not geometry: the geometry hash leaves out `appearances` and `snapshots`.

## Display quality

An assembly can show its parts at another quality than their own. The
settings live in the file's `settings.json` (`soliddraft_tessellation::Settings`, see
[tessellation.md](tessellation.md)), outside both hashes, so changing them never gives
the assembly a new revision.

- `viewport`: a quality for every part. `auto`, the default, shows each part at the
  quality saved in it.
- `overrides`: components shown at their own quality, with everything under them, by
  occurrence path. The nearest override wins over the assembly's quality.

```json
{
  "format": "soliddraft-settings/1",
  "viewport": { "mode": "slider", "value": 2 },
  "overrides": [ { "path": "4", "quality": { "mode": "slider", "value": 10 } } ]
}
```

A part shown at a quality other than its own is a definition of its own: its
`DefinitionKey` carries the quality (`Quality::key`). Two placements of one part at two
qualities are two definitions with two meshes. The scene, the culling pass and picking
draw them like any other definitions, and all placements at the same quality still
share one mesh. Its copy sits in the part's copy folder as `display-<code>.bin`
(`file::display_entry`), beside the usual `display.bin`. When an assembly opens without
that copy, the loader asks its display source for the part at that quality
(`DisplaySource::part_mesh_at`). `KernelSource` rebuilds it; a source that cannot shows
the part at its own quality instead of failing.

Only the open assembly's settings apply. A sub-assembly's own are read when it is opened
on its own, the way a part's quality applies when the part is open.

`AssemblyDocument::set_quality(paths, Some(q) | None)` and `set_viewport_quality(q)`
change them as one undo step each, re-flattening the table; `settings()` and
`quality_at(path)` read them, and `is_dirty()` counts them.

## The occurrence table

An open assembly is flattened into one table with a row per placed part or
sub-assembly, anywhere in the tree. Row 0 is the root. Each row has its parent row, the
step from the parent, its definition, its placement relative to the parent and in the
root's frame (both `f64`), its bounds in the root's frame, and its flags: hidden or
suppressed (its own and inherited), sub-assembly, removed. The renderer, the picker and
the bounding volume hierarchy read the table's columns; nothing walks the tree per frame.

Row numbers are stable while the assembly is open and are never written to a file.
Paths are what persists.

- Moving an occurrence updates the world placements and bounds of its subtree and the
  bounds of its ancestors, and nothing else.
- Hiding or showing one updates the inherited flag of its subtree.
- Deleting a component flags its rows removed instead of moving other rows.
- Inserting a component, adding a pattern, or unsuppressing a sub-assembly that was
  suppressed when the file opened rereads the structure. Meshes already loaded are kept.

A pattern member starts visible and unsuppressed whatever its seed is; an override hides
or suppresses it.

## Opening

Opening never waits for meshes or for other files.

1. **Structure.** The root's truth, every sub-assembly's structure at its pinned revision
   (from the file's copies, each level read on every core), and the occurrence table.
   The assembly can be drawn, at first with nothing in it, and edited from here. For a
   million occurrences this takes one to two seconds, so the interface opens with
   `AssemblyDocument::open_in_background`, which also builds the `Scene`, and keeps
   drawing meanwhile.
2. **Display.** Each part's display mesh at its pinned revision is decoded on worker
   threads and handed over in batches. A definition's occurrences appear when its mesh
   arrives. `AssemblyDocument::poll(budget)` takes in what has arrived within a time
   budget; call it once a frame.
3. **Freshness.** Every referenced document is resolved, once per document, from the
   file that holds the reference (sub-assemblies first, then their parts on every
   core), and its revision compared with the pin.

`poll(budget)` takes in one batch at a time and stops when the budget is spent, so a
frame never pays for more than it asked. Once loading is done the bounding volume
hierarchy is built on another thread.

When the file has no copy of a pinned revision, the loader tries, in order: the copies
of the sub-assembly files it has opened, the referenced file if it is still at that
revision, the revision store, and last the referenced file at its current revision,
which marks the definition stale. For a part, those three read the part file's own
display data first (`FileSource` reads only that and needs no kernel); `KernelSource`
rebuilds the part on a kernel when the file has none for its current shape. Without any
of them the definition fails and the rest of the assembly opens.

Exact work (measure, mass, drawing projection) asks for a part's kernel body with
`AssemblyDocument::exact`. That reads the part at the pinned revision; when the document
has moved on and no revision store has the old bytes, it fails with "Rebuild to use the
current version" rather than measuring the wrong revision.

## Editing

Every edit is one undo step. An occurrence that is one of the root's own components is
changed on the component. Anything deeper, or a pattern member, is changed by an
override in the root; the sub-assembly's own file is not touched. This is how a
SolidWorks top-level assembly hides or moves a part of a sub-assembly.

| Edit | Call |
|---|---|
| move to a placement relative to the containing assembly | `move_to(&[(path, placement)])` |
| move by a rigid motion in the root's frame | `move_by(&paths, &delta)` |
| hide, show | `set_hidden(&paths, bool)` |
| colour, or remove the colour | `set_colour(&paths, Option<Appearance>)` |
| suppress, unsuppress | `set_suppressed(&paths, bool)` |
| insert a part or assembly | `insert(doc_ref, placement)` |
| pattern components | `add_pattern(seeds, rule)` |
| rename a component | `rename(id, name)` |
| delete components | `delete(&ids)` |
| form parts into a sub-assembly file | `form_subassembly(&request)`, see [subassemblies.md](subassemblies.md) |
| undo, redo | `undo()`, `redo()`, `undo_label()`, `redo_label()` |

`take_changes()` returns what the renderer must re-read since it last asked: definitions
whose meshes arrived, rows that moved, rows whose flags changed, or a new structure.

## Rebuild

`AssemblyDocument::rebuild` implements the app's explicit Update. It resolves every
source of the root and loads the proposed revisions before accepting any of them.
If any source is missing or cannot be shown at its proposed revision, it refuses the
whole Update. The report names the blocking files. Pins, displayed definitions,
placements, dirty state and undo history stay unchanged.

A successful Update accepts the new pins and moved paths together in one undo step
labelled `Update`. Saving afterwards writes the accepted revisions' display copies.
Part edits mark components out of date immediately, including unsaved edits in the
open part. Returning the part to its pinned revision clears the warning. The assembly
keeps showing its accepted revision until Update succeeds.

A sub-assembly shows its own pins: Rebuild the sub-assembly to move its parts on, then
Rebuild the assembly above it.

Saving writes a copy for every definition a live occurrence shows: carried over from the
file it was loaded from, or encoded from what is loaded. Stale and failed definitions get
no copy, because none of their pinned revision exists.

## Rendering and picking

`soliddraft_assembly::Scene` packs an open assembly for the GPU without naming a GPU
API: one 64-byte instance per part occurrence (a 3 x 4 placement, the occurrence's row,
flags for hidden, selected, hovered and coloured, and the colour), grouped by
definition. Placements stay `f64` in the table; the instances hold them as `f32`
relative to an origin in the middle of the parts, and the camera is moved to the same
origin in `f64` before it becomes `f32`, so precision does not fall off far from the
world origin.

All loaded meshes share one vertex buffer and one index buffer. Uploads follow
`take_changes`: a mesh is uploaded once when it arrives (a frame takes a bounded number,
`Scene::take_new_meshes(max)`), and a move, hide or colour rewrites only the changed
instances.

Where the device has compute, storage buffers in the vertex stage and indirect
first-instance (native and WebGPU; the app asks for the feature and for the adapter's
buffer limits), the GPU culls every frame. One thread per instance drops what the camera
cannot see and what is under `Detail::drop_px` (half a pixel of radius by default) on
screen. It draws what is under `Detail::box_px` (three pixels) as its definition's
bounding box, and writes the rest into per-definition lists. The colour pass then draws
every mesh with one `multi_draw_indexed_indirect` call and every box with one more. The
id pass draws only what may cover the 64 by 32 pixel window read back around the cursor,
and nothing when there is no cursor. `Detail` is the light-mode setting: larger values
draw less. Without those features (WebGL2) every visible instance is drawn with one
`draw_indexed` per definition and hidden instances are dropped in the vertex shader.

Each definition has three levels of detail (coarse, standard and fine, see
[tessellation.md](tessellation.md)), and where the GPU culls each placement draws the
one its size on screen needs. The cull pass applies the same rules as
`soliddraft_tessellation::select`, per instance, keeping each instance's level and how
long it has been quiet in one word that lasts from frame to frame:

- it moves to a finer level as soon as the current level's deviation covers more than a
  pixel on screen, and to a coarser one after the error has stayed under a tenth of a
  pixel for 120 frames;
- the app's `DeviceBudget` caps the level, from the triangles and bytes uploaded;
- a level not uploaded yet is drawn as the nearest one that is, coarser first, and its
  definition is marked as wanting it.

So a definition has three draws, one per level, and three lists. Every 15 frames the
wanted marks are read back (`ViewportRenderer::take_assembly_wanted`), and the app hands
them to `AssemblyDocument::request_levels`. A worker thread loads them from the file's
copies, then from the part file's display data, then from the display source
(`DisplaySource::part_mesh_level`; `KernelSource` rebuilds the fine level), and they
arrive through `poll` as `Changes::levels`, `Definition::coarse` and `Definition::fine`,
and `Scene::take_new_levels`. A level that cannot be made is not asked for again until
the structure changes. Saving keeps each definition's coarse level beside its standard
one, as `coarse.bin` (or `coarse-<code>.bin` for another quality), so the next open has
it without the part file. A coarse or fine mesh's faces are numbered like the standard
mesh's, by name, so picks name the same faces at every level.

The id buffer is `Rg32Uint`: the occurrence's row plus one, and the entity (kind 1 for a
face, and the face's index in the display mesh plus one). Both are good for this session
only. `Scene::decode` turns a texel into an occurrence and a face, and
`AssemblyDocument::selection` into what lasts: the occurrence path and the part's
persistent name for the face.

The CPU bounding volume hierarchy (`AssemblyDocument::bvh`) holds every part
occurrence's box, `f32` relative to its own origin, built with binned surface-area
splits. It answers rays (`along_ray`, for the exact point under the cursor and for
measuring), boxes (`in_box`) and the camera's view (`in_frustum`). It is built on
another thread from a copy of the boxes once loading is done (0.7 s for a million
occurrences); `bvh_ready()` returns it when it is there and never waits, `bvh()` waits
for it. A move refits the boxes of the moved occurrences' leaves and their ancestors; a
new structure starts a new build.

Edges are not drawn for assemblies yet; the viewport's kernel-sampled wide edges will
draw them from the display copies' edge polylines.

## Scale

The scale test is a generated assembly with 100,000 unique part files and 1,000,000 part
occurrences: a root placing 50 sub-assemblies, each placing 2,000 unique parts once
(`placements.bin`) and stacking every one of them ten times with one linear pattern. The
parts are blocks and pins of seeded random sizes, with display copies of 12 and 60
triangles; 36 million triangles at full detail. `soliddraft asm bench` opens it through
the app's own viewport renderer, headless on the machine's GPU, and uses it: frames
while it reads the structure, while meshes stream in, and while the camera orbits with
all of it on screen, then select under the cursor, move that part, move its whole
module (20,000 occurrences), hide and show both, colour the module, and undo everything.

```
soliddraft asm generate scale --unique 100000 --placements 1000000 --modules 50
soliddraft asm bench scale/root.dwasm --frames 120 --json
```

A frame's time is all of it end to end: taking in what the loader found, updating the
scene, uploading, culling, drawing the colour and id passes, reading back the pick
window, and waiting for the GPU. The machine was shared with other builds, so each
frame also records how long its thread waited for a core (Linux schedstat). The
"own" columns leave that wait out; the bench counts a stall only when a frame's own time
is over 100 ms.

Measured on 2026-10-15, RTX 2070 Max-Q (Vulkan), 12 threads with a load average of about 17 from other builds, 1280 x 800, release build:

| Phase | Result |
|---|---|
| Generate | 32 s (parts 14 s, assemblies 19 s); root 85 MB, parts 391 MB on disk |
| Read the structure (background) | 2.4 s; 248 interface frames meanwhile, own time max 6 ms |
| Stream 100,000 meshes | 7.1 s over 103 frames; own time p50 36 ms, p95 42 ms, max 99 ms; wall p50 64 ms, 11 frames over 100 ms waiting for a core |
| Orbit, everything on screen | own time p50 35 ms, p95 43 ms, max 45 ms; wall p50 45 ms, max 82 ms |
| Select under the cursor | 0.01 ms; its frame 24 ms |
| Move one part (an override) | 0.2 ms; frame 24 ms; 64 bytes uploaded |
| Move a module, 20,000 occurrences | 2.1 ms; frame 26 ms; 1.28 MB uploaded |
| Hide, show a module | 0.4 ms each; frames 26 to 27 ms |
| Colour a module | 0.3 ms; frame 26 ms |
| Undo all seven | 4.5 ms; frame 32 ms |
| Bounding volume hierarchy | built in the background, ready when loading ends; a ray 0.01 ms |
| Frames over 100 ms of their own | 0 |
| Memory | 1.07 GB resident loaded, 1.11 GB at the end; occurrence table 232 MB, definition store 237 MB, scene 66 MB; 346 MB on the GPU |

The interface pays for what it shows, not for the size of the assembly: edits touch
only their own rows and instances, and drawing is bounded by what is on screen.

## Command line

```
soliddraft asm new <file.dwasm> [--name <name>]
soliddraft asm info <file.dwasm> [--json]
soliddraft asm tree <file.dwasm> [--depth <n>] [--json]
soliddraft asm insert <file.dwasm> <part.dwprt | sub.dwasm> [--at x,y,z] [--rotate ax,ay,az,deg]
soliddraft asm pattern <file.dwasm> <component>... --linear dx,dy,dz --spacing <mm> --count <n>
soliddraft asm pattern <file.dwasm> <component>... --circular ox,oy,oz,ax,ay,az --angle <deg> --count <n>
soliddraft asm move <file.dwasm> <path>... (--to x,y,z | --by x,y,z) [--rotate ax,ay,az,deg]
soliddraft asm hide|show|suppress|unsuppress <file.dwasm> <path>...
soliddraft asm colour <file.dwasm> <path>... (--rgb '#rrggbb' [--metallic <0-1>] [--roughness <0-1>] | --none)
soliddraft asm quality <file.dwasm> [<path>... (--set <quality> | --none)] [--every <quality>] [--json]
soliddraft asm rename <file.dwasm> <component> <name>
soliddraft asm delete <file.dwasm> <component>...
soliddraft asm rebuild <file.dwasm> [--search <folder>]... [--json]
soliddraft asm generate <folder> [--unique <n>] [--placements <n>] [--modules <n>] [--no-part-files] [--seed <n>]
soliddraft asm bench <file.dwasm> [--size <w>x<h>] [--frames <n>] [--json]
```

`soliddraft doc rebuild` on an assembly is `asm rebuild`; `doc info`, `doc refs` and
`doc where-used` read assemblies like any document. `asm info` reports how long each
opening phase took and roughly how much memory the open assembly holds.
Both rebuild commands use the same transactional Update as the app. A refused Update
exits with status 1 and writes nothing to the assembly file.

## In the app

The desktop app opens one assembly at a time, in a tab beside the part
(`crates/soliddraft-app/src/assembly.rs`). While that tab is active the ribbon, the tree,
the property panel and the status bar are the assembly's.

- **File > New assembly** starts an empty assembly. **File > Make assembly from part**
  saves the open part if it has unsaved changes, then starts an assembly that places it
  at the origin. The component is floating. The `make_assembly` MCP tool requires a
  saved part without unsaved changes.
- **Insert component** (Ctrl+I) asks for part or assembly files; dropping files on the
  window while the assembly is active does the same. The first component goes at the
  origin; each later one sits 20 mm beside what is already placed, along x. Each is pinned
  at its file's revision now. Before the assembly's first save it has no folder to resolve
  references from, so the app hands the loader each inserted file's bytes through its
  revision store (`AssemblyDocument::new_with` with `OpenOptions::revisions`). The first
  save stores every reference relative to the new file.
- **Update** accepts every source together or keeps the previous assembly with a repair
  message. Undo and redo act only on the active document.
- **The tree** lists the components by `label`, with sub-assemblies opening to their own;
  a failed or stale definition shows its problem. A click in the tree or the viewport
  selects a component of the root.
- **The property panel** places the selection: x, y and z in millimetres and rotations
  about x, then y, then z in degrees, relative to the assembly that holds it. A change
  applies with `move_to` when the field lets go, as one undo step.
- **Save assembly** (Ctrl+S) and **Save assembly as** (Ctrl+Shift+S) write the `.dwasm`
  with its display copies; **File > Open** and Open recent open one with
  `open_in_background`. Undo and redo follow the active tab.

Mates and constraints between components are the next slice; placements stay fixed until
then. The browser build leaves these commands off, because the loader needs threads.

## For the interface

The Assembly tab, its tree and Insert component build on:

- `AssemblyDocument::open_in_background(path, OpenOptions)`, then `Opening::try_take()`
  each frame until it yields the document and its `Scene`; then `poll(budget)` each
  frame, `Scene::update` and `Scene::take_new_meshes(max)`; `is_loading()` for a
  progress indicator; `bvh_ready()` for exact queries without waiting;
- the tree: `table().children(row)`, `label(row)`, `table().flags(row)`, `table().path(row)`,
  and for a row's definition `store().get(def)`: its `state` (the loading state) and
  `freshness` (the "newer version available" and missing markers);
- Insert component: `insert(doc_ref, placement)`, with the `DocRef` pinned at the
  inserted document's current revision;
- placement drag and numeric entry: `move_to` and `move_by`;
- the viewport: a `Scene` updated from the document each frame, handed to the renderer
  as `viewport::assembly::AssemblyFrame` in `FrameInput::assembly` with the meshes that
  arrived, the levels from `Scene::take_new_levels`, each new definition's
  `Definition::deviations` and the device budget; picks come back through
  `PickSample::texel`, `Scene::decode` and `AssemblyDocument::selection`, and the levels
  the GPU wanted through `ViewportRenderer::take_assembly_wanted`, for
  `AssemblyDocument::request_levels`;
- colours: `set_colour` with an `Appearance` from the material library;
- Rebuild: `rebuild()`, whose report fills the update banner;
- where-used: `soliddraft_document::FolderIndex`, as for drawings.
