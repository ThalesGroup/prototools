# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""The inputs of test_rewrite_collisions.py, made by its own `_inputs`
(4da0f2d6), copied verbatim but for the output directory: written to
OUT/{in-binary,in-text}/, then rendered as #@prototext fixtures by
prototext decode. Also says whether the stock copy's bytes are protoc's
own for descriptor.proto alone. Run with a Python that has protobuf.

Usage: python3 -I -B test_rewrite_collisions.py OUT
"""
import subprocess
import sys
from pathlib import Path
from google.protobuf import descriptor_pb2

ROOT = "internal/proto2/descriptor.proto"
STOCK = "google/protobuf/descriptor.proto"
USER = """syntax = "proto2";
package user;
option java_package = "com.example.user";
message U { optional int32 x = 1; }
"""


def _write(inputs, fdp, as_text=False):
    fds = descriptor_pb2.FileDescriptorSet()
    fds.file.append(fdp)
    path = inputs / fdp.name.replace(".proto", ".textpb" if as_text else ".pb")
    path.parent.mkdir(parents=True, exist_ok=True)
    if as_text:
        path.write_bytes(subprocess.run(
            ["protoc", "--decode=google.protobuf.FileDescriptorSet",
             "google/protobuf/descriptor.proto"],
            input=fds.SerializeToString(), capture_output=True, check=True,
        ).stdout)
    else:
        path.write_bytes(fds.SerializeToString())


def _inputs(tmp, user_options, as_text):
    src = tmp / "src"
    src.mkdir(parents=True)
    (src / "user.proto").write_text(USER, encoding="utf-8")
    compiled = tmp / "compiled.pb"
    subprocess.run(
        ["protoc", f"-I{src}", "--include_imports", f"--descriptor_set_out={compiled}",
         "user.proto", STOCK],
        check=True, capture_output=True,
    )
    fds = descriptor_pb2.FileDescriptorSet()
    fds.ParseFromString(compiled.read_bytes())
    files = {f.name: f for f in fds.file}

    inputs = tmp / "in"
    stock = files[STOCK]
    _write(inputs, stock)
    root = descriptor_pb2.FileDescriptorProto()
    root.CopyFrom(stock)
    root.name, root.package = ROOT, "proto2"

    def rename(m):
        for f in [*m.field, *m.extension]:
            if f.HasField("type_name"):
                f.type_name = f.type_name.replace(".google.protobuf.", ".proto2.", 1)
        for n in m.nested_type:
            rename(n)

    for m in root.message_type:
        rename(m)
    file_options = next(m for m in root.message_type if m.name == "FileOptions")
    kept = [f for f in file_options.field if f.number != 20]
    del file_options.field[:]
    file_options.field.extend(kept)
    file_options.field.add(
        name="internal_flag", number=99,
        label=descriptor_pb2.FieldDescriptorProto.LABEL_OPTIONAL,
        type=descriptor_pb2.FieldDescriptorProto.TYPE_INT32,
    )
    _write(inputs, root)
    user = files["user.proto"]
    user.options.MergeFromString(user_options)
    _write(inputs, user, as_text)
    return inputs


out = Path(sys.argv[1])
a = _inputs(out / "in-binary", b"\x98\x06\x05", as_text=False)
b = _inputs(out / "in-text", b"\xa0\x01\x01", as_text=True)
for name in ("internal/proto2/descriptor.pb", "google/protobuf/descriptor.pb"):
    print(name, "same in both:", (a / name).read_bytes() == (b / name).read_bytes())
alone = out / "stock-alone.pb"
subprocess.run(["protoc", f"--descriptor_set_out={alone}", STOCK], check=True)
print("stock is protoc's own:", alone.read_bytes() == (a / "google/protobuf/descriptor.pb").read_bytes())
