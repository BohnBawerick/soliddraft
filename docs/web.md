# The web build

SolidDraft also runs in a web browser. The app is compiled to WebAssembly and draws into a
canvas with the same egui interface and wgpu viewport as the desktop build. The SMLib
kernel runs in the browser too, as a second WebAssembly module next to the app. A small
server serves the page and does the one thing a browser must not: it holds the
assistant's API key and makes the model calls for it.

## Can the whole app run in the browser?

Yes, kernel included. This was a bounded spike, and it went further than expected.

**The app.** eframe's web target is `wasm32-unknown-unknown` with wasm-bindgen, built
with trunk. Everything in `soliddraft-app` compiles for it after these changes:

- `std::time::Instant` and `SystemTime` panic on that target. The app uses `web-time`,
  which is the standard library's type on the desktop.
- There are no threads. The assistant's request thread became `Provider::start`, which
  spawns a thread on the desktop and runs a `fetch` in the background in the browser.
- There is no file system. Open uploads a file through the browser's file picker; Save,
  Save drawing and Export download the file (`platform.rs`).
- reqwest's blocking client is desktop-only, and the browser has no keychain, so the
  browser reaches its model through the server (below). The MCP server compiles but cannot listen in a
  browser, so `--mcp` is a desktop feature.

**The kernel.** SMLib is about 880,000 lines of C++. It throws and catches exceptions
(about 760 `try` blocks), uses `std::mutex` and thread-local storage, and never starts a
thread (`SM_USE_TBB` stays off, as upstream builds it). Rust's `wasm32-unknown-unknown`
target has no C or C++ standard library, so SMLib cannot be linked into the app's module.
Emscripten has both, and it compiled SMLib, SM_API and the cxx bridge with no source
change, using WebAssembly exception handling (`-fwasm-exceptions`). Rust has an Emscripten
target too, so `soliddraft-kernel-smlib` itself builds for `wasm32-unknown-emscripten`.
The only code change is that the kernel runs its calls in place there instead of on a
kernel thread.

The kernel's contract tests pass as WebAssembly under node: 18 of 18, everything except
the test that calls the kernel from several threads at once.

**How the two modules meet.** The app and the kernel are separate WebAssembly modules,
because they come from two toolchains. `soliddraft-kernel-wasm` wraps the kernel in two
exported functions that take a kernel request as JSON and return the reply, in the
protocol of `soliddraft-kernel-remote`. `index.html` loads that module and defines
`soliddraftKernelCall`. The app's kernel is a `RemoteKernel` whose transport calls that
function, so every kernel call is a synchronous call between the two modules on the
page's own thread. A long boolean or fillet blocks the page while it runs, as it blocks
the desktop app's frame.

If the kernel module is missing or fails to load, or the address has `?kernel=server`,
the app uses the same SMLib kernel on the server instead, with the same protocol over
synchronous HTTP requests. The browser console says which kernel answered.

**Sizes.** The app's module is 12.5 MB, 5.0 MB gzipped. The kernel's module is 6.1 MB,
2.1 MB gzipped. The server gzips both, and the browser keeps them until a new build
changes them.

## Pieces

| Crate or file | What it does |
|---|---|
| `soliddraft-kernel-remote` | The kernel protocol: `Request` (one variant per `Kernel` method), the `RemoteKernel` client over any `Transport`, and `Session`, the answering side, which tracks the bodies each client made. |
| `soliddraft-kernel-wasm` | The kernel's WebAssembly module, built with Emscripten. |
| `soliddraft-web` | The app's WebAssembly entry point and `index.html`, built with trunk. `?demo` opens the sample part. |
| `soliddraft-app/src/web.rs` | The browser side: the kernel transports, the assistant relay client, upload and download. |
| `soliddraft-assistant/src/relay.rs` | The assistant relay's line format: provider-neutral stream events, then the response or the error. |
| `soliddraft-server` | Serves the page and the modules, the assistant relay, and the fallback kernel. |
| `deploy/web/` | A Docker image and local compose example. |

## The server

`soliddraft-server --listen 0.0.0.0:8766 --web DIR` serves:

| Path | What |
|---|---|
| `GET /` and the files under it | The web build. trunk's file names carry a content hash and are cached for a year; the page and the kernel module are checked with an ETag on each load. Responses over 16 KB are gzipped. |
| `GET /api/info` | The kernel version and whether the assistant is on. |
| `GET /version` | The commit the running build was made from, and the crate version: `{"commit": "a3d148c…", "version": "0.1.0"}`. The commit comes from `SOLIDDRAFT_COMMIT`, which the image sets; it is `null` in a build made by hand. |
| `POST /api/assistant` | Runs one chat request on the server's provider and streams the reply back as JSON lines. |
| `POST /api/kernel` | One kernel call, for the fallback. Each browser tab sends a random session id; a session sees only its own bodies. |
| `POST /api/session/ping`, `/api/session/close` | A tab's heartbeat (every minute) and its goodbye (a beacon when the page closes). A session's bodies are released on close, or after ten minutes without a heartbeat. |

**The assistant's key stays on the server.** The browser sends the conversation, a
provider-neutral `ChatRequest`. The server runs it on the provider its environment
configures, with the key from its environment, and streams back the same `StreamEvent`s
and `ProviderResponse` the desktop app reads from a provider. The tools still run in the
browser, against the part there. The relay does not know which provider the server uses,
so a new provider needs no change on the browser side.

Anyone who can reach the server can use the assistant and spend the configured key.
Bind it to localhost for local use. For remote use, put authentication and TLS in front
of the server before configuring a provider key.

**Files** are opened by upload and saved by download. Nothing is stored on the server.

## Building and running it locally

You need the `wasm32-unknown-unknown` and `wasm32-unknown-emscripten` targets, trunk,
and Emscripten (`em++` on the path; SolidDraft is tested with 6.0.12).

```sh
rustup target add wasm32-unknown-unknown wasm32-unknown-emscripten
(cd crates/soliddraft-web && trunk build --release --dist ../../target/web-dist)
cargo build --release -p soliddraft-kernel-wasm --target wasm32-unknown-emscripten
mkdir -p target/web-dist/kernel
cp target/wasm32-unknown-emscripten/release/soliddraft-kernel-wasm.js \
   target/wasm32-unknown-emscripten/release/soliddraft_kernel_wasm.wasm target/web-dist/kernel/
cargo run --release -p soliddraft-server -- --web target/web-dist
```

Then open <http://127.0.0.1:8766/>. Without the kernel module the page still works, on
the server's kernel. The first Emscripten build of SMLib takes as long as the native one.

`.cargo/config.toml` holds the Emscripten flags. The kernel's tests run under node with:

```sh
CARGO_TARGET_WASM32_UNKNOWN_EMSCRIPTEN_RUNNER=node \
  cargo test -p soliddraft-kernel-smlib --target wasm32-unknown-emscripten --test contract -- --test-threads=1 --skip concurrent
```

CI builds the app's module on every pull request (the `web` job in `hosted.yml`). It does
not build the kernel module, which would add a full Emscripten build of SMLib to every
run on runners that are already the bottleneck.
