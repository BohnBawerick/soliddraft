# Part export

SolidDraft exports B-rep parts as STEP AP242, STL, 3MF or OBJ. The code is the `soliddraft-exchange` crate. The kernel call is `Kernel::export_step`, described in [kernel-bridge.md](kernel-bridge.md). Every export returns the file and a report of how far the file may lie from the exact part.

## Formats

| Format | Made from | The report's bound |
|---|---|---|
| STEP AP242 (`.step`, `.stp`) | the exact B-rep, written by Catallax's exporter | Catallax's proven largest distance between written and exact geometry, rounded up |
| STL (`.stl`, binary) | the kernel's mesh at the part's export tolerance | the export chord height |
| 3MF (`.3mf`, core specification) | the same mesh, which must be closed | the export chord height |
| OBJ (`.obj`, Wavefront) | the display mesh, retaining face groups and normals | the export chord height |

STEP writes exact planes, cylinders, cones, spheres, tori, lines and circles. Intersection curves that have no exact STEP form are fitted splines, and the report counts them. The requested tolerance is 0.0000001 mm. If decimal rounding or spline fitting cannot meet it, the CLI refuses the whole export batch before writing any file. The app still writes the file, reports the bound over tolerance with Catallax's warning, and shows the message as an error.

Meshes use the export tolerance in the part's `settings.json`, 0.01 mm and 10 degrees by default; see [tessellation.md](tessellation.md#the-four-tolerances). The chord height is proven after rounding to the file's 32-bit coordinates. Catallax stage 1 certifies the chord height only, so the report lists the angle as a control it could not guarantee.

## Meshes

The kernel's display mesh repeats a position on every face that meets there, so each face keeps its own normals. Catallax's meshes share edges exactly, so `mesh::weld` merges vertices whose 32-bit coordinates are bit for bit equal. That restores the shared mesh without a distance tolerance. A triangle left with a repeated vertex has no area and is dropped.

The mesh report says whether the welded mesh is closed: every edge joins exactly two triangles that run it in opposite directions. 3MF requires a closed mesh, so a 3MF export of an open mesh is refused. STL and OBJ have no such rule; their reports still say whether the mesh is closed.

Binary STL headers never start with `solid` in any case, because some readers then take the file for ASCII STL. 3MF packages carry a fixed timestamp, so the same mesh always gives the same bytes.

OBJ writes the part name in an `o` record and the source face IDs in `g face_<id>` groups, following Catallax's OBJ convention. It writes the existing display mesh through the same kernel boundary as STL and 3MF, without rebuilding a Catallax mesh just to call its writer. Per-face vertices and `vn` records preserve sharp-face normals. Coordinates round-trip to the display mesh's 32-bit values, and triangles retain their outward winding. A comment records the chord height in millimetres. OBJ has no standard unit field, so the header states millimetres. Control characters in the part name become spaces to keep the name on one line. As with STL and 3MF, triangles collapsed to repeated positions are dropped.

## Terminal fields

An active lattice or gyroid exports only as binary STL, through the native field stage. The stage certifies one closed, connected surface at the feature's accuracy and minimum detail settings. Its `catallax-print-stl/1` header records the field route, exact requested accuracy, caps and proven distance bound. The `field_mesh` JSON report names the label, marks the result approximate and includes the exact requested accuracy and retained minimum detail, the composed bound, the topology verdict (closed, free of self-intersection, connected, components), mesh counts, the cell and vertex caps, the deterministic work the export used and the file SHA-256. The viewport remains an unproved preview. STEP, OBJ and 3MF are refused while the field is active. Returning before the field restores ordinary B-rep export.

A field's mass stand-in is a separate STEP output, written only when asked for: the Mass stand-in command, the `export_stand_in` tool or `soliddraft export part.dwprt --stand-in mass.step`. It is never the field's boundary and never stands in for a refused field export. The file holds the field's source region as one massless `SHELL_BASED_SURFACE_MODEL` envelope and one or two unit-density `MANIFOLD_SOLID_BREP` proxy blocks, with no density or material metadata. Catallax first proves the field's own moments to 0.01 mm³, 0.001 mm and 0.1 mm⁵; the proxy's exact moments, and then its moments after the file's coordinate rounding, must lie inside those enclosures, or the export is refused (`stand-in.properties-not-matched`, `stand-in.exchange-not-enclosed`) before any byte is written. A field read through a subtracted operand is refused with `field.stand-in-envelope-coverage`. The `stand_in` JSON report is labelled "mass stand-in — not field boundary geometry" and carries the field enclosures, the proxy's exact moments, their exchanged bounds, the inertia residual, the body counts, the file's rounding bound, the deterministic work and the file SHA-256. The proxy's file is the same, byte for byte, as Catallax's public `catallax-stand-in/1` request answers for the field's native recipe. A consumer that sums the mass of the solids alone gets the field's mass properties; the envelope carries none. `soliddraft export --max-work <units>` runs the whole request, opening and rebuilding the part included, on one deterministic allowance, refuses with `numbers.operation-budget` and writes nothing when it runs out, and adds the work used to each `--json` line.

## Refusals

A part with a feature that needs a repair, is blocked or failed is refused, so no file shows a body the part does not have. Suppressed and rolled-back features are fine; the export is the body the tree shows. A refused export writes no file. The CLI also refuses a batch containing any over-tolerance STEP report before creating or replacing any destination, including other formats in that batch.

## Where to export

- In the app, File > Export offers STEP AP242, STL, 3MF and OBJ. The status bar shows each export's report in one line.
- On the command line, `soliddraft export <part.dwprt | recipe.json> --out bracket.step --out bracket.3mf --out bracket.obj` writes each file; `--json` prints each export report. It does not measure the body for acceptance checks.

## The OpenCASCADE check

Hosted CI's STEP job exports the bracket and opens its STEP in OpenCASCADE through `ci/check-step.py`. The check passes when OpenCASCADE reads one valid solid with the face count of the report and of the exact body less the report's `removed_faces`, removed counts that are whole numbers from 0 to the exact body's face and edge counts, and a volume within the reported deviation of Catallax's exact volume. A surface moved by at most `d` changes the enclosed volume by at most `area * d` to first order, so the allowance is that product, plus OpenCASCADE's own integration error estimate and the exact volume's measurement width.

OpenCASCADE and the exact-body acceptance measurements are test tools only. The application never links or runs OpenCASCADE. To capture the bracket and its measurement report locally, run `SOLIDDRAFT_STEP_PROOF_DIR=/tmp/bracket-proof cargo test -p soliddraft-exchange --test bracket step_carries_the_proven_deviation_and_the_body_faces -- --exact`, then `python3 ci/check-step.py /tmp/bracket-proof/bracket.step /tmp/bracket-proof/step-report.json`.
