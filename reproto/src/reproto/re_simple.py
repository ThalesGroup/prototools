# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Facade re-exporting all public classes from simple_types and option_renderers."""

from __future__ import annotations

# Re-export everything from option_renderers
from .option_renderers import (
    ReEnumOptions,
    ReExtensions,
    ReFieldOptions,
    ReFileOptions,
    ReMessageOptions,
    ReMethodOptions,
    ReOptions,
    ReServiceOptions,
)

# Re-export everything from simple_types
from .simple_types import (
    ReExtensionRange,
    ReExtensionRangeOptions,
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
    "ReEnumOptions",
    "ReExtensionRangeOptions",
    "ReExtensions",
    "ReFieldOptions",
    "ReFileOptions",
    "ReMessageOptions",
    "ReMethodOptions",
    "ReOptions",
    "ReServiceOptions",
]
