# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Warning codes, counted, and made fatal on request (spec 0438)."""

import os
import subprocess
from pathlib import Path

import pytest
from google.protobuf import descriptor_pb2

from .conftest import REPROTO_CMD

A = """syntax = "proto2";
package a;
import "b.proto";
message A { optional b.B b = 1; }
"""
B = """syntax = "proto2";
package b;
message B { optional int32 x = 1; }
"""


def _env() -> dict[str, str]:
    return {k: v for k, v in os.environ.items() if k != "REPROTO_VARIANT"}


@pytest.fixture
def inputs(tmp_path: Path) -> Path:
    """a.proto importing b.proto, which the input lacks (W5); a.proto's
    options as text, one by a name nothing declares (T4) and one by a
    number nothing declares (W22 once rendered)."""
    src = tmp_path / "src"
    src.mkdir()
    (src / "a.proto").write_text(A, encoding="utf-8")
    (src / "b.proto").write_text(B, encoding="utf-8")
    compiled = tmp_path / "a.pb"
    subprocess.run(["protoc", f"-I{src}", f"--descriptor_set_out={compiled}", "a.proto"],
                   check=True, capture_output=True)
    fds = descriptor_pb2.FileDescriptorSet()
    fds.ParseFromString(compiled.read_bytes())
    fds.file[0].options.MergeFromString(b"\x98\x06\x05")   # field 99 = 5
    text = subprocess.run(
        ["protoc", "--decode=google.protobuf.FileDescriptorSet", "google/protobuf/descriptor.proto"],
        input=fds.SerializeToString(), capture_output=True, check=True,
    ).stdout.decode()
    text = text.replace("  options {\n", "  options {\n    no_such_option: 1\n", 1)
    d = tmp_path / "in"
    d.mkdir()
    (d / "a.textpb").write_text(text, encoding="utf-8")
    return d


def _run(inputs: Path, *args: str) -> subprocess.CompletedProcess[str]:
    out = inputs.parent / "out"
    return subprocess.run(
        [*REPROTO_CMD, f"-I{inputs}", "-O", str(out), "--use-variant", "descriptor",
         *args, "a.textpb"],
        capture_output=True, text=True, env=_env(),
    )


def test_every_warning_line_shows_its_code_and_text_ones_are_counted(inputs: Path) -> None:
    result = _run(inputs)
    assert result.returncode == 0, result.stderr
    lines = result.stderr.splitlines()
    # Missing while loading (W1) and while rendering (W5): one summary line.
    assert "Warning [W5]: missing dependency file:b.proto (2 occurrences)" in lines
    assert ("Warning [T4]: field no_such_option of google.protobuf.FileOptions is "
            "declared nowhere in the input: skipped (1 occurrence)") in lines
    assert ("Warning [W22]: 'a.proto': option field 99 of google.protobuf.FileOptions "
            "is declared nowhere: written as an orphan (1 occurrence)") in lines
    assert not [line for line in lines if line.startswith("Warning:")]


def test_an_undeclared_option_field_is_an_orphan_redacted_or_not(inputs: Path) -> None:
    out = inputs.parent / "out" / "a.proto"
    assert _run(inputs).returncode == 0
    assert "///option <99> = 5;" in out.read_text(encoding="utf-8")
    result = _run(inputs, "--redact-orphans")
    assert result.returncode == 0
    assert "<99>" not in out.read_text(encoding="utf-8")
    assert "Warning [W22]:" in result.stderr


@pytest.mark.parametrize("quiet", [[], ["-q"]])
def test_fail_on_completes_the_run_then_exits_3(inputs: Path, quiet: list[str]) -> None:
    result = _run(inputs, "--fail-on", "T4", "--fail-on", "W22", "--fail-on", "W3", *quiet)
    assert result.returncode == 3, result.stderr
    assert result.stderr.splitlines()[-1] == (
        "Error: --fail-on: W22 (1 occurrence), T4 (1 occurrence)"
    )
    assert (inputs.parent / "out" / "a.proto").exists()
    if quiet:
        assert "Warning [" not in result.stderr


def test_fail_on_a_code_that_did_not_occur_passes(inputs: Path) -> None:
    assert _run(inputs, "--fail-on", "W3").returncode == 0


def test_fail_on_an_unknown_code_is_a_usage_error(inputs: Path) -> None:
    result = _run(inputs, "--fail-on", "W99")
    assert result.returncode == 2
    assert "unknown warning code 'W99'" in result.stderr


@pytest.mark.skipif(
    "reproto.tests.via_text" in REPROTO_CMD,
    reason="the text format cannot keep a group: protoc prints it as a message, read "
           "back as length-delimited, with T1",
)
def test_an_undeclared_group_is_written_with_its_content(tmp_path: Path) -> None:
    # File options holding, undeclared, a group (field 7: <1> = 5, <2> =
    # "x") and a varint (field 99 = 5): each an orphan, with its value.
    src = tmp_path / "src"
    src.mkdir()
    (src / "b.proto").write_text(B, encoding="utf-8")
    compiled = tmp_path / "b.pb"
    subprocess.run(["protoc", f"-I{src}", f"--descriptor_set_out={compiled}", "b.proto"],
                   check=True, capture_output=True)
    fds = descriptor_pb2.FileDescriptorSet()
    fds.ParseFromString(compiled.read_bytes())
    fds.file[0].options.MergeFromString(
        bytes([0x3b, 0x08, 0x05, 0x12, 0x01, 0x78, 0x3c, 0x98, 0x06, 0x05]))
    d = tmp_path / "in"
    d.mkdir()
    (d / "b.pb").write_bytes(fds.SerializeToString())
    result = subprocess.run(
        [*REPROTO_CMD, f"-I{d}", "-O", str(tmp_path / "out"), "--use-variant", "descriptor",
         "b.pb"],
        capture_output=True, text=True, env=_env(),
    )
    assert result.returncode == 0, result.stderr
    text = (tmp_path / "out" / "b.proto").read_text(encoding="utf-8")
    assert '///option <7> = { <1>: 5 <2>: "x" };' in text
    assert "///option <99> = 5;" in text
    assert result.stderr.count("Warning [W22]:") == 2
