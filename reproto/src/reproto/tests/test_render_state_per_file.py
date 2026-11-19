# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""A failed render leaves nothing to the next file (spec 0429 S15).

Suspected: `ctx.syntax` and `ctx.out_sci` carried from one file into the
next when a render fails. Each render sets the file's state first
(`current_file`, `_set_target_syntax`, `out_sci`) and phase 7 gives each
file its own slot, so nothing is carried; this test holds it. A render
fails only on a bug, so the failure is made: a.proto's render dirties
the state as a failing render would, and raises. b.proto, rendered next,
comes out byte for byte as rendered alone, its text and its descriptor.

reproto only, in-process: protoback renders each file with its own
context, in parallel (spec 0429 S4), so nothing can be carried there.
"""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest
from click.testing import CliRunner

import reproto.re_file as re_file
from reproto.cli import main
from reproto.context import SourceCodeInfoOut

A = 'edition = "2023";\npackage a;\nmessage A { int32 x = 1; }\n'
B = 'syntax = "proto2";\npackage b;\nmessage B { optional int32 y = 1; }\n'


def _inputs(tmp: Path, names: list[str]) -> Path:
    src, root = tmp / "src", tmp / "pb"
    src.mkdir(parents=True, exist_ok=True)
    root.mkdir(parents=True, exist_ok=True)
    for name in names:
        (src / name).write_text({"a.proto": A, "b.proto": B}[name], encoding="utf-8")
        subprocess.run(["protoc", f"-I{src}", "--include_source_info",
                        f"--descriptor_set_out={root / name.replace('.proto', '.pb')}", name],
                       check=True, capture_output=True)
    return root


def _run(tmp: Path, names: list[str]) -> Path:
    root = _inputs(tmp, names)
    out = tmp / "out"
    result = CliRunner().invoke(
        main, ["--builtin", "descriptor", "--emit-binary", f"-I{root}", f"--proto-out={out}",
               *(n.replace(".proto", ".pb") for n in names)])
    assert result.exit_code == 0, result.output
    return out


def test_a_failed_render_leaves_nothing_to_the_next_file(
        tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    original = re_file.ReFileDescriptorProto.render
    order: list[str] = []

    def render(self, ctx, depth=0):  # type: ignore[no-untyped-def]
        order.append(self.name)
        if self.name == "a.proto":
            ctx.syntax = ctx.target_syntax = "editions"
            ctx.out_sci = SourceCodeInfoOut()
            raise KeyError("a render failing on a bug")
        return original(self, ctx, depth)

    monkeypatch.setattr(re_file.ReFileDescriptorProto, "render", render)
    out = _run(tmp_path / "both", ["a.proto", "b.proto"])
    monkeypatch.undo()
    alone = _run(tmp_path / "alone", ["b.proto"])

    rendered = [n for n in order if n in ("a.proto", "b.proto")]
    assert rendered.index("a.proto") < rendered.index("b.proto"), order
    assert not (out / "a.proto").exists()
    assert (out / "b.proto").read_bytes() == (alone / "b.proto").read_bytes()
    assert (out / "b.pb").read_bytes() == (alone / "b.pb").read_bytes()
