# Modelling (phase 2 plumbing)

Phase 2 makes SolidDraft a parametric modeller: sketches with constraints, extrusions that
add or cut material, fillets and chamfers, persistent references that survive edits,
undo and redo, and a native file. This page covers the crates that do it. The screens on
top of them are described in [DESIGN.md](DESIGN.md).

## Crates

| Crate | What it holds |
|---|---|
| `soliddraft-sketch` | Sketches: points, lines, arcs, circles and construction geometry with relations and dimensions, solved by ezpz behind an app-owned interface, and the closed regions they make. It depends on the kernel interface's profile types only, not on a kernel. |
| `soliddraft-model` | The part document: features, the command layer with undo and redo, rebuild, persistent references, native files. It talks to a kernel only through the `Kernel` trait. |
| `soliddraft-cli` | Adds `soliddraft part [--json] <part.dwprt>...`, which opens part files, rebuilds them and reports every feature's state. |

## Sketches

The sketch model is the one the sketcher was built on, moved into `soliddraft-sketch` so
the model crate stores what the interface edits. Entities own their points: a line has
two, an arc a centre and two ends (counter-clockwise from start to end), a circle a
centre and a radius. Points of different entities are joined by coincident relations,
the SolidWorks and ezpz way, so an entity can be deleted without rewriting its
neighbours. Entity 0 is the origin, a fixed point. Ids are never reused.

| Entities | Relations and dimensions |
|---|---|
| point, line, arc, circle; any of them can be construction geometry | coincident, point on a curve, horizontal, vertical, parallel, perpendicular, tangent, equal, midpoint, concentric, fixed; aligned, horizontal and vertical distance, point-to-line distance, angle, radius or diameter |

`solve::solve(sketch, drag, analyse)` moves the geometry until the relations hold and
leaves a `SolveStatus` on the sketch: empty, under defined (with the entities that can
still move), fully defined, over defined (with the relations that could not hold), or
failed. It never fails on an inconsistent sketch: it keeps the solver's best compromise
and names the conflicting relations. A dragged point is a low-priority wish, so real
relations win. Shape builders (`rectangle`, `rounded_rectangle`, `circle_at`, `slot`,
`polyline_loop`) add fully defined geometry and return the dimensions a user edits.

