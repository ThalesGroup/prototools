# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Tests for anomaly call sites in reproto (spec 0041 TC-A*, TC-B*, TC-C*, TC-D*).

Most of these anomalies are triggered by constructs that protoc itself rejects
at source level (e.g. proto3 with extension ranges, required fields, explicit
defaults).  They are exercised by building FileDescriptorProto objects
programmatically, serialising them to .pb files, and running reproto.

TC-A1/A2/A4 use real fixture files compiled by protoc; the gap was only
a missing stderr assertion, not a missing fixture.

TC-D1/D2 are pure unit tests that call ReFieldDescriptor.dump_option()
directly.
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest
from google.protobuf.descriptor_pb2 import (
    FieldDescriptorProto,
    FileDescriptorProto,
    FileDescriptorSet,
    MessageOptions,
)

from reproto.tests.conftest import FIXTURES_DIR, REPROTO_CMD, compile_proto


# ---------------------------------------------------------------------------
# Helpers
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

    dirs = [FIXTURES_DIR] + (include_dirs or [])
    include_flags = [f"-I{d}" for d in dirs]

    cmd = [
        *REPROTO_CMD,
        "--use-variant", "descriptor",
        *include_flags,
        f"--proto-out={out_dir}",
    ]
    if extra_args:
        cmd.extend(extra_args)
    cmd.extend(str(p) for p in pb_files)
    return subprocess.run(cmd, capture_output=True, text=True, env=env)


def _write_fds(path: Path, *fdps: FileDescriptorProto) -> Path:
    """Wrap one or more FileDescriptorProtos in a FileDescriptorSet and write."""
    fds = FileDescriptorSet()
    for fdp in fdps:
        fds.file.append(fdp)
    path.write_bytes(fds.SerializeToString())
    return path


def _minimal_proto3(name: str) -> FileDescriptorProto:
    fdp = FileDescriptorProto()
    fdp.name = name
    fdp.syntax = "proto3"
    return fdp


def _minimal_proto2(name: str) -> FileDescriptorProto:
    fdp = FileDescriptorProto()
    fdp.name = name
    fdp.syntax = "proto2"
    return fdp


def _read_output(out_dir: Path, name: str) -> str:
    p = out_dir / name
    return p.read_text(encoding="utf-8") if p.exists() else ""


# ---------------------------------------------------------------------------
# TC-A1  A1: editions file rendered as proto2 (--force-proto2-output)
# ---------------------------------------------------------------------------

@pytest.mark.roundtrip
def test_A1_editions_force_proto2_warning(tmp_path: Path) -> None:
    """Running --force-proto2-output on an editions file must emit WARNING[editions]."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    proto_src = FIXTURES_DIR / "editions_rendering.proto"
    pb_path = pb_dir / "editions_rendering.pb"
    result = subprocess.run(
        ["protoc",
         f"--descriptor_set_out={pb_path}",
         "--include_imports",
         f"-I{FIXTURES_DIR}",
         str(proto_src)],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, f"protoc failed: {result.stderr}"

    result = _run_reproto([pb_path], out_dir, extra_args=["--force-proto2-output"])
    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    # A1 is not printed, as A2 is not (commit 0ef3aa2): --force-proto2-output
    # is an explicit opt-in; the rendered file carries the comment.
    assert "editions file rendered as proto2" not in result.stderr, (
        f"A1 warning must be suppressed from stderr. stderr:\n{result.stderr}"
    )
    rendered = (out_dir / "editions_rendering.proto").read_text(encoding="utf-8")
    assert "editions file rendered as proto2" in rendered


# ---------------------------------------------------------------------------
# TC-A2  A2: proto3 syntax downconverted to proto2
# ---------------------------------------------------------------------------

@pytest.mark.roundtrip
def test_A2_proto3_force_proto2_warning(tmp_path: Path) -> None:
    """Running --force-proto2-output on a proto3 file must NOT emit a stderr warning
    (the comment is in the rendered file instead), but must succeed and write output."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    # Use a simple proto3 fixture
    (pb,) = compile_proto(pb_dir, "prune_base.proto")   # prune_base is proto3

    result = _run_reproto([pb], out_dir, extra_args=["--force-proto2-output"])
    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    # A2 warning must NOT appear in stderr — the rendered file carries the comment
    assert "output syntax downconverted" not in result.stderr, (
        f"A2 warning must be suppressed from stderr. stderr:\n{result.stderr}"
    )
    # But the rendered file must contain the WARNING[downconvert] comment
    out_file = out_dir / "prune_base.proto"
    assert out_file.exists(), "output file must be written"
    assert "WARNING[downconvert]" in out_file.read_text(), (
        "rendered file must contain WARNING[downconvert] comment"
    )


