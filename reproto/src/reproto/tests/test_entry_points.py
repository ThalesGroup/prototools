# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Every program reproto installs answers ``--version`` (spec 0413 S5).

The programs are read from ``[project.scripts]``, so one added later is
checked too. Each runs in a subprocess, as an installed script would, and
must print its own name and the version ``reproto.version()`` reports:
the installed distribution's, or ``dev`` from source.
"""

from __future__ import annotations

import subprocess
import sys
import tomllib
from pathlib import Path

import pytest

import reproto

# src/reproto/tests/ -> the directory that holds pyproject.toml.
_PYPROJECT = Path(__file__).resolve().parents[3] / 'pyproject.toml'


def _scripts() -> dict[str, str]:
    with _PYPROJECT.open('rb') as f:
        return tomllib.load(f)['project']['scripts']


@pytest.mark.parametrize('name', sorted(_scripts()))
def test_every_program_answers_version(name: str) -> None:
    module, function = _scripts()[name].split(':')
    code = (
        f'import sys; sys.argv[0] = {name!r}; '
        f'from {module} import {function}; {function}()'
    )
    result = subprocess.run(
        [sys.executable, '-c', code, '--version'],
        capture_output=True, text=True, check=False,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == f'{name}, version {reproto.version()}'
