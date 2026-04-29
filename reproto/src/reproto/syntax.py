# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Syntax helpers for proto descriptor rendering (spec 0016)."""

from __future__ import annotations


def fdp_syntax(fdp) -> str:
    """Return the syntax of a FileDescriptorProto as a non-empty string.

    fdp.syntax is "" for proto2 files (protoc omits the field); normalise
    that to "proto2".  All other values ("proto3", "editions", …) are
    returned as-is.
    """
    return fdp.syntax or "proto2"


def packed_option(
    source_syntax: str,
    target_syntax: str,
    has_field: bool,
    effective_packed: bool,
) -> str | None:
    """Return the string to emit for the packed option, or None to emit nothing.

    Args:
        source_syntax:    syntax of the input file ("proto2" or "proto3")
        target_syntax:    syntax reproto will emit ("proto2" or "proto3")
        has_field:        True if packed was explicitly set in the source .proto
        effective_packed: fo_msg.packed — the wire-level effective value
                          (includes proto3 defaults)

    Rules:
      - has_field=True  → emit the explicit value regardless of syntax
      - has_field=False, source==target → emit nothing (default preserved)
      - has_field=False, source=proto3, target=proto2 → emit "true" if
        effective_packed is True (preserve wire semantics across downconversion)
    """
    if has_field:
        return "true" if effective_packed else "false"
    if source_syntax == target_syntax:
        return None
    # Cross-syntax conversion: source=proto3 (packed by default), target=proto2
    # (unpacked by default) — must emit explicit annotation to preserve semantics.
    if effective_packed:
        return "true"
    return None