# ---------------------------------------------------------------------------
# TC-A4  A4: import weak in proto3 rendered as plain import
# ---------------------------------------------------------------------------

@pytest.mark.roundtrip
def test_A4_weak_import_proto3_warning(tmp_path: Path) -> None:
    """A proto3 descriptor with a weak dependency emits WARNING[proto3] for import weak."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    # Build a proto3 file that weakly imports another.
    # protoc rejects this in source, so we craft the descriptor directly.
    dep = _minimal_proto3("weak_dep.proto")
    dep_msg = dep.message_type.add()
    dep_msg.name = "Dep"
    dep_field = dep_msg.field.add()
    dep_field.name = "value"
    dep_field.number = 1
    dep_field.label = FieldDescriptorProto.LABEL_OPTIONAL
    dep_field.type = FieldDescriptorProto.TYPE_INT32

    main = _minimal_proto3("weak_main.proto")
    main.dependency.append("weak_dep.proto")
    main.weak_dependency.append(0)   # index 0 in dependency list is weak
    msg = main.message_type.add()
    msg.name = "Main"
    f = msg.field.add()
    f.name = "value"
    f.number = 1
    f.label = FieldDescriptorProto.LABEL_OPTIONAL
    f.type = FieldDescriptorProto.TYPE_INT32

    # Two separate .pb files so reproto seeds both (each is a separate seed file)
    dep_pb = _write_fds(pb_dir / "weak_dep.pb", dep)
    main_pb = _write_fds(pb_dir / "weak_main.pb", main)

    result = _run_reproto([dep_pb, main_pb], out_dir)
    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    # The A4 anomaly stderr message (from ANOMALIES["A4"].stderr format string)
    assert "'import weak' is not valid in proto3" in result.stderr, (
        f"A4 warning must appear for weak import in proto3. stderr:\n{result.stderr}"
    )
    content = _read_output(out_dir, "weak_main.proto")
    # The warning comment contains 'import weak "..."'; check only the actual import line.
    import_line = next(
        (ln for ln in content.splitlines() if ln.startswith("import ")), ""
    )
    assert import_line == 'import "weak_dep.proto";', (
        f"Weak import must be rendered as plain import, got: {import_line!r}"
    )


# ---------------------------------------------------------------------------
# TC-A5  A5: file-level extend block not valid in proto3
# ---------------------------------------------------------------------------

def test_A5_file_level_extend_proto3(tmp_path: Path) -> None:
    """A proto3 descriptor with a file-level extension emits OMITTED[proto3]."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    # We need a message with extension_range so extend is valid at descriptor level.
    # Define it in a proto2 helper file.
    helper = _minimal_proto2("extendable_helper.proto")
    extendable = helper.message_type.add()
    extendable.name = "Extendable"
    er = extendable.extension_range.add()
    er.start = 100
    er.end = 200

    # proto3 file with a file-level extension targeting that message
    main = _minimal_proto3("a5_main.proto")
    main.dependency.append("extendable_helper.proto")
    ext_field = main.extension.add()
    ext_field.name = "my_ext"
    ext_field.number = 100
    ext_field.label = FieldDescriptorProto.LABEL_OPTIONAL
    ext_field.type = FieldDescriptorProto.TYPE_INT32
    ext_field.extendee = ".Extendable"
    msg = main.message_type.add()
    msg.name = "Main"

    helper_pb = _write_fds(pb_dir / "a5_helper.pb", helper)
    main_pb = _write_fds(pb_dir / "a5_main.pb", main)

    result = _run_reproto([helper_pb, main_pb], out_dir)
    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    content = _read_output(out_dir, "a5_main.proto")
    assert "OMITTED[proto3]" in content, (
        f"A5 comment must appear in rendered output.\nContent:\n{content}"
    )
    # Ensure no actual "extend" block syntax was emitted (comment lines are OK)
    assert not any(ln.startswith("extend ") for ln in content.splitlines())


# ---------------------------------------------------------------------------
# TC-A5-regression  A5 regression: .proto2.*Options must not be omitted
# ---------------------------------------------------------------------------

