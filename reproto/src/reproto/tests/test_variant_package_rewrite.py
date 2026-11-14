# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Tests for variant namespace-rewrite package consistency (spec 0159).

Regression tests for a bug where a variant's namespace_rewrites rules
rewrote a file's own type references (type_name/extendee/input_type/
output_type) without rewriting that same file's own `package` field,
producing an internally self-inconsistent FileDescriptorProto that fails
to load into a real DescriptorPool whenever the file contains a
same-package self-reference — most severely for a variant's own
descriptor_proto: file, where every field is such a self-reference.
"""

from __future__ import annotations

import os
import subprocess
import textwrap
from pathlib import Path

from google.protobuf.descriptor_pb2 import FileDescriptorSet
from google.protobuf.descriptor_pool import DescriptorPool
from reproto.tests.conftest import REPROTO_CMD


# create_option_message_classes() (option_messages.py) hardcodes the
# expected package for a non-'google/'-rooted descriptor_proto to
# 'proto2', and looks up these exact message names in the pool — so a
# synthetic descriptor_proto fixture must declare package proto2 and
# define all of them (even though this test does not exercise options).
_REQUIRED_OPTIONS_MESSAGES = "\n".join(
    f"message {name} {{}}" for name in (
        "EnumOptions", "EnumValueOptions", "FieldOptions", "FileOptions",
        "MessageOptions", "MethodOptions", "OneofOptions", "ServiceOptions",
    )
)


def _write_proto(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(textwrap.dedent(content))


def _compile(src_dir: Path, out_dir: Path, rel_name: str) -> Path:
    pb_path = out_dir / (rel_name.replace("/", "_") + ".pb")
    result = subprocess.run(
        ["protoc", f"-I{src_dir}", f"--descriptor_set_out={pb_path}", rel_name],
        capture_output=True, text=True, cwd=src_dir,
    )
    assert result.returncode == 0, f"protoc failed on {rel_name}: {result.stderr}"
    return pb_path


def _write_variant(tmp_path: Path) -> Path:
    yaml_file = tmp_path / "selfref.yaml"
    yaml_file.write_text(textwrap.dedent("""\
        name: selfref-test
        descriptor_proto: legacy/proto/schema.proto
        import_rewrites:
          - match: legacy/proto/
            action: rewrite
            to: canonical/
        namespace_rewrites:
          - match: .proto2.
            action: rewrite
            to: .canonical.
    """))
    return yaml_file


def _run(
    variant: Path, db_path: Path, out_dir: Path, *pb_paths: Path,
    extra_args: list[str] | None = None,
) -> subprocess.CompletedProcess[str]:
    src_path = str(Path(__file__).parent.parent.parent)
    pythonpath_parts = [src_path]
    if existing := os.environ.get("PYTHONPATH"):
        pythonpath_parts.append(existing)
    env = {**os.environ, "PYTHONPATH": os.pathsep.join(pythonpath_parts)}
    env.pop("REPROTO_VARIANT", None)

    cmd = [
        *REPROTO_CMD,
        f"--proto-variant={variant}",
        f"--proto-out={out_dir}",
        f"--schema-db-out={db_path}",
        *(extra_args or []),
        *[str(p) for p in pb_paths],
    ]
    return subprocess.run(cmd, capture_output=True, text=True, env=env)


def _setup_fixtures(tmp_path: Path) -> tuple[Path, Path]:
    src_dir = tmp_path / "src"
    schema_content = textwrap.dedent("""\
        syntax = "proto3";
        package proto2;
        message Outer {
          Inner inner = 1;
        }
        message Inner {
          string value = 1;
        }
    """) + _REQUIRED_OPTIONS_MESSAGES + "\n"
    _write_proto(src_dir / "legacy" / "proto" / "schema.proto", schema_content)
    _write_proto(src_dir / "client.proto", """\
        syntax = "proto3";
        package client;
        import "legacy/proto/schema.proto";
        message UsesOuter {
          proto2.Outer outer = 1;
        }
    """)
    out_dir = tmp_path / "pb"
    out_dir.mkdir()
    schema_pb = _compile(src_dir, out_dir, "legacy/proto/schema.proto")
    client_pb = _compile(src_dir, out_dir, "client.proto")
    return schema_pb, client_pb


def test_G1_default_mode_self_reference_loads_into_pool(tmp_path: Path) -> None:
    """Default (no --keep-descriptor-path): the variant's own
    descriptor_proto file's package is rewritten alongside its
    self-referencing type names, so the resulting .desc loads cleanly
    into a real DescriptorPool (spec 0159 G1/G3)."""
    schema_pb, client_pb = _setup_fixtures(tmp_path)

    variant = _write_variant(tmp_path)
    db_path = tmp_path / "schema.desc"
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    result = _run(variant, db_path, out_dir, schema_pb, client_pb)
    assert result.returncode == 0, f"reproto failed:\n{result.stderr}"

    fds = FileDescriptorSet()
    fds.MergeFromString(db_path.read_bytes())

    pool = DescriptorPool()
    for f in fds.file:
        pool.Add(f)

    outer = pool.FindMessageTypeByName("canonical.Outer")
    inner_field = outer.fields_by_name["inner"]
    assert inner_field.message_type.full_name == "canonical.Inner", (
        f"Outer.inner must resolve to canonical.Inner, got "
        f"{inner_field.message_type.full_name!r}"
    )


def test_G2_keep_descriptor_path_stays_fully_unrewritten(tmp_path: Path) -> None:
    """--keep-descriptor-path: the variant's own descriptor_proto file's
    name, package, and type references are all left in the original
    namespace — self-consistent under the original names (spec 0159 G2/G3)."""
    schema_pb, client_pb = _setup_fixtures(tmp_path)

    variant = _write_variant(tmp_path)
    db_path = tmp_path / "schema.desc"
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    result = _run(
        variant, db_path, out_dir, schema_pb, client_pb,
        extra_args=["--keep-descriptor-path"],
    )
    assert result.returncode == 0, f"reproto failed:\n{result.stderr}"

    fds = FileDescriptorSet()
    fds.MergeFromString(db_path.read_bytes())

    schema_fdp = next(f for f in fds.file if f.name == "legacy/proto/schema.proto")
    assert schema_fdp.package == "proto2", (
        f"package must stay unrewritten under --keep-descriptor-path, got "
        f"{schema_fdp.package!r}"
    )
    outer = next(m for m in schema_fdp.message_type if m.name == "Outer")
    inner_field = next(f for f in outer.field if f.name == "inner")
    assert inner_field.type_name == ".proto2.Inner", (
        f"Outer.inner.type_name must stay unrewritten under "
        f"--keep-descriptor-path, got {inner_field.type_name!r}"
    )

    pool = DescriptorPool()
    for f in fds.file:
        pool.Add(f)
    outer_desc = pool.FindMessageTypeByName("proto2.Outer")
    assert outer_desc.fields_by_name["inner"].message_type.full_name == "proto2.Inner"
