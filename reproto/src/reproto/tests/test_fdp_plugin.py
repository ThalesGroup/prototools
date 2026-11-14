# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Regression tests for --fdp-plugin (spec 0369).

The plugin used to run only inside the phase-2 rank loop, by which point
phase 1 had already built ReFile.targets and finished the import-discovery
loop from the *unpatched* fdp.dependency.  An import a plugin added was
therefore never resolved.  Applying the plugin at every FDP materialisation
site, the first being parse_qfile, is what these tests pin down.

Fixtures (under tests/fixtures/):
  plugin_subject.proto  — imports nothing; the file the plugin patches
  plugin_target.proto   — referenced by nothing; reachable only via the patch
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

from reproto.tests.conftest import FIXTURES_DIR, REPROTO_CMD, compile_proto

# A plugin that adds an import of plugin_target.proto to plugin_subject.
ADD_IMPORT = """
def fdp_plugin(fdp):
    if fdp.name == 'plugin_subject.proto':
        fdp.dependency.append('plugin_target.proto')
"""

# The same, plus a field whose type lives in the imported file.  The field
# only survives if plugin_target.proto is ranked ahead of its importer.
ADD_IMPORT_AND_FIELD = """
def fdp_plugin(fdp):
    if fdp.name != 'plugin_subject.proto':
        return
    fdp.dependency.append('plugin_target.proto')
    field = fdp.message_type[0].field.add()
    field.name = 'target'
    field.number = 2
    field.type = 11              # TYPE_MESSAGE
    field.label = 1              # LABEL_OPTIONAL
    field.type_name = '.plugin_test.Target'
"""

ADD_MISSING_IMPORT = """
def fdp_plugin(fdp):
    if fdp.name == 'plugin_subject.proto':
        fdp.dependency.append('nowhere_to_be_found.proto')
"""


def _plugin(tmp_path: Path, source: str) -> Path:
    path = tmp_path / "plugin.py"
    path.write_text(source, encoding="utf-8")
    return path


def _run_reproto(
    pb_files: list[Path],
    out_dir: Path,
    extra_args: list[str] | None = None,
    include_dirs: list[Path] | None = None,
) -> subprocess.CompletedProcess[str]:
    src_path = str(Path(__file__).parent.parent.parent)
    pythonpath_parts = [src_path]
    if existing := os.environ.get("PYTHONPATH"):
        pythonpath_parts.append(existing)
    env = {**os.environ, "PYTHONPATH": os.pathsep.join(pythonpath_parts)}
    env.pop("REPROTO_VARIANT", None)

    dirs = include_dirs if include_dirs is not None else [FIXTURES_DIR]
    cmd = [
        *REPROTO_CMD,
        "--use-variant", "descriptor",
        *[f"-I{d}" for d in dirs],
        f"--proto-out={out_dir}",
    ]
    if extra_args:
        cmd.extend(extra_args)
    cmd.extend(str(p) for p in pb_files)

    return subprocess.run(cmd, capture_output=True, text=True, env=env)


def _setup(tmp_path: Path) -> tuple[Path, Path, Path]:
    """Compile both fixtures into pb_dir; seed only the subject.

    pb_dir doubles as the -I root, so plugin_target.proto resolves there
    (as plugin_target.pb) if and only if the discovery loop looks for it.
    """
    pb_dir = tmp_path / "pb"
    pb_dir.mkdir()
    out_dir = tmp_path / "out"
    out_dir.mkdir()
    subject_pb, _target_pb = compile_proto(
        pb_dir, "plugin_subject.proto", "plugin_target.proto"
    )
    return pb_dir, out_dir, subject_pb


# ---------------------------------------------------------------------------
# 1 — a plugin-added import is discovered and loaded
# ---------------------------------------------------------------------------

