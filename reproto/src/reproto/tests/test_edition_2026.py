# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""An edition 2026 file renders with `edition = "2026";` (specs 0457,
0460): from text, parsed against the built-in descriptor.proto of the
release every copy comes from. The suite runs on reproto and protoback
alike, so both write the same."""

import os
import subprocess
from pathlib import Path

from .conftest import REPROTO_CMD

INPUT = """name: "a/b.proto" package: "a" syntax: "editions" edition: EDITION_2026
message_type { name: "M" field { name: "x" number: 1 type: TYPE_INT32 json_name: "x" } }
"""


def test_an_edition_2026_file_renders(tmp_path: Path) -> None:
    inp = tmp_path / "in"
    inp.mkdir()
    (inp / "b.textpb").write_text(INPUT, encoding="utf-8")
    env = {k: v for k, v in os.environ.items() if k != "REPROTO_VARIANT"}
    result = subprocess.run(
        [*REPROTO_CMD, f"-I{inp}", "-O", str(tmp_path / "out")],
        capture_output=True, text=True, env=env,
    )
    assert result.returncode == 0, result.stderr
    assert "[W15]" not in result.stderr and "[W6]" not in result.stderr, result.stderr
    text = (tmp_path / "out" / "a" / "b.proto").read_text(encoding="utf-8")
    assert 'edition = "2026";' in text, text
    assert "int32 x = 1;" in text, text
