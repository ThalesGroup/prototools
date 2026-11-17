# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""A node is reported once, whatever is written (spec 0429 S11).

With -b (or --schema-db-out), the descriptor is built from renders of the
children after the text's: those report nothing. A warning raised two
messages deep used to print 2^(d+1) times, 8 here.
"""

import os
import subprocess
from pathlib import Path

import pytest
from google.protobuf import descriptor_pb2 as d

from .conftest import REPROTO_CMD

F = d.FieldDescriptorProto


def _write_input(path: Path) -> None:
    """A proto3 message C, in B, in A, extending a proto2 Base: B1."""
    base = d.FileDescriptorProto(name="base.proto", package="p", syntax="proto2")
    base.message_type.add(name="Base").extension_range.add(start=100, end=200)
    f = d.FileDescriptorProto(name="m.proto", package="p", syntax="proto3",
                              dependency=["base.proto"])
    a = f.message_type.add(name="A")
    a.field.add(name="i", number=1, type=F.TYPE_INT32, label=F.LABEL_OPTIONAL)
    c = a.nested_type.add(name="B").nested_type.add(name="C")
    c.extension.add(name="x", number=100, type=F.TYPE_INT32,
                    label=F.LABEL_OPTIONAL, extendee=".p.Base")
    fds = d.FileDescriptorSet()
    fds.file.extend([base, f])
    path.write_bytes(fds.SerializeToString())


@pytest.mark.parametrize("flags", [[], ["-b"], ["--detailed-warnings", "-b"]])
def test_a_nested_warning_is_printed_once(tmp_path: Path, flags: list[str]) -> None:
    _write_input(tmp_path / "m.pb")
    env = {k: v for k, v in os.environ.items() if k != "REPROTO_VARIANT"}
    result = subprocess.run(
        [*REPROTO_CMD, *flags, "--use-variant", "descriptor", f"-I{tmp_path}",
         "-O", str(tmp_path / "out"), "m.pb"],
        capture_output=True, text=True, env=env,
    )
    assert result.returncode == 0, result.stderr
    lines = [ln for ln in result.stderr.splitlines() if "[B1]" in ln]
    assert lines == [
        "Warning [B1]: message 'C': nested extend block for '.p.Base' is not "
        "valid in proto3; omitting"
    ], result.stderr
