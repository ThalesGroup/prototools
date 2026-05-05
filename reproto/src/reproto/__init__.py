# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""
Redescriptor module for Protocol Buffer descriptors.

This module provides re-descriptors for protobuf descriptor types. Re-descriptors
are decorators (wrappers) for protobuf descriptors that add specific attributes
and methods, and organize descriptors into a graph structure.

Each redescriptor instance:
- Has a "_this" attribute pointing to the associated descriptor instance
- Inherits from the NodeBase class (representing a node in the descriptor graph)
- Has a fully qualified descriptor name (fqdn) that uniquely identifies it

Redescriptors can be created by providing either:
1. A descriptor instance (for full initialization)
2. A reference string (for creating stub instances before the actual descriptor is available)

Example:
    from reproto import ReFileDescriptorProto, Context

    ctx = Context()
    file = ReFileDescriptorProto(ctx, file_descriptor_proto)
"""

from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .base import Node, NodeBase
    from .context import Context, Fqdn, Options
    from .load import QualFile
    from .re_descriptor import ReDescriptorProto
    from .re_enum import ReEnumDescriptorProto
    from .re_enum_value import ReEnumValueDescriptorProto
    from .re_field import ReFieldDescriptorProto
    from .re_file import ReFileDescriptorProto
    from .re_method import ReMethodDescriptorProto
    from .re_service import ReServiceDescriptorProto
    from .reproto import reproto
    from .utils import short_ref, shorten_type_name

__all__ = [
    "Node", "NodeBase",
    "Context", "Fqdn", "Options",
    "QualFile",
    "ReDescriptorProto",
    "ReEnumDescriptorProto",
    "ReEnumValueDescriptorProto",
    "ReFieldDescriptorProto",
    "ReFileDescriptorProto",
    "ReMethodDescriptorProto",
    "ReServiceDescriptorProto",
    "reproto",
    "short_ref", "shorten_type_name",
]

_module_map = {
    "Node":                    ".base",
    "NodeBase":                ".base",
    "Context":                 ".context",
    "Fqdn":                    ".context",
    "Options":                 ".context",
    "QualFile":                ".load",
    "ReDescriptorProto":       ".re_descriptor",
    "ReEnumDescriptorProto":   ".re_enum",
    "ReEnumValueDescriptorProto": ".re_enum_value",
    "ReFieldDescriptorProto":  ".re_field",
    "ReFileDescriptorProto":   ".re_file",
    "ReMethodDescriptorProto": ".re_method",
    "ReServiceDescriptorProto": ".re_service",
    "reproto":                 ".reproto",
    "short_ref":               ".utils",
    "shorten_type_name":       ".utils",
}


def __getattr__(name: str) -> object:
    if name in _module_map:
        import importlib
        mod = importlib.import_module(_module_map[name], package=__name__)
        return getattr(mod, name)
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")