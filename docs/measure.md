# Measure and track

Open Evaluate > Measure and pick an edge or face. The panel shows the kernel's proven length in mm or area in mm2 and labels it exact or bounded. A bounded result carries an enclosure. The MCP `measure_entity` tool returns its outward-rounded endpoints, part frame and geometry hash.

Enter a value name, or leave it blank for an automatic name, then press Track this. An optional check takes `<`, `<=`, `>` or `>=` and one limit. Bare limits use the measured unit. Length limits also accept the value model's length units. A check passes or fails only when the whole enclosure proves that answer. Otherwise it is unknown.

Tracked rows follow persistent geometry references through edits and feature renames. Each accepted rebuild re-proves the requested quantities. A missing source, pending proof or kernel refusal marks its row stale and its check unknown. The last measured number stays visible for that session. A stale row never blocks the edit or save. Reopening re-proves the definitions; a source still missing has no cached number.

Row definitions share the named values' namespace and undo history. Measured numbers live outside document truth, so reading or re-measuring changes neither revision nor geometry. Track changes the revision and leaves geometry unchanged. `rename_value` also renames tracked rows. Tracked names cannot drive geometry or appear in expressions in this batch.

MCP example:

```json
{"name":"block_edge","entity":"Boss-Extrude1 · Edge 9","check":"< 12 mm"}
```

Pass that object to `track`; pass only `entity` to `measure_entity`. `model_summary.values` reports the row's current source label, certainty, bounds, frame, proof geometry, stale reason and check state.

This batch covers B-rep edge lengths and face areas in the part frame. Pair measurements, other frames, hover points, guides and row removal remain follow-up work. The older two-entity `measure` tool retains its rounded reference-point output and does not feed tracked rows.

The pre-tracked-values fixture `crates/soliddraft-model/tests/fixtures/bracket-inch.dwprt` is copied from the `application-loop` artifact of hosted run 41034351115 on V1 merge 2 merge commit `aec6be4bc0aacc7369a1f8b778f6c3b6f9aa9fa0`. Its recorded hashes are checked without regenerating it.
