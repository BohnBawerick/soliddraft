#!/usr/bin/env python3
"""Independent fixtures and decoders for the configuration grid scenarios.

The expected rows come from the designer's generator, never from the application. The
decoders read exported CSV, OOXML cells and SVG text with Python's standard library.
"""

import argparse
import csv
import hashlib
import importlib.util
import io
import json
import pathlib
import subprocess
import sys
import zipfile
from xml.etree import ElementTree as E

GENERATOR = pathlib.Path(__file__).with_name("configuration-grid-fixtures.py")
_spec = importlib.util.spec_from_file_location("configuration_grid_fixtures", GENERATOR)
generator = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(generator)

NS = {"s": generator.NS}
UNICODE_TITLE = 'Bracket Ø12, "wide"'


def expected(name):
    rows = {"g0": generator.G0, "g1": generator.G1}[name.removesuffix("-unicode")]
    rows = [row[:] for row in rows]
    if name.endswith("-unicode"):
        rows[-1][-1] = UNICODE_TITLE
    return rows


def fixtures(directory):
    subprocess.run([sys.executable, str(GENERATOR), str(directory)], check=True, stdout=subprocess.DEVNULL)

    def both(name, rows):
        (directory / f"{name}.csv").write_bytes(generator.csv_bytes(rows))
        (directory / f"{name}.xlsx").write_bytes(generator.xlsx(rows))

    g1 = expected("g1")
    (directory / "short-record.csv").write_bytes(generator.csv_bytes(g1[:-1] + [g1[-1][:-1]]))
    both("duplicate-row", g1 + [g1[2][:]])
    both("duplicate-header", [row + [row[1]] for row in g1])
    unknown = [row[:] for row in g1]
    unknown[0][1] = "value:missing_w [mm]"
    both("unknown-value", unknown)
    cycle = [row[:] for row in g1]
    cycle[-1][1] = "flange_d * 2"
    both("cycle", cycle)
    both("unicode", expected("g1-unicode"))


def workbook_rows(path):
    with zipfile.ZipFile(path) as z:
        assert z.testzip() is None, "a workbook member fails its CRC"
        book = E.fromstring(z.read("xl/workbook.xml"))
        names = [s.get("name") for s in book.findall("s:sheets/s:sheet", NS)]
        assert names == ["Configurations"], f"sheets are {names}"
        shared = []
        if "xl/sharedStrings.xml" in z.namelist():
            root = E.fromstring(z.read("xl/sharedStrings.xml"))
            shared = ["".join(t.text or "" for t in si.iter(f"{{{generator.NS}}}t"))
                      for si in root.findall("s:si", NS)]
        sheet = E.fromstring(z.read("xl/worksheets/sheet1.xml"))
    assert sheet.find(".//s:f", NS) is None, "an exported cell is a formula"
    table = {}
    for cell in sheet.iter(f"{{{generator.NS}}}c"):
        reference = cell.get("r")
        column = ord(reference[0]) - 65
        row = int(reference[1:]) - 1
        kind = cell.get("t")
        assert kind in ("inlineStr", "s"), f"{reference} is not a string cell: {kind}"
        if kind == "s":
            text = shared[int(cell.find("s:v", NS).text)]
        else:
            text = "".join(t.text or "" for t in cell.iter(f"{{{generator.NS}}}t"))
        table[row, column] = text
    height = 1 + max(r for r, _ in table)
    width = 1 + max(c for _, c in table)
    return [[table.get((r, c), "") for c in range(width)] for r in range(height)]


def grid(path, name):
    if path.suffix == ".csv":
        text = path.read_bytes().decode("utf-8")
        rows = list(csv.reader(io.StringIO(text, newline="")))
    else:
        assert zipfile.is_zipfile(path), "the export is not an OOXML package"
        rows = workbook_rows(path)
    want = expected(name)
    assert rows == want, f"decoded {rows}, expected {want}"
    print(json.dumps({"file": path.name, "rows": len(rows), "columns": len(rows[0]), "matches": name}))


def unchanged(path, record):
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    if record.exists():
        assert record.read_text() == digest, f"{path.name} changed"
    else:
        record.write_text(digest)
    print(json.dumps({"file": path.name, "sha256": digest}))


def title(path, wanted):
    texts = ["".join(node.itertext()) for node in E.parse(path).getroot().iter()
             if node.tag.rsplit("}", 1)[-1] == "text"]
    assert wanted in texts, f"{wanted!r} is not among {len(texts)} SVG text items"
    print(json.dumps({"file": path.name, "title": wanted}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("fixtures").add_argument("directory", type=pathlib.Path)
    for name, second, kind in (("grid", "name", str), ("title", "text", str), ("unchanged", "record", pathlib.Path)):
        command = commands.add_parser(name)
        command.add_argument("file", type=pathlib.Path)
        command.add_argument(second, type=kind)
    args = parser.parse_args()
    if args.command == "fixtures":
        args.directory.mkdir(parents=True, exist_ok=True)
        fixtures(args.directory)
    elif args.command == "grid":
        grid(args.file, args.name)
    elif args.command == "unchanged":
        unchanged(args.file, args.record)
    else:
        title(args.file, args.text)


if __name__ == "__main__":
    main()
