# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Whether the Python protobuf imported is as new as reproto's own
descriptor.proto (spec 0467 S2).

reproto is built with one protobuf release (spec 0460), and its built-in
descriptor.proto is that release's. A `PYTHONPATH` can still put an older
Python protobuf first: its `Edition` enum then lacks editions reproto's
descriptor.proto has, and a file of those editions reads as having none.
The enums are compared, not version numbers: they say what matters, and
need nothing recorded at build time.
"""

import importlib.resources

from google.protobuf import descriptor_pb2


def _builtin_editions() -> dict[str, int]:
    """The `Edition` values of reproto's built-in descriptor.proto."""
    data = (importlib.resources.files('reproto.variants')
            / 'google-protobuf' / 'google' / 'protobuf' / 'descriptor.pb').read_bytes()
    fds = descriptor_pb2.FileDescriptorSet()
    fds.ParseFromString(data)
    return {v.name: v.number
            for f in fds.file for e in f.enum_type if e.name == 'Edition'
            for v in e.value}


def missing_editions() -> list[str]:
    """The built-in `Edition` values the imported protobuf lacks."""
    imported = set(descriptor_pb2.Edition.values())
    return sorted(name for name, number in _builtin_editions().items()
                  if number not in imported)


def describe() -> str:
    """The Python protobuf imported, and where from."""
    import google.protobuf as pb
    return f"Python protobuf {pb.__version__} ({pb.__path__[0]})"


def check() -> None:
    """W37, once, when the imported protobuf is older than reproto's
    descriptor.proto."""
    missing = missing_editions()
    if missing:
        from .lib.warnings import warn
        warn('W37', f"{describe()} is older than reproto's descriptor.proto: "
                    f"{', '.join(missing)} can't be read; check PYTHONPATH")
