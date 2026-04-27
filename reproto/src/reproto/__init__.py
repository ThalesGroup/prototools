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
    # Base classes
    "Node",
    "NodeBase",

    # Core types and context
    "Context",
    "Fqdn",
    "Options",

    # Redescriptor classes
    "ReDescriptorProto",
    "ReEnumDescriptorProto",
    "ReEnumValueDescriptorProto",
    "ReFieldDescriptorProto",
    "ReFileDescriptorProto",
    "ReMethodDescriptorProto",
    "ReServiceDescriptorProto",

    # Main function
    "reproto",

    # Loading and utility functions
    "QualFile",
    "short_ref",
    "shorten_type_name",
]