def test_A5_proto2_options_extendee_not_omitted(tmp_path: Path) -> None:
    """Extending .proto2.MethodOptions in a proto3 file must NOT trigger A5.

    Regression test for the bug introduced in 94e9a10 (spec 0024): when reproto
    is run without a namespace-rewriting variant (e.g. --force-proto2-for-editions
    without --use-variant), extendees compiled against a descriptor.proto
    under package proto2 use the .proto2.* package instead of
    .google.protobuf.*.  allow_extend_block() must recognise these as valid
    custom-option targets in proto3.
    """
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    # Simulate a proto3 file that extends .proto2.MethodOptions (as produced
    # when compiling against a descriptor.proto under package proto2 without a
    # namespace-rewriting variant).
    main = _minimal_proto3("a5_regression.proto")
    ext_field = main.extension.add()
    ext_field.name = "my_method_opt"
    ext_field.number = 50000
    ext_field.label = FieldDescriptorProto.LABEL_OPTIONAL
    ext_field.type = FieldDescriptorProto.TYPE_BOOL
    ext_field.extendee = ".proto2.MethodOptions"

    pb = _write_fds(pb_dir / "a5_regression.pb", main)

    # Run without --use-variant so no namespace rewriting is active.
    result = _run_reproto([pb], out_dir)
    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    content = _read_output(out_dir, "a5_regression.proto")
    assert "OMITTED[proto3]" not in content, (
        "A5 must NOT fire for .proto2.MethodOptions extendee — "
        f"it is a valid proto3 custom-option target.\nContent:\n{content}"
    )
    # The extend block must actually appear in the output, not be omitted.
    # Its extendee is in no file, so its field is an orphan, and so is the
    # block around it (spec 0459): `///extend proto2.MethodOptions {`.
    assert any(ln.lstrip("/").startswith("extend ") for ln in content.splitlines()), (
        f"extend block must be rendered.\nContent:\n{content}"
    )


# ---------------------------------------------------------------------------
# TC-B1  B1: message-nested extend block not valid in proto3
# ---------------------------------------------------------------------------

def test_B1_nested_extend_proto3(tmp_path: Path) -> None:
    """A proto3 message with a nested extension emits OMITTED[proto3]."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    helper = _minimal_proto2("b1_helper.proto")
    extendable = helper.message_type.add()
    extendable.name = "Extendable"
    er = extendable.extension_range.add()
    er.start = 100
    er.end = 200

    main = _minimal_proto3("b1_main.proto")
    main.dependency.append("b1_helper.proto")
    outer = main.message_type.add()
    outer.name = "Outer"
    # A regular field so Outer gets summoned and rendered
    regular = outer.field.add()
    regular.name = "value"
    regular.number = 1
    regular.label = FieldDescriptorProto.LABEL_OPTIONAL
    regular.type = FieldDescriptorProto.TYPE_INT32
    # Extension inside Outer targeting Extendable
    ext_field = outer.extension.add()
    ext_field.name = "nested_ext"
    ext_field.number = 101
    ext_field.label = FieldDescriptorProto.LABEL_OPTIONAL
    ext_field.type = FieldDescriptorProto.TYPE_INT32
    ext_field.extendee = ".Extendable"

    helper_pb = _write_fds(pb_dir / "b1_helper.pb", helper)
    main_pb = _write_fds(pb_dir / "b1_main.pb", main)

    result = _run_reproto([helper_pb, main_pb], out_dir)
    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    content = _read_output(out_dir, "b1_main.proto")
    assert "OMITTED[proto3]" in content, (
        f"B1 comment must appear in rendered output.\nContent:\n{content}"
    )


# ---------------------------------------------------------------------------
# TC-B2  B2: extension range not valid in proto3
# ---------------------------------------------------------------------------

def test_B2_extension_range_proto3(tmp_path: Path) -> None:
    """A proto3 message with an extension_range emits OMITTED[proto3]."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    main = _minimal_proto3("b2_main.proto")
    msg = main.message_type.add()
    msg.name = "WithRange"
    field = msg.field.add()
    field.name = "value"
    field.number = 1
    field.label = FieldDescriptorProto.LABEL_OPTIONAL
    field.type = FieldDescriptorProto.TYPE_INT32
    er = msg.extension_range.add()
    er.start = 100
    er.end = 200

    pb = _write_fds(pb_dir / "b2.pb", main)

    result = _run_reproto([pb], out_dir)
    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    content = _read_output(out_dir, "b2_main.proto")
    assert "OMITTED[proto3]" in content, (
        f"B2 comment must appear in rendered output.\nContent:\n{content}"
    )


