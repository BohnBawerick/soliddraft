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
| `deploy/web/` | Docker image, compose configuration and release deployment timer. |

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

## Release deployments

`deploy/web/deploy.sh --latest-release` serves the highest stable `vMAJOR.MINOR.PATCH`
tag by numeric version, including tags whose GitHub release is marked as a prerelease.
The tag must be reachable from `origin/main`. Before the first tag exists, it deploys
`main` once as a preview. Later main commits do not trigger deployments. Tags are
immutable; publish a new patch tag to fix a release.

The controller keeps its own clone, separate from development checkouts. Install the
controller and compose file outside that clone so a release need not contain either
file. The default install layout is:

```sh
install -Dm755 deploy/web/deploy.sh ~/.local/share/soliddraft-web/deploy.sh
install -Dm644 deploy/web/compose.yaml ~/.local/share/soliddraft-web/compose.yaml
~/.local/share/soliddraft-web/deploy.sh --latest-release
```

The first invocation clones the repository into `~/.local/share/soliddraft-web/src`.
It builds `soliddraft-web:candidate`, runs it on a temporary localhost port, and checks
`/version`, `/api/info`, the page and its application WebAssembly module before touching
the live service. The revision label and `SOLIDDRAFT_COMMIT` identify the source commit.
The image sets `SOLIDDRAFT_NO_KEYCHAIN=1`.

A failed build or candidate check leaves the live container alone. A failed live check
restores the old image. A successful deploy retains `soliddraft-web:previous` for manual
rollback. Failed commits are recorded in `~/.local/state/soliddraft-web/failed`; timer
ticks skip them until a newer release appears. Deploying an explicit ref retries it.
The lock in that state directory serializes manual and timer deployments.

The optional assistant environment file is `~/.config/soliddraft-web/env`. Keep it
outside the repository, readable only by its owner. Set `OPENROUTER_API_KEY` or
`ANTHROPIC_API_KEY` there, with optional `SOLIDDRAFT_PROVIDER` and `SOLIDDRAFT_MODEL`.
An absent or empty file leaves the assistant off.

### Timer setup

Install the units for the user who owns the clone and can use Docker:

```sh
install -Dm644 deploy/web/systemd/soliddraft-web-deploy.service \
  ~/.config/systemd/user/soliddraft-web-deploy.service
install -Dm644 deploy/web/systemd/soliddraft-web-deploy.timer \
  ~/.config/systemd/user/soliddraft-web-deploy.timer
systemctl --user daemon-reload
systemctl --user enable --now soliddraft-web-deploy.timer
```

Enable lingering for that user if the timer must run without a login. The container's
`unless-stopped` restart policy handles Docker daemon restarts independently.
The timer checks every two minutes and does not require a GitHub API token.

The optional `~/.config/soliddraft-web/deploy.env` contains timer configuration, one
`VAR=value` per line. Export the same variables for manual deploys:

| Variable | Default or contract |
|---|---|
| `SOLIDDRAFT_WEB_SRC` | `~/.local/share/soliddraft-web/src` |
| `SOLIDDRAFT_WEB_STATE` | `$XDG_STATE_HOME/soliddraft-web`, falling back to `~/.local/state/soliddraft-web` |
| `SOLIDDRAFT_REPO` | The public SolidDraft Git repository |
| `SOLIDDRAFT_WEB_PORT` | `127.0.0.1:8766`, accepts a bind address and port |
| `SOLIDDRAFT_WEB_ENV` | `~/.config/soliddraft-web/env`, container environment only |
| `SOLIDDRAFT_WEB_COMPOSE` | `compose.yaml` beside the installed controller |
| `CARGO_BUILD_JOBS` | Docker build concurrency, defaults to 4 |
| `SOLIDDRAFT_WEB_BUILDER` | Optional executable replacing `docker compose build` |

A custom builder receives `SOLIDDRAFT_WEB_SRC` and `SOLIDDRAFT_COMMIT`. It must build
the image `soliddraft-web:candidate`, with the revision label and runtime environment
described above, listening on port 8766 with the web application included. This allows
host toolchains and shared compiler caches without baking host paths into the Dockerfile.
The runtime image must match the builder's ABI and include TLS certificate roots.
The builder owns its temporary files and cache cleanup.

Update the installed controller, compose file and units explicitly after changes to
`deploy/web/`. Deploying an application tag does not replace these operational files.

### Manual deploy and rollback

```sh
~/.local/share/soliddraft-web/deploy.sh v0.1.0  # explicit ref, including a failed retry
systemctl --user list-timers soliddraft-web-deploy.timer
journalctl --user -u soliddraft-web-deploy.service
curl -fsS http://127.0.0.1:8766/version
```

To roll back, stop the timer first and retain the same port and environment overrides
used for deployment:

```sh
systemctl --user stop soliddraft-web-deploy.timer
docker tag soliddraft-web:previous soliddraft-web:latest
docker compose -p soliddraft-web -f ~/.local/share/soliddraft-web/compose.yaml \
  up -d --no-build --force-recreate
```

The image carries the commit, so `/version` still identifies a rolled-back build.
Resuming the timer restores the newest release unless that commit has a failed record.
