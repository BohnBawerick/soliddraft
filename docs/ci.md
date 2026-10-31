# Continuous integration

`hosted.yml` runs on pushes, pull requests and manual dispatch. Windows and macOS jobs use GitHub-hosted runners. Linux jobs use the repository's self-hosted runners while the repository variable `USE_LOCAL_RUNNERS` is `true`, except for pull requests from forks, and `ubuntu-latest` otherwise. All jobs use read-only repository permissions.

Pull requests run the affected crates and their consumers on Linux. The selector uses Cargo metadata, including optional, test and target dependencies, and keeps base-manifest dependencies for renamed or deleted code. Platform source, manifests, lockfiles, toolchains, packaging and GPU-backend edits also select macOS tests and the macOS rendered bracket under the rules below. Release-branch pushes and manual dispatch run every hosted and packaging unit except Windows, which is [deferred](#windows-is-deferred). Main pushes run all Linux units as a selection audit. A file under `ci/` selects the jobs that read it, listed under [CI file consumers](#ci-file-consumers). Workflow edits and unknown paths broaden to all units. Both the base and candidate selection rules apply. All builds use a checkout with no submodules. Linux also runs formatting, clippy, the browser application build and Rust 1.88 checks for the crates that still support it. The browser build compiles Catallax into the same `wasm32-unknown-unknown` module as the application. Linux and macOS also drive the rendered Catallax bracket and drawing flows, retaining screenshots and part files as artifacts. No job needs SMLib, Emscripten or a C++ kernel toolchain.

A Catallax revision-only change in `Cargo.lock` or a `Cargo.toml` selects all eight Linux units, including the adapter and recipe tests, app scenarios, rendered bracket and STEP validation. In `Cargo.lock` the Catallax source revision must move, and every added, removed or changed package table must come from the official Catallax repository. This allows Catallax crates that the bump adds or removes, and new edges from Catallax crates to packages already locked. Package source decides, not package name. Every other package table must stay the same, including workspace members, so a new third-party crate keeps the broad rules. A manifest may change only the `rev` of dependencies from the official Catallax repository; adding a dependency to an application manifest, even a Catallax one, keeps the broad rules. Other edits in the same diff still add their required checks. macOS checks revision-only kernel moves at release, accepting that a kernel difference visible only on one platform may reach main first.

An edit limited to one Linux job in `hosted.yml` selects that job plus lint. It must preserve the job list, workflow-wide settings and that job's runner, and contain no matrix or shared YAML nodes. The runner must be a single-line label or expression; multiline runner forms keep the full checks. Edits to multiple jobs, platform jobs, the gate or another workflow keep the full checks. Unrecognised or malformed input keeps the broad rules.

A Rust file whose base or candidate text holds a desktop `cfg`, `cfg!` or `cfg_attr` (target OS, family, vendor or environment, `windows` or `unix`) is platform source. So are the native window, GPU setup and dialog files `soliddraft-app/src/main.rs`, `app/workspace.rs` and `platform.rs`, and `soliddraft-cli/src/screenshot.rs`, whatever they contain. Under `soliddraft-app/src/viewport/` every file is platform source except five reviewed shared UI and math files: `camera.rs`, `dimensions.rs`, `environment.rs`, `mod.rs` and `pick.rs`. These run the Linux checks only, unless either side of the edit adds a desktop `cfg` or names `wgpu::`, `winit::`, `rfd::`, `raw_window_handle::`, `std::os::`, a `.wgsl` file, `NativeOptions`, `WgpuConfiguration`, `SurfaceConfig` or `run_native(`. The `egui_wgpu` paint callback alone does not count. The renderer, assembly renderer, look tiers, offscreen capture, every shader and any new viewport file keep the macOS checks. This is a file policy, not a dependency analysis: native setup moved behind a helper in a shared file must be rejected in review. Shared code can still differ on another platform; release qualification runs every platform that is not deferred.

A Linux job exports the bracket as STEP, STL and 3MF and opens the STEP in OpenCASCADE through `ci/check-step.py`, which needs `cadquery-ocp`; see [part export](exchange.md#the-opencascade-check). The application-scenario job also uses this OCP 7.9 check for saved-part exports. Every `checked_step_export` additionally calls `tools/checks/raw_step.py`'s `valid()` from `.ci-catallax` at the exact adapter pin, using a separate `.ci-ocp8` virtual environment with `cadquery-ocp==8.0.1.1.0`. It checks the binding version and checkout revision, and fails on missing bindings or raw-invalid STEP. The standard and direct no-healing checks retain separate stdout and stderr reports.

Run the same checks with `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-features --locked` and `cargo build -p soliddraft-web --target wasm32-unknown-unknown --locked`.

The browser job also packages a runnable `web-application` artifact with trunk.
`ci/browser-journal.js` runs against that complete application through
`chrome-devtools-axi` and headless Chromium, using real localStorage quota and Web
Locks. It covers a full store, reload, failed retry, redacted report export without
deletion, explicit discard without clearing unrelated data, and successful startup
after space is available. To rerun it locally, serve the packaged app, open its
origin in an isolated `chrome-devtools-axi` session, then run
`chrome-devtools-axi run < ci/browser-journal.js`. The scenario clears that
session's origin storage when seeding each case; never use a profile with user
documents.

After a deliberate Catallax pin change, manually dispatch Hosted CI with `refresh_catallax_views` enabled to run the existing view and drawing baseline generators. Download the `catallax-view-baseline` artifact, review its diff and commit the generated JSON and SVG files. Require a normal run against that committed baseline before merging.

`packages.yml` provides the shared installer and portable-archive dry run. Hosted CI calls it for packaging and check-policy changes, and includes its result in the merge gate. Its `windows` input, off by default, adds the Windows package, the C runtime check and the Windows files in the release inventory. `packaging/release.py desktop` lists the desktop package cells. `release.yml` calls the same jobs after validating the source. Manual dispatch is a non-publishing dry run. Version tags publish only after a successful Hosted CI push run on the matching release branch at that exact commit; Pages remains disabled behind a repository variable. See [releasing](RELEASING.md) for artifacts, signing secrets, smoke checks and the first-release gate.

`ci/check-registry.json` maps selection units to workflow jobs. The always-running `Merge gate` requires success from every selected unit, rejects unexpected jobs and failed selection, and compares the base registry with the candidate. A removed unit needs an explicit retirement record with a reason. The gate reads only its own run's dependency results. Selection reasons and the gate verdict appear in the job summaries. Repository merge policy must require this check; this change does not alter repository settings.

`ci/scenario-inventory.json` registers every application scenario and its owners. The `bracket` acceptance scenario runs MCP document commands through the real adapter at Catallax `1599a8db3a8cc45be8f87e8ab085be72fb938ef6`. Its 32 steps check measured geometry, suffix rebuilds, width editing, undo/redo, save/reopen, drawing generation and checked STEP export. A refused zero-depth edit preserves revision and features. The result records the app checkout revision and kernel pin. The 25 existing steps and their accepted click-budget gaps stay unchanged. All scenarios still run when `application-loop` is selected. The `lattice` and `gyroid` scenarios check each certified STL with an independent oracle named by the step: `ci/check-lattice-stl.py` by default, `ci/check-gyroid-stl.py` for `"kind": "gyroid"`, and `ci/check-graded-stl.py` for `"graded-lattice"` and `"graded-gyroid"`. An unknown kind fails the export. The `graded-fields` scenario also exports each designed state's mass stand-in with `"stand_in": true`; `ci/check-field-moments.py` checks its moments, and those of the `field_mass` tool on a step with `"moments"`, against the V7d design's independent references, and checks the STEP file's envelope and solids, with OpenCASCADE where its binding is installed. A stand-in export with `"replay"` asks the pinned `catallax-verify` the same `catallax-stand-in/1` question for the step's native recipe and requires the same bytes. A scenario's `max_action_work` caps every step's whole work, click and background calls together. An export's `max_work` runs the fresh CLI process's whole request, opening and rebuilding the part included, on that one allowance (`soliddraft export --max-work`), and caps both that request's work and the work its report states. Finer scenario selection, a separate nightly audit and broader packaged edit/reopen checks remain follow-up work.

The `measure` acceptance scenario checks picked edge length and face area, tracked checks, upstream edits, stale sources, undo, reopen and a checked STEP export; `measure_panel` drives the same path through a real viewport pick in each interactive job. The `measure-pairs` scenario extends it to midpoints, edge and face pairs, tracked pairs, a fillet that removes one end, and stale and current reopen; `measure_panel` and `measure_guides` drive the hovered midpoint, the pair, Track this and the sketch guide through the real window.

The `mate-distance` acceptance scenario builds two parts, mates them through the MCP commands, drives the distance from a typed value, edits a part, updates, reopens and refuses a dangling reference, comparing the whole accepted state across every refusal. `mate_panel` makes the same mate in the window with real picks.

Run CI tooling checks with `python3 -m unittest discover -s ci -p 'test_*.py'`. The lint job always runs this suite, including the frozen designer routing and gate fixtures.

## Windows is deferred

By the captain's decision, CI builds and checks the browser, Linux and macOS only until a later Windows catch-up. No pull request, main push, release branch, tag or full qualification selects `test-windows` or `package-windows`, and releases publish no Windows packages. Windows product code stays in the tree, unbuilt by CI, so a Windows-only fault can land unnoticed until the catch-up.

The routing rules still name Windows, and the frozen routing fixtures still pin them. `select()` in `ci/select-checks.py` drops the two units last, and the accepted selector goes through the same function. The `test-windows` job, the Windows package cell and both registry entries stay defined. The merge gate skips Windows when it is unselected. When Windows is selected, the gate requires it to be green like any other unit.

To catch up, dispatch Hosted CI manually with `windows_catch_up` on. That is the only way to select Windows: the selector ignores the input on any other event, and `release.yml` passes no input to `packages.yml`, so its dry runs and tags have no Windows path. The run is a full qualification plus both Windows units, and takes 2 to 3 hours on Windows. Branch `fm/sdc-ci-round4-windows-shards-parked` holds unfinished work to split those tests across runners.

## CI file consumers

`ci/check-consumers.json` names every file under `ci/` and the jobs that read it. `files` maps exact paths to unit names, or to `"all"` for every unit and package. `patterns` maps full-match regular expressions to unit names. Lint always runs, so an empty list means no other job reads the file.

The selector looks up every changed `ci/` path, for all statuses and for both sides of a rename or copy. A deleted or renamed-away path may use the base map. The result is united with every other path's selection, main pushes retain all eight Linux units, and release branches and manual dispatch retain every unit and package except the deferred Windows ones.

Every tracked file under `ci/` needs exactly one entry, even when the diff does not touch it or another path already selects everything. An unmapped file, an invalid map or a path with two matching entries makes `ci/select-checks.py` exit nonzero without writing a selection, on every event, so the `select` job and the merge gate fail. A new script needs an exact entry, and adding that entry edits the map, which selects everything once. A new one-level scenario file matches the pattern and needs no map edit.

The running selector and its map are the candidate policy; `--head` names the tree they judge. The accepted selector runs from the base commit with the base map, so a candidate map edit cannot remove a consumer. If the accepted selector cannot judge a path, or its map is unreadable, every unit and package except the deferred Windows ones is selected and the `base:` reason says why. A commit whose selector predates the map has no base map to read, and its older rules apply.

| Path | Jobs beyond lint | Reader |
|---|---|---|
| `ci/browser-journal.js` | `web` | Browser journal check in the web job |
| `ci/check-consumers.json` | all | The selector; it controls its own obligations |
| `ci/check-gate.py` | all | Merge gate, and the gate tests in lint; it controls mandatory checks |
| `ci/check-registry.json` | all | Merge gate, and the wiring tests in lint; it controls mandatory checks |
| `ci/select-checks.py` | all | Selection job, and the routing tests in lint; it controls its own obligations |
| `ci/check-field-moments.py`, `ci/check-graded-stl.py`, `ci/check-gyroid-stl.py`, `ci/check-lattice-stl.py` | `application-loop` | Called by `ci/run-scenarios.py` |
| `ci/check-step.py` | `step-opencascade`, `application-loop` | OpenCASCADE job, and called by `ci/run-scenarios.py` |
| `ci/fake_mcp_server.py` | `application-loop` | Used by `ci/test_scenarios.py` in both jobs |
| `ci/known-over-budget.json` | `application-loop` | `ci/run-scenarios.py`, the lint inventory tests, and `packaging/release.py`; see below |
| `ci/run-scenarios.py` | `application-loop` | Scenario steps, and `ci/test_scenarios.py` in both jobs |
| `ci/scenarios/[^/]+\.json` | `application-loop` | Scenario steps, and the lint inventory tests |
| `ci/test_scenarios.py` | `application-loop` | Python discovery in lint, and the application job |
| `ci/check-ownership.py`, `ci/ownership.toml` | None | `ci/test_ownership.py` |
| `ci/gate-cases.json` | None | Gate tests in `ci/test_checks.py` |
| `ci/routing-cases.json`, `ci/routing-replays/*` | None | Routing tests in `ci/test_checks.py`; frozen test input, not live policy. Each file has its own exact entry |
| `ci/scenario-inventory.json` | None | Inventory tests in `ci/test_checks.py` |
| `ci/test_checks.py`, `ci/test_ownership.py`, `ci/test_scenario_bundles.py`, `ci/test_workflows.py` | None | Python discovery in lint |
| `ci/cargo-cache.sh`, `ci/check-unchanged-tests.py`, `ci/compare-scenario-bundles.py`, `ci/public-items.py`, `ci/refactor_baseline.py` | None | Manual commands; no workflow runs them |
| `ci/scenarios/p3-rounded-boss.md` | None | Description only; scenario runs read JSON |
| `ci/scenarios/p5-flanged-housing.md` | None | Description only; scenario runs read JSON |

`ci/known-over-budget.json` has one more reader. `packaging/release.py` reads it for the release notes, called by the Linux `assemble` job in `packages.yml`, which also runs when Hosted CI calls the packaging jobs. An edit to this file does not rebuild packages because `Inventory.test_known_gaps_render_release_notes` runs that reader in lint. This is the only such substitution. Any other file that packaging reads must list the package units.

## Superseded runs

Hosted CI has a concurrency group keyed on the pull request's head branch (the pushed ref for a manual run), so a new push cancels that branch's older runs. A run on main is never cancelled: its group is its own run id, so no later push shares it and GitHub's replacement of a waiting run in a shared group cannot drop a main result. `ci/test_workflows.py`, run in the lint job, evaluates the group and cancel expressions for pull request, manual and main runs, and for a pull request from a branch that is itself called `main`.

## Reliability tests

Four property-test files use `proptest` as a dev-dependency. Each runs a fixed number of cases from a fixed seed by default, so a failure reproduces, and prints the failing input and does not write regression files. To search further, set `PROPTEST_CASES` and `PROPTEST_RNG_SEED`, for example `PROPTEST_CASES=2000 PROPTEST_RNG_SEED=7 cargo test -p soliddraft-app --test command_fuzz`. A failure found that way becomes a named test beside the fix.

| File | Input fuzzed | Properties |
|---|---|---|
| `crates/soliddraft-assistant/tests/protocol_fuzz.rs` | The MCP server's JSON-RPC layer, the schema check, the stdio line loop and the HTTP endpoint: arbitrary JSON, text, ids, batches, nesting, non-UTF-8 lines, oversized lines, malformed requests, lying `Content-Length` | No panic; every request gets a well-formed reply that names an error when the input was bad; the server answers the next message; no error echoes the input back whole |
| `crates/soliddraft-app/tests/command_fuzz.rs` | Every tool of the command layer, over MCP and as the assistant, with arguments built from each tool's schema and then bent: wrong types, missing and extra fields, extreme numbers, odd and long names, unknown tools, repeated calls | No panic and no case over ten minutes; a refusal is typed and changes nothing (revision, features, undo stack, rollback bar); the part still saves and reopens as the same revision; the assistant cannot apply unasked |
| `crates/soliddraft-cli/tests/recipe_fuzz.rs` | The corpus recipes `soliddraft replay`, `view` and `draw` take, with numbers made extreme, values retyped, keys and steps deleted, duplicated, swapped and repeated | No panic and no replay over ten minutes; a refusal is typed; a replay that succeeds returns bodies the kernel proved valid |
| `crates/soliddraft-cli/tests/fuzz_arguments.rs` | The `soliddraft` binary on generated arguments for every subcommand, over good, empty, damaged, missing and folder-shaped files | Ends within ten minutes with status 0, 1 or 2; says why when it fails; keeps the fixtures in place. `asm generate`, `asm bench`, `screenshot`, `key` and `mcp --http` are left out: their sizes are the caller's choice, or they need the machine |

`crates/soliddraft-cli/tests/kernel_abort.rs` holds the isolated reproductions of a known Catallax abort (a plane origin a hair off zero overflows the stack of the main thread, in about 20 s). They are ignored until the pinned Catallax revision bounds that recursion (the Catallax fix is in progress); run them with `-- --ignored`. A green `recipe_fuzz` or `command_fuzz` run does not cover those inputs, nor lengths over 100 m (refused before the kernel), nor a mutated bracket; each exclusion is stated in the generator's doc comment and comes out with the `#[ignore]` lines. `recipe_fuzz` and `command_fuzz` keep nonzero numbers smaller than 0.01 out of their inputs for the same reason.

`crates/soliddraft-cli/tests/damaged_files.rs` opens truncated, corrupted and wrong-version variants of a part, an assembly and a drawing through the libraries, the application's open functions and the binary's reading and writing commands. Each must be refused with a typed error of the right kind (damage, or a version or kind this build does not read) and a message, and the file's bytes, hash and folder must be unchanged afterwards. `soliddraft doc info`, `doc refs`, `doc where-used`, `doc migrate` and `asm insert` of a part read the manifest alone, so they report a file whose manifest reads even when the truth entry is damaged; opening it is what refuses.

`FUZZ_TRACE=1 cargo test ... -- --nocapture` prints each command-line run's exit status and arguments, and each tool call's outcome in `command_fuzz`.

The `countersink` acceptance scenario extends its original seven steps with a boss beside the countersinks, a third hole and diameter edit, face-name checks, save/reopen and fresh-process checked STEP export. Its last step requires the typed refusal for a countersink that truly meets the boss wall and verifies unchanged geometry. Volume and bounds expectations come from the V8 merge B test design.

The `assembly` acceptance scenario checks separate part and assembly undo, a floating
component at the origin, in-session freshness, transactional Update, failure retention
and reopen from accepted display copies through MCP at the current Catallax pin.

The `subassembly-form` scenario runs the V6 batch 1 design's manifest: a base and three
placements of one pin, mated and driven, formed into `modules/pack.dwasm` with every pose,
reference, mate id and freedom checked against hand-derived values, then saved and both
files reopened. Its later increments add history, refusals, the two-file Update and
the reopened driver; until its typed refusal step lands it is not an acceptance scenario.
`crates/soliddraft-assembly/tests/subassembly_form.rs` and
`crates/soliddraft-app/tests/subassembly_form.rs` compare file bytes and package contents
the runner cannot.

## Configuration acceptance

`ci/scenarios/configurations.json` checks three width variants against hand-derived volume, area, centroid and bounds, then checks exact geometry identity across undo, redo, switches and reopens. Two checked STEP exports must match the selected variant. The scenario records click timings. S13 checks zero-depth refusal and a valid row whose cut would empty the body. Activation must preserve the previous variant; switching and repairing that row then prove recovery. Model cases M1-M9 cover old files, display meshes, inheritance, write targets, rename, row validation, undo and damaged truth. Command fuzzing includes configuration edits and switches.

`ci/scenarios/configuration-grid-csv.json` and `-xlsx.json` run one journey per format against an independent fixture and hand-derived geometry: preview, Cancel, Apply, undo, redo, both exports, a fresh process, checked STEP and every row. `ci/scenarios/configuration-title.json` binds a drawing title and proves it follows the accepted source through part switches, undo, a fresh process and a part changed on disk. `ci/configuration-grid-fixtures.py` is the test designer's generator, and `ci/check-configuration-grid.py` decodes exported CSV, workbook cells and SVG text with Python's standard library alone. The `configuration_grid` test drives the window and covers file refusals and preview tokens.

A scenario's `setup` runs check scripts before the first step, and a step's `helpers` run them after it. A step with `fresh_process` first closes the application and starts another, whose evidence goes to `commands-N`.
