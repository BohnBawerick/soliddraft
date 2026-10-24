# SolidDraft in the browser

The application and Catallax compile into one `wasm32-unknown-unknown` module. The browser runs the same egui interface and wgpu viewport as the desktop app. There is no second kernel module or server kernel fallback. Catallax's current operation limits apply on both platforms.

A small Rust server serves the web build and relays assistant requests. The assistant's API key stays on the server. Tools execute in the browser against the part there. Files open by upload and save by download; the server stores no parts.

## Pieces

| Crate or file | What it does |
|---|---|
| `soliddraft-kernel-catallax` | The Catallax application entry, compiled into the desktop and browser app. |
| `soliddraft-web` | The WebAssembly entry point and `index.html`, built with trunk. `?demo` opens the sample part. |
| `soliddraft-app/src/web.rs` | Assistant relay client, upload and download. |
| `soliddraft-assistant/src/relay.rs` | Provider-neutral stream events, followed by the response or error. |
| `soliddraft-server` | Static files and the assistant relay. |
| `deploy/web/` | Docker image and local compose example. |

## The server

`soliddraft-server --listen 0.0.0.0:8766 --web DIR` serves:

| Path | What |
|---|---|
| `GET /` and the files under it | The web build. Content-hashed files are cached for a year; the page is checked with an ETag. Responses over 16 KB are gzipped. |
| `GET /api/info` | Whether the assistant is on, with its provider and model or the reason it is off. |
| `GET /version` | The running build's commit and crate version. The commit comes from `SOLIDDRAFT_COMMIT`, or is `null` when unset. |
| `POST /api/assistant` | Runs one chat request and streams the reply as JSON lines. |

The environment selects the provider, model and key as it does for the desktop app. Set `SOLIDDRAFT_NO_KEYCHAIN=1` on a server. Without a key, the app works and the assistant says why it is off.

Anyone who can reach the server can use the assistant and spend the configured key. Bind to localhost for local use. Put authentication and TLS in front of a remotely accessible server before configuring a provider key.

## Building and running locally

You need Rust 1.95 or later, trunk and the Rust WebAssembly target. No Emscripten or C++ compiler is needed for the kernel.

```sh
rustup target add wasm32-unknown-unknown
(cd crates/soliddraft-web && trunk build --release --locked --dist ../../target/web-dist)
cargo run --release -p soliddraft-server -- --web target/web-dist
```

Open <http://127.0.0.1:8766/>. The kernel runs locally in the browser, including when the assistant is off.

Hosted CI builds the complete application module. The Docker image builds the Rust server and uses trunk for the browser application. See [continuous integration](ci.md) for the native and browser checks.

The [release workflow](RELEASING.md) also produces a static browser zip and a GitHub Pages payload. Pages deployment remains off until the owner approves it. Static hosting runs the kernel locally without the assistant relay; it does not need the Rust server or a public self-hosted service.
