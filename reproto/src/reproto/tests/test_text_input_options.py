# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Custom options read from protoc's text of a descriptor set (spec 0426).

`protoc --decode=google.protobuf.FileDescriptorSet descriptor.proto` prints
a custom option, which descriptor.proto does not declare, by number. Each
test compiles a fixture with protoc, prints its set so, and requires reproto
to render from the text what it renders from the binary set, custom options
included, with the warnings of spec 0426 S4 where the text leaves the
framing open.
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

from reproto.tests.conftest import REPROTO_CMD

_OPTS = """\
syntax = "proto2";

package txt;

import "google/protobuf/descriptor.proto";

enum Level {
  LOW = 0;
  HIGH = 1;
}

message Info {
  optional string owner = 1;
  repeated int32 codes = 2;
  optional Info child = 3;
}

extend google.protobuf.FileOptions {
  optional string label = 50001;
}

extend google.protobuf.MessageOptions {
  optional Info info = 50002;
  optional Level level = 50003;
  optional fixed32 tag32 = 50004;
  optional double ratio = 50005;
}

extend google.protobuf.FieldOptions {
  repeated int64 marks = 50006;
  optional bool hidden = 50007;
  optional bytes blob = 50008;
}
"""

_MAIN = """\
syntax = "proto2";

package txt;

import "opts.proto";

option (txt.label) = "main";

message Holder {
  option (txt.info) = { owner: "me" codes: 1 codes: 2 child { owner: "kid" } };
  option (txt.level) = HIGH;
  option (txt.tag32) = 7;
  option (txt.ratio) = 0.5;

  optional int32 a = 1 [(txt.marks) = 3, (txt.marks) = -4, (txt.hidden) = true];
  optional string b = 2 [(txt.blob) = "\\001\\002"];
}
"""

# Options protoc prints as `N { … }`, declared in the file that uses them:
# a value framed as a group, a message, and strings whose bytes happen to
# read as a message, with a group inside, not empty ("shoppingProduct": W2)
# or empty ("UrlList": exact, no warning).
_DELIMITED = """\
edition = "2023";

package dlm;

import "google/protobuf/descriptor.proto";

message Holder {
  option (dlm.as_group) = { x: 1 inner { y: 2 } };
  option (dlm.as_message) = { x: 3 };
  option (dlm.as_string) = "shoppingProduct";
  option (dlm.as_string_exact) = "UrlList";
}

message Value {
  int32 x = 1;
  Inner inner = 2 [features.message_encoding = DELIMITED];

  message Inner {
    int32 y = 1;
  }
}

extend google.protobuf.MessageOptions {
  Value as_group = 50001 [features.message_encoding = DELIMITED];
  Value as_message = 50002;
  string as_string = 50003;
  string as_string_exact = 50004;
}
"""


