# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""variant.py — load and parse a reproto variant specification file.

Resolution order (first match wins):
  1. explicit path passed by the caller (from --proto_variant CLI flag)
  2. REPROTO_VARIANT environment variable
  3. built-in OSS default: reproto/variants/google-protobuf.yaml

The returned dict contains all variant_* keys expected by Options.
Edition orphans (features, feature_support, verification) are always
present in variant_orphans regardless of what the file says.
Unknown YAML keys are silently ignored.
"""

from __future__ import annotations

import importlib.resources
import os
from pathlib import Path

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


def _parse(raw: dict, root: object, stem: str) -> dict:
    """Convert a raw YAML dict into a variant_* dict.

    Args:
        raw:  parsed YAML content.
        root: Traversable pointing to the directory containing <stem>.yaml.
        stem: variant stem (filename without extension).
    """
    orphans_raw: dict[str, list[str]] = {}
    for kind, names in (raw.get('orphans') or {}).items():
        orphans_raw[kind] = list(names)

    annotation_modules_raw = raw.get('annotation_modules') or []
    if not isinstance(annotation_modules_raw, list) or not all(
        isinstance(m, str) for m in annotation_modules_raw
    ):
        raise ValueError(
            f"'annotation_modules' must be a list of strings, got: {annotation_modules_raw!r}"
        )

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
        'variant_annotation_modules': list(annotation_modules_raw),
    }


def load(path: str | None = None) -> dict:
    """Load a variant file and return a variant_* dict.

    Args:
        path: explicit path (from --proto_variant), or None.

    Resolution order:
        1. path argument (if not None)
        2. REPROTO_VARIANT environment variable
        3. built-in google-protobuf.yaml (via importlib.resources)
    """
    resolved: str | None = path or os.environ.get('REPROTO_VARIANT')

    if resolved is None:
        root = importlib.resources.files('reproto.variants')
        stem = 'google-protobuf'
    else:
        abs_resolved = str(Path(resolved).resolve())
        root = Path(abs_resolved).parent
        stem = Path(abs_resolved).stem

    text = root.joinpath(f'{stem}.yaml').read_text(encoding='utf-8')
    raw = yaml.safe_load(text)
    return _parse(raw if isinstance(raw, dict) else {}, root, stem)
