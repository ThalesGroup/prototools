# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Tests for spec 0243 — a blob is a root, and the defaults fill the rest.

Three shortcuts, exercised through the real CLI:

- --schema-db-out FILE.desc implies -O FILE-stem/proto (S1), which
  --no-proto-out suppresses (S2);
- -I accepts a blob file and reads it as the descriptors inside it (S5);
- no DESCRIPTOR_FILES means '.' (S13).
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

from google.protobuf import descriptor_pb2

FIXTURES = Path(__file__).parent / "fixtures"


def _run(*args: str) -> subprocess.CompletedProcess[str]:
    src_path = str(Path(__file__).parent.parent.parent)
    pythonpath_parts = [src_path]
    if existing := os.environ.get("PYTHONPATH"):
        pythonpath_parts.append(existing)
    env = {**os.environ, "PYTHONPATH": os.pathsep.join(pythonpath_parts)}
    env.pop("REPROTO_VARIANT", None)
    # --use-variant descriptor on every run: the members below import
    # nothing, but reproto still wants descriptor.proto in the input set,
    # and supplying it as a third blob member would put a 6 KB descriptor
    # in the middle of tests that are about paths and defaults.
    return subprocess.run(
        [sys.executable, "-m", "reproto.cli", "--use-variant=descriptor", *args],
        capture_output=True, text=True, env=env,
    )


def _fdp(name: str, package: str, message: str) -> descriptor_pb2.FileDescriptorProto:
    fdp = descriptor_pb2.FileDescriptorProto()
    fdp.name = name
    fdp.syntax = "proto3"
    fdp.package = package
    msg = fdp.message_type.add()
    msg.name = message
    field = msg.field.add()
    field.name = "value"
    field.number = 1
    field.type = descriptor_pb2.FieldDescriptorProto.TYPE_STRING
    field.label = descriptor_pb2.FieldDescriptorProto.LABEL_OPTIONAL
    return fdp


def _members() -> list[descriptor_pb2.FileDescriptorProto]:
    """Two FDPs that import nothing, built here rather than compiled.

    Self-contained on purpose: a fixture with dependencies would make
    every test below also a test of dependency resolution. Two of them,
    because with one neither member selection nor pruning could be
    observed.
    """
    return [
        _fdp("first.proto", "one", "First"),
        _fdp("extra/second.proto", "two", "Second"),
    ]


def _write_blob(path: Path) -> Path:
    """Concatenate the members with filler around them.

    A NUL between records is the shape the scanner is specified against
    (a record ends where the next one's field-1 tag begins); the header
    and footer make the point that a blob's descriptors need not start
    at offset zero.
    """
    body = b"\x00".join(fdp.SerializeToString() for fdp in _members())
    path.write_bytes(b"HEADER\x00" + body + b"\x00FOOTER")
    return path


def _write_dir(root: Path) -> Path:
    """The same members, extracted — what protoscan --proto_out writes."""
    for fdp in _members():
        out = root / Path(fdp.name).with_suffix(".pb")
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_bytes(fdp.SerializeToString())
    return root


def _protos(root: Path) -> set[str]:
    return {
        str(p.relative_to(root))
        for p in root.rglob("*.proto")
    }


# --- S1/S2: the schema DB names the proto directory ------------------------

def test_schema_db_out_implies_proto_out(tmp_path: Path) -> None:
    blob = _write_blob(tmp_path / "blob")
    result = _run(f"--schema-db-out={tmp_path / 'db.desc'}", f"-I{blob}")
    assert result.returncode == 0, result.stderr
    assert _protos(tmp_path / "db" / "proto") == {
        "first.proto", "extra/second.proto",
    }


def test_no_proto_out_suppresses_the_implied_proto_out(tmp_path: Path) -> None:
    blob = _write_blob(tmp_path / "blob")
    result = _run(
        f"--schema-db-out={tmp_path / 'db.desc'}", "--no-proto-out", f"-I{blob}")
    assert result.returncode == 0, result.stderr
    assert (tmp_path / "db.desc").exists()
    assert not (tmp_path / "db" / "proto").exists()


def test_no_proto_out_with_explicit_proto_out_is_a_usage_error(
    tmp_path: Path,
) -> None:
    result = _run(
        f"--schema-db-out={tmp_path / 'db.desc'}",
        "--no-proto-out",
        f"-O{tmp_path / 'out'}",
        str(tmp_path / "missing.pb"),
    )
    assert result.returncode != 0
    assert "--no-proto-out contradicts" in result.stderr


# --- S5/S6: a blob is a root ----------------------------------------------

def test_desc_root_may_be_a_blob(tmp_path: Path) -> None:
    """A blob root produces exactly what the extracted directory does."""
    blob = _write_blob(tmp_path / "blob")
    extracted = _write_dir(tmp_path / "extracted")

    from_blob = tmp_path / "from_blob"
    from_dir = tmp_path / "from_dir"
    assert _run(f"-O{from_blob}", f"-I{blob}").returncode == 0
    assert _run(f"-O{from_dir}", f"-I{extracted}").returncode == 0

    assert _protos(from_blob) == _protos(from_dir)
    for name in _protos(from_blob):
        assert (from_blob / name).read_text() == (from_dir / name).read_text()


def test_blob_member_paths_are_the_fdp_names(tmp_path: Path) -> None:
    """-p matches a blob member on its root-relative path, as for a directory."""
    blob = _write_blob(tmp_path / "blob")
    out = tmp_path / "out"
    result = _run(f"-O{out}", f"-I{blob}", "-p", "extra/second.pb")
    assert result.returncode == 0, result.stderr
    assert _protos(out) == {"first.proto"}


def test_a_named_blob_member_loads_alone(tmp_path: Path) -> None:
    blob = _write_blob(tmp_path / "blob")
    out = tmp_path / "out"
    result = _run(f"-O{out}", f"-I{blob}", "extra/second.pb")
    assert result.returncode == 0, result.stderr
    assert _protos(out) == {"extra/second.proto"}


def test_blob_with_no_descriptors_is_an_error(tmp_path: Path) -> None:
    junk = tmp_path / "junk.bin"
    junk.write_bytes(bytes(range(256)) * 4)
    result = _run(f"-O{tmp_path / 'out'}", f"-I{junk}")
    assert result.returncode != 0
    assert "holds no FileDescriptorProto" in result.stderr


def test_completion_offers_blob_members(tmp_path: Path) -> None:
    from reproto.cli import _complete_blob_members
    blob = _write_blob(tmp_path / "blob")
    assert [item.value for item in _complete_blob_members(blob, "")] == [
        "extra/second.pb", "first.pb",
    ]
    assert [item.value for item in _complete_blob_members(blob, "extra/")] == [
        "extra/second.pb",
    ]


# --- S13: no argument means '.' -------------------------------------------

def test_no_arguments_means_dot(tmp_path: Path) -> None:
    extracted = _write_dir(tmp_path / "extracted")
    implied = tmp_path / "implied"
    spelled = tmp_path / "spelled"
    assert _run(f"-O{implied}", f"-I{extracted}").returncode == 0
    assert _run(f"-O{spelled}", f"-I{extracted}", ".").returncode == 0
    assert _protos(implied) == _protos(spelled) != set()
