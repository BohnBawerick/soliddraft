# Continuous integration

`hosted.yml` runs on pushes, pull requests and manual dispatch. Windows and macOS jobs use GitHub-hosted runners. Linux jobs use the repository's self-hosted runners while the repository variable `USE_LOCAL_RUNNERS` is `true`, except for pull requests from forks, and `ubuntu-latest` otherwise. All jobs use read-only repository permissions.

Pull requests run the affected crates and their consumers on Linux. The selector uses Cargo metadata, including optional, test and target dependencies, and keeps base-manifest dependencies for renamed or deleted code. Platform source, manifests, lockfiles, toolchains, packaging and GPU-backend edits also select macOS and Windows tests and the macOS rendered bracket under the rules below. Release-branch pushes and manual dispatch run every hosted and packaging unit. Main pushes run all Linux units as a selection audit. Check-policy changes and unknown paths broaden to all units. Both the base and candidate selection rules apply. All builds use a checkout with no submodules. Linux also runs formatting, clippy, the browser application build and Rust 1.88 checks for the crates that still support it. The browser build compiles Catallax into the same `wasm32-unknown-unknown` module as the application. Linux and macOS also drive the rendered Catallax bracket and drawing flows, retaining screenshots and part files as artifacts. No job needs SMLib, Emscripten or a C++ kernel toolchain.

A Catallax revision-only change in `Cargo.lock` or a `Cargo.toml` selects all eight Linux units, including the adapter and recipe tests, app scenarios, rendered bracket and STEP validation. The lockfile may change edges between Catallax crates, but every other package and third-party dependency edge must stay the same. A manifest may change only the `rev` of dependencies from the official Catallax repository. Other edits in the same diff still add their required checks. Windows and macOS check revision-only kernel moves at release, accepting that a kernel difference visible only on one platform may reach main first.

An edit limited to one Linux job in `hosted.yml` selects that job plus lint. It must preserve the job list, workflow-wide settings and that job's runner, and contain no matrix or shared YAML nodes. The runner must be a single-line label or expression; multiline runner forms keep the full checks. Edits to multiple jobs, platform jobs, the gate or another workflow keep the full checks. Unrecognised or malformed input keeps the broad rules. Selector and routing-fixture edits still select every unit and package under both base and candidate rules.

Edits confined to `ci/run-scenarios.py`, `ci/test_scenarios.py`, `ci/known-over-budget.json`, `ci/fake_mcp_server.py`, `ci/scenario-inventory.json` and one-level `ci/scenarios/*.json` select only lint and application scenarios. This includes added, deleted, renamed and copied files, and every inventory edit, including pin records. The lint suite also checks that the known-budget gaps render release notes. Other `ci/` paths keep the full checks. Mixed diffs union every path's selection, main pushes retain all eight Linux units, and release branches and manual dispatch retain every unit and package.

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

`packages.yml` provides the shared installer and portable-archive dry run. Hosted CI calls it for packaging and check-policy changes, and includes its result in the merge gate. `release.yml` calls the same jobs after validating the source. Manual dispatch is a non-publishing dry run. Version tags publish only after a successful Hosted CI push run on the matching release branch at that exact commit; Pages remains disabled behind a repository variable. See [releasing](RELEASING.md) for artifacts, signing secrets, smoke checks and the first-release gate.

`ci/check-registry.json` maps selection units to workflow jobs. The always-running `Merge gate` requires success from every selected unit, rejects unexpected jobs and failed selection, and compares the base registry with the candidate. A removed unit needs an explicit retirement record with a reason. The gate reads only its own run's dependency results. Selection reasons and the gate verdict appear in the job summaries. Repository merge policy must require this check; this change does not alter repository settings.

`ci/scenario-inventory.json` registers every application scenario and its owners. The `bracket` acceptance scenario runs MCP document commands through the real adapter at Catallax `1556d52678efa27af32a32914723cad5e222746f`. Its 32 steps check measured geometry, suffix rebuilds, width editing, undo/redo, save/reopen, drawing generation and checked STEP export. A refused zero-depth edit preserves revision and features. The result records the app checkout revision and kernel pin. The 25 existing steps and their accepted click-budget gaps stay unchanged. All scenarios still run when `application-loop` is selected. Finer scenario selection, a separate nightly audit and broader packaged edit/reopen checks remain follow-up work.

The `measure` acceptance scenario checks picked edge length and face area, tracked checks, upstream edits, stale sources, undo, reopen and a checked STEP export; `measure_panel` drives the same path through a real viewport pick in each interactive job.

