# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""
Roundtrip tests for reproto.

These tests verify that reproto can perfectly reconstruct proto files by:
1. Compiling original .proto to descriptor set
2. Using reproto to regenerate .proto from descriptor set
3. Recompiling regenerated .proto to descriptor set
4. Comparing both descriptor sets (must be identical)
"""

import importlib
import importlib.resources
import os
import subprocess
import sys
from pathlib import Path

import pytest

from reproto.tests.proto_normalize import normalize_proto_batch, pb_diff, pb_diff_fields



# Default fixtures to test (can be overridden via pytest CLI)
DEFAULT_FIXTURES = [
    "enum.proto",
    "enum_value.proto",
    "field_comprehensive.proto",
    "file_comprehensive.proto",
    "message_comprehensive.proto",
    "name_resolution.proto",
    "service.proto",
    "well_known_types.proto",
]


def get_fixture_content(fixture_name: str) -> tuple[str, str]:
    """
    Load a .proto fixture file using importlib.resources.

    Args:
        fixture_name: Name of the fixture file (e.g., "enum.proto")

    Returns:
        Tuple of (fixture_name, fixture_content_as_string)

    Raises:
        FileNotFoundError: If fixture doesn't exist
    """
    module_path = 'reproto.tests.fixtures'
    try:
        # Get a Traversable object for the package
        package = importlib.import_module(module_path)
        files = importlib.resources.files(package)

        # Check if the file exists
        proto_file = files.joinpath(fixture_name)
        if proto_file.is_file():
            content = proto_file.read_text(encoding='utf-8')
            return (fixture_name, content)
        else:
            raise FileNotFoundError(
                f"{fixture_name} not found in {module_path}"
            )
    except ModuleNotFoundError:
        raise ModuleNotFoundError(f"Package '{module_path}' not found.")


def get_all_fixtures() -> list[str]:
    """
    Discover all .proto fixture files using importlib.resources.

    Returns:
        List of fixture names (e.g., ["enum.proto", "message.proto"])
    """
    module_path = 'reproto.tests.fixtures'
    try:
        package = importlib.import_module(module_path)
        files = importlib.resources.files(package)

        # Iterate through the package contents
        fixtures = []
        for item in files.iterdir():
            if item.is_file() and item.name.endswith('.proto'):
                fixtures.append(item.name)

        return sorted(fixtures)
    except ModuleNotFoundError:
        return []


POLYGLOT_FIXTURES = [
    "packed_proto2.proto",
    "packed_proto3.proto",
]


@pytest.fixture
def temp_dirs(tmp_path):
    """Create temporary directories for test artifacts."""
    orig_dir = tmp_path / "orig"
    new_dir = tmp_path / "new"
    orig_dir.mkdir()
    new_dir.mkdir()
    yield orig_dir, new_dir


def _run_roundtrip(
    fixture_name: str,
    content: str,
    orig_dir: Path,
    new_dir: Path,
    extra_reproto_args: list[str] | None = None,
    compare_pb: bool = True,
    compare_proto: bool = True,
) -> None:
    """Shared roundtrip logic used by both test_roundtrip and test_roundtrip_polyglot."""
    fixture_path = orig_dir / fixture_name
    fixture_path.write_text(content, encoding='utf-8')

    stem = fixture_path.stem
    orig_pb = orig_dir / f"{stem}.pb"
    new_pb = new_dir / f"{stem}.pb"
    new_proto = new_dir / fixture_name

    # Step 1: Compile original proto to descriptor set
    result = subprocess.run(
        ["protoc", f"--descriptor_set_out={orig_pb}", f"-I{orig_dir}", str(fixture_path)],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, f"protoc failed on original: {result.stderr}"
    assert orig_pb.exists(), f"Descriptor set not created: {orig_pb}"

    # Step 2: Use reproto to regenerate proto file from descriptor
    src_path = str(Path(__file__).parent.parent.parent)
    pythonpath_parts = [src_path]
    if existing_pythonpath := os.environ.get("PYTHONPATH"):
        pythonpath_parts.append(existing_pythonpath)

    reproto_cmd = [
        sys.executable, "-m", "reproto.cli",
        "-d",
        f"-I{orig_dir}",
        f"--proto-out={new_dir}",
        str(orig_pb),
    ]
    if extra_reproto_args:
        reproto_cmd.extend(extra_reproto_args)
    if fixture_name == "well_known_types.proto":
        reproto_cmd.extend([
            "--use-variant", "any",
            "--use-variant", "empty",
            "--use-variant", "timestamp",
            "--use-variant", "duration",
        ])

    env = {**os.environ, "PYTHONPATH": os.pathsep.join(pythonpath_parts)}
    env.pop("REPROTO_VARIANT", None)

    result = subprocess.run(reproto_cmd, capture_output=True, text=True, env=env)
    assert result.returncode == 0, f"reproto failed: {result.stderr}"
    assert new_proto.exists(), f"Regenerated proto not created: {new_proto}"

    # Step 3: Compile regenerated proto to descriptor set
    result = subprocess.run(
        ["protoc", f"-I{new_dir}", f"--descriptor_set_out={new_pb}", str(new_proto)],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, f"protoc failed on regenerated: {result.stderr}"
    assert new_pb.exists(), f"Regenerated descriptor set not created: {new_pb}"

    # Step 4: Compare descriptor sets
    if compare_pb:
        pb1 = orig_pb.read_bytes()
        pb2 = new_pb.read_bytes()
        assert pb1 == pb2, (
            f"Descriptor sets differ for {fixture_name}:\n" + pb_diff(pb1, pb2)
        )

    # Step 5: Compare .proto text (comments and whitespace stripped)
    if compare_proto:
        normalized = normalize_proto_batch({
            "fixture": content,
            "output":  new_proto.read_text(encoding="utf-8"),
        })
        assert normalized["fixture"] == normalized["output"], (
            f".proto text differs after normalization for {fixture_name}"
        )


@pytest.mark.parametrize("fixture_name", DEFAULT_FIXTURES)
def test_roundtrip(fixture_name: str, temp_dirs):
    """Test that reproto can perfectly roundtrip a proto file."""
    orig_dir, new_dir = temp_dirs
    _, content = get_fixture_content(fixture_name)
    _run_roundtrip(fixture_name, content, orig_dir, new_dir)


@pytest.mark.parametrize("fixture_name", POLYGLOT_FIXTURES)
def test_roundtrip_polyglot(fixture_name: str, tmp_path):
    """Roundtrip test run twice: with and without --polyglot.

    Without --polyglot: compare .pb only (proto2 output, .proto text differs).
    With --polyglot: full comparison including .proto text.
    """
    _, content = get_fixture_content(fixture_name)

    # Run without --polyglot: .proto text differs (output is always proto2),
    # and .pb may differ — but only by the syntax field.
    no_polyglot_dir = tmp_path / "no_polyglot"
    orig_dir = no_polyglot_dir / "orig"
    new_dir = no_polyglot_dir / "new"
    orig_dir.mkdir(parents=True)
    new_dir.mkdir(parents=True)
    _run_roundtrip(fixture_name, content, orig_dir, new_dir, compare_pb=False, compare_proto=False)
    orig_pb = orig_dir / f"{fixture_name.removesuffix('.proto')}.pb"
    new_pb = new_dir / f"{fixture_name.removesuffix('.proto')}.pb"
    differing = pb_diff_fields(orig_pb.read_bytes(), new_pb.read_bytes())
    assert differing <= {"syntax"}, (
        f"Without --polyglot, .pb differs beyond the syntax field: {differing}\n"
        + pb_diff(orig_pb.read_bytes(), new_pb.read_bytes())
    )

    # Run with --polyglot: full comparison.
    polyglot_dir = tmp_path / "polyglot"
    orig_dir = polyglot_dir / "orig"
    new_dir = polyglot_dir / "new"
    orig_dir.mkdir(parents=True)
    new_dir.mkdir(parents=True)
    _run_roundtrip(fixture_name, content, orig_dir, new_dir, extra_reproto_args=["--polyglot"])


def test_fixture_discovery():
    """Verify that we can discover fixture files."""
    fixtures = get_all_fixtures()
    assert len(fixtures) > 0, \
        "No fixture files found in reproto.tests.fixtures"
    assert all(f.endswith(".proto") for f in fixtures), \
        "All fixtures must be .proto files"


if __name__ == "__main__":
    # Allow running this test file directly
    pytest.main([__file__, "-v"])
