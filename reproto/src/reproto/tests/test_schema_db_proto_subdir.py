# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Tests for -O/--schema-db-out stub-directory overlap validation
(spec 0155 G1).

The validation runs before any DESCRIPTOR_FILES are loaded, so these
tests pass a nonexistent dummy file as the positional argument and
only assert on the specific UsageError text (or its absence).
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path
from reproto.tests.conftest import REPROTO_CMD

_ERROR_MARKER = "only an immediate 'proto' child directory is allowed"


def _run(db_path: Path, proto_out: Path, dummy_pb: Path) -> subprocess.CompletedProcess[str]:
    src_path = str(Path(__file__).parent.parent.parent)
    pythonpath_parts = [src_path]
    if existing := os.environ.get("PYTHONPATH"):
        pythonpath_parts.append(existing)
    env = {**os.environ, "PYTHONPATH": os.pathsep.join(pythonpath_parts)}
    env.pop("REPROTO_VARIANT", None)

    cmd = [
        *REPROTO_CMD,
        f"--schema-db-out={db_path}",
        f"-O{proto_out}",
        str(dummy_pb),
    ]
    return subprocess.run(cmd, capture_output=True, text=True, env=env)


def test_G1_proto_child_of_stub_is_allowed(tmp_path: Path) -> None:
    db_path = tmp_path / "schema.desc"
    result = _run(db_path, tmp_path / "schema" / "proto", tmp_path / "missing.pb")
    assert _ERROR_MARKER not in result.stderr


def test_G1_stub_itself_is_forbidden(tmp_path: Path) -> None:
    db_path = tmp_path / "schema.desc"
    result = _run(db_path, tmp_path / "schema", tmp_path / "missing.pb")
    assert result.returncode != 0
    assert _ERROR_MARKER in result.stderr


def test_G1_differently_named_child_is_forbidden(tmp_path: Path) -> None:
    db_path = tmp_path / "schema.desc"
    result = _run(db_path, tmp_path / "schema" / "desc", tmp_path / "missing.pb")
    assert result.returncode != 0
    assert _ERROR_MARKER in result.stderr


def test_G1_nested_proto_is_forbidden(tmp_path: Path) -> None:
    db_path = tmp_path / "schema.desc"
    result = _run(db_path, tmp_path / "schema" / "nested" / "proto", tmp_path / "missing.pb")
    assert result.returncode != 0
    assert _ERROR_MARKER in result.stderr


def test_G1_outside_stub_is_allowed(tmp_path: Path) -> None:
    db_path = tmp_path / "schema.desc"
    result = _run(db_path, tmp_path / "elsewhere", tmp_path / "missing.pb")
    assert _ERROR_MARKER not in result.stderr


def test_N1_no_schema_db_out_is_unaffected(tmp_path: Path) -> None:
    src_path = str(Path(__file__).parent.parent.parent)
    pythonpath_parts = [src_path]
    if existing := os.environ.get("PYTHONPATH"):
        pythonpath_parts.append(existing)
    env = {**os.environ, "PYTHONPATH": os.pathsep.join(pythonpath_parts)}
    env.pop("REPROTO_VARIANT", None)

    cmd = [
        *REPROTO_CMD,
        f"-O{tmp_path / 'out'}",
        str(tmp_path / "missing.pb"),
    ]
    result = subprocess.run(cmd, capture_output=True, text=True, env=env)
    assert _ERROR_MARKER not in result.stderr
