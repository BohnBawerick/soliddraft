# Continuous integration

`hosted.yml` runs on pushes, pull requests and manual dispatch. All jobs use GitHub-hosted runners and read-only repository permissions.

The complete Catallax-only workspace builds and tests on Linux, macOS and Windows from a checkout with no submodules. Linux also runs formatting, clippy, the browser application build and Rust 1.88 checks for the crates that still support it. The browser build compiles Catallax into the same `wasm32-unknown-unknown` module as the application. Linux and macOS also drive the rendered Catallax bracket and drawing flows, retaining screenshots and part files as artifacts. No job needs SMLib, Emscripten or a C++ kernel toolchain.

A Linux job exports the bracket as STEP, STL and 3MF and opens the STEP in OpenCASCADE through `ci/check-step.py`, which needs `cadquery-ocp`; see [part export](exchange.md#the-opencascade-check). OpenCASCADE is installed only in that job.

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

`release.yml` builds installers and portable archives in a separate workflow. Manual dispatch and release-configuration pull requests are non-publishing dry runs. Version tags can publish after Hosted CI passes; Pages remains disabled behind a repository variable. See [releasing](RELEASING.md) for artifacts, signing secrets, smoke checks and the first-release gate.

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
