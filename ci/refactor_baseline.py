"""Run refactor comparisons at the landed base and candidate in one checkout."""

import contextlib
import pathlib
import subprocess


ROOT = pathlib.Path(__file__).resolve().parents[1]


def git(*args):
    return subprocess.check_output(['git', '-C', str(ROOT), *args], text=True).strip()


@contextlib.contextmanager
def at_base(base):
    if git('status', '--porcelain'):
        raise RuntimeError('Commit or stash changes before comparing the base')
    head = git('rev-parse', 'HEAD')
    branch = subprocess.run(['git', '-C', str(ROOT), 'symbolic-ref', '--short', '-q', 'HEAD'],
                            capture_output=True, text=True).stdout.strip()
    subprocess.run(['git', '-C', str(ROOT), 'switch', '--detach', base], check=True)
    try:
        yield
    finally:
        target = [branch] if branch else ['--detach', head]
        subprocess.run(['git', '-C', str(ROOT), 'switch', *target], check=True)


def cargo(*args):
    return subprocess.check_output(['cargo', *args], cwd=ROOT, text=True)
