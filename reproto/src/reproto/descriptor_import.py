# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""The import rule (spec 0447 S4).

A file setting a built-in option field that its reader knows only from the
descriptor in use (a field of an options message the stock descriptor
lacks), and not importing that descriptor, gets the import: protoc reads
built-in options against the descriptor in its pool, its compiled-in copy
unless the file loads another.

The fields set are read from each file's serialized descriptor, by number,
the same walk as protoback's `descriptor_import.rs`.
"""

from __future__ import annotations

import importlib.resources

from google.protobuf.descriptor_pb2 import FileDescriptorProto, FileDescriptorSet

# What each kind of descriptor holds: its children, by field number and
# kind, and the number of its options and their message.
_FILE, _MSG, _FIELD, _ONEOF, _ENUM, _VALUE, _RANGE, _SERVICE, _METHOD = range(9)
_CHILDREN: dict[int, dict[int, int]] = {
    _FILE: {4: _MSG, 5: _ENUM, 6: _SERVICE, 7: _FIELD},
    _MSG: {2: _FIELD, 6: _FIELD, 3: _MSG, 4: _ENUM, 5: _RANGE, 8: _ONEOF},
    _ENUM: {2: _VALUE},
    _SERVICE: {2: _METHOD},
}
_OPTIONS: dict[int, tuple[int, str]] = {
    _FILE: (8, 'FileOptions'),
    _MSG: (7, 'MessageOptions'),
    _FIELD: (8, 'FieldOptions'),
    _ONEOF: (2, 'OneofOptions'),
    _ENUM: (3, 'EnumOptions'),
    _VALUE: (3, 'EnumValueOptions'),
    _RANGE: (3, 'ExtensionRangeOptions'),
    _SERVICE: (3, 'ServiceOptions'),
    _METHOD: (4, 'MethodOptions'),
}


def _varint(buf: bytes, i: int) -> tuple[int, int]:
    value = shift = 0
    while True:
        b = buf[i]
        i += 1
        value |= (b & 0x7F) << shift
        if b < 0x80:
            return value, i
        shift += 7


def _fields(buf: bytes) -> list[tuple[int, bytes | None]]:
    """Each field of a serialized message: its number, and its bytes when
    length-delimited. A malformed buffer gives the fields before the flaw."""
    out: list[tuple[int, bytes | None]] = []
    i = 0
    try:
        while i < len(buf):
            tag, i = _varint(buf, i)
            number, wire = tag >> 3, tag & 7
            if wire == 0:
                _, i = _varint(buf, i)
                out.append((number, None))
            elif wire == 1:
                i += 8
                out.append((number, None))
            elif wire == 2:
                n, i = _varint(buf, i)
                out.append((number, buf[i:i + n]))
                i += n
            elif wire == 5:
                i += 4
                out.append((number, None))
            else:
                return out   # groups: not in a descriptor's options
    except IndexError:
        pass
    return out


def options_set(fdp: bytes) -> dict[str, set[int]]:
    """The field numbers each options message has set in the file, at any
    depth, by the message's name."""
    out: dict[str, set[int]] = {}
    todo: list[tuple[int, bytes]] = [(_FILE, fdp)]
    while todo:
        kind, buf = todo.pop()
        children = _CHILDREN.get(kind, {})
        options_number, options_name = _OPTIONS[kind]
        for number, body in _fields(buf):
            if body is None:
                continue
            if number == options_number:
                out.setdefault(options_name, set()).update(n for n, _ in _fields(body))
            elif number in children:
                todo.append((children[number], body))
    return out


def with_dependency(fdp: bytes, name: str) -> bytes:
    """`fdp` with one more `dependency`, `name`, after the others: field 3
    appended to the serialization, the rest untouched."""
    data = name.encode('utf-8')
    n, length = len(data), bytearray()
    while True:
        if n < 0x80:
            length.append(n)
            break
        length.append((n & 0x7F) | 0x80)
        n >>= 7
    return fdp + b'\x1a' + bytes(length) + data


def message_fields(fdp: FileDescriptorProto) -> dict[str, set[int]]:
    """The numbers of the fields of each top-level message of `fdp`."""
    return {m.name: {f.number for f in m.field} for m in fdp.message_type}


def stock_fields() -> dict[str, set[int]]:
    """The fields of the stock descriptor's messages: the tools' embedded
    copy (the built-in variant's), whatever variant is active."""
    data = (importlib.resources.files('reproto.variants')
            .joinpath('google-protobuf/google/protobuf/descriptor.pb').read_bytes())
    fds = FileDescriptorSet()
    fds.ParseFromString(data)
    return message_fields(fds.file[0])


def needs_import(fdp: bytes, descriptor: dict[str, set[int]],
                 stock: dict[str, set[int]]) -> bool:
    """Whether the file (its serialized descriptor `fdp`) sets a field of an
    options message that `descriptor` declares and `stock` lacks."""
    for message, numbers in options_set(fdp).items():
        if numbers & (descriptor.get(message, set()) - stock.get(message, set())):
            return True
    return False
