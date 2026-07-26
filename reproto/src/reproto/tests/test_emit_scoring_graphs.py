# SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Tests for the --emit-scoring-yaml option (spec 0045).

Each test compiles fixture .proto files with protoc, then runs the reproto CLI
with --emit-scoring-yaml and inspects the resulting YAML files.

TC-6 (spec 0086) covers canonized output paths when a variant rewrites file names.
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
        f"--proto-out={out_dir}",
        "--emit-scoring-yaml",
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
    assert prim[1]["type"] == "int32"      # req_int32
    assert prim[2]["type"] == "uint64"     # opt_int64  (int64 → UINT64 leaf)
    assert prim[3]["type"] == "uint32"     # rep_uint32
    assert prim[4]["type"] == "uint64"     # req_uint64
    assert prim[5]["type"] == "uint32"     # opt_sint32 (sint32 → UINT32 leaf)
    assert prim[6]["type"] == "uint64"     # rep_sint64 (sint64 → UINT64 leaf)
    assert prim[7]["type"] == "float"      # req_fixed32
    assert prim[8]["type"] == "double"     # opt_fixed64
    assert prim[9]["type"] == "float"      # rep_sfixed32
    assert prim[10]["type"] == "double"    # req_sfixed64
    assert prim[11]["type"] == "float"     # opt_float
    assert prim[12]["type"] == "double"    # rep_double
    assert prim[13]["type"] == "bool"      # req_bool
    assert prim[14]["type"] == "string"    # opt_string
    assert prim[15]["type"] == "bytes"     # rep_bytes

    # ComplexTypes: message field must have type message and correct child
    complex_fields = {f["number"]: f for f in messages["test.field.ComplexTypes"]["fields"]}
    msg_field = complex_fields[1]
    assert msg_field["type"] == "message"
    assert msg_field["child"] == "test.field.NestedMessage"

    # req_enum = 4 is a proto2 enum, hence *closed*: a value outside the
    # declared set is genuine evidence against this candidate, so the range is
    # emitted and kept (spec 0176 Finding 2).  Contrast TC-4, where an open
    # proto3 enum has no range at all.
    assert complex_fields[4]["type"] == "enum"
    assert complex_fields[4]["range"] == [0, 2]  # Status: UNKNOWN..INACTIVE
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

    # entries: all non-pruned messages in the file, top-level and nested
    # alike (spec 0140), sorted, full FQDNs
    entries = data["entries"]
    assert "test.field.PrimitiveTypes" in entries
    assert "test.field.ComplexTypes" in entries
    assert "test.field.Outer" in entries
    # entries must be sorted
    assert entries == sorted(entries)
    # nested types MUST appear in entries (spec 0140)
    assert "test.field.Outer.Middle" in entries
    assert "test.field.Outer.Middle.Inner" in entries


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
# TC-2b: A pruned nested message type is excluded from both messages and
# entries (spec 0140 G2 — pruning is now consulted at every nesting level,
# not just the top level).
# ---------------------------------------------------------------------------

