# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Custom editions features are written (spec 0435).

A feature set's extensions (`features.(pb.cpp).string_type = VIEW`) are
written beside the standard features, at every level that can carry one:
compiling the output gives every element's options back, byte for byte.
"""

import subprocess
from pathlib import Path

import pytest
from google.protobuf import descriptor_pb2

from .conftest import REPROTO_CMD

FILES = {
    # Edition 2023: custom features on a file and on fields, alone and
    # beside a standard one.
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
    # Edition 2024: on a message and on an enum.
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


def _compile(root: Path, out: Path) -> descriptor_pb2.FileDescriptorSet:
    result = subprocess.run(
        ["protoc", f"-I{root}", "--include_imports", f"--descriptor_set_out={out}", *FILES],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, result.stderr
    fds = descriptor_pb2.FileDescriptorSet()
    fds.ParseFromString(out.read_bytes())
    return fds


def _options(fds: descriptor_pb2.FileDescriptorSet) -> dict[str, bytes]:
    """Every element's serialized options, by where it is."""
    out: dict[str, bytes] = {}
    for fdp in fds.file:
        if fdp.name not in FILES:
            continue
        out[fdp.name] = fdp.options.SerializeToString()
        for m in fdp.message_type:
            out[f"{fdp.name}:{m.name}"] = m.options.SerializeToString()
            for f in m.field:
                out[f"{fdp.name}:{m.name}.{f.name}"] = f.options.SerializeToString()
        for e in fdp.enum_type:
            out[f"{fdp.name}:{e.name}"] = e.options.SerializeToString()
    return out


@pytest.fixture
def rendered(tmp_path: Path) -> tuple[dict[str, bytes], dict[str, bytes], Path]:
    src = tmp_path / "src"
    for name, text in FILES.items():
        (src / name).parent.mkdir(parents=True, exist_ok=True)
        (src / name).write_text(text, encoding="utf-8")
    original = _compile(src, tmp_path / "original.pb")
    out = tmp_path / "out"
    result = subprocess.run(
        [*REPROTO_CMD, "-q", f"-I{tmp_path / 'original.pb'}", "-O", str(out),
         "--use-variant", "descriptor", "."],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, result.stderr
    again = _compile(out, tmp_path / "again.pb")
    return _options(original), _options(again), out


def test_every_custom_feature_comes_back(
    rendered: tuple[dict[str, bytes], dict[str, bytes], Path],
) -> None:
    original, again, _ = rendered
    assert again == original


def test_custom_features_are_written_as_features(
    rendered: tuple[dict[str, bytes], dict[str, bytes], Path],
) -> None:
    _, _, out = rendered
    text = (out / "f/feat.proto").read_text(encoding="utf-8")
    # Written as any message-valued custom option is: an aggregate.
    assert "features.(.pb.cpp) = {" in text
    assert "string_type: VIEW" in text and "string_type: CORD" in text


# A variant rooted on a descriptor.proto of another package, its names kept
# (no renaming): custom features extend that package's FeatureSet, and are
# read from it. protoc's own descriptor.proto and cpp_features.proto, moved
# to package `proto2` under other paths, stand for such a root; a stock
# protoc cannot compile against them, so the rendered text is checked.
ROOT = "internal/proto2/descriptor.proto"
CPP = "internal/pb/cpp_features.proto"
KEEP_VARIANT = f"""name: internal-keep
description: a proto2-rooted descriptor, names kept
descriptor_proto: {ROOT}
well_known: {{}}
import_rewrites: []
namespace_rewrites: []
orphans: {{}}
"""


def _rename_package(fdp: descriptor_pb2.FileDescriptorProto) -> None:
    """Every `.google.protobuf.` reference of `fdp` moved to `.proto2.`."""
    def ref(name: str) -> str:
        return name.replace(".google.protobuf.", ".proto2.", 1)

    def field(f: descriptor_pb2.FieldDescriptorProto) -> None:
        # Only where set: assigning "" would set the field (proto2 presence).
        if f.HasField("type_name"):
            f.type_name = ref(f.type_name)
        if f.HasField("extendee"):
            f.extendee = ref(f.extendee)

    def message(m: descriptor_pb2.DescriptorProto) -> None:
        for f in [*m.field, *m.extension]:
            field(f)
        for n in m.nested_type:
            message(n)

    for m in fdp.message_type:
        message(m)
    for f in fdp.extension:
        field(f)


def test_custom_features_under_a_root_of_another_package(tmp_path: Path) -> None:
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
    variant = tmp_path / "keep.yaml"
    variant.write_text(KEEP_VARIANT, encoding="utf-8")

    out = tmp_path / "out"
    result = subprocess.run(
        [*REPROTO_CMD, "-q", f"-I{inputs}", "-O", str(out),
         "--proto-variant", str(variant), "f/feat.pb", ROOT.replace(".proto", ".pb")],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, result.stderr
    text = (out / "f/feat.proto").read_text(encoding="utf-8")
    assert f'import "{CPP}";' in text
    assert "option features.(.pb.cpp) = {\n  string_type: CORD\n};" in text
    assert "features.(.pb.cpp) = {\n      string_type: VIEW\n    }" in text
    assert "features.field_presence = EXPLICIT" in text


def test_a_custom_option_on_a_root_of_another_package_is_kept(tmp_path: Path) -> None:
    # A custom option declared on proto2.FieldOptions (the root above,
    # names kept), set on a field: written, and its bytes kept.
    src = tmp_path / "src"
    src.mkdir()
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
    variant = tmp_path / "keep.yaml"
    variant.write_text(KEEP_VARIANT, encoding="utf-8")

    out = tmp_path / "out"
    result = subprocess.run(
        [*REPROTO_CMD, "-q", "-b", f"-I{inputs}", "-O", str(out),
         "--proto-variant", str(variant), "u.pb", ROOT.replace(".proto", ".pb")],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, result.stderr
    assert "[(.ext.level) = 7]" in (out / "u.proto").read_text(encoding="utf-8")
    # -b writes each file's FileDescriptorProto, alone.
    written = descriptor_pb2.FileDescriptorProto()
    written.ParseFromString((out / "u.pb").read_bytes())
    assert written.message_type[0].field[0].options.SerializeToString() == original
