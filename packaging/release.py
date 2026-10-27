#!/usr/bin/env python3
"""Release-only metadata, archives and integrity checks. No application hooks."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parent.parent
TARGETS = {
    "linux": ("x86_64-unknown-linux-gnu", ["AppImage", "deb"]),
    "windows": ("x86_64-pc-windows-msvc", ["msi"]),
    "macos": ("universal-apple-darwin", ["dmg"]),
}


def version():
    value = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    if not re.fullmatch(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", value):
        raise ValueError("Release versions must be X.Y.Z, without dates or suffixes")
    return value


def prefix(platform):
    architecture = {"linux": "linux-x86_64", "windows": "windows-x64", "macos": "macos-universal", "web": "web"}
    return f"soliddraft-{version()}-{architecture[platform]}"


def configure(platform):
    target, _ = TARGETS[platform]
    config = json.loads((ROOT / "packaging/Packager.json").read_text())
    config.update(version=version(), targetTriple=target,
                  binariesDir=str(ROOT / "target" / target / "release"),
                  outDir=str(ROOT / "target/packages"))
    # Only the protected tag path supplies signing secrets. Partial credentials fail closed.
    if platform == "macos" and os.environ.get("APPLE_CERTIFICATE"):
        for key in ("APPLE_CERTIFICATE_PASSWORD", "APPLE_SIGNING_IDENTITY", "APPLE_ID", "APPLE_PASSWORD", "APPLE_TEAM_ID"):
            if not os.environ.get(key):
                raise ValueError(f"Missing {key} for signed, notarized macOS distribution")
        config["macos"]["signingIdentity"] = os.environ["APPLE_SIGNING_IDENTITY"]
    if platform == "windows" and os.environ.get("WINDOWS_CERTIFICATE_THUMBPRINT"):
        config["windows"].update(certificateThumbprint=os.environ["WINDOWS_CERTIFICATE_THUMBPRINT"],
                                 digestAlgorithm="sha256", tsp=True,
                                 timestampUrl="http://timestamp.digicert.com")
    # Keep beside the template so its resource paths stay relative to packaging/.
    (ROOT / "packaging/Packager.generated.json").write_text(json.dumps(config, indent=2) + "\n")


def archive(platform, source):
    output = ROOT / "dist"
    output.mkdir(exist_ok=True)
    if platform == "linux":
        with tarfile.open(output / f"{prefix(platform)}.tar.gz", "w:gz") as package:
            for path in (source / "soliddraft-app", ROOT / "LICENSE", ROOT / "NOTICE"):
                package.add(path, arcname=f"{prefix(platform)}/{path.name}")
    else:
        with zipfile.ZipFile(output / f"{prefix(platform)}.zip", "w", zipfile.ZIP_DEFLATED) as package:
            files = sorted(source.rglob("*")) if platform == "web" else [source / "soliddraft-app.exe"]
            for path in files:
                if path.is_file():
                    package.write(path, path.relative_to(source))
            for path in (ROOT / "LICENSE", ROOT / "NOTICE"):
                package.write(path, path.name)


def collect(platform):
    output = ROOT / "dist"
    output.mkdir(exist_ok=True)
    for extension in TARGETS[platform][1]:
        matches = list((ROOT / "target/packages").glob(f"*.{extension}"))
        if len(matches) != 1:
            raise ValueError(f"Expected one {extension} package, found {matches}")
        shutil.copy2(matches[0], output / f"{prefix(platform)}.{extension}")
    if platform != "macos":
        archive(platform, ROOT / "target" / TARGETS[platform][0] / "release")


def checksums():
    output = ROOT / "dist"
    expected = {
        f"{prefix('linux')}.AppImage", f"{prefix('linux')}.deb", f"{prefix('linux')}.tar.gz",
        f"{prefix('windows')}.msi", f"{prefix('windows')}.zip", f"{prefix('macos')}.dmg",
        f"{prefix('web')}.zip", "release-notes.md", f"soliddraft-{version()}.spdx.json",
    }
    actual = {p.name for p in output.iterdir() if p.is_file()}
    if actual != expected:
        raise ValueError(f"Release inventory mismatch: missing={expected - actual}, extra={actual - expected}")
    lines = []
    for name in sorted(expected):
        with (output / name).open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        lines.append(f"{digest}  {name}\n")
    (output / "SHA256SUMS").write_text("".join(lines))


def formats(ref):
    path = "crates/soliddraft-document/src/kind.rs"
    text = subprocess.check_output(["git", "show", f"{ref}:{path}"], cwd=ROOT, text=True)
    found = re.findall(r'DocKind::(Part|Assembly|Drawing)\s*=>\s*"(soliddraft-[^"]+/\d+(?:\.\d+)?)"', text)
    if len(found) != 3:
        raise ValueError(f"Cannot read all document format versions at {ref}")
    return dict(found)


def previous_release(releases):
    candidates = []
    for tag in json.loads(releases.read_text()):
        if not re.fullmatch(r"v\d+\.\d+\.\d+", tag) or tag == f"v{version()}":
            continue
        if subprocess.run(["git", "merge-base", "--is-ancestor", tag, "HEAD"], cwd=ROOT).returncode == 0:
            distance = int(subprocess.check_output(["git", "rev-list", "--count", f"{tag}..HEAD"], cwd=ROOT))
            candidates.append((distance, tag))
    return min(candidates)[1] if candidates else ""


def pull_request_notes(pulls, previous):
    revision = f"{previous}..HEAD" if previous else "HEAD"
    commits = set(subprocess.check_output(["git", "rev-list", revision], cwd=ROOT, text=True).splitlines())
    merged = [pr for page in json.loads(pulls.read_text()) for pr in page
              if pr.get("merged_at") and pr.get("merge_commit_sha") in commits]
    merged.sort(key=lambda pr: pr["number"])
    if not merged:
        return "## Merged pull requests\n\nNo merged pull requests in this revision range.\n"
    return "## Merged pull requests\n\n" + "\n".join(
        f"- {pr['title']} by @{pr['user']['login']} in {pr['html_url']}" for pr in merged
    )


def source_revisions():
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    adapter = ROOT / "crates/soliddraft-kernel-catallax"
    manifest = tomllib.loads((adapter / "Cargo.toml").read_text())
    pins = {dependency["rev"] for name, dependency in manifest["dependencies"].items()
            if name.startswith("catallax-")}
    if len(pins) != 1 or not re.fullmatch(r"[0-9a-f]{40}", next(iter(pins))):
        raise ValueError("Catallax dependencies must share one full commit pin")
    kernel = pins.pop()
    declared = re.findall(r'^pub const CATALLAX_REVISION: &str = "([0-9a-f]{40})";$',
                          (adapter / "src/lib.rs").read_text(), re.MULTILINE)
    if declared != [kernel]:
        raise ValueError("Catallax backend identity differs from its dependency pin")
    locked = tomllib.loads((ROOT / "Cargo.lock").read_text())["package"]
    expected = f"git+https://github.com/BohnBawerick/catallax?rev={kernel}#{kernel}"
    sources = {package.get("source") for package in locked
               if package["name"].startswith("catallax-")}
    if sources != {expected}:
        raise ValueError("Locked Catallax sources differ from the dependency pin")
    return revision, kernel


def notes(pulls, previous):
    current = formats("HEAD")
    old = formats(previous) if previous else {}
    revision, kernel = source_revisions()
    lines = [f"# SolidDraft {version()}{' alpha' if version().startswith('0.') else ''}", "",
             "Alpha software. Keep backups of your documents before opening or saving them in a newer release.", "",
             "## Source revisions", "",
             f"- SolidDraft: `{revision}`.",
             f"- Catallax dependency and lockfile: `{kernel}`.", "",
             "These identify the packaged sources, not a claim of completed application acceptance.", "",
             "## Document formats", ""]
    for kind, value in current.items():
        change = f"changed from {old[kind]}" if kind in old and old[kind] != value else "unchanged" if old else "first release"
        lines.append(f"- {kind}: `{value}` ({change}).")
    lines += ["", "Files keep the `.dwprt`, `.dwasm` and `.dwdrw` extensions.", "",
              "## Installation and verification", "",
              "Unsigned downloads trigger Windows SmartScreen and macOS Gatekeeper warnings. "
              "Signing is enabled only when the release credentials are configured. "
              "Check SHA256SUMS and verify GitHub build attestations before installing.", "",
              "The browser zip runs the kernel locally. GitHub Pages has no assistant relay or API keys.", "",
              pull_request_notes(pulls, previous)]
    (ROOT / "dist").mkdir(exist_ok=True)
    (ROOT / "dist/release-notes.md").write_text("\n".join(lines) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("version")
    commands.add_parser("previous").add_argument("releases", type=Path)
    for name in ("configure", "collect"):
        commands.add_parser(name).add_argument("platform", choices=TARGETS)
    commands.add_parser("web").add_argument("source", type=Path)
    commands.add_parser("checksums")
    note = commands.add_parser("notes")
    note.add_argument("pulls", type=Path)
    note.add_argument("--previous")
    args = parser.parse_args()
    if args.command == "version":
        print(version())
    elif args.command == "previous":
        print(previous_release(args.releases))
    elif args.command == "configure":
        configure(args.platform)
    elif args.command == "collect":
        collect(args.platform)
    elif args.command == "web":
        archive("web", args.source)
    elif args.command == "checksums":
        checksums()
    elif args.command == "notes":
        notes(args.pulls, args.previous)


if __name__ == "__main__":
    main()
