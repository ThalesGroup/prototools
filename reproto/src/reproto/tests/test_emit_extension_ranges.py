# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Tests for --emit-extension-ranges and its canonical form (spec 0238 S2-S3).

A message that declares `extensions 1000 to max` is telling the scorer that
an unrecognized field number in that band is legal, not evidence against the
type.  Recording that is what these tests cover, plus the property the rest
of the pipeline leans on: the emitted form is **unique for a given set of
field numbers**.  Spec 0238 S5 interns range sets and uses the intern index
as the equality test, so two messages admitting the same numbers must reach
the same index however their `.proto` spelled it.

The fixture spells one single set — 1000..2999 — four ways, and the
end-to-end tests assert all four land on the same canonical list.  Overlap
and degeneracy are unit-tested against the function instead, because protoc
rejects an overlapping `extensions` clause outright, so no fixture can reach
those branches; a hand-assembled descriptor can.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path
from typing import Any

import yaml

from reproto.phases import _canonical_extension_ranges
from reproto.tests.conftest import compile_proto, FIXTURES_DIR

# 2**29 - 1, the largest legal protobuf field number, which is what protoc
# has already materialized `max` into by the time reproto sees a descriptor.
MAX_FIELD_NUMBER = 536870911


class _FakeDescriptor:
    """Minimal stand-in exposing only what the canonicalizer reads."""

    def __init__(self, extension_ranges: list[tuple[int, int]]):
        self.extension_ranges = extension_ranges


def _run_reproto(pb: Path, out_dir: Path, extra_args: list[str]) -> None:
    src_path = str(Path(__file__).parent.parent.parent)
    pythonpath_parts = [src_path]
    if existing := os.environ.get("PYTHONPATH"):
        pythonpath_parts.append(existing)
    env = {**os.environ, "PYTHONPATH": os.pathsep.join(pythonpath_parts)}
    env.pop("REPROTO_VARIANT", None)

    result = subprocess.run(
        [
            sys.executable, "-m", "reproto.cli",
            "--use-variant", "descriptor",
            f"-I{FIXTURES_DIR}",
            f"--proto-out={out_dir}",
            "--emit-scoring-yaml",
            *extra_args,
            str(pb),
        ],
        capture_output=True, text=True, env=env,
    )
    assert result.returncode == 0, result.stderr


def _messages(tmp_path: Path, extra_args: list[str]) -> dict[str, Any]:
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()
    (pb,) = compile_proto(pb_dir, "extension_ranges.proto")
    _run_reproto(pb, out_dir, extra_args)

    (yaml_path,) = out_dir.rglob("extension_ranges.yaml")
    data = yaml.safe_load(yaml_path.read_text())
    assert isinstance(data, dict)
    messages = data["messages"]
    assert isinstance(messages, dict)
    return messages


# ---------------------------------------------------------------------------
# The flag (S2)
# ---------------------------------------------------------------------------

def test_flag_off_emits_no_ext_ranges_key(tmp_path: Path) -> None:
    """Without the flag the YAML is exactly what it was before spec 0238."""
    messages = _messages(tmp_path, [])
    assert messages, "fixture should have produced messages"
    for name, body in messages.items():
        assert "ext_ranges" not in body, f"{name} carries ext_ranges without the flag"


def test_flag_on_records_the_declared_band(tmp_path: Path) -> None:
    messages = _messages(tmp_path, ["--emit-extension-ranges"])
    assert messages["extranges.OneClause"]["ext_ranges"] == [[1000, 2999]]


def test_closed_message_gets_no_key(tmp_path: Path) -> None:
    """A message with no `extensions` clause stays closed by absence.

    Spec 0238 S5: closedness is the `NO_EXT_RANGES` sentinel downstream, not
    an empty set, so an empty list must never be emitted either.
    """
    messages = _messages(tmp_path, ["--emit-extension-ranges"])
    assert "ext_ranges" not in messages["extranges.Closed"]


# ---------------------------------------------------------------------------
# Uniqueness of the canonical form (S3)
# ---------------------------------------------------------------------------

def test_every_spelling_of_one_set_canonicalizes_alike(tmp_path: Path) -> None:
    """Four spellings of 1000..2999 must produce one identical list.

    Adjacency is the interesting one: `1000 to 1999` and `2000 to 2999` do
    not overlap, so a merge that only handled overlap would leave two ranges
    here and intern this message separately from `OneClause` — splitting two
    states that admit exactly the same field numbers.
    """
    messages = _messages(tmp_path, ["--emit-extension-ranges"])
    spellings = [
        "extranges.OneClause",
        "extranges.TwoAdjacentClauses",
        "extranges.OutOfOrderClauses",
        "extranges.SingleNumberClause",
    ]
    emitted = {name: messages[name]["ext_ranges"] for name in spellings}
    assert set(map(str, emitted.values())) == {"[[1000, 2999]]"}, emitted


def test_to_max_is_materialized_not_symbolic(tmp_path: Path) -> None:
    """`to max` must come out as the concrete number, never a sentinel.

    Otherwise `1000 to max` and `1000 to 536870911` — the same set — would
    intern as two.
    """
    messages = _messages(tmp_path, ["--emit-extension-ranges"])
    assert messages["extranges.Unbounded"]["ext_ranges"] == [[1000, MAX_FIELD_NUMBER]]


# ---------------------------------------------------------------------------
# The canonicalizer itself, on inputs protoc cannot produce
# ---------------------------------------------------------------------------

def test_end_is_exclusive() -> None:
    """`extensions 10000` arrives as the half-open (10000, 10001)."""
    assert _canonical_extension_ranges(_FakeDescriptor([(10000, 10001)])) == [[10000, 10000]]


def test_overlapping_ranges_merge() -> None:
    assert _canonical_extension_ranges(
        _FakeDescriptor([(1000, 2001), (1500, 3001)])
    ) == [[1000, 3000]]


def test_a_contained_range_does_not_shrink_its_container() -> None:
    """The merge must take the max of the two ends, not the later one."""
    assert _canonical_extension_ranges(
        _FakeDescriptor([(1000, 5001), (2000, 3001)])
    ) == [[1000, 5000]]


def test_degenerate_ranges_are_dropped() -> None:
    assert _canonical_extension_ranges(_FakeDescriptor([(1000, 1000), (5000, 4000)])) == []


def test_disjoint_non_adjacent_ranges_survive_separately() -> None:
    """Merging must stop at a genuine gap, or distinct sets would collide."""
    assert _canonical_extension_ranges(
        _FakeDescriptor([(3000, 4001), (1000, 2001)])
    ) == [[1000, 2000], [3000, 4000]]


def test_no_ranges_is_the_empty_list() -> None:
    assert _canonical_extension_ranges(_FakeDescriptor([])) == []