def _compile(
    src: Path, protos: dict[str, str], main: str, schema: str | None = None,
) -> tuple[Path, Path]:
    """Compile ``main`` and its imports: the binary set, and its text,
    printed with ``descriptor.proto`` alone, or with ``schema`` and what it
    imports, which names the options it declares ``[pkg.ext]``."""
    for name, content in protos.items():
        (src / name).write_text(content, encoding="utf-8")
    pb = src / "set.pb"
    result = subprocess.run(
        ["protoc", "--include_imports", f"--descriptor_set_out={pb}",
         f"-I{src}", str(src / main)],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, result.stderr
    text = subprocess.run(
        ["protoc", "--decode=google.protobuf.FileDescriptorSet",
         f"-I{src}", schema or "google/protobuf/descriptor.proto"],
        input=pb.read_bytes(), capture_output=True,
    )
    assert text.returncode == 0, text.stderr.decode()
    textpb = src / "set.textpb"
    textpb.write_bytes(text.stdout)
    return pb, textpb


def _reproto(out: Path, inputs: list[Path]) -> tuple[dict[str, str], str]:
    """Render ``inputs``: each output file's text, and reproto's stderr,
    its whitespace collapsed (the console wraps long warnings)."""
    out.mkdir()
    env = {**os.environ}
    env.pop("REPROTO_VARIANT", None)
    result = subprocess.run(
        [*REPROTO_CMD, "--use-variant", "descriptor", f"--proto-out={out}",
         *[str(p) for p in inputs]],
        capture_output=True, text=True, env=env,
    )
    assert result.returncode == 0, result.stderr
    files = {
        str(p.relative_to(out)): p.read_text(encoding="utf-8")
        for p in sorted(out.rglob("*.proto"))
    }
    return files, " ".join(result.stderr.split())


def _from_binary_and_text(
    tmp_path: Path, protos: dict[str, str], main: str,
) -> tuple[dict[str, str], dict[str, str], str]:
    src = tmp_path / "src"
    src.mkdir()
    pb, textpb = _compile(src, protos, main)
    from_binary, _ = _reproto(tmp_path / "binary", [pb])
    from_text, stderr = _reproto(tmp_path / "text", [textpb])
    return from_binary, from_text, stderr


def test_custom_options_survive_the_text_form(tmp_path: Path) -> None:
    """Options of each kind, declared in an import: as from the binary set."""
    from_binary, from_text, stderr = _from_binary_and_text(
        tmp_path, {"opts.proto": _OPTS, "main.proto": _MAIN}, "main.proto",
    )
    assert "(txt.info)" in from_binary["main.proto"]
    assert from_text == from_binary
    assert "declared nowhere" not in stderr
    assert "printed as a message" not in stderr


def test_a_group_option_declared_in_its_own_file(tmp_path: Path) -> None:
    """Framings the text leaves open, settled by the file's declarations."""
    from_binary, from_text, stderr = _from_binary_and_text(
        tmp_path, {"delimited.proto": _DELIMITED}, "delimited.proto",
    )
    assert "shoppingProduct" in from_binary["delimited.proto"]
    assert from_text == from_binary
    assert stderr.count("printed as a message") == 1
    assert "field 50003 of google.protobuf.MessageOptions" in stderr


def test_an_option_declared_nowhere_is_reported(tmp_path: Path) -> None:
    """The declaring file left out of the input: W1, and the file rendered."""
    src = tmp_path / "src"
    src.mkdir()
    _, textpb = _compile(
        src, {"opts.proto": _OPTS, "main.proto": _MAIN}, "main.proto",
    )
    # main.proto's entry alone: opts.proto, which declares its options, is
    # missing from the input.
    text = textpb.read_text(encoding="utf-8")
    start = text.index('file {\n  name: "main.proto"')
    alone = src / "main.textpb"
    alone.write_text(text[start:], encoding="utf-8")
    files, stderr = _reproto(tmp_path / "out", [alone])
    assert "main.proto" in files
    assert ("field 50002 of google.protobuf.MessageOptions is declared "
            "nowhere") in stderr


def test_options_printed_by_name_survive_the_text_form(tmp_path: Path) -> None:
    """Text printed with the options' schema, ``[txt.info] { … }``: the
    names resolved from the input's own declarations.

    Old reproto resolved them only through the variant's
    ``annotation_modules``, Python modules declaring the same options.
    """
    src = tmp_path / "src"
    src.mkdir()
    pb, textpb = _compile(
        src, {"opts.proto": _OPTS, "main.proto": _MAIN}, "main.proto",
        schema="opts.proto",
    )
    assert "[txt.info]" in textpb.read_text(encoding="utf-8")
    from_binary, _ = _reproto(tmp_path / "binary", [pb])
    from_text, stderr = _reproto(tmp_path / "text", [textpb])
    assert "(txt.info)" in from_binary["main.proto"]
    assert from_text == from_binary
    assert "declared nowhere" not in stderr


def test_an_option_by_name_declared_nowhere_is_reported(tmp_path: Path) -> None:
    """Text by name, the declaring file left out of the input: W3, and the
    file rendered."""
    src = tmp_path / "src"
    src.mkdir()
    _, textpb = _compile(
        src, {"opts.proto": _OPTS, "main.proto": _MAIN}, "main.proto",
        schema="opts.proto",
    )
    text = textpb.read_text(encoding="utf-8")
    start = text.index('file {\n  name: "main.proto"')
    alone = src / "main.textpb"
    alone.write_text(text[start:], encoding="utf-8")
    files, stderr = _reproto(tmp_path / "out", [alone])
    assert "main.proto" in files
    assert ("extension [txt.info] of google.protobuf.MessageOptions is "
            "declared nowhere") in stderr


def test_a_file_whose_import_is_missing_keeps_its_own_options(
    tmp_path: Path,
) -> None:
    """Options declared in the file itself, whose import is missing from
    the input: as from the binary set (the input's pool mends the file,
    spec 0427 S2), with only the W2 of ``"shoppingProduct"``."""
    src = tmp_path / "src"
    src.mkdir()
    delimited = _DELIMITED.replace(
        'import "google/protobuf/descriptor.proto";',
        'import "google/protobuf/descriptor.proto";\nimport "gone.proto";',
    )
    pb, textpb = _compile(
        src,
        {"gone.proto": 'syntax = "proto3";\npackage gone;\nmessage G {}\n',
         "delimited.proto": delimited},
        "delimited.proto",
    )
    # delimited.proto alone, without gone.proto, in both forms.
    alone_pb = src / "alone.pb"
    result = subprocess.run(
        ["protoc", f"--descriptor_set_out={alone_pb}", f"-I{src}",
         str(src / "delimited.proto")],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, result.stderr
    text = textpb.read_text(encoding="utf-8")
    alone_text = src / "alone.textpb"
    alone_text.write_text(
        text[text.index('file {\n  name: "delimited.proto"'):], encoding="utf-8",
    )
    from_binary, _ = _reproto(tmp_path / "binary", [alone_pb])
    from_text, stderr = _reproto(tmp_path / "text", [alone_text])
    assert "shoppingProduct" in from_binary["delimited.proto"]
    assert from_text == from_binary
    assert "declared nowhere" not in stderr
    assert stderr.count("printed as a message") == 1


def test_text_that_does_not_parse_is_reported_and_skipped(tmp_path: Path) -> None:
    """One file of the input does not parse: said so, with its line and
    column, and the others rendered."""
    src = tmp_path / "src"
    src.mkdir()
    _, textpb = _compile(
        src, {"opts.proto": _OPTS, "main.proto": _MAIN}, "main.proto",
    )
    text = textpb.read_text(encoding="utf-8")
    start = text.index('file {\n  name: "main.proto"')
    # main.proto's entry broken: a value missing.
    broken = text[:start] + text[start:].replace(
        'name: "main.proto"', 'name: "main.proto"\n  package:', 1)
    bad = src / "bad.textpb"
    bad.write_text(broken, encoding="utf-8")
    files, stderr = _reproto(tmp_path / "out", [bad])
    assert "opts.proto" in files
    assert "main.proto" not in files
    assert "Cannot parse" in stderr