def test_plugin_added_import_is_discovered(tmp_path: Path) -> None:
    """The named file is read off -I, ranked, and rendered.

    Tier 3 of the spec Background: before the fix the file was never read
    from disk at all and the import dangled in the output.
    """
    pb_dir, out_dir, subject_pb = _setup(tmp_path)
    plugin = _plugin(tmp_path, ADD_IMPORT)

    result = _run_reproto(
        [subject_pb], out_dir,
        extra_args=[f"--fdp-plugin={plugin}"],
        include_dirs=[pb_dir],
    )

    assert result.returncode == 0, f"reproto failed:\n{result.stderr}"
    assert (out_dir / "plugin_subject.proto").exists()
    assert (out_dir / "plugin_target.proto").exists(), (
        "the plugin-added import was never loaded from -I"
    )
    rendered = (out_dir / "plugin_subject.proto").read_text()
    assert 'import "plugin_target.proto";' in rendered


# ---------------------------------------------------------------------------
# 2 — a field typed by the added import keeps its type
# ---------------------------------------------------------------------------

def test_plugin_added_import_keeps_its_fields(tmp_path: Path) -> None:
    """Tier 1: the silent failure, where the field is deleted with only a W4."""
    pb_dir, out_dir, subject_pb = _setup(tmp_path)
    plugin = _plugin(tmp_path, ADD_IMPORT_AND_FIELD)

    result = _run_reproto(
        [subject_pb], out_dir,
        extra_args=[f"--fdp-plugin={plugin}"],
        include_dirs=[pb_dir],
    )

    assert result.returncode == 0, f"reproto failed:\n{result.stderr}"
    rendered = (out_dir / "plugin_subject.proto").read_text()
    assert "Target target = 2;" in rendered, (
        f"plugin-added field was stripped:\n{rendered}"
    )
    assert "unresolvable" not in result.stderr.lower()


# ---------------------------------------------------------------------------
# 3 — the import is emitted once, though the plugin runs at every parse
# ---------------------------------------------------------------------------

def test_plugin_added_import_appears_once(tmp_path: Path) -> None:
    """Establishes the contents/desc invariant (S1).

    contents stays raw, so every parse hands the plugin a pristine FDP and
    the append cannot accumulate.  Would fail if a patched FDP were reused.
    """
    pb_dir, out_dir, subject_pb = _setup(tmp_path)
    plugin = _plugin(tmp_path, ADD_IMPORT)

    result = _run_reproto(
        [subject_pb], out_dir,
        extra_args=[f"--fdp-plugin={plugin}"],
        include_dirs=[pb_dir],
    )

    assert result.returncode == 0, f"reproto failed:\n{result.stderr}"
    rendered = (out_dir / "plugin_subject.proto").read_text()
    assert rendered.count('import "plugin_target.proto";') == 1, (
        f"import emitted more than once:\n{rendered}"
    )


# ---------------------------------------------------------------------------
# 4 — an unresolvable added import is stripped, not left dangling
# ---------------------------------------------------------------------------

def test_plugin_added_import_unresolvable_is_stripped(tmp_path: Path) -> None:
    """The negative case: the import must not stay live and point at nothing.

    It is not deleted outright — spec 0053 keeps a stripped dependency as an
    orphan comment line so the evidence survives — but it must be gone from
    the compilable output.
    """
    pb_dir, out_dir, subject_pb = _setup(tmp_path)
    plugin = _plugin(tmp_path, ADD_MISSING_IMPORT)

    result = _run_reproto(
        [subject_pb], out_dir,
        extra_args=[f"--fdp-plugin={plugin}"],
        include_dirs=[pb_dir],
    )

    assert result.returncode == 0, f"reproto failed:\n{result.stderr}"
    lines = [ln.strip() for ln in (out_dir / "plugin_subject.proto").read_text().splitlines()]
    assert 'import "nowhere_to_be_found.proto";' not in lines, (
        "unresolvable import left live in the output"
    )
    assert any(
        "nowhere_to_be_found.proto" in ln and ln.startswith("//") for ln in lines
    ), "a stripped import must be retained as an orphan comment (spec 0053)"
    assert "nowhere_to_be_found.proto" in result.stderr, (
        "stripping an import must be reported"
    )


