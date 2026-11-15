# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Descriptors in the text format, read back (spec 0424 S5, spec 0426).

The texts are written as ``protoc --decode`` prints a descriptor set
without the options' schema: a custom option by number. The declaring
files are built with protobuf's Python API, so there is no protoc here.
"""

from __future__ import annotations

import pytest
from google.protobuf.descriptor_pb2 import (
    DescriptorProto,
    FieldDescriptorProto,
    FileDescriptorProto,
    FileDescriptorSet,
)

import prototext_codec_lib
from prototext_codec_lib import (
    parse_descriptor_text,
    read_descriptors,
    scan_descriptor_text,
    split_descriptor_set,
)

# A message's options: field 7 of a DescriptorProto.
_TEXT = """\
name: "use.proto"
message_type {
  name: "Holder"
  options {
    50001 {
      1: 3
    }
  }
}
"""


def _varint(n: int) -> bytes:
    out = bytearray()
    while n >= 0x80:
        out.append((n & 0x7F) | 0x80)
        n >>= 7
    out.append(n)
    return bytes(out)


def _tag(number: int, wire_type: int) -> bytes:
    return _varint(number << 3 | wire_type)


def _len(number: int, payload: bytes) -> bytes:
    return _tag(number, 2) + _varint(len(payload)) + payload


def _use_proto(option: bytes) -> bytes:
    """``use.proto`` as ``_TEXT`` says, with ``option`` as Holder's options."""
    holder = _len(1, b"Holder") + _len(7, option)
    return _len(1, b"use.proto") + _len(4, holder)


def _declaring_file(label_type: FieldDescriptorProto.Type.ValueType) -> bytes:
    """A file declaring option 50001 of MessageOptions, of ``label_type``."""
    fdp = FileDescriptorProto(
        name="decl.proto", package="decl",
        dependency=["google/protobuf/descriptor.proto"],
    )
    fdp.message_type.append(DescriptorProto(
        name="Value",
        field=[FieldDescriptorProto(
            name="x", number=1,
            type=FieldDescriptorProto.TYPE_INT32,
            label=FieldDescriptorProto.LABEL_OPTIONAL,
        )],
    ))
    ext = FieldDescriptorProto(
        name="opt", number=50001, type=label_type,
        label=FieldDescriptorProto.LABEL_OPTIONAL,
        extendee=".google.protobuf.MessageOptions",
    )
    if label_type in (FieldDescriptorProto.TYPE_MESSAGE,
                      FieldDescriptorProto.TYPE_GROUP):
        ext.type_name = ".decl.Value"
    fdp.extension.append(ext)
    return fdp.SerializeToString()


_DESCRIPTOR = "google/protobuf/descriptor.proto"


def _read(*binary: bytes, text: str = _TEXT) -> tuple[bytes, list[str]]:
    """``text`` read in import order with ``binary``."""
    texts, skipped = read_descriptors(list(binary), [text], _DESCRIPTOR)
    assert skipped == []
    [(parsed, warnings, error)] = texts
    assert error is None and parsed is not None
    return parsed, warnings


def test_exported() -> None:
    for name in ("parse_descriptor_text", "read_descriptors",
                 "scan_descriptor_text", "split_descriptor_set"):
        assert name in prototext_codec_lib.__all__


def test_alone_an_option_by_number_is_length_delimited() -> None:
    assert parse_descriptor_text(_TEXT) == _use_proto(
        _len(50001, _tag(1, 0) + b"\x03"))


def test_a_declared_message_is_length_delimited() -> None:
    parsed, warnings = _read(_declaring_file(FieldDescriptorProto.TYPE_MESSAGE))
    assert parsed == _use_proto(_len(50001, _tag(1, 0) + b"\x03"))
    assert warnings == []


def test_a_declared_group_is_a_group() -> None:
    parsed, warnings = _read(_declaring_file(FieldDescriptorProto.TYPE_GROUP))
    group = _tag(50001, 3) + _tag(1, 0) + b"\x03" + _tag(50001, 4)
    assert parsed == _use_proto(group)
    assert warnings == []


def test_a_declared_string_keeps_its_bytes() -> None:
    """``1: 3`` inside is exact, so the string is ``\\x08\\x03``: no group,
    hence no warning."""
    parsed, warnings = _read(_declaring_file(FieldDescriptorProto.TYPE_STRING))
    assert parsed == _use_proto(_len(50001, b"\x08\x03"))
    assert warnings == []


_NAMED = _TEXT.replace("50001 {\n      1: 3", "[decl.opt] {\n      x: 3")


def test_an_option_by_name() -> None:
    declaring = _declaring_file(FieldDescriptorProto.TYPE_MESSAGE)
    assert _read(declaring, text=_NAMED) == _read(declaring)


def test_an_option_declared_nowhere_is_reported() -> None:
    parsed, warnings = _read()
    assert parsed == _use_proto(_len(50001, _tag(1, 0) + b"\x03"))
    assert len(warnings) == 1
    assert warnings[0].startswith(
        "field 50001 of google.protobuf.MessageOptions is declared nowhere"
    )


def test_an_extension_by_name_declared_nowhere_is_reported() -> None:
    parsed, warnings = _read(text=_NAMED)
    assert parsed == _use_proto(b"")
    assert warnings == [
        "extension [decl.opt] of google.protobuf.MessageOptions is declared"
        " nowhere in the input: skipped"
    ]


def test_text_that_does_not_parse() -> None:
    texts, _ = read_descriptors([], ['name: "a.proto"\n}'], _DESCRIPTOR)
    assert texts == [(None, [], "2:1: unbalanced brackets")]


def test_the_scan_and_the_split() -> None:
    assert scan_descriptor_text(_TEXT) == ("use.proto", [])
    assert split_descriptor_set("file {\n" + _TEXT + "}\n") == ["\n" + _TEXT]
    with pytest.raises(ValueError, match=r"^1:\d+: "):
        scan_descriptor_text('name: "a')


def test_a_set() -> None:
    parsed = parse_descriptor_text("file {\n" + _TEXT + "}\n", is_set=True)
    s = FileDescriptorSet()
    s.ParseFromString(parsed)
    assert [f.name for f in s.file] == ["use.proto"]


def test_an_error_has_its_line_and_column() -> None:
    with pytest.raises(ValueError, match=r"^2:1: "):
        parse_descriptor_text('name: "a.proto"\n}')


def test_a_missing_import_is_mended() -> None:
    lonely = FileDescriptorProto(name="lonely.proto", dependency=["gone.proto"])
    _, skipped = read_descriptors([lonely.SerializeToString()], [], _DESCRIPTOR)
    assert skipped == []


def test_a_file_that_does_not_load_even_mended_is_reported() -> None:
    """Two fields of one number: nothing mending removes."""
    field = {"label": FieldDescriptorProto.LABEL_OPTIONAL,
             "type": FieldDescriptorProto.TYPE_INT32, "number": 1}
    bad = FileDescriptorProto(name="bad.proto", message_type=[DescriptorProto(
        name="M", field=[FieldDescriptorProto(name="a", **field),
                         FieldDescriptorProto(name="b", **field)])])
    _, skipped = read_descriptors([bad.SerializeToString()], [], _DESCRIPTOR)
    assert [name for name, _ in skipped] == ["bad.proto"]
