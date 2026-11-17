# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""A proto3 `optional` field written as proto2 (spec 0429 S13).

Under --force-proto2-output, proto3's `optional` is proto2's: a field with
presence, no synthetic oneof. The text says `optional int32 x = 1;`, and
the descriptor written with -b is the one protoc compiles from that text:
no `proto3_optional` (which protoc refuses in proto2), no synthetic oneof,
the real oneofs kept.
"""

import os
import subprocess
from pathlib import Path

from google.protobuf import descriptor_pb2

from .conftest import REPROTO_CMD

SOURCE = """syntax = "proto3";
package p3;
message M {
  optional int32 x = 1;
  int32 y = 2;
  oneof choice {
    string a = 3;
    int64 b = 4;
  }
  optional string z = 5;
}
"""


def test_proto3_optional_is_proto2_optional(tmp_path: Path) -> None:
    src = tmp_path / "src"
    src.mkdir()
    (src / "m.proto").write_text(SOURCE, encoding="utf-8")
    original = tmp_path / "m.pb"
    subprocess.run(["protoc", f"-I{src}", f"--descriptor_set_out={original}", "m.proto"],
                   check=True, capture_output=True)
    out = tmp_path / "out"
    env = {k: v for k, v in os.environ.items() if k != "REPROTO_VARIANT"}
    result = subprocess.run(
        [*REPROTO_CMD, "-q", "-b", "--no-source-info", "--force-proto2-output",
         "--use-variant", "descriptor", f"-I{tmp_path}", "-O", str(out), "m.pb"],
        capture_output=True, text=True, env=env,
    )
    assert result.returncode == 0, result.stderr

    text = (out / "m.proto").read_text(encoding="utf-8")
    assert "optional int32 x = 1;" in text
    assert "optional string z = 5;" in text
    assert "oneof _" not in text
    assert "oneof choice {" in text

    # The emitted descriptor is the one protoc compiles from the text.
    again = tmp_path / "again.pb"
    compiled = subprocess.run(
        ["protoc", f"-I{out}", f"--descriptor_set_out={again}", "m.proto"],
        capture_output=True, text=True,
    )
    assert compiled.returncode == 0, compiled.stderr
    fds = descriptor_pb2.FileDescriptorSet()
    fds.ParseFromString(again.read_bytes())
    emitted = descriptor_pb2.FileDescriptorProto()
    emitted.ParseFromString((out / "m.pb").read_bytes())
    assert emitted.message_type[0] == fds.file[0].message_type[0], (
        f"emitted:\n{emitted.message_type[0]}\nprotoc:\n{fds.file[0].message_type[0]}"
    )
