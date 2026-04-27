# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Unit tests for reproto.variant."""

from __future__ import annotations

import textwrap


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

def test_load_none_returns_oss_defaults(monkeypatch):
    """load(None) with no REPROTO_VARIANT set returns OSS defaults."""
    monkeypatch.delenv('REPROTO_VARIANT', raising=False)
    result = V.load(None)
    assert result['variant_descriptor_proto'] == 'google/protobuf/descriptor.proto'
    assert result['variant_well_known'] == {}
    assert result['variant_import_rules'] == []
    assert result['variant_ns_rules'] == []


def test_load_builtin_matches_oss_defaults(monkeypatch, tmp_path):
    """load() with no path and no REPROTO_VARIANT loads google-protobuf.yaml."""
    monkeypatch.delenv('REPROTO_VARIANT', raising=False)
    result = V.load()
    assert result['variant_descriptor_proto'] == 'google/protobuf/descriptor.proto'
    assert result['variant_well_known'] == {}
    assert result['variant_import_rules'] == []
    assert result['variant_ns_rules'] == []
    assert _has_edition_orphans(result)


def test_edition_orphans_always_present(monkeypatch, tmp_path):
    """Edition orphans are always present even if not listed in the yaml."""
    yaml_file = tmp_path / 'minimal.yaml'
    yaml_file.write_text('name: minimal\n')
    monkeypatch.delenv('REPROTO_VARIANT', raising=False)
    result = V.load(str(yaml_file))
    assert _has_edition_orphans(result)


def test_extra_orphans_merged(monkeypatch, tmp_path):
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


def test_unknown_keys_ignored(monkeypatch, tmp_path):
    """Unknown YAML keys are silently ignored."""
    yaml_file = tmp_path / 'unknown.yaml'
    yaml_file.write_text('name: test\nfuture_key: some_value\n')
    monkeypatch.delenv('REPROTO_VARIANT', raising=False)
    result = V.load(str(yaml_file))
    assert 'future_key' not in result


def test_reproto_variant_env_var(monkeypatch, tmp_path):
    """REPROTO_VARIANT env var is picked up when no path is passed."""
    yaml_file = tmp_path / 'env.yaml'
    yaml_file.write_text(textwrap.dedent("""\
        name: env-test
        descriptor_proto: net/proto2/proto/descriptor.proto
    """))
    monkeypatch.setenv('REPROTO_VARIANT', str(yaml_file))
    result = V.load()
    assert result['variant_descriptor_proto'] == 'net/proto2/proto/descriptor.proto'


def test_explicit_path_overrides_env_var(monkeypatch, tmp_path):
    """Explicit path argument overrides REPROTO_VARIANT env var."""
    env_file = tmp_path / 'env.yaml'
    env_file.write_text('name: env\ndescriptor_proto: net/proto2/proto/descriptor.proto\n')
    explicit_file = tmp_path / 'explicit.yaml'
    explicit_file.write_text('name: explicit\ndescriptor_proto: custom/descriptor.proto\n')
    monkeypatch.setenv('REPROTO_VARIANT', str(env_file))
    result = V.load(str(explicit_file))
    assert result['variant_descriptor_proto'] == 'custom/descriptor.proto'


def test_builtin_variant_file_is_none(monkeypatch):
    """Built-in variant returns variant_file=None and variant_stem='google-protobuf'."""
    monkeypatch.delenv('REPROTO_VARIANT', raising=False)
    result = V.load(None)
    assert result['variant_file'] is None
    assert result['variant_stem'] == 'google-protobuf'


def test_external_variant_file_and_stem(monkeypatch, tmp_path):
    """External variant returns variant_file=abs_path and variant_stem from filename."""
    yaml_file = tmp_path / 'proto2.yaml'
    yaml_file.write_text('name: proto2\n')
    monkeypatch.delenv('REPROTO_VARIANT', raising=False)
    result = V.load(str(yaml_file))
    assert result['variant_file'] == str(yaml_file.resolve())
    assert result['variant_stem'] == 'proto2'


def test_import_rules_parsed(monkeypatch, tmp_path):
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
