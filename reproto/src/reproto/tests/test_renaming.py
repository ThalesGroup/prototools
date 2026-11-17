# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""The variant's rewrites, one renaming (spec 0440).

A variant rewriting `priv/` to `google/protobuf/` and `.priv.` to
`.google.protobuf.`: names are written as spec 0434 writes them, in the
output namespace (S3); two symbols rewritten onto one name stop the run
(S2); --keep-descriptor-path keeps the input's names.
"""

import os
import subprocess
from pathlib import Path

import pytest

from .conftest import REPROTO_CMD

VARIANTS = Path(__file__).resolve().parents[1] / "variants"

EXTRA = """syntax = "proto2";
package priv;
import "google/protobuf/descriptor.proto";
message Empty { optional int32 marker = 1; }
message Note { optional string text = 1; optional Empty empty = 2; }
extend google.protobuf.FieldOptions { optional string tag = 50201; }
"""
USE = """syntax = "proto2";
package app;
import "priv/extra.proto";
import "google/protobuf/empty.proto";
message M {
  optional priv.Note note = 1 [(priv.tag) = "t"];
  optional priv.Empty mine = 2;
  optional google.protobuf.Empty theirs = 3;
}
message N {
  message google {}
  optional .priv.Note note = 1;
}
"""


def _variant(tmp_path: Path) -> Path:
    text = (VARIANTS / "google-protobuf.yaml").read_text(encoding="utf-8")
    text = text.replace("name: google.protobuf", "name: priv-test", 1)
    text = text.replace(
        "import_rewrites: []",
        "import_rewrites:\n  - match: priv/\n    action: rewrite\n    to: google/protobuf/", 1)
    text = text.replace(
        "namespace_rewrites: []",
        "namespace_rewrites:\n  - match: .priv.\n    action: rewrite\n    to: .google.protobuf.", 1)
    path = tmp_path / "variant.yaml"
    path.write_text(text, encoding="utf-8")
    return path


def _run(tmp_path: Path, *flags: str) -> subprocess.CompletedProcess[str]:
    src = tmp_path / "src"
    (src / "priv").mkdir(parents=True, exist_ok=True)
    (src / "app").mkdir(exist_ok=True)
    (src / "priv" / "extra.proto").write_text(EXTRA, encoding="utf-8")
    (src / "app" / "use.proto").write_text(USE, encoding="utf-8")
    inp = tmp_path / "in"
    inp.mkdir(exist_ok=True)
    subprocess.run(["protoc", f"-I{src}", "--include_imports",
                    f"--descriptor_set_out={inp / 'in.pb'}", "app/use.proto"],
                   check=True, capture_output=True)
    env = {k: v for k, v in os.environ.items() if k != "REPROTO_VARIANT"}
    return subprocess.run(
        [*REPROTO_CMD, "--proto-variant", str(_variant(tmp_path)), *flags,
         f"-I{inp}", "-O", str(tmp_path / "out")],
        capture_output=True, text=True, env=env,
    )


def test_two_symbols_rewritten_onto_one_name_stop_the_run(tmp_path: Path) -> None:
    result = _run(tmp_path)
    assert result.returncode == 1, result.stderr
    assert ("'priv/extra.proto' and 'google/protobuf/empty.proto' both define "
            "'google.protobuf.Empty' once rewritten; prune one (-p)") in result.stderr
    # The outermost name only: not its field.
    assert "Empty.marker" not in result.stderr
    assert not (tmp_path / "out" / "app" / "use.proto").exists()


def test_rewritten_names_are_written_in_the_output_namespace(tmp_path: Path) -> None:
    result = _run(tmp_path, "-p", "file:google/protobuf/empty.proto")
    assert result.returncode == 0, result.stderr
    out = tmp_path / "out"
    use = (out / "app" / "use.proto").read_text(encoding="utf-8")
    extra = (out / "google" / "protobuf" / "extra.proto").read_text(encoding="utf-8")
    # From another package: in full, no dot; the option name too.
    assert 'optional google.protobuf.Note note = 1 [(google.protobuf.tag) = "t"];' in use
    # Where `google` is captured (N.google): the dot.
    assert "optional .google.protobuf.Note note = 1;" in use
    # Within its own output package: short.
    assert "optional Empty empty = 2;" in extra
    compiled = subprocess.run(["protoc", f"-I{out}", "-o", os.devnull, "app/use.proto"],
                              capture_output=True, text=True)
    assert compiled.returncode == 0, compiled.stderr


@pytest.mark.parametrize("prune", [[], ["-p", "file:google/protobuf/empty.proto"]])
def test_keep_descriptor_path_keeps_the_input_names(tmp_path: Path, prune: list[str]) -> None:
    result = _run(tmp_path, "--keep-descriptor-path", *prune)
    assert result.returncode == 0, result.stderr
    text = (tmp_path / "out" / "app" / "use.proto").read_text(encoding="utf-8")
    assert 'optional priv.Note note = 1 [(priv.tag) = "t"];' in text
