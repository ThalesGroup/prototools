# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Regression tests for extension-field option rendering (spec 0041 / re_field.py fix).

Before the fix, reproto called FindExtensionByNumber() on the descriptor pool
to obtain a FieldDescriptor so it could render field options.  For extension
fields whose defining file was not successfully added to the pool (e.g. because
of missing transitive dependencies), this lookup raised KeyError("Couldn't find
Extension N"), which was caught and emitted as a spurious W6 warning even when
the extension field carried no options at all.

The fix: build the dynamic FieldOptions message directly from the FDP's own
options bytes, bypassing the pool lookup entirely.

Fixtures (under tests/fixtures/):
  ext_no_options_config.proto   — Config message with extension range 100-200
  ext_no_options_extender.proto — extends Config; extension field has NO options
  ext_with_options_extender.proto — extends Config; extension field has [deprecated = true]
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

from reproto.tests.conftest import FIXTURES_DIR, REPROTO_CMD, compile_proto


def _run_reproto(
    pb_files: list[Path],
    out_dir: Path,
    extra_args: list[str] | None = None,
) -> subprocess.CompletedProcess[str]:
    src_path = str(Path(__file__).parent.parent.parent)
    pythonpath_parts = [src_path]
    if existing := os.environ.get("PYTHONPATH"):
        pythonpath_parts.append(existing)
    env = {**os.environ, "PYTHONPATH": os.pathsep.join(pythonpath_parts)}
    env.pop("REPROTO_VARIANT", None)

    cmd = [
        *REPROTO_CMD,
        "--use-variant", "descriptor",
        f"-I{FIXTURES_DIR}",
        f"--proto-out={out_dir}",
    ]
    if extra_args:
        cmd.extend(extra_args)
    cmd.extend(str(p) for p in pb_files)
    return subprocess.run(cmd, capture_output=True, text=True, env=env)


# ---------------------------------------------------------------------------
# T1 — Extension field with no options: no W6 "Couldn't find Extension" warning
# ---------------------------------------------------------------------------

def test_extension_no_options_no_w6_warning(tmp_path: Path) -> None:
    """An extension field with no field options must not produce a W6 warning."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    config_pb, no_opts_pb = compile_proto(
        pb_dir,
        "ext_no_options_config.proto",
        "ext_no_options_extender.proto",
    )

    result = _run_reproto([config_pb, no_opts_pb], out_dir)

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    assert "Couldn't find Extension" not in result.stderr, (
        f"Spurious W6 warning must not appear for option-less extension.\n"
        f"stderr:\n{result.stderr}"
    )
    assert "field options could not be rendered" not in result.stderr, (
        f"No render warning expected.\nstderr:\n{result.stderr}"
    )
    # The field must appear in the output without any WARNING comment
    out = (out_dir / "ext_no_options_extender.proto").read_text()
    assert "optional Provider provider = 100;" in out
    assert "WARNING" not in out


# ---------------------------------------------------------------------------
# T2 — Extension field WITH options: options render correctly, no W6 warning
# ---------------------------------------------------------------------------

def test_extension_with_options_renders_correctly(tmp_path: Path) -> None:
    """An extension field with [deprecated = true] must render the option correctly."""
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()

    config_pb, with_opts_pb = compile_proto(
        pb_dir,
        "ext_no_options_config.proto",
        "ext_with_options_extender.proto",
    )

    result = _run_reproto([config_pb, with_opts_pb], out_dir)

    assert result.returncode == 0, f"reproto crashed:\n{result.stderr}"
    assert "Couldn't find Extension" not in result.stderr, (
        f"Spurious W6 warning must not appear.\nstderr:\n{result.stderr}"
    )
    out = (out_dir / "ext_with_options_extender.proto").read_text()
    assert "deprecated = true" in out, (
        f"[deprecated = true] must be rendered.\nOutput:\n{out}"
    )
    assert "WARNING" not in out