def test_TC2b_pruned_nested_message_excluded(tmp_path: Path) -> None:
    """Pruning a nested message excludes it from both messages and entries."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    (pb,) = compile_proto(pb_dir, "field_comprehensive.proto")
    result = _run_reproto(
        [pb], out_dir,
        extra_args=["--prune", "desc:test.field.Outer.Middle"],
    )
    assert result.returncode == 0, result.stderr

    data = _load_yaml(out_dir / "field_comprehensive.yaml")
    messages = data["messages"]
    entries = data["entries"]

    # The pruned nested type and its own nested children are gone.
    assert "test.field.Outer.Middle" not in messages
    assert "test.field.Outer.Middle" not in entries
    assert "test.field.Outer.Middle.Inner" not in messages
    assert "test.field.Outer.Middle.Inner" not in entries

    # The enclosing type is unaffected.
    assert "test.field.Outer" in messages
    assert "test.field.Outer" in entries


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

    # Person.phones (field 4) is repeated PhoneNumber — message with child
    person_fields = {f["number"]: f for f in ab_messages["tutorial.Person"]["fields"]}
    assert person_fields[4]["type"] == "message"
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
# TC-4: the [packed] option does not affect the emitted kind
# ---------------------------------------------------------------------------

def test_TC4_packed_option_is_not_consulted(tmp_path: Path) -> None:
    """Every repeated scalar emits its element kind, whatever [packed] says.

    Spec 0175: a repeated scalar has two legal wire encodings and a reader must
    accept both, so the option carries no information a scorer may act on.
    Emitting the old `LEN_PACKED` collapse traded away the one thing the scorer
    does need — the element type — for a bit it then had to ignore.  All three
    fields below differ only in their [packed] option, so all three must agree.
    """
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    (pb,) = compile_proto(pb_dir, "packed_proto3.proto")
    result = _run_reproto([pb], out_dir)
    assert result.returncode == 0, result.stderr

    data = _load_yaml(out_dir / "packed_proto3.yaml")
    fields = {f["number"]: f for f in data["messages"]["mockup.Packed"]["fields"]}

    assert fields[1]["type"] == "int32"   # default_int: implicitly packed in proto3
    assert fields[2]["type"] == "int32"   # explicit_true
    assert fields[3]["type"] == "int32"   # explicit_false
    assert fields[9]["type"] == "double"  # doubles_def: implicitly packed
    assert fields[10]["type"] == "float"  # floats_true

    # `label: repeated` is what tells the scorer the field is packable; it is
    # emitted independently of [packed] and must be present on all of them.
    for n in (1, 2, 3, 9, 10):
        assert fields[n].get("label") == "repeated", n

    # enums_default = 6 is `repeated Status` in proto3, so the enum is *open*:
    # every 32-bit value is legal and forward-compatible, so it has no range
    # and is emitted as int32 (spec 0176).  `label: repeated` also makes it
    # packable, so this one field pins both specs at once.
    assert fields[6]["type"] == "int32"
    assert "range" not in fields[6]
    assert fields[6].get("label") == "repeated"


# ---------------------------------------------------------------------------
# TC-5: GROUP kind and child FQDN
# ---------------------------------------------------------------------------

def test_TC5_group_child_fqdn(tmp_path: Path) -> None:
    """GroupTest: group field entries use kind MESSAGE (spec 0058); group
    message entries carry kind GROUP at the message level."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    (pb,) = compile_proto(pb_dir, "field_comprehensive.proto")
    result = _run_reproto([pb], out_dir)
    assert result.returncode == 0, result.stderr

    data = _load_yaml(out_dir / "field_comprehensive.yaml")
    messages = data["messages"]
    fields = {f["number"]: f for f in messages["test.field.GroupTest"]["fields"]}

    # field 1: repeated group RepeatedGroup — field type is group (spec 0058)
    assert fields[1]["type"] == "group"
    assert fields[1]["child"] == "test.field.GroupTest.RepeatedGroup"

    # field 3: optional group OptionalGroup — field type is group (spec 0058)
    assert fields[3]["type"] == "group"
    assert fields[3]["child"] == "test.field.GroupTest.OptionalGroup"

    # The group message entries must have kind GROUP at the message level
    assert messages["test.field.GroupTest.RepeatedGroup"]["kind"] == "GROUP"
    assert messages["test.field.GroupTest.OptionalGroup"]["kind"] == "GROUP"


# ---------------------------------------------------------------------------
# TC-9: enum openness under editions
# ---------------------------------------------------------------------------

def test_TC9_editions_closed_enum_keeps_its_range(tmp_path: Path) -> None:
    """Openness comes from `is_closed`, not from the file's syntax.

    `editions_roundtrip.proto` is `edition = "2023"` with
    `features.enum_type = CLOSED`, so `Status` is closed even though the file
    is neither proto2 nor proto3.  This is the case a test on the declaring
    file's `syntax` would get wrong, which is why spec 0176 uses
    `enum_type.is_closed` — it resolves the edition feature.
    """
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    pbs = compile_proto(
        pb_dir,
        "editions_roundtrip.proto",
        "editions_custom_option_dep.proto",
        "weak_import_proto2_dep.proto",
    )
    result = _run_reproto(list(pbs), out_dir)
    assert result.returncode == 0, result.stderr

    data = _load_yaml(out_dir / "editions_roundtrip.yaml")
    fields = {
        f["number"]: f
        for f in data["messages"]["mockup.editions.AllFeatures"]["fields"]
    }

    # status = 7 is `Status`, closed by the file- and enum-level feature.
    assert fields[7]["type"] == "enum"
    assert fields[7]["range"] == [0, 2]  # STATUS_UNKNOWN..STATUS_INACTIVE


