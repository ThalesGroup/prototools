# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Regression tests for glob pattern support in --seed and --prune (spec 0074).

Tests:
  T1 — load-time prune: user-pruned file does not win symbol conflict
  T2 — glob pattern --prune at load time; * does not cross /
  T3 — glob pattern --seed
  T4 — glob --prune combined with plain --seed
  T5 — fqdn_match unit tests
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

from reproto.phases import fqdn_match, fqdn_matches_any
from reproto import Fqdn
from reproto.tests.conftest import FIXTURES_DIR, REPROTO_CMD, compile_proto, compile_proto_multi


# ---------------------------------------------------------------------------
# Helper: run reproto CLI as subprocess
# ---------------------------------------------------------------------------

def _run_reproto(
    pb_files: list[Path],
    out_dir: Path,
    extra_args: list[str] | None = None,
) -> subprocess.CompletedProcess[str]:
    src_path = str(Path(__file__).parent.parent.parent)
    pythonpath_parts = [src_path]
    if existing := os.environ.get("PYTHONPATH"):
        pythonpath_parts.append(existing)
    env = {**os.environ, "PYTHONPATH": os.pathsep.join(pythonpath_parts)}
    env.pop("REPROTO_VARIANT", None)

    cmd = [
        *REPROTO_CMD,
        "--use-variant", "descriptor",
        f"-I{FIXTURES_DIR}",
        f"--proto-out={out_dir}",
    ]
    if extra_args:
        cmd.extend(extra_args)
    cmd.extend(str(p) for p in pb_files)

    return subprocess.run(cmd, capture_output=True, text=True, env=env)


# ---------------------------------------------------------------------------
# T1 — load-time prune: user-pruned file does not win symbol conflict
# ---------------------------------------------------------------------------

def test_T1_load_time_prune_no_conflict_win(tmp_path: Path) -> None:
    """Pruning winner at load time lets loser survive; no spurious auto-prune warning."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    winner_pb, loser_pb = compile_proto(
        pb_dir,
        "prune_conflict_winner.proto",
        "prune_conflict_loser.proto",
    )

    result = _run_reproto(
        [winner_pb, loser_pb], out_dir,
        extra_args=["--prune", "file:prune_conflict_winner.proto"],
    )

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    # winner was pruned at load time — not in output
    assert not (out_dir / "prune_conflict_winner.proto").exists()
    # loser loaded without conflict — must be in output
    assert (out_dir / "prune_conflict_loser.proto").exists()
    # no auto-prune warning about loser
    assert "prune_conflict_loser" not in result.stderr or "duplicate" not in result.stderr


# ---------------------------------------------------------------------------
# T2 — glob pattern --prune at load time; * does not cross /
# ---------------------------------------------------------------------------

def test_T2_glob_prune_single_segment(tmp_path: Path) -> None:
    """file:prune_glob_b* prunes prune_glob_b.proto but not a, c."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    pb_a, pb_b, pb_c = compile_proto(
        pb_dir,
        "prune_glob_a.proto",
        "prune_glob_b.proto",
        "prune_glob_c.proto",
    )

    result = _run_reproto(
        [pb_a, pb_b, pb_c], out_dir,
        extra_args=["--prune", "file:prune_glob_b*"],
    )

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    assert not (out_dir / "prune_glob_b.proto").exists()
    assert (out_dir / "prune_glob_a.proto").exists()
    assert (out_dir / "prune_glob_c.proto").exists()


def test_T2_star_does_not_cross_slash(tmp_path: Path) -> None:
    """* does not match across /; ** does."""
    # file: FQDNs use / as separator, so this tests path-segment semantics.
    assert not fqdn_match(Fqdn("file:subdir/*"), Fqdn("file:subdir/nested/foo.proto"))
    assert fqdn_match(Fqdn("file:subdir/**"), Fqdn("file:subdir/nested/foo.proto"))
    assert fqdn_match(Fqdn("file:subdir/*"), Fqdn("file:subdir/foo.proto"))


# ---------------------------------------------------------------------------
# T3 — glob pattern --seed
# ---------------------------------------------------------------------------

def test_T3_glob_seed(tmp_path: Path) -> None:
    """file:prune_glob_b* as seed renders b and its import a; c is excluded."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    pb_multi = compile_proto_multi(
        pb_dir / "all.pb",
        "prune_glob_a.proto",
        "prune_glob_b.proto",
        "prune_glob_c.proto",
    )

    result = _run_reproto(
        [pb_multi], out_dir,
        extra_args=["--seed", "file:prune_glob_b*"],
    )

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    assert (out_dir / "prune_glob_b.proto").exists()
    assert (out_dir / "prune_glob_a.proto").exists()
    assert not (out_dir / "prune_glob_c.proto").exists()


# ---------------------------------------------------------------------------
# T4 — glob --prune combined with plain --seed
# ---------------------------------------------------------------------------

def test_T4_glob_prune_with_plain_seed(tmp_path: Path) -> None:
    """--seed c --prune 'file:prune_glob_a*': c rendered, a excluded at load time."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    pb_a, pb_b, pb_c = compile_proto(
        pb_dir,
        "prune_glob_a.proto",
        "prune_glob_b.proto",
        "prune_glob_c.proto",
    )

    result = _run_reproto(
        [pb_a, pb_b, pb_c], out_dir,
        extra_args=[
            "--seed", "file:prune_glob_c.proto",
            "--prune", "file:prune_glob_a*",
        ],
    )

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    assert not (out_dir / "prune_glob_a.proto").exists()
    assert (out_dir / "prune_glob_c.proto").exists()


# ---------------------------------------------------------------------------
# T5 — fqdn_match unit tests
# ---------------------------------------------------------------------------

def test_T5_fqdn_match_exact() -> None:
    assert fqdn_match(Fqdn("file:foo.proto"), Fqdn("file:foo.proto"))
    assert not fqdn_match(Fqdn("file:foo.proto"), Fqdn("file:bar.proto"))


def test_T5_fqdn_match_star() -> None:
    assert fqdn_match(Fqdn("file:dir/*.proto"), Fqdn("file:dir/foo.proto"))
    assert not fqdn_match(Fqdn("file:dir/*.proto"), Fqdn("file:dir/sub/foo.proto"))


def test_T5_fqdn_match_doublestar() -> None:
    assert fqdn_match(Fqdn("file:dir/**"), Fqdn("file:dir/sub/foo.proto"))
    assert fqdn_match(Fqdn("file:dir/**"), Fqdn("file:dir/foo.proto"))


def test_T5_fqdn_match_desc_dot_notation() -> None:
    # . in desc: names is treated as / for matching purposes
    assert fqdn_match(Fqdn("desc:my.pkg.*"), Fqdn("desc:.my.pkg.MyMsg"))
    assert not fqdn_match(Fqdn("desc:my.pkg.*"), Fqdn("desc:.my.pkg.sub.MyMsg"))
    assert fqdn_match(Fqdn("desc:my.pkg.**"), Fqdn("desc:.my.pkg.sub.MyMsg"))


def test_T5_fqdn_match_prefix_mismatch() -> None:
    assert not fqdn_match(Fqdn("file:foo.proto"), Fqdn("desc:foo.proto"))


def test_T5_fqdn_matches_any() -> None:
    patterns = [Fqdn("file:a.proto"), Fqdn("file:b*")]
    assert fqdn_matches_any(Fqdn("file:a.proto"), patterns)
    assert fqdn_matches_any(Fqdn("file:baz.proto"), patterns)
    assert not fqdn_matches_any(Fqdn("file:c.proto"), patterns)
