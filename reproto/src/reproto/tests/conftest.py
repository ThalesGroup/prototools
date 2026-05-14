# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Pytest configuration for reproto tests."""

import shutil
import subprocess
from pathlib import Path

import pytest


FIXTURES_DIR = Path(__file__).parent / "fixtures"


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
