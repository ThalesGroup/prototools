# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""W23: an import of a pruned file stripped from its importer (spec 0438 S1).

Apart from W3 (a duplicate pruned), so that `--fail-on W3` asserts that no
duplicate was resolved by load order, whatever a prune pattern strips.
"""

import os
import subprocess
from pathlib import Path

from .conftest import REPROTO_CMD

BASE = 'syntax = "proto2";\npackage p;\nmessage Base { optional int32 x = 1; }\n'
USER = ('syntax = "proto2";\npackage p;\nimport "base.proto";\n'
        'message User { optional int32 y = 1; }\n')


def _run(tmp_path: Path, *flags: str) -> subprocess.CompletedProcess[str]:
    src = tmp_path / "src"
    src.mkdir(exist_ok=True)
    (src / "base.proto").write_text(BASE, encoding="utf-8")
    (src / "user.proto").write_text(USER, encoding="utf-8")
    inp = tmp_path / "in"
    inp.mkdir(exist_ok=True)
    subprocess.run(["protoc", f"-I{src}", "--include_imports",
                    f"--descriptor_set_out={inp / 'set.pb'}", "user.proto"],
                   check=True, capture_output=True)
    env = {k: v for k, v in os.environ.items() if k != "REPROTO_VARIANT"}
    return subprocess.run(
        [*REPROTO_CMD, "--use-variant", "descriptor", "-p", "file:base.proto",
         *flags, f"-I{inp}", "-O", str(tmp_path / "out")],
        capture_output=True, text=True, env=env,
    )


def test_a_stripped_import_of_a_pruned_file_is_w23_not_w3(tmp_path: Path) -> None:
    result = _run(tmp_path)
    assert result.returncode == 0, result.stderr
    assert ('Warning [W23]: stripped pruned dependency "base.proto" from user.proto'
            in result.stderr), result.stderr
    assert "[W3]" not in result.stderr, result.stderr


def test_fail_on_w3_ignores_a_stripped_import(tmp_path: Path) -> None:
    assert _run(tmp_path, "--fail-on", "W3").returncode == 0


def test_fail_on_w23_counts_it(tmp_path: Path) -> None:
    result = _run(tmp_path, "--fail-on", "W23")
    assert result.returncode == 3, result.stderr
    assert "--fail-on: W23 (1 occurrence)" in result.stderr, result.stderr
