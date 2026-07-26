# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Tests for open-enum range emission, end to end (spec 0176).

An *open* enum (proto3, or editions with `features.enum_type = OPEN`)
accepts every 32-bit value: an unrecognized number is preserved on
round-trip and is exactly what a newer sender emits to an older reader.
Emitting a `[min, max]` range for one therefore vetoed a blob that its
own true schema fully accepts. A *closed* enum (proto2, or editions with
CLOSED) does reject unrecognized numbers, so its range must survive.

These assertions run through the shipped `prototext` binary. When this
file was written that was load-bearing: `ScoringOpts::default()` had
`strict_ranges: false` while the CLI computed `strict_ranges:
!relax_ranges` from a bare flag and so defaulted to strict, meaning a
`score_all`-level assertion would have passed vacuously. Spec 0178 has
since deleted the knob, so there is only one behavior -- but the binary
remains the right level, because it is what a user sees.

The proto2 case below is the control that keeps the proto3 case from
passing for the wrong reason. Spec 0178 sharpened it: both enums now take
the same non-vetoing path and differ only in the `out_of_range` counter,
so the control proves the closed enum's range is still *emitted and
consulted* rather than merely that something somewhere still vetoes.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import textwrap
from pathlib import Path

# Paint{ color: 99 } -- field 1, varint, a number no declared value has.
_UNKNOWN_VALUE_PAYLOAD = b"\x08\x63"


def _prototext_bin() -> str:
    path = shutil.which("prototext")
    assert path is not None, (
        "prototext must be built (cargo build --release) and on PATH"
    )
    return path


def _build_db(tmp_path: Path, name: str, proto: str) -> Path:
    """Compile `proto` and turn it into a schema DB (.desc + hopcroft.rkyv)."""
    src_dir = tmp_path / name / "src"
    src_dir.mkdir(parents=True)
    (src_dir / f"{name}.proto").write_text(textwrap.dedent(proto))

    pb_path = tmp_path / name / f"{name}.pb"
    result = subprocess.run(
        ["protoc", f"-I{src_dir}", f"--descriptor_set_out={pb_path}",
         f"{name}.proto"],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, f"protoc failed: {result.stderr}"

    src_path = str(Path(__file__).parent.parent.parent)
    pythonpath_parts = [src_path]
    if existing := os.environ.get("PYTHONPATH"):
        pythonpath_parts.append(existing)
    env = {**os.environ, "PYTHONPATH": os.pathsep.join(pythonpath_parts)}
    env.pop("REPROTO_VARIANT", None)

    db_path = tmp_path / name / f"{name}.desc"
    result = subprocess.run(
        [
            sys.executable, "-m", "reproto.cli",
            "--use-variant", "descriptor",
            f"--proto-out={tmp_path / name / 'out'}",
            f"--schema-db-out={db_path}",
            str(pb_path),
        ],
        capture_output=True, text=True, env=env,
    )
    assert result.returncode == 0, f"reproto failed:\n{result.stderr}"
    return db_path


def _payload(tmp_path: Path) -> Path:
    path = tmp_path / "paint.bin"
    path.write_bytes(_UNKNOWN_VALUE_PAYLOAD)
    return path


def _prototext(db_path: Path, payload: Path, *args: str) -> str:
    result = subprocess.run(
        [_prototext_bin(), "--descriptor-set", str(db_path), *args,
         "--assume-binary", str(payload)],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, f"prototext failed:\n{result.stderr}"
    return result.stdout


_OPEN_ENUM_PROTO = """\
    syntax = "proto3";
    package openenum;
    enum Color {
      COLOR_UNSPECIFIED = 0;
      COLOR_RED = 1;
      COLOR_GREEN = 2;
    }
    message Paint {
      Color color = 1;
    }
"""

_CLOSED_ENUM_PROTO = """\
    syntax = "proto2";
    package closedenum;
    enum Color {
      COLOR_UNSPECIFIED = 0;
      COLOR_RED = 1;
      COLOR_GREEN = 2;
    }
    message Paint {
      optional Color color = 1;
    }
"""


def test_E1_open_enum_unknown_value_is_not_vetoed(tmp_path: Path) -> None:
    """A proto3 enum value outside the declared set scores as a plain
    match against its own true schema -- not merely un-vetoed, but
    un-penalized, because an open enum has no range to be outside of
    (spec 0176 S1)."""
    db_path = _build_db(tmp_path, "openenum", _OPEN_ENUM_PROTO)
    out = _prototext(db_path, _payload(tmp_path), "score", "--type",
                     "openenum.Paint")
    assert "vetoed" not in out, f"open enum must not veto:\n{out}"
    assert "matches: 1" in out, out
    assert "out_of_range: 0" in out, f"an open enum has no range:\n{out}"


def test_E2_open_enum_true_type_wins_the_ranking(tmp_path: Path) -> None:
    """The same blob's true FQDN is a surviving `list-schemas` candidate
    rather than being eliminated before ranking (spec 0176 S1)."""
    db_path = _build_db(tmp_path, "openenum", _OPEN_ENUM_PROTO)
    out = _prototext(db_path, _payload(tmp_path), "list-schemas", "--top", "5")
    assert "type: openenum.Paint" in out, out


def test_E3_closed_enum_unknown_value_is_penalized(tmp_path: Path) -> None:
    """Control (spec 0176 N1, spec 0178 S2): the identical bytes against a
    proto2 *closed* enum are charged `out_of_range`, so E1/E2 are not
    passing merely because the CLI stopped checking ranges at all.

    Before spec 0178 this asserted a veto. The charge is now a penalty --
    an unrecognized closed-enum number goes to the unknown-field set
    rather than failing the parse -- so the candidate survives ranking.
    """
    db_path = _build_db(tmp_path, "closedenum", _CLOSED_ENUM_PROTO)
    payload = _payload(tmp_path)
    out = _prototext(db_path, payload, "score", "--type", "closedenum.Paint")
    assert "vetoed" not in out, f"out of range is not impossible:\n{out}"
    assert "out_of_range: 1" in out, f"closed enum keeps its range:\n{out}"
    assert "non_canonical: 0" in out, f"the encoding itself is fine:\n{out}"

    ranked = _prototext(db_path, payload, "list-schemas", "--top", "5")
    assert "closedenum.Paint" in ranked, ranked
