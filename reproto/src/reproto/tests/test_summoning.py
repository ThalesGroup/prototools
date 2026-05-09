# SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Regression tests for the summoning algorithm (spec 0046).

Each test compiles fixture .proto files with protoc, then runs the reproto CLI
and asserts which files appear in the output directory.

Fixture files live under tests/fixtures/ and are named tc<N>_*.proto.
TC-5 reuses the bridge_lex_*.proto fixtures.
"""

from __future__ import annotations

import os
import sys
import subprocess
from pathlib import Path

from reproto.tests.conftest import compile_proto, FIXTURES_DIR


# ---------------------------------------------------------------------------
# Helper
# ---------------------------------------------------------------------------

def _run_reproto(
    pb_files: list[Path],
    out_dir: Path,
    seeds: list[str] | None = None,
    prunings: list[str] | None = None,
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
        sys.executable, "-m", "reproto.cli",
        "--use-variant", "descriptor",
        *include_flags,
        f"--output-root={out_dir}",
    ]
    for seed in (seeds or []):
        cmd.extend(["--seed", seed])
    for pruning in (prunings or []):
        cmd.extend(["--prune", pruning])
    cmd.extend(str(p) for p in pb_files)

    return subprocess.run(cmd, capture_output=True, text=True, env=env)


def _written(out_dir: Path) -> set[str]:
    """Return the set of .proto file names written under out_dir."""
    return {p.name for p in out_dir.rglob("*.proto")}


# ---------------------------------------------------------------------------
# TC-1: Direct import — no bridge needed
# ---------------------------------------------------------------------------

def test_TC1_direct_import_no_bridge(tmp_path: Path) -> None:
    """A directly imports C; TA has a field of type TC.  Only A and C written."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    tc1_c, tc1_a = compile_proto(pb_dir, "tc1_c.proto", "tc1_a.proto")

    result = _run_reproto([tc1_c, tc1_a], out_dir, seeds=["desc:.tc1.TA"])

    assert result.returncode == 0, result.stderr
    written = _written(out_dir)
    assert written == {"tc1_a.proto", "tc1_c.proto"}


# ---------------------------------------------------------------------------
# TC-2: One-hop bridge — A imports B imports C; A references TC
# ---------------------------------------------------------------------------

def test_TC2_one_hop_bridge(tmp_path: Path) -> None:
    """A imports B imports C; TA has a field of type TC.
    B must be written as a bridge even though nothing in B is seeded."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    tc2_c, tc2_b, tc2_a = compile_proto(pb_dir, "tc2_c.proto", "tc2_b.proto", "tc2_a.proto")

    result = _run_reproto([tc2_c, tc2_b, tc2_a], out_dir, seeds=["desc:.tc2.TA"])

    assert result.returncode == 0, result.stderr
    written = _written(out_dir)
    assert "tc2_b.proto" in written, "tc2_b.proto must be written as a bridge"
    assert written == {"tc2_a.proto", "tc2_b.proto", "tc2_c.proto"}


# ---------------------------------------------------------------------------
# TC-3: Two-hop bridge — A imports B imports D imports C; A references TC
# ---------------------------------------------------------------------------

def test_TC3_two_hop_bridge(tmp_path: Path) -> None:
    """A→B→D→C chain; TA references TC.  Both B and D must be written as bridges."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    tc3_c, tc3_d, tc3_b, tc3_a = compile_proto(
        pb_dir, "tc3_c.proto", "tc3_d.proto", "tc3_b.proto", "tc3_a.proto"
    )

    result = _run_reproto([tc3_c, tc3_d, tc3_b, tc3_a], out_dir, seeds=["desc:.tc3.TA"])

    assert result.returncode == 0, result.stderr
    assert _written(out_dir) == {"tc3_a.proto", "tc3_b.proto", "tc3_d.proto", "tc3_c.proto"}


# ---------------------------------------------------------------------------
# TC-4: No spurious summoning — leaf type with no message fields
# ---------------------------------------------------------------------------