# ---------------------------------------------------------------------------
# TC-B3  B3: message_set_wire_format not valid in proto3
# ---------------------------------------------------------------------------

def test_B3_message_set_wire_format_proto3(tmp_path: Path) -> None:
    """A proto3 message with message_set_wire_format=true emits WARNING[proto3]."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    main = _minimal_proto3("b3_main.proto")
    msg = main.message_type.add()
    msg.name = "MsgSet"
    msg.options.CopyFrom(MessageOptions())
    msg.options.message_set_wire_format = True

    pb = _write_fds(pb_dir / "b3.pb", main)

    result = _run_reproto([pb], out_dir)
    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    content = _read_output(out_dir, "b3_main.proto")
    assert "WARNING[proto3]" in content, (
        f"B3 comment must appear in rendered output.\nContent:\n{content}"
    )
    # The option must not appear as actual proto syntax — only in comment/warning lines
    option_lines = [
        ln for ln in content.splitlines()
        if "message_set_wire_format" in ln and not ln.strip().startswith("//")
    ]
    assert option_lines == [], (
        f"message_set_wire_format must not appear as proto syntax. Content:\n{content}"
    )


# ---------------------------------------------------------------------------
# TC-C1  C1: non-canonical map entry (missing key or value field)
# ---------------------------------------------------------------------------

def test_C1_non_canonical_map_entry(tmp_path: Path) -> None:
    """A map-entry message missing its value field (field 2) triggers C1."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    main = _minimal_proto3("c1_main.proto")
    outer = main.message_type.add()
    outer.name = "Outer"

    # Synthetic map entry with only key field (field 1), missing value (field 2)
    entry = outer.nested_type.add()
    entry.name = "BadMapEntry"
    entry.options.CopyFrom(MessageOptions())
    entry.options.map_entry = True
    key_field = entry.field.add()
    key_field.name = "key"
    key_field.number = 1
    key_field.label = FieldDescriptorProto.LABEL_OPTIONAL
    key_field.type = FieldDescriptorProto.TYPE_STRING

    # The "map" field referencing this malformed entry
    map_field = outer.field.add()
    map_field.name = "bad_map"
    map_field.number = 1
    map_field.label = FieldDescriptorProto.LABEL_REPEATED
    map_field.type = FieldDescriptorProto.TYPE_MESSAGE
    map_field.type_name = ".Outer.BadMapEntry"

    pb = _write_fds(pb_dir / "c1.pb", main)

    result = _run_reproto([pb], out_dir)
    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    content = _read_output(out_dir, "c1_main.proto")
    assert "WARNING[render]" in content, (
        f"C1 comment must appear in rendered output.\nContent:\n{content}"
    )
    assert "repeated" in content, "Fallback repeated field must be rendered"


# ---------------------------------------------------------------------------
# TC-C2  C2: group field not valid in proto3
# ---------------------------------------------------------------------------

def test_C2_group_field_proto3(tmp_path: Path) -> None:
    """A proto3 field of TYPE_GROUP emits WARNING[proto3]."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    main = _minimal_proto3("c2_main.proto")
    msg = main.message_type.add()
    msg.name = "Outer"

    # The group's synthetic message type
    grp = main.message_type.add()
    grp.name = "Mygroup"
    grp_field = grp.field.add()
    grp_field.name = "val"
    grp_field.number = 1
    grp_field.label = FieldDescriptorProto.LABEL_OPTIONAL
    grp_field.type = FieldDescriptorProto.TYPE_INT32

    # Group field inside msg
    field = msg.field.add()
    field.name = "mygroup"
    field.number = 1
    field.label = FieldDescriptorProto.LABEL_OPTIONAL
    field.type = FieldDescriptorProto.TYPE_GROUP
    field.type_name = ".Mygroup"

    pb = _write_fds(pb_dir / "c2.pb", main)

    result = _run_reproto([pb], out_dir)
    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    content = _read_output(out_dir, "c2_main.proto")
    assert "WARNING[proto3]" in content, (
        f"C2 comment must appear in rendered output.\nContent:\n{content}"
    )


# ---------------------------------------------------------------------------
# TC-C3  C3: required label not valid in proto3
# ---------------------------------------------------------------------------

def test_C3_required_field_proto3(tmp_path: Path) -> None:
    """A proto3 field with LABEL_REQUIRED emits WARNING[proto3]; output has no 'required'."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    main = _minimal_proto3("c3_main.proto")
    msg = main.message_type.add()
    msg.name = "Msg"
    field = msg.field.add()
    field.name = "mandatory"
    field.number = 1
    field.label = FieldDescriptorProto.LABEL_REQUIRED
    field.type = FieldDescriptorProto.TYPE_STRING

    pb = _write_fds(pb_dir / "c3.pb", main)

    result = _run_reproto([pb], out_dir)
    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    content = _read_output(out_dir, "c3_main.proto")
    assert "WARNING[proto3]" in content, (
        f"C3 comment must appear in rendered output.\nContent:\n{content}"
    )
    # The rendered field line must not carry 'required' as a label
    field_lines = [
        ln for ln in content.splitlines()
        if "mandatory" in ln and not ln.strip().startswith("//")
    ]
    assert field_lines and all("required" not in ln for ln in field_lines), (
        f"'required' must not appear on the actual field line. Content:\n{content}"
    )


