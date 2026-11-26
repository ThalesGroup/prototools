# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""The inputs of test_warning_codes.py, made by its own code (4da0f2d6),
copied verbatim but for the output paths: OUT/in/a.textpb (the `inputs`
fixture: a.proto's set, options added in Python, printed by protoc, then
edited as text) and OUT/group/b.pb (test_an_undeclared_group_is_written_with
_its_content's input). Run with a Python that has protobuf, protoc on PATH.

Usage: python3 -I -B test_warning_codes.py OUT
"""
import subprocess
import sys
from pathlib import Path

from google.protobuf import descriptor_pb2

A = """syntax = "proto2";
package a;
import "b.proto";
message A { optional b.B b = 1; }
"""
B = """syntax = "proto2";
package b;
message B { optional int32 x = 1; }
"""


def inputs(tmp_path):
    src = tmp_path / "src"
    src.mkdir(parents=True)
    (src / "a.proto").write_text(A, encoding="utf-8")
    (src / "b.proto").write_text(B, encoding="utf-8")
    compiled = tmp_path / "a.pb"
    subprocess.run(["protoc", f"-I{src}", f"--descriptor_set_out={compiled}", "a.proto"],
                   check=True, capture_output=True)
    fds = descriptor_pb2.FileDescriptorSet()
    fds.ParseFromString(compiled.read_bytes())
    fds.file[0].options.MergeFromString(b"\x98\x06\x05")   # field 99 = 5
    text = subprocess.run(
        ["protoc", "--decode=google.protobuf.FileDescriptorSet", "google/protobuf/descriptor.proto"],
        input=fds.SerializeToString(), capture_output=True, check=True,
    ).stdout.decode()
    text = text.replace("  options {\n", "  options {\n    no_such_option: 1\n", 1)
    d = tmp_path / "in"
    d.mkdir()
    (d / "a.textpb").write_text(text, encoding="utf-8")
    return d


def group(tmp_path):
    src = tmp_path / "src"
    src.mkdir(parents=True)
    (src / "b.proto").write_text(B, encoding="utf-8")
    compiled = tmp_path / "b.pb"
    subprocess.run(["protoc", f"-I{src}", f"--descriptor_set_out={compiled}", "b.proto"],
                   check=True, capture_output=True)
    fds = descriptor_pb2.FileDescriptorSet()
    fds.ParseFromString(compiled.read_bytes())
    fds.file[0].options.MergeFromString(
        bytes([0x3b, 0x08, 0x05, 0x12, 0x01, 0x78, 0x3c, 0x98, 0x06, 0x05]))
    d = tmp_path / "in"
    d.mkdir()
    (d / "b.pb").write_bytes(fds.SerializeToString())
    return d


out = Path(sys.argv[1])
print(inputs(out / "inputs"), group(out / "group"))