# ---------------------------------------------------------------------------
# TC-MS: MessageSet structural recognition (spec 0108, Tier 0)
# ---------------------------------------------------------------------------

def test_TC_MS1_message_set_synthesizes_item_edge(tmp_path: Path) -> None:
    """A message_set_wire_format=true type gets a synthetic field 1 -> Item."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    (pb,) = compile_proto(pb_dir, "message_set_proto2.proto")
    result = _run_reproto([pb], out_dir)
    assert result.returncode == 0, result.stderr

    data = _load_yaml(out_dir / "message_set_proto2.yaml")
    messages = data["messages"]

    ms_fields = messages["mockup.MessageSetMessage"]["fields"]
    assert len(ms_fields) == 1
    field1 = ms_fields[0]
    assert field1["number"] == 1
    assert field1["type"] == "message"
    assert field1["child"] == "mockup.MessageSetMessage.Item"
    assert field1["label"] == "repeated"


def test_TC_MS2_item_node_shape(tmp_path: Path) -> None:
    """The synthesized Item node is a GROUP with type_id (int32=2) and message (bytes=3)."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    (pb,) = compile_proto(pb_dir, "message_set_proto2.proto")
    result = _run_reproto([pb], out_dir)
    assert result.returncode == 0, result.stderr

    data = _load_yaml(out_dir / "message_set_proto2.yaml")
    messages = data["messages"]

    assert "mockup.MessageSetMessage.Item" in messages
    item = messages["mockup.MessageSetMessage.Item"]
    assert item["kind"] == "GROUP"
    item_fields = {f["number"]: f for f in item["fields"]}
    assert item_fields[2]["type"] == "int32"
    assert item_fields[3]["type"] == "bytes"
    assert "child" not in item_fields[2]
    assert "child" not in item_fields[3]


def test_TC_MS3_non_message_set_unaffected(tmp_path: Path) -> None:
    """A plain message (message_set_wire_format unset) is emitted normally."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    (pb,) = compile_proto(pb_dir, "message_set_proto2.proto")
    result = _run_reproto([pb], out_dir)
    assert result.returncode == 0, result.stderr

    data = _load_yaml(out_dir / "message_set_proto2.yaml")
    messages = data["messages"]

    payload_fields = messages["mockup.Payload"]["fields"]
    assert len(payload_fields) == 1
    assert payload_fields[0]["number"] == 1
    assert payload_fields[0]["type"] == "string"
    assert "mockup.Payload.Item" not in messages


def test_TC_MS4_item_node_excluded_from_entries(tmp_path: Path) -> None:
    """The synthesized Item node is a messages key but never a candidate entry (spec 0140 G3)."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    (pb,) = compile_proto(pb_dir, "message_set_proto2.proto")
    result = _run_reproto([pb], out_dir)
    assert result.returncode == 0, result.stderr

    data = _load_yaml(out_dir / "message_set_proto2.yaml")
    messages = data["messages"]
    entries = data["entries"]

    # Item is a real messages key (existing TC-MS2 coverage) but must not
    # be an entry — it never corresponds to a real Descriptor.
    assert "mockup.MessageSetMessage.Item" in messages
    assert "mockup.MessageSetMessage.Item" not in entries
    # The real message that owns it IS an entry.
    assert "mockup.MessageSetMessage" in entries


# ---------------------------------------------------------------------------
# TC-6: Canonized output paths via variant import rewrites (spec 0086)
# ---------------------------------------------------------------------------

