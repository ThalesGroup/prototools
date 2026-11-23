# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""A file the descriptor pool refuses keeps its file options, its features
among them (spec 0475): rendered from its descriptor, W19 still reported;
a custom option whose extension the refused file itself declares, an
orphan with W22. The suite runs on reproto and on protoback alike."""

import os
import subprocess
from pathlib import Path

from .conftest import REPROTO_CMD

ROW = """name: "a/row.proto" package: "a" syntax: "editions" edition: EDITION_2023
message_type { name: "Base" extension_range { start: 100 end: 200 } }
"""

# Refused: extension 500 is in no extension range of a.Base.
USER = """name: "b/user.proto" package: "b" dependency: "a/row.proto"
dependency: "google/protobuf/descriptor.proto"
syntax: "editions" edition: EDITION_2023
options { java_package: "com.example.b" features { enum_type: CLOSED } 50001: 5 }
enum_type { name: "E" value { name: "E_ONE" number: 1 } }
extension { name: "far" number: 500 type: TYPE_INT32 extendee: ".a.Base" json_name: "far" }
extension { name: "tag" number: 50001 type: TYPE_INT32
  extendee: ".google.protobuf.FileOptions" json_name: "tag" }
"""


def test_a_refused_file_keeps_its_options(tmp_path: Path) -> None:
    inp = tmp_path / "in"
    inp.mkdir()
    (inp / "row.textpb").write_text(ROW, encoding="utf-8")
    (inp / "user.textpb").write_text(USER, encoding="utf-8")
    env = {k: v for k, v in os.environ.items() if k != "REPROTO_VARIANT"}
    result = subprocess.run(
        [*REPROTO_CMD, f"-I{inp}", "-O", str(tmp_path / "out")],
        capture_output=True, text=True, env=env,
    )
    assert result.returncode == 0, result.stderr
    assert "[W19]" in result.stderr and "[W22]" in result.stderr, result.stderr
    text = (tmp_path / "out" / "b" / "user.proto").read_text(encoding="utf-8")
    for expected in (
        "// WARNING[render]: the descriptor pool refused it: field number 500",
        "option features.enum_type = CLOSED;",
        'option java_package = "com.example.b";',
        "///option <50001> = 5;",
    ):
        assert expected in text, f"missing: {expected}\n{text}"
    assert "OMITTED[render]" not in text, text
