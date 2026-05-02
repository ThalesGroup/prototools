# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Unit tests for the editions feature resolution engine (spec 0025).

Tests T1-T6 cover build_edition_defaults and resolve_features directly.
The golden regression test (test_editions_resolution_golden) runs reproto
end-to-end and compares YAML output to the checked-in golden file.
"""

from __future__ import annotations

import importlib
import importlib.resources
import os
import subprocess
import sys
from pathlib import Path

import yaml
from google.protobuf.descriptor_pb2 import Edition, FeatureSet

from reproto.feature_resolution import (
    FIELD_PRESENCE_EXPLICIT,
    FIELD_PRESENCE_IMPLICIT,
    FIELD_PRESENCE_UNKNOWN,
    REPEATED_FIELD_ENCODING_EXPANDED,
    REPEATED_FIELD_ENCODING_PACKED,
    build_edition_defaults,
    resolve_features,
)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _load_builtin_descriptor_fdp():
    """Return the built-in google.protobuf descriptor.proto as a FileDescriptorProto."""
    from google.protobuf.descriptor_pb2 import FileDescriptorProto
    from google.protobuf.descriptor_pool import Default

    pool = Default()
    desc = pool.FindFileByName("google/protobuf/descriptor.proto")
    fdp = FileDescriptorProto()
    desc.CopyToProto(fdp)
    return fdp


def _make_synthetic_fdp(feature_defs: dict[str, list[tuple[Edition.ValueType, str]]]):
    """Build a minimal synthetic FileDescriptorProto containing a FeatureSet message.

    feature_defs maps field_name -> list of (Edition value, value_name) pairs.
    Each field is backed by a trivial enum defined inside FeatureSet.
    """
    from google.protobuf.descriptor_pb2 import (
        DescriptorProto,
        EnumDescriptorProto,
        EnumValueDescriptorProto,
        FieldDescriptorProto,
        FileDescriptorProto,
    )

    fdp = FileDescriptorProto()
    fdp.name = "synthetic_descriptor.proto"
    fdp.syntax = "proto2"

    fset = DescriptorProto()
    fset.name = "FeatureSet"

    for field_num, (field_name, defaults) in enumerate(feature_defs.items(), start=1):
        # Create a nested enum for this feature field.
        enum = EnumDescriptorProto()
        enum_name = field_name.upper()
        enum.name = enum_name
        # Value 0 = UNKNOWN, 1 = FIRST, 2 = SECOND
        v0 = EnumValueDescriptorProto()
        v0.name = f"{enum_name}_UNKNOWN"
        v0.number = 0
        v1 = EnumValueDescriptorProto()
        v1.name = "FIRST"
        v1.number = 1
        v2 = EnumValueDescriptorProto()
        v2.name = "SECOND"
        v2.number = 2
        enum.value.extend([v0, v1, v2])
        fset.enum_type.append(enum)

        # Create the field.
        f = FieldDescriptorProto()
        f.name = field_name
        f.number = field_num
        f.label = FieldDescriptorProto.LABEL_OPTIONAL
        f.type = FieldDescriptorProto.TYPE_ENUM
        f.type_name = f".FeatureSet.{enum_name}"
        for edition_val, value_name in defaults:
            ed_def = f.options.edition_defaults.add()
            ed_def.edition = edition_val
            ed_def.value = value_name
        fset.field.append(f)

    fdp.message_type.append(fset)
    return fdp


# Edition numbers used in tests (matches google.protobuf.Edition enum).
EDITION_LEGACY  = 900
EDITION_PROTO3  = 999
EDITION_2023    = 1000


# ---------------------------------------------------------------------------
# T1 — real descriptor: build_edition_defaults on built-in descriptor.pb
# ---------------------------------------------------------------------------

def test_T1_real_descriptor_builds_table():
    """build_edition_defaults on the built-in descriptor.pb returns non-empty table
    with known entries for field_presence and repeated_field_encoding."""
    fdp = _load_builtin_descriptor_fdp()
    table = build_edition_defaults(fdp)

    assert table, "edition_defaults table must not be empty"
    assert "field_presence" in table, "table must contain field_presence"
    assert "repeated_field_encoding" in table, "table must contain repeated_field_encoding"

    # Verify field_presence has at least entries for EDITION_LEGACY and EDITION_2023.
    fp_entries = table["field_presence"]
    fp_editions = [e for e, _ in fp_entries]
    assert EDITION_LEGACY in fp_editions, "field_presence must have EDITION_LEGACY default"
    assert EDITION_2023 in fp_editions, "field_presence must have EDITION_2023 default"


# ---------------------------------------------------------------------------
# T2 — default lookup across editions
# ---------------------------------------------------------------------------

def test_T2_default_lookup():
    """resolve_features with no override chain returns edition defaults."""
    fdp = _load_builtin_descriptor_fdp()
    table = build_edition_defaults(fdp)

    # At EDITION_LEGACY: field_presence = EXPLICIT (1)
    r_legacy = resolve_features(table, EDITION_LEGACY)
    assert r_legacy.field_presence == FIELD_PRESENCE_EXPLICIT

    # At EDITION_PROTO3: field_presence = IMPLICIT (2)
    r_proto3 = resolve_features(table, EDITION_PROTO3)
    assert r_proto3.field_presence == FIELD_PRESENCE_IMPLICIT

    # At EDITION_2023: field_presence = EXPLICIT (1)
    r_2023 = resolve_features(table, EDITION_2023)
    assert r_2023.field_presence == FIELD_PRESENCE_EXPLICIT

    # At EDITION_LEGACY: repeated_field_encoding = EXPANDED (2)
    assert r_legacy.repeated_field_encoding == REPEATED_FIELD_ENCODING_EXPANDED

    # At EDITION_2023: repeated_field_encoding = PACKED (1)
    assert r_2023.repeated_field_encoding == REPEATED_FIELD_ENCODING_PACKED

    # Below all defaults (edition 0): result should be 0 (unknown)
    r_zero = resolve_features(table, 0)
    assert r_zero.field_presence == FIELD_PRESENCE_UNKNOWN


# ---------------------------------------------------------------------------
# T3 — explicit override wins over edition default
# ---------------------------------------------------------------------------

def test_T3_explicit_override_wins():
    """A FeatureSet with field_presence = IMPLICIT overrides the edition default."""
    fdp = _load_builtin_descriptor_fdp()
    table = build_edition_defaults(fdp)

    # At EDITION_2023 the default is EXPLICIT; override to IMPLICIT.
    fs = FeatureSet(field_presence=FeatureSet.IMPLICIT)

    r = resolve_features(table, EDITION_2023, fs)
    assert r.field_presence == FIELD_PRESENCE_IMPLICIT


# ---------------------------------------------------------------------------
# T4 — finer override wins over coarser
# ---------------------------------------------------------------------------

def test_T4_finer_wins_over_coarser():
    """A field-level FeatureSet overrides a file-level FeatureSet."""
    fdp = _load_builtin_descriptor_fdp()
    table = build_edition_defaults(fdp)

    file_fs = FeatureSet(field_presence=FeatureSet.IMPLICIT)
    field_fs = FeatureSet(field_presence=FeatureSet.EXPLICIT)

    # file_fs then field_fs — field wins.
    r = resolve_features(table, EDITION_2023, file_fs, field_fs)
    assert r.field_presence == FIELD_PRESENCE_EXPLICIT

    # Only file_fs — should be IMPLICIT.
    r2 = resolve_features(table, EDITION_2023, file_fs)
    assert r2.field_presence == FIELD_PRESENCE_IMPLICIT


# ---------------------------------------------------------------------------
# T5 — None levels skipped
# ---------------------------------------------------------------------------

def test_T5_none_levels_skipped():
    """resolve_features with all-None levels returns plain edition defaults."""
    fdp = _load_builtin_descriptor_fdp()
    table = build_edition_defaults(fdp)

    r_none = resolve_features(table, EDITION_2023, None, None, None)
    r_bare = resolve_features(table, EDITION_2023)
    assert r_none == r_bare


# ---------------------------------------------------------------------------
# T6 — empty variant (no FeatureSet message)
# ---------------------------------------------------------------------------

def test_T6_empty_variant_returns_empty_table():
    """build_edition_defaults on a descriptor without FeatureSet returns {}."""
    from google.protobuf.descriptor_pb2 import FileDescriptorProto
    empty_fdp = FileDescriptorProto()
    empty_fdp.name = "no_featureset.proto"
    # No message_type entries — so no FeatureSet.
    table = build_edition_defaults(empty_fdp)
    assert table == {}


# ---------------------------------------------------------------------------
# T7 — synthetic descriptor round-trip
# ---------------------------------------------------------------------------

def test_T7_synthetic_descriptor():
    """build_edition_defaults and resolve_features with a synthetic descriptor.

    Uses 'field_presence' (a real _RESOLVED_FIELDS name) so the table is
    populated.  The synthetic enum assigns FIRST=1, SECOND=2; the defaults
    are: edition 900 → FIRST, edition 1000 → SECOND.
    """
    fdp = _make_synthetic_fdp({
        "field_presence": [(Edition.EDITION_LEGACY, "FIRST"), (Edition.EDITION_2023, "SECOND")],
    })
    table = build_edition_defaults(fdp)

    assert "field_presence" in table

    entries = table["field_presence"]
    assert entries == [(int(Edition.EDITION_LEGACY), "FIRST"), (int(Edition.EDITION_2023), "SECOND")]

    enum_entry = table["_enum_field_presence"]
    assert isinstance(enum_entry, dict)
    assert enum_entry.get("FIRST") == 1
    assert enum_entry.get("SECOND") == 2

    # At edition 900 the default is FIRST (1).
    r900 = resolve_features(table, 900)
    assert r900.field_presence == 1

    # At edition 1000 the default is SECOND (2).
    r1000 = resolve_features(table, 1000)
    assert r1000.field_presence == 2

    # Below all defaults (edition 0): result is 0 (unknown).
    r0 = resolve_features(table, 0)
    assert r0.field_presence == 0


# ---------------------------------------------------------------------------
# Golden regression test
# ---------------------------------------------------------------------------

def _get_fixture_path(name: str) -> Path:
    pkg = importlib.import_module('reproto.tests.fixtures')
    files = importlib.resources.files(pkg)
    return Path(str(files.joinpath(name)))


def test_editions_resolution_golden(tmp_path: Path) -> None:
    """Compile editions_resolution.proto, run --dump-resolved-features,
    compare YAML output against the checked-in golden file."""
    proto_src = _get_fixture_path("editions_resolution.proto")
    golden_src = _get_fixture_path("editions_resolution.yaml")

    orig_dir = tmp_path / "orig"
    orig_dir.mkdir()

    # Copy fixture into tmp dir.
    proto_content = proto_src.read_text(encoding="utf-8")
    proto_path = orig_dir / "editions_resolution.proto"
    proto_path.write_text(proto_content, encoding="utf-8")

    # Compile to .pb (include_imports so descriptor.proto is bundled).
    pb_path = orig_dir / "editions_resolution.pb"
    result = subprocess.run(
        ["protoc",
         f"--descriptor_set_out={pb_path}",
         "--include_imports",
         f"-I{orig_dir}",
         str(proto_path)],
        capture_output=True, text=True,
    )
    assert result.returncode == 0, f"protoc failed: {result.stderr}"
    assert pb_path.exists()

    # Run reproto --dump-resolved-features.
    src_path = str(Path(__file__).parent.parent.parent)
    pythonpath_parts = [src_path]
    if existing := os.environ.get("PYTHONPATH"):
        pythonpath_parts.append(existing)

    reproto_cmd = [
        sys.executable, "-m", "reproto.cli",
        "--use-variant", "descriptor",
        f"-I{orig_dir}",
        "--dump-resolved-features", "editions_resolution.proto",
        str(pb_path),
    ]
    env = {**os.environ, "PYTHONPATH": os.pathsep.join(pythonpath_parts)}
    env.pop("REPROTO_VARIANT", None)

    result = subprocess.run(reproto_cmd, capture_output=True, text=True, env=env)
    assert result.returncode == 0, f"reproto failed: {result.stderr}"

    # Parse both as YAML and compare structurally (tolerates whitespace diffs).
    actual = yaml.safe_load(result.stdout)
    golden = yaml.safe_load(golden_src.read_text(encoding="utf-8"))
    assert actual == golden, (
        f"YAML output differs from golden.\n"
        f"--- golden ---\n{golden}\n--- actual ---\n{actual}"
    )
