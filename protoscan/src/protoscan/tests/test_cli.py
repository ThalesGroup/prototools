# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Integration tests for the protoscan CLI.

Each test invokes `protoscan` as a subprocess and checks stdout, stderr,
exit code, and (where applicable) written output files.

Fixtures are synthesised in-process using protobuf's Python API so that
the test suite has no dependency on protoc.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

from google.protobuf.descriptor_pb2 import FileDescriptorProto


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _make_fdp_bytes(name: str) -> bytes:
    """Return a minimal serialised FileDescriptorProto with the given name.

    Sets ``package`` as well as ``name`` so that the record passes the
    structural floor in ``declares_more_than_a_name`` (spec 0313 S4):
    a descriptor with only its name is indistinguishable from a Java
    package option and is deliberately rejected by the scanner.
    """
    fdp = FileDescriptorProto()
    fdp.name = name
    fdp.package = "test"
    return fdp.SerializeToString()


def _run(args: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-m", "protoscan.cli", *args],
        capture_output=True,
        text=True,
    )


# ---------------------------------------------------------------------------
# TC-1  Empty binary — no FDPs found, no output
# ---------------------------------------------------------------------------

def test_empty_binary_no_output(tmp_path: Path) -> None:
    """Scanning a file with no FDP blobs produces no stdout and exits 0."""
    f = tmp_path / "empty.bin"
    f.write_bytes(b"\x00" * 64)

    result = _run([str(f)])

    assert result.returncode == 0, f"unexpected exit code: {result.stderr}"
    assert result.stdout == "", f"unexpected output: {result.stdout!r}"


# ---------------------------------------------------------------------------
# TC-2  Single FDP blob — name is printed
# ---------------------------------------------------------------------------

def test_single_fdp_printed(tmp_path: Path) -> None:
    """A binary containing one FDP blob prints its proto name."""
    payload = _make_fdp_bytes("google/protobuf/descriptor.proto")
    f = tmp_path / "single.bin"
    f.write_bytes(payload)

    result = _run([str(f)])

    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == "google/protobuf/descriptor.proto"


# ---------------------------------------------------------------------------
# TC-3  Multiple FDP blobs — all names printed, one per line
# ---------------------------------------------------------------------------

def test_multiple_fdps_all_printed(tmp_path: Path) -> None:
    """A binary containing two concatenated FDP blobs prints both names."""
    blob = _make_fdp_bytes("foo/bar.proto") + _make_fdp_bytes("baz/qux.proto")
    f = tmp_path / "multi.bin"
    f.write_bytes(blob)

    result = _run([str(f)])

    assert result.returncode == 0, result.stderr
    names = result.stdout.strip().splitlines()
    assert set(names) == {"foo/bar.proto", "baz/qux.proto"}, f"got: {names}"


# ---------------------------------------------------------------------------
# TC-4  --proto_out writes extracted .pb file
# ---------------------------------------------------------------------------

def test_proto_out_writes_pb(tmp_path: Path) -> None:
    """With --proto_out the extracted blob is written to <out>/<name>.pb."""
    payload = _make_fdp_bytes("mypkg/schema.proto")
    f = tmp_path / "input.bin"
    f.write_bytes(payload)
    out_dir = tmp_path / "out"

    result = _run([str(f), f"--proto_out={out_dir}"])

    assert result.returncode == 0, result.stderr
    expected = out_dir / "mypkg" / "schema.pb"
    assert expected.exists(), f"expected output file not found: {expected}"
    # The written bytes must be the original FDP blob.
    assert expected.read_bytes() == payload


# ---------------------------------------------------------------------------
# TC-5  --proto_out with nested path creates intermediate directories
# ---------------------------------------------------------------------------

def test_proto_out_creates_parent_dirs(tmp_path: Path) -> None:
    """Intermediate directories under --proto_out are created automatically."""
    payload = _make_fdp_bytes("a/b/c/deep.proto")
    f = tmp_path / "input.bin"
    f.write_bytes(payload)
    out_dir = tmp_path / "out"

    result = _run([str(f), f"--proto_out={out_dir}"])

    assert result.returncode == 0, result.stderr
    assert (out_dir / "a" / "b" / "c" / "deep.pb").exists()


# ---------------------------------------------------------------------------
# TC-6  FDP preceded by non-protobuf bytes and followed by a 0x00 terminator
# ---------------------------------------------------------------------------

def test_fdp_preceded_by_noise(tmp_path: Path) -> None:
    """An FDP blob with leading noise bytes is detected when followed by 0x00."""
    payload = _make_fdp_bytes("embedded/thing.proto")
    # The scanner needs a 0x00 terminator (or end of a valid protobuf stream)
    # after each FDP.  Trailing arbitrary bytes break field-tag parsing, so
    # the standard embedding format is: <noise> <fdp> <0x00>.
    noise = b"\xDE\xAD\xBE\xEF" * 16
    f = tmp_path / "noisy.bin"
    f.write_bytes(noise + payload + b"\x00")

    result = _run([str(f)])

    assert result.returncode == 0, result.stderr
    assert "embedded/thing.proto" in result.stdout


# ---------------------------------------------------------------------------
# TC-7  Non-existent file → non-zero exit and error message
# ---------------------------------------------------------------------------

def test_nonexistent_file_error(tmp_path: Path) -> None:
    """Passing a non-existent path exits non-zero with an error message."""
    result = _run([str(tmp_path / "does_not_exist.bin")])

    assert result.returncode != 0
    assert "does_not_exist.bin" in result.stderr or "Error" in result.stderr \
        or "Invalid value" in result.stderr


# ---------------------------------------------------------------------------
# TC-8  --proto_out with multiple FDPs writes all extracted files
# ---------------------------------------------------------------------------

def test_proto_out_multiple_fdps(tmp_path: Path) -> None:
    """With --proto_out and multiple FDPs, each blob is written separately."""
    blob = _make_fdp_bytes("pkg/alpha.proto") + _make_fdp_bytes("pkg/beta.proto")
    f = tmp_path / "multi.bin"
    f.write_bytes(blob)
    out_dir = tmp_path / "out"

    result = _run([str(f), f"--proto_out={out_dir}"])

    assert result.returncode == 0, result.stderr
    assert (out_dir / "pkg" / "alpha.pb").exists()
    assert (out_dir / "pkg" / "beta.pb").exists()
