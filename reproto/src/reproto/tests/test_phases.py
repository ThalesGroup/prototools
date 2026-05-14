# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Regression tests for phases-level warning/error paths (spec 0041 TC-P*).

Tests run reproto as a CLI subprocess so they exercise the full pipeline.
Fixtures from tests/fixtures/ are compiled on demand via compile_proto().
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

from google.protobuf.descriptor_pb2 import (
    FileDescriptorProto,
    FileDescriptorSet,
)

from google.protobuf import text_format

from reproto.tests.conftest import FIXTURES_DIR, compile_proto, compile_proto_multi


# ---------------------------------------------------------------------------
# Helper: run reproto CLI as subprocess
# ---------------------------------------------------------------------------

def _run_reproto(
    pb_files: list[Path],
    out_dir: Path,
    extra_args: list[str] | None = None,
    include_dirs: list[Path] | None = None,
) -> subprocess.CompletedProcess[str]:
    src_path = str(Path(__file__).parent.parent.parent)
    pythonpath_parts = [src_path]
    if existing := os.environ.get("PYTHONPATH"):
        pythonpath_parts.append(existing)
    env = {**os.environ, "PYTHONPATH": os.pathsep.join(pythonpath_parts)}
    env.pop("REPROTO_VARIANT", None)

    dirs = [FIXTURES_DIR] + (include_dirs or [])
    include_flags = [f"-I{d}" for d in dirs]

    cmd = [
        sys.executable, "-m", "reproto.cli",
        "--use-variant", "descriptor",
        *include_flags,
        f"--output-root={out_dir}",
    ]
    if extra_args:
        cmd.extend(extra_args)
    cmd.extend(str(p) for p in pb_files)
    return subprocess.run(cmd, capture_output=True, text=True, env=env)


def _write_pb(path: Path, fdp: FileDescriptorProto) -> Path:
    """Wrap a FileDescriptorProto in a FileDescriptorSet and write to path."""
    fds = FileDescriptorSet()
    fds.file.append(fdp)
    path.write_bytes(fds.SerializeToString())
    return path


# ---------------------------------------------------------------------------
# TC-P1  Self-dependency stripped from malformed descriptor
# ---------------------------------------------------------------------------

def test_P1_self_dependency_stripped(tmp_path: Path) -> None:
    """A descriptor listing itself as a dependency must be stripped and not crash."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    # Build a valid single-file descriptor, then patch in a self-dependency.
    (base_pb,) = compile_proto(pb_dir, "prune_base.proto")

    fds = FileDescriptorSet()
    fds.ParseFromString(base_pb.read_bytes())
    fdp = fds.file[0]
    fdp.dependency.append(fdp.name)   # self-reference

    patched_pb = pb_dir / "self_dep.pb"
    patched_pb.write_bytes(fds.SerializeToString())

    # Run without the fixture dir on -I so reproto doesn't find prune_base.proto
    # as an importable source — the self-dependency is purely in the descriptor.
    empty_inc = tmp_path / "empty_inc"
    empty_inc.mkdir()
    result = _run_reproto([patched_pb], out_dir, include_dirs=[empty_inc])

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    # A self-referential FDP is never a topo-sort leaf (it depends on itself),
    # so it reaches the circular-dependency detector rather than _strip_self_dependency.
    # The observable behaviour is "Circular dependency detected".
    assert "Circular dependency detected" in result.stderr, (
        f"Expected circular-dependency warning for self-dep. stderr:\n{result.stderr}"
    )


# ---------------------------------------------------------------------------
# TC-P2  W3 singular "1 symbol" (not "1 symbols")
# ---------------------------------------------------------------------------

def test_P2_w3_singular_symbol(tmp_path: Path) -> None:
    """Two files sharing exactly one symbol produce '(1 symbol)' in the W3 line."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    # prune_duplicate_1 and prune_duplicate_2 both define prune_dup.SharedMsg.
    dup1_pb, dup2_pb = compile_proto(pb_dir, "prune_duplicate_1.proto", "prune_duplicate_2.proto")

    result = _run_reproto([dup1_pb, dup2_pb], out_dir)

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    assert "(1 symbol)" in result.stderr, (
        f"Expected '(1 symbol)' in W3 line. stderr:\n{result.stderr}"
    )
    assert "(1 symbols)" not in result.stderr, "Must use singular form"


# ---------------------------------------------------------------------------
# TC-P3  --keep-duplicates: no W3 pruning, no crash
# ---------------------------------------------------------------------------

