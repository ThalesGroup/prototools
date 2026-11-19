# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Edition 2024 elements written as protoc reads them back.

- `local` / `export` before a message or an enum whose visibility is set,
  whatever the file's default;
- `import option "…";` for each option dependency, its options read as
  the extensions they are, not orphans;
- an empty `features` written `option features = {};`, on a file and on a
  oneof (whose options are built-in options like the others).

In a file written as proto2, none of these exists: the keywords dropped,
an `import option` a plain import.
"""

import os
import subprocess
from pathlib import Path

import pytest

from .conftest import REPROTO_CMD

OPT = '''edition = "2024";
package o;
import "google/protobuf/descriptor.proto";
extend google.protobuf.MessageOptions { int32 tag = 50001; }
'''
VIS = '''edition = "2024";
package v;
import option "opt.proto";
local message Hidden {
  export enum Shown { SHOWN_UNSPECIFIED = 0; }
  option (o.tag) = 7;
}
export message Out {
  oneof choice {
    option features = {};
    int32 a = 1;
    string b = 2;
  }
}
option features = {};
'''


def _render(tmp_path: Path, *flags: str) -> tuple[subprocess.CompletedProcess[str], Path]:
    src, inp, out = tmp_path / "src", tmp_path / "in", tmp_path / "out"
    src.mkdir()
    inp.mkdir()
    (src / "opt.proto").write_text(OPT, encoding="utf-8")
    (src / "vis.proto").write_text(VIS, encoding="utf-8")
    subprocess.run(["protoc", f"-I{src}", "--include_imports",
                    f"--descriptor_set_out={inp / 'vis.pb'}", "vis.proto"],
                   check=True, capture_output=True)
    env = {k: v for k, v in os.environ.items() if k != "REPROTO_VARIANT"}
    result = subprocess.run(
        [*REPROTO_CMD, "--use-variant", "descriptor", *flags, f"-I{inp}", "-O", str(out)],
        capture_output=True, text=True, env=env)
    assert result.returncode == 0, result.stderr
    return result, out


def _compiles(out: Path) -> None:
    r = subprocess.run(["protoc", f"-I{out}", "--descriptor_set_out=/dev/null", "vis.proto"],
                       capture_output=True, text=True)
    assert r.returncode == 0, r.stderr


def _code(path: Path) -> str:
    return "\n".join(line for line in path.read_text(encoding="utf-8").splitlines()
                     if line and not line.lstrip().startswith("//"))


def test_edition_2024_elements_roundtrip(tmp_path: Path) -> None:
    result, out = _render(tmp_path)
    text = _code(out / "vis.proto")
    assert "local message Hidden {" in text, text
    assert "export enum Shown {" in text, text
    assert "export message Out {" in text, text
    assert 'import option "opt.proto";' in text, text
    assert "option (.o.tag) = 7;" in text, text
    assert "W22" not in result.stderr, result.stderr
    assert text.count("option features = {};") == 2, text
    assert "OneofOptions" not in text, text
    _compiles(out)


@pytest.mark.parametrize("flag", ["--force-proto2-output"])
def test_edition_2024_elements_as_proto2(tmp_path: Path, flag: str) -> None:
    _, out = _render(tmp_path, flag)
    text = _code(out / "vis.proto")
    assert "local " not in text and "export " not in text, text
    assert 'import "opt.proto";' in text and "import option" not in text, text
    assert "features" not in text, text
    _compiles(out)
