# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Tests for out-of-range scoring as a penalty, end to end (spec 0178).

After spec 0176 the `Range` leaf reaches exactly two kinds of field:
`bool`, with range `(0, 1)`, and a *closed* enum. An out-of-range value on
either used to veto, which eliminated the candidate outright. But neither
value is impossible on the wire -- `bool` is `value != 0` in every
generated parser, so 2 reads as `true` -- and the governing principle
vetoes only the impossible. The charge is now `out_of_range`, weighted
-15.

The closed-enum half is covered by test_open_enum_scoring.py's E3, which
already had the fixture. This file covers `bool`, the ranking consequence,
and the removal of the `--relax-ranges` knob that used to be the escape
hatch.

Why the ranking assertion matters more than the counter: the counter only
shows that the charge landed somewhere. What spec 0178 actually buys is
that the candidate is still *in* the ranking to be outscored, rather than
absent from it -- and simultaneously that it does not win, because -15 is
still a heavy penalty. A test that only asserted survival would pass
equally well if the check had been deleted.
"""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path

from reproto.tests.test_open_enum_scoring import _build_db, _prototext

# field 1, varint 802 -- outside a bool's [0, 1], but an ordinary int32.
_OUT_OF_RANGE_PAYLOAD = b"\x08\xa2\x06"

# Two competing single-field schemas over the same tag, so 802 is
# out-of-range under one and clean under the other. This is
# `tc77_12_bool_vs_int32_discrimination` (prototext-graph) driven through
# the binary.
_BOOL_VS_INT32_PROTO = """\
    syntax = "proto3";
    package oor;
    message Flags { bool enabled = 1; }
    message Count { int32 n = 1; }
"""


def _payload(tmp_path: Path) -> Path:
    path = tmp_path / "oor.bin"
    path.write_bytes(_OUT_OF_RANGE_PAYLOAD)
    return path


def test_R1_bool_out_of_range_is_penalized_not_vetoed(tmp_path: Path) -> None:
    """A `bool` of 802 is charged `out_of_range`, not vetoed (spec 0178 S2).

    `non_canonical: 0` is asserted too: the varint is minimally encoded, so
    the only thing wrong with it is the value, and the two counters must
    stay distinguishable.
    """
    db_path = _build_db(tmp_path, "oor", _BOOL_VS_INT32_PROTO)
    out = _prototext(db_path, _payload(tmp_path), "score", "--type",
                     "oor.Flags")
    assert "vetoed" not in out, f"802 parses as `true`; no veto:\n{out}"
    assert "out_of_range: 1" in out, out
    assert "non_canonical: 0" in out, f"the encoding is canonical:\n{out}"


def test_R2_penalized_candidate_stays_in_the_ranking_but_loses(
    tmp_path: Path,
) -> None:
    """`oor.Flags` survives to be ranked, and `oor.Count` outranks it.

    Both halves are the point. Survival is what spec 0178 changed; losing
    is what proves the -15 penalty still discriminates, so the fix did not
    buy correctness by giving up precision.
    """
    db_path = _build_db(tmp_path, "oor", _BOOL_VS_INT32_PROTO)
    out = _prototext(db_path, _payload(tmp_path), "list-schemas", "--top", "5")
    assert "oor.Flags" in out, f"a penalized candidate is not eliminated:\n{out}"
    assert "oor.Count" in out, out
    assert out.index("oor.Count") < out.index("oor.Flags"), (
        f"list-schemas is score-descending; Count must come first:\n{out}"
    )


def test_R3_detailed_score_reports_out_of_range(tmp_path: Path) -> None:
    """`list-schemas --detailed-score` surfaces the new counter (spec 0178 S4)."""
    db_path = _build_db(tmp_path, "oor", _BOOL_VS_INT32_PROTO)
    out = _prototext(db_path, _payload(tmp_path), "list-schemas",
                     "--top", "5", "--detailed-score")
    assert "out_of_range: 1" in out, out


def test_R4_the_relax_ranges_knob_is_gone(tmp_path: Path) -> None:
    """`--relax-ranges` and its `--no-strict-ranges` alias now error.

    Spec 0178 S3 deletes the knob rather than leaving it inert: with one
    behavior there is nothing to opt into, and an accepted-but-ignored flag
    would silently mislead. A hard error is the honest signal.
    """
    db_path = _build_db(tmp_path, "oor", _BOOL_VS_INT32_PROTO)
    payload = _payload(tmp_path)
    bin_path = shutil.which("prototext")
    assert bin_path is not None

    for flag in ("--relax-ranges", "--no-strict-ranges"):
        result = subprocess.run(
            [bin_path, "--descriptor-set", str(db_path), "score",
             "--type", "oor.Flags", flag, "--assume-binary", str(payload)],
            capture_output=True, text=True,
        )
        assert result.returncode != 0, f"{flag} must be rejected:\n{result.stdout}"
        assert flag in result.stderr, result.stderr
