# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Tests for synthesized SourceCodeInfo in --emit-binary output (spec 0141).

Step 1: message-level (top-level and nested) Location path + span synthesis.
Step 2: enum, field (including extension), service and method Location
synthesis. All gated by --source-info/--no-source-info.
"""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest
from google.protobuf.descriptor_pb2 import (
    DescriptorProto,
    EnumDescriptorProto,
    FileDescriptorProto,
    ServiceDescriptorProto,
)

from reproto.tests.test_emit_binary import _build_env, _get_fixture_path
from reproto.tests.conftest import REPROTO_CMD

FIXTURE = "message_comprehensive.proto"


def _run_reproto(orig_dir: Path, out_dir: Path, extra_args: list[str]) -> None:
    cmd = [
        *REPROTO_CMD,
        "--use-variant", "descriptor",
        "--emit-binary",
        f"-I{orig_dir}",
        f"--proto-out={out_dir}",
        *extra_args,
    ]
    r = subprocess.run(cmd, capture_output=True, text=True, env=_build_env())
    assert r.returncode == 0, f"reproto failed: {r.stderr}"


def _render(
    tmp_path: Path, extra_args: list[str], fixture: str = FIXTURE,
) -> tuple[FileDescriptorProto, list[str]]:
    orig_dir = tmp_path / "orig"
    out_dir = tmp_path / "out"
    orig_dir.mkdir()
    out_dir.mkdir()

    proto_path = orig_dir / fixture
    proto_path.write_text(
        _get_fixture_path(fixture).read_text(encoding="utf-8"), encoding="utf-8"
    )

    pb_path = orig_dir / proto_path.with_suffix(".pb").name
    r = subprocess.run(
        ["protoc", f"--descriptor_set_out={pb_path}", f"-I{orig_dir}", str(proto_path)],
        capture_output=True, text=True,
    )
    assert r.returncode == 0, f"protoc failed: {r.stderr}"

    _run_reproto(orig_dir, out_dir, [*extra_args, str(pb_path)])

    stem = Path(fixture).stem
    fdp = FileDescriptorProto()
    fdp.ParseFromString((out_dir / f"{stem}.pb").read_bytes())
    proto_lines = (out_dir / f"{stem}.proto").read_text(
        encoding="utf-8"
    ).splitlines()
    return fdp, proto_lines


def _message_name_at_path(fdp: FileDescriptorProto, path: list[int]) -> str:
    """Resolve a message-only SourceCodeInfo path ([4,i] (,[3,j])*) to a name."""
    assert path[0] == 4, f"unexpected top-level path field number: {path}"
    node = fdp.message_type[path[1]]
    i = 2
    while i < len(path):
        assert path[i] == 3, f"unexpected nested path field number: {path}"
        node = node.nested_type[path[i + 1]]
        i += 2
    return node.name


def _is_message_only_path(path: list[int]) -> bool:
    """Whether `path` is a pure message chain: [4,i] (,[3,j])*."""
    if not path or path[0] != 4:
        return False
    return all(path[i] == 3 for i in range(2, len(path), 2))


def _resolve_breadcrumb(fdp: FileDescriptorProto, path: list[int]) -> str:
    """Resolve any spec-0141 SourceCodeInfo path to a dotted breadcrumb.

    Understands every path shape synthesized by G1/G2/G3/G8: messages
    (nested arbitrarily deep), enums, enum values, fields, extensions
    (file- and message-level), services and methods.
    """
    assert path and len(path) % 2 == 0, f"malformed path: {path}"
    field_num, idx = path[0], path[1]
    if field_num == 4:  # FileDescriptorProto.message_type
        node = fdp.message_type[idx]
        return f'message:{node.name}' + _resolve_message_tail(node, path[2:])
    if field_num == 5:  # FileDescriptorProto.enum_type
        node = fdp.enum_type[idx]
        return f'enum:{node.name}' + _resolve_enum_tail(node, path[2:])
    if field_num == 6:  # FileDescriptorProto.service
        node = fdp.service[idx]
        return f'service:{node.name}' + _resolve_service_tail(node, path[2:])
    if field_num == 7:  # FileDescriptorProto.extension
        assert not path[2:], f"unexpected tail after file extension: {path}"
        return f'extension:{fdp.extension[idx].name}'
    raise AssertionError(f"unexpected top-level path field number: {path}")


def _resolve_message_tail(node: DescriptorProto, rest: list[int]) -> str:
    if not rest:
        return ''
    field_num, idx = rest[0], rest[1]
    if field_num == 2:  # DescriptorProto.field
        assert not rest[2:], f"unexpected tail after field: {rest}"
        return f'.field:{node.field[idx].name}'
    if field_num == 3:  # DescriptorProto.nested_type
        nested = node.nested_type[idx]
        return f'.message:{nested.name}' + _resolve_message_tail(nested, rest[2:])
    if field_num == 4:  # DescriptorProto.enum_type
        enum = node.enum_type[idx]
        return f'.enum:{enum.name}' + _resolve_enum_tail(enum, rest[2:])
    if field_num == 6:  # DescriptorProto.extension
        assert not rest[2:], f"unexpected tail after extension: {rest}"
        return f'.extension:{node.extension[idx].name}'
    raise AssertionError(f"unexpected nested path field number: {rest}")


def _resolve_enum_tail(node: EnumDescriptorProto, rest: list[int]) -> str:
    if not rest:
        return ''
    field_num, idx = rest[0], rest[1]
    assert field_num == 2, f"unexpected enum tail field number: {rest}"  # value
    assert not rest[2:], f"unexpected tail after enum value: {rest}"
    return f'.value:{node.value[idx].name}'


def _resolve_service_tail(node: ServiceDescriptorProto, rest: list[int]) -> str:
    if not rest:
        return ''
    field_num, idx = rest[0], rest[1]
    assert field_num == 2, f"unexpected service tail field number: {rest}"  # method
    assert not rest[2:], f"unexpected tail after method: {rest}"
    return f'.method:{node.method[idx].name}'


def _span_lines(span: list[int]) -> tuple[int, int, int, int]:
    """Return (start_line, start_col, end_line, end_col) from a 3- or 4-elem span."""
    if len(span) == 3:
        start_line, start_col, end_col = span
        return start_line, start_col, start_line, end_col
    start_line, start_col, end_line, end_col = span
    return start_line, start_col, end_line, end_col


def _kind_and_name(breadcrumb: str) -> tuple[str, str]:
    """Split a breadcrumb's last segment into (kind, name)."""
    last = breadcrumb.rsplit('.', 1)[-1]
    kind, name = last.split(':', 1)
    return kind, name