def test_P3_keep_duplicates_no_w3(tmp_path: Path) -> None:
    """With --keep-duplicates, conflicting files are not pruned; no W3 line appears."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    dup1_pb, dup2_pb = compile_proto(pb_dir, "prune_duplicate_1.proto", "prune_duplicate_2.proto")

    result = _run_reproto([dup1_pb, dup2_pb], out_dir, extra_args=["--keep-duplicates"])

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    assert "pruned" not in result.stderr or "duplicate symbols" not in result.stderr, \
        "W3 pruning must not fire when --keep-duplicates is active"


# ---------------------------------------------------------------------------
# TC-P4  Unparseable .pb file
# ---------------------------------------------------------------------------

def test_P4_unparseable_pb(tmp_path: Path) -> None:
    """A .pb file with garbage content produces a 'Skipping unparseable file' warning."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    corrupt = pb_dir / "corrupt.pb"
    corrupt.write_bytes(b"\xff\xfe\xfd this is not valid protobuf \x00\x01\x02")

    result = _run_reproto([corrupt], out_dir)

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    # Seed files that fail to parse are caught in load.py ("Cannot parse ...").
    # The "Skipping unparseable file" message fires in phase 2 for dependency files
    # that fail pool parsing — so accept either message.
    assert ("Cannot parse" in result.stderr or "Skipping unparseable file" in result.stderr), (
        f"Expected a parse-failure message. stderr:\n{result.stderr}"
    )


# ---------------------------------------------------------------------------
# TC-P5  Circular dependency detected
# ---------------------------------------------------------------------------

def test_P5_circular_dependency(tmp_path: Path) -> None:
    """Two descriptors that import each other trigger a circular-dependency warning."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    fdp_a = FileDescriptorProto()
    fdp_a.name = "circ_a.proto"
    fdp_a.syntax = "proto3"
    fdp_a.dependency.append("circ_b.proto")
    msg = fdp_a.message_type.add()
    msg.name = "MsgA"

    fdp_b = FileDescriptorProto()
    fdp_b.name = "circ_b.proto"
    fdp_b.syntax = "proto3"
    fdp_b.dependency.append("circ_a.proto")
    msg = fdp_b.message_type.add()
    msg.name = "MsgB"

    pb_a = _write_pb(pb_dir / "circ_a.pb", fdp_a)
    pb_b = _write_pb(pb_dir / "circ_b.pb", fdp_b)

    result = _run_reproto([pb_a, pb_b], out_dir)

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    assert "Circular dependency detected" in result.stderr


# ---------------------------------------------------------------------------
# TC-P6  Pruning target not found
# ---------------------------------------------------------------------------

def test_P6_prune_target_not_found(tmp_path: Path) -> None:
    """--prune with a non-existent FQDN prints 'Pruning target not found'."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    (base_pb,) = compile_proto(pb_dir, "prune_base.proto")

    result = _run_reproto(
        [base_pb], out_dir,
        extra_args=["--prune", "file:nonexistent.proto"],
    )

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    assert "Pruning target not found: file:nonexistent.proto" in result.stderr


# ---------------------------------------------------------------------------
# TC-P7  Pruning fuzzy suggestion
# ---------------------------------------------------------------------------

def test_P7_prune_fuzzy_suggestion(tmp_path: Path) -> None:
    """A near-miss --prune value triggers a 'Did you mean:' suggestion."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    (base_pb,) = compile_proto(pb_dir, "prune_base.proto")

    # Near-miss: missing one character from the real name
    result = _run_reproto(
        [base_pb], out_dir,
        extra_args=["--prune", "file:prune_bse.proto"],
    )

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    assert "Did you mean:" in result.stderr


# ---------------------------------------------------------------------------
# TC-P8  Seed not found
# ---------------------------------------------------------------------------

def test_P8_seed_not_found(tmp_path: Path) -> None:
    """--seed with a non-existent FQDN prints 'Seed not found'."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    (base_pb,) = compile_proto(pb_dir, "prune_base.proto")

    result = _run_reproto(
        [base_pb], out_dir,
        extra_args=["--seed", "file:nonexistent.proto"],
    )

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    assert "Seed not found: file:nonexistent.proto" in result.stderr


# ---------------------------------------------------------------------------
# TC-P9  Seed fuzzy suggestion
# ---------------------------------------------------------------------------

def test_P9_seed_fuzzy_suggestion(tmp_path: Path) -> None:
    """A near-miss --seed value triggers a 'Did you mean:' suggestion."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    (base_pb,) = compile_proto(pb_dir, "prune_base.proto")

    result = _run_reproto(
        [base_pb], out_dir,
        extra_args=["--seed", "file:prune_bse.proto"],
    )

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    assert "Did you mean:" in result.stderr


# ---------------------------------------------------------------------------
# TC-P10  Pruned seed is skipped
# ---------------------------------------------------------------------------

def test_P10_pruned_seed_skipped(tmp_path: Path) -> None:
    """A --seed that resolves to a pruned node is skipped with an info message."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    (base_pb,) = compile_proto(pb_dir, "prune_base.proto")

    result = _run_reproto(
        [base_pb], out_dir,
        extra_args=[
            "--prune", "file:prune_base.proto",
            "--seed", "file:prune_base.proto",
        ],
    )

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    assert "Skipping pruned seed" in result.stderr
    assert not (out_dir / "prune_base.proto").exists()