# ---------------------------------------------------------------------------
# 5-8 — a plugin exception aborts the run and is never blamed on the input
#
# Each raises from a plugin that is otherwise a no-op.  Tests 6-8 are named
# for the handler that would absorb the throw if apply_fdp_plugin did not
# re-raise as PluginError; all three pass trivially with a bare raise and
# only fail once that wrapping is removed.
# ---------------------------------------------------------------------------

def _assert_plugin_abort(result: subprocess.CompletedProcess[str], out_dir: Path) -> None:
    assert result.returncode != 0, (
        f"a raising plugin must abort the run:\n{result.stdout}\n{result.stderr}"
    )
    assert "Traceback" not in result.stderr, (
        f"a plugin fault must not surface as a reproto crash:\n{result.stderr}"
    )
    assert "plugin raised" in result.stderr, (
        f"the diagnostic must name the plugin as the culprit:\n{result.stderr}"
    )
    assert not list(out_dir.glob("*.proto")), "no output may be written"


def test_plugin_exception_aborts_the_run(tmp_path: Path) -> None:
    """Baseline: an exception no handler on the path would catch."""
    pb_dir, out_dir, subject_pb = _setup(tmp_path)
    plugin = _plugin(tmp_path, """
def fdp_plugin(fdp):
    raise RuntimeError('boom')
""")

    result = _run_reproto(
        [subject_pb], out_dir,
        extra_args=[f"--fdp-plugin={plugin}"],
        include_dirs=[pb_dir],
    )

    _assert_plugin_abort(result, out_dir)
    assert "RuntimeError" in result.stderr


def test_plugin_decode_error_is_not_a_corrupt_input(tmp_path: Path) -> None:
    """Unwrapped, phases.py's DecodeError guard prunes the file and blames it.

    That is the worst of the three: the file silently vanishes from the
    output under a 'Skipping unparseable file' warning.
    """
    pb_dir, out_dir, subject_pb = _setup(tmp_path)
    plugin = _plugin(tmp_path, """
from google.protobuf.message import DecodeError


def fdp_plugin(fdp):
    raise DecodeError('not actually a decode failure')
""")

    result = _run_reproto(
        [subject_pb], out_dir,
        extra_args=[f"--fdp-plugin={plugin}"],
        include_dirs=[pb_dir],
    )

    _assert_plugin_abort(result, out_dir)
    assert "Skipping unparseable file" not in result.stderr, (
        "a plugin fault must not be reported as a corrupt input file"
    )


def test_plugin_type_error_is_not_a_pool_conflict(tmp_path: Path) -> None:
    """Unwrapped, phases.py's pool_db.Add guard blames a descriptor conflict."""
    pb_dir, out_dir, subject_pb = _setup(tmp_path)
    plugin = _plugin(tmp_path, """
def fdp_plugin(fdp):
    raise TypeError('not actually a pool conflict')
""")

    result = _run_reproto(
        [subject_pb], out_dir,
        extra_args=[f"--fdp-plugin={plugin}"],
        include_dirs=[pb_dir],
    )

    _assert_plugin_abort(result, out_dir)
    assert "Could not add descriptor" not in result.stderr, (
        "a plugin fault must not be reported as a pool conflict"
    )


def test_plugin_exception_on_a_fallback_aborts(tmp_path: Path) -> None:
    """The embedded-fallback load swallows AttributeError and reports nothing.

    Its only diagnostic is gated on --debug, so unwrapped this is silent by
    default: the fallback is never registered and its importers lose the
    import in phase 2.
    """
    pb_dir, out_dir, subject_pb = _setup(tmp_path)
    plugin = _plugin(tmp_path, """
def fdp_plugin(fdp):
    if fdp.name == 'google/protobuf/descriptor.proto':
        raise AttributeError('raised while patching a fallback')
""")

    result = _run_reproto(
        [subject_pb], out_dir,
        extra_args=[f"--fdp-plugin={plugin}"],
        include_dirs=[pb_dir],
    )

    _assert_plugin_abort(result, out_dir)
    assert "AttributeError" in result.stderr
