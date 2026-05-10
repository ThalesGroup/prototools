# SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Tests for the --emit-scoring-graphs option (spec 0045).

Each test compiles fixture .proto files with protoc, then runs the reproto CLI
with --emit-scoring-graphs and inspects the resulting YAML files.
"""

from __future__ import annotations

import os
import sys
import subprocess
from pathlib import Path
from typing import Any

import yaml

from reproto.tests.conftest import compile_proto, FIXTURES_DIR


def _load_yaml(path: Path) -> dict[str, Any]:
    data = yaml.safe_load(path.read_text())
    assert isinstance(data, dict)
    return data


# ---------------------------------------------------------------------------
# Helper
# ---------------------------------------------------------------------------

def _run_reproto(
    pb_files: list[Path],
    out_dir: Path,
    extra_args: list[str] | None = None,
    include_dirs: list[Path] | None = None,
) -> subprocess.CompletedProcess[str]:
    src_path = str(Path(__file__).parent.parent.parent)
    pythonpath_parts = [src_path]
    if existing := os.environ.get("PYTHONPATH"):
        pythonpath_parts.append(existing)
    env = {**os.environ, "PYTHONPATH": os.pathsep.join(pythonpath_parts)}
    env.pop("REPROTO_VARIANT", None)

    dirs = include_dirs if include_dirs is not None else [FIXTURES_DIR]
    include_flags = [f"-I{d}" for d in dirs]

    cmd = [
        sys.executable, "-m", "reproto.cli",
        "--use-variant", "descriptor",
        *include_flags,
        f"--output-root={out_dir}",
        "--emit-scoring-graphs",
    ]
    if extra_args:
        cmd.extend(extra_args)
    cmd.extend(str(p) for p in pb_files)

    return subprocess.run(cmd, capture_output=True, text=True, env=env)


# ---------------------------------------------------------------------------
# TC-1: Basic emission — field kinds and child FQDNs
# ---------------------------------------------------------------------------

def test_TC1_basic_emission(tmp_path: Path) -> None:
    """field_comprehensive.proto: YAML is written; field kinds and child FQDNs are correct."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    (pb,) = compile_proto(pb_dir, "field_comprehensive.proto")
    result = _run_reproto([pb], out_dir)
    assert result.returncode == 0, result.stderr

    yaml_path = out_dir / "field_comprehensive.yaml"
    assert yaml_path.exists(), "YAML file must be written alongside .proto output"

    data = _load_yaml(yaml_path)
    messages = data["messages"]

    # PrimitiveTypes covers most scalar kinds (field numbers per fixture)
    prim = {f["number"]: f for f in messages["test.field.PrimitiveTypes"]["fields"]}
    assert prim[1]["kind"] == "VARINT"     # int32
    assert prim[2]["kind"] == "VARINT"     # int64
    assert prim[3]["kind"] == "VARINT"     # uint32
    assert prim[4]["kind"] == "VARINT"     # uint64
    assert prim[5]["kind"] == "VARINT"     # sint32
    assert prim[6]["kind"] == "VARINT"     # sint64
    assert prim[7]["kind"] == "I32"        # fixed32
    assert prim[8]["kind"] == "I64"        # fixed64
    assert prim[9]["kind"] == "I32"        # sfixed32
    assert prim[10]["kind"] == "I64"       # sfixed64
    assert prim[11]["kind"] == "I32"       # float
    assert prim[12]["kind"] == "I64"       # double
    assert prim[13]["kind"] == "VARINT"    # bool
    assert prim[14]["kind"] == "LEN_STRING"
    assert prim[15]["kind"] == "LEN_BYTES"

    # ComplexTypes: message field must have kind LEN_MSG and correct child
    complex_fields = {f["number"]: f for f in messages["test.field.ComplexTypes"]["fields"]}
    msg_field = complex_fields[1]
    assert msg_field["kind"] == "LEN_MSG"
    assert msg_field["child"] == "test.field.NestedMessage"
    assert "child" not in complex_fields[4]  # enum field — no child

    # No child on scalar fields
    for f in prim.values():
        assert "child" not in f

    # label: required/repeated are emitted; optional is the default (omitted)
    assert prim[1].get("label") == "required"   # req_int32  (required)
    assert prim[4].get("label") == "required"   # req_uint64 (required)
    assert prim[3].get("label") == "repeated"   # rep_uint32 (repeated)
    assert "label" not in prim[2]               # opt_int64  (optional — default, omitted)

    # Fields listed in ascending number order
    field_numbers = [f["number"] for f in messages["test.field.PrimitiveTypes"]["fields"]]
    assert field_numbers == sorted(field_numbers)

    # entries: all top-level messages in the file (sorted, full FQDNs)
    entries = data["entries"]
    assert "test.field.PrimitiveTypes" in entries
    assert "test.field.ComplexTypes" in entries
    assert "test.field.Outer" in entries
    # entries must be sorted
    assert entries == sorted(entries)
    # nested types must NOT appear in entries
    assert "test.field.Outer.Middle" not in entries
    assert "test.field.Outer.Middle.Inner" not in entries


