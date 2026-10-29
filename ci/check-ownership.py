#!/usr/bin/env python3
"""Check Rust source references against the slice ownership map."""

import argparse
import bisect
import pathlib
import re
import tomllib


def tokens(source):
    """Discard Rust comments and literals while keeping source line numbers."""
    pattern = re.compile(
        r'//[^\n]*|/\*|(?:br|r)\#*"|b?"|b?\'(?:\\.|[^\'\\\n])\'|'
        r'[A-Za-z_][A-Za-z_0-9]*|::|[^\s]', re.MULTILINE
    )
    result = []
    newlines = [match.start() for match in re.finditer('\n', source)]
    string_end = re.compile(r'(?:\\.|[^"\\])*"', re.DOTALL)
    offset = 0
    while match := pattern.search(source, offset):
        word = match.group()
        offset = match.end()
        if word.startswith('//') or word.startswith("'") or word.startswith("b'"):
            continue
        if word == '/*':
            depth = 1
            while depth and (comment := re.search(r'/\*|\*/', source[offset:])):
                depth += 1 if comment.group() == '/*' else -1
                offset += comment.end()
            if depth:
                offset = len(source)
            continue
        if word.endswith('"'):
            if word in ('"', 'b"'):
                end = string_end.match(source, offset)
                offset = end.end() if end else len(source)
            else:
                end = source.find('"' + '#' * word.count('#'), offset)
                offset = end + 1 + word.count('#') if end >= 0 else len(source)
            continue
        result.append((word, bisect.bisect_left(newlines, match.start()) + 1))
    return result


def use_paths(words):
    """Expand a use tree into (path, alias, line) entries."""
    index = 0
    paths = []

    def tree(prefix):
        nonlocal index
        path = list(prefix)
        line = words[index][1] if index < len(words) else 1
        while index < len(words):
            word = words[index][0]
            if word == '{':
                index += 1
                while index < len(words) and words[index][0] != '}':
                    tree(path)
                    if index < len(words) and words[index][0] == ',':
                        index += 1
                index += 1
                return
            if word in (',', '}', ';'):
                break
            index += 1
            if word == 'as':
                alias = words[index][0]
                index += 1
                paths.append((path, alias, line))
                return
            if word != '::':
                path.append(word)
        if path:
            alias = path[-1]
            if alias == 'self':
                path = path[:-1]
                alias = path[-1] if path else 'self'
            paths.append((path, alias, line))

    tree([])
    return paths


def imports(words, public_only=False):
    found = []
    for i, (word, _) in enumerate(words):
        if word != 'use' or (public_only and (i == 0 or words[i - 1][0] != 'pub')):
            continue
        end = i + 1
        while end < len(words) and words[end][0] != ';':
            end += 1
        found.extend(use_paths(words[i + 1:end]))
    return found


def absolute(path, module, exports):
    if not path:
        return []
    if path[0] == 'crate':
        path = path[1:]
    elif path[0] == 'self':
        path = module + path[1:]
    elif path[0] == 'super':
        parent = list(module)
        while path and path[0] == 'super':
            parent = parent[:-1]
            path = path[1:]
        path = parent + path
    else:
        return []
    seen = set()
    while path and path[0] in exports and path[0] not in seen:
        seen.add(path[0])
        path = exports[path[0]] + path[1:]
    return path


def check(root):
    areas = tomllib.loads((root / 'ci/ownership.toml').read_text())['areas']
    violations = []
    for crate in ('soliddraft-app', 'soliddraft-model'):
        src = root / 'crates' / crate / 'src'
        exports = {}
        for path, alias, _ in imports(tokens((src / 'lib.rs').read_text()), public_only=True):
            exports[alias] = path[1:] if path[0] == 'crate' else path
        crate_areas = [area for area in areas if area['crate'] == crate]
        for file in sorted(src.rglob('*.rs')):
            relative = file.relative_to(root).as_posix()
            matches = [area for area in crate_areas if any(
                relative == path or (path.endswith('/') and relative.startswith(path))
                for path in area['paths']
            )]
            if len(matches) != 1:
                violations.append(f'{relative}:1: O3: file belongs to {len(matches)} areas; expected exactly one')
                continue
            origin = matches[0]
            module = list(file.relative_to(src).with_suffix('').parts)
            if module[-1] in ('mod', 'lib'):
                module.pop()
            words = tokens(file.read_text())
            references = [(path, line) for path, _, line in imports(words)]
            for i, (word, line) in enumerate(words):
                if word not in ('crate', 'self', 'super') or i + 1 >= len(words) or words[i + 1][0] != '::':
                    continue
                path = [word]
                j = i + 1
                while j + 1 < len(words) and words[j][0] == '::' and re.fullmatch(r'\w+', words[j + 1][0]):
                    path.append(words[j + 1][0])
                    j += 2
                references.append((path, line))
            seen = set()
            for path, line in references:
                target_path = absolute(path, module, exports)
                targets = [area for area in crate_areas if target_path[:len(area['module'])] == area['module']]
                if not target_path or not targets:
                    continue
                target = max(targets, key=lambda area: len(area['module']))
                rule = None
                if origin['owner'] != 'core' and target['owner'] != 'core' and origin['owner'] != target['owner']:
                    rule = 'O1'
                if origin['name'] in ('model.evaluate', 'model.persistence') and target['name'] == 'model.commands':
                    rule = 'O2'
                key = (line, target['name'], rule)
                if rule and key not in seen:
                    seen.add(key)
                    violations.append(f"{relative}:{line}: {rule}: {origin['name']} references {target['name']}")
    return violations


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=pathlib.Path, default=pathlib.Path(__file__).resolve().parents[1])
    args = parser.parse_args()
    errors = check(args.root)
    for error in errors:
        print(error)
    raise SystemExit(bool(errors))
