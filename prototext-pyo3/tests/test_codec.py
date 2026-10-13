# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Unit tests for prototext_codec_lib.

Fixtures are synthesised in-process using protobuf's Python API so that
the test suite has no dependency on protoc and no committed binary files.
"""

from __future__ import annotations

from google.protobuf.any_pb2 import Any
from google.protobuf.descriptor_pb2 import (
    FieldDescriptorProto,
    FileDescriptorProto,
    FileDescriptorSet,
)

import prototext_codec_lib


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _make_fdp_bytes(name: str = "test/schema.proto") -> bytes:
    """Return a minimal serialised FileDescriptorProto."""
    fdp = FileDescriptorProto()
    fdp.name = name
    return fdp.SerializeToString()


def _fdp_schema_bytes() -> bytes:
    """Return a FileDescriptorSet describing FileDescriptorProto itself.

    register_schema expects a FileDescriptorSet (not a bare FileDescriptorProto).
    """
    fds = FileDescriptorSet()
    FileDescriptorProto.DESCRIPTOR.file.CopyToProto(fds.file.add())
    return fds.SerializeToString()


def _any_schema_bytes() -> bytes:
    """FileDescriptorSet for `acme.Container { google.protobuf.Any payload = 1; }`
    plus `acme.Payload { string label = 1; }`, both self-contained.
    """
    fds = FileDescriptorSet()
    Any.DESCRIPTOR.file.CopyToProto(fds.file.add())

    payload_file = fds.file.add()
    payload_file.name = "acme/payload.proto"
    payload_file.package = "acme"
    payload_file.syntax = "proto3"
    payload_msg = payload_file.message_type.add()
    payload_msg.name = "Payload"
    label_field = payload_msg.field.add()
    label_field.name = "label"
    label_field.number = 1
    label_field.type = FieldDescriptorProto.TYPE_STRING
    label_field.label = FieldDescriptorProto.LABEL_OPTIONAL

    container_file = fds.file.add()
    container_file.name = "acme/container.proto"
    container_file.package = "acme"
    container_file.syntax = "proto3"
    container_file.dependency.append("google/protobuf/any.proto")
    container_msg = container_file.message_type.add()
    container_msg.name = "Container"
    payload_field = container_msg.field.add()
    payload_field.name = "payload"
    payload_field.number = 1
    payload_field.type = FieldDescriptorProto.TYPE_MESSAGE
    payload_field.type_name = ".google.protobuf.Any"
    payload_field.label = FieldDescriptorProto.LABEL_OPTIONAL

    return fds.SerializeToString()


def _any_wire_bytes() -> bytes:
    """Wire bytes for `Container{payload: Any{type_url: ".../acme.Payload",
    value: Payload{label: "hello"}}}`, hand-encoded (no dependency on the
    generated Python message classes for these ad-hoc types).
    """
    payload_bytes = b"\x0a\x05hello"  # Payload.label = "hello" (field 1, LEN)
    type_url = b"type.googleapis.com/acme.Payload"
    any_bytes = (
        b"\x0a" + bytes([len(type_url)]) + type_url  # Any.type_url (field 1, LEN)
        + b"\x12" + bytes([len(payload_bytes)]) + payload_bytes  # Any.value (field 2, LEN)
    )
    return b"\x0a" + bytes([len(any_bytes)]) + any_bytes  # Container.payload (field 1, LEN)


# ---------------------------------------------------------------------------
# TC-1  Empty input → returns b""
# ---------------------------------------------------------------------------

def test_format_as_text_empty() -> None:
    """format_as_text on empty bytes returns empty bytes without raising."""
    result = prototext_codec_lib.format_as_text(b"")
    assert result == b""


# ---------------------------------------------------------------------------
# TC-2  format_as_text without schema always renders (spec 0097 goal 5)
# ---------------------------------------------------------------------------

def test_format_as_text_no_schema_renders_unknown_fields() -> None:
    """format_as_text without a schema always renders unknown fields.

    Spec 0097 goal 5: when no descriptor is active, unknown fields are always
    rendered regardless of --no-annotations / include_annotations.  The
    three-step cascade (nested message → UTF-8 string → bytes) is applied.
    A FileDescriptorProto's name field (field 1, LEN) is valid UTF-8, so it
    is rendered as a quoted string.
    """
    result = prototext_codec_lib.format_as_text(_make_fdp_bytes())
    assert result == b'1: "test/schema.proto"\n'


# ---------------------------------------------------------------------------
# TC-3  format_as_text with annotations produces the #@ prototext: header
# ---------------------------------------------------------------------------

def test_format_as_text_annotations_has_prototext_header() -> None:
    """format_as_text with include_annotations=True produces a #@ prototext: header."""
    result = prototext_codec_lib.format_as_text(
        _make_fdp_bytes(), include_annotations=True
    )
    assert result.startswith(b"#@ prototext:"), (
        f"expected #@ prototext: header, got: {result[:40]!r}"
    )


# ---------------------------------------------------------------------------
# TC-4  Round-trip: format_as_bytes(format_as_text(data, annotations)) == data
# ---------------------------------------------------------------------------

def test_round_trip() -> None:
    """Encoding to annotated text and back to binary is lossless."""
    original = _make_fdp_bytes("roundtrip/check.proto")
    text = prototext_codec_lib.format_as_text(original, include_annotations=True)
    recovered = prototext_codec_lib.format_as_bytes(text)
    assert recovered == original, (
        f"round-trip mismatch: {len(original)} bytes in, {len(recovered)} bytes out"
    )


# ---------------------------------------------------------------------------
# TC-5  format_as_bytes with assume_binary=True returns input unchanged
# ---------------------------------------------------------------------------

