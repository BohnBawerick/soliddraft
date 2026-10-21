# Continuous integration

`hosted.yml` runs on pushes, pull requests and manual dispatch. All jobs use
GitHub-hosted runners and read-only repository permissions.

Linux runs formatting, clippy, the pinned USD BRep check, workspace tests, the browser
app build and Rust 1.88 compatibility checks. Rust-only crates run on Linux, macOS
and Windows. The kernel's Emscripten build is checked separately.

Run the same checks locally with `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets --all-features -- -D warnings`,
`cargo test --workspace --all-features` and
`cargo build -p soliddraft-web --target wasm32-unknown-unknown`.
