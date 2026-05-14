# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Tests for reproto.split_fdps — spec 0065."""

import pytest
from google.protobuf.descriptor_pb2 import FileDescriptorProto, FileDescriptorSet

from reproto.split_fdps import split_fdps
from reproto.tests.conftest import compile_proto


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _make_binary_fds(*names: str) -> bytes:
    fds = FileDescriptorSet()
    for name in names:
        fds.file.add(name=name)
    return fds.SerializeToString()


def _make_binary_fdp(name: str) -> bytes:
    fdp = FileDescriptorProto(name=name)
    return fdp.SerializeToString()


# ---------------------------------------------------------------------------
# Text path — FDS
# ---------------------------------------------------------------------------

class TestTextFds:
    def test_two_file_blocks(self):
        src = 'file { name: "a.proto" } file { name: "b.proto" }'
        result = split_fdps(src, '.textpb')
        assert len(result) == 2
        assert result[0][0] == 'a.proto'
        assert result[1][0] == 'b.proto'

    def test_three_file_blocks(self):
        src = ('file { name: "a.proto" } '
               'file { name: "b.proto" } '
               'file { name: "c.proto" }')
        result = split_fdps(src, '.textpb')
        assert len(result) == 3
        assert [name for name, _ in result] == ['a.proto', 'b.proto', 'c.proto']

    def test_entry_wrapper(self):
        src = 'entry { file { name: "a.proto" } file { name: "b.proto" } }'
        result = split_fdps(src, '.textpb')
        assert len(result) == 2
        assert result[0][0] == 'a.proto'
        assert result[1][0] == 'b.proto'

    def test_colon_syntax(self):
        src = 'file: { name: "a.proto" } file: { name: "b.proto" }'
        result = split_fdps(src, '.textpb')
        assert len(result) == 2
        assert result[0][0] == 'a.proto'

    def test_angle_delimiters(self):
        src = 'file < name: "a.proto" > file < name: "b.proto" >'
        result = split_fdps(src, '.textpb')
        assert len(result) == 2
        assert result[0][0] == 'a.proto'
        assert result[1][0] == 'b.proto'

    def test_interior_verbatim(self):
        # The interior must contain exactly what was between the braces.
        src = 'file { name: "a.proto"\n  package: "foo" # a comment\n}'
        result = split_fdps(src, '.textpb')
        assert len(result) == 1
        interior = result[0][1]
        assert isinstance(interior, str)
        assert 'package: "foo"' in interior
        assert '# a comment' in interior
        assert 'name: "a.proto"' in interior

    def test_nested_message_verbatim(self):
        src = 'file { name: "a.proto" message_type { name: "Msg" field { name: "f" number: 1 } } }'
        result = split_fdps(src, '.textpb')
        assert len(result) == 1
        interior = result[0][1]
        assert isinstance(interior, str)
        assert 'message_type' in interior
        assert 'field' in interior

    def test_string_with_braces(self):
        src = 'file { name: "a.proto" options { java_package: "com.{example}" } }'
        result = split_fdps(src, '.textpb')
        assert len(result) == 1
        interior = result[0][1]
        assert isinstance(interior, str)
        assert 'com.{example}' in interior


# ---------------------------------------------------------------------------
# Text path — bare FDP
# ---------------------------------------------------------------------------

class TestTextFdp:
    def test_bare_fdp(self):
        src = 'name: "a.proto"\npackage: "foo"'
        result = split_fdps(src, '.textpb')
        assert len(result) == 1
        assert result[0][0] == 'a.proto'
        # Fragment is the whole source
        assert isinstance(result[0][1], str)

    def test_single_file_block(self):
        src = 'file { name: "a.proto" }'
        result = split_fdps(src, '.textpb')
        assert len(result) == 1
        assert result[0][0] == 'a.proto'


# ---------------------------------------------------------------------------
# Binary path
# ---------------------------------------------------------------------------

class TestBinaryFds:
    def test_two_fdps(self):
        data = _make_binary_fds('a.proto', 'b.proto')
        result = split_fdps(data, '.pb')
        assert len(result) == 2
        names = [name for name, _ in result]
        assert 'a.proto' in names
        assert 'b.proto' in names
        for _, fragment in result:
            assert isinstance(fragment, bytes)
            fdp = FileDescriptorProto()
            fdp.ParseFromString(fragment)
            assert fdp.name in names

    def test_bare_fdp(self):
        data = _make_binary_fdp('a.proto')
        result = split_fdps(data, '.pb')
        assert len(result) == 1
        assert result[0][0] == 'a.proto'
        assert isinstance(result[0][1], bytes)


# ---------------------------------------------------------------------------
# Extension tiebreaker
# ---------------------------------------------------------------------------

class TestExtensionTiebreaker:
    def test_text_extension_prefers_text(self):
        # Build bytes that parse as both text and binary.
        # A trivial FDS with one FDP serialises to short bytes that won't
        # accidentally parse as valid textproto, so we test the preference
        # direction by giving a known-text input with a text extension.
        src = 'file { name: "a.proto" }'
        result = split_fdps(src, '.textpb')
        assert isinstance(result[0][1], str)

    def test_binary_extension_prefers_binary(self):
        data = _make_binary_fds('a.proto')
        result = split_fdps(data, '.pb')
        assert isinstance(result[0][1], bytes)


# ---------------------------------------------------------------------------
# Error cases
# ---------------------------------------------------------------------------

class TestErrors:
    def test_unrecognisable_raises(self):
        with pytest.raises(ValueError):
            split_fdps(b'\x00\x01\x02\x03garbage', '.pb')

    def test_empty_bytes_raises(self):
        with pytest.raises(ValueError):
            split_fdps(b'', '.pb')


# ---------------------------------------------------------------------------
# Real fixture smoke test
# ---------------------------------------------------------------------------

class TestFixtures:
    def test_address_book_binary(self, tmp_path):
        pb_paths = compile_proto(tmp_path, 'address_book.proto')
        data = pb_paths[0].read_bytes()
        result = split_fdps(data, '.pb')
        assert len(result) >= 1
        assert all(name.endswith('.proto') for name, _ in result)
