# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""The options types come from the variant's own descriptor.proto.

A variant names its descriptor.proto by path (`descriptor_proto`); the
package that file declares is the package of the options types reproto
parses options with, and of the nine `*Options` a proto3 file may extend
(A5). Neither is assumed from the path: here the variant's descriptor.proto
is `alt/descriptor.proto`, `package altpb`.
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

from google.protobuf import descriptor_pb2
from google.protobuf.descriptor_pb2 import (
    FieldDescriptorProto,
    FileDescriptorProto,
    FileDescriptorSet,
)

from reproto.tests.conftest import REPROTO_CMD

_NAME = "alt/descriptor.proto"
_PACKAGE = "altpb"


def _alt_descriptor() -> FileDescriptorProto:
    """descriptor.proto, renamed and repackaged, its references with it."""
    fdp = FileDescriptorProto()
    fdp.ParseFromString(descriptor_pb2.DESCRIPTOR.serialized_pb)
    fdp.name = _NAME
    fdp.package = _PACKAGE

    def walk(messages) -> None:
        for m in messages:
            for f in m.field:
                if f.type_name.startswith(".google.protobuf."):
                    f.type_name = f".{_PACKAGE}." + f.type_name[len(".google.protobuf."):]
            walk(m.nested_type)

    walk(fdp.message_type)
    return fdp


def _set(tmp: Path) -> Path:
    use = FileDescriptorProto(name="use.proto", package="use", syntax="proto2",
                              dependency=[_NAME])
    m = use.message_type.add(name="M")
    field = m.field.add(name="s", number=1, type=FieldDescriptorProto.TYPE_STRING,
                        label=FieldDescriptorProto.LABEL_OPTIONAL)
    field.options.deprecated = True

    ext3 = FileDescriptorProto(name="ext3.proto", package="ext3", syntax="proto3",
                               dependency=[_NAME])
    ext3.extension.add(name="sensitivity", number=50001,
                       type=FieldDescriptorProto.TYPE_INT32,
                       label=FieldDescriptorProto.LABEL_OPTIONAL,
                       extendee=f".{_PACKAGE}.FieldOptions")

    root = tmp / "in"
    root.mkdir()
    fds = FileDescriptorSet()
    fds.file.extend([_alt_descriptor(), use, ext3])
    pb = root / "set.pb"
    pb.write_bytes(fds.SerializeToString())
    (tmp / "alt.yaml").write_text(f"name: alt\ndescriptor_proto: {_NAME}\n",
                                  encoding="utf-8")
    return pb


def _reproto(tmp: Path) -> tuple[subprocess.CompletedProcess[str], Path]:
    pb = _set(tmp)
    out = tmp / "out"
    env = {**os.environ}
    env.pop("REPROTO_VARIANT", None)
    result = subprocess.run(
        [*REPROTO_CMD, "--proto-variant", str(tmp / "alt.yaml"), f"-I{pb.parent}",
         f"--proto-out={out}", "set.pb"],
        capture_output=True, text=True, env=env,
    )
    return result, out


def test_options_parse_with_the_variants_own_package(tmp_path: Path) -> None:
    result, out = _reproto(tmp_path)
    assert result.returncode == 0, result.stderr
    text = (out / "use.proto").read_text(encoding="utf-8")
    assert "[deprecated = true]" in text


def test_a_proto3_file_may_extend_the_variants_options(tmp_path: Path) -> None:
    """A5 allows extending the nine *Options of the input's own
    descriptor.proto, whatever its package."""
    result, out = _reproto(tmp_path)
    assert result.returncode == 0, result.stderr
    text = (out / "ext3.proto").read_text(encoding="utf-8")
    assert "not valid in proto3" not in text
    assert "not valid in proto3" not in " ".join(result.stderr.split())
    assert "FieldOptions {" in text
    assert "int32 sensitivity = 50001;" in text
