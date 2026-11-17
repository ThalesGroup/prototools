# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""A descriptor.proto the descriptor pool cannot build is refused.

The variant's descriptor.proto is what every option is read against: one
the pool refuses (here, a scalar field claiming a type name) ends the run
with an error, exit status 1, in reproto and protoback alike; not a
traceback, and not a run that writes every file without its options.
"""

import os
import subprocess
from pathlib import Path

from google.protobuf import descriptor_pb2

from .conftest import REPROTO_CMD

USER = """syntax = "proto2";
package user;
import "google/protobuf/descriptor.proto";
extend google.protobuf.FileOptions { optional int32 level = 50001; }
message U { optional int32 x = 1; }
"""


def test_an_unbuildable_descriptor_proto_is_refused(tmp_path: Path) -> None:
    src = tmp_path / "src"
    src.mkdir()
    (src / "user.proto").write_text(USER, encoding="utf-8")
    compiled = tmp_path / "compiled.pb"
    result = subprocess.run(
        ["protoc", f"-I{src}", "--include_imports", f"--descriptor_set_out={compiled}",
         "user.proto"],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, result.stderr
    fds = descriptor_pb2.FileDescriptorSet()
    fds.ParseFromString(compiled.read_bytes())

    inputs = tmp_path / "in"
    for fdp in fds.file:
        if fdp.name == "google/protobuf/descriptor.proto":
            fdm = next(m for m in fdp.message_type if m.name == "FileDescriptorProto")
            # A string field with a type name: the pool refuses the file.
            next(f for f in fdm.field if f.name == "name").type_name = ""
        path = inputs / fdp.name.replace(".proto", ".pb")
        path.parent.mkdir(parents=True, exist_ok=True)
        one = descriptor_pb2.FileDescriptorSet()
        one.file.append(fdp)
        path.write_bytes(one.SerializeToString())

    env = {k: v for k, v in os.environ.items() if k != "REPROTO_VARIANT"}
    result = subprocess.run(
        [*REPROTO_CMD, f"-I{inputs}", "-O", str(tmp_path / "out"),
         "user.pb", "google/protobuf/descriptor.pb"],
        capture_output=True, text=True, env=env,
    )
    assert result.returncode == 1, result.stderr
    assert "Traceback" not in result.stderr
    assert result.stderr.rstrip().endswith(
        "Error: descriptor.proto could not be built into a descriptor pool; "
        "it is not a valid descriptor.proto"
    ), result.stderr
