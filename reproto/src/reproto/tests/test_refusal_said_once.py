# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""A refusal said once on stderr (spec 0474): no W20 repeating its file's
W19; the files refused only through an import counted on their root's
line. --detailed-warnings keeps every occurrence, and
--fail-on fails as before. The suite runs on reproto and on protoback
alike."""

import os
import subprocess
from pathlib import Path

from .conftest import REPROTO_CMD

ROW = """name: "a/row.proto" package: "a"
message_type { name: "Base" extension_range { start: 100 end: 200 } }
"""

# The root: extension 500 is in no extension range of a.Base.
USER = """name: "b/user.proto" package: "b" dependency: "a/row.proto"
message_type { name: "User" }
extension { name: "far" number: 500 label: LABEL_OPTIONAL type: TYPE_INT32
  extendee: ".a.Base" json_name: "far" }
"""


def _importer(name: str, deps: list[str]) -> str:
    package = name.split("/")[0]
    imports = " ".join(f'dependency: "{d}"' for d in deps)
    return f'name: "{name}" package: "{package}" {imports} message_type {{ name: "M" }}\n'


FILES = {
    "row.textpb": ROW,
    "user.textpb": USER,
    "c1.textpb": _importer("c1/c1.proto", ["b/user.proto"]),
    "c2.textpb": _importer("c2/c2.proto", ["b/user.proto"]),
    "d.textpb": _importer("d/d.proto", ["c1/c1.proto", "c2/c2.proto"]),
}

ROOT = ("'b/user.proto' : the descriptor pool refused it: field number 500 in "
        "extension b.far has no extension range in message a.Base")


def _run(tmp: Path, *args: str) -> subprocess.CompletedProcess[str]:
    inp = tmp / "in"
    inp.mkdir(exist_ok=True)
    for name, text in FILES.items():
        (inp / name).write_text(text, encoding="utf-8")
    env = {k: v for k, v in os.environ.items() if k != "REPROTO_VARIANT"}
    return subprocess.run(
        [*REPROTO_CMD, f"-I{inp}", "-O", str(tmp / "out"),
         f"--schema-db-out={tmp / 'db.desc'}", *args],
        capture_output=True, text=True, env=env,
    )


def _words(text: str) -> str:
    return " ".join(text.split())


def test_a_refusal_is_said_once(tmp_path: Path) -> None:
    result = _run(tmp_path)
    assert result.returncode == 0, result.stderr
    stderr = _words(result.stderr)
    assert f"[W19]: {ROOT} (and 3 files importing it) (1 occurrence)" in stderr, stderr
    assert stderr.count("[W19]") == 1, stderr
    assert "[W20]" not in stderr, stderr
    # Every occurrence, under --detailed-warnings (the report: protoback's
    # tests/repair.rs).
    detailed = _words(_run(tmp_path, "--detailed-warnings").stderr)
    assert detailed.count("[W19]") == 4 and detailed.count("[W20]") == 4, detailed
    assert "(and 3 files" not in detailed


def test_fail_on_still_fails(tmp_path: Path) -> None:
    result = _run(tmp_path, "--fail-on", "W20")
    assert result.returncode != 0, result.stderr
