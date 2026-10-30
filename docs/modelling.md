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
| Extrude end | blind, through all, up to the next boundary, or up to a face by persistent name |
| Fillet, Chamfer | edges by persistent name, radius or distance |
| Hole | a planar face by persistent name, centres in the face's sketch frame, diameter, blind depth or through, flat or drill-point tip, counterbore or countersink |
| Linear and circular pattern | a direction and spacing, or an axis and angle, and the number of copies; copies the body and unites the copies with it |
| Mirror | a reference plane or a planar face, and whether the original stays |
| Draft | faces by persistent name, a neutral plane (reference plane or planar face), an angle and the pull's sense |

A part has one body. The first boss makes it, later bosses join it, and cuts remove from
it. Every closed region of the sketch is swept; four circles make four holes. Through all
reaches past the far side of the body's bounding box.

A sketch on a face of the part lies on the face's plane with its normal pointing out of
the material. Its X axis is horizontal when the face is not, and the world X axis
projected onto the face when it is; its origin is the world origin projected onto the
face. A boss or cut from such a sketch starts exactly on the face, forwards or
reversed; the tool is never moved off the face, and the kernel's coplanar Boolean rules
decide the face the tool and the part share.

A hole's faces are named by the hole's place in the point list and the part of the hole: entry, counterbore wall and floor, countersink, wall, floor or tip. A pattern's or mirror's copy faces are named by the copy number and ranked by the names of the faces they copy, so editing the spacing, the count or an earlier feature keeps them. A drafted face keeps the name of the face it tilts. An extrusion up to a boundary starts on its sketch face like every other extrusion.

Mass properties are requested explicitly from Inspect, Mass props, after selecting
a solid feature. Editing or displaying the hole does not request this proof. The
kernel request includes volume, surface area, centre of mass and centroidal
unit-density inertia. The application response exposes volume, area and centroid
as midpoints of the kernel's proven enclosures. A measurement does not alter the
hole's feature definition or history.

`ci/scenarios/drill-point.json` keeps the 118-degree drill-point build and its
diameter/depth edit under the ordinary 20-million-work policy. Its volume checks
request full mass properties only after the changed body has reached the viewport.
The complete requested measurement has its own elapsed time, separate from the
hole click. Both the original and edited case must retain their existing values.

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
changes in it. An assistant proposal's dry run uses a fork and keeps its new builds
until Apply or Discard. Apply imports those builds by exact inputs instead of building
the same geometry again. Import translates the fork's local sketch-build versions;
an intervening edit cannot make an unrelated cached sketch match. Undo still records
the actual commands, and a body is released when the last cache holding it lets go.
The application's proposal copy also shares the current display buffers and cached
LOD meshes, so a sketch-only dry run does not convert or pack the solid's mesh again.

Asynchronous consumers use `execute_deferred`, `undo_deferred` and `redo_deferred`
to change definitions through that same command stack without running the kernel.
`is_dirty()` identifies definitions that have not been rebuilt; body and status getters
still describe the preceding rebuild. A worker rebuilds a fork of the pending document.
After rejecting an obsolete application revision, the caller passes its completed fork
to `rebuild_reusing`. Adoption requires the same document and kernel, a completed
source, and bit-exact feature and rollback definitions. It translates the fork's local
build versions and makes those builds live before trimming the cache. No geometry is
evaluated again, even with a zero-byte reuse budget. History, identity, materials and
settings stay on the receiving document.

Export refuses dirty definitions rather than emitting their preceding body. Saving
authored definitions is still possible, but omits display caches while dirty so an old
mesh cannot acquire the pending definition's geometry hash. Deferred transitions and
explicit `rebuild` operations have distinct journal intents; replay preserves the
unbuilt interval and its export refusal.

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
same generator, ranked by the sketch region that made them. An edge is named by the two
faces it separates.

The origin is the identity. Catallax records on every face the operations that made it
and the part each one played (its role), and keeps them through later operations: every
piece of a split face keeps the origin, and a face merged from coplanar faces carries
all of their origins. The kernel interface reports them as `FaceInfo::origins`, with the
document's operation tags. On every rebuild each feature says which origin each of its
names stands for, and a name resolves to the faces of the input body that carry that
origin. Renames, splits, merges and dimension edits keep a reference without following
history or comparing geometry. No face role is guessed: a face whose role the interface
cannot express has no name.

What a reference needs from those faces depends on its use:

- a sketch plane needs one plane, so it accepts the pieces of a split face when they lie
  in one plane;
- an appearance follows every piece;
- a drawing anchor or a fillet edge needs exactly one face or edge.

The result is checked against a fingerprint taken when the reference was made: the
surface or curve type must not change, and a planar face must still face the same way.
The fingerprint never chooses a face. Its position and size only rank the candidates
offered when a reference breaks.