def test_format_as_bytes_assume_binary_passthrough() -> None:
    """format_as_bytes(data, assume_binary=True) is a no-op passthrough."""
    data = _make_fdp_bytes()
    result = prototext_codec_lib.format_as_bytes(data, assume_binary=True)
    assert result == data


# ---------------------------------------------------------------------------
# TC-6  format_as_text on already-textual input returns it unchanged
# ---------------------------------------------------------------------------

def test_format_as_text_text_passthrough() -> None:
    """format_as_text on input that already starts with #@ prototext: is a no-op."""
    original = _make_fdp_bytes()
    text = prototext_codec_lib.format_as_text(original, include_annotations=True)
    assert text.startswith(b"#@ prototext:")
    # Calling format_as_text again must return the same bytes object (fast path).
    result = prototext_codec_lib.format_as_text(text)
    assert result == text


# ---------------------------------------------------------------------------
# TC-7  register_schema returns a SchemaHandle without raising
# ---------------------------------------------------------------------------

def test_register_schema_schemaless() -> None:
    """register_schema(b'', '') returns a SchemaHandle for schema-less decoding."""
    handle = prototext_codec_lib.register_schema(b"", "")
    assert isinstance(handle, prototext_codec_lib.SchemaHandle)


def test_register_schema_with_fdp() -> None:
    """register_schema with a real FileDescriptorProto schema returns a handle."""
    schema_bytes = _fdp_schema_bytes()
    handle = prototext_codec_lib.register_schema(
        schema_bytes, "google.protobuf.FileDescriptorProto"
    )
    assert isinstance(handle, prototext_codec_lib.SchemaHandle)


# ---------------------------------------------------------------------------
# TC-8  Schema-aware format_as_text includes field name annotations
# ---------------------------------------------------------------------------

def test_format_as_text_with_schema_annotations() -> None:
    """format_as_text with a schema and include_annotations=True annotates field names."""
    # Use FileDescriptorProto's own descriptor as the schema, and a
    # serialised FileDescriptorProto instance as the data.
    schema_bytes = _fdp_schema_bytes()
    handle = prototext_codec_lib.register_schema(
        schema_bytes, "google.protobuf.FileDescriptorProto"
    )
    data = _make_fdp_bytes("annotated/schema.proto")
    result = prototext_codec_lib.format_as_text(
        data, schema=handle, include_annotations=True
    )
    # The "name" field (field 1 of FileDescriptorProto) should appear as an
    # annotation comment in the output.
    assert b"name" in result, (
        f"expected 'name' annotation in output, got: {result!r}"
    )


# ---------------------------------------------------------------------------
# TC-9  google.protobuf.Any expansion (spec 0106 S2 regression guard)
# ---------------------------------------------------------------------------
#
# prototext-pyo3 never calls set_any_loader itself from Python — S2 removed
# the `all_schemas` fast path that used to make Any expansion work here for
# free. Without prototext-pyo3 installing its own ANY_LOADER (spec 0106 S2)
# around render_as_text/format_as_text, this would silently regress to raw
# bytes instead of a symbolic `value { ... }` block, even though the
# Payload type is present in the registered schema's pool.

def test_format_as_text_expands_any_field() -> None:
    """format_as_text expands a google.protobuf.Any field's value symbolically."""
    handle = prototext_codec_lib.register_schema(_any_schema_bytes(), "acme.Container")
    result = prototext_codec_lib.format_as_text(
        _any_wire_bytes(), schema=handle, assume_binary=True, include_annotations=True
    )
    text = result.decode("utf-8")
    assert "type.googleapis.com/acme.Payload" in text, (
        f"expected type_url in Any expansion: {text}"
    )
    assert "value {" in text, f"expected expanded value block in Any expansion: {text}"
    assert "hello" in text, f"expected Payload.label value in Any expansion: {text}"


# ---------------------------------------------------------------------------
# Spec 0415 S10: two upb copies in one process
# ---------------------------------------------------------------------------
#
# Python's protobuf runs on its own upb, and this extension now embeds
# another (spec 0415 S1), possibly of a different version. Both must work
# side by side, which holds only if the extension keeps its upb symbols to
# itself: an exported `upb_*` symbol could bind Python's protobuf to our
# copy, or our code to its.


def test_works_beside_python_protobufs_own_upb() -> None:
    """Python's upb and ours, used in turn in one process, both answer."""
    from google.protobuf import descriptor_pool

    pool = descriptor_pool.DescriptorPool()
    fdp = FileDescriptorProto()
    FileDescriptorProto.DESCRIPTOR.file.CopyToProto(fdp)
    pool.Add(fdp)
    assert pool.FindMessageTypeByName("google.protobuf.FileDescriptorProto")

    handle = prototext_codec_lib.register_schema(
        _fdp_schema_bytes(), "google.protobuf.FileDescriptorProto"
    )
    result = prototext_codec_lib.format_as_text(
        _make_fdp_bytes("side/by_side.proto"), schema=handle
    )
    assert b'name: "side/by_side.proto"' in result, result

    # And Python's pool still works after ours has.
    assert pool.FindMessageTypeByName("google.protobuf.DescriptorProto")


def test_exports_no_upb_symbol() -> None:
    """The extension's dynamic symbol table holds no upb function."""
    import ctypes

    lib = ctypes.CDLL(prototext_codec_lib.prototext_codec_lib.__file__)
    for symbol in ("upb_DefPool_New", "upb_Arena_New", "upb_Decode",
                   "prototext_upb_add_file"):
        try:
            getattr(lib, symbol)
        except AttributeError:
            continue
        raise AssertionError(f"the extension exports {symbol}")