def _assert_open_close(
    proto_lines: list[str], span: list[int], expected_open_prefix: str, label: str,
) -> None:
    """Assert a `kind Name {` ... `}` Location span (message/enum/service)."""
    start_line, start_col, end_line, end_col = _span_lines(span)
    open_text = proto_lines[start_line]
    assert open_text[start_col:].startswith(expected_open_prefix), (
        f"{label}: open line {start_line} doesn't start with "
        f"{expected_open_prefix!r} at col {start_col}: {open_text!r}"
    )
    close_text = proto_lines[end_line]
    assert close_text[:end_col].rstrip().endswith('}'), (
        f"{label}: close line {end_line} doesn't end with '}}' "
        f"before col {end_col}: {close_text!r}"
    )


def _assert_method_span(
    proto_lines: list[str], span: list[int], name: str, label: str,
) -> None:
    """Assert a method Location span: `rpc Name(...` ending in ';' or '}'."""
    start_line, start_col, end_line, end_col = _span_lines(span)
    open_text = proto_lines[start_line]
    assert open_text[start_col:].startswith(f"rpc {name}("), (
        f"{label}: open line {start_line} doesn't start with "
        f"'rpc {name}(' at col {start_col}: {open_text!r}"
    )
    close_text = proto_lines[end_line]
    trimmed = close_text[:end_col].rstrip()
    assert trimmed.endswith(';') or trimmed.endswith('}'), (
        f"{label}: close line {end_line} doesn't end with ';' or '}}' "
        f"before col {end_col}: {close_text!r}"
    )


def _assert_leaf_span(
    proto_lines: list[str], span: list[int], name: str, label: str,
) -> None:
    """Assert a field/value/extension Location span.

    Its open line must contain the node's own name, and its close line
    must end in ';' (regular statement) or '}' (group field body).

    Group fields have a lower-cased FieldDescriptorProto.name while the
    source text uses the (capitalized) group type name, hence the
    case-insensitive comparison.
    """
    start_line, start_col, end_line, end_col = _span_lines(span)
    open_text = proto_lines[start_line]
    assert name.lower() in open_text[start_col:].lower(), (
        f"{label}: open line {start_line} doesn't contain {name!r} "
        f"at col {start_col}: {open_text!r}"
    )
    close_text = proto_lines[end_line]
    trimmed = close_text[:end_col].rstrip()
    assert trimmed.endswith(';') or trimmed.endswith('}'), (
        f"{label}: close line {end_line} doesn't end with ';' or '}}' "
        f"before col {end_col}: {close_text!r}"
    )


