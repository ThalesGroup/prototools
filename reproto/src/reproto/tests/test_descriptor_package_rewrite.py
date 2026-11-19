# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""The renamed descriptor's package, consistent (specs 0159, 0447 S3).

Regression tests for a bug where a renaming rewrote a file's own type
references (type_name/extendee/input_type/output_type) without rewriting
that same file's own `package` field, producing an internally
self-inconsistent FileDescriptorProto that fails to load into a real
DescriptorPool whenever the file contains a same-package self-reference:
most severely the descriptor's own file, where every field is one. Under
--target protoc the descriptor's definitions are renamed to
google.protobuf, its package with them, in its file alone.
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

from google.protobuf.descriptor_pb2 import FileDescriptorSet
from google.protobuf.descriptor_pool import DescriptorPool
from reproto.tests.conftest import REPROTO_CMD, vendor_descriptor

DESCRIPTOR = "vendor/proto/descriptor.proto"
# Definitions of the descriptor's file, beside its options messages.
SELF_REFERENCES = """
message Outer { optional Inner inner = 1; }
message Inner { optional string value = 1; }
message ItemSet { option message_set_wire_format = true; extensions 4 to max; }
"""


def _setup_fixtures(tmp_path: Path) -> Path:
    """The vendor descriptor, with SELF_REFERENCES: the input folder."""
    vendor_descriptor(tmp_path, top=SELF_REFERENCES)
    return tmp_path / "in"


def _run(inputs: Path, db_path: Path, out_dir: Path,
         *flags: str) -> subprocess.CompletedProcess[str]:
    env = {k: v for k, v in os.environ.items() if k != "REPROTO_VARIANT"}
    # Named: nothing here extends its options, for spec 0447 S1 to infer it.
    cmd = [*REPROTO_CMD, "--descriptor", DESCRIPTOR, f"-I{inputs}",
           f"--proto-out={out_dir}", f"--schema-db-out={db_path}", *flags]
    return subprocess.run(cmd, capture_output=True, text=True, env=env)


def test_G1_protoc_self_reference_loads_into_pool(tmp_path: Path) -> None:
    """--target protoc: the descriptor's own file's package is rewritten
    alongside its self-referencing type names, so the resulting .desc
    loads cleanly into a real DescriptorPool (spec 0159 G1/G3)."""
    inputs = _setup_fixtures(tmp_path)
    db_path = tmp_path / "schema.desc"
    result = _run(inputs, db_path, tmp_path / "out", "--target", "protoc")
    assert result.returncode == 0, f"reproto failed:\n{result.stderr}"

    fds = FileDescriptorSet()
    fds.MergeFromString(db_path.read_bytes())
    pool = DescriptorPool()
    for f in fds.file:
        pool.Add(f)
    outer = pool.FindMessageTypeByName("google.protobuf.Outer")
    assert outer.fields_by_name["inner"].message_type.full_name == "google.protobuf.Inner"


def test_G2_keep_stays_fully_unrewritten(tmp_path: Path) -> None:
    """--target keep: the descriptor's file's name, package, and type
    references are all left in the original namespace, self-consistent
    under the original names (spec 0159 G2/G3)."""
    inputs = _setup_fixtures(tmp_path)
    db_path = tmp_path / "schema.desc"
    result = _run(inputs, db_path, tmp_path / "out")
    assert result.returncode == 0, f"reproto failed:\n{result.stderr}"

    fds = FileDescriptorSet()
    fds.MergeFromString(db_path.read_bytes())
    desc = next(f for f in fds.file if f.name == DESCRIPTOR)
    assert desc.package == "vendor", desc.package
    outer = next(m for m in desc.message_type if m.name == "Outer")
    assert next(f for f in outer.field if f.name == "inner").type_name == ".vendor.Inner"

    pool = DescriptorPool()
    for f in fds.file:
        pool.Add(f)
    outer_desc = pool.FindMessageTypeByName("vendor.Outer")
    assert outer_desc.fields_by_name["inner"].message_type.full_name == "vendor.Inner"
