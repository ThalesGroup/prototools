# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Two files rewritten onto one output path (spec 0437).

A variant renaming `internal/proto2/` to `google/protobuf/` maps its own
descriptor.proto onto protobuf's. With both in the input, the variant's
is kept, and said; any other collision stops the run.
"""

import os
import subprocess
from pathlib import Path

from google.protobuf import descriptor_pb2

from .conftest import REPROTO_CMD

ROOT = "internal/proto2/descriptor.proto"
STOCK = "google/protobuf/descriptor.proto"
RENAMING = f"""name: renaming
description: internal/proto2/ renamed to google/protobuf/
descriptor_proto: {ROOT}
well_known: {{}}
import_rewrites:
  - match: internal/proto2/
    action: rewrite
    to: google/protobuf/
namespace_rewrites:
  - match: .proto2.
    action: rewrite
    to: .google.protobuf.
"""
USER = """syntax = "proto2";
package user;
option java_package = "com.example.user";
message U { optional int32 x = 1; }
"""


def _env() -> dict[str, str]:
    return {k: v for k, v in os.environ.items() if k != "REPROTO_VARIANT"}


def _write(inputs: Path, fdp: descriptor_pb2.FileDescriptorProto, as_text: bool = False) -> None:
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


def _inputs(tmp: Path, user_options: bytes, as_text: bool) -> Path:
    """protoc's descriptor.proto as itself and, moved to package `proto2`,
    as the variant's: `FileOptions` gains `internal_flag = 99` and loses
    field 20, which only protobuf's copy then declares. A user file whose
    options are `user_options` (serialized)."""
    src = tmp / "src"
    src.mkdir()
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

    def rename(m: descriptor_pb2.DescriptorProto) -> None:
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
    (tmp / "renaming.yaml").write_text(RENAMING, encoding="utf-8")
    return inputs


def _run(tmp: Path, inputs: Path, *names: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [*REPROTO_CMD, f"-I{inputs}", "-O", str(tmp / "out"),
         "--proto-variant", str(tmp / "renaming.yaml"), *names],
        capture_output=True, text=True, env=_env(),
    )


def test_the_variants_descriptor_proto_is_kept_and_said(tmp_path: Path) -> None:
    # internal_flag = 5: field 99, varint.
    inputs = _inputs(tmp_path, b"\x98\x06\x05", as_text=False)
    result = _run(tmp_path, inputs, "user.pb", ROOT.replace(".proto", ".pb"),
                  STOCK.replace(".proto", ".pb"))
    assert result.returncode == 0, result.stderr
    assert (
        f"'{STOCK}' and '{ROOT}' are both written as '{STOCK}': '{ROOT}', "
        "the variant's descriptor.proto, is kept"
    ) in " ".join(result.stderr.split())
    written = (tmp_path / "out" / STOCK).read_text(encoding="utf-8")
    assert "internal_flag = 99" in written
    user = (tmp_path / "out" / "user.proto").read_text(encoding="utf-8")
    assert "option internal_flag = 5;" in user


def test_a_value_only_the_dropped_copy_declares_is_said(tmp_path: Path) -> None:
    # Field 20 (java_generate_equals_and_hash) = true: protobuf's copy
    # declares it, the variant's does not. Read as text, its name is
    # declared nowhere in the schema: W4.
    inputs = _inputs(tmp_path, b"\xa0\x01\x01", as_text=True)
    result = _run(tmp_path, inputs, "user.textpb", ROOT.replace(".proto", ".pb"),
                  STOCK.replace(".proto", ".pb"))
    assert result.returncode == 0, result.stderr
    assert "field java_generate_equals_and_hash of proto2.FileOptions is declared " \
        "nowhere in the input: skipped" in " ".join(result.stderr.split())


def test_any_other_collision_stops_the_run(tmp_path: Path) -> None:
    src = tmp_path / "src"
    for d in ("a", "b"):
        (src / d).mkdir(parents=True)
        (src / d / "x.proto").write_text(
            f'syntax = "proto3";\npackage {d};\nmessage X {{ int32 v = 1; }}\n',
            encoding="utf-8",
        )
    inputs = tmp_path / "in"
    for d in ("a", "b"):
        (inputs / d).mkdir(parents=True)
        subprocess.run(
            ["protoc", f"-I{src}", f"--descriptor_set_out={inputs / d / 'x.pb'}", f"{d}/x.proto"],
            check=True, capture_output=True,
        )
    # The variant's descriptor.proto, protobuf's own.
    (inputs / "google/protobuf").mkdir(parents=True)
    subprocess.run(
        ["protoc", f"--descriptor_set_out={inputs / 'google/protobuf/descriptor.pb'}", STOCK],
        check=True, capture_output=True,
    )
    (tmp_path / "renaming.yaml").write_text(
        "name: ab\ndescription: a/ onto b/\ndescriptor_proto: google/protobuf/descriptor.proto\n"
        "well_known: {}\nimport_rewrites:\n  - match: a/\n    action: rewrite\n    to: b/\n"
        "namespace_rewrites: []\n",
        encoding="utf-8",
    )
    result = subprocess.run(
        [*REPROTO_CMD, f"-I{inputs}", "-O", str(tmp_path / "out"),
         "--proto-variant", str(tmp_path / "renaming.yaml"), "a/x.pb", "b/x.pb",
         "google/protobuf/descriptor.pb"],
        capture_output=True, text=True, env=_env(),
    )
    assert result.returncode == 1, result.stderr
    assert result.stderr.rstrip().endswith(
        "Error: 'a/x.proto' and 'b/x.proto' are both written as 'b/x.proto'; prune one (-p)"
    ), result.stderr
    assert not (tmp_path / "out" / "b" / "x.proto").exists()