# ---------------------------------------------------------------------------
# TC-C4  C4: explicit default value not valid in proto3
# ---------------------------------------------------------------------------

def test_C4_explicit_default_proto3(tmp_path: Path) -> None:
    """A proto3 field with default_value set emits WARNING[proto3]."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    main = _minimal_proto3("c4_main.proto")
    msg = main.message_type.add()
    msg.name = "Msg"
    field = msg.field.add()
    field.name = "greeting"
    field.number = 1
    field.label = FieldDescriptorProto.LABEL_OPTIONAL
    field.type = FieldDescriptorProto.TYPE_STRING
    field.default_value = "hello"

    pb = _write_fds(pb_dir / "c4.pb", main)

    result = _run_reproto([pb], out_dir)
    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    content = _read_output(out_dir, "c4_main.proto")
    assert "WARNING[proto3]" in content, (
        f"C4 comment must appear in rendered output.\nContent:\n{content}"
    )


# ---------------------------------------------------------------------------
# TC-D1/D2  Unit tests for ReFieldDescriptor.dump_option()
# ---------------------------------------------------------------------------

def _make_context():
    from reproto.context import Context
    from reproto.load import PathPatterns
    return Context(pruned_fqdns=set(), pruned_paths=PathPatterns(set()), path_seeds=PathPatterns(set()))


def test_D1_scalar_type_mismatch() -> None:
    """get_scalar() with a Python type that mismatches the proto field type raises RuntimeError.

    The D1 comment path in dump_option() catches TypeError from get_scalar()'s case _
    branch.  In practice the case _ branch is only reached when value is not
    bool/int/float/str/bytes, which dump_option() already handles in its own case _
    arm (D2).  For ordinary Python scalar→proto mismatches (e.g. int to a string
    field), get_scalar() raises RuntimeError — which propagates up through dump_option.
    This test documents that observable behaviour.
    """
    import pytest
    from google.protobuf.descriptor_pool import Default
    from reproto.field_descriptor import ReFieldDescriptor

    ctx = _make_context()
    pool = Default()

    # FileOptions.java_package is a string field — pass an int to trigger RuntimeError.
    fo_desc = pool.FindMessageTypeByName("google.protobuf.FileOptions")
    java_pkg_field = fo_desc.fields_by_name["java_package"]
    rfd = ReFieldDescriptor(java_pkg_field)

    with pytest.raises(RuntimeError, match="Unexpected FieldDescriptor type"):
        rfd.dump_option(ctx, 42, lev=0, custom=False)


def test_D2_unknown_descriptor_type() -> None:
    """dump_option() with an unrecognised value type → D2 OMITTED comment."""
    from google.protobuf.descriptor_pool import Default
    from reproto.field_descriptor import ReFieldDescriptor
    from reproto.text import COMMENT

    ctx = _make_context()
    pool = Default()

    fo_desc = pool.FindMessageTypeByName("google.protobuf.FileOptions")
    java_pkg_field = fo_desc.fields_by_name["java_package"]
    rfd = ReFieldDescriptor(java_pkg_field)

    # Pass a list — not matched by any case arm → D2
    block, _ = rfd.dump_option(ctx, [1, 2, 3], lev=0, custom=False)

    lines = list(block)
    assert any(ln.type == COMMENT and "OMITTED[render]" in ln.text for ln in lines), (
        f"D2 OMITTED[render] comment must appear. Block: {[ln.text for ln in lines]}"
    )
