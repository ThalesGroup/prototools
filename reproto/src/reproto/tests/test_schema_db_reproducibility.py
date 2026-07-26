# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Compiling the same input twice yields identical artifacts (spec 0177).

Both schema-DB sidecars used to differ on every build:

- `hopcroft.rkyv`, because `graph::build` numbered nodes by iterating a
  `HashMap` and Hopcroft's refinement allocated block IDs while iterating
  another one;
- `index.rkyv`, because rkyv lays an `ArchivedHashMap` out in the *source*
  map's iteration order (each key takes the first empty slot in its probe
  sequence), and `std::HashMap`'s `RandomState` is seeded per process.

Scoring never depended on any of it -- Hopcroft is confluent and every
consumer of `score_all` breaks ties on the unique FQDN -- so what this
recovers is content-addressability: verifying a DB by digest, caching or
deduplicating by digest, and diffing two DBs to tell "the schema changed"
from "someone rebuilt it".

Each build must run in its own **process**. `RandomState` is per process,
so an in-process loop would have passed even before the fix.

The schema has to be large enough to make Hopcroft actually *split* blocks
during refinement, which is where the block-numbering nondeterminism lived.
A handful of messages does not do it -- a four-message schema produced an
identical `hopcroft.rkyv` even before the fix. Hence the WKT import below:
`--include_imports` pulls in descriptor.proto and friends, which is a real
schema with enough structure to refine.
"""

from __future__ import annotations

import hashlib
import os
import subprocess
import sys
import textwrap
from pathlib import Path

_BUILDS = 4

_PROTO = """\
    syntax = "proto3";
    package repro;
    import "google/protobuf/descriptor.proto";
    import "google/protobuf/struct.proto";
    import "google/protobuf/timestamp.proto";
    message Holder {
      google.protobuf.FileDescriptorSet fds = 1;
      google.protobuf.Struct payload = 2;
      google.protobuf.Timestamp when = 3;
      repeated google.protobuf.FileDescriptorProto files = 4;
    }
"""


def _compile_proto(tmp_path: Path) -> Path:
    src_dir = tmp_path / "src"
    src_dir.mkdir(parents=True)
    (src_dir / "repro.proto").write_text(textwrap.dedent(_PROTO))

    pb_path = tmp_path / "repro.pb"
    result = subprocess.run(
        ["protoc", f"-I{src_dir}", "--include_imports",
         f"--descriptor_set_out={pb_path}", "repro.proto"],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, f"protoc failed: {result.stderr}"
    return pb_path


def _build_db(tmp_path: Path, pb_path: Path, tag: str) -> Path:
    """Run reproto in a fresh process and return the schema-DB sidecar dir."""
    src_path = str(Path(__file__).parent.parent.parent)
    pythonpath_parts = [src_path]
    if existing := os.environ.get("PYTHONPATH"):
        pythonpath_parts.append(existing)
    env = {**os.environ, "PYTHONPATH": os.pathsep.join(pythonpath_parts)}
    env.pop("REPROTO_VARIANT", None)

    db_path = tmp_path / tag / "repro.desc"
    db_path.parent.mkdir(parents=True, exist_ok=True)
    result = subprocess.run(
        [
            sys.executable, "-m", "reproto.cli",
            "--use-variant", "descriptor",
            f"--proto-out={tmp_path / tag / 'out'}",
            f"--schema-db-out={db_path}",
            str(pb_path),
        ],
        capture_output=True, text=True, env=env,
    )
    assert result.returncode == 0, f"reproto failed:\n{result.stderr}"
    return db_path.parent / "repro"


def _digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def test_R1_schema_db_sidecars_are_byte_reproducible(tmp_path: Path) -> None:
    """Four independent builds of one input agree byte for byte on both
    rkyv sidecars (spec 0177 S1-S3)."""
    pb_path = _compile_proto(tmp_path)
    dirs = [_build_db(tmp_path, pb_path, f"b{i}") for i in range(_BUILDS)]

    for name in ("hopcroft.rkyv", "index.rkyv"):
        digests = {_digest(d / name) for d in dirs}
        assert len(digests) == 1, (
            f"{name} differs across {_BUILDS} builds of identical input: "
            f"{sorted(digests)}"
        )
