# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

from __future__ import annotations

from typing import NewType

Fqdn = NewType("Fqdn", str)
"""Fully Qualified Descriptor Name.

Format: '<prefix>:<type_name>'
Examples: 'file:path/to/file.proto', 'desc:.google.protobuf.Timestamp'
"""

Prefix = NewType("Prefix", str)
"""Type name prefix used for scoping.

Format: '.<package>.<name>'
Examples: '.google.protobuf', '.test.MyMessage'
"""

Ref = NewType("Ref", str)
"""Type reference that may be shortened based on scope.

Examples: '.google.protobuf.Timestamp', 'Timestamp', 'MyMessage'
"""

def parse_fqdn(fqdn: Fqdn) -> tuple[str, Ref]:
    """Parse FQDN into prefix and reference components.

    Args:
        fqdn: Fully qualified descriptor name

    Returns:
        Tuple of (prefix, ref) where prefix is the descriptor type
        (e.g., 'file', 'desc', 'enum') and ref is the type name

    Example:
        >>> parse_fqdn(Fqdn('desc:.google.protobuf.Timestamp'))
        ('desc', '.google.protobuf.Timestamp')
    """
    prefix, _, ref = str(fqdn).partition(":")
    return prefix, Ref(ref)