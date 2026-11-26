# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""The input of test_pruned_dependency.py's
test_stripped_import_keeps_public_and_weak_on_their_files, made by its own
`_file` and `_write_set` (4da0f2d6), copied verbatim but for the output
path: OUT/set.pb. Run with a Python that has protobuf.

Usage: python3 -I -B test_pruned_dependency.py OUT
"""
import sys
from collections.abc import Sequence
from pathlib import Path

from google.protobuf.descriptor_pb2 import FileDescriptorProto, FileDescriptorSet


def _write_set(path, *fdps):
    fds = FileDescriptorSet()
    for fdp in fdps:
        fds.file.add().CopyFrom(fdp)
    path.write_bytes(fds.SerializeToString())
    return path


def _file(name: str, message: str, deps: Sequence[str] = ()) -> FileDescriptorProto:
    fdp = FileDescriptorProto(name=name, package="strip_test", syntax="proto2")
    fdp.dependency.extend(deps)
    fdp.message_type.add(name=message)
    return fdp


tmp_path = Path(sys.argv[1])
tmp_path.mkdir(parents=True, exist_ok=True)
importer = _file("importer.proto", "Importer", ["missing.proto", "weak.proto", "public.proto"])
importer.weak_dependency.append(1)
importer.public_dependency.append(2)
print(_write_set(tmp_path / "set.pb", _file("weak.proto", "Weak"), _file("public.proto", "Public"), importer))
