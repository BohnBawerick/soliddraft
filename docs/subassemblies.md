# Sub-assemblies

Form subassembly moves chosen parts of the open assembly into a new assembly file of their
own, and places that file in the root where the parts were. Nothing moves in the world.
The code is `crates/soliddraft-assembly/src/subassembly/`; the command is
`crates/soliddraft-app/src/subassembly/`. The file format is [assemblies.md](assemblies.md)'s
and mates are [mates.md](mates.md)'s.

## What it takes

An ordered list of distinct part occurrences placed directly in the root, the new file's
path, and which of the parts gives the new file its frame. This version forms:

- part components of the root, not sub-assemblies or pattern members;
- in their default state: not hidden or suppressed, without an assembly colour, a display
  quality of their own, overrides, configurations or a pattern seeded on them;
- with mates the new file can hold whole: a distance between two chosen parts moves only
  when its distance is a literal. One that reads a root value, an Origin mate on a chosen
  part, or a mate of a later version is refused, never dropped.

Anything else is refused by name before anything is made.

## What it makes

Let E be the accepted placement of the origin part. The new file's frame is E: each part
keeps its component id and is placed at `E⁻¹ ∘ W` in it, where W is its root placement,
so the origin part sits at the new file's origin and every part's world pose is the same.
The root places the new file once, at E, under a source and a component taken from its own
id counter.

| Reference | After |
|---|---|
| A chosen part | Its id and source move into the new file. The source keeps its id, document, configuration and pin; its path points, relative to the new file, where the root finds the document now, even when that is not where the root's stored path says. Parts that shared a source still share one |
| A mate between two chosen parts | Moves into the new file under the same id, with the same endpoints |
| A mate between a chosen part and another | Stays in the root under the same id; the chosen end's path gains the new component in front, such as `4` to `16/4` |
| Any other mate, every value and binding | Unchanged in the root |
| The root's sources | Only those its remaining components place, and the new file |

The new file's `next_id` is above every id it carries. The command returns the whole map:
each occurrence's path before, after and in the new file, and each mate's id with its
owner before and after (`Migration`), so whatever else names them can follow.

A part no mate holds stays as free as it was: no mate, fixed flag or Origin relation is
added, in the new file or in the root.

## Solving across the boundary

The root solves its own mates and every sub-assembly's, each read with the sub-assembly's
path in front, over the part occurrences they name. A sub-assembly's frame stays where it is
authored: it is a frame, not a body with freedoms of its own. A part inside it that a solve
moves is kept as an override in the root (relative to the sub-assembly, as every override
is), and the sub-assembly's own file is never written. Freedoms count the part occurrences
that move on their own, at any depth; a pattern member follows its seed.

So a root driving a part inside a sub-assembly moves that part and whatever the
sub-assembly's own mates tie to it, and leaves the sub-assembly's free parts alone.

## Publication and history

Both candidate truths are made, flattened and solved before anything is written: the root
with the new file placed in it, and the new file on its own. Neither may move anything.
Committing then checks that the root and every chosen source are still at the revisions the
plan was prepared on, writes the new file with `soliddraft_document::save_new`, which never
replaces a file (even one made a moment before), and only then makes the formed root the
accepted one, in one undo step labelled `Form subassembly`. The root's own file is written
when it is saved.

A refusal at any point leaves the accepted root, its history and every file as they were.

| Code | Why |
|---|---|
| `invalid` | An empty, repeated or unknown selection, an origin outside it, or a name not ending in `.dwasm` |
| `unsupported` | Something the new file could not carry whole (see above) |
| `cycle` | The new structure would contain itself |
| `missing_reference` | A chosen part's document cannot be found; the copy the root shows does not stand in for it |
| `stale_revision` | The root or a chosen source changed after the plan was prepared |
| `destination_exists` | The new file's name is taken |
| `io` | The new file could not be written |

A crash after the new file is written and before the root changes leaves a complete new file
that nothing references, beside the root's file as last saved. Both open; forming again
needs a new name.

## Command

`assembly_form_subassembly {paths, path, origin}` forms the open assembly's parts named by
`paths` (such as `["4", "5", "6"]`) into `path`, with the frame of `origin`. Its result holds
`formed` (the file, the migration and both solver statuses), `child` (the new file's id,
revision and truth) and `selected`, the selection after the move: a selected part is
selected again at its nested path. `model_summary`'s assembly lists every occurrence at any
depth under `occurrences`, keyed by path, with its source, pin, local and root placement,
box, the mates it takes part in and the motions it keeps.

`ci/scenarios/subassembly-form.json` runs the V6 batch 1 design's manifest through MCP.

## Limits for later batches

Forming from inside a sub-assembly, sub-assemblies or pattern members as selected
occurrences, configurations, values passed into a sub-assembly, and dissolving or
reorganising a tree (M10).
