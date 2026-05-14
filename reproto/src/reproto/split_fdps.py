# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Unified FDP splitter — spec 0065.

Accepts raw file contents (text or binary), detects the encoding and proto
type, and returns a list of (name, fragment) pairs — one per FileDescriptorProto.

Text fragments are verbatim slices of the original source (no reserialization).
Binary fragments are serialised FileDescriptorProto bytes.
"""

import logging
from collections.abc import Sequence as _Sequence

import prototext_codec_lib as _pt_codec
from google.protobuf import text_format
from google.protobuf.descriptor_pb2 import FileDescriptorProto, FileDescriptorSet
from google.protobuf.message import DecodeError
from tree_sitter import Language, Node, Parser

import textproto as _textproto_mod

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Type alias
# ---------------------------------------------------------------------------

FdpFragments = _Sequence[tuple[str, str | bytes]]

# ---------------------------------------------------------------------------
# Grammar — initialised once per process
# ---------------------------------------------------------------------------

_language: Language | None = None
_parser: Parser | None = None


def _get_parser() -> Parser:
    global _language, _parser
    if _parser is None:
        _language = Language(_textproto_mod.language())
        _parser = Parser(_language)
    return _parser


# ---------------------------------------------------------------------------
# AST helpers
# ---------------------------------------------------------------------------

def _field_name(field_node: Node) -> bytes:
    """Return the raw field-name bytes for a `field` AST node."""
    # field -> (scalar_field | message_field) -> field_name -> identifier
    inner = field_node.children[0]          # scalar_field or message_field
    fn_node = inner.children[0]             # field_name
    ident = fn_node.children[0]             # identifier
    return _src_bytes[ident.start_byte:ident.end_byte]


def _message_value(field_node: Node) -> Node | None:
    """Return the `message_value` child of a message_field, or None."""
    inner = field_node.children[0]  # message_field
    for child in inner.children:
        if child.type == 'message_value':
            return child
    return None


def _scalar_string_value(field_node: Node) -> str | None:
    """Return the unquoted string value of a scalar string field, or None."""
    inner = field_node.children[0]  # scalar_field
    for child in inner.children:
        if child.type == 'scalar_value':
            # scalar_value -> string -> (double_string | single_string)
            # -> (double_string_contents | single_string_contents)
            str_node = child.children[0]
            if not str_node.children:
                return None
            quote_node = str_node.children[0]  # double_string or single_string
            if not quote_node.children:
                return None
            # children: open-quote, contents, close-quote
            if len(quote_node.children) < 3:
                return None
            contents = quote_node.children[1]
            return _src_bytes[contents.start_byte:contents.end_byte].decode()
    return None


def _find_name_in_message(message_node: Node) -> str | None:
    """Search the direct children of a `message` node for a `name` scalar."""
    for field_node in message_node.children:
        if field_node.type != 'field':
            continue
        if _field_name(field_node) == b'name':
            return _scalar_string_value(field_node)
    return None


def _interior(msg_value_node: Node) -> str:
    """Verbatim interior of a message_value node (exclusive of delimiters)."""
    # skip open delimiter (1 byte) and close delimiter (1 byte)
    return _src_bytes[msg_value_node.start_byte + 1:
                      msg_value_node.end_byte - 1].decode()


# Module-level source bytes — set before each parse call.
_src_bytes: bytes = b''


# ---------------------------------------------------------------------------
# Text-path scanner
# ---------------------------------------------------------------------------

def _split_text(src: str) -> list[tuple[str, str]]:
    """Parse a textproto FDS/FDP and return (name, interior) pairs."""
    global _src_bytes
    _src_bytes = src.encode()
    parser = _get_parser()
    tree = parser.parse(_src_bytes)
    root = tree.root_node

    # Determine the search level: unwrap `entry { … }` if present.
    search_node = root  # default: top-level message

    # Check if the sole top-level field is `entry`
    top_fields = [c for c in root.children if c.type == 'field']
    if (len(top_fields) == 1
            and _field_name(top_fields[0]) == b'entry'):
        msg_val = _message_value(top_fields[0])
        if msg_val is not None:
            # Descend into the entry body
            for child in msg_val.children:
                if child.type == 'message':
                    search_node = child
                    break

    # Collect all `file` fields within search_node
    results: list[tuple[str, str]] = []
    for field_node in search_node.children:
        if field_node.type != 'field':
            continue
        if _field_name(field_node) != b'file':
            continue
        msg_val = _message_value(field_node)
        if msg_val is None:
            continue
        # Find the inner message node to extract `name`
        inner_msg = None
        for child in msg_val.children:
            if child.type == 'message':
                inner_msg = child
                break
        if inner_msg is None:
            continue
        name = _find_name_in_message(inner_msg)
        if name is None:
            continue
        results.append((name, _interior(msg_val)))

    # Bare FDP: no `file` fields found — try treating the whole thing as an FDP
    if not results:
        name = _find_name_in_message(search_node)
        if name is not None:
            # Interior is the whole source (no outer delimiters to strip)
            results.append((name, src))

    return results


# ---------------------------------------------------------------------------
# Binary-path splitter
# ---------------------------------------------------------------------------

def _split_binary(data: bytes) -> list[tuple[str, bytes]]:
    """Split binary FDS/FDP bytes into (name, fragment) pairs."""
    # Try FileDescriptorSet first
    fds = FileDescriptorSet()
    try:
        fds.ParseFromString(data)
        if fds.file:
            return [(fdp.name, fdp.SerializeToString()) for fdp in fds.file
                    if fdp.name]
    except (DecodeError, ValueError):
        pass

    # Try bare FileDescriptorProto
    fdp = FileDescriptorProto()
    try:
        fdp.ParseFromString(data)
        if fdp.name:
            return [(fdp.name, data)]
    except (DecodeError, ValueError):
        pass

    raise ValueError("binary data is neither a FileDescriptorSet nor a FileDescriptorProto")


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------

TEXT_EXTENSIONS = {'.textpb', '.pbtxt', '.prototxt', '.ascii_proto'}
BINARY_EXTENSIONS = {'.pb', '.binpb', '.pbset', '.protoset', '.desc'}


def split_fdps(contents: str | bytes, ext: str) -> FdpFragments:
    """Split raw file contents into a list of (name, fragment) FDP pairs.

    Args:
        contents: raw file contents — str for text files, bytes for binary.
        ext: file extension (e.g. ".textpb", ".pb") used as tiebreaker when
             both text and binary parses succeed.

    Returns:
        list of (fdp_name, fragment) — fragment is str (text path) or bytes
        (binary path).  Always non-empty on success.

    Raises:
        ValueError: if the input cannot be identified as a valid FDS or FDP.
    """
    # --- Step 1: handle #@ prototext: protoc encoding ---
    if isinstance(contents, bytes):
        try:
            contents = _pt_codec.format_as_bytes(contents)
        except Exception:
            pass  # not encoded; proceed with raw bytes

    # --- Step 2: attempt text and binary parses ---
    text_result: list[tuple[str, str]] | None = None
    binary_result: list[tuple[str, bytes]] | None = None

    if isinstance(contents, str):
        try:
            text_result = _split_text(contents)
            if not text_result:
                text_result = None
        except Exception:
            pass
    else:
        # Try text parse on decoded bytes
        try:
            as_str = contents.decode('utf-8', errors='strict')
            text_result = _split_text(as_str)
            if not text_result:
                text_result = None
        except Exception:
            pass

        try:
            binary_result = _split_binary(contents)
            if not binary_result:
                binary_result = None
        except Exception:
            pass

    # --- Step 3: resolve ambiguity via extension ---
    if text_result is not None and binary_result is not None:
        if ext in TEXT_EXTENSIONS:
            binary_result = None
        else:
            text_result = None

    if text_result is not None:
        return text_result
    if binary_result is not None:
        return binary_result

    # str input that produced no results
    if isinstance(contents, str):
        # try as text FDP/FDS via text_format as a last resort
        for proto in (FileDescriptorSet(), FileDescriptorProto()):
            try:
                msg = text_format.Parse(
                    contents, proto,
                    allow_unknown_field=True,
                    allow_unknown_extension=True,
                )
                if isinstance(msg, FileDescriptorSet) and msg.file:
                    return [(fdp.name, fdp.SerializeToString())
                            for fdp in msg.file if fdp.name]
                if isinstance(msg, FileDescriptorProto) and msg.name:
                    return [(msg.name, contents)]
            except Exception:
                pass

    raise ValueError(
        f"input with extension {ext!r} is not a recognised FDS or FDP"
    )
