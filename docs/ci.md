# Continuous integration

`hosted.yml` runs on pushes, pull requests and manual dispatch. All jobs use GitHub-hosted runners and read-only repository permissions.

The complete Catallax-only workspace builds and tests on Linux, macOS and Windows from a checkout with no submodules. Linux also runs formatting, clippy, the browser application build and Rust 1.88 checks for the crates that still support it. The browser build compiles Catallax into the same `wasm32-unknown-unknown` module as the application. Linux and macOS also drive the rendered Catallax bracket and drawing flows, retaining screenshots and part files as artifacts. No job needs SMLib, Emscripten or a C++ kernel toolchain.

Run the same checks with `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-features --locked` and `cargo build -p soliddraft-web --target wasm32-unknown-unknown --locked`.

After a deliberate Catallax pin change, manually dispatch Hosted CI with `refresh_catallax_views` enabled to run the existing view and drawing baseline generators. Download the `catallax-view-baseline` artifact, review its diff and commit the generated JSON and SVG files. Require a normal run against that committed baseline before merging.
