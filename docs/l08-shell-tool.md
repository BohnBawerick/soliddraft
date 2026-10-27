# Analytic shell tool

This slice exposes Catallax's supported analytic shell operation in SolidDraft. The
public target is the planar prefix of Catallax `parts/P4-shelled-tray.json`: a
centred 40 × 30 × 12 mm blank, top open, wall 2 mm edited to 1.5 mm. This demonstration
is not full P4: the final radius-23 cylinder intersection remains separate.

The Shell editor selects openings by persistent face reference and previews wall
changes through the same kernel operation as a committed feature. Invalid wall
values report an editor error and discard the invalid preview using the existing
editor behavior. Features save their openings and thickness in the native document;
inner walls are named by the source faces' persistent ordering. Measurement is used
only by tests, not by shell construction or preview. STEP uses the existing export
path and its proof requirements.

## Defect ledger and acceptance

| Item | Historical behavior | Current status |
| --- | --- | --- |
| Shell command | Disabled with stage-3 message | Enabled when a body exists |
| Kernel shell dispatch | Unconditional stage-3 refusal | Native shell operation with resolved face origins |
| Script/MCP command registration | Shell window action had no tool declaration, failing the workspace entry-point contract | `shell` and `edit_shell` expose the existing model operation through checked schemas and the standard command handler |
| Inner-wall history | `Other` role | `Role::Inner` maps to `FaceRole::Offset` |
| Planar tray | No application shell tool | Kernel checks and rendered create/edit/save/reopen/select/export path pass |
| Circular-extrusion shell | Generic stage-3 refusal | Exact `distance.cylinder-domain` refusal retained; unsupported in this slice |
| Nonconvex planar shell | Generic stage-3 refusal | Exact `distance.unsupported-envelope` refusal retained; input unchanged |
| Full P4 intersection/edit | Outside the original tool's evidence | Kernel adapter geometry/export passes on the checked-in pin; application intersection feature and complete rendered acceptance remain pending |

## Validation

Focused commands, with the repository's stable Rust toolchain:

```sh
cargo test --locked -p soliddraft-kernel-catallax --test shell --test acceptance -j 2
cargo test --locked -p soliddraft-app --test shell_tool -j 2
```

Local validation: 17 adapter acceptance tests, six shell tests, the rendered shell
click-path test, no-body palette test, remaining waiting-command test, eight workflow
checks, workspace clippy with warnings denied, and formatting checks pass.
The click-path observation recorded 2704.516 ms for the creation sequence and
2052.861 ms for the edit sequence, including typing and frame settling. These
numbers do not establish a latency budget pass or complete target acceptance.

The rendered test requires a working GPU or software Vulkan adapter and fails if
none is available. It drives the actual ribbon, viewport selection, thickness
field and editor confirmation, then the application's save/open/export actions.
It checks inner-wall reference survival across the edit and reopen. Screenshots,
the native part, STEP and labelled click timings are written beneath
`target/exit-test/l08-shell/`. Click timings include test frame settling; they are
not kernel timings and do not establish a latency budget pass.

Local public behavior is scoped to SINGLEPAIR; no qualification run or green
SINGLEPAIR/FULL SIX claim is made here. Interpretation of the new Shell feature
schema requires the parent-routed FULL SIX tier. No budgets, tolerances or pins
were changed.

## Script and MCP commands

`shell` takes `open_faces` (an array of persistent face labels; an empty array
requests a closed shell) and positive `thickness` in mm. `edit_shell` takes the
shell `feature` name and positive `thickness`, retaining the openings. Both use
the normal feature commands and undo history. Missing faces, invalid thickness,
wrong feature kinds and malformed arguments refuse without changing the part.

PR44's macOS and Windows workspace failure was the unchanged entry-point contract
`a_command_without_a_tool_says_why`. The fix supplies actual headless commands;
it does not exempt Shell from the contract or classify the operation as window-only.

The subsequent Ubuntu Workspace run (40462778792, job 122051030874) failed
the rendered Shell test's adapter prerequisite: no Vulkan driver was installed.
The Workspace Linux leg now installs Mesa's software Vulkan adapter and sets its
runtime directory/backend, as the dedicated interactive job already does. The
renderer assertion and all geometry/UI expectations remain unchanged. Local
validation with the Vulkan driver unavailable fails to create a render state;
with the software adapter available, the full rendered tray path passes. Subsequent
Ubuntu Workspace and bracket jobs passed before PR44 merged as `f1d50ca`.

## U8 canonical P4 dependency checkpoint

Source pairing: SolidDraft main `f1d50cabe9c5f77739ea7eacc984351dae40494f`
and its unchanged Catallax pin `a5c42583dd684e54c473bc377267e5e190c37bb6`.
The public specification is Catallax `parts/P4-shelled-tray.json`,
`parts/edit-checks.json`, and PLAN section 9: centred 40 × 30 × 12 blank,
top open, wall 2 mm, intersected with a centred radius-23 cylinder from z=-1
to 13; the prescribed wall edit is 1.5 mm. The original simple tray is still
separate evidence.

`tests/u8_p4.rs` uses the normal application adapter with no dependency override
or work-allowance change. It rebuilds the shell and final intersection at walls
2, 1.5, 2, 1.5 mm, checking valid topology, 15 faces, volume against the analytic
rectangle/circle intersection minus the cavity, inner/outer floor orientation,
and STEP face count and the existing default deviation bound. All four pass.
Observed intersection adapter calls were 357–417 ms in the local test build;
these include the adapter's construction/display work, exclude requested
measurement/export, and are neither rendered click times nor budget acceptance.

The remaining integration dependency is an authored intersection feature. The
current model's `ExtrudeOp` only has `Boss` and `Cut`; its build dispatch chooses
Union or Subtract, and the app has no Intersect command/editor. A coordinated
extension needs an explicit Intersect operation (never the existing Cut fallback),
a circle sketch on z=-1 extruded +14, and persistent serialization, command schema,
preview/commit and tree support. The wall edit must rebuild this downstream feature.
This crosses shared model/app dispatch files and must be reconciled with L03 before
editing them; lifecycle, rebuild policy, pins and shared export code are untouched.

Owned changes at this checkpoint are only the new adapter test and this ledger.
Rendered full-P4 creation, repeated interactive edits, inner/outer pick identity,
save/reopen, exact-revision application export, STEP import, and preview/proof
latency gates remain untested. Kernel success does not complete U8 or qualify a
SINGLEPAIR/FULL SIX tier. No earlier waiver is applied here.