Run CI tooling checks with `python3 -m unittest discover -s ci -p 'test_*.py'`. The lint job always runs this suite, including the frozen designer routing and gate fixtures.

## Superseded runs

Hosted CI has a concurrency group keyed on the pull request's head branch (the pushed ref for a manual run), so a new push cancels that branch's older runs. A run on main is never cancelled: its group is its own run id, so no later push shares it and GitHub's replacement of a waiting run in a shared group cannot drop a main result. `ci/test_workflows.py`, run in the lint job, evaluates the group and cancel expressions for pull request, manual and main runs, and for a pull request from a branch that is itself called `main`.

## Reliability tests

Four property-test files use `proptest` as a dev-dependency. Each runs a fixed number of cases from a fixed seed by default, so a failure reproduces, and prints the failing input and does not write regression files. To search further, set `PROPTEST_CASES` and `PROPTEST_RNG_SEED`, for example `PROPTEST_CASES=2000 PROPTEST_RNG_SEED=7 cargo test -p soliddraft-app --test command_fuzz`. A failure found that way becomes a named test beside the fix.

| File | Input fuzzed | Properties |
|---|---|---|
| `crates/soliddraft-assistant/tests/protocol_fuzz.rs` | The MCP server's JSON-RPC layer, the schema check, the stdio line loop and the HTTP endpoint: arbitrary JSON, text, ids, batches, nesting, non-UTF-8 lines, oversized lines, malformed requests, lying `Content-Length` | No panic; every request gets a well-formed reply that names an error when the input was bad; the server answers the next message; no error echoes the input back whole |
| `crates/soliddraft-app/tests/command_fuzz.rs` | Every tool of the command layer, over MCP and as the assistant, with arguments built from each tool's schema and then bent: wrong types, missing and extra fields, extreme numbers, odd and long names, unknown tools, repeated calls | No panic and no call over a minute; a refusal is typed and changes nothing (revision, features, undo stack, rollback bar); the part still saves and reopens as the same revision; the assistant cannot apply unasked |
| `crates/soliddraft-cli/tests/recipe_fuzz.rs` | The corpus recipes `soliddraft replay`, `view` and `draw` take, with numbers made extreme, values retyped, keys and steps deleted, duplicated, swapped and repeated | No panic and no replay over two minutes; a refusal is typed; a replay that succeeds returns bodies the kernel proved valid |
| `crates/soliddraft-cli/tests/fuzz_arguments.rs` | The `soliddraft` binary on generated arguments for every subcommand, over good, empty, damaged, missing and folder-shaped files | Ends within two minutes with status 0, 1 or 2; says why when it fails; keeps the fixtures in place. `asm generate`, `asm bench`, `screenshot`, `key` and `mcp --http` are left out: their sizes are the caller's choice, or they need the machine |

`crates/soliddraft-cli/tests/kernel_abort.rs` holds the isolated reproductions of a known Catallax abort (a plane origin a hair off zero overflows the stack of the main thread, in about 20 s). They are ignored until the pinned Catallax revision bounds that recursion (the Catallax fix is in progress); run them with `-- --ignored`. A green `recipe_fuzz` or `command_fuzz` run does not cover those inputs, nor lengths over 100 m (refused before the kernel), nor a mutated bracket; each exclusion is stated in the generator's doc comment and comes out with the `#[ignore]` lines. `recipe_fuzz` and `command_fuzz` keep nonzero numbers smaller than 0.01 out of their inputs for the same reason.

`crates/soliddraft-cli/tests/damaged_files.rs` opens truncated, corrupted and wrong-version variants of a part, an assembly and a drawing through the libraries, the application's open functions and the binary's reading and writing commands. Each must be refused with a typed error of the right kind (damage, or a version or kind this build does not read) and a message, and the file's bytes, hash and folder must be unchanged afterwards. `soliddraft doc info`, `doc refs`, `doc where-used`, `doc migrate` and `asm insert` of a part read the manifest alone, so they report a file whose manifest reads even when the truth entry is damaged; opening it is what refuses.

`FUZZ_TRACE=1 cargo test ... -- --nocapture` prints each command-line run's exit status and arguments, and each tool call's outcome in `command_fuzz`.

The `countersink` acceptance scenario extends its original seven steps with a boss beside the countersinks, a third hole and diameter edit, face-name checks, save/reopen and fresh-process checked STEP export. Its last step requires the typed refusal for a countersink that truly meets the boss wall and verifies unchanged geometry. Volume and bounds expectations come from the V8 merge B test design.