# ---------------------------------------------------------------------------
# TC-2: Nested message types appear as top-level entries
# ---------------------------------------------------------------------------

def test_TC2_nested_message_types(tmp_path: Path) -> None:
    """Outer, Outer.Middle, Outer.Middle.Inner all appear as top-level keys."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    (pb,) = compile_proto(pb_dir, "field_comprehensive.proto")
    result = _run_reproto([pb], out_dir)
    assert result.returncode == 0, result.stderr

    data = _load_yaml(out_dir / "field_comprehensive.yaml")
    messages = data["messages"]

    assert "test.field.Outer" in messages
    assert "test.field.Outer.Middle" in messages
    assert "test.field.Outer.Middle.Inner" in messages


# ---------------------------------------------------------------------------
# TC-3: Cross-file reference
# ---------------------------------------------------------------------------

def test_TC3_cross_file_reference(tmp_path: Path) -> None:
    """address_book.yaml has child FQDN for PhoneNumber but not the type definition."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    phone_pb, ab_pb = compile_proto(pb_dir, "phone_number.proto", "address_book.proto")
    result = _run_reproto([phone_pb, ab_pb], out_dir)
    assert result.returncode == 0, result.stderr

    ab_data = _load_yaml(out_dir / "address_book.yaml")
    ab_messages = ab_data["messages"]

    # Person.phones (field 4) is repeated PhoneNumber — LEN_MSG with child
    person_fields = {f["number"]: f for f in ab_messages["tutorial.Person"]["fields"]}
    assert person_fields[4]["kind"] == "LEN_MSG"
    assert person_fields[4]["child"] == "tutorial.PhoneNumber"

    # PhoneNumber must NOT be defined in address_book.yaml
    assert "tutorial.PhoneNumber" not in ab_messages

    # entries in address_book.yaml: only types defined in that file
    assert "tutorial.Person" in ab_data["entries"]
    assert "tutorial.PhoneNumber" not in ab_data["entries"]

    # PhoneNumber must be defined in phone_number.yaml
    phone_data = _load_yaml(out_dir / "phone_number.yaml")
    assert "tutorial.PhoneNumber" in phone_data["messages"]
    assert "tutorial.PhoneNumber" in phone_data["entries"]


# ---------------------------------------------------------------------------
# TC-4: Proto3 implicit packing
# ---------------------------------------------------------------------------

def test_TC4_proto3_implicit_packing(tmp_path: Path) -> None:
    """default_int (no [packed] option) -> LEN_PACKED; explicit_false -> VARINT."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    (pb,) = compile_proto(pb_dir, "packed_proto3.proto")
    result = _run_reproto([pb], out_dir)
    assert result.returncode == 0, result.stderr

    data = _load_yaml(out_dir / "packed_proto3.yaml")
    fields = {f["number"]: f for f in data["messages"]["mockup.Packed"]["fields"]}

    assert fields[1]["kind"] == "LEN_PACKED"  # default_int: implicitly packed in proto3
    assert fields[2]["kind"] == "LEN_PACKED"  # explicit_true
    assert fields[3]["kind"] == "VARINT"      # explicit_false


# ---------------------------------------------------------------------------
# TC-5: GROUP kind and child FQDN
# ---------------------------------------------------------------------------

def test_TC5_group_child_fqdn(tmp_path: Path) -> None:
    """GroupTest fields appear with kind GROUP and a child FQDN."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    (pb,) = compile_proto(pb_dir, "field_comprehensive.proto")
    result = _run_reproto([pb], out_dir)
    assert result.returncode == 0, result.stderr

    data = _load_yaml(out_dir / "field_comprehensive.yaml")
    fields = {f["number"]: f for f in data["messages"]["test.field.GroupTest"]["fields"]}

    # field 1: repeated group RepeatedGroup
    assert fields[1]["kind"] == "GROUP"
    assert fields[1]["child"] == "test.field.GroupTest.RepeatedGroup"

    # field 3: optional group OptionalGroup
    assert fields[3]["kind"] == "GROUP"
    assert fields[3]["child"] == "test.field.GroupTest.OptionalGroup"