@pytest.mark.roundtrip
def test_source_info_message_locations(tmp_path: Path) -> None:
    fdp, proto_lines = _render(tmp_path, [])

    assert fdp.source_code_info.location, "expected synthesized source_code_info"

    seen_names: set[str] = set()
    for loc in fdp.source_code_info.location:
        path = list(loc.path)
        if not _is_message_only_path(path):
            continue
        name = _message_name_at_path(fdp, path)
        seen_names.add(name)

        span = list(loc.span)
        if len(span) == 3:
            start_line, start_col, end_col = span
            end_line = start_line
        else:
            start_line, start_col, end_line, end_col = span

        open_text = proto_lines[start_line]
        assert open_text[start_col:].startswith(f"message {name} "), (
            f"{name}: open line {start_line} doesn't start with "
            f"'message {name} ' at col {start_col}: {open_text!r}"
        )
        close_text = proto_lines[end_line]
        assert close_text[:end_col].rstrip().endswith('}'), (
            f"{name}: close line {end_line} doesn't end with '}}' "
            f"before col {end_col}: {close_text!r}"
        )

    # OuterMessage > MiddleMessage > InnerMessage exercises three levels of
    # nesting; EmptyMessage exercises the single-line body-collapse case.
    for expected in ("OuterMessage", "MiddleMessage", "InnerMessage", "EmptyMessage"):
        assert expected in seen_names, f"missing Location for {expected}"


@pytest.mark.roundtrip
@pytest.mark.parametrize(
    ("fixture", "expected_breadcrumbs"),
    [
        (
            "enum.proto",
            (
                "enum:AllEnumFeatures",
                "enum:AllEnumFeatures.value:VALUE_ZERO",
                "enum:AllEnumFeatures.value:VALUE_DEPRECATED",
                "enum:ExtendedEnum",
                "enum:EmptyEnum",
                "extension:example_enum_option",
            ),
        ),
        (
            "enum_value.proto",
            (
                "enum:EnumValueOptionsTest.value:ZERO",
                "enum:EnumValueOptionsTest.value:THREE",
                "enum:EnumValueOptionsTest.value:COMPREHENSIVE",
                "extension:example_value_option",
                "extension:value_number_option",
                "extension:value_tags",
            ),
        ),
        (
            "service.proto",
            (
                "service:BasicService",
                "service:BasicService.method:SimpleRpc",
                "service:StreamingService.method:ClientStream",
                "service:OptionsService.method:DeprecatedMethod",
                "service:DeprecatedService.method:OldMethod",
                "service:EmptyService",
                "message:Request.field:query",
            ),
        ),
        (
            "message_comprehensive.proto",
            (
                "message:ComprehensiveMessage.field:required_int32",
                "message:ComprehensiveMessage.field:repeatedgroup",
                "message:ComprehensiveMessage.extension:extension_field1",
                "message:ComprehensiveMessage.enum:NestedEnum",
                "message:ComprehensiveMessage.enum:NestedEnum.value:NESTED_FIRST",
                "message:OnlyExtensions.extension:extension_self",
                "message:ComprehensiveMessage.message:NestedMessage"
                ".message:DeeplyNested",
            ),
        ),
    ],
)
def test_source_info_g2_g3_g8_locations(
    tmp_path: Path, fixture: str, expected_breadcrumbs: tuple[str, ...],
) -> None:
    """Step 2: enum, field/extension, service and method Locations."""
    fdp, proto_lines = _render(tmp_path, [], fixture=fixture)

    assert fdp.source_code_info.location, "expected synthesized source_code_info"

    seen: set[str] = set()
    for loc in fdp.source_code_info.location:
        breadcrumb = _resolve_breadcrumb(fdp, list(loc.path))
        seen.add(breadcrumb)
        kind, name = _kind_and_name(breadcrumb)
        span = list(loc.span)

        if kind in ('message', 'enum', 'service'):
            _assert_open_close(proto_lines, span, f'{kind} {name} ', breadcrumb)
        elif kind == 'method':
            _assert_method_span(proto_lines, span, name, breadcrumb)
        else:
            assert kind in ('field', 'value', 'extension'), \
                f"unexpected kind: {kind}"
            _assert_leaf_span(proto_lines, span, name, breadcrumb)

    for expected in expected_breadcrumbs:
        assert expected in seen, f"missing Location for {expected}"


@pytest.mark.roundtrip
def test_source_info_disabled(tmp_path: Path) -> None:
    fdp, _ = _render(tmp_path, ["--no-source-info"])
    assert not fdp.HasField("source_code_info")
