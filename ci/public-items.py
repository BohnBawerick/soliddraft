#!/usr/bin/env python3
"""Compare documented public items and members at the S0 base and candidate."""

import html.parser
import pathlib
import re
import sys

from refactor_baseline import ROOT, at_base, cargo


class Items(html.parser.HTMLParser):
    def __init__(self):
        super().__init__()
        self.items = set()
        self.in_list = False

    def handle_starttag(self, tag, attrs):
        if tag == 'li':
            self.in_list = True
        href = dict(attrs).get('href', '')
        if self.in_list and tag == 'a' and href.endswith('.html'):
            self.items.add(href)

    def handle_endtag(self, tag):
        if tag == 'li':
            self.in_list = False


def snapshot():
    cargo('doc', '--no-deps', '--locked', '-p', 'soliddraft-model', '-p', 'soliddraft-app')
    result = set()
    for crate in ('soliddraft_model', 'soliddraft_app'):
        directory = ROOT / 'target/doc' / crate
        parser = Items()
        parser.feed((directory / 'all.html').read_text())
        for item in parser.items:
            result.add((crate, item, ''))
            text = (directory / pathlib.PurePosixPath(item)).read_text()
            for anchor in re.findall(r'id="((?:method|tymethod|variant|structfield|associatedconstant)\.[^"]+)"', text):
                result.add((crate, item, anchor))
    return result


if __name__ == '__main__':
    with at_base(sys.argv[1]):
        before = snapshot()
    after = snapshot()
    for item in sorted(before - after):
        print('removed:', *item)
    for item in sorted(after - before):
        print('added:', *item)
    print(f'{len(before)} base public items/members; {len(after)} candidate public items/members')
    raise SystemExit(before != after)
