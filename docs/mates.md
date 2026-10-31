# Mates

A mate is an authored relationship between components. Solving it writes placements, and a
component's placement stays the truth of where it is. Mates, values and their expressions
are saved in `assembly.json`. Proofs, solver status and diagnostics are derived and never
saved. The code is `crates/soliddraft-assembly/src/mates/`; the commands and the Mate panel
are in `crates/soliddraft-app/src/mates/`.

## What this version supports

| Family | Fixes |
|---|---|
| Origin | A component's origin on the assembly's. With `align_axes` its axes match too |
| Distance | The relation between two planar faces' normals, and one translation |

Faces must be axis-aligned planes with rational offsets, on parts in the default
configuration. The commands below name the root's own part components; a mate read from a
file may also name a part inside a sub-assembly by its path, as forming one leaves it (see
[subassemblies.md](subassemblies.md)). A face placed off the world axes is refused. Only
identity and exact half-turn placements carry the exact claim below: a quarter turn is
accepted when its normals round onto the axes, but a later general rotation needs a bounded
transform contract and does not inherit this one.

A file may carry a configuration on an endpoint or on the reference a mate reads. It is kept as
read and refused as `unsupported_configuration`: nothing solves on the default instead, and an
unmated reference is left alone.

Anything else is refused by name. Nothing falls back to a mesh, a bounding box or a displayed
centroid.

## In assembly.json

```json
{
  "mates": [
    { "kind": "origin", "id": 5, "name": "O", "component": 2, "align_axes": true },
    {
      "kind": "distance", "id": 8, "name": "Z",
      "a": { "path": "2", "document": "3f0c...", "face": { "feature": 3, "generator": { "kind": "end_cap" } } },
      "b": { "path": "4", "document": "91ab...", "face": { "feature": 3, "generator": { "kind": "start_cap" } } },
      "alignment": "opposed",
      "distance": { "text": "gap", "tree": { "Ref": 9 } }
    }
  ],
  "values": [
    { "id": 9, "name": "gap", "expression": { "text": "1.2 cm", "tree": { "Number": [12.0, 1, 0] } } }
  ]
}
```

- **Endpoints** name a face by occurrence path, part document, optional configuration and
  the part's persistent `FaceRef`, which is a feature id and what that feature made. Labels
  and kernel handles are never saved as identities. The face reference's own label and
  fingerprint are kept as written; only a repair message reads them.
- **Alignment** is the vector relation between the two outward normals: `aligned` when they
  agree, `opposed` when they oppose. The window may call one of them flipped. The saved word
  is always one of these two.
- **Distance** is signed, measured from `a`'s plane toward `b`'s along `a`'s outward normal.
  These are supporting planes: the faces need not overlap, so this is not the minimum
  distance between two trimmed faces unless their projections overlap.
- **Values** use the part's expression grammar, units, decimal conversion and cycle checks
  (`soliddraft_model::ValueTable`). There is one evaluator. A value keeps its text as typed.
- **A driver** is a distance whose expression reads a value. It reads by stable id, so
  renaming the value changes the text and nothing else. A distance has one expression, so
  it has one driver; replacing it is an explicit request.
- **Mates of a later version** are kept exactly as read. While one is present, `solver.state`
  is `error` and nothing that would solve can commit, Update included.

The geometry hash leaves out `mates` and `values`, because the placements they decide are
hashed. An assembly with neither writes neither key, so old files keep their hashes.

## Solving

Plane normals are exact signed axes, which splits a solve in two:

1. **Rotations.** An Origin mate with axes fixes a rotation. Otherwise each distance relates
   two normals, and rotations spread out from what is fixed. Two perpendicular normals fix a
   rotation exactly. One leaves its spin alone and turns as little as it can.
2. **Translations.** Each distance becomes an exact difference of one world coordinate, so
   each axis is a graph of differences. Walking it places everything connected to the
   origin, or to the group's first component. An edge that disagrees by more than 1e-9 mm is
   an over-constrained set of mates, and every mate on that cycle is named.