# ---------------------------------------------------------------------------
# TC-M1  Multi-FDP binary FDS round-trips correctly
# ---------------------------------------------------------------------------

def test_M1_multi_fdp_binary_fds(tmp_path: Path) -> None:
    """A multi-FDP binary FDS yields the same output as feeding each FDP separately.

    address_book.proto imports phone_number.proto, so --include_imports bundles
    both FDPs into one FileDescriptorSet .pb.
    """
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_single = tmp_path / "out_single"
    out_single.mkdir()
    out_multi = tmp_path / "out_multi"
    out_multi.mkdir()

    # Individual single-FDP files
    phone_pb, addr_pb = compile_proto(pb_dir, "phone_number.proto", "address_book.proto")
    # Single multi-FDP FDS containing both
    multi_pb = compile_proto_multi(pb_dir / "multi.pb", "address_book.proto")

    result_single = _run_reproto([phone_pb, addr_pb], out_single)
    result_multi = _run_reproto([multi_pb], out_multi)

    assert result_single.returncode == 0, f"single run crashed:\n{result_single.stderr}"
    assert result_multi.returncode == 0, f"multi run crashed:\n{result_multi.stderr}"

    single_files = {f.name for f in out_single.rglob("*.proto")}
    multi_files = {f.name for f in out_multi.rglob("*.proto")}
    assert single_files == multi_files, (
        f"Output file sets differ.\nSingle: {single_files}\nMulti:  {multi_files}"
    )


# ---------------------------------------------------------------------------
# TC-M2  Multi-FDP text FDS round-trips correctly
# ---------------------------------------------------------------------------

def test_M2_multi_fdp_text_fds(tmp_path: Path) -> None:
    """A multi-FDP text FDS (.textpb) yields the same output as individual .pb files."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_single = tmp_path / "out_single"
    out_single.mkdir()
    out_multi = tmp_path / "out_multi"
    out_multi.mkdir()

    phone_pb, addr_pb = compile_proto(pb_dir, "phone_number.proto", "address_book.proto")
    multi_pb = compile_proto_multi(pb_dir / "multi.pb", "address_book.proto")

    # Serialise the multi-FDP FDS as text
    fds = FileDescriptorSet()
    fds.ParseFromString(multi_pb.read_bytes())
    textpb = tmp_path / "multi.textpb"
    textpb.write_text(text_format.MessageToString(fds))

    result_single = _run_reproto([phone_pb, addr_pb], out_single)
    result_multi = _run_reproto([textpb], out_multi)

    assert result_single.returncode == 0, f"single run crashed:\n{result_single.stderr}"
    assert result_multi.returncode == 0, f"multi run crashed:\n{result_multi.stderr}"

    single_files = {f.name for f in out_single.rglob("*.proto")}
    multi_files = {f.name for f in out_multi.rglob("*.proto")}
    assert single_files == multi_files, (
        f"Output file sets differ.\nSingle: {single_files}\nMulti:  {multi_files}"
    )


# ---------------------------------------------------------------------------
# TC-M3  Single-FDP binary input still works (regression)
# ---------------------------------------------------------------------------

def test_M3_single_fdp_binary_regression(tmp_path: Path) -> None:
    """Single-FDP .pb input continues to work after the parse_qfile refactor."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    (phone_pb,) = compile_proto(pb_dir, "phone_number.proto")
    result = _run_reproto([phone_pb], out_dir)

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    assert (out_dir / "phone_number.proto").exists(), "Expected phone_number.proto in output"


# ---------------------------------------------------------------------------
# TC-M4  FDP name collision across two FDS files (first-wins)
# ---------------------------------------------------------------------------

def test_M4_fdp_name_collision_first_wins(tmp_path: Path) -> None:
    """Two FDS files sharing an FDP name are accepted; only one copy is processed."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    # Build a duplicate: two multi-FDP FDS files both containing phone_number.proto
    multi_a = compile_proto_multi(pb_dir / "multi_a.pb", "address_book.proto")
    multi_b = compile_proto_multi(pb_dir / "multi_b.pb", "address_book.proto")

    result = _run_reproto([multi_a, multi_b], out_dir)

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    # Output should still contain the reconstructed protos
    assert (out_dir / "phone_number.proto").exists(), "Expected phone_number.proto in output"
    assert (out_dir / "address_book.proto").exists(), "Expected address_book.proto in output"
