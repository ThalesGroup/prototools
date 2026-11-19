# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Profiles (spec 0447 S6): a saved set of command-line options.

`--profile NAME`, repeatable. NAME is looked up as a path (`.yaml`
optional), then in each folder of `$PROTOBACK_PROFILE_PATH`, then among the
built-in profiles (this package's `profiles/`). A key is an option's long
name; a later profile overrides an earlier one, the command line overrides
them all, and lists accumulate. A profile `X.yaml` may have a folder `X/`
beside it, laid out as a descriptor root: every file in it replaces the
input's copy.

protoback reads the same files the same way (`protoback/src/profile.rs`).
"""

from __future__ import annotations

import importlib.resources
import os
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import yaml

# Each key, and the type of its value.
_SCALARS = {'descriptor': str, 'target': str, 'editions': str,
            'import_descriptor': bool, 'emit_descriptor': bool}
_LISTS = ('builtin', 'fail_on')
# The values a key takes, where the option limits them.
_CHOICES = {
    'target': ('keep', 'protoc', 'portable'),
    'editions': ('keep', 'proto2'),
    'builtin': ('any', 'empty', 'timestamp', 'duration', 'struct', 'wrappers',
                'descriptor', 'source_context', 'field_mask', 'type', 'api', 'all'),
}
ENV = 'PROTOBACK_PROFILE_PATH'


@dataclass
class Profiles:
    """The profiles named, merged in order."""
    scalars: dict[str, Any] = field(default_factory=dict)
    lists: dict[str, list[str]] = field(default_factory=lambda: {k: [] for k in _LISTS})
    # The profiles' folders, in order: each file in one replaces the input's.
    folders: list[Path] = field(default_factory=list)


def _locate(name: str) -> tuple[str, Path | None]:
    """The profile's text, and the path of its file (None for a built-in
    one). ValueError when found nowhere."""
    stems = [name] if name.endswith('.yaml') else [name, f'{name}.yaml']
    for stem in stems:
        path = Path(stem)
        if path.is_file():
            return path.read_text(encoding='utf-8'), path.resolve()
    folders = [Path(d) for d in os.environ.get(ENV, '').split(':') if d]
    for folder in folders:
        for stem in stems:
            path = folder / stem
            if path.is_file():
                return path.read_text(encoding='utf-8'), path.resolve()
    builtin = importlib.resources.files('reproto.profiles')
    for stem in stems:
        resource = builtin.joinpath(stem)
        if resource.is_file():
            return resource.read_text(encoding='utf-8'), None
    where = ['the current directory', *(f'{f}' for f in folders), 'the built-in profiles']
    raise ValueError(f"profile '{name}' not found (looked in: {', '.join(where)})")


def _check_choice(name: str, key: str, value: Any) -> None:
    choices = _CHOICES.get(key)
    if choices is not None and value not in choices:
        raise ValueError(f"profile '{name}': '{key}': '{value}' is not one of "
                         f"{', '.join(choices)}")


def load(names: tuple[str, ...] | list[str]) -> Profiles:
    """The profiles `names`, merged in order. ValueError on a profile not
    found, a key no option has, or a value of the wrong type."""
    out = Profiles()
    for name in names:
        text, path = _locate(name)
        raw = yaml.safe_load(text)
        if raw is None:
            raw = {}
        if not isinstance(raw, dict):
            raise ValueError(f"profile '{name}': must be a mapping of options")
        unknown = sorted(str(k) for k in raw if k not in _SCALARS and k not in _LISTS)
        if unknown:
            raise ValueError(f"profile '{name}': unknown key(s): {', '.join(unknown)}")
        for key, kind in _SCALARS.items():
            if key in raw:
                value = raw[key]
                if not isinstance(value, kind):
                    raise ValueError(f"profile '{name}': '{key}' must be a {kind.__name__}")
                _check_choice(name, key, value)
                out.scalars[key] = value
        for key in _LISTS:
            if key in raw:
                value = raw[key]
                values = [value] if isinstance(value, str) else value
                if not isinstance(values, list) or not all(isinstance(v, str) for v in values):
                    raise ValueError(f"profile '{name}': '{key}' must be a list of strings")
                for v in values:
                    _check_choice(name, key, v)
                out.lists[key].extend(values)
        if path is not None:
            folder = path.with_suffix('')
            if folder.is_dir():
                out.folders.append(folder)
    return out
