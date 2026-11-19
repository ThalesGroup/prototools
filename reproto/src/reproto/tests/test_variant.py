# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Unit tests for reproto.variant: the built-in variant, the tools' data
since variant files are gone (spec 0447 S8)."""

from __future__ import annotations

import importlib.resources
from reproto import variant as V


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

_EDITION_KINDS = {
    'EnumOptions', 'EnumValueOptions', 'ExtensionRangeOptions',
    'FieldOptions', 'FileOptions', 'MessageOptions', 'MethodOptions',
    'ServiceOptions',
}


def _has_edition_orphans(result: dict) -> bool:
    """Return True if all edition orphans are present in result."""
    orphans = result['variant_orphans']
    for kind in _EDITION_KINDS:
        if 'features' not in orphans.get(kind, []):
            return False
        if 'feature_support' not in orphans.get(kind, []):
            return False
    if 'verification' not in orphans.get('ExtensionRangeOptions', []):
        return False
    return True


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

def test_load_returns_the_builtin_defaults() -> None:
    result = V.load()
    assert result['variant_descriptor_proto'] == 'google/protobuf/descriptor.proto'
    assert result['variant_well_known'] == {}
    assert result['variant_import_rules'] == []
    assert result['variant_ns_rules'] == []
    assert _has_edition_orphans(result)


def test_builtin_variant_root_and_stem() -> None:
    """The built-in variant: a Traversable root and stem='google-protobuf'."""
    result = V.load()
    assert result['variant_stem'] == 'google-protobuf'
    text = result['variant_root'].joinpath('google-protobuf.yaml').read_text(encoding='utf-8')
    assert 'google/protobuf/descriptor.proto' in text
    assert str(result['variant_root']) == str(importlib.resources.files('reproto.variants'))


def test_the_embedded_copies_are_reachable() -> None:
    """variant_root.joinpath(stem, pb_name): how the fallbacks are read."""
    result = V.load()
    data = result['variant_root'].joinpath(
        result['variant_stem'], 'google', 'protobuf', 'descriptor.pb').read_bytes()
    assert len(data) > 0
