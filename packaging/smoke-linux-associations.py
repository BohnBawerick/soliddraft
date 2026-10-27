#!/usr/bin/env python3
"""Check an extracted Linux package through isolated native MIME databases."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile


def run(*command, env):
    return subprocess.check_output(command, env=env, text=True, stderr=subprocess.STDOUT)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package_root", type=Path)
    args = parser.parse_args()
    root = args.package_root.resolve()
    share = root / "usr/share"
    desktop = share / "applications/soliddraft-app.desktop"
    if not (share / "mime/packages/soliddraft.xml").is_file():
        raise ValueError("Package is missing its shared MIME definitions")
    with tempfile.TemporaryDirectory() as directory:
        scratch = Path(directory)
        env = dict(os.environ, LC_ALL="C", XDG_DATA_DIRS=str(share),
                   XDG_DATA_HOME=str(scratch / "data"), XDG_CONFIG_HOME=str(scratch / "config"),
                   XDG_CONFIG_DIRS=str(scratch / "config"), XDG_CURRENT_DESKTOP="",
                   GIO_USE_VFS="local", PATH=str(root / "usr/bin") + os.pathsep + os.environ["PATH"])
        run("desktop-file-validate", str(desktop), env=env)
        run("update-mime-database", str(share / "mime"), env=env)
        run("update-desktop-database", str(share / "applications"), env=env)
        detected = {}
        for extension, mime in (("dwprt", "application/vnd.soliddraft.part"),
                                ("dwdrw", "application/vnd.soliddraft.drawing")):
            sample = scratch / f"document.{extension}"
            # MIME registration depends on the extension, not document validity.
            sample.write_text("{}\n")
            info = run("gio", "info", "--attributes=standard::content-type", str(sample), env=env)
            if f"standard::content-type: {mime}" not in {line.strip() for line in info.splitlines()}:
                raise ValueError(f"Wrong MIME type for .{extension}: {info}")
            association = run("gio", "mime", mime, env=env)
            if "soliddraft-app.desktop" not in association.splitlines()[0]:
                raise ValueError(f"SolidDraft is not the default handler for {mime}: {association}")
            detected[extension] = mime
        assembly = scratch / "reserved.dwasm"
        assembly.write_text("{}\n")
        info = run("gio", "info", "--attributes=standard::content-type", str(assembly), env=env)
        if any(mime in info for mime in detected.values()):
            raise ValueError(f"Reserved assembly extension claimed by SolidDraft: {info}")
        association = run("gio", "mime", "application/vnd.soliddraft.assembly", env=env)
        if "soliddraft-app.desktop" in association:
            raise ValueError("Reserved assembly MIME type claimed by SolidDraft")
        print(json.dumps({"associations": detected, "assembly_association": False}))


if __name__ == "__main__":
    main()
