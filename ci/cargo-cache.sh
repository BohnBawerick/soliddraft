#!/usr/bin/env bash
# Runs a cargo command in this job's persistent target directory. See docs/ci.md.
#
#   ci/cargo-cache.sh cargo <subcommand> [args...]
#
# CARGO_TARGET_DIR names the job's directory on the shared target volume, and
# TARGET_CACHE_MAX_GB its size cap. The script:
#
# 1. Locks the directory, so a job of the same kind on another runner waits for it.
# 2. Deletes the directory when the Rust toolchain has changed since its last build, since
#    nothing in it can be reused, or when it has grown past the cap.
# 3. Sets the modification time of every file in the checkout (see below).
# 4. Runs the command with --timings.
# 5. After a successful build, deletes the builds it did not use (see prune below).
# 6. Reports how many build units were fresh, what pruning
#    freed and the directory's size.
#
# Cargo decides whether a workspace crate is fresh by comparing its source files'
# modification times with its last build. The four runners have four checkouts, and a
# file in one of them can hold different content from the last build with an older
# time, so cargo would reuse a stale build. The script therefore records what each
# successful build was built from. When the next build is of the same sources with the
# same toolchain and command, every file gets an old time and cargo rebuilds nothing.
# Otherwise every file gets the current time and cargo rebuilds the workspace crates;
# dependencies from crates.io and git are keyed by version, not by time, and stay.
set -euo pipefail

if [[ ${1:-} != cargo || -z ${2:-} ]]; then
    echo "usage: ci/cargo-cache.sh cargo <subcommand> [args...]" >&2
    exit 2
fi
target=${CARGO_TARGET_DIR:?CARGO_TARGET_DIR is not set}
max_gb=${TARGET_CACHE_MAX_GB:?TARGET_CACHE_MAX_GB is not set, in whole gigabytes}
root=$(git rev-parse --show-toplevel)
stamp=$target/soliddraft-ci-stamp
toolchain_file=$target/soliddraft-ci-toolchain
# Older than any build, newer than zero, which some tools treat as missing.
old_time=@946684800

mkdir -p "$target"
exec 9>"$target.lock"
if ! flock --nonblock 9; then
    echo "waiting for another job that is using $target"
    flock 9
fi

toolchain=$(rustc -vV)
used_kb=$(du -sk "$target" | cut -f1)
if [[ -f $toolchain_file && $(<"$toolchain_file") != "$toolchain" ]]; then
    echo "$target was built by another Rust toolchain; deleting it"
    rm -rf -- "$target"
elif ((used_kb > max_gb * 1024 * 1024)); then
    echo "$target holds $((used_kb / 1024)) MB, over the ${max_gb} GB cap; deleting it"
    rm -rf -- "$target"
fi
mkdir -p "$target"
echo "$toolchain" >"$toolchain_file"

# Everything that decides what the build produces.
sources_id() {
    git -C "$root" ls-files --stage --recurse-submodules
    echo "$toolchain"
    cargo -V
    printf '%s\n' "$@"
    env | grep -E '^(CARGO|RUST|CC=|CXX=)' | grep -vE '^(CARGO_TERM_|RUSTUP_)' | sort
}

dirty=$(git -C "$root" status --porcelain --ignore-submodules=none)
id=$(sources_id "$@" | sha256sum | cut -d' ' -f1)
if [[ -z $dirty && -f $stamp && $(<"$stamp") == "$id" ]]; then
    echo "same sources as the last build in $target; marking every file as old"
    when=$old_time
else
    echo "sources differ from the last build in $target; marking every file as new"
    when=now
fi
rm -f -- "$stamp"
(
    cd "$root"
    git ls-files -z --recurse-submodules | xargs -0 touch -c -d "$when" --
    # Directories too, since a build script can watch a whole directory.
    git ls-files -z --recurse-submodules | xargs -0 dirname -z | sort -zu | xargs -0 touch -c -d "$when" --
)

