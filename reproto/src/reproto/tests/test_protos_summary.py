# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""The ``protos:`` summary line: where phase 7 wrote the .proto files, and
how many. After the schema database's summary, or at the end of phase 7
without one; none in a dry run or under ``--quiet``. protoback prints the
same line (its tests/summary.rs).
"""

import os
import subprocess
from pathlib import Path

import pytest

from .conftest import REPROTO_CMD

A = 'name: "a.proto" package: "p" message_type { name: "A" }\n'
B = '''name: "b.proto" package: "p" dependency: "a.proto"
message_type { name: "B"
  field { name: "a" number: 1 label: LABEL_OPTIONAL type: TYPE_MESSAGE type_name: ".p.A" } }
'''


def _run(tmp_path: Path, *flags: str) -> subprocess.CompletedProcess[str]:
    inp = tmp_path / "in"
    inp.mkdir(exist_ok=True)
    (inp / "a.textpb").write_text(A, encoding="utf-8")
    (inp / "b.textpb").write_text(B, encoding="utf-8")
    env = {k: v for k, v in os.environ.items() if k != "REPROTO_VARIANT"}
    return subprocess.run([*REPROTO_CMD, "-I", "in", *flags],
                          capture_output=True, text=True, env=env, cwd=tmp_path)


def test_proto_out_alone_reports_after_rendering(tmp_path: Path) -> None:
    r = _run(tmp_path, "-O", "./out/")
    assert r.returncode == 0, r.stderr
    lines = r.stderr.splitlines()
    at = lines.index("  protos:     out (2 files)")
    assert lines[at - 1] == "Rendering proto files", r.stderr


def test_schema_db_reports_after_its_summary(tmp_path: Path) -> None:
    r = _run(tmp_path, "--schema-db-out", "db.desc")
    assert r.returncode == 0, r.stderr
    lines = r.stderr.splitlines()
    at = lines.index("  protos:     db/proto (2 files)")
    assert lines[at - 1].startswith("  index:      "), r.stderr


@pytest.mark.parametrize("flags", [
    ("-O", "out", "--dry-run"),
    ("-O", "out", "--quiet"),
    ("--schema-db-out", "db.desc", "--no-proto-out"),
])
def test_nothing_written_nothing_reported(tmp_path: Path, flags: tuple[str, ...]) -> None:
    r = _run(tmp_path, *flags)
    assert r.returncode == 0, r.stderr
    assert "protos:" not in r.stderr, r.stderr
