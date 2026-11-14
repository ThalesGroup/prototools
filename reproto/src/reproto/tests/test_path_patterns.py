# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Tests for path-pattern support in -s/-p/-C (spec 0149).

Tests:
  U2 — PathPatterns (G8): literal vs glob partitioning and matching
  U3 — glob semantics documentation-pinning: **/*~2.pb vs bare *~2.pb
  G3 — prune-by-path eliminating every physical candidate for an import
       fires the existing W1 "missing dependency" warning
  G4 — seed-by-path: a multi-FDP bundle seeds every FDP it contains
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

from reproto import Fqdn
from reproto.load import PathPatterns
from reproto.phases import fqdn_match
from reproto.tests.conftest import FIXTURES_DIR, REPROTO_CMD, compile_proto, compile_proto_multi


# ---------------------------------------------------------------------------
# U2 — PathPatterns (G8): literal vs glob partitioning and matching
# ---------------------------------------------------------------------------

def test_U2_partition_literal_vs_glob() -> None:
    pp = PathPatterns({"foo/bar.pb", "foo/*.pb", "foo?.pb", "foo[0].pb"})
    assert pp.literals == {"foo/bar.pb"}
    assert set(pp.globs) == {"foo/*.pb", "foo?.pb", "foo[0].pb"}


def test_U2_literal_matches_exact_string_only() -> None:
    pp = PathPatterns({"foo/bar.pb"})
    assert pp.matches(Path("foo/bar.pb"))
    assert not pp.matches(Path("foo/barx.pb"))
    assert not pp.matches(Path("foo/ba"))
    assert not pp.matches(Path("foo/bar.pbx"))


def test_U2_glob_matches_via_fqdn_match() -> None:
    pp = PathPatterns({"foo/*.pb"})
    assert pp.matches(Path("foo/bar.pb"))
    assert not pp.matches(Path("foo/sub/bar.pb")), "* must not cross /"


def test_U2_empty_pattern_set_matches_nothing() -> None:
    pp = PathPatterns(set())
    assert not pp.matches(Path("anything.pb"))


# ---------------------------------------------------------------------------
# U3 — glob semantics documentation-pinning: **/*~2.pb vs bare *~2.pb
# ---------------------------------------------------------------------------

def test_U3_doublestar_matches_top_level_and_nested() -> None:
    assert fqdn_match(Fqdn("path:**/*~2.pb"), Fqdn("path:foo~2.pb"))
    assert fqdn_match(Fqdn("path:**/*~2.pb"), Fqdn("path:bar/foo~2.pb"))


def test_U3_bare_star_matches_only_top_level() -> None:
    assert fqdn_match(Fqdn("path:*~2.pb"), Fqdn("path:foo~2.pb"))
    assert not fqdn_match(Fqdn("path:*~2.pb"), Fqdn("path:bar/foo~2.pb"))


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
# G3 — prune-by-path eliminating every physical candidate for an import
# fires the existing W1 "missing dependency" warning; no crash.
# ---------------------------------------------------------------------------

def test_G3_prune_by_path_eliminates_every_import_candidate(tmp_path: Path) -> None:
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    pb_a, pb_b = compile_proto(pb_dir, "prune_glob_a.proto", "prune_glob_b.proto")

    # prune_glob_b.proto imports prune_glob_a.proto; pruning the import's
    # declared .proto path eliminates every physical extension candidate
    # for it (spec 0149 G3).
    result = _run_reproto(
        [Path(pb_b.name)], out_dir,
        extra_args=[f"-I{pb_dir}", "--prune", "prune_glob_a.proto"],
    )

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    assert "missing dependency file:prune_glob_a.proto" in result.stderr
    assert "Traceback" not in result.stderr
    assert not (out_dir / "prune_glob_a.proto").exists()


# ---------------------------------------------------------------------------
# G4 — seed-by-path: a multi-FDP bundle seeds every FDP it contains
# ---------------------------------------------------------------------------

def test_G4_seed_by_path_multi_fdp_bundle(tmp_path: Path) -> None:
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    pb_multi = compile_proto_multi(
        pb_dir / "bundle.protoset",
        "prune_glob_a.proto",
        "prune_glob_b.proto",
        "prune_glob_c.proto",
    )

    result = _run_reproto(
        [Path(pb_multi.name)], out_dir,
        extra_args=[f"-I{pb_dir}", "--seed", "bundle.protoset"],
    )

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    assert (out_dir / "prune_glob_a.proto").exists()
    assert (out_dir / "prune_glob_b.proto").exists()
    assert (out_dir / "prune_glob_c.proto").exists()
