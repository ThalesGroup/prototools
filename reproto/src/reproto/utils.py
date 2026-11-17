# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""
Utility functions for redescriptors.

This module provides name shortening functions that match protoc's behavior,
including quirks and bugs in name resolution.

Key Protoc Quirks (see tests/fixtures/name_resolution.proto for examples):

1. **Enum Value FQDNs**: Enum value FQDNs don't include the enum name itself.
   Example: StatusEnum.OK has FQDN `.test.OK` not `.test.StatusEnum.OK`
   Impact: Two enums in same scope cannot have values with same names.

2. **Nested Extensions vs Nested Types**: Nested type references can be
   unqualified, but nested extension references often cannot
   (GitHub issue protocolbuffers/protobuf#9550).
   Example: `optional Inner msg = 1;` works but `option (ext) = ...;` may not.

3. **Extension Option Resolution**: When resolving custom option names, protoc
   skips the current message scope and goes directly to parent scope.

4. **Extensions in Option Message Values**: When resolving extension names inside
   the message value syntax for options (e.g., `option (my_opt) = { field: "val" }`),
   protoc skips ALL message scopes and uses only file scope.

5. **Collision Detection**: Protoc uses an incomplete path matching heuristic.
   If the first component of a shortened name matches an identifier in a closer
   scope, protoc will attempt to resolve from there, even if the full path doesn't
   exist. This forces us to avoid shortenings that would trigger this behavior.

For comprehensive documentation and test cases, see:
- tests/fixtures/name_resolution.proto
- GitHub protocolbuffers/protobuf issues:
  - #9550: inconsistent scope rules for types vs custom options
  - #6296: inconsistent name resolution behavior
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any


from .context import Context
from .fake_types import Ref, parse_fqdn
from .mappings import apply_variant_namespace

if TYPE_CHECKING:
    from .base import NodeBase
    from .names import Mode
    from .re_descriptor import ReDescriptorProto
    from .re_file import ReFileDescriptorProto
    from .re_service import ReServiceDescriptorProto


def get_file_node(node: 'NodeBase') -> 'ReFileDescriptorProto':
    """Walk parent pointers until a ReFileDescriptorProto is found and return it."""
    from .re_file import ReFileDescriptorProto
    current = node
    while not isinstance(current, ReFileDescriptorProto):
        assert current._parent is not None, f"No file ancestor found for {node.fqdn}"
        current = current._parent
    return current


def scope_name(node: 'ReDescriptorProto | ReServiceDescriptorProto | ReFileDescriptorProto') -> str:
    """The full name (no leading dot) of the scope `node` opens: a file's
    package, a message's or service's own name."""
    from .re_file import ReFileDescriptorProto
    if isinstance(node, ReFileDescriptorProto):
        return node.package
    return str(parse_fqdn(node.fqdn)[1]).lstrip('.')


def _parent(node: 'NodeBase[Any]') -> 'NodeBase[Any]':
    """`node`'s parent: an element always has one."""
    parent = node.parent
    assert parent is not None, f"{node.fqdn} has no parent"
    return parent


def option_scope(node: 'NodeBase[Any]') -> str:
    """The first scope protoc searches for an option name on `node`'s
    options (spec 0439 S1): `name_scope` is the element's full name, of
    which `LookupSymbol` drops the last component first: its parent's
    scope. (An enum value, named as its enum's sibling, takes its enum's.)"""
    from .re_descriptor import ReDescriptorProto
    from .re_file import ReFileDescriptorProto
    from .re_service import ReServiceDescriptorProto
    parent = _parent(node)
    assert isinstance(parent, (ReDescriptorProto, ReServiceDescriptorProto, ReFileDescriptorProto))
    return scope_name(parent)


def type_ref(ctx: Context, target: str, scope: str, mode: 'Mode') -> str:
    """The name to write for the type `target` (its full name, leading dot
    included) in the file being rendered, from `scope` (spec 0434): the
    variant's namespace rewriting first, as before (its N2), then the name
    protoc resolves back to `target`, in googleapis' convention (S4)."""
    from .names import symbol_table
    if not ctx.keep_variant_descriptor:
        rewritten = str(apply_variant_namespace(ctx, Ref(target)))
        if rewritten != target:
            return rewritten
    return symbol_table(ctx).type_name(target, ctx.current_file, scope, mode)
    