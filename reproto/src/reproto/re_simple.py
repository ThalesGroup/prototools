# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Facade re-exporting all public classes from simple_types and option_renderers."""

from __future__ import annotations

# Re-export everything from option_renderers
from .option_renderers import (
    ReExtensions,
    ReOptions,
)

# Re-export everything from simple_types
from .simple_types import (
    ReExtensionRange,
    ReFieldDescriptor,
    ReMessage,
    ReReservedRange,
)

__all__ = [
    # From simple_types
    "ReExtensionRange",
    "ReFieldDescriptor",
    "ReMessage",
    "ReReservedRange",
    # From option_renderers
    "ReExtensions",
    "ReOptions",
]