An unconstrained coordinate keeps its authored value. A group with no Origin mate keeps its
first component where it is, because something has to stay put, and it still reports its
freedoms. Nothing is ever a hidden fixed flag.

The root solves its own mates and those of every sub-assembly in it, each with the
sub-assembly's path in front, over part occurrences at any depth. A sub-assembly's frame
stays where it is authored; a part inside it that a solve moves is kept as an override in
the root, never written to the sub-assembly's file.

Freedoms come from the rank of the rigid-body constraint Jacobian at the solved pose, so a
shared rotation about an anchored origin counts. Every part occurrence that moves on its own
has six, wherever it is in the tree; a pattern member follows its seed. A partial mate is valid: mates go on one at
a time. `require_fully_constrained` refuses a result with any freedom left.

## Commands

| Command | What it does |
|---|---|
| `insert_component` | Places a file, optionally at a placement |
| `assembly_measure_planes` | Reads the proven signed distance between two faces' planes |
| `assembly_mate_origin` | Adds an Origin mate |
| `assembly_mate_distance` | Adds a distance mate |
| `assembly_edit_mate` | Changes a literal distance, an alignment or `align_axes` |
| `assembly_delete_mate` | Removes a mate; components stay where they are |
| `assembly_set_value` | Adds or changes a value |
| `assembly_rename_value` | Renames a value |
| `assembly_drive_mate` | Binds a distance to values |
| `assembly_update` | Accepts the current source revisions and solves on them |

A face is named by its label, or by `feature`, `generator` and, for a side, `normal`. Each must
find exactly one face. The stored result is the `FaceRef`.

Every change runs one transaction: copy the truth, change the copy, prove and solve it, then
publish truth, placements and display as one undo step. A refusal changes nothing and keeps
the redo stack. Refusals are JSON: `refused` holds the code, with `message`, `mates`,
`remaining_dof` and `free` where they apply.

| Code | Why |
|---|---|
| `under_constrained` | A strict request left freedom |
| `over_constrained` | The mates cannot all hold |
| `dangling_reference` | A stored face is gone at the proposed revision |
| `ambiguous_reference` | A selector or reference finds several faces |
| `no_such_face` | A selector finds nothing |
| `unsupported_configuration` | A mate or its source names a configuration other than the default |
| `unsupported_plane` | The kernel will not prove this face's plane |
| `unsupported_placement` | A placed normal is off the world axes |
| `not_parallel` | The two planes are not parallel |
| `proof_unavailable` | The pinned revision's bytes or a kernel are not available |
| `unknown_mate` | The assembly holds a mate this version cannot solve |
| `driven_input` | A literal edit of a driven distance |
| `already_driven` | A second driver, without `replace` |
| `unit_mismatch`, `value_cycle`, `invalid_value` | The expression cannot be used |
| `too_large` | More than 128 mated components |

## Proofs and Update

A proof belongs to a document, a revision and a face. It comes from the part rebuilt from that
revision's bytes on the kernel, through `Kernel::face_support_plane`, which reads the native
plane definition exactly and costs no numerical work. Gaps are computed in rationals from
those offsets and the stored placements. `measurement.rational` holds the exact value;
`lo` and `hi` are rounded outward.

Editing or saving a part moves nothing. The accepted pin, pose and proof stand, and a
measurement whose source moved on says `retained`. Update is the only way forward: it loads
every source's proposed revision, proves the mates on them, solves, and commits pins, display
and pose together. A face whose feature is gone refuses Update even when a new feature made an
identical box with the same label, because the reference is to the feature, not the shape.

A proof needs the pinned revision's bytes. They come from the file while it is at that
revision, otherwise from the session's revision store. A file opened after its part moved on
still shows, solves nothing and reports `proof_unavailable` per mate until Update.

## Limits for later batches

Other mate families, picking a face inside a sub-assembly for a new mate, mates on pattern
members, configurations, dragging, general rotations and curved faces. See M6 in the plan.
