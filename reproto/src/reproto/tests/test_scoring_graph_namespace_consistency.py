# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Tests for hopcroft.rkyv / .desc name consistency (spec 0166).

Regression tests for a bug where a variant's namespace_rewrites rules
were applied to .desc's names at render time (spec 0159) but not to
hopcroft.rkyv's names, which were read directly off ctx.pool, populated
before any rewriting exists. A schema-db consumer scoring by .desc's
(rewritten) name then got "not found in scoring graph" for types that
were, in fact, present in the compiled graph under their old name.

Verifying hopcroft.rkyv's actual content needs a real reader. reproto's
own test harness drives the CLI via subprocess.run
(test_variant_package_rewrite.py's _run), so a same-process Python
monkeypatch on prototext_graph_lib.build_graph would not reach the
subprocess. Instead, these tests chain a second subprocess: prototext's
own `score --type <NAME>` subcommand, a real consumer of hopcroft.rkyv
that fails with "type '<NAME>' not found in scoring graph" if the name
is absent from the compiled graph -- the exact failure mode originally
reported.
"""

from __future__ import annotations

import shutil
import subprocess
import textwrap
from pathlib import Path

from reproto.tests.test_variant_package_rewrite import (
    _REQUIRED_OPTIONS_MESSAGES,
    _compile,
    _run,
    _setup_fixtures,
    _write_proto,
    _write_variant,
)

# Outer{ inner: Inner{ value: "x" } } wire bytes -- name-agnostic, valid
# scoring input regardless of which namespace Outer/Inner are looked up
# under.
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


def test_G1_rewritten_name_is_a_scoring_graph_root_unrewritten_is_not(
    tmp_path: Path,
) -> None:
    """hopcroft.rkyv's node names track .desc's rewritten names (spec
    0166 G1): scoring by the variant-rewritten name succeeds, scoring by
    the original (pre-rewrite) name fails."""
    schema_pb, client_pb = _setup_fixtures(tmp_path)
    variant = _write_variant(tmp_path)
    db_path = tmp_path / "schema.desc"
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    result = _run(variant, db_path, out_dir, schema_pb, client_pb)
    assert result.returncode == 0, f"reproto failed:\n{result.stderr}"

    rewritten = _score(db_path, "canonical.Outer", _OUTER_PAYLOAD, tmp_path)
    assert rewritten.returncode == 0, f"scoring canonical.Outer failed:\n{rewritten.stderr}"

    original = _score(db_path, "proto2.Outer", _OUTER_PAYLOAD, tmp_path)
    assert original.returncode != 0, "proto2.Outer must not be a scoring-graph root"
    assert "not found in scoring graph" in original.stderr


def test_G3_keep_descriptor_path_scoring_graph_stays_unrewritten(
    tmp_path: Path,
) -> None:
    """--keep-descriptor-path: hopcroft.rkyv stays in the original
    namespace too, matching .desc (spec 0166 G3) -- the inverse of G1."""
    schema_pb, client_pb = _setup_fixtures(tmp_path)
    variant = _write_variant(tmp_path)
    db_path = tmp_path / "schema.desc"
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    result = _run(
        variant, db_path, out_dir, schema_pb, client_pb,
        extra_args=["--keep-descriptor-path"],
    )
    assert result.returncode == 0, f"reproto failed:\n{result.stderr}"

    original = _score(db_path, "proto2.Outer", _OUTER_PAYLOAD, tmp_path)
    assert original.returncode == 0, f"scoring proto2.Outer failed:\n{original.stderr}"

    rewritten = _score(db_path, "canonical.Outer", _OUTER_PAYLOAD, tmp_path)
    assert rewritten.returncode != 0, "canonical.Outer must not be a scoring-graph root"
    assert "not found in scoring graph" in rewritten.stderr


def test_regression_message_set_item_lands_in_rewritten_namespace(
    tmp_path: Path,
) -> None:
    """A MessageSet-wire-format message's synthesized `Item` sub-node
    (spec 0108) is threaded through the same namespace rewrite as its
    parent (spec 0166 S3): a wire payload matching the synthesized
    Item{type_id, message} shape scores successfully under the
    rewritten name, with real match credit (not blind-group-skip)."""
    src_dir = tmp_path / "src"
    schema_content = textwrap.dedent("""\
        syntax = "proto2";
        package proto2;
        message ItemSet {
          option message_set_wire_format = true;
        }
    """) + _REQUIRED_OPTIONS_MESSAGES + "\n"
    _write_proto(src_dir / "legacy" / "proto" / "schema.proto", schema_content)

    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    schema_pb = _compile(src_dir, pb_dir, "legacy/proto/schema.proto")

    variant = _write_variant(tmp_path)
    db_path = tmp_path / "schema.desc"
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    result = _run(variant, db_path, out_dir, schema_pb)
    assert result.returncode == 0, f"reproto failed:\n{result.stderr}"

    rewritten = _score(db_path, "canonical.ItemSet", _ITEM_PAYLOAD, tmp_path)
    assert rewritten.returncode == 0, f"scoring canonical.ItemSet failed:\n{rewritten.stderr}"
    assert "matches: 3" in rewritten.stdout, (
        f"expected the synthesized Item group/type_id/message to all "
        f"register as matches:\n{rewritten.stdout}"
    )
