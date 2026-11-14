# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Regression tests for pruned-file dependency handling (spec 0041).

Before the fix, a file that imported a pruned file would crash in phase 7
with "Node ... not initialized with message".  The crash occurred for both
--prune-triggered pruning and duplicate-symbol pruning.

Fixtures (under tests/fixtures/):
  prune_base.proto          — defines prune_test.Base
  prune_importer.proto      — imports prune_base.proto, defines prune_test.Importer
  prune_duplicate_1.proto   — defines prune_dup.SharedMsg  (first copy)
  prune_duplicate_2.proto   — defines prune_dup.SharedMsg  (second copy, same package)
  prune_dup_importer.proto  — imports prune_duplicate_2.proto, defines prune_dup_consumer.Consumer
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

from reproto.tests.conftest import FIXTURES_DIR, REPROTO_CMD, compile_proto


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

    dirs = include_dirs if include_dirs is not None else [FIXTURES_DIR]
    include_flags = [f"-I{d}" for d in dirs]

    cmd = [
        *REPROTO_CMD,
        "--use-variant", "descriptor",
        *include_flags,
        f"--proto-out={out_dir}",
    ]
    if extra_args:
        cmd.extend(extra_args)
    cmd.extend(str(p) for p in pb_files)

    return subprocess.run(cmd, capture_output=True, text=True, env=env)


# ---------------------------------------------------------------------------
# T1 — --prune: importer of pruned file renders without crash
# ---------------------------------------------------------------------------

def test_explicit_prune_no_crash(tmp_path: Path) -> None:
    """Rendering a file that imports an explicitly pruned file must not crash."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    base_pb, importer_pb = compile_proto(pb_dir, "prune_base.proto", "prune_importer.proto")

    result = _run_reproto(
        [base_pb, importer_pb], out_dir,
        extra_args=["--prune", "file:prune_base.proto"],
    )

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    assert (out_dir / "prune_importer.proto").exists()
    assert not (out_dir / "prune_base.proto").exists()


# ---------------------------------------------------------------------------
# T2 — duplicate symbols: importer of pruned file renders without crash
# ---------------------------------------------------------------------------

def test_duplicate_prune_no_crash(tmp_path: Path) -> None:
    """Rendering a file that imports a duplicate-pruned file must not crash."""
    # prune_duplicate_1 and prune_duplicate_2 define the same symbol (prune_dup.SharedMsg).
    # One will be pruned by the duplicate-detection logic; which one wins is
    # non-deterministic (set iteration order), so we only assert that exactly
    # one survives.
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    dup1_pb, dup2_pb, importer_pb = compile_proto(
        pb_dir,
        "prune_duplicate_1.proto",
        "prune_duplicate_2.proto",
        "prune_dup_importer.proto",
    )

    result = _run_reproto([dup1_pb, dup2_pb, importer_pb], out_dir)

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    assert (out_dir / "prune_dup_importer.proto").exists()
    winner_count = sum(
        1 for f in ["prune_duplicate_1.proto", "prune_duplicate_2.proto"]
        if (out_dir / f).exists()
    )
    assert winner_count == 1, (
        f"Expected exactly one of prune_duplicate_1/2 in output, got {winner_count}"
    )
    # W3 warning must appear for whichever was pruned
    assert "pruned" in result.stderr and "duplicate symbols" in result.stderr
    # TC-R2: no W5 line for either duplicate (whichever was pruned should be suppressed)
    assert "missing dependency file:prune_duplicate_1.proto" not in result.stderr, (
        "W5 for a pruned duplicate must be suppressed"
    )
    assert "missing dependency file:prune_duplicate_2.proto" not in result.stderr, (
        "W5 for a pruned duplicate must be suppressed"
    )


# ---------------------------------------------------------------------------
# TC-R1  Zero W5 for importers of an explicitly pruned file
# ---------------------------------------------------------------------------

def test_explicit_prune_no_w5(tmp_path: Path) -> None:
    """No 'missing dependency' warning must appear for an explicitly pruned file."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    base_pb, importer_pb = compile_proto(pb_dir, "prune_base.proto", "prune_importer.proto")

    result = _run_reproto(
        [base_pb, importer_pb], out_dir,
        extra_args=["--prune", "file:prune_base.proto"],
    )

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    assert "missing dependency file:prune_base.proto" not in result.stderr, (
        "W5 for an explicitly pruned file must be suppressed"
    )


