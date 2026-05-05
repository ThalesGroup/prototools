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



# Additional fixture files that must be co-present in orig_dir for compilation.
# Maps a fixture name to the list of companion file names it imports.
FIXTURE_COMPANIONS: dict[str, list[str]] = {
    "weak_import_proto2.proto": ["weak_import_proto2_dep.proto"],
    "editions_roundtrip.proto": [
        "editions_custom_option_dep.proto",
        "weak_import_proto2_dep.proto",
    ],
}

# Default fixtures to test (can be overridden via pytest CLI)
DEFAULT_FIXTURES = [
    "custom_options_proto3.proto",
    "default_values_proto2.proto",
    "enum.proto",
    "enum_value.proto",
    "extensions_proto2.proto",
    "field_comprehensive.proto",
    "group_proto2.proto",
    "file_comprehensive.proto",
    "message_comprehensive.proto",
    "name_resolution.proto",
    "service.proto",
    "message_set_proto2.proto",
    "weak_import_proto2.proto",
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


# Polyglot fixtures where the only no-polyglot .pb difference is `syntax`.
POLYGLOT_FIXTURES_STRICT = [
    "json_name.proto",
    "packed_proto2.proto",
    "packed_proto3.proto",
]

# Polyglot fixtures that use proto3-only descriptor fields (proto3_optional,
# synthetic oneof_decl, oneof_index) which cannot be reproduced from proto2
# source.  The no-polyglot roundtrip runs for crash-safety only; the field-diff
# assertion is widened to PROTO3_ONLY_FIELDS.
POLYGLOT_FIXTURES_LOSSY = [
    "field_labels_proto3.proto",  # spec 0019
    "synthetic_oneof.proto",      # spec 0019
]

# Fields that may differ in the no-polyglot .pb comparison for lossy fixtures.
# "name" appears because pb_diff_fields traverses into missing oneof_decl
# sub-messages and surfaces their name child field.
PROTO3_ONLY_FIELDS = {"syntax", "proto3_optional", "oneof_index", "oneof_decl", "name"}


@pytest.fixture
def temp_dirs(tmp_path: Path):
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

    # Write companion files (e.g. imported dependencies) into orig_dir and
    # compile each to its own .pb so reproto can load them as context.
    companion_pbs: list[Path] = []
    for companion in FIXTURE_COMPANIONS.get(fixture_name, []):
        _, companion_content = get_fixture_content(companion)
        companion_path = orig_dir / companion
        companion_path.write_text(companion_content, encoding='utf-8')
        companion_pb = orig_dir / f"{Path(companion).stem}.pb"
        result = subprocess.run(
            ["protoc", f"--descriptor_set_out={companion_pb}", f"-I{orig_dir}", str(companion_path)],
            capture_output=True, text=True,
        )
        assert result.returncode == 0, f"protoc failed on companion {companion}: {result.stderr}"
        companion_pbs.append(companion_pb)

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
        "--use-variant", "descriptor",
        f"-I{orig_dir}",
        f"--output-root={new_dir}",
        str(orig_pb),
        *[str(p) for p in companion_pbs],
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

    # Step 3: Compile regenerated proto to descriptor set.
    # Companions must also be present in new_dir so protoc can resolve imports.
    for companion in FIXTURE_COMPANIONS.get(fixture_name, []):
        _, companion_content = get_fixture_content(companion)
        (new_dir / companion).write_text(companion_content, encoding='utf-8')
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


EDITION_FIXTURES: list[str] = [
    "editions_roundtrip.proto",
]


@pytest.mark.parametrize("fixture_name", EDITION_FIXTURES)
def test_roundtrip_edition(fixture_name: str, tmp_path: Path) -> None:
    """End-to-end roundtrip for edition .proto files.

    Same two-level check as for proto2/proto3: .pb descriptor byte-identity
    and normalized .proto text equality (uncomment + buf format).
    """
    orig_dir = tmp_path / "orig"
    new_dir = tmp_path / "new"
    orig_dir.mkdir()
    new_dir.mkdir()
    _, content = get_fixture_content(fixture_name)
    _run_roundtrip(fixture_name, content, orig_dir, new_dir)


@pytest.mark.parametrize("fixture_name", DEFAULT_FIXTURES)
def test_roundtrip(fixture_name: str, temp_dirs: tuple[Path, Path]) -> None:
    """Test that reproto can perfectly roundtrip a proto file."""
    orig_dir, new_dir = temp_dirs
    _, content = get_fixture_content(fixture_name)
    _run_roundtrip(fixture_name, content, orig_dir, new_dir)


@pytest.mark.parametrize("fixture_name", POLYGLOT_FIXTURES_STRICT + POLYGLOT_FIXTURES_LOSSY)
def test_roundtrip_polyglot(fixture_name: str, tmp_path: Path) -> None:
    """Roundtrip test run twice: with and without --force-proto2-output.

    With --force-proto2-output: .proto text is not compared (output is always proto2).
    - Strict fixtures: .pb may only differ by the syntax field.
    - Lossy fixtures: .pb may also differ by proto3-only descriptor fields
      (proto3_optional, synthetic oneof_decl/oneof_index) which cannot be
      reproduced from proto2 source.
    Without --force-proto2-output (default polyglot): full .pb and .proto comparison.
    """
    _, content = get_fixture_content(fixture_name)
    allowed = (PROTO3_ONLY_FIELDS if fixture_name in POLYGLOT_FIXTURES_LOSSY
               else {"syntax"})

    # Run with --force-proto2-output.
    no_polyglot_dir = tmp_path / "no_polyglot"
    orig_dir = no_polyglot_dir / "orig"
    new_dir = no_polyglot_dir / "new"
    orig_dir.mkdir(parents=True)
    new_dir.mkdir(parents=True)
    _run_roundtrip(fixture_name, content, orig_dir, new_dir,
                   extra_reproto_args=["--force-proto2-output"],
                   compare_pb=False, compare_proto=False)
    orig_pb = orig_dir / f"{fixture_name.removesuffix('.proto')}.pb"
    new_pb = new_dir / f"{fixture_name.removesuffix('.proto')}.pb"
    differing = pb_diff_fields(orig_pb.read_bytes(), new_pb.read_bytes())
    assert differing <= allowed, (
        f"With --force-proto2-output, .pb differs beyond allowed fields {allowed}: {differing}\n"
        + pb_diff(orig_pb.read_bytes(), new_pb.read_bytes())
    )

    # Run without --force-proto2-output (default polyglot): full comparison.
    polyglot_dir = tmp_path / "polyglot"
    orig_dir = polyglot_dir / "orig"
    new_dir = polyglot_dir / "new"
    orig_dir.mkdir(parents=True)
    new_dir.mkdir(parents=True)
    _run_roundtrip(fixture_name, content, orig_dir, new_dir)


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
