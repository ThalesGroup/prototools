# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Tests for schema-db transitive completeness (spec 0150).

Regression test for a bug where --schema-db-out/--build-schema-db
silently omitted google/protobuf/descriptor.proto (or a variant's
equivalent) from the produced FileDescriptorSet whenever it was needed
only as a custom-option dependency and --no-emit-descriptor was
passed, violating the transitive-completeness invariant required by
consumers such as prost_reflect::DescriptorPool::decode.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

from google.protobuf.descriptor_pb2 import FileDescriptorSet

from reproto.tests.conftest import FIXTURES_DIR


def _compile(orig_dir: Path, proto_name: str) -> Path:
    pb_path = orig_dir / f"{Path(proto_name).stem}.pb"
    result = subprocess.run(
        ["protoc", f"--descriptor_set_out={pb_path}", f"-I{FIXTURES_DIR}",
         str(FIXTURES_DIR / proto_name)],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, f"protoc failed on {proto_name}: {result.stderr}"
    return pb_path


def test_G1_schema_db_includes_suppressed_descriptor_proto(tmp_path: Path) -> None:
    """--schema-db-out with --no-emit-descriptor must still include
    google/protobuf/descriptor.proto when a custom option pulls it in
    (spec 0150), while --proto-out continues to omit it (N1)."""
    orig_dir = tmp_path / "orig"
    orig_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()
    db_path = tmp_path / "schema.desc"

    main_pb = _compile(orig_dir, "editions_roundtrip.proto")
    dep_pb = _compile(orig_dir, "editions_custom_option_dep.proto")
    weak_pb = _compile(orig_dir, "weak_import_proto2_dep.proto")

    src_path = str(Path(__file__).parent.parent.parent)
    pythonpath_parts = [src_path]
    if existing := os.environ.get("PYTHONPATH"):
        pythonpath_parts.append(existing)
    env = {**os.environ, "PYTHONPATH": os.pathsep.join(pythonpath_parts)}
    env.pop("REPROTO_VARIANT", None)

    cmd = [
        sys.executable, "-m", "reproto.cli",
        "--use-variant", "descriptor",
        "--no-emit-descriptor",
        f"-I{FIXTURES_DIR}",
        f"--proto-out={out_dir}",
        f"--schema-db-out={db_path}",
        str(main_pb), str(dep_pb), str(weak_pb),
    ]
    result = subprocess.run(cmd, capture_output=True, text=True, env=env)
    assert result.returncode == 0, f"reproto failed:\n{result.stderr}"
    assert db_path.exists(), "schema DB not created"

    fds = FileDescriptorSet()
    fds.MergeFromString(db_path.read_bytes())
    names = [f.name for f in fds.file]
    assert "google/protobuf/descriptor.proto" in names, (
        f"descriptor.proto missing from schema DB; files present: {names}"
    )

    # Transitive completeness: every dependency listed by an included file
    # must itself be included (mirrors prost_reflect's own requirement,
    # the exact invariant this bug violated).
    name_set = set(names)
    for f in fds.file:
        for dep in f.dependency:
            assert dep in name_set, (
                f"{f.name} depends on {dep}, which is missing from the schema DB"
            )

    # N1: --proto-out still does not contain descriptor.proto as text.
    assert not (out_dir / "google" / "protobuf" / "descriptor.proto").exists()