# Deletes the builds in $target that the command just run did not use, such as old
# versions of dependencies. Cargo lists every unit of a build, fresh ones included, with
# --message-format=json, and names each unit's fingerprint directory and output files
# with the same 16-digit hash. A binary that tests need is listed by its copy outside
# deps/, a hard link to the hashed file. Prints what it did.
prune() {
    local -A keep=() fingerprints=()
    local units=0 missing=() before path linked name hash paths unit_hashes
    local extra=() listing
    [[ $2 == test ]] && extra=(--no-run)
    # Everything is fresh now, so this only lists the units, one line of paths each. A
    # partial list would delete builds in use, so any failure stops here.
    listing=$(mktemp)
    if ! "$1" "$2" --message-format=json "${extra[@]}" "${@:3}" 2>/dev/null >"$listing.json" ||
        ! jq -r 'select(.reason == "compiler-artifact" or .reason == "build-script-executed")
            | [.target.name // "build script run"] + (.filenames // [.out_dir]) | @tsv' \
            "$listing.json" >"$listing"; then
        rm -f -- "$listing" "$listing.json"
        echo "skipped, cargo did not list the build's units"
        return
    fi
    for path in "$target"/debug/.fingerprint/*; do
        [[ ${path##*/} =~ -([0-9a-f]{16})$ ]] && fingerprints[${BASH_REMATCH[1]}]=1
    done
    while IFS=$'\t' read -r -a paths; do
        name=${paths[0]}
        units=$((units + 1))
        unit_hashes=()
        for path in "${paths[@]:1}"; do
            # deps/libfoo-<hash>.rlib, build/foo-<hash>/out and the like.
            if [[ ${path#"$target"/debug/} =~ ^[^/]+/[^/]*-([0-9a-f]{16})($|[./]) ]]; then
                unit_hashes+=("${BASH_REMATCH[1]}")
            elif [[ -e $path ]]; then
                while read -r linked; do
                    [[ ${linked##*/} =~ -([0-9a-f]{16})($|\.) ]] && unit_hashes+=("${BASH_REMATCH[1]}")
                done < <(find "$target/debug/deps" -maxdepth 1 -samefile "$path")
            fi
        done
        hash=
        for hash in "${unit_hashes[@]}"; do
            keep[$hash]=1
            [[ -n ${fingerprints[$hash]:-} ]] && break
            hash=
        done
        [[ -z $hash ]] && missing+=("$name")
    done <"$listing"
    rm -f -- "$listing" "$listing.json"
    # A cargo that names files another way would lose every build here, so stop.
    if ((units == 0 || ${#missing[@]} > 0)); then
        echo "skipped, ${#missing[@]} of $units build units had no fingerprint: ${missing[*]:0:5}"
        return
    fi
    before=$(du -sm "$target" | cut -f1)
    for path in "$target"/debug/{.fingerprint,build,deps,examples}/*; do
        name=${path##*/}
        if [[ $name =~ -([0-9a-f]{16})($|\.) && -z ${keep[${BASH_REMATCH[1]}]:-} ]]; then
            rm -rf -- "$path"
        fi
    done
    echo "deleted $((before - $(du -sm "$target" | cut -f1))) MB of builds this command did not use"
}

rm -rf -- "$target/cargo-timings"
status=0
pruned=
# --timings goes straight after the subcommand, before any `--` for the compiler.
"$1" "$2" --timings "${@:3}" || status=$?
if ((status == 0)); then
    pruned=$(prune "$@" || echo "failed")
    echo "$id" >"$stamp"
fi

report=$(mktemp)
{
    echo "### Build cache: ${target##*/}"
    echo
    timings=("$target"/cargo-timings/cargo-timing-*.html)
    timing=${timings[0]}
    if [[ -f $timing ]]; then
        units=$(grep -oE 'Total units:</td><td>[0-9]+|Fresh units:</td><td>[0-9]+|Dirty units:</td><td>[0-9]+' "$timing" |
            sed 's/:<\/td><td>/: /' | paste -sd ',' | sed 's/,/, /g')
        echo "- Cargo: ${units:-no unit counts in the timings report}"
    fi
    if [[ -n $pruned ]]; then
        echo "- Pruning: $pruned"
    fi
    echo "- Size: $(du -sh "$target" | cut -f1) of the ${max_gb} GB cap"
} >"$report"
used_kb=$(du -sk "$target" | cut -f1)
if ((status == 0 && used_kb > max_gb * 1024 * 1024)); then
    # The build alone is over the cap, so every job of this kind would start cold.
    echo "::warning title=Build cache over its cap::${target##*/} needs $((used_kb / 1024)) MB, over its ${max_gb} GB cap, so the next job will delete it and build from scratch. Raise TARGET_CACHE_MAX_GB in the calling workflow if disk space allows."
fi
cat "$report"
if [[ -n ${GITHUB_STEP_SUMMARY:-} ]]; then
    cat "$report" >>"$GITHUB_STEP_SUMMARY"
fi
rm -f -- "$report"
exit "$status"
