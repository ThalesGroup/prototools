# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""reproto prints its own completion scripts (spec 0422).

Each call runs the program in a fresh Python process, as a shell does:
the completion classes are registered per process, when reproto.cli is
imported, and reproto-instantiate-schema must keep Click's own.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

_SRC = Path(__file__).parent.parent.parent
_SCRIPTS = _SRC / "reproto"


def _run(module: str, prog_name: str, env: dict[str, str],
         cwd: Path | None = None) -> str:
    pythonpath_parts = [str(_SRC)]
    if existing := os.environ.get("PYTHONPATH"):
        pythonpath_parts.append(existing)
    result = subprocess.run(
        [sys.executable, "-c",
         f"from {module} import main; main(prog_name={prog_name!r})"],
        capture_output=True, text=True, cwd=cwd,
        env={**os.environ, "PYTHONPATH": os.pathsep.join(pythonpath_parts),
             **env},
    )
    assert result.returncode == 0, result.stderr
    return result.stdout


def _source(shell: str) -> str:
    return _run("reproto.cli", "reproto",
                {"_REPROTO_COMPLETE": f"{shell}_source"})


def test_S2_bash_source_is_completions_sh() -> None:
    # Trailing newlines aside: Click 8.3 prints one after the script, 8.4
    # none (spec 0423 S2).
    script = (_SCRIPTS / "completions.sh").read_text()
    assert _source("bash").rstrip("\n") == script.rstrip("\n")


def test_S3_S4_zsh_and_fish_sources_are_reprotos() -> None:
    for shell in ("zsh", "fish"):
        script = _source(shell)
        assert "_REPROTO_COMPLETE=" + shell + "_complete" in script, script
        assert "--command reproto" in script or "compdef _reproto_completion reproto" in script
        assert "arg_I" in script, f"{shell}: not reproto's script"
        assert "%(" not in script, f"{shell}: placeholder left"


def test_N2_instantiate_keeps_clicks_scripts() -> None:
    for shell in ("zsh", "fish"):
        script = _run(
            "reproto.instantiate_cli", "reproto-instantiate-schema",
            {"_REPROTO_INSTANTIATE_SCHEMA_COMPLETE": f"{shell}_source"},
        )
        assert "_REPROTO_INSTANTIATE_SCHEMA_COMPLETE" in script, script
        assert "arg_I" not in script, f"{shell}: reproto's script leaked"


def test_S3_S4_responses_carry_reprotos_types(tmp_path: Path) -> None:
    """zsh and fish get the same candidates as bash, in Click's formats."""
    (tmp_path / "descs" / "sub").mkdir(parents=True)
    (tmp_path / "descs" / "a.pb").touch()
    (tmp_path / "descs" / "notes.txt").touch()

    zsh = _run("reproto.cli", "reproto", {
        "_REPROTO_COMPLETE": "zsh_complete",
        "COMP_WORDS": "reproto -I descs ", "COMP_CWORD": "3",
    }, cwd=tmp_path)
    assert zsh.splitlines() == ["arg_I", "a.pb", "_", "arg_I", "sub/", "_"]

    fish = _run("reproto.cli", "reproto", {
        "_REPROTO_COMPLETE": "fish_complete",
        "COMP_WORDS": "reproto -I descs ", "COMP_CWORD": "",
    }, cwd=tmp_path)
    assert fish.splitlines() == ["arg_I,a.pb", "arg_I,sub/"]
