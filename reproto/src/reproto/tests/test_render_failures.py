# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""A failed render's messages are fixed (spec 0429 S16).

What a render failure writes, in the `.proto` comment and the W6
warning, comes from its cause, not from the exception's text: that text
is upb's, which changes with its version, and protoback's upb is not
Python protobuf's. The text is shown under `--debug` only.
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

from reproto.tests.conftest import FIXTURES_DIR, REPROTO_CMD

# Why the pool refuses the extender (spec 0438 S8): upb's own message, the
# one text both tools' upb share; never Python's wrapping of it.
_REFUSED = ("the descriptor pool refused it: "
            "couldn't resolve name '.ext_test.Config'")


def _words(text: str) -> str:
    """The text with its whitespace normalized: the console wraps long
    warnings at its width."""
    return " ".join(text.split())


def _reproto(tmp: Path, pb: Path, include: Path, *args: str) -> tuple[subprocess.CompletedProcess[str], Path]:
    out = tmp / "out"
    env = {**os.environ}
    env.pop("REPROTO_VARIANT", None)
    result = subprocess.run(
        [*REPROTO_CMD, "--use-variant", "descriptor", f"-I{include}",
         f"--proto-out={out}", *args, str(pb.relative_to(include))],
        capture_output=True, text=True, env=env,
    )
    assert result.returncode == 0, result.stderr
    return result, out


def _extender_alone(tmp: Path) -> Path:
    """ext_no_options_extender.proto without the file declaring the
    message it extends: the pool cannot build it."""
    root = tmp / "pb"
    root.mkdir()
    pb = root / "extender.pb"
    result = subprocess.run(
        ["protoc", f"-I{FIXTURES_DIR}", f"--descriptor_set_out={pb}",
         "ext_no_options_extender.proto"],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, result.stderr
    return pb


def test_a_file_the_pool_refuses_says_why(tmp_path: Path) -> None:
    pb = _extender_alone(tmp_path)
    result, out = _reproto(tmp_path, pb, pb.parent)
    text = (out / "ext_no_options_extender.proto").read_text(encoding="utf-8")
    stderr = _words(result.stderr)
    assert f"// OMITTED[render]: file options could not be rendered ({_REFUSED})" in text
    assert f"'ext_no_options_extender.proto' : {_REFUSED}" in stderr
    # upb's message, not Python's wrapping of it, nor a type parsed out.
    assert "Couldn't build proto file" not in text + stderr
    assert "unresolvable type .ext_test.Config" not in stderr
    # The missing import is still reported, as before.
    assert "missing dependency file:ext_no_options_config.proto" in stderr


def test_debug_shows_the_pools_own_reason(tmp_path: Path) -> None:
    pb = _extender_alone(tmp_path)
    result, out = _reproto(tmp_path, pb, pb.parent, "--debug")
    assert "couldn't resolve name '.ext_test.Config'" in _words(result.stderr)


_M = 'syntax = "proto2";\npackage p;\nmessage M { optional int32 x = 1; }\n'
_U = ('syntax = "proto2";\npackage q;\nimport "m2.proto";\n'
      'message U { optional p.M m = 1; }\n')


def test_a_file_refused_for_its_import_names_it(tmp_path: Path) -> None:
    """m.proto and m2.proto both define p.M; u.proto imports m2.proto. Kept
    both (--keep-duplicates), the pool refuses m2.proto itself, and u.proto
    for its import: W19 and W20 say which."""
    src = tmp_path / "src"
    root = tmp_path / "pb"
    src.mkdir()
    root.mkdir()
    for name, body in {"m.proto": _M, "m2.proto": _M.replace("x = 1", "y = 1"),
                       "u.proto": _U}.items():
        (src / name).write_text(body, encoding="utf-8")
        subprocess.run(["protoc", f"-I{src}",
                        f"--descriptor_set_out={root / name.replace('.proto', '.pb')}",
                        name], check=True, capture_output=True)
    env = {**os.environ}
    env.pop("REPROTO_VARIANT", None)
    result = subprocess.run(
        [*REPROTO_CMD, "--use-variant", "descriptor", "--keep-duplicates",
         f"-I{root}", f"--proto-out={tmp_path / 'out'}",
         f"--schema-db-out={tmp_path / 'db.desc'}"],
        capture_output=True, text=True, env=env,
    )
    assert result.returncode == 0, result.stderr
    stderr = _words(result.stderr)
    itself = "the descriptor pool refused it: duplicate symbol 'p.M'"
    imported = "the descriptor pool refused its import 'm2.proto'"
    assert f"[W19]: 'm2.proto' : {itself}" in stderr
    assert f"[W19]: 'u.proto' : {imported}" in stderr
    assert f"[W20]: 'm2.proto' schema db: {itself}" in stderr
    assert f"[W20]: 'u.proto' schema db: {imported}" in stderr
