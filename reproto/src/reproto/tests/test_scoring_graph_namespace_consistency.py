# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Tests for hopcroft.rkyv / .desc name consistency (spec 0166).

Regression tests for a bug where a renaming was applied to .desc's names
at render time (spec 0159) but not to hopcroft.rkyv's names, which were
read directly off ctx.pool, populated before any renaming exists. A
schema-db consumer scoring by .desc's (renamed) name then got "not found
in scoring graph" for types that were, in fact, present in the compiled
graph under their old name. The renaming is --target protoc's (spec 0447
S3): the vendor descriptor's definitions renamed to google.protobuf.

Verifying hopcroft.rkyv's actual content needs a real reader: these tests
chain a second subprocess, prototext's own `score --type <NAME>`
subcommand, a real consumer of hopcroft.rkyv that fails with "type
'<NAME>' not found in scoring graph" if the name is absent from the
compiled graph -- the exact failure mode originally reported.
"""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path

from reproto.tests.test_descriptor_package_rewrite import _run, _setup_fixtures

_OUTER_PAYLOAD = b"\x0a\x03\x0a\x01x"

# A MessageSet's synthesized `Item` group (spec 0108): group(1) {
# type_id: int32(2) = 1000, message: bytes(3) = "" }.  message_set_wire
# format messages have no other declared fields, so this is the whole
# top-level payload.
_ITEM_PAYLOAD = bytes([0x0B, 0x10, 0xE8, 0x07, 0x1A, 0x00, 0x0C])


def _prototext_bin() -> str:
    path = shutil.which("prototext")
    assert path is not None, (
        "prototext must be built (cargo build --release) and on PATH"
    )
    return path


def _score(
    db_path: Path, type_name: str, payload: bytes, tmp_path: Path
) -> subprocess.CompletedProcess[str]:
    payload_path = tmp_path / f"{type_name.replace('.', '_')}.bin"
    payload_path.write_bytes(payload)
    return subprocess.run(
        [
            _prototext_bin(), "--descriptor-set", str(db_path),
            "score", "--type", type_name, "--assume-binary", str(payload_path),
        ],
        capture_output=True, text=True,
    )


def test_G1_renamed_name_is_a_scoring_graph_root_unrenamed_is_not(tmp_path: Path) -> None:
    """hopcroft.rkyv's node names track .desc's renamed names (spec 0166
    G1): scoring by the renamed name succeeds, by the input's fails."""
    inputs = _setup_fixtures(tmp_path)
    db_path = tmp_path / "schema.desc"
    result = _run(inputs, db_path, tmp_path / "out", "--target", "protoc")
    assert result.returncode == 0, f"reproto failed:\n{result.stderr}"

    renamed = _score(db_path, "google.protobuf.Outer", _OUTER_PAYLOAD, tmp_path)
    assert renamed.returncode == 0, f"scoring google.protobuf.Outer failed:\n{renamed.stderr}"
    original = _score(db_path, "vendor.Outer", _OUTER_PAYLOAD, tmp_path)
    assert original.returncode != 0, "vendor.Outer must not be a scoring-graph root"
    assert "not found in scoring graph" in original.stderr


def test_G3_keep_scoring_graph_stays_unrenamed(tmp_path: Path) -> None:
    """--target keep: hopcroft.rkyv stays in the original namespace too,
    matching .desc (spec 0166 G3) -- the inverse of G1."""
    inputs = _setup_fixtures(tmp_path)
    db_path = tmp_path / "schema.desc"
    result = _run(inputs, db_path, tmp_path / "out")
    assert result.returncode == 0, f"reproto failed:\n{result.stderr}"

    original = _score(db_path, "vendor.Outer", _OUTER_PAYLOAD, tmp_path)
    assert original.returncode == 0, f"scoring vendor.Outer failed:\n{original.stderr}"
    renamed = _score(db_path, "google.protobuf.Outer", _OUTER_PAYLOAD, tmp_path)
    assert renamed.returncode != 0, "google.protobuf.Outer must not be a scoring-graph root"
    assert "not found in scoring graph" in renamed.stderr


def test_regression_message_set_item_lands_in_renamed_namespace(tmp_path: Path) -> None:
    """A MessageSet-wire-format message's synthesized `Item` sub-node
    (spec 0108) is threaded through the same renaming as its parent
    (spec 0166 S3): a wire payload matching the synthesized
    Item{type_id, message} shape scores successfully under the renamed
    name, with real match credit (not blind-group-skip)."""
    inputs = _setup_fixtures(tmp_path)
    db_path = tmp_path / "schema.desc"
    result = _run(inputs, db_path, tmp_path / "out", "--target", "protoc")
    assert result.returncode == 0, f"reproto failed:\n{result.stderr}"

    renamed = _score(db_path, "google.protobuf.ItemSet", _ITEM_PAYLOAD, tmp_path)
    assert renamed.returncode == 0, f"scoring google.protobuf.ItemSet failed:\n{renamed.stderr}"
    assert "matches: 3" in renamed.stdout, (
        f"expected the synthesized Item group/type_id/message to all "
        f"register as matches:\n{renamed.stdout}"
    )
