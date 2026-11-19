# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""variant.py — the built-in google.protobuf variant: the descriptor's
path, the well-known files' fallbacks, and the orphans.

Variant files are gone (spec 0447 S8): --proto-variant is a usage error,
$REPROTO_VARIANT ignored; --target protoc derives its renaming from the
descriptor in use. The built-in file stays the tools' data.

The returned dict contains all variant_* keys expected by Options.
Edition orphans (features, feature_support, verification) are always
present in variant_orphans regardless of what the file says.
Unknown YAML keys are silently ignored.
"""

from __future__ import annotations

import importlib.resources

import yaml

# Edition-related options that are always orphans, regardless of variant.
_EDITION_ORPHANS: dict[str, list[str]] = {
    'EnumOptions':           ['features', 'feature_support'],
    'EnumValueOptions':      ['features', 'feature_support'],
    'ExtensionRangeOptions': ['features', 'feature_support', 'verification'],
    'FieldOptions':          ['features', 'feature_support'],
    'FileOptions':           ['features', 'feature_support'],
    'MessageOptions':        ['features', 'feature_support'],
    'MethodOptions':         ['features', 'feature_support'],
    'OneofOptions':          ['features', 'feature_support'],
    'ServiceOptions':        ['features', 'feature_support'],
}


def _merge_orphans(
    base: dict[str, list[str]],
    extra: dict[str, list[str]],
) -> dict[str, list[str]]:
    """Return a new dict merging extra into base, preserving order, no duplicates."""
    result: dict[str, list[str]] = {}
    keys = list(base) + [k for k in extra if k not in base]
    for k in keys:
        seen: set[str] = set()
        merged: list[str] = []
        for v in list(base.get(k, [])) + list(extra.get(k, [])):
            if v not in seen:
                seen.add(v)
                merged.append(v)
        result[k] = merged
    return result


# The keys a variant file may have, every one read.
_KEYS = ('name', 'description', 'descriptor_proto', 'well_known',
         'import_rewrites', 'namespace_rewrites', 'orphans')


def _parse(raw: dict, root: object, stem: str) -> dict:
    """Convert a raw YAML dict into a variant_* dict.

    Args:
        raw:  parsed YAML content.
        root: Traversable pointing to the directory containing <stem>.yaml.
        stem: variant stem (filename without extension).
    """
    # Spec 0438 S5: a key no tool reads (annotation_modules, retired by spec
    # 0427) is said, so that a stale variant does not look meaningful.
    from .lib.warnings import warn
    for key in raw:
        if key not in _KEYS:
            warn("W17", f"variant '{raw.get('name', stem)}': key '{key}' is not read; "
                        "it has no effect")
    orphans_raw: dict[str, list[str]] = {}
    for kind, names in (raw.get('orphans') or {}).items():
        orphans_raw[kind] = list(names)

    return {
        'variant_descriptor_proto': raw.get(
            'descriptor_proto', 'google/protobuf/descriptor.proto'
        ),
        'variant_well_known': dict(raw.get('well_known') or {}),
        'variant_import_rules': list(raw.get('import_rewrites') or []),
        'variant_ns_rules':     list(raw.get('namespace_rewrites') or []),
        'variant_orphans':      _merge_orphans(_EDITION_ORPHANS, orphans_raw),
        'variant_root':         root,
        'variant_stem':         stem,
    }


def load() -> dict:
    """The built-in variant, as a variant_* dict."""
    root = importlib.resources.files('reproto.variants')
    stem = 'google-protobuf'
    text = root.joinpath(f'{stem}.yaml').read_text(encoding='utf-8')
    raw = yaml.safe_load(text)
    return _parse(raw if isinstance(raw, dict) else {}, root, stem)
