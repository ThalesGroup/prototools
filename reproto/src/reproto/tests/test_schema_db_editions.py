# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""A schema DB keeps editions files as editions (spec 0421).

Before it, --schema-db-out converted every editions file to proto2 for
prost-reflect. The conversion is lossy: a delimited message field that
is not a group has no proto2 spelling, so prototext decoded it as a
TYPE_MISMATCH. These tests build a DB from an editions 2023 and an
editions 2024 file, check that it and the .proto files beside it keep
their editions, and decode a message of each through the shipped
prototext binary. --force-proto2-for-editions, and its alias
--prost-workaround, still give the proto2 form for older readers.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import textwrap
from pathlib import Path

from google.protobuf.descriptor_pb2 import Edition, FileDescriptorSet

_ED2023_PROTO = """\
    edition = "2023";
    package ed2023;
    option features.repeated_field_encoding = EXPANDED;
    message Inner { int32 a = 1; }
    message Fields {
      repeated int32 expanded = 1;
      repeated int32 packed = 2 [features.repeated_field_encoding = PACKED];
      Inner delimited = 3 [features.message_encoding = DELIMITED];
      int32 implicit = 4 [features.field_presence = IMPLICIT];
    }
"""

_ED2024_PROTO = """\
    edition = "2024";
    package ed2024;
    import "ed2023.proto";
    option features.message_encoding = DELIMITED;
    message Holder {
      ed2023.Inner delimited = 1;
      repeated sint64 packed = 2;
    }
"""

# Fields{ expanded: [1, 2], packed: [3, 4], delimited { a: 5 }, implicit: 6 }
# with field 3 as a group (start tag 0x1b, end tag 0x1c).
_FIELDS_PAYLOAD = b"\x08\x01\x08\x02\x12\x02\x03\x04\x1b\x08\x05\x1c\x20\x06"
# Holder{ delimited { a: 7 }, packed: [1, -1] }, field 1 as a group.
_HOLDER_PAYLOAD = b"\x0b\x08\x07\x0c\x12\x02\x02\x01"


def _compile(tmp_path: Path) -> Path:
    src_dir = tmp_path / "src"
    src_dir.mkdir()
    (src_dir / "ed2023.proto").write_text(textwrap.dedent(_ED2023_PROTO))
    (src_dir / "ed2024.proto").write_text(textwrap.dedent(_ED2024_PROTO))
    pb_path = tmp_path / "in.pb"
    result = subprocess.run(
        ["protoc", f"-I{src_dir}", f"--descriptor_set_out={pb_path}",
         "ed2023.proto", "ed2024.proto"],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, f"protoc failed: {result.stderr}"
    return pb_path


def _build_db(tmp_path: Path, name: str, *flags: str) -> Path:
    """Run reproto on the two files; return the DB's directory."""
    pb_path = tmp_path / "in.pb"
    if not pb_path.exists():
        _compile(tmp_path)

    src_path = str(Path(__file__).parent.parent.parent)
    pythonpath_parts = [src_path]
    if existing := os.environ.get("PYTHONPATH"):
        pythonpath_parts.append(existing)
    env = {**os.environ, "PYTHONPATH": os.pathsep.join(pythonpath_parts)}
    env.pop("REPROTO_VARIANT", None)

    out = tmp_path / name
    result = subprocess.run(
        [
            sys.executable, "-m", "reproto.cli",
            "--use-variant", "descriptor",
            *flags,
            f"--proto-out={out / 'proto'}",
            f"--schema-db-out={out / 'db.desc'}",
            str(pb_path),
        ],
        capture_output=True, text=True, env=env,
    )
    assert result.returncode == 0, f"reproto failed:\n{result.stderr}"
    return out


def _syntaxes(out: Path) -> dict[str, tuple[str, int]]:
    fds = FileDescriptorSet()
    fds.MergeFromString((out / "db.desc").read_bytes())
    return {f.name: (f.syntax or "proto2", f.edition) for f in fds.file}


def _first_line(proto: Path) -> str:
    return next(
        line for line in proto.read_text().splitlines()
        if line.startswith(("edition", "syntax"))
    )


def _decode(out: Path, type_name: str, payload: bytes) -> str:
    prototext = shutil.which("prototext")
    assert prototext is not None, (
        "prototext must be built (cargo build --release) and on PATH"
    )
    blob = out / f"{type_name}.bin"
    blob.write_bytes(payload)
    result = subprocess.run(
        [prototext, "--descriptor-set", str(out / "db.desc"), "decode",
         "--type", type_name, "--assume-binary", str(blob)],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, f"prototext failed:\n{result.stderr}"
    return result.stdout


def test_S1_schema_db_keeps_editions(tmp_path: Path) -> None:
    """The DB and the .proto files beside it keep each file's edition."""
    out = _build_db(tmp_path, "editions")
    syntaxes = _syntaxes(out)
    assert syntaxes["ed2023.proto"] == ("editions", Edition.EDITION_2023)
    assert syntaxes["ed2024.proto"] == ("editions", Edition.EDITION_2024)
    assert _first_line(out / "proto" / "ed2023.proto") == 'edition = "2023";'
    assert _first_line(out / "proto" / "ed2024.proto") == 'edition = "2024";'


def test_S1_prototext_decodes_delimited_and_packed(tmp_path: Path) -> None:
    """Delimited messages decode as their type, packed and expanded
    fields as declared: nothing reads as a mismatch."""
    out = _build_db(tmp_path, "editions")

    fields = _decode(out, "ed2023.Fields", _FIELDS_PAYLOAD)
    assert "TYPE_MISMATCH" not in fields, fields
    assert "delimited {  #@ group; Inner = 3" in fields, fields
    assert "expanded: 2  #@ repeated int32 = 1" in fields, fields
    assert "packed: 3  #@ repeated int32 [packed=true] = 2" in fields, fields
    assert "implicit: 6" in fields, fields

    holder = _decode(out, "ed2024.Holder", _HOLDER_PAYLOAD)
    assert "TYPE_MISMATCH" not in holder, holder
    assert "delimited {  #@ group; Inner = 1" in holder, holder
    assert "packed: -1  #@ repeated sint64 [packed=true] = 2" in holder, holder


def test_S2_flag_still_gives_proto2(tmp_path: Path) -> None:
    """--force-proto2-for-editions applies to the DB, as to the rest of
    the output; its alias --prost-workaround gives the same DB (S3)."""
    flagged = _build_db(tmp_path, "flag", "--force-proto2-for-editions")
    assert _syntaxes(flagged) == {
        "ed2023.proto": ("proto2", Edition.EDITION_UNKNOWN),
        "ed2024.proto": ("proto2", Edition.EDITION_UNKNOWN),
    }
    assert _first_line(flagged / "proto" / "ed2024.proto") == 'syntax = "proto2";'

    alias = _build_db(tmp_path, "alias", "--prost-workaround")
    assert (alias / "db.desc").read_bytes() == (flagged / "db.desc").read_bytes()
