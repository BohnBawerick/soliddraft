# Intersect extrusion dependency for U8

This dependency slice starts from SolidDraft main
`f1d50cabe9c5f77739ea7eacc984351dae40494f` and retains its qualified Catallax pin
`a5c42583dd684e54c473bc377267e5e190c37bb6`. The separate canonical P4 kernel
checkpoint remains on `dot/l08-u8-p4`; it is not full application acceptance.

## Behavior and compatibility

The Intersect ribbon command opens the existing extrusion editor with depth and
flip controls. It keeps the material shared by the current body and one closed
profile extruded to a fixed depth. A selected unconsumed sketch supplies the
profile; otherwise the editor offers the existing built-in rectangle/circle.
The preview displays the actual common material, and editing previews against
the body before the edited feature using the existing lifecycle.

The model serializes `ExtrudeOp::Intersect` as `"intersect"`. The existing
`"boss"` and `"cut"` spellings and dispatch remain unchanged. The app's
`ExtrudeParams.intersect` defaults to false when absent from older serialized
parameters. Native files persist the model operation; reopen restores the
Intersect title and behavior. Intersect uses the exact extrusion start plane,
without the boss/cut face-overlap extension.

The strict script/MCP verb `intersect` requires `sketch`, positive `depth`
in mm, and `flip`. Existing `edit_extrude` changes its depth/direction while
retaining the intersection operation. Inspection reports type `intersect`.
Neither construction nor preview requests mass measurement.

## Defect ledger

| Item | Previous behavior | This slice |
| --- | --- | --- |
| Authored intersection | No persistent operation or command | Explicit model operation, command/schema and ribbon editor |
| Extrusion Boolean dispatch | Boss union; otherwise subtract | Explicit Boss/Union, Cut/Subtract, Intersect/Intersect |
| Intersect preview | Unavailable | Actual clipped result, transient bodies released |
| Multiple closed profiles or non-fixed end | No Intersect contract | Explicit refusal; no sequential intersection substitute |
| Empty result | Adapter cannot represent empty BodyKind/centroid | Existing typed refusal preserved; previous body retained |
| Existing parts | Boss/Cut operations | Unchanged serialized spellings and additive default for older app data |
| Full canonical U8 | Kernel-only checkpoint | Still pending rendered complete P4 and all acceptance gates |

## Validation scope

Focused coverage in `intersect_extrude.rs` distinguishes additive, subtractive
and intersected volumes for nontrivial overlap; exercises depth edit, undo/redo,
native save/reopen and serialization; checks empty/unsupported/no-body and malformed
requests; and requires a renderer for actual ribbon creation, clipped preview,
commit and depth edit. The rendered evidence is written to
`target/exit-test/l08-intersect/edited-intersection.png`. No renderer skip counts
as a pass. The dedicated hosted interactive job also invokes this target.

The exhaustive tool registry adds a top-open shell and final centred radius-23
intersection from z=-1 to 13. This command-path exercise does not replace complete
U8 acceptance: repeated shell-wall edits with the downstream intersection,
persistent inner/outer viewport picks, save/reopen and exact-revision app export
still need their combined rendered scenario. STEP import and click/proof timing
gates remain separate. No latency waiver or qualification-tier claim applies.

Only Intersect dispatch/parameter/control sections are changed. Drawing,
edit-lifecycle/rebuild policy, kernel adapter optimization, dependency pins, shared
export implementation and release machinery are untouched.

Local checks passed: 48 distinct focused Rust tests across the Intersect and
Shell rendered paths, Shell/Revolve commands, exhaustive tools, strict schemas,
CLI entry points/scripts and model document/features; nine workflow tests; formatting;
workspace all-target/all-feature clippy with warnings denied; and application/CLI builds. The rendered tests
ran with the software Vulkan adapter. This is functional evidence for the dependency,
not a measured full-U8 latency or qualification result.
