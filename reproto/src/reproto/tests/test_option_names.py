# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Option names as googleapis writes them, custom features as paths (spec
0439): the text, and every options message byte-identical after
recompiling."""

import os
import subprocess
from pathlib import Path

from google.protobuf import descriptor_pb2

from .conftest import REPROTO_CMD

FILES = {
    "ext.proto": """syntax = "proto2";
package ext;
import "google/protobuf/descriptor.proto";
message Rule { optional string get = 1; optional string body = 2; }
extend google.protobuf.FileOptions { optional int32 f_opt = 50001; }
extend google.protobuf.MessageOptions {
  optional int32 m_opt = 50002;
  optional Rule rule = 50006;
}
extend google.protobuf.FieldOptions { optional string fld = 50003; }
extend google.protobuf.EnumValueOptions { optional bool ev = 50004; }
extend google.protobuf.MethodOptions { optional int32 meth = 50005; }
""",
    "u.proto": """syntax = "proto2";
package u;
import "ext.proto";
option (ext.f_opt) = 1;
message M {
  option (ext.m_opt) = 2;
  option (ext.rule) = { get: "/v1/x" body: "*" };
  optional string s = 1 [(ext.fld) = "s"];
}
enum E { E_ZERO = 0 [(ext.ev) = true]; }
service S { rpc Get(M) returns (M) { option (ext.meth) = 3; } }
""",
    # In package y.ext, `ext.f_opt` is captured: its first component finds
    # the package y.ext first, which has no f_opt.
    "y/c.proto": """syntax = "proto2";
package y.ext;
import "ext.proto";
option (.ext.f_opt) = 4;
message C { optional int32 x = 1; }
""",
    "f/feat.proto": """edition = "2023";
package feat;
import "google/protobuf/cpp_features.proto";
option features.(pb.cpp).string_type = CORD;
message M {
  string s = 1 [features.(pb.cpp).legacy_closed_enum = true, features.(pb.cpp).string_type = VIEW];
  string t = 2 [features.(pb.cpp) = {}];
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
    for f in fds.file:
        if f.name not in FILES:
            continue
        out[f.name] = f.options.SerializeToString()
        for m in f.message_type:
            out[f"{f.name}:{m.name}"] = m.options.SerializeToString()
            for fd in m.field:
                out[f"{f.name}:{m.name}.{fd.name}"] = fd.options.SerializeToString()
        for e in f.enum_type:
            for v in e.value:
                out[f"{f.name}:{v.name}"] = v.options.SerializeToString()
        for s in f.service:
            for meth in s.method:
                out[f"{f.name}:{s.name}.{meth.name}"] = meth.options.SerializeToString()
    return out


def test_option_names_and_feature_paths(tmp_path: Path) -> None:
    src = tmp_path / "src"
    for name, text in FILES.items():
        (src / name).parent.mkdir(parents=True, exist_ok=True)
        (src / name).write_text(text, encoding="utf-8")
    original = _compile(src, tmp_path / "original.pb")
    out = tmp_path / "out"
    env = {k: v for k, v in os.environ.items() if k != "REPROTO_VARIANT"}
    result = subprocess.run(
        [*REPROTO_CMD, "-q", f"-I{tmp_path / 'original.pb'}", "-O", str(out),
         "--use-variant", "descriptor", "."],
        capture_output=True, text=True, env=env,
    )
    assert result.returncode == 0, result.stderr
    again = _compile(out, tmp_path / "again.pb")
    assert _options(again) == _options(original)

    def rendered(name: str) -> str:
        return " ".join((out / name).read_text(encoding="utf-8").split())

    u = rendered("u.proto")
    # In full, without the leading dot (googleapis' way).
    for written in ("option (ext.f_opt) = 1;", "option (ext.m_opt) = 2;",
                    '(ext.fld) = "s"', "(ext.ev) = true", "option (ext.meth) = 3;",
                    "option (ext.rule) = {"):
        assert written in u, written
    assert "(.ext." not in u
    # Captured: the dot kept.
    assert "option (.ext.f_opt) = 4;" in rendered("y/c.proto")
    # A custom feature: one path per field (protobuf's way); empty, {}.
    feat = rendered("f/feat.proto")
    assert "option features.(pb.cpp).string_type = CORD;" in feat
    assert ("features.(pb.cpp).legacy_closed_enum = true, "
            "features.(pb.cpp).string_type = VIEW") in feat
    assert "features.(pb.cpp) = {}" in feat
