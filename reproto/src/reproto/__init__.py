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


_LAZY: dict[str, tuple[str, str]] = {
    "Node":                       (".base",        "Node"),
    "NodeBase":                   (".base",        "NodeBase"),
    "Context":                    (".context",     "Context"),
    "Fqdn":                       (".context",     "Fqdn"),
    "Options":                    (".context",     "Options"),
    "QualFile":                   (".load",        "QualFile"),
    "ReDescriptorProto":          (".re_descriptor",   "ReDescriptorProto"),
    "ReEnumDescriptorProto":      (".re_enum",         "ReEnumDescriptorProto"),
    "ReEnumValueDescriptorProto": (".re_enum_value",   "ReEnumValueDescriptorProto"),
    "ReFieldDescriptorProto":     (".re_field",        "ReFieldDescriptorProto"),
    "ReFileDescriptorProto":      (".re_file",         "ReFileDescriptorProto"),
    "ReMethodDescriptorProto":    (".re_method",       "ReMethodDescriptorProto"),
    "ReServiceDescriptorProto":   (".re_service",      "ReServiceDescriptorProto"),
    "reproto":                    (".reproto",         "reproto"),
    "short_ref":                  (".utils",           "short_ref"),
    "shorten_type_name":          (".utils",           "shorten_type_name"),
}


def __getattr__(name: str) -> object:
    if name in _LAZY:
        module_name, attr = _LAZY[name]
        import importlib
        mod = importlib.import_module(module_name, package=__name__)
        return getattr(mod, attr)
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")