#!/usr/bin/env python3
"""Renders the independent references for tests/ribbon_icons.rs.

source_icons.py is a byte copy of the prototype's icon set, and rsvg-convert shares no code
with the app's renderer, so a light reference cannot drift with the product. The prototype has
no dark form: dark references apply the documented change to the free colours, which only
the committed assets mark. Run this only to change the design on purpose.
"""
import hashlib
import json
import os
import pathlib
import subprocess
import tempfile

import source_icons as source

HERE = pathlib.Path(__file__).resolve().parent
FONTS = HERE.parents[2] / "assets/fonts"
SIZE = 96
DARK = {
    "#3A4658": "#C3CCD9",
    "#17408E": "#8FB4FF",
    "#0E6F67": "#5FD3C6",
    "#0E7C72": "#4FD1C2",
    "#1D9A8D": "#7BE8DB",
    "#0B5F58": "#39BFAF",
    "#C9EEE7": "#1F6B63",
    "#2F67DD": "#7FA8FF",
}
ASSETS = HERE.parents[2] / "assets/icons/cad"


def render(svg, out, env):
    out.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile("w", suffix=".svg") as f:
        f.write(svg)
        f.flush()
        subprocess.run(["rsvg-convert", "-w", str(SIZE), "-h", str(SIZE), "-o", str(out), f.name], check=True, env=env)


def main():
    conf = tempfile.NamedTemporaryFile("w", suffix=".conf")
    conf.write(f"<fontconfig><dir>{FONTS}</dir></fontconfig>")
    conf.flush()
    env = dict(os.environ, FONTCONFIG_FILE=conf.name)
    ns = 'xmlns="http://www.w3.org/2000/svg"'
    cad = {}
    for key, fn in source.CAD.items():
        source.HOLES.clear()
        body, view = source.fitted(fn)
        cad[key] = view
        svg = f'<svg {ns} viewBox="{view}">{body}</svg>'
        render(svg, HERE / "light/cad" / f"{key}.png", env)
        dark = (ASSETS / f"{key}.svg").read_text()
        for light, shade in DARK.items():
            dark = dark.replace(light, shade)
        render(dark, HERE / "dark/cad" / f"{key}.png", env)
    for key, body in source.LINE.items():
        svg = (
            f'<svg {ns} viewBox="0 0 16 16"><g fill="none" stroke="#000000" stroke-width="1.3" '
            f'stroke-linecap="round" stroke-linejoin="round">{body.replace("currentColor", "#000000")}</g></svg>'
        )
        render(svg, HERE / "line" / f"{key}.png", env)
    version = subprocess.run(["rsvg-convert", "--version"], capture_output=True, text=True).stdout.splitlines()[0]
    manifest = {
        "source_sha256": hashlib.sha256((HERE / "source_icons.py").read_bytes()).hexdigest(),
        "renderer": version,
        "size": SIZE,
        "cad": cad,
        "line": sorted(source.LINE),
    }
    (HERE / "manifest.json").write_text(json.dumps(manifest, indent=1, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()
