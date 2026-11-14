# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Tests for --filter-config (spec 0149): filter_config.load() and CLI wiring.

Tests:
  U1 — filter_config.load() unit tests (valid/invalid YAML shapes)
  T1 — -C/--filter-config alone reproduces equivalent -s/-p output
  T2 — -C/--filter-config merges (union) with CLI -s/-p flags
  T3 — missing --filter-config file -> clean click error
  T4 — unknown top-level key in filter-config -> clean UsageError
  G6 — bare filter-config entry (no prefix) resolves to a path pattern
  G7 — colon-containing value with an unrecognized prefix -> clean UsageError
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

from reproto.filter_config import load as load_filter_config
from reproto.tests.conftest import FIXTURES_DIR, REPROTO_CMD, compile_proto


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
# U1 — filter_config.load() unit tests
# ---------------------------------------------------------------------------

def test_U1_load_valid_both_keys(tmp_path: Path) -> None:
    p = tmp_path / "filter.yaml"
    p.write_text("seed:\n  - file:foo.proto\nprune:\n  - file:bar.proto\n")
    seed, prune = load_filter_config(p)
    assert seed == ["file:foo.proto"]
    assert prune == ["file:bar.proto"]


def test_U1_load_only_seed_key(tmp_path: Path) -> None:
    p = tmp_path / "filter.yaml"
    p.write_text("seed:\n  - file:foo.proto\n")
    seed, prune = load_filter_config(p)
    assert seed == ["file:foo.proto"]
    assert prune == []


def test_U1_load_empty_file(tmp_path: Path) -> None:
    p = tmp_path / "filter.yaml"
    p.write_text("")
    seed, prune = load_filter_config(p)
    assert seed == []
    assert prune == []


def test_U1_load_unknown_key(tmp_path: Path) -> None:
    p = tmp_path / "filter.yaml"
    p.write_text("prunes:\n  - file:bar.proto\n")  # typo: 'prunes' not 'prune'
    with pytest.raises(ValueError, match="unknown filter-config key"):
        load_filter_config(p)


def test_U1_load_not_a_mapping(tmp_path: Path) -> None:
    p = tmp_path / "filter.yaml"
    p.write_text("- foo\n- bar\n")
    with pytest.raises(ValueError, match="must be a YAML mapping"):
        load_filter_config(p)


def test_U1_load_seed_not_a_list(tmp_path: Path) -> None:
    p = tmp_path / "filter.yaml"
    p.write_text("seed: file:foo.proto\n")
    with pytest.raises(ValueError, match="'seed' must be a list of strings"):
        load_filter_config(p)


def test_U1_load_seed_list_non_string_item(tmp_path: Path) -> None:
    p = tmp_path / "filter.yaml"
    p.write_text("seed:\n  - foo: bar\n")
    with pytest.raises(ValueError, match="'seed' must be a list of strings"):
        load_filter_config(p)


def test_U1_load_malformed_yaml(tmp_path: Path) -> None:
    p = tmp_path / "filter.yaml"
    p.write_text("seed: [unterminated\n")
    with pytest.raises(ValueError, match="invalid YAML"):
        load_filter_config(p)


# ---------------------------------------------------------------------------
# T1 — -C/--filter-config alone reproduces equivalent -s/-p output
# ---------------------------------------------------------------------------

def test_T1_filter_config_equivalent_to_cli_seed(tmp_path: Path) -> None:
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

    config = tmp_path / "filter.yaml"
    config.write_text("seed:\n  - file:prune_glob_b*\n")

    result = _run_reproto(
        [pb_a, pb_b, pb_c], out_dir,
        extra_args=["--filter-config", str(config)],
    )

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    assert (out_dir / "prune_glob_b.proto").exists()
    assert (out_dir / "prune_glob_a.proto").exists()
    assert not (out_dir / "prune_glob_c.proto").exists()


# ---------------------------------------------------------------------------
# T2 — -C/--filter-config merges (union) with CLI -s/-p flags
# ---------------------------------------------------------------------------

def test_T2_filter_config_unions_with_cli_flags(tmp_path: Path) -> None:
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

    # Config seeds 'a'; CLI seeds 'c'; b is excluded by neither -> not reachable.
    config = tmp_path / "filter.yaml"
    config.write_text("seed:\n  - file:prune_glob_a.proto\n")

    result = _run_reproto(
        [pb_a, pb_b, pb_c], out_dir,
        extra_args=[
            "-C", str(config),
            "--seed", "file:prune_glob_c.proto",
        ],
    )

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    assert (out_dir / "prune_glob_a.proto").exists()
    assert (out_dir / "prune_glob_c.proto").exists()
    assert not (out_dir / "prune_glob_b.proto").exists()


# ---------------------------------------------------------------------------
# T3 — missing --filter-config file -> clean click error
# ---------------------------------------------------------------------------

def test_T3_filter_config_missing_file(tmp_path: Path) -> None:
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()
    pb_a, = compile_proto(pb_dir, "prune_glob_a.proto")

    result = _run_reproto(
        [pb_a], out_dir,
        extra_args=["--filter-config", str(tmp_path / "nonexistent.yaml")],
    )

    assert result.returncode != 0
    assert "Traceback" not in result.stderr


# ---------------------------------------------------------------------------
# T4 — unknown top-level key in filter-config -> clean UsageError
# ---------------------------------------------------------------------------

def test_T4_filter_config_unknown_key(tmp_path: Path) -> None:
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()
    pb_a, = compile_proto(pb_dir, "prune_glob_a.proto")

    config = tmp_path / "filter.yaml"
    config.write_text("prunes:\n  - file:bar.proto\n")  # typo

    result = _run_reproto(
        [pb_a], out_dir,
        extra_args=["--filter-config", str(config)],
    )

    assert result.returncode != 0
    assert "unknown filter-config key" in result.stderr
    assert "Traceback" not in result.stderr


# ---------------------------------------------------------------------------
# G6 — bare filter-config entry (no prefix) resolves to a path pattern
# ---------------------------------------------------------------------------

def test_G6_bare_filter_config_entry_resolves_to_path(tmp_path: Path) -> None:
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

    # No 'file:' prefix -> classified as a (literal) path pattern (G2),
    # matched against the root-relative path of the physical .pb candidate.
    config = tmp_path / "filter.yaml"
    config.write_text("seed:\n  - prune_glob_b.pb\n")

    # Pass relative filenames + an extra -I root so QualFile.rel_path is a
    # bare relative filename that the literal path pattern can match.
    result = _run_reproto(
        [Path(pb_a.name), Path(pb_b.name), Path(pb_c.name)], out_dir,
        extra_args=[f"-I{pb_dir}", "--filter-config", str(config)],
    )

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    assert (out_dir / "prune_glob_b.proto").exists()
    assert (out_dir / "prune_glob_a.proto").exists()
    assert not (out_dir / "prune_glob_c.proto").exists()


# ---------------------------------------------------------------------------
# G7 — colon-containing value with an unrecognized prefix -> clean UsageError
# ---------------------------------------------------------------------------

def test_G7_unrecognized_prefix_raises_clean_usage_error(tmp_path: Path) -> None:
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()
    pb_a, = compile_proto(pb_dir, "prune_glob_a.proto")

    result = _run_reproto(
        [pb_a], out_dir,
        extra_args=["--prune", "fille:foo.proto"],  # typo'd prefix
    )

    assert result.returncode != 0
    assert "fille" in result.stderr
    assert "path:" in result.stderr
    assert "Traceback" not in result.stderr
