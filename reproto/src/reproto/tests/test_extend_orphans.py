# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""An extend block whose every field is an orphan is an orphan whole,
nested or top-level; one holding a field still written is not (spec
0459). The same input and text as protoback's `tests/extend.rs`."""

import os
import subprocess
from pathlib import Path

from .conftest import REPROTO_CMD

# `x.JobSpec` is in no file: both blocks extending it hold orphans only.
# `e.Base` is: its block holds `ok`, written, and `bad`, whose type is in
# no file.
INPUT = """name: "e.proto" package: "e" dependency: "gone.proto"
message_type { name: "Base" extension_range { start: 100 end: 200 } }
message_type { name: "Ext"
  extension { name: "field" number: 109825718 label: LABEL_OPTIONAL type: TYPE_MESSAGE
    type_name: ".e.Ext" extendee: ".x.JobSpec" } }
extension { name: "top" number: 5 label: LABEL_OPTIONAL type: TYPE_INT32 extendee: ".x.JobSpec" }
extension { name: "ok" number: 100 label: LABEL_OPTIONAL type: TYPE_INT32 extendee: ".e.Base" }
extension { name: "bad" number: 101 label: LABEL_OPTIONAL type: TYPE_MESSAGE
  type_name: ".x.Gone" extendee: ".e.Base" }
"""


def test_an_extend_block_of_orphans_is_an_orphan(tmp_path: Path) -> None:
    inp = tmp_path / "in"
    inp.mkdir()
    (inp / "e.textpb").write_text(INPUT, encoding="utf-8")
    env = {k: v for k, v in os.environ.items() if k != "REPROTO_VARIANT"}
    result = subprocess.run(
        [*REPROTO_CMD, f"-I{inp}", "-O", str(tmp_path / "out")],
        capture_output=True, text=True, env=env,
    )
    assert result.returncode == 0, result.stderr
    text = (tmp_path / "out" / "e.proto").read_text(encoding="utf-8")
    for expected in (
        "message Ext {\n///extend x.JobSpec {\n/// optional Ext field = 109825718;\n///}\n}",
        "///extend x.JobSpec {\n///optional int32 top = 5;\n///}",
        "extend Base {\n  optional int32 ok = 100;\n",
    ):
        assert expected in text, f"missing:\n{expected}\nin:\n{text}"
