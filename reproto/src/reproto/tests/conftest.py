# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Pytest configuration for reproto tests."""

import os
import shlex
import shutil
import subprocess
import sys
from pathlib import Path

import pytest


FIXTURES_DIR = Path(__file__).parent / "fixtures"

# The command the CLI tests run (spec 0424 S2): this tree's reproto by
# default; `REPROTO="protoback"` or any other command line runs the same
# tests against it.
REPROTO_CMD: list[str] = (
    shlex.split(os.environ["REPROTO"]) if os.environ.get("REPROTO")
    else [sys.executable, "-m", "reproto.cli"]
)


@pytest.fixture(autouse=True)
def no_ambient_descriptor_set(monkeypatch: pytest.MonkeyPatch) -> None:
    """Hide the toolset's default descriptor set from every test.

    The dev-shell exports PROTOTEXT_DESCRIPTOR_SET (spec 0228 S6) and
    nix-build does not, so a test that read it would behave differently in
    the two environments.  No test relies on the fallback today; this is
    here so that none acquires the dependency by accident.
    """
    for var in ("PROTOTEXT_DESCRIPTOR_SET", "PROTOTEXT_DEFAULT_DESCRIPTOR"):
        monkeypatch.delenv(var, raising=False)


def pytest_configure(config: pytest.Config) -> None:
    """Configure pytest with custom markers."""
    config.addinivalue_line(
        "markers", "roundtrip: marks tests as roundtrip tests (deselect with '-m \"not roundtrip\"')"
    )


@pytest.fixture(scope="session", autouse=True)
def check_dependencies():
    """Verify that required external dependencies are available."""
    # Check for protoc
    if not shutil.which("protoc"):
        pytest.skip(
            "protoc not found - install protocol buffers compiler",
            allow_module_level=True
        )

    # Verify protoc version
    result = subprocess.run(
        ["protoc", "--version"], capture_output=True, text=True
    )
    if result.returncode != 0:
        pytest.skip("protoc version check failed", allow_module_level=True)

    if not shutil.which("buf"):
        pytest.skip("buf not found", allow_module_level=True)


def compile_proto(
    out_dir: Path,
    *proto_names: str,
    include_dirs: list[Path] | None = None,
) -> list[Path]:
    """Compile fixture .proto files to individual .pb descriptor sets.

    Each name in proto_names is compiled separately (one .pb per file),
    with FIXTURES_DIR always on the include path.  Additional directories
    may be added via include_dirs.

    Returns the list of resulting .pb paths in the same order as proto_names.
    """
    dirs = [FIXTURES_DIR] + (include_dirs or [])
    include_flags = [f"-I{d}" for d in dirs]
    pb_paths: list[Path] = []
    for name in proto_names:
        proto_path = FIXTURES_DIR / name
        pb_path = out_dir / (Path(name).stem + ".pb")
        result = subprocess.run(
            ["protoc", *include_flags, f"--descriptor_set_out={pb_path}", str(proto_path)],
            capture_output=True, text=True,
        )
        assert result.returncode == 0, (
            f"protoc failed compiling {name}:\n{result.stderr}"
        )
        pb_paths.append(pb_path)
    return pb_paths


def compile_proto_multi(
    out_path: Path,
    *proto_names: str,
    include_dirs: list[Path] | None = None,
) -> Path:
    """Compile multiple .proto files into a single multi-FDP FDS .pb.

    Uses protoc --include_imports so all transitive dependencies are bundled
    into one FileDescriptorSet binary at out_path.

    Returns out_path.
    """
    dirs = [FIXTURES_DIR] + (include_dirs or [])
    include_flags = [f"-I{d}" for d in dirs]
    proto_paths = [str(FIXTURES_DIR / name) for name in proto_names]
    result = subprocess.run(
        [
            "protoc", *include_flags,
            "--include_imports",
            f"--descriptor_set_out={out_path}",
            *proto_paths,
        ],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, (
        f"protoc failed compiling {proto_names}:\n{result.stderr}"
    )
    return out_path


# protoc's own descriptor.proto, beside the protoc on PATH.
STOCK_INCLUDE = Path(shutil.which("protoc") or "protoc").resolve().parent.parent / "include"


def vendor_descriptor(tmp_path: Path, extra: str = "") -> None:
    """protoc's descriptor.proto as a vendor's (spec 0447): package `vendor`,
    at vendor/proto/descriptor.proto, compiled alone into in/vdesc.pb;
    `extra`, fields added to its FieldOptions."""
    src = tmp_path / "src" / "vendor" / "proto"
    src.mkdir(parents=True)
    text = (STOCK_INCLUDE / "google/protobuf/descriptor.proto").read_text(encoding="utf-8")
    text = text.replace("package google.protobuf;", "package vendor;")
    text = text.replace("message FieldOptions {\n", "message FieldOptions {\n" + extra, 1)
    (src / "descriptor.proto").write_text(text, encoding="utf-8")
    (tmp_path / "in").mkdir(exist_ok=True)
    subprocess.run(["protoc", f"-I{tmp_path / 'src'}",
                    f"--descriptor_set_out={tmp_path / 'in' / 'vdesc.pb'}",
                    "vendor/proto/descriptor.proto"], check=True, capture_output=True)