# ---------------------------------------------------------------------------
# TC-R3  W1 loading miss merged into W5 squashed line
# ---------------------------------------------------------------------------

def test_w1_loading_miss_appears_as_w5(tmp_path: Path) -> None:
    """A missing dependency from the loading phase appears as a single W5 squashed line."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    # Compile only prune_importer (which imports prune_base), but do NOT provide
    # prune_base.pb and do NOT put prune_base.proto on any -I path.
    (importer_pb,) = compile_proto(pb_dir, "prune_importer.proto")

    # Use an empty include dir so prune_base.proto cannot be found during loading.
    empty_dir = tmp_path / "empty"
    empty_dir.mkdir()

    result = _run_reproto(
        [importer_pb], out_dir,
        include_dirs=[empty_dir],
    )

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    # Exactly one "missing dependency" line for prune_base.proto
    w5_lines = [
        ln for ln in result.stderr.splitlines()
        if "missing dependency file:prune_base.proto" in ln
    ]
    assert len(w5_lines) == 1, (
        f"Expected exactly one W5 squashed line for prune_base.proto.\nstderr:\n{result.stderr}"
    )
    # No separate "missing file" line
    assert "missing file 'prune_base.proto'" not in result.stderr, (
        "W1 must not produce a separate line — it must merge into the W5 counter"
    )


# ---------------------------------------------------------------------------
# TC-R4  Stripping an import keeps the public and weak indices on their files
# ---------------------------------------------------------------------------

def _write_set(path: Path, *fdps: object) -> Path:
    from google.protobuf.descriptor_pb2 import FileDescriptorSet
    fds = FileDescriptorSet()
    for fdp in fdps:
        fds.file.add().CopyFrom(fdp)  # type: ignore[arg-type]
    path.write_bytes(fds.SerializeToString())
    return path


def _file(name: str, message: str, deps: list[str] = []) -> object:
    from google.protobuf.descriptor_pb2 import FileDescriptorProto
    fdp = FileDescriptorProto(name=name, package="strip_test", syntax="proto2")
    fdp.dependency.extend(deps)
    fdp.message_type.add(name=message)
    return fdp


def test_stripped_import_keeps_public_and_weak_on_their_files(tmp_path: Path) -> None:
    """An import stripped before the pool shifts the ones after it: the
    public and weak indices must follow them (spec 0424 S1).

    importer.proto imports missing.proto (never provided), then weak.proto
    as weak, then public.proto as public. Stripping missing.proto moves
    weak.proto to index 0 and public.proto to index 1.
    """
    out_dir = tmp_path / "out"
    out_dir.mkdir()
    importer = _file(
        "importer.proto", "Importer",
        ["missing.proto", "weak.proto", "public.proto"],
    )
    importer.weak_dependency.append(1)  # type: ignore[attr-defined]
    importer.public_dependency.append(2)  # type: ignore[attr-defined]
    pb = _write_set(
        tmp_path / "set.pb",
        _file("weak.proto", "Weak"),
        _file("public.proto", "Public"),
        importer,
    )
    empty_dir = tmp_path / "empty"
    empty_dir.mkdir()

    result = _run_reproto([pb], out_dir, include_dirs=[empty_dir])

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    text = (out_dir / "importer.proto").read_text()
    imports = [ln.strip() for ln in text.splitlines() if "import" in ln]
    assert 'import weak "weak.proto";' in imports, text
    assert 'import public "public.proto";' in imports, text
    assert '///import "missing.proto";' in imports, text
    assert not any(ln.startswith('import public "weak') for ln in imports), text
