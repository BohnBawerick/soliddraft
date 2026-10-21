# The web build of SolidDraft and its server, in one image. Build it from the repository
# root with `docker compose -f deploy/web/compose.yaml build`;
# docs/web.md describes the setup.
#
# Three build stages:
# - kernel: the SMLib kernel as its own WebAssembly module, with Emscripten.
# - build: the server, natively with the SMLib kernel, and the app to WebAssembly with
#   trunk.
# - the runtime image: the server and the files it serves.
# Cache mounts keep cargo's registry and build output between builds, so a rebuild after
# a small change takes minutes, not the full kernel build.

ARG RUST_VERSION=1.99.0

FROM emscripten/emsdk:6.0.12 AS kernel
ARG RUST_VERSION
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal \
      --default-toolchain "${RUST_VERSION}" --target wasm32-unknown-emscripten
WORKDIR /src
COPY . .
ARG BUILD_JOBS=4
RUN --mount=type=cache,target=/opt/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -j "${BUILD_JOBS}" -p soliddraft-kernel-wasm --target wasm32-unknown-emscripten \
 && mkdir -p /out/kernel \
 && cp target/wasm32-unknown-emscripten/release/soliddraft-kernel-wasm.js \
       target/wasm32-unknown-emscripten/release/soliddraft_kernel_wasm.wasm /out/kernel/

FROM rust:${RUST_VERSION}-bookworm AS build

# trunk builds the web app. It downloads the matching wasm-bindgen and wasm-opt itself.
ARG TRUNK_VERSION=0.21.14
ARG TRUNK_SHA256=a67f4054b249fe9acc5fabc25de1aebf19783aca3ad6ff64bf34d7da44d0ea20
RUN rustup target add wasm32-unknown-unknown \
 && curl -sSfL -o /tmp/trunk.tgz \
      "https://github.com/trunk-rs/trunk/releases/download/v${TRUNK_VERSION}/trunk-x86_64-unknown-linux-musl.tar.gz" \
 && echo "${TRUNK_SHA256}  /tmp/trunk.tgz" | sha256sum -c - \
 && tar xzf /tmp/trunk.tgz -C /usr/local/bin \
 && rm /tmp/trunk.tgz

WORKDIR /src
COPY . .
ARG BUILD_JOBS=4
ENV CARGO_BUILD_JOBS=${BUILD_JOBS}
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p soliddraft-server \
 && cp target/release/soliddraft-server /out-server \
 && cd crates/soliddraft-web \
 && trunk build --release --locked --dist /out/web

FROM debian:bookworm-slim
# CA certificates for the assistant's calls to the model provider.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /out-server /usr/local/bin/soliddraft-server
COPY --from=build /out/web /srv/soliddraft/web
COPY --from=kernel /out/kernel /srv/soliddraft/web/kernel
USER nobody
EXPOSE 8766
ENV SOLIDDRAFT_NO_KEYCHAIN=1
# The commit the image was built from: GET /version reports it and image inspection reads the
# label. Only this stage takes it, so the build stages above do not depend on it.
ARG SOLIDDRAFT_COMMIT=
LABEL org.opencontainers.image.revision=${SOLIDDRAFT_COMMIT}
ENV SOLIDDRAFT_COMMIT=${SOLIDDRAFT_COMMIT}
CMD ["soliddraft-server", "--listen", "0.0.0.0:8766", "--web", "/srv/soliddraft/web"]
