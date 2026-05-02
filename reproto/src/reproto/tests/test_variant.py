# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Unit tests for reproto.variant."""

from __future__ import annotations

import importlib.resources
import sys
import textwrap
from pathlib import Path

import pytest

from reproto import variant as V
from reproto.reproto import import_annotations


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

def test_load_none_returns_oss_defaults(monkeypatch: pytest.MonkeyPatch) -> None:
    """load(None) with no REPROTO_VARIANT set returns OSS defaults."""
    monkeypatch.delenv('REPROTO_VARIANT', raising=False)
    result = V.load(None)
    assert result['variant_descriptor_proto'] == 'google/protobuf/descriptor.proto'
    assert result['variant_well_known'] == {}
    assert result['variant_import_rules'] == []
    assert result['variant_ns_rules'] == []


def test_load_builtin_matches_oss_defaults(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    """load() with no path and no REPROTO_VARIANT loads google-protobuf.yaml."""
    monkeypatch.delenv('REPROTO_VARIANT', raising=False)
    result = V.load()
    assert result['variant_descriptor_proto'] == 'google/protobuf/descriptor.proto'
    assert result['variant_well_known'] == {}
    assert result['variant_import_rules'] == []
    assert result['variant_ns_rules'] == []
    assert _has_edition_orphans(result)


def test_edition_orphans_always_present(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    """Edition orphans are always present even if not listed in the yaml."""
    yaml_file = tmp_path / 'minimal.yaml'
    yaml_file.write_text('name: minimal\n')
    monkeypatch.delenv('REPROTO_VARIANT', raising=False)
    result = V.load(str(yaml_file))
    assert _has_edition_orphans(result)


def test_extra_orphans_merged(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    """Extra orphans from the yaml are merged with edition orphans."""
    yaml_file = tmp_path / 'extra.yaml'
    yaml_file.write_text(textwrap.dedent("""\
        name: test
        orphans:
          FileOptions:
            - cc_api_version
            - features
    """))
    monkeypatch.delenv('REPROTO_VARIANT', raising=False)
    result = V.load(str(yaml_file))
    file_orphans = result['variant_orphans']['FileOptions']
    assert 'cc_api_version' in file_orphans
    assert 'features' in file_orphans
    assert 'feature_support' in file_orphans  # merged from edition orphans


def test_unknown_keys_ignored(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    """Unknown YAML keys are silently ignored."""
    yaml_file = tmp_path / 'unknown.yaml'
    yaml_file.write_text('name: test\nfuture_key: some_value\n')
    monkeypatch.delenv('REPROTO_VARIANT', raising=False)
    result = V.load(str(yaml_file))
    assert 'future_key' not in result


def test_reproto_variant_env_var(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    """REPROTO_VARIANT env var is picked up when no path is passed."""
    yaml_file = tmp_path / 'env.yaml'
    yaml_file.write_text(textwrap.dedent("""\
        name: env-test
        descriptor_proto: net/proto2/proto/descriptor.proto
    """))
    monkeypatch.setenv('REPROTO_VARIANT', str(yaml_file))
    result = V.load()
    assert result['variant_descriptor_proto'] == 'net/proto2/proto/descriptor.proto'


def test_explicit_path_overrides_env_var(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    """Explicit path argument overrides REPROTO_VARIANT env var."""
    env_file = tmp_path / 'env.yaml'
    env_file.write_text('name: env\ndescriptor_proto: net/proto2/proto/descriptor.proto\n')
    explicit_file = tmp_path / 'explicit.yaml'
    explicit_file.write_text('name: explicit\ndescriptor_proto: custom/descriptor.proto\n')
    monkeypatch.setenv('REPROTO_VARIANT', str(env_file))
    result = V.load(str(explicit_file))
    assert result['variant_descriptor_proto'] == 'custom/descriptor.proto'


def test_builtin_variant_root_and_stem(monkeypatch: pytest.MonkeyPatch) -> None:
    """Built-in variant returns a Traversable root and stem='google-protobuf'."""
    monkeypatch.delenv('REPROTO_VARIANT', raising=False)
    result = V.load(None)
    assert result['variant_root'] is not None
    assert result['variant_stem'] == 'google-protobuf'
    # The root must be readable as a Traversable — the YAML must be accessible.
    text = result['variant_root'].joinpath('google-protobuf.yaml').read_text(encoding='utf-8')
    assert 'google/protobuf/descriptor.proto' in text
    # The built-in root should equal importlib.resources.files('reproto.variants').
    assert str(result['variant_root']) == str(importlib.resources.files('reproto.variants'))


def test_external_variant_root_and_stem(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    """External variant returns a Path root and stem from filename."""
    yaml_file = tmp_path / 'proto2.yaml'
    yaml_file.write_text('name: proto2\n')
    monkeypatch.delenv('REPROTO_VARIANT', raising=False)
    result = V.load(str(yaml_file))
    assert result['variant_stem'] == 'proto2'
    # Root is the parent directory; joining the stem YAML must give the original file.
    assert result['variant_root'].joinpath('proto2.yaml').read_text() == 'name: proto2\n'


def test_import_annotations_prepends_resource_root(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """import_annotations() prepends resource_root to sys.path when modules are listed."""
    root = str(tmp_path)
    # Ensure root is not already on sys.path.
    monkeypatch.setattr(sys, 'path', [p for p in sys.path if p != root])
    # Use a module name that won't be found — we only care about sys.path mutation.
    import_annotations(['_nonexistent_module_xyz'], resource_root=root)
    assert root in sys.path
    assert sys.path[0] == root


def test_import_annotations_no_duplicate(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """import_annotations() does not add resource_root twice."""
    root = str(tmp_path)
    monkeypatch.setattr(sys, 'path', [root] + [p for p in sys.path if p != root])
    import_annotations(['_nonexistent_module_xyz'], resource_root=root)
    assert sys.path.count(root) == 1


def test_import_annotations_empty_modules_no_side_effects(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """import_annotations() with empty modules list does not touch sys.path."""
    root = str(tmp_path)
    monkeypatch.setattr(sys, 'path', [p for p in sys.path if p != root])
    import_annotations([], resource_root=root)
    assert root not in sys.path


def test_load_embedded_proto_fallback_uses_variant_root(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    """variant_root.joinpath(stem, pb_name) is used for .pb loading in both cases."""
    # Verify that for the built-in variant the root traversal reaches descriptor.pb.
    monkeypatch.delenv('REPROTO_VARIANT', raising=False)
    result = V.load(None)
    root = result['variant_root']
    stem = result['variant_stem']
    # descriptor.pb must be reachable via the Traversable.
    data = root.joinpath(stem, 'google', 'protobuf', 'descriptor.pb').read_bytes()
    assert len(data) > 0


def test_import_rules_parsed(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    """import_rewrites rules are parsed correctly."""
    yaml_file = tmp_path / 'rules.yaml'
    yaml_file.write_text(textwrap.dedent("""\
        name: test
        import_rewrites:
          - match: proto2/bridge/
            action: keep
          - match: proto2/
            action: rewrite
            to: google/protobuf/
    """))
    monkeypatch.delenv('REPROTO_VARIANT', raising=False)
    result = V.load(str(yaml_file))
    rules = result['variant_import_rules']
    assert len(rules) == 2
    assert rules[0] == {'match': 'proto2/bridge/', 'action': 'keep'}
    assert rules[1] == {'match': 'proto2/', 'action': 'rewrite', 'to': 'google/protobuf/'}
