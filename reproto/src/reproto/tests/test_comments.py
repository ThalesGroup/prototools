# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""A comment comes back as written (spec 0376).

`protoc` stores, for each comment line, exactly the text after `//`.
reproto renders it back as `//` plus that text: no stripping, and no
space of its own, so a decompiled comment reads as the original did.
"""

from __future__ import annotations

import subprocess
from pathlib import Path

from google.protobuf.descriptor_pb2 import FileDescriptorProto, FileDescriptorSet

from reproto.tests.test_emit_binary import _build_env, _get_fixture_path
from reproto.tests.conftest import REPROTO_CMD

FIXTURE = "comments.proto"


def _compile(proto_dir: Path, name: str, out: Path) -> FileDescriptorProto:
    """protoc with source info: the comments reproto renders come from it."""
    r = subprocess.run(
        [
            "protoc",
            "--include_source_info",
            f"-I{proto_dir}",
            f"--descriptor_set_out={out}",
            str(proto_dir / name),
        ],
        capture_output=True,
        text=True,
    )
    assert r.returncode == 0, f"protoc failed: {r.stderr}"
    fds = FileDescriptorSet()
    fds.ParseFromString(out.read_bytes())
    return fds.file[0]


def _decompile(tmp_path: Path) -> tuple[FileDescriptorProto, str]:
    """The fixture's descriptor, and reproto's rendering of it."""
    orig_dir = tmp_path / "orig"
    out_dir = tmp_path / "out"
    orig_dir.mkdir()
    out_dir.mkdir()
    (orig_dir / FIXTURE).write_text(
        _get_fixture_path(FIXTURE).read_text(encoding="utf-8"), encoding="utf-8"
    )
    pb = orig_dir / "comments.pb"
    fdp = _compile(orig_dir, FIXTURE, pb)
    r = subprocess.run(
        [
            *REPROTO_CMD,
            "--use-variant", "descriptor",
            f"-I{orig_dir}",
            f"--proto-out={out_dir}",
            str(pb),
        ],
        capture_output=True,
        text=True,
        env=_build_env(),
    )
    assert r.returncode == 0, f"reproto failed: {r.stderr}"
    return fdp, (out_dir / FIXTURE).read_text(encoding="utf-8")


def _rendered_comments(fdp: FileDescriptorProto) -> list[str]:
    """The stored comments reproto renders: the file's (path [] or [12],
    the syntax line) and each top-level message's (path [4, i])."""
    out = []
    for loc in fdp.source_code_info.location:
        path = list(loc.path)
        if path in ([], [12]) or (len(path) == 2 and path[0] == 4):
            out += [loc.leading_comments, loc.trailing_comments]
            out += list(loc.leading_detached_comments)
    return [c for c in out if c]


def test_the_fixture_covers_every_case(tmp_path: Path) -> None:
    fdp, _ = _decompile(tmp_path)
    lines = [
        line
        for comment in _rendered_comments(fdp)
        for line in comment.removesuffix("\n").split("\n")
    ]
    assert "" in lines, "an empty comment line"
    assert any(line and not line.startswith(" ") for line in lines), "//text"
    assert any(line.startswith("   ") for line in lines), "spaces after //"
    assert " before the syntax line." in lines, "a block comment"


def test_comments_come_back_as_written(tmp_path: Path) -> None:
    fdp, rendered = _decompile(tmp_path)
    # A line as rendered, without the indentation reproto gives it.
    out = [line.lstrip(" ") for line in rendered.splitlines()]
    for comment in _rendered_comments(fdp):
        for line in comment.removesuffix("\n").split("\n"):
            assert f"//{line}" in out, f"//{line!r} not rendered as written"
    trailing = [line for line in rendered.splitlines() if line.endswith(" ")]
    assert trailing == [], f"lines ending in a space: {trailing}"


def test_the_file_header_survives_a_second_protoc(tmp_path: Path) -> None:
    fdp, rendered = _decompile(tmp_path)
    again_dir = tmp_path / "again"
    again_dir.mkdir()
    (again_dir / FIXTURE).write_text(rendered, encoding="utf-8")
    again = _compile(again_dir, FIXTURE, again_dir / "again.pb")

    def header(f: FileDescriptorProto) -> list[str]:
        (syntax,) = [loc for loc in f.source_code_info.location if list(loc.path) == [12]]
        return list(syntax.leading_detached_comments)

    # reproto's own header comes first: the file's name.
    assert header(again) == [f" {FIXTURE}\n", *header(fdp)]


def test_reprotos_own_header_keeps_its_space(tmp_path: Path) -> None:
    _, rendered = _decompile(tmp_path)
    assert rendered.splitlines()[0] == f"// {FIXTURE}"
