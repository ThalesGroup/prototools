# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""A file renders the same whatever renders before it (spec 0429 S14).

ord_use.proto sets (ord.tag), declared in ord_ext.proto, its import of
ord_ext.proto dropped. Its options see only its own import closure: the
option is an orphan, with W22, whether ord_ext.proto renders before it or
after. It used to be written as (ord.tag) when ord_ext.proto came first.
"""

import os
import subprocess
from pathlib import Path

import pytest
from google.protobuf import descriptor_pb2

from .conftest import REPROTO_CMD

EXT = """syntax = "proto2";
package ord;
import "google/protobuf/descriptor.proto";
extend google.protobuf.FileOptions { optional string tag = 50101; }
"""
USE = """syntax = "proto2";
package ord;
import "ord_ext.proto";
option (ord.tag) = "set";
message M {}
"""


def _inputs(tmp_path: Path) -> tuple[Path, Path]:
    src = tmp_path / "src"
    src.mkdir()
    (src / "ord_ext.proto").write_text(EXT, encoding="utf-8")
    (src / "ord_use.proto").write_text(USE, encoding="utf-8")
    ext, use = tmp_path / "ext.pb", tmp_path / "use.pb"
    subprocess.run(["protoc", f"-I{src}", "--include_imports",
                    f"--descriptor_set_out={ext}", "ord_ext.proto"],
                   check=True, capture_output=True)
    subprocess.run(["protoc", f"-I{src}", f"--descriptor_set_out={use}", "ord_use.proto"],
                   check=True, capture_output=True)
    fds = descriptor_pb2.FileDescriptorSet()
    fds.ParseFromString(use.read_bytes())
    del fds.file[0].dependency[:]
    use.write_bytes(fds.SerializeToString())
    return ext, use


@pytest.mark.parametrize("ext_first", [True, False])
def test_an_option_outside_the_closure_is_an_orphan(tmp_path: Path, ext_first: bool) -> None:
    ext, use = _inputs(tmp_path)
    first, second = (ext, use) if ext_first else (use, ext)
    inp = tmp_path / "in"
    inp.mkdir()
    (inp / f"1_{first.stem}.pb").write_bytes(first.read_bytes())
    (inp / f"2_{second.stem}.pb").write_bytes(second.read_bytes())
    env = {k: v for k, v in os.environ.items() if k != "REPROTO_VARIANT"}
    result = subprocess.run(
        [*REPROTO_CMD, "--use-variant", "descriptor", f"-I{inp}", "-O", str(tmp_path / "out")],
        capture_output=True, text=True, env=env,
    )
    assert result.returncode == 0, result.stderr
    text = (tmp_path / "out" / "ord_use.proto").read_text(encoding="utf-8")
    assert "(ord.tag)" not in text, text
    assert 'option <50101> = "set";' in text, text
    assert "[W22]" in result.stderr, result.stderr
