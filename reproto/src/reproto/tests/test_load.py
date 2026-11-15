# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Unit tests for multi-root FDP loading and shadowing (spec 0148, 0149)."""

from __future__ import annotations

import logging
from io import StringIO
from pathlib import Path

from reproto.context import Context
from reproto.load import PathPatterns, QualFile, load_from_path


def _make_ctx(
    pruned_paths: set[str] | None = None,
    path_seeds: set[str] | None = None,
) -> Context:
    return Context(
        set(),
        PathPatterns(pruned_paths or set()),
        PathPatterns(path_seeds or set()),
    )


def _capture_immediate(fn, *args, **kwargs) -> tuple[list[QualFile], list[str]]:
    """Call fn(*args) and return its result plus the stderr lines it produced."""
    buf = StringIO()
    handler = logging.StreamHandler(buf)
    handler.setLevel(logging.WARNING)
    root = logging.getLogger()
    root.addHandler(handler)
    try:
        result = fn(*args, **kwargs)
    finally:
        root.removeHandler(handler)
    buf.seek(0)
    return result, [ln.rstrip() for ln in buf.read().splitlines() if ln.strip()]


# ---------------------------------------------------------------------------
# G1 — every -I root is scanned for directory-shaped seed arguments
# ---------------------------------------------------------------------------

def test_directory_seed_arg_loads_from_every_root(tmp_path: Path) -> None:
    root_a = tmp_path / "a"
    root_b = tmp_path / "b"
    root_a.mkdir()
    root_b.mkdir()
    (root_a / "foo.textpb").write_text('name: "foo.proto"\n')
    (root_b / "bar.textpb").write_text('name: "bar.proto"\n')

    ctx = _make_ctx()
    qual_files, warnings = _capture_immediate(
        load_from_path, ctx, [root_a, root_b], Path('.')
    )

    names = sorted(qf.name for qf in qual_files)
    assert names == ["bar.proto", "foo.proto"]
    assert warnings == []


# ---------------------------------------------------------------------------
# G2/G3 — same FileDescriptorProto.name across roots: first wins, W7 fires
# ---------------------------------------------------------------------------

def test_same_name_across_roots_shadows_and_warns(tmp_path: Path) -> None:
    root_a = tmp_path / "a"
    root_b = tmp_path / "b"
    root_a.mkdir()
    root_b.mkdir()
    (root_a / "foo.textpb").write_text('name: "foo.proto"\npackage: "pkg_a"\n')
    (root_b / "foo.textpb").write_text('name: "foo.proto"\npackage: "pkg_b"\n')

    ctx = _make_ctx()
    qual_files, warnings = _capture_immediate(
        load_from_path, ctx, [root_a, root_b], Path('.')
    )

    assert len(qual_files) == 1
    assert qual_files[0].desc.package == "pkg_a", "first root (-I order) must win"
    assert len(warnings) == 1
    assert "foo.proto" in warnings[0]
    assert str(root_a / "foo.textpb") in warnings[0]
    assert str(root_b / "foo.textpb") in warnings[0]
    assert warnings[0].startswith("Warning:")


# ---------------------------------------------------------------------------
# G4 — a same-name collision within a single root also shadows and warns
# ---------------------------------------------------------------------------

def test_same_name_within_one_root_shadows_and_warns(tmp_path: Path) -> None:
    root = tmp_path / "a"
    root.mkdir()
    (root / "foo.textpb").write_text('name: "foo.proto"\npackage: "pkg_1"\n')
    (root / "foo_copy.textpb").write_text('name: "foo.proto"\npackage: "pkg_2"\n')

    ctx = _make_ctx()
    qual_files, warnings = _capture_immediate(
        load_from_path, ctx, [root], Path('.')
    )

    assert len(qual_files) == 1
    assert len(warnings) == 1


# ---------------------------------------------------------------------------
# N2 — single-file resolution is unaffected: first root wins silently
# ---------------------------------------------------------------------------

