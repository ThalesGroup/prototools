# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""reproto-instantiate-schema's instances (spec 0432 S5, S6).

An instance is serialized deterministically, its map entries sorted by
key: upb seeds its string hash per process, so a map's hash-table order
varied from one run to the next. A singular group is set, as a singular
message is.
"""

import functools
import subprocess
import sys
from pathlib import Path

from google.protobuf import descriptor_pb2, message_factory

from reproto.instantiate import generate_instance, load_pool


def _schema(tmp_path: Path) -> Path:
    """A proto2 file: a map of string keys, a singular and a repeated group."""
    f = descriptor_pb2.FileDescriptorProto()
    f.name = 'inst.proto'
    f.package = 'inst'
    f.syntax = 'proto2'
    m = f.message_type.add()
    m.name = 'M'
    entry = m.nested_type.add()
    entry.name = 'LabelsEntry'
    entry.options.map_entry = True
    for name, number in (('key', 1), ('value', 2)):
        e = entry.field.add()
        e.name, e.number = name, number
        e.label, e.type = e.LABEL_OPTIONAL, e.TYPE_STRING
    labels = m.field.add()
    labels.name, labels.number = 'labels', 1
    labels.label, labels.type = labels.LABEL_REPEATED, labels.TYPE_MESSAGE
    labels.type_name = '.inst.M.LabelsEntry'
    group = m.nested_type.add()
    group.name = 'G'
    x = group.field.add()
    x.name, x.number = 'x', 1
    x.label, x.type = x.LABEL_OPTIONAL, x.TYPE_INT32
    for name, number, label in (
        ('single', 2, descriptor_pb2.FieldDescriptorProto.LABEL_OPTIONAL),
        ('many', 3, descriptor_pb2.FieldDescriptorProto.LABEL_REPEATED),
    ):
        g = m.field.add()
        g.name, g.number, g.label = name, number, label
        g.type, g.type_name = g.TYPE_GROUP, '.inst.M.G'
    fds = descriptor_pb2.FileDescriptorSet()
    fds.file.append(f)
    path = tmp_path / 'inst.desc'
    path.write_bytes(fds.SerializeToString())
    return path


def _generate_in_a_fresh_process(desc: Path) -> bytes:
    code = (
        'import sys; from pathlib import Path\n'
        'from reproto.instantiate import generate_instance, load_pool\n'
        'pool = load_pool(Path(sys.argv[1]))\n'
        "sys.stdout.buffer.write(generate_instance('inst.M', pool, seed=3, "
        'max_repeated=8))\n'
    )
    return subprocess.run([sys.executable, '-c', code, str(desc)],
                          capture_output=True, check=True).stdout


def test_instances_are_reproducible(tmp_path: Path) -> None:
    desc = _schema(tmp_path)
    runs = {_generate_in_a_fresh_process(desc) for _ in range(8)}
    assert len(runs) == 1, f'{len(runs)} different instances over 8 runs'


def test_map_entries_are_in_upbs_deterministic_order(tmp_path: Path) -> None:
    pool = load_pool(_schema(tmp_path))
    wire = generate_instance('inst.M', pool, seed=3, max_repeated=8)
    msg = message_factory.GetMessageClass(pool.FindMessageTypeByName('inst.M'))()
    msg.ParseFromString(wire)
    assert len(getattr(msg, 'labels')) >= 2, 'the test needs a map of several keys'
    keys = [_string_field(entry, 1) for entry in _entries(wire, field=1)]
    assert keys == sorted(keys, key=functools.cmp_to_key(_upb_order))


def _upb_order(a: bytes, b: bytes) -> int:
    """upb's deterministic order for string keys: ascending, but a key that
    is a prefix of another after it (its comparator negates memcmp, not the
    length tie-break, and its encoder writes the sorted entries back to
    front)."""
    common = min(len(a), len(b))
    if a[:common] != b[:common]:
        return -1 if a[:common] < b[:common] else 1
    return len(b) - len(a)


def test_a_singular_group_is_set(tmp_path: Path) -> None:
    pool = load_pool(_schema(tmp_path))
    wire = generate_instance('inst.M', pool, seed=3)
    # Field 2, wire type 3 (start group): tag 0x13.
    assert b'\x13' in wire


def _varint(b: bytes, i: int) -> tuple[int, int]:
    n, shift = 0, 0
    while True:
        c = b[i]
        i += 1
        n |= (c & 0x7F) << shift
        if not c & 0x80:
            return n, i
        shift += 7


def _entries(wire: bytes, field: int) -> list[bytes]:
    """The length-delimited records of `field` at the top level, in order."""
    out, i = [], 0
    while i < len(wire):
        tag, i = _varint(wire, i)
        number, wt = tag >> 3, tag & 7
        if wt == 0:
            _, i = _varint(wire, i)
        elif wt == 2:
            n, i = _varint(wire, i)
            if number == field:
                out.append(wire[i:i + n])
            i += n
        elif wt == 1:
            i += 8
        elif wt == 5:
            i += 4
        elif wt in (3, 4):
            pass
    return out


def _string_field(record: bytes, field: int) -> bytes:
    values = _entries(record, field)
    return values[0] if values else b''


def test_the_sets_own_descriptor_proto_is_used(tmp_path: Path) -> None:
    """A set's own descriptor.proto, not the one Python protobuf carries
    (spec 0432 S3): instances follow the schema given."""
    own = descriptor_pb2.FileDescriptorProto()
    descriptor_pb2.DESCRIPTOR.CopyToProto(own)
    # A descriptor.proto Python's does not match: an Edition value more.
    edition = next(e for e in own.enum_type if e.name == 'Edition')
    extra = edition.value.add()
    extra.name, extra.number = 'EDITION_TEST_EXTRA', 123456
    fds = descriptor_pb2.FileDescriptorSet()
    fds.file.append(own)
    path = tmp_path / 'own.desc'
    path.write_bytes(fds.SerializeToString())
    pool = load_pool(path)
    values = [v.name for v in pool.FindEnumTypeByName('google.protobuf.Edition').values]
    assert 'EDITION_TEST_EXTRA' in values