A reference that resolves to no entity, to several, or to one that fails the check stops
its feature with `RepairNeeded`: which reference broke, why (`Missing`, `Ambiguous`,
`Mismatch`), and candidates ranked by fingerprint. It never re-attaches silently, picks
the nearest face or guesses an identity from geometry. A repair is an ordinary edit with
a new reference.

`Document::face_ref` and `Document::edge_ref` make a reference from a picked kernel id,
and check that it resolves back to that entity. `Document::plane_ref` makes a sketch
plane reference and also accepts a piece of a split face. `Document::face_label` names a
face for people, such as "Boss-Extrude1 · End cap".

`tests/references.rs` checks survival on the bracket. It edits the plate thickness and
the boss diameter, renames features and the Top datum, splits the plate's top with a
groove, merges it with a flush tab, suppresses and restores the groove, undoes and redoes,
and saves and reopens the part. Every reference survives these steps. Deleting the boss
then raises the repair prompt for the bore's missing plane instead of choosing another.
The same steps run through MCP as the `references` application scenario
([application-loop.md](application-loop.md)).

Datum references store `DatumPlane`, never its display name. `RenameDatumPlane` changes
the label without rebuilding geometry; it is undoable and saved in `part.json` as
`datum_names`. Files without labels use Top, Front and Right. Labels affect the semantic
hash, not the geometry hash.

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

Application file, byte-upload/recovery and MCP loaders can seed the new document's
existing build cache from the current model. Reuse requires the same document id and
kernel instance, preserves the source's retention budget, and still runs the ordinary
rebuild checks against the incoming definitions. Authored input is journaled before
kernel work; imported builds become live before trimming, so a zero-byte retention
budget does not discard the current tree during handoff. The new cache shares ownership
of retained bodies, not the source's history or built statuses. Changed inputs invalidate
their dependent builds; names and refusals are resolved by the new rebuild.
`Document::from_bytes` and `Document::open` remain cold constructors for fresh replay.


## Exit test

`crates/soliddraft-model/tests/exit_test.rs` builds the sample L-bracket through the command layer, saves and reopens it, edits its base thickness and checks undo, redo and reference repair. Its saved inside fillet builds through Catallax's elementary fillet strips. Supported downstream features keep building.

The document tests also reopen a part containing a chamfer and check that it rebuilds to the same body. Ribbon tests check that waiting commands stay visible, stay disabled and name their stages, and that the switched-on commands are enabled. These run in the workspace CI suite. The application scenarios in `ci/scenarios` build and edit each switched-on feature through MCP.

## Terminal cubic lattice

With a finished body, choose Lattice on the Features ribbon. Cell size and density are in millimetres; density is the constant beam radius. The cubic grid uses the part's origin and axes. Accuracy and minimum detail control certified STL export. The viewport and tree label the displayed mesh as an unproved preview.

The lattice stays last. Edit an earlier feature to rebuild it, or return before the lattice to continue CAD work without losing its settings. The `lattice` and `edit_lattice` agent verbs use the same document commands. Old two-setting lattice files open with their earlier body visible and a message naming the missing cell size and density. Configure those inputs to build the field.

## Terminal network gyroid

With a finished body, choose Gyroid on the Features ribbon. The body becomes the part of it where `sin X cos Y + sin Y cos Z + sin Z cos X` is at most the level, with `X = 2πx/a` and `a` the period. Period, accuracy and minimum detail are in millimetres. The level is a dimensionless constant, not a wall thickness or a volume fraction, and zero and negative levels are valid. The gyroid uses the part's origin and axes. The viewport and tree label the displayed mesh as an unproved preview, and the part has no field volume or mass.

A gyroid stays last like a lattice, and a part has one terminal field. The `gyroid` and `edit_gyroid` agent verbs use the same document commands. Only the network with a constant level is available. Sheet gyroids, graded levels and position functions come later.

Export can refuse a valid gyroid. The certified STL must be one closed, connected surface, and some levels leave separate pieces. At period 10 mm and level 1/2, the box from (-10, -10, -5) to (10, 10, 15) leaves a separate corner piece, so the export refuses instead of dropping it.

## Named values

The `set_value` tool creates or changes a named expression. `drive` binds a sketch
dimension, numbered from 1, or an extrusion's depth to that expression. For example,
`plate_w = 140` can drive Sketch1 dimension 1 and `plate_w / 2` can drive a depth.
`rename_value` preserves references and updates their displayed text.

Values and bindings persist in the part and participate in undo. A value edit resolves
all its dependents before committing. A cycle, unknown unit or incompatible driver
unit leaves the accepted document unchanged. Change a driven input through its value.

Bare literals use millimetres; bare factors in multiplication and division are scalars.
Length literals support mm, cm, m and in, with exact decimal conversion before rounding
to the stored number. A plain `6 in` is stored as `152.4 mm`. Angle literals use deg.
Expressions with names or operators keep their text. Names are ASCII identifiers,
excluding unit symbols. This increment supports 256 values, dependency depth 32 and 64 tokens per expression.
Automatic dotted names, functions and configuration overrides remain for later slices.
