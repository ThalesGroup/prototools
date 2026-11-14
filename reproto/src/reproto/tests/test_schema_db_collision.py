# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Tests for schema-db canonical-name collision detection (spec 0158).

Regression test for a bug where --schema-db-out/--build-schema-db could
silently produce an invalid FileDescriptorSet: two files with distinct
declared .name's that canonize to the same name under a variant's
import_rewrites, but carry different content, used to be resolved by a
plain dict comprehension that silently kept whichever entry happened to
appear last and dropped the other — instead of raising a loud error.
"""

from __future__ import annotations

import os
import subprocess
import textwrap
from pathlib import Path

from google.protobuf.descriptor_pb2 import FileDescriptorSet
from reproto.tests.conftest import REPROTO_CMD


def _write_proto(path: Path, package: str, message: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(textwrap.dedent(f"""\
        syntax = "proto3";
        package {package};
        message {message} {{
          string value = 1;
        }}
    """))


def _compile(src_dir: Path, out_dir: Path, rel_name: str) -> Path:
    pb_path = out_dir / (Path(rel_name).stem + "_" + rel_name.replace("/", "_") + ".pb")
    result = subprocess.run(
        ["protoc", f"-I{src_dir}", f"--descriptor_set_out={pb_path}", rel_name],
        capture_output=True, text=True, cwd=src_dir,
    )
    assert result.returncode == 0, f"protoc failed on {rel_name}: {result.stderr}"
    return pb_path


def _compile_descriptor_proto(out_dir: Path) -> Path:
    """Compile protoc's built-in google/protobuf/descriptor.proto, so the
    schema-db-out run has a real descriptor.proto without needing the
    variant's embedded --use-variant fallback (only shipped for the
    built-in variant, not for the ad-hoc test variant here)."""
    pb_path = out_dir / "descriptor.pb"
    result = subprocess.run(
        ["protoc", "--descriptor_set_out=" + str(pb_path), "google/protobuf/descriptor.proto"],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, f"protoc failed on descriptor.proto: {result.stderr}"
    return pb_path


def _write_variant(tmp_path: Path) -> Path:
    yaml_file = tmp_path / "collision.yaml"
    yaml_file.write_text(textwrap.dedent("""\
        name: collision-test
        import_rewrites:
          - match: a/
            action: rewrite
            to: shared/
          - match: b/
            action: rewrite
            to: shared/
    """))
    return yaml_file


def _run(variant: Path, db_path: Path, out_dir: Path, *pb_paths: Path) -> subprocess.CompletedProcess[str]:
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
        *[str(p) for p in pb_paths],
    ]
    return subprocess.run(cmd, capture_output=True, text=True, env=env)


def test_G1_colliding_names_different_content_is_fatal(tmp_path: Path) -> None:
    """Two files canonizing to the same name with different content abort
    with a clear error naming both original sources and the shared
    canonical name, instead of silently dropping one (spec 0158 G1/G3)."""
    src_dir = tmp_path / "src"
    _write_proto(src_dir / "a" / "dupe.proto", "collision.a", "DupeA")
    _write_proto(src_dir / "b" / "dupe.proto", "collision.b", "DupeB")

    out_dir = tmp_path / "out"
    out_dir.mkdir()
    a_pb = _compile(src_dir, tmp_path, "a/dupe.proto")
    b_pb = _compile(src_dir, tmp_path, "b/dupe.proto")
    descriptor_pb = _compile_descriptor_proto(tmp_path)

    variant = _write_variant(tmp_path)
    db_path = tmp_path / "schema.desc"

    result = _run(variant, db_path, out_dir, a_pb, b_pb, descriptor_pb)
    assert result.returncode != 0, f"expected failure, got success:\n{result.stdout}"
    assert "schema-db name collision" in result.stderr
    assert "a/dupe.proto" in result.stderr
    assert "b/dupe.proto" in result.stderr
    assert "shared/dupe.proto" in result.stderr


def test_G2_colliding_names_identical_content_succeeds(tmp_path: Path) -> None:
    """Two files canonizing to the same name with byte-identical content
    are not an error — the run succeeds and the resulting .desc contains
    exactly one entry under the canonical name (spec 0158 G2)."""
    src_dir = tmp_path / "src"
    _write_proto(src_dir / "a" / "dupe.proto", "collision.same", "Dupe")
    _write_proto(src_dir / "b" / "dupe.proto", "collision.same", "Dupe")

    out_dir = tmp_path / "out"
    out_dir.mkdir()
    a_pb = _compile(src_dir, tmp_path, "a/dupe.proto")
    b_pb = _compile(src_dir, tmp_path, "b/dupe.proto")
    descriptor_pb = _compile_descriptor_proto(tmp_path)

    variant = _write_variant(tmp_path)
    db_path = tmp_path / "schema.desc"

    result = _run(variant, db_path, out_dir, a_pb, b_pb, descriptor_pb)
    assert result.returncode == 0, f"reproto failed:\n{result.stderr}"
    assert db_path.exists()

    fds = FileDescriptorSet()
    fds.MergeFromString(db_path.read_bytes())
    names = [f.name for f in fds.file]
    assert names.count("shared/dupe.proto") == 1, (
        f"expected exactly one 'shared/dupe.proto' entry, got: {names}"
    )