def test_TC4_no_spurious_summoning(tmp_path: Path) -> None:
    """Seeding only Leaf (scalar fields only) must not pull in tc4_user.proto."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    tc4_leaf, tc4_user = compile_proto(pb_dir, "tc4_leaf.proto", "tc4_user.proto")

    result = _run_reproto([tc4_leaf, tc4_user], out_dir, seeds=["desc:.tc4.Leaf"])

    assert result.returncode == 0, result.stderr
    written = _written(out_dir)
    assert "tc4_leaf.proto" in written
    assert "tc4_user.proto" not in written, (
        "tc4_user.proto imports Leaf but is not reachable — must not be written"
    )


# ---------------------------------------------------------------------------
# TC-5: Lexicographic tie-breaking — two equal-length paths, lex-smallest wins
# ---------------------------------------------------------------------------

def test_TC5_lex_tie_breaking(tmp_path: Path) -> None:
    """A imports B1 and B2, both import C; A references LC.
    Both paths have length 2.  B1 < B2 lexicographically so only B1 is written."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    lex_c, lex_b1, lex_b2, lex_a = compile_proto(
        pb_dir,
        "bridge_lex_c.proto",
        "bridge_lex_b1.proto",
        "bridge_lex_b2.proto",
        "bridge_lex_a.proto",
    )

    result = _run_reproto(
        [lex_c, lex_b1, lex_b2, lex_a], out_dir, seeds=["desc:.bridgelex.LexA"]
    )

    assert result.returncode == 0, result.stderr
    written = _written(out_dir)
    assert "bridge_lex_a.proto" in written
    assert "bridge_lex_c.proto" in written
    assert "bridge_lex_b1.proto" in written, "B1 is lex-smaller — must be the chosen bridge"
    assert "bridge_lex_b2.proto" not in written, "B2 is lex-larger — must not be written"


# ---------------------------------------------------------------------------
# TC-6: Multiple references, shared bridge used once
# ---------------------------------------------------------------------------

def test_TC6_shared_bridge(tmp_path: Path) -> None:
    """A imports B imports C; TA has fields of both TB and TC.
    B serves as both a direct dependency and a bridge for TC.
    All three files must be written, B exactly once."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    tc6_c, tc6_b, tc6_a = compile_proto(pb_dir, "tc6_c.proto", "tc6_b.proto", "tc6_a.proto")

    result = _run_reproto([tc6_c, tc6_b, tc6_a], out_dir, seeds=["desc:.tc6.TA"])

    assert result.returncode == 0, result.stderr
    assert _written(out_dir) == {"tc6_a.proto", "tc6_b.proto", "tc6_c.proto"}


# ---------------------------------------------------------------------------
# TC-7: Import-only file not written — phase 7 output filter regression
# ---------------------------------------------------------------------------

def test_TC7_import_only_not_written(tmp_path: Path) -> None:
    """A file that only imports a seeded file must not itself be written.

    Regression for the old phase 7 filter: any(target.is_summoned for target
    in re_fdp.targets) — which wrote any file importing a summoned file.
    """
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    tc7_s, tc7_u = compile_proto(pb_dir, "tc7_s.proto", "tc7_u.proto")

    result = _run_reproto([tc7_s, tc7_u], out_dir, seeds=["desc:.tc7.TS"])

    assert result.returncode == 0, result.stderr
    written = _written(out_dir)
    assert "tc7_s.proto" in written
    assert "tc7_u.proto" not in written, (
        "tc7_u.proto only imports the seeded file — must not be written"
    )


# ---------------------------------------------------------------------------
# TC-8: Pruned seed file — prune overrides seed
# ---------------------------------------------------------------------------

def test_TC8_pruned_seed_not_written(tmp_path: Path) -> None:
    """A file that is both a seed and explicitly pruned must not be written.

    Regression for the phase 5 default-seed path not respecting is_pruned:
    tc8_p.proto was marked reachable by the seed loop even after phase 4
    set is_pruned on it.
    """
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    tc8_p, tc8_q = compile_proto(pb_dir, "tc8_p.proto", "tc8_q.proto")

    result = _run_reproto(
        [tc8_p, tc8_q], out_dir,
        prunings=["file:tc8_p.proto"],
    )

    assert result.returncode == 0, result.stderr
    written = _written(out_dir)
    assert "tc8_q.proto" in written
    assert "tc8_p.proto" not in written, (
        "tc8_p.proto is explicitly pruned — must not be written even though "
        "it was loaded as a seed"
    )
