# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Build an FdsIndex (spec 0068) from a raw .pb FDS and its decoded proto."""

from pathlib import Path

from google.protobuf.descriptor_pb2 import (
    DescriptorProto,
    FileDescriptorProto,
    FileDescriptorSet,
)


def _decode_varint(buf: bytes, pos: int) -> tuple[int, int]:
    """Decode a protobuf varint; return (value, new_pos)."""
    result = 0
    shift = 0
    while True:
        b = buf[pos]
        pos += 1
        result |= (b & 0x7F) << shift
        if not (b & 0x80):
            return result, pos
        shift += 7


def extract_spans(raw_pb_bytes: bytes) -> list[tuple[int, int]]:
    """Return (start, end) byte offsets for each FileDescriptorProto in the FDS.

    Performs a single linear scan of the wire format without decoding each FDP.
    FileDescriptorSet has one repeated field (file, field number 1, wire type 2).
    """
    spans: list[tuple[int, int]] = []
    pos = 0
    n = len(raw_pb_bytes)
    while pos < n:
        tag, pos = _decode_varint(raw_pb_bytes, pos)
        field_number = tag >> 3
        wire_type = tag & 0x7
        assert field_number == 1 and wire_type == 2, (
            f"unexpected field {field_number} wire_type {wire_type} at offset {pos}"
        )
        length, pos = _decode_varint(raw_pb_bytes, pos)
        start = pos
        end = pos + length
        spans.append((start, end))
        pos = end
    return spans


def _collect_nested(
    prefix: str,
    msg: DescriptorProto,
    file_name: str,
    type_to_file: dict[str, str],
) -> None:
    """Recursively add all message and enum FQDNs under prefix to type_to_file."""
    for nested in msg.nested_type:
        fqdn = f"{prefix}{nested.name}"
        type_to_file[fqdn] = file_name
        _collect_nested(f"{fqdn}.", nested, file_name, type_to_file)
    for enum_type in msg.enum_type:
        type_to_file[f"{prefix}{enum_type.name}"] = file_name


def _collect_types(
    prefix: str,
    fdp: FileDescriptorProto,
    file_name: str,
    type_to_file: dict[str, str],
) -> None:
    """Add all top-level message and enum FQDNs from fdp to type_to_file."""
    for msg_type in fdp.message_type:
        fqdn = f"{prefix}{msg_type.name}"
        type_to_file[fqdn] = file_name
        _collect_nested(f"{fqdn}.", msg_type, file_name, type_to_file)
    for enum_type in fdp.enum_type:
        type_to_file[f"{prefix}{enum_type.name}"] = file_name


def build_fds_index(raw_pb_bytes: bytes, fds: FileDescriptorSet) -> bytes:
    """Build and serialize an FdsIndex from the raw .pb bytes and the decoded FDS.

    Computes type_to_file, file_to_span, and dep_graph for every file in
    the FDS (including WKT files), then calls
    scoring_graph_lib.build_fds_index() to serialize to rkyv with the
    PTSGRAPH header.

    Returns the serialized index.rkyv content as bytes.

    The FDS is assumed to be self-contained (produced with --include_imports).
    """
    try:
        from scoring_graph_lib import build_fds_index as _rust_build
    except ImportError as e:
        raise RuntimeError(
            f'build_fds_index requires the scoring_graph_lib extension: {e}'
        ) from e

    spans = extract_spans(raw_pb_bytes)
    assert len(spans) == len(fds.file), (
        f"span count {len(spans)} != FDP count {len(fds.file)}"
    )

    type_to_file: dict[str, str] = {}
    file_to_span: dict[str, tuple[int, int]] = {}
    dep_graph: dict[str, list[str]] = {}

    for i, fdp in enumerate(fds.file):
        name = fdp.name
        start, end = spans[i]
        file_to_span[name] = (start, end)
        dep_graph[name] = list(fdp.dependency)

        pkg = fdp.package
        prefix = f"{pkg}." if pkg else ""
        _collect_types(prefix, fdp, name, type_to_file)

    return bytes(_rust_build(
        type_to_file=type_to_file,
        file_to_span=file_to_span,
        dep_graph=dep_graph,
    ))


def write_fds_index(raw_pb_bytes: bytes, fds: FileDescriptorSet, out_path: Path) -> None:
    """Build the FdsIndex and write it to out_path."""
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_bytes(build_fds_index(raw_pb_bytes, fds))