def test_TC6_canonized_output_paths(tmp_path: Path) -> None:
    """Variant import_rewrites + namespace_rewrites: all outputs use canonized names.

    Uses address_book + phone_number fixtures with a variant that rewrites:
    - phone_number.proto -> canonical/phone_number.proto  (import_rewrites)
    - .tutorial.          -> .canonical.tutorial.          (namespace_rewrites)

    Verifies §86.1/§86.2 (file paths):
    - output .proto is at canonical/phone_number.proto (not phone_number.proto)
    - output .yaml  is at canonical/phone_number.yaml  (not phone_number.yaml)
    - import statement inside address_book.proto references canonical/phone_number.proto

    Verifies §86.3 (binary fdp.name and fdp.dependency):
    - phone_number.pb carries fdp.name == canonical/phone_number.proto
    - address_book.pb carries the canonized dependency path

    Verifies §86.4 (binary type_name):
    - address_book.pb Person.phones field carries
      type_name == .canonical.tutorial.PhoneNumber (not .tutorial.PhoneNumber)
    """
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    # Compile address_book + phone_number into a single FDS with --include_imports
    from reproto.tests.conftest import compile_proto_multi
    pb = compile_proto_multi(pb_dir / "ab.pb", "address_book.proto", "phone_number.proto")

    # Write a variant YAML that rewrites phone_number.proto -> canonical/phone_number.proto
    variant_yaml = tmp_path / "test_variant.yaml"
    variant_yaml.write_text(
        "name: test\n"
        "descriptor_proto: google/protobuf/descriptor.proto\n"
        "well_known: {}\n"
        "import_rewrites:\n"
        "  - match: phone_number.proto\n"
        "    action: rewrite\n"
        "    to: canonical/phone_number.proto\n"
        "namespace_rewrites:\n"
        "  - match: .tutorial.\n"
        "    action: rewrite\n"
        "    to: .canonical.tutorial.\n"
        "orphans: {}\n"
        "annotation_modules: []\n"
    )

    # The custom variant has no embedded .pb files, so --use-variant descriptor
    # cannot load the fallback from it.  Copy the embedded descriptor.pb from
    # the built-in google-protobuf variant into the temp variant directory so
    # load_embedded_proto_fallback() finds it under variant_root/variant_stem/.
    import importlib.resources
    embedded_pb = (
        importlib.resources.files("reproto.variants")
        .joinpath("google-protobuf")
        .joinpath("google").joinpath("protobuf").joinpath("descriptor.pb")
        .read_bytes()
    )
    descriptor_dir = tmp_path / "test_variant" / "google" / "protobuf"
    descriptor_dir.mkdir(parents=True)
    (descriptor_dir / "descriptor.pb").write_bytes(embedded_pb)

    src_path = str(Path(__file__).parent.parent.parent)
    import os
    pythonpath_parts = [src_path]
    if existing := os.environ.get("PYTHONPATH"):
        pythonpath_parts.append(existing)
    env = {**os.environ, "PYTHONPATH": os.pathsep.join(pythonpath_parts)}
    env.pop("REPROTO_VARIANT", None)

    result = subprocess.run(
        [
            sys.executable, "-m", "reproto.cli",
            "--proto-variant", str(variant_yaml),
            "--use-variant", "descriptor",
            f"-I{FIXTURES_DIR}",
            f"--proto-out={out_dir}",
            "--emit-scoring-yaml",
            "--emit-binary",
            str(pb),
        ],
        capture_output=True, text=True, env=env,
    )
    assert result.returncode == 0, result.stderr

    # §86.1 — .proto output must be at canonized path
    assert (out_dir / "canonical" / "phone_number.proto").exists(), (
        "phone_number.proto must be written at canonical/phone_number.proto"
    )
    assert not (out_dir / "phone_number.proto").exists(), (
        "phone_number.proto must NOT be written at the original path"
    )

    # §86.2 — .yaml output must be at canonized path
    assert (out_dir / "canonical" / "phone_number.yaml").exists(), (
        "phone_number.yaml must be written at canonical/phone_number.yaml"
    )
    assert not (out_dir / "phone_number.yaml").exists(), (
        "phone_number.yaml must NOT be written at the original path"
    )

    # §86.1 — import statement inside address_book.proto must reference the canonized path
    ab_proto = (out_dir / "address_book.proto").read_text()
    assert 'import "canonical/phone_number.proto"' in ab_proto, (
        "address_book.proto must import canonical/phone_number.proto"
    )
    assert 'import "phone_number.proto"' not in ab_proto, (
        "address_book.proto must not import the original phone_number.proto path"
    )

    # §86.3 — binary phone_number.pb must carry the canonized file name
    from google.protobuf.descriptor_pb2 import FileDescriptorProto
    pn_pb_path = out_dir / "canonical" / "phone_number.pb"
    assert pn_pb_path.exists(), "phone_number.pb must be written at canonical/phone_number.pb"
    pn_fdp = FileDescriptorProto()
    pn_fdp.ParseFromString(pn_pb_path.read_bytes())
    assert pn_fdp.name == "canonical/phone_number.proto", (
        f"phone_number.pb fdp.name must be canonized, got {pn_fdp.name!r}"
    )

    # spec 0159 — phone_number.pb's own package must be canonized alongside
    # its self-referencing type names, so PhoneNumber.type's type_name stays
    # consistent with its own file's declared package.
    assert pn_fdp.package == "canonical.tutorial", (
        f"phone_number.pb package must be canonized, got {pn_fdp.package!r}"
    )
    phone_number_msg = next(m for m in pn_fdp.message_type if m.name == "PhoneNumber")
    type_field = next(f for f in phone_number_msg.field if f.name == "type")
    assert type_field.type_name == ".canonical.tutorial.PhoneNumber.PhoneType", (
        f"PhoneNumber.type type_name must stay consistent with the canonized "
        f"package, got {type_field.type_name!r}"
    )

    # §86.3 — binary address_book.pb must carry the canonized dependency path
    ab_pb_path = out_dir / "address_book.pb"
    assert ab_pb_path.exists(), "address_book.pb must be written"
    ab_fdp = FileDescriptorProto()
    ab_fdp.ParseFromString(ab_pb_path.read_bytes())
    assert "canonical/phone_number.proto" in ab_fdp.dependency, (
        f"address_book.pb dependency must contain canonized path, got {list(ab_fdp.dependency)!r}"
    )
    assert "phone_number.proto" not in ab_fdp.dependency, (
        "address_book.pb must not contain the original phone_number.proto dependency"
    )

    # §86.4 — binary address_book.pb Person.phones field must carry the canonized type_name
    person_msg = next(m for m in ab_fdp.message_type if m.name == "Person")
    phones_field = next(f for f in person_msg.field if f.name == "phones")
    assert phones_field.type_name == ".canonical.tutorial.PhoneNumber", (
        f"phones field type_name must be canonized, got {phones_field.type_name!r}"
    )


