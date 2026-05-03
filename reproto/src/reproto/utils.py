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

from typing import TYPE_CHECKING

from google.protobuf.descriptor_pb2 import FieldDescriptorProto

from .context import Context, Fqdn
from .fake_types import Ref, parse_fqdn
from .globals import type_names
from .mappings import apply_variant_namespace

if TYPE_CHECKING:
    from .base import NodeBase
    from .re_descriptor import ReDescriptorProto
    from .re_enum import ReEnumDescriptorProto
    from .re_field import ReFieldDescriptorProto
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


def short_ref(
    ctx: Context,
    type_descriptor: 'int | Fqdn | ReDescriptorProto | ReEnumDescriptorProto',
    scope: 'ReDescriptorProto | ReFieldDescriptorProto | ReFileDescriptorProto',
) -> Ref:
    """
    Generate a shortened reference for a type descriptor following protoc's C++ scoping rules.

    This function implements protoc's type name resolution with 100% compatibility,
    including critical collision detection quirks.

    Type Name Resolution (C++ Scoping):
    -----------------------------------
    Protoc follows C++ scoping rules for type names:
    1. Search innermost scope first, then parent scopes
    2. Leading '.' forces resolution from outermost (package) scope
    3. First matching type in search order wins

    Critical Protoc Quirk - "First Component Collision":
    ---------------------------------------------------
    The collision detection is subtle: "it suffices for the first item in the
    path to coincide, for protoc to be misled" (see line 157 below).

    This means protoc may reject a shortening even if the FULL paths differ,
    as long as the FIRST component matches something in scope.

    Parameters
    ----------
    ctx : Context
        Context containing all known type descriptors. Used to check for naming
        collisions.
    type_descriptor : int | Fqdn | ReDescriptorProto | ReEnumDescriptorProto
        The type to be referenced. Can be:
        - `int`: representing a simple `ValueType` (anything that is not a
           group, message, or enum)
        - `ReDescriptorProto` or `ReEnumDescriptorProto`: a re-descriptor for a
           message, group, or enum
        - `Fqdn`: a fully-qualified type name
    scope : ReDescriptorProto | ReFieldDescriptorProto | ReFileDescriptorProto
        The scope in which the reference appears. Determines the context for
        shortening.

    Returns
    -------
    Ref
        A shortened, unambiguous reference to the type descriptor if possible.
        If no shortening is safe, returns the fully qualified reference.

    Examples
    --------
    Example 1: Basic shortening
    ```protobuf
    package test;
    message Outer {
        message Inner {}
        optional Inner field = 1;  // Can shorten to just "Inner"
    }
    ```

    Example 2: Collision prevents shortening
    ```protobuf
    package test;
    enum TestEnum { VALUE = 1; }
    message TestMessage {
        enum TestEnum { VALUE = 2; }
        // Cannot shorten to "TestEnum" - would resolve to TestMessage.TestEnum
        optional test.TestEnum outer_enum = 1;
    }
    ```

    Example 3: "First component collision" quirk
    ```protobuf
    package test;
    message A { message B {} }
    message C {
        message B { message D {} }
        // Cannot shorten to "B" even though full paths differ:
        // - "B" would resolve to C.B (wrong!)
        // - Need "A.B" to get test.A.B (correct)
        optional A.B field = 1;
    }
    ```

    Notes
    -----
    This function is used for TYPE references (messages, enums).
    Extension references have different scoping rules - see mappings.canonize_opt_name().

    For enum VALUES (not enum types), remember the quirk: enum value FQDNs don't
    include the enum name. So StatusEnum.OK has FQDN `.test.OK` not `.test.StatusEnum.OK`.
    """
    from .re_file import ReFileDescriptorProto
    
    if isinstance(type_descriptor, int):
        return Ref(type_names[type_descriptor])
    if isinstance(type_descriptor, str):
        fqdn = type_descriptor
    else:
        fqdn = type_descriptor.fqdn
    d_prefix, d_ref = parse_fqdn(fqdn)

    if not ctx.keep_variant_descriptor:
        r2 = apply_variant_namespace(ctx, d_ref)
        if r2 != d_ref:
            return r2

    r_parts = d_ref.split('.')
    r_len = len(r_parts)
    if isinstance(scope, ReFileDescriptorProto):
        s_ref = f'.{scope.package}' if scope.package else ''
    else:
        _, s_ref = parse_fqdn(scope.fqdn)
    l_parts = s_ref.split('.')
    l_len = len(l_parts)

    found = ''
    for r_ndx in range(1, r_len):
        right = '.'.join(r_parts[-r_ndx:])
        collision_found = False

        for l_ndx in range(l_len, 0, -1):
            left = '.'.join(l_parts[:l_ndx])
            path = '.'.join([left, right])

            if path == d_ref:
                # Found our target - this shortening works from this scope level
                found = right
                break

            # CRITICAL FIX: Check if the constructed path actually exists
            # ============================================================
            # Protoc resolves names by searching from innermost to outermost scope.
            # If a shorter path exists and resolves to something (even if wrong),
            # protoc will use it and never reach our target in an outer scope.
            #
            # Example bug scenario:
            #   message CollisionTest {
            #     message Inner {}           // FQDN: CollisionTest.Inner
            #     message Container {
            #       message Inner {}         // FQDN: CollisionTest.Container.Inner
            #       // Trying to reference CollisionTest.Inner:
            #       optional ??? outer_inner = 1;
            #     }
            #   }
            #
            # When shortening CollisionTest.Inner from within Container:
            # - Try "Inner": would resolve to Container.Inner (WRONG!)
            # - Try "CollisionTest.Inner": would resolve correctly
            #
            # So we must check: does "Container" + "Inner" exist?
            # If yes, it's a collision - can't shorten to just "Inner".
            potential_fqdn = Fqdn(f'{d_prefix}:{path}')
            if ctx.find_node(potential_fqdn) is not None:
                # This path exists and is not our target - collision!
                # Protoc would resolve the shortened name to this instead of our target
                collision_found = True
                break

            # HEURISTIC: "First Component Collision"
            # ========================================
            # This heuristic detects collisions even when the colliding node
            # may not be in the context at all.
            #
            # Protoc's name resolution uses an incomplete path matching strategy:
            # if the first component of a shortened name matches an identifier in
            # a closer scope, protoc will attempt to resolve from there. It only
            # discovers later (potentially at link time) that the full resolved
            # path doesn't actually exist, resulting in an error.
            #
            # This appears to be a legacy bug in how protoc queries for symbols
            # during compilation. We must avoid shortenings that would trigger this.
            #
            # If the first component of our shortened name matches the next
            # element in the scope path, assume a collision exists.
            if l_ndx < l_len and r_parts[-r_ndx] == l_parts[l_ndx]:
                collision_found = True
                break

        if found and not collision_found:
            # Found target and no collisions - use this shortening
            break

        # Reset for next iteration if collision found
        found = ''

    return Ref(found) if found else d_ref


