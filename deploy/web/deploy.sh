#!/usr/bin/env bash
# Install this controller outside the source clone so older release tags need not
# contain it. Builds and checks a candidate before replacing the live container.
set -euo pipefail

src=${SOLIDDRAFT_WEB_SRC:-$HOME/.local/share/soliddraft-web/src}
state=${SOLIDDRAFT_WEB_STATE:-${XDG_STATE_HOME:-$HOME/.local/state}/soliddraft-web}
repo=${SOLIDDRAFT_REPO:-https://github.com/BohnBawerick/soliddraft.git}
port=${SOLIDDRAFT_WEB_PORT:-127.0.0.1:8766}
# The compose file is installed beside this script, independently of release tags.
compose_file=${SOLIDDRAFT_WEB_COMPOSE:-$(dirname "$(realpath "$0")")/compose.yaml}
export SOLIDDRAFT_WEB_SRC="$src"
export SOLIDDRAFT_WEB_CONTEXT="$src"
serving=

compose() { docker compose -f "$compose_file" "$@"; }
live() { SOLIDDRAFT_WEB_TAG=latest SOLIDDRAFT_WEB_PORT="$port" compose -p soliddraft-web "$@"; }
candidate() { SOLIDDRAFT_WEB_TAG=candidate SOLIDDRAFT_WEB_PORT=127.0.0.1: compose -p soliddraft-web-candidate "$@"; }

served_commit() {
  local id
  id=$(live ps -q web)
  [ -n "$id" ] || return 0
  docker inspect --format '{{index .Config.Labels "org.opencontainers.image.revision"}}' "$id"
}

healthy() {
  local base=$1 commit=$2 page wasm
  for _ in $(seq 1 60); do
    if [ "$(curl --max-time 5 -fsS "$base/version" 2>/dev/null | jq -r .commit 2>/dev/null)" = "$commit" ]; then
      curl --max-time 10 -fsS "$base/api/info" | jq -e '.assistant.on | type == "boolean"' >/dev/null || return 1
      page=$(curl --max-time 10 -fsS "$base/") || return 1
      wasm=$(grep -oE '/[A-Za-z0-9_-]+_bg\.wasm' <<<"$page") || return 1
      # Trunk names the same module in both its preload link and initialization.
      wasm=${wasm%%$'\n'*}
      curl --max-time 30 -fsS -o /dev/null "$base$wasm" || return 1
      return 0
    fi
    sleep 2
  done
  echo "Health check failed at $base for $commit" >&2
  return 1
}

cleanup() {
  candidate down --timeout 5 >/dev/null 2>&1 || true
}

fail() {
  printf '%s\n' "$1" >"$state/failed"
  echo "Deploy of $1 failed: $2" >&2
  docker image rm soliddraft-web:candidate >/dev/null 2>&1 || true
  if [ -n "$serving" ]; then git -C "$src" checkout --quiet --detach "$serving"; fi
}
deploy() {
  local commit old_image new_image older_image address
  commit=$(git -C "$src" rev-parse HEAD)
  export SOLIDDRAFT_COMMIT="$commit"
  echo "Building SolidDraft web at $commit"
  # A custom executable may build with host toolchains and shared compiler caches.
  # It must produce soliddraft-web:candidate with the revision label and runtime env.
  if [ -n "${SOLIDDRAFT_WEB_BUILDER:-}" ]; then
    if ! "$SOLIDDRAFT_WEB_BUILDER"; then fail "$commit" "build failed"; return 1; fi
  elif ! SOLIDDRAFT_WEB_TAG=candidate compose build; then
    fail "$commit" "build failed"; return 1
  fi
  if ! candidate up -d --no-build; then fail "$commit" "candidate did not start"; return 1; fi
  address=$(candidate port web 8766)
  if ! healthy "http://$address" "$commit"; then
    candidate logs --tail 40 >&2 || true
    cleanup
    fail "$commit" "candidate health check failed"
    return 1
  fi
  candidate down --timeout 5 >/dev/null
  old_image=$(docker inspect --format '{{.Image}}' "$(live ps -q web)" 2>/dev/null || true)
  docker tag soliddraft-web:candidate soliddraft-web:latest
  if ! live up -d --no-build --force-recreate || ! healthy "http://127.0.0.1:${port##*:}" "$commit"; then
    live logs --tail 40 >&2 || true
    if [ -n "$old_image" ]; then
      docker tag "$old_image" soliddraft-web:latest
      live up -d --no-build --force-recreate
      healthy "http://127.0.0.1:${port##*:}" "$serving" || echo 'Rollback health check failed' >&2
    else
      live down --timeout 5
    fi
    fail "$commit" "live health check failed; restored previous image if available"
    return 1
  fi
  new_image=$(docker image inspect --format '{{.Id}}' soliddraft-web:latest)
  older_image=$(docker image inspect --format '{{.Id}}' soliddraft-web:previous 2>/dev/null || true)
  if [ -n "$old_image" ] && [ "$old_image" != "$new_image" ]; then docker tag "$old_image" soliddraft-web:previous; fi
  docker image rm soliddraft-web:candidate >/dev/null
  if [ -n "$older_image" ] && [ "$older_image" != "$old_image" ] && [ "$older_image" != "$new_image" ]; then
    docker image rm "$older_image" >/dev/null 2>&1 || true
  fi
  rm -f "$state/failed"
  printf '%s\n' "$commit" >"$state/serving"
  echo "SolidDraft web $commit is serving on $port"
}

main() {
  local mode=${1:---latest-release} ref commit release previous
  case "$mode" in --latest-release) ;; --*) echo "Usage: $0 [--latest-release | REF]" >&2; return 2 ;; esac
  mkdir -p "$state"
  exec 9>"$state/deploy.lock"
  if [ "$mode" = --latest-release ]; then flock -n 9 || return 0; else flock 9; fi
  if [ ! -d "$src/.git" ]; then
    mkdir -p "$(dirname "$src")"
    git clone --quiet "$repo" "$src"
  fi
  git -C "$src" fetch --quiet --prune --tags origin \
    '+refs/heads/main:refs/remotes/origin/main' \
    '+refs/heads/release/*:refs/remotes/origin/release/*'
  serving=$(served_commit)
  if [ "$mode" = --latest-release ]; then
    # Only stable vMAJOR.MINOR.PATCH tags. Version order, not tag creation time.
    release=$(git -C "$src" tag --list | grep -E '^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' | sort -V | tail -n 1 || true)
    if [ -z "$release" ]; then
      # A preview is deployed once. Main changes alone never trigger timer builds.
      [ -z "$serving" ] || return 0
      ref=origin/main
    else
      previous=$(cat "$state/release" 2>/dev/null || true)
      if [ -n "$previous" ] && [ "$(printf '%s\n' "$previous" "$release" | sort -V | tail -n 1)" != "$release" ]; then return 0; fi
      ref=refs/tags/$release
      if ! git -C "$src" merge-base --is-ancestor "$ref^{commit}" origin/main &&
        ! git -C "$src" merge-base --is-ancestor "$ref^{commit}" "origin/release/${release#v}" 2>/dev/null; then
        echo "Release $release is not on main or release/${release#v}" >&2
        return 1
      fi
    fi
  else
    git -C "$src" fetch --quiet origin "$mode"
    ref=FETCH_HEAD
    release=
  fi
  commit=$(git -C "$src" rev-parse "$ref^{commit}")
  if [ "$mode" = --latest-release ]; then
    if [ "$commit" = "$serving" ]; then
      if [ -n "$release" ]; then printf '%s\n' "$release" >"$state/release"; fi
      return 0
    fi
    [ "$commit" != "$(cat "$state/failed" 2>/dev/null || true)" ] || return 0
  fi
  git -C "$src" checkout --quiet --detach "$commit"
  trap cleanup EXIT
  deploy
  if [ -n "$release" ]; then printf '%s\n' "$release" >"$state/release"; fi
}

main "$@"; exit
