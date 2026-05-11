# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Regression test for proto-gen (spec 0055)."""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
WKT_DIR = REPO_ROOT / "reproto" / "src" / "reproto" / "variants" / "google-protobuf"
PROTO_GEN = REPO_ROOT / "bin" / "proto-gen"


@pytest.fixture(scope="module")
def generated_file(tmp_path_factory: pytest.TempPathFactory) -> Path:
    out_dir = tmp_path_factory.mktemp("proto_gen_out")
    result = subprocess.run(
        [
            str(PROTO_GEN),
            "-I", str(WKT_DIR),
            "-t", ".google.protobuf.FileOptions",
            "-s", "0",
            "-O", str(out_dir),
        ],
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stderr
    candidates = list(out_dir.glob("*.pb"))
    assert candidates, "no output file produced"
    return candidates[0]


def test_exit_code_zero(generated_file: Path) -> None:
    # fixture already asserts returncode == 0; just confirm file exists
    assert generated_file.exists()


def test_magic_line(generated_file: Path) -> None:
    lines = generated_file.read_text(encoding="utf-8").splitlines()
    assert lines[0].startswith("#@ prototext:")


def test_ground_truth_comment(generated_file: Path) -> None:
    lines = generated_file.read_text(encoding="utf-8").splitlines()
    assert lines[1] == "# ground_truth: .google.protobuf.FileOptions"


def test_round_trip_encode(generated_file: Path) -> None:
    result = subprocess.run(
        ["prototext", "-e", str(generated_file)],
        capture_output=True,
    )
    assert result.returncode == 0, result.stderr.decode()
