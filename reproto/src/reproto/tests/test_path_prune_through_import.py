# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""A path: prune applies to a file reached through an import.

A path pattern prunes "that physical file, never loaded". An import of
a.proto resolves to a.pb (or a.textpb, ...); the pattern was matched against
the import's name, a.proto, so `-p path:a.pb` let a.pb load as soon as a file
imported it. It is now matched against the file found, and against the
import's name as before.
"""

import os
import subprocess
from pathlib import Path

import pytest

from .conftest import REPROTO_CMD

A = 'syntax = "proto2";\npackage a;\nmessage A { optional int32 x = 1; }\n'
B = ('syntax = "proto2";\npackage b;\nimport "a.proto";\n'
     'message B { optional a.A a = 1; }\n')


@pytest.mark.parametrize("pattern", ["path:a.pb", "path:a.*"])
def test_a_path_prune_holds_through_an_import(tmp_path: Path, pattern: str) -> None:
    src = tmp_path / "src"
    inp = tmp_path / "in"
    src.mkdir()
    inp.mkdir()
    for name, text in {"a.proto": A, "b.proto": B}.items():
        (src / name).write_text(text, encoding="utf-8")
        subprocess.run(["protoc", f"-I{src}",
                        f"--descriptor_set_out={inp / name.replace('.proto', '.pb')}",
                        name], check=True, capture_output=True)
    env = {k: v for k, v in os.environ.items() if k != "REPROTO_VARIANT"}
    result = subprocess.run(
        [*REPROTO_CMD, "--use-variant", "descriptor", "-p", pattern,
         f"-I{inp}", "-O", str(tmp_path / "out")],
        capture_output=True, text=True, env=env,
    )
    assert result.returncode == 0, result.stderr
    assert not (tmp_path / "out" / "a.proto").exists(), result.stderr
    assert (tmp_path / "out" / "b.proto").exists(), result.stderr
    assert "missing dependency file:a.proto" in result.stderr, result.stderr
