# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Tests for --emit-binary output (spec 0076).

Three cases:

  Case 1 — no-translation (proto2, proto3, editions without forced downconversion)
    Run reproto --emit-binary on the full set of roundtrip fixtures.
    Extract the FileDescriptorProto for the target file from orig.pb (a
    FileDescriptorSet), decode both orig and emit as annotated prototext, strip
    source_code_info lines, and compare textually.

  Case 2 — proto3/editions→proto2 via --force-proto2-output
    Golden .pb comparison using editions_rendering.proto as input fixture.

  Case 3 — editions→proto2 via --force-proto2-for-editions
    Golden .pb comparison using editions_rendering.proto as input fixture.
"""

from __future__ import annotations

import importlib
import importlib.resources
import os
import subprocess
from pathlib import Path

import pytest

from reproto.tests.test_roundtrip import (
    DEFAULT_FIXTURES,
    EDITION_FIXTURES,
    FIXTURE_COMPANIONS,
    POLYGLOT_FIXTURES_LOSSY,
    POLYGLOT_FIXTURES_STRICT,
    get_fixture_content,
)
from reproto.tests.conftest import REPROTO_CMD

# All case-1 fixtures: same coverage as the existing roundtrip suite.
EMIT_BINARY_FIXTURES = (
    DEFAULT_FIXTURES
    + EDITION_FIXTURES
    + POLYGLOT_FIXTURES_STRICT
    + POLYGLOT_FIXTURES_LOSSY
)


def _get_fixture_path(name: str) -> Path:
    pkg = importlib.import_module('reproto.tests.fixtures')
    files = importlib.resources.files(pkg)
    return Path(str(files.joinpath(name)))


def _build_env() -> dict:
    src_path = str(Path(__file__).parent.parent.parent)
    pythonpath_parts = [src_path]
    if existing := os.environ.get("PYTHONPATH"):
        pythonpath_parts.append(existing)
    env = {**os.environ, "PYTHONPATH": os.pathsep.join(pythonpath_parts)}
    env.pop("REPROTO_VARIANT", None)
    return env


def _reproto_cmd(orig_dir: Path, out_dir: Path, extra_args: list[str]) -> list[str]:
    return [
        *REPROTO_CMD,
        "--use-variant", "descriptor",
        "--emit-binary",
        f"-I{orig_dir}",
        f"--proto-out={out_dir}",
        *extra_args,
    ]


def _decode_fdp(pb_path: Path) -> str:
    """Decode a bare FileDescriptorProto .pb as annotated prototext."""
    r = subprocess.run(
        ["prototext", "decode",
         "-t", "google.protobuf.FileDescriptorProto",
         str(pb_path)],
        capture_output=True, text=True,
    )
    assert r.returncode == 0, f"prototext decode failed: {r.stderr}"
    return r.stdout


def _extract_fdp(fds_pb: Path, file_name: str, out_path: Path) -> None:
    """Extract a single FileDescriptorProto by name from a FileDescriptorSet .pb."""
    from google.protobuf.descriptor_pb2 import FileDescriptorSet
    fds = FileDescriptorSet()
    fds.ParseFromString(fds_pb.read_bytes())
    for fdp in fds.file:
        if fdp.name == file_name:
            out_path.write_bytes(fdp.SerializeToString())
            return
    raise AssertionError(f"{file_name!r} not found in {fds_pb}")


def _strip_source_code_info(text: str) -> str:
    """Remove source_code_info blocks and location lines from annotated prototext.

    reproto intentionally omits source_code_info; we strip it from the orig
    side so the textual comparison is not thrown off by its presence.
    """
    out_lines: list[str] = []
    depth = 0
    in_sci = False
    for line in text.splitlines(keepends=True):
        stripped = line.lstrip()
        if not in_sci:
            if stripped.startswith("source_code_info {"):
                in_sci = True
                depth = 1
                continue
            out_lines.append(line)
        else:
            depth += stripped.count("{") - stripped.count("}")
            if depth <= 0:
                in_sci = False
    return "".join(out_lines)


# ---------------------------------------------------------------------------
# Case 1 — no-translation: orig FDP ≡ emit.pb (modulo source_code_info)
# ---------------------------------------------------------------------------

@pytest.mark.roundtrip
@pytest.mark.parametrize("fixture_name", EMIT_BINARY_FIXTURES)
def test_emit_binary_no_translation(fixture_name: str, tmp_path: Path) -> None:
    """--emit-binary output must match the original FDP (modulo source_code_info).

    orig.pb from protoc is a FileDescriptorSet; we extract the target
    FileDescriptorProto from it, then compare both sides as annotated prototext
    after stripping source_code_info.
    """
    _, content = get_fixture_content(fixture_name)

    orig_dir = tmp_path / "orig"
    out_dir  = tmp_path / "out"
    orig_dir.mkdir()
    out_dir.mkdir()

    fixture_path = orig_dir / fixture_name
    fixture_path.write_text(content, encoding="utf-8")

    # Write companion files and compile them.
    companion_pbs: list[Path] = []
    for companion in FIXTURE_COMPANIONS.get(fixture_name, []):
        _, companion_content = get_fixture_content(companion)
        companion_path = orig_dir / companion
        companion_path.write_text(companion_content, encoding="utf-8")
        companion_pb = orig_dir / Path(companion).with_suffix(".pb").name
        r = subprocess.run(
            ["protoc", f"--descriptor_set_out={companion_pb}",
             f"-I{orig_dir}", str(companion_path)],
            capture_output=True, text=True,
        )
        assert r.returncode == 0, f"protoc failed on companion {companion}: {r.stderr}"
        companion_pbs.append(companion_pb)

    stem = fixture_path.stem
    orig_fds_pb = orig_dir / f"{stem}.pb"   # FileDescriptorSet from protoc
    orig_fdp_pb = tmp_path  / f"{stem}.fdp.pb"  # extracted FileDescriptorProto
    emit_pb     = out_dir   / f"{stem}.pb"  # bare FileDescriptorProto from reproto

    # Compile original to a FileDescriptorSet.
    r = subprocess.run(
        ["protoc", f"--descriptor_set_out={orig_fds_pb}",
         f"-I{orig_dir}", str(fixture_path)],
        capture_output=True, text=True,
    )
    assert r.returncode == 0, f"protoc failed: {r.stderr}"

    # Extract the target FDP from the FDS.
    _extract_fdp(orig_fds_pb, fixture_name, orig_fdp_pb)

    # Run reproto --emit-binary (no extra flags → no syntax translation).
    # well_known_types.proto needs WKT variant stubs so its imports resolve.
    extra_variant_args: list[str] = []
    if fixture_name == "well_known_types.proto":
        extra_variant_args = [
            "--use-variant", "any",
            "--use-variant", "empty",
            "--use-variant", "timestamp",
            "--use-variant", "duration",
        ]
    r = subprocess.run(
        _reproto_cmd(orig_dir, out_dir,
                     [*extra_variant_args,
                      str(orig_fds_pb), *[str(p) for p in companion_pbs]]),
        capture_output=True, text=True, env=_build_env(),
    )
    assert r.returncode == 0, f"reproto failed: {r.stderr}"
    assert emit_pb.exists(), f"emit.pb not created: {emit_pb}"

    # Decode both as annotated prototext; strip source_code_info from both
    # sides (reproto now synthesizes its own — spec 0141 — which has no
    # reason to textually match protoc's; only the rest of the FDP does).
    orig_text = _strip_source_code_info(_decode_fdp(orig_fdp_pb))
    emit_text = _strip_source_code_info(_decode_fdp(emit_pb))

    assert orig_text == emit_text, (
        f"emit.pb differs from orig FDP for {fixture_name}.\n"
        f"--- orig (source_code_info stripped) ---\n{orig_text}"
        f"--- emit ---\n{emit_text}"
    )


# ---------------------------------------------------------------------------
# Cases 2 & 3 — translation: golden .pb comparison
# ---------------------------------------------------------------------------

def _run_emit_binary_golden(
    tmp_path: Path,
    extra_reproto_flag: str,
    golden_name: str,
) -> None:
    """Shared logic for cases 2 and 3."""
    proto_src  = _get_fixture_path("editions_rendering.proto")
    golden_src = _get_fixture_path(golden_name)

    orig_dir = tmp_path / "orig"
    out_dir  = tmp_path / "out"
    orig_dir.mkdir()
    out_dir.mkdir()

    proto_path = orig_dir / "editions_rendering.proto"
    proto_path.write_text(proto_src.read_text(encoding="utf-8"), encoding="utf-8")

    pb_path = orig_dir / "editions_rendering.pb"
    r = subprocess.run(
        ["protoc",
         f"--descriptor_set_out={pb_path}",
         "--include_imports",
         f"-I{orig_dir}",
         str(proto_path)],
        capture_output=True, text=True,
    )
    assert r.returncode == 0, f"protoc failed: {r.stderr}"

    # --no-source-info: these golden .pb files predate spec 0141 and encode
    # translation behavior (groups, labels, packed options, feature
    # residuals), not source-info synthesis.
    cmd = _reproto_cmd(
        orig_dir, out_dir,
        [extra_reproto_flag, "--no-source-info", str(pb_path)]
    )
    r = subprocess.run(cmd, capture_output=True, text=True, env=_build_env())
    assert r.returncode == 0, f"reproto failed: {r.stderr}"

    emit_pb = out_dir / "editions_rendering.pb"
    assert emit_pb.exists(), f"emit.pb not created: {emit_pb}"

    # Decode emit.pb as annotated prototext and compare against golden.
    r = subprocess.run(
        ["prototext", "decode",
         "-t", "google.protobuf.FileDescriptorProto",
         str(emit_pb)],
        capture_output=True, text=True,
    )
    assert r.returncode == 0, f"prototext decode failed: {r.stderr}"
    actual   = r.stdout
    golden   = golden_src.read_text(encoding="utf-8")

    assert actual == golden, (
        f"emit.pb differs from golden {golden_name}.\n"
        f"--- golden ---\n{golden}\n--- actual ---\n{actual}"
    )


@pytest.mark.roundtrip
def test_emit_binary_force_proto2_output(tmp_path: Path) -> None:
    """--emit-binary --force-proto2-output: verify binary translation against golden.

    Checks that editions→proto2 translation is applied correctly:
    - DELIMITED field → TYPE_GROUP
    - LEGACY_REQUIRED field → LABEL_REQUIRED
    - EXPANDED repeated scalar → options.packed = true (proto2 explicit)
    - No residual features anywhere
    """
    _run_emit_binary_golden(
        tmp_path,
        extra_reproto_flag="--force-proto2-output",
        golden_name="editions_rendering.emit_binary.force_proto2_output.golden.pb",
    )


@pytest.mark.roundtrip
def test_emit_binary_force_proto2_for_editions(tmp_path: Path) -> None:
    """--emit-binary --force-proto2-for-editions: verify binary translation against golden.

    Lighter-weight editions→proto2 path (--force-proto2-for-editions):
    - DELIMITED field stays TYPE_MESSAGE (no group conversion)
    - LEGACY_REQUIRED field → LABEL_REQUIRED
    - No residual features anywhere
    """
    _run_emit_binary_golden(
        tmp_path,
        extra_reproto_flag="--force-proto2-for-editions",
        golden_name="editions_rendering.emit_binary.force_proto2_for_editions.golden.pb",
    )
