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

import filecmp
import importlib
import importlib.resources
import os
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest


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


@pytest.fixture
def temp_dirs():
    """Create temporary directories for test artifacts."""
    with tempfile.TemporaryDirectory(
        prefix="reproto_orig_"
    ) as orig_dir, \
         tempfile.TemporaryDirectory(
             prefix="reproto_new_"
         ) as new_dir:
        yield Path(orig_dir), Path(new_dir)


@pytest.mark.parametrize("fixture_name", DEFAULT_FIXTURES)
def test_roundtrip(fixture_name: str, temp_dirs):
    """
    Test that reproto can perfectly roundtrip a proto file.

    Args:
        fixture_name: Name of the fixture file
        temp_dirs: Tuple of (original_dir, regenerated_dir)
    """
    orig_dir, new_dir = temp_dirs

    # Load fixture content
    name, content = get_fixture_content(fixture_name)

    # Write fixture to temporary directory
    fixture_path = orig_dir / name
    fixture_path.write_text(content, encoding='utf-8')

    # Paths for descriptor sets
    stem = fixture_path.stem
    orig_pb = orig_dir / f"{stem}.pb"
    new_pb = new_dir / f"{stem}.pb"
    new_proto = new_dir / name

    # Step 1: Compile original proto to descriptor set
    result = subprocess.run(
        [
            "protoc",
            f"--descriptor_set_out={orig_pb}",
            f"-I{orig_dir}",
            str(fixture_path),
        ],
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, \
        f"protoc failed on original: {result.stderr}"
    assert orig_pb.exists(), \
        f"Descriptor set not created: {orig_pb}"

    # Step 2: Use reproto to regenerate proto file from descriptor
    # Build PYTHONPATH: append src/ to existing PYTHONPATH
    src_path = str(Path(__file__).parent.parent.parent)
    pythonpath_parts = [src_path]
    if existing_pythonpath := os.environ.get("PYTHONPATH"):
        pythonpath_parts.append(existing_pythonpath)

    # Build reproto command — OSS google.protobuf variant (default).
    # -d is needed because protoc does not include descriptor.proto in the
    # descriptor set unless --include_imports is used.
    reproto_cmd = [
        sys.executable,
        "-m", "reproto.cli",
        "-d",
        f"-I{orig_dir}",
        f"--proto-out={new_dir}",
        str(orig_pb),
    ]

    # Add well-known type fallbacks for fixtures that import them
    if fixture_name == "well_known_types.proto":
        reproto_cmd.extend([
            "--use-variant", "any",
            "--use-variant", "empty",
            "--use-variant", "timestamp",
            "--use-variant", "duration",
        ])

    env = {**os.environ, "PYTHONPATH": os.pathsep.join(pythonpath_parts)}
    env.pop("REPROTO_VARIANT", None)  # ensure built-in OSS default is used

    result = subprocess.run(
        reproto_cmd,
        capture_output=True,
        text=True,
        env=env,
    )
    assert result.returncode == 0, \
        f"reproto failed: {result.stderr}"
    assert new_proto.exists(), \
        f"Regenerated proto not created: {new_proto}"

    # Step 3: Compile regenerated proto to descriptor set
    result = subprocess.run(
        [
            "protoc",
            f"-I{new_dir}",
            f"--descriptor_set_out={new_pb}",
            str(new_proto),
        ],
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, \
        f"protoc failed on regenerated: {result.stderr}"
    assert new_pb.exists(), \
        f"Regenerated descriptor set not created: {new_pb}"

    # Step 4: Compare descriptor sets (must be identical)
    if not filecmp.cmp(orig_pb, new_pb, shallow=False):
        # Descriptor sets differ - decode them to text format for debugging
        orig_txt = orig_dir / f"{stem}_orig.txt"
        new_txt = new_dir / f"{stem}_new.txt"
        diff_txt = new_dir / f"{stem}_diff.txt"

        # Decode original descriptor set
        result = subprocess.run(
            [
                "protoc",
                "--decode=google.protobuf.FileDescriptorSet",
                "google/protobuf/descriptor.proto",
            ],
            stdin=open(orig_pb, 'rb'),
            capture_output=True,
            text=True,
        )
        if result.returncode == 0:
            orig_txt.write_text(result.stdout)

        # Decode regenerated descriptor set
        result = subprocess.run(
            [
                "protoc",
                "--decode=google.protobuf.FileDescriptorSet",
                "google/protobuf/descriptor.proto",
            ],
            stdin=open(new_pb, 'rb'),
            capture_output=True,
            text=True,
        )
        if result.returncode == 0:
            new_txt.write_text(result.stdout)

        # Generate diff
        diff_output = ""
        if orig_txt.exists() and new_txt.exists():
            result = subprocess.run(
                ["diff", "-u", str(orig_txt), str(new_txt)],
                capture_output=True,
                text=True,
            )
            diff_output = result.stdout
            diff_txt.write_text(diff_output)

        # Fail with helpful message
        assert False, \
            f"Descriptor sets differ:\n" \
            f"  Original: {orig_pb}\n" \
            f"  Regenerated: {new_pb}\n" \
            f"  Decoded original: {orig_txt}\n" \
            f"  Decoded regenerated: {new_txt}\n" \
            f"  Diff: {diff_txt}\n" \
            f"\nDiff output (first 3000 chars):\n{diff_output[:3000]}"


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
