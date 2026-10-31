# Measure and track

Open Inspect > Measure and pick an edge or face. The panel shows the kernel's proven length in mm or area in mm2 and labels it exact or bounded. A bounded result carries an enclosure. The MCP `measure_entity` tool returns its outward-rounded endpoints, part frame and geometry hash.

Enter a value name, or leave it blank for an automatic name, then press Track this. An optional check takes `<`, `<=`, `>` or `>=` and one limit. Bare limits use the measured unit. Length limits also accept the value model's length units. A check passes or fails only when the whole enclosure proves that answer. Otherwise it is unknown.

Tracked rows follow persistent geometry references through edits and feature renames. Each accepted rebuild re-proves the requested quantities. A missing source, pending proof or kernel refusal marks its row stale and its check unknown. The last measured number stays visible for that session. A stale row never blocks the edit or save. Reopening re-proves the definitions; a source still missing has no cached number.

Row definitions share the named values' namespace and undo history. Measured numbers live outside document truth, so reading or re-measuring changes neither revision nor geometry. Track changes the revision and leaves geometry unchanged. `rename_value` also renames tracked rows. Tracked names cannot drive geometry or appear in expressions in this batch.

MCP example:

```json
{"name":"block_edge","entity":"Boss-Extrude1 · Edge 9","check":"< 12 mm"}
```

Pass that object to `track`; pass only `entity` to `measure_entity`. `model_summary.values` reports the row's current source label, certainty, bounds, frame, proof geometry, stale reason and check state.

## Pairs and midpoints

With Measure open, hover a straight edge: within eight points of its midpoint the viewport marks the proven midpoint and labels its part-frame coordinates. The id buffer chooses the edge; the coordinate is the kernel's arc-length midpoint proof, never the pick ray or a display polyline. Click there to pick the point, or anywhere else on an edge or face to pick the entity. A single midpoint pick shows the coordinates beside the edge's length.

A second pick proves one scalar between the two, in the part frame, and a third replaces the second. Choose the quantity in the panel:

| Quantity | Supported pair | Result |
|---|---|---|
| Minimum distance | Two finite straight edges; two parallel planar faces of the part | The finite minimum, respecting trims and holes. Never a supporting-plane gap alone |
| Midpoint distance | Two straight edges | The distance between their proven midpoints |
| Direction angle | Two straight edges proven parallel or perpendicular | Exactly 0 or 90 deg, ignoring edge orientation |

Other pairs are refused by name and never estimated: `measure.unsupported-pair` for a mixed or mismatched pair, `measure.unsupported-curve` for a curved edge (including a closed rim's midpoint), `measure.nonparallel-faces`, `measure.nonplanar-face`, `measure.angle-domain` for an oblique angle, `measure.degenerate-edge`, and `measure.bounds-incomplete` for a proof with accuracy marks or no converged upper bound. Kernel causes such as work exhaustion stay in the message.

Track this saves a pair as one row: its quantity and both persistent references, with any midpoint selectors. The row's unit follows the quantity, mm or deg; an angle check takes a degree limit, and a bare limit on an angle row is in degrees. Either end going missing, as when a fillet removes an edge, makes the row stale with `measure.unresolved-reference` and the end's name; no nearby edge takes its place. A stale pair names both ends as they were defined. Undo, save and reopen keep the definitions; proofs are always fresh.

MCP: `measure` takes `a`, `b`, an optional `quantity` (`minimum_distance` by default, `point_distance`, `direction_angle`) and, for `point_distance`, `a_point` and `b_point` set to `midpoint`. Its result names the quantity, unit, value, bounds, certainty, frame, geometry, `source_revision` and both sources with their persistent references. `distance_mm` repeats a proven minimum distance and `gap_mm` a face gap, for the earlier callers of this tool; a refused pair returns no number. `measure_entity` with `quantity: midpoint` returns one enclosure per coordinate. `track` takes `entity_b`, `quantity`, `a_point` and `b_point` for a pair:

```json
{"name":"diagonal_gap","entity":"Boss-Extrude1 · Edge 4","entity_b":"Boss-Extrude1 · Edge 7","quantity":"minimum_distance","check":"< 32 mm"}
```

## Sketch guides

While drawing, the sketcher snaps to points, line midpoints, level or plumb alignment with the previous point and curves, in that order, and every snap a click lands becomes the relation it showed: a line started on a midpoint keeps a Midpoint relation, and one drawn plumb keeps Vertical. These are 2D sketch relations, not references to the solid. The Auto relations switch in the viewport corner turns this inference off: the point then stays under the pointer and nothing is inferred, while existing and hand-added relations still solve. Draw construction makes new entities construction geometry, outside the profile. Neither switch changes the document or adds an undo step.

Other frames, oblique angles, curved pairs, cross-component picks and row removal remain follow-up work.

The pre-tracked-values fixture `crates/soliddraft-model/tests/fixtures/bracket-inch.dwprt` is copied from the `application-loop` artifact of hosted run 41034351115 on V1 merge 2 merge commit `aec6be4bc0aacc7369a1f8b778f6c3b6f9aa9fa0`. Its recorded hashes are checked without regenerating it.

The batch 1 fixture `crates/soliddraft-model/tests/fixtures/measure-batch1.dwprt` is the `measure` scenario's `measure.dwprt` from the `application-loop` artifact of hosted run 41137611800 on main `3403d2134d5f1d8e76e6f2d143eedd48648fdd5f`. U3 opens and re-saves it without regenerating it.