def test_single_file_seed_arg_stops_at_first_root_no_warning(tmp_path: Path) -> None:
    root_a = tmp_path / "a"
    root_b = tmp_path / "b"
    root_a.mkdir()
    root_b.mkdir()
    (root_a / "foo.textpb").write_text('name: "foo.proto"\npackage: "pkg_a"\n')
    (root_b / "foo.textpb").write_text('name: "foo.proto"\npackage: "pkg_b"\n')

    ctx = _make_ctx()
    qual_files, warnings = _capture_immediate(
        load_from_path, ctx, [root_a, root_b], Path('foo.textpb')
    )

    assert len(qual_files) == 1
    assert qual_files[0].desc.package == "pkg_a"
    assert warnings == [], (
        "single-file resolution must stop at the first root without probing "
        "later roots, so no shadow warning is possible"
    )


# ---------------------------------------------------------------------------
# G3 — prune-by-path disambiguates a same-name collision across roots
# (spec 0149; the "~2"-duplicate scenario, with two physically
# distinct paths that happen to parse to the same FileDescriptorProto.name)
# ---------------------------------------------------------------------------

def test_G3_prune_by_path_disambiguates_duplicate_name(tmp_path: Path) -> None:
    root_a = tmp_path / "a"
    root_b = tmp_path / "b"
    root_a.mkdir()
    root_b.mkdir()
    (root_a / "nested").mkdir()
    (root_b / "other").mkdir()
    (root_a / "nested" / "foo.textpb").write_text(
        'name: "foo.proto"\npackage: "pkg_a"\n')
    (root_b / "other" / "foo.textpb").write_text(
        'name: "foo.proto"\npackage: "pkg_b"\n')

    ctx = _make_ctx(pruned_paths={"nested/foo.textpb"})
    qual_files, warnings = _capture_immediate(
        load_from_path, ctx, [root_a, root_b], Path('.')
    )

    assert len(qual_files) == 1
    assert qual_files[0].desc.package == "pkg_b", (
        "root_a's candidate was pruned by path; root_b's survives"
    )
    assert warnings == [], "only one physical candidate remains -> no W7"


def test_G3_prune_by_path_disambiguation_independent_of_root_order(
        tmp_path: Path) -> None:
    root_a = tmp_path / "a"
    root_b = tmp_path / "b"
    root_a.mkdir()
    root_b.mkdir()
    (root_a / "nested").mkdir()
    (root_b / "other").mkdir()
    (root_a / "nested" / "foo.textpb").write_text(
        'name: "foo.proto"\npackage: "pkg_a"\n')
    (root_b / "other" / "foo.textpb").write_text(
        'name: "foo.proto"\npackage: "pkg_b"\n')

    ctx = _make_ctx(pruned_paths={"nested/foo.textpb"})
    qual_files, warnings = _capture_immediate(
        load_from_path, ctx, [root_b, root_a], Path('.')  # reversed order
    )

    assert len(qual_files) == 1
    assert qual_files[0].desc.package == "pkg_b"
    assert warnings == []


# ---------------------------------------------------------------------------
# G6 — bare path pattern matches identically-shaped files under two
# different -I roots (root-independence; spec 0149)
# ---------------------------------------------------------------------------

def test_G6_bare_path_pattern_matches_under_every_root(tmp_path: Path) -> None:
    root_a = tmp_path / "a"
    root_b = tmp_path / "b"
    root_a.mkdir()
    root_b.mkdir()
    (root_a / "pkg").mkdir()
    (root_b / "pkg").mkdir()
    (root_a / "pkg" / "thing_a.textpb").write_text('name: "thing_a.proto"\n')
    (root_b / "pkg" / "thing_b.textpb").write_text('name: "thing_b.proto"\n')

    ctx = _make_ctx(path_seeds={"pkg/*.textpb"})
    _capture_immediate(load_from_path, ctx, [root_a, root_b], Path('.'))

    assert ctx.path_seed_fqdns == {"file:thing_a.proto", "file:thing_b.proto"}, (
        "a bare glob pattern must match a candidate the same way regardless "
        "of which -I root it was found under"
    )