# ---------------------------------------------------------------------------
# TC-7: protoc-synthesized map-entry types appear as entries (spec 0140 G4)
# ---------------------------------------------------------------------------

def test_TC7_map_entry_types_are_entries(tmp_path: Path) -> None:
    """map<K, V> fields' auto-generated *Entry types appear in both messages and entries."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    (pb,) = compile_proto(pb_dir, "field_comprehensive.proto")
    result = _run_reproto([pb], out_dir)
    assert result.returncode == 0, result.stderr

    data = _load_yaml(out_dir / "field_comprehensive.yaml")
    messages = data["messages"]
    entries = data["entries"]

    map_entry_fqdn = "test.field.MapFields.StringToIntEntry"
    assert map_entry_fqdn in messages
    assert map_entry_fqdn in entries


# ---------------------------------------------------------------------------
# TC-8: entries stay sorted and duplicate-free across multiple nesting
# levels and multiple files (spec 0140 G5)
# ---------------------------------------------------------------------------

def test_TC8_entries_sorted_dedup_multi_file(tmp_path: Path) -> None:
    """entries lists across several files are each sorted, with no duplicate FQDNs."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    phone_pb, ab_pb = compile_proto(pb_dir, "phone_number.proto", "address_book.proto")
    result = _run_reproto([phone_pb, ab_pb], out_dir)
    assert result.returncode == 0, result.stderr

    for yaml_name in ("phone_number.yaml", "address_book.yaml"):
        data = _load_yaml(out_dir / yaml_name)
        entries = data["entries"]
        assert entries == sorted(entries), f"{yaml_name}: entries must be sorted"
        assert len(entries) == len(set(entries)), f"{yaml_name}: entries must be duplicate-free"