The solver is [ezpz](https://github.com/KittyCAD/ezpz) (MIT), pinned to commit
`915882cc731da31042ce494c89b10334ee57837a` (release 0.2.29) since it has no tagged
releases. Only `solve.rs` uses it, and no ezpz type is public, so planegcs or a
home-grown Gauss-Newton solver can replace it. It converges to 1e-11, so solved corners
land within 1e-9. A line and an arc that share an end are tangent through a right angle
between the line and the radius at that end: ezpz's line-to-circle tangency lets a
shared end slide along the circle with almost no change in the residual.

`Sketch::regions` turns the non-construction curves into closed regions: ends join
through coincident relations or within 1e-6, loops nest by containment, outer loops run
counter-clockwise and holes clockwise, and regions and loops are ordered by entity id so
the same sketch always gives the same order. A region becomes a kernel `Profile` on the
sketch's plane, and every profile segment remembers the sketch entity it came from. An
arc traversed against its own direction, such as a concave bite out of an edge, becomes
`Segment::Arc { clockwise: true }`.

## Features

| Feature | Parameters |
|---|---|
| Sketch | a plane (Top, Front or Right, optionally offset, or a planar face of the part by persistent name) and a sketch |
| Extrude | a profile (a sketch, or a built-in rectangle or circle on a reference plane), boss or cut, blind depth or through all, reversed or not |
| Fillet, Chamfer | edges by persistent name, radius or distance |

A part has one body. The first boss makes it, later bosses join it, and cuts remove from
it. Every closed region of the sketch is swept; four circles make four holes. Through all
reaches past the far side of the body's bounding box.

A sketch on a face of the part lies on the face's plane with its normal pointing out of
the material. Its X axis is horizontal when the face is not, and the world X axis
projected onto the face when it is; its origin is the world origin projected onto the
face. A boss growing out of such a face, or a cut going into it, starts 0.01 mm behind
the face (`FACE_OVERLAP`), so the tool overlaps the part instead of touching it along a
coplanar face; behind the face is material for the boss and air for the cut, so the
overlap changes nothing. A reversed boss or cut starts on the face.

## Commands, undo and redo

Every change is a `Command`: add, insert, edit, set a sketch dimension, rename, delete,
move, suppress, roll back, or a batch of these as one undo entry. The ribbon, the
assistant (phase 3) and scripts all issue the same commands, and commands serialise to
JSON for the assistant and MCP.

`Document::execute` applies a command, records its exact inverse and rebuilds. Undo is a
stack of those inverses over the document, not over kernel state: undoing an add deletes
the feature, and redoing it puts back the same feature with the same id, so later
entries that name it still apply. A refused command changes nothing; a batch that fails
part-way is unwound.

Moves are refused when a feature would come before one it reads. Deleting a feature is
allowed; the features that read it report what broke.

## Rebuild

A command rebuilds from the first feature it changed, and editing a fillet keeps the
bodies above it. Every feature build is also kept by its exact inputs: the feature, the
exact bytes of its definition, the body it starts from and, for an extrusion, the build
of the sketch it sweeps. Floats count by bit pattern and are never rounded. Kernel bodies are
immutable and a body handle is never reused, so the input body stands for everything
that built it. A rebuild asks for the kept build before building, so:

- renaming a feature builds nothing; checks whose messages name another feature run
  again, without the kernel;
- undo, redo, a value put back, unsuppressing and rolling forward reuse the bodies built
  for those inputs;
- the result is the one a full replay of the saved part gives.

The document owns the kept bodies and releases them when it drops a build. It keeps
them within a budget of retained bytes, 256 MiB unless `Document::set_rebuild_budget`
says otherwise, using the kernel's `retained_bytes` estimate for each body. The builds of the
current tree are always kept; beyond the budget the least recently used others go, with
every build that started from a body they held.

`Document::fork` copies a part and shares its kept builds, so the copy builds only what
changes in it. An assistant proposal's dry run uses a fork. A body is released when the
last document holding it lets go.

Tests count the work, not the rebuild. `soliddraft_kernel_api::counting::Counting`,
behind the `counting` feature, wraps a backend and writes down every operation's tag
and every tessellation. The bracket exit test edits the last feature and checks that only it
was built, that a value put back, undo, redo and renames build nothing, and that each
result, save and reopen included, matches a full replay in statuses, face names,
measures, validation and mesh.

Suppressed features and features below the rollback bar pass the body through
unchanged. A feature's state after a rebuild is one of:

| State | Meaning |
|---|---|
| `Ok` | built |
| `Suppressed`, `RolledBack` | not built, on purpose |
| `RepairNeeded` | a reference did not resolve to exactly one matching entity |
| `Blocked` | an input feature, such as its sketch, needs a repair, failed or is suppressed |
| `Failed` | bad parameters, a missing input, or a kernel error |

Every state but `Ok` passes the body through, so the rest of the tree still builds.

## Persistent references

A feature never stores a kernel id. It names a face by origin: the feature that
generated it, the generator (start cap, end cap, the side swept from sketch entity 5,
the blend of the feature's edge 2) and an ordinal among that feature's faces with the
same generator. The generators come from the kernel's face roles (see
[kernel-bridge.md](kernel-bridge.md)). An edge is named by the two faces it separates.

On every rebuild a name is looked up among the faces its feature generated in this
rebuild, then followed through the kernel's history: split faces become their pieces,
deleted ones drop out. For an edge, both faces are followed and the edge between them is
taken, so an edge survives a split of one of its faces.

The result is checked against a fingerprint taken when the reference was made: the
surface or curve type must not change, and a planar face must still face the same way.
Position and size may change, since that is what editing a dimension does.

A reference that resolves to no entity, to several, or to one that fails the check stops
its feature with `RepairNeeded`: which reference broke, why (`Missing`, `Ambiguous`,
`Mismatch`), and candidates ranked by fingerprint. It never re-attaches silently. A
repair is an ordinary edit with a new reference.

`Document::face_ref` and `Document::edge_ref` make a reference from a picked kernel id,
and check that it resolves back to that entity. `Document::face_label` names a face for
people, such as "Boss-Extrude1 · End cap".

## Native files

A part file (`.dwprt`) is a zip archive with two entries:

- `manifest.json`: the part's id, kind and format, the application and kernel backend
  that saved it, its semantic and geometry hashes and its sections (see
  [documents.md](documents.md)).
- `part.json`: format tag `soliddraft-part/2`, name, units, every feature with its
  parameters, sketch and references, the rollback bar and the next feature id. No
  kernel ids, no computed geometry: it rebuilds on Catallax.

Writing is deterministic and floats round-trip bit for bit, so saving an opened file
gives the same bytes. A file with a newer major version is refused, as is a recipe with
repeated ids or a feature that reads one below it. Saving writes next to the target and
renames over it. Part files from before the manifest (`.dwpart`, `recipe.json` with
`soliddraft-part/1`) still open, with an id derived from their content.

## Exit test

`crates/soliddraft-model/tests/exit_test.rs` builds the sample L-bracket through the command layer, saves and reopens it, edits its base thickness and checks undo, redo and reference repair. Its saved inside fillet now reports its Catallax stage and passes the preceding body through. Supported downstream features keep building.

The document tests also reopen a part containing a chamfer, check the stage 3 rebuild error, and verify that the body before and after that feature is identical. Ribbon tests check that waiting commands stay visible, stay disabled and name their stages. These run in the workspace CI suite.