def shorten_type_name(
    ctx: Context,
    this: 'ReFieldDescriptorProto | ReDescriptorProto | ReServiceDescriptorProto',
    name: str = '',
    with_kind: bool = False,
) -> str:
    """
    Shorten a type name for display.
    
    Parameters
    ----------
    ctx : Context
        The context containing descriptor information
    this : ReFieldDescriptorProto | ReDescriptorProto | ReServiceDescriptorProto
        The descriptor from which we're referencing
    name : str
        Optional explicit type name to shorten
    with_kind : bool
        Whether to include the kind prefix (e.g., "message ")
    
    Returns
    -------
    str
        The shortened type name
    """
    from .re_field import ReFieldDescriptorProto
    from .re_file import ReFileDescriptorProto
    
    # Deal first with simple types
    if (
        not name
        and isinstance(this, ReFieldDescriptorProto)
        and this.type != FieldDescriptorProto.TYPE_GROUP
        and this.type != FieldDescriptorProto.TYPE_MESSAGE
        and this.type != FieldDescriptorProto.TYPE_ENUM
    ):
        return type_names[this.type]

    # Now for the interesting case:
    if name:
        type_name = name
    elif isinstance(this, ReFieldDescriptorProto):
        type_name = this.type_name
    else:
        # For ReDescriptorProto or ReServiceDescriptorProto, 
        # name must be provided as parameter
        raise ValueError(f"name parameter required for {type(this).__name__}")

    if isinstance(this, ReFieldDescriptorProto) and with_kind:
        prefix = f'{type_names[this.type]} '
    else:
        prefix = ''

    if not ctx.keep_variant_descriptor:
        name2 = str(apply_variant_namespace(ctx, Ref(type_name)))
        if name2 != type_name:
            return Ref(name2)

    # Special treatment for leading '.google.protobuf.': return it unmodified
    if type_name.startswith('.google.protobuf.'):
        return type_name[1:]

    # Recursively shorten as much as possible from the type_name
    def shorten(
        name: str,
        proto: 'ReDescriptorProto | ReServiceDescriptorProto | ReFileDescriptorProto',
    ) -> tuple[bool, str]:
        from .re_descriptor import ReDescriptorProto
        from .re_service import ReServiceDescriptorProto
        
        match proto:
            case ReFileDescriptorProto():
                # Remove the package prefix, if matching
                package_name = proto.package
                if not package_name:
                    return True, name
                elif name.startswith(f'.{package_name}.'):
                    return True, name[len(package_name) + 1:]
                else:
                    return False, name
            case ReDescriptorProto() | ReServiceDescriptorProto():
                do_more, name = shorten(name, proto.parent)
                # Remove the container prefix, if matching
                container_name = proto.name
                if do_more and name.startswith(f'.{container_name}.'):
                    return True, name[len(container_name) + 1:]
                else:
                    # Return do_more=False: shortening must be contiguous from
                    # the package root, so a non-matching level stops all
                    # further stripping by callers.
                    return False, name

    from .re_descriptor import ReDescriptorProto
    
    _, short_name = shorten(
        type_name,
        this if isinstance(this, ReDescriptorProto) else this.parent)
    if short_name == type_name:
        return f'{prefix}{short_name}'
    else:
        return f'{prefix}{short_name[1:]}'
    