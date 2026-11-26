# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""The inputs of test_custom_features.py's last two tests, made by their own
code (4da0f2d6), copied verbatim but for the output directories: OUT/root/
for test_custom_features_under_a_root_of_another_package, OUT/option/ for
test_a_custom_option_on_a_root_of_another_package_is_kept. Prints the
latter's expected value (`original`, the field's options serialized), and
whether both tests' renamed descriptor.proto are the same bytes. Run with a
Python that has protobuf, protoc on PATH.

Usage: python3 -I -B test_custom_features.py OUT
"""
import subprocess
import sys
from pathlib import Path

from google.protobuf import descriptor_pb2

FILES = {
    "f/feat.proto": """edition = "2023";
package feat;
import "google/protobuf/cpp_features.proto";
option features.field_presence = IMPLICIT;
option features.(pb.cpp).string_type = CORD;
message M {
  string s = 1 [features.(pb.cpp).string_type = VIEW];
  string t = 2 [features.field_presence = EXPLICIT, features.(pb.cpp).string_type = STRING];
  string u = 3;
}
""",
    "f/feat24.proto": """edition = "2024";
package feat24;
import "google/protobuf/cpp_features.proto";
import "google/protobuf/java_features.proto";
message M {
  option features.(pb.java).nest_in_file_class = YES;
  int32 x = 1;
}
enum E {
  option features.(pb.cpp).enum_name_uses_string_view = true;
  E_UNKNOWN = 0;
}
""",
}
ROOT = "internal/proto2/descriptor.proto"
CPP = "internal/pb/cpp_features.proto"


def _compile(root, out):
    result = subprocess.run(
        ["protoc", f"-I{root}", "--include_imports", f"--descriptor_set_out={out}", *FILES],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, result.stderr
    fds = descriptor_pb2.FileDescriptorSet()
    fds.ParseFromString(out.read_bytes())
    return fds


def _rename_package(fdp):
    def ref(name):
        return name.replace(".google.protobuf.", ".proto2.", 1)

    def field(f):
        if f.HasField("type_name"):
            f.type_name = ref(f.type_name)
        if f.HasField("extendee"):
            f.extendee = ref(f.extendee)

    def message(m):
        for f in [*m.field, *m.extension]:
            field(f)
        for n in m.nested_type:
            message(n)

    for m in fdp.message_type:
        message(m)
    for f in fdp.extension:
        field(f)


def under_a_root(tmp_path):
    src = tmp_path / "src"
    (src / "f").mkdir(parents=True)
    for name, text in FILES.items():
        (src / name).write_text(text, encoding="utf-8")
    compiled = _compile(src, tmp_path / "stock.pb")
    files = {f.name: f for f in compiled.file}

    root = files["google/protobuf/descriptor.proto"]
    root.name, root.package = ROOT, "proto2"
    _rename_package(root)
    cpp = files["google/protobuf/cpp_features.proto"]
    cpp.name = CPP
    cpp.dependency[:] = [ROOT]
    _rename_package(cpp)
    user = files["f/feat.proto"]
    user.dependency[:] = [CPP]

    inputs = tmp_path / "in"
    for fdp in (root, cpp, user):
        path = inputs / fdp.name.replace(".proto", ".pb")
        path.parent.mkdir(parents=True, exist_ok=True)
        fds = descriptor_pb2.FileDescriptorSet()
        fds.file.append(fdp)
        path.write_bytes(fds.SerializeToString())
    return inputs


def option_kept(tmp_path):
    src = tmp_path / "src"
    src.mkdir(parents=True)
    (src / "ext.proto").write_text(
        'syntax = "proto2";\npackage ext;\nimport "google/protobuf/descriptor.proto";\n'
        "extend google.protobuf.FieldOptions { optional int32 level = 50001; }\n",
        encoding="utf-8",
    )
    (src / "u.proto").write_text(
        'syntax = "proto2";\npackage u;\nimport "ext.proto";\n'
        "message M { optional int32 x = 1 [(ext.level) = 7]; }\n",
        encoding="utf-8",
    )
    compiled = tmp_path / "stock.pb"
    result = subprocess.run(
        ["protoc", f"-I{src}", "--include_imports", f"--descriptor_set_out={compiled}", "u.proto"],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, result.stderr
    fds = descriptor_pb2.FileDescriptorSet()
    fds.ParseFromString(compiled.read_bytes())
    files = {f.name: f for f in fds.file}
    root = files["google/protobuf/descriptor.proto"]
    root.name, root.package = ROOT, "proto2"
    _rename_package(root)
    ext = files["ext.proto"]
    ext.dependency[:] = [ROOT]
    _rename_package(ext)
    user = files["u.proto"]
    original = user.message_type[0].field[0].options.SerializeToString()

    inputs = tmp_path / "in"
    for fdp in (root, ext, user):
        path = inputs / fdp.name.replace(".proto", ".pb")
        path.parent.mkdir(parents=True, exist_ok=True)
        one = descriptor_pb2.FileDescriptorSet()
        one.file.append(fdp)
        path.write_bytes(one.SerializeToString())
    return inputs, original


out = Path(sys.argv[1])
a = under_a_root(out / "root")
b, original = option_kept(out / "option")
print("original:", original.hex())
print("roots identical:", (a / "internal/proto2/descriptor.pb").read_bytes()
      == (b / "internal/proto2/descriptor.pb").read_bytes())
