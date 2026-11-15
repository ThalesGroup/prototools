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

_POOL_REFUSED = "the descriptor pool refused this file or one of its imports"


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


def test_a_file_the_pool_refuses_says_so_in_fixed_words(tmp_path: Path) -> None:
    pb = _extender_alone(tmp_path)
    result, out = _reproto(tmp_path, pb, pb.parent)
    text = (out / "ext_no_options_extender.proto").read_text(encoding="utf-8")
    stderr = _words(result.stderr)
    assert f"// OMITTED[render]: file options could not be rendered ({_POOL_REFUSED})" in text
    assert f"'ext_no_options_extender.proto' : {_POOL_REFUSED}" in stderr
    # Neither upb's text nor what was parsed out of it.
    for upb in ("couldn't resolve name", "Couldn't build proto file"):
        assert upb not in text
        assert upb not in stderr
    assert "unresolvable type .ext_test.Config" not in stderr
    # The missing import is still reported, as before.
    assert "missing dependency file:ext_no_options_config.proto" in stderr


def test_debug_shows_the_pools_own_reason(tmp_path: Path) -> None:
    pb = _extender_alone(tmp_path)
    result, out = _reproto(tmp_path, pb, pb.parent, "--debug")
    assert "couldn't resolve name '.ext_test.Config'" in _words(result.stderr)
    text = (out / "ext_no_options_extender.proto").read_text(encoding="utf-8")
    assert "couldn't resolve name" not in text

