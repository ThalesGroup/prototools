# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Spec 0447 S3, S6, S7: --target, --editions, and profiles.

A profile is a saved set of options, found as a path, in
$PROTOBACK_PROFILE_PATH, or among the built-in ones; the command line
overrides it, lists accumulate. Its folder's files replace the input's,
with W29 when the replacement lacks a definition of the copy it replaces.
"""

import os
import subprocess
from pathlib import Path

from .conftest import REPROTO_CMD

EDITIONS = '''name: "e.proto" package: "e" syntax: "editions" edition: EDITION_2023
message_type { name: "E" field { name: "x" number: 1 label: LABEL_OPTIONAL type: TYPE_INT32 } }
'''
TWO_FIELDS = '''name: "a.proto" package: "a" message_type { name: "A"
  field { name: "x" number: 1 label: LABEL_OPTIONAL type: TYPE_INT32 }
  field { name: "y" number: 2 label: LABEL_OPTIONAL type: TYPE_INT32 } }
'''
ONE_FIELD = '''name: "a.proto" package: "a" message_type { name: "A"
  field { name: "x" number: 1 label: LABEL_OPTIONAL type: TYPE_INT32 } }
'''


def _run(tmp_path: Path, files: dict[str, str], *flags: str,
         profile_path: Path | None = None) -> subprocess.CompletedProcess[str]:
    inp = tmp_path / "in"
    inp.mkdir(exist_ok=True)
    for name, text in files.items():
        (inp / name).write_text(text, encoding="utf-8")
    env = {k: v for k, v in os.environ.items()
           if k not in ("REPROTO_VARIANT", "PROTOBACK_PROFILE_PATH")}
    if profile_path is not None:
        env["PROTOBACK_PROFILE_PATH"] = str(profile_path)
    return subprocess.run([*REPROTO_CMD, *flags, f"-I{inp}", "-O", str(tmp_path / "out")],
                          capture_output=True, text=True, env=env, cwd=tmp_path)


def test_the_builtin_prost_profile_writes_editions_as_proto2(tmp_path: Path) -> None:
    r = _run(tmp_path, {"e.textpb": EDITIONS}, "--profile", "prost")
    assert r.returncode == 0, r.stderr
    e = (tmp_path / "out" / "e.proto").read_text(encoding="utf-8")
    assert 'syntax = "proto2";' in e, e


def test_the_command_line_overrides_a_profile(tmp_path: Path) -> None:
    r = _run(tmp_path, {"e.textpb": EDITIONS}, "--profile", "prost", "--editions", "keep")
    assert r.returncode == 0, r.stderr
    e = (tmp_path / "out" / "e.proto").read_text(encoding="utf-8")
    assert 'edition = "2023";' in e, e


def test_a_profile_is_found_as_a_path(tmp_path: Path) -> None:
    (tmp_path / "mine.yaml").write_text("editions: proto2\n", encoding="utf-8")
    r = _run(tmp_path, {"e.textpb": EDITIONS}, "--profile", "mine")
    assert r.returncode == 0, r.stderr
    e = (tmp_path / "out" / "e.proto").read_text(encoding="utf-8")
    assert 'syntax = "proto2";' in e, e


def test_a_profile_not_found_is_a_usage_error(tmp_path: Path) -> None:
    r = _run(tmp_path, {"a.textpb": ONE_FIELD}, "--profile", "ghost")
    assert r.returncode == 2, r.stderr
    assert ("profile 'ghost' not found (looked in: the current directory, "
            "the built-in profiles)") in r.stderr, r.stderr


def test_an_unknown_key_is_a_usage_error(tmp_path: Path) -> None:
    profs = tmp_path / "profs"
    profs.mkdir()
    (profs / "bad.yaml").write_text("target: keep\nnope: 1\n", encoding="utf-8")
    r = _run(tmp_path, {"a.textpb": ONE_FIELD}, "--profile", "bad", profile_path=profs)
    assert r.returncode == 2, r.stderr
    assert "profile 'bad': unknown key(s): nope" in r.stderr, r.stderr


def test_a_profile_folder_replaces_the_input_and_w29_says_what_is_lost(tmp_path: Path) -> None:
    profs = tmp_path / "profs"
    (profs / "rich").mkdir(parents=True)
    (profs / "rich.yaml").write_text("fail_on: [W29]\n", encoding="utf-8")
    (profs / "rich" / "a.textpb").write_text(ONE_FIELD, encoding="utf-8")
    r = _run(tmp_path, {"a.textpb": TWO_FIELDS}, "--profile", "rich", profile_path=profs)
    assert (f"Warning [W29]: 'a.proto': the copy in {profs / 'rich'} lacks what the "
            "input's defines: a.A.y") in r.stderr, r.stderr
    # fail_on came from the profile.
    assert r.returncode == 3, r.stderr
    a = (tmp_path / "out" / "a.proto").read_text(encoding="utf-8")
    assert "int32 x = 1" in a and "int32 y" not in a, a


def test_a_superset_replaces_silently(tmp_path: Path) -> None:
    profs = tmp_path / "profs"
    (profs / "rich").mkdir(parents=True)
    (profs / "rich.yaml").write_text("fail_on: [W29]\n", encoding="utf-8")
    (profs / "rich" / "a.textpb").write_text(TWO_FIELDS, encoding="utf-8")
    r = _run(tmp_path, {"a.textpb": ONE_FIELD}, "--profile", "rich", profile_path=profs)
    assert r.returncode == 0, r.stderr
    a = (tmp_path / "out" / "a.proto").read_text(encoding="utf-8")
    assert "int32 y = 2" in a, a


def test_target_portable_is_a_usage_error_for_now(tmp_path: Path) -> None:
    # reproto never implements it (spec 0447 S9); protoback in phase C.
    r = _run(tmp_path, {"a.textpb": ONE_FIELD}, "--target", "portable")
    assert r.returncode == 2, r.stderr
    assert "--target portable: not implemented" in r.stderr, r.stderr
