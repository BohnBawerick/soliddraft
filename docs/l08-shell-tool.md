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
| Full P4 intersection/edit | Outside this tool's completed evidence | Not run or claimed complete |

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
