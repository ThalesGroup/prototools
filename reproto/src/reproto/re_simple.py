# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""
Compatibility wrapper for resimple.py refactoring.

This module re-exports all classes from the split modules to maintain
backward compatibility with existing imports.

The original resimple.py was split into:
- simple_types.py: Simple proto structure renderers
- option_renderers.py: Option rendering classes
"""

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
