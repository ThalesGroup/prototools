# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""
Option name canonization and mapping utilities.

This module handles:
1. Canonization of builtin options (e.g., deprecated, packed)
2. Custom extension option name shortening
3. Namespace mapping between proto2.* and google.protobuf.*

Key Protoc Quirks for Extension Options:
---------------------------------------
Extension options have DIFFERENT scoping rules than type references:

1. **Option Resolution Skips Current Message Scope**:
   When resolving a custom option name for a message option, protoc skips the
   current message scope and goes directly to the parent scope.

2. **Nested Extensions Require Qualification**:
   Even though nested types can be referenced without qualification, nested
   extensions often cannot (GitHub issue protocolbuffers/protobuf#9550).

   Example:
   ```protobuf
   message Outer {
       message Inner {}              // Type
       extend MessageOptions {
           optional string ext = 50000;  // Extension
       }
       message Test {
           optional Inner msg = 1;           // Works: type can be unqualified
           option (Outer.ext) = "qualified"; // Required: extension must be qualified
       }
   }
   ```

3. **Extensions in Option Message Values**:
   When resolving extension names inside the message value syntax for options
   (e.g., `option (my_opt) = { field: "val" }`), protoc skips ALL message scopes
   and uses only file scope.

Implementation Notes:
--------------------
- Builtin options are shortened using canonize_opt_name()
- Custom extensions keep full FQDN (protoc-compatible, matches nested extension behavior)
- Both google.protobuf.* and proto2.* namespaces are handled
- See tests/fixtures/name_resolution.proto for comprehensive examples
"""

from __future__ import annotations

import functools

import re2 as re

from .context import Context
from .fake_types import Fqdn, Ref
from .globals import FIELD

# --- The renaming (spec 0440) ---------------------------------------------
# The variant's rules are applied here and nowhere else: each output name
# is one of these functions of the input name. Symbols and packages keep
# their input names under --keep-descriptor-path; files are rewritten but
# the variant's descriptor.proto (canonize_dependency).

def output_symbol(ctx: Context, name: str) -> str:
    """A symbol's output full name, from its input one, leading dot kept
    as given."""
    if ctx.keep_variant_descriptor:
        return name
    dotted = name if name.startswith('.') else f'.{name}'
    out = str(apply_variant_namespace(ctx, Ref(dotted)))
    return out if name.startswith('.') else out[1:]


def output_package(ctx: Context, package: str) -> str:
    """A package's output name; also a scope's (a package or a message's
    full name, no leading dot), matched as `.scope.`."""
    if ctx.keep_variant_descriptor:
        return package
    return apply_variant_namespace_to_package(ctx, package)


def apply_variant_namespace(ctx: Context, r: Ref) -> Ref:
    """The variant's namespace rules alone, whatever --keep-descriptor-path
    says: callers use output_symbol, but for a predicate on what the rules
    would give (syntax.descriptor_options_fqns).

    Map a type reference according to the active variant's namespace rules.

    Iterates ctx.variant_ns_rules in order; the first matching rule wins
    (unless rule has continue: true).  Rules with action 'keep' leave the
    reference unchanged and stop; rules with action 'rewrite' substitute
    the prefix.
    """
    s = str(r)
    for rule in ctx.variant_ns_rules:
        if s.startswith(rule['match']):
            if rule['action'] == 'rewrite':
                s = rule['to'] + s[len(rule['match']):]
            # action == 'keep': leave s unchanged
            if not rule.get('continue', False):
                return Ref(s)
    return Ref(s)


def apply_variant_namespace_to_package(ctx: Context, package: str) -> str:
    """Rewrite a file's own `package` field through the same
    variant_ns_rules that apply_variant_namespace applies to type
    references, treating `package` as an implicit '.' + package + '.'
    for matching purposes (spec 0159).

    Callers are responsible for checking ctx.keep_variant_descriptor
    first — this function itself applies the rules unconditionally,
    matching apply_variant_namespace's own unconditional-application
    contract.
    """
    if not package:
        return package
    rewritten = str(apply_variant_namespace(ctx, Ref(f'.{package}.')))
    return rewritten.strip('.')


@functools.cache
def builtin_option_pattern(package: str) -> re._Regexp:
    """A built-in option's full name: a field of one of the options types of
    `package`, the variant descriptor.proto's (ctx.options_package).

    match.group(3) is the intangible part of the option, e.g.: FileOptions.java_package
    match.group(5) it the option short name, e.g.: java_package
    """
    return re.compile(
        rf'^(\.?)({re.escape(package)})\.'
        r'((Enum|EnumValue|ExtensionRange|Field|File|Message|Method|Oneof|Service)Options\.(.*))$')


FileOptions_extensions: dict[int, str] = dict()

def canonize_opt_name(ctx: Context, n: str, custom: bool = False) -> tuple[str, bool]:
    """
    Canonize an option name to its shortest safe form.

    For builtin options (fields of the options types of ctx.options_package,
    the variant descriptor.proto's package):
        Converts to canonical short form:
        - .google.protobuf.FieldOptions.deprecated -> deprecated
        - .google.protobuf.MessageOptions.map_entry -> map_entry

    For custom extensions (custom=True):
        Returns full FQDN unchanged (conservative, protoc-compatible approach).
        Protoc itself often requires qualification for nested extensions, so
        using full FQDNs matches protoc's behavior in most cases.

    Parameters
    ----------
    ctx : Context
        Build context for extension lookup and scope resolution
    n : str
        Full option name (e.g., '.google.protobuf.FieldOptions.packed')
    custom : bool
        True if this is a custom extension option, False for builtin options

    Returns
    -------
    tuple[str, bool]
        - str: Canonical name (shortened when safe, full FQDN otherwise)
        - bool: is_orphan flag (True if option is deprecated/orphaned)

    Examples
    --------
    Builtin option:
    >>> canonize_opt_name(ctx, '.google.protobuf.FieldOptions.deprecated', False)
    ('deprecated', False)

    Custom extension:
    >>> canonize_opt_name(ctx, '.my.package.my_extension', True)
    ('.my.package.my_extension', False)  # Returns full FQDN

    Orphaned option:
    >>> canonize_opt_name(ctx, '.google.protobuf.FileOptions.java_package', False)
    ('java_package', True)  # is_orphan=True for deprecated options
    """
    if custom:
        # CUSTOM EXTENSION HANDLING
        # =========================
        # For custom extensions, we use the conservative approach: always return
        # the full FQDN unchanged. This is 100% safe and protoc-compatible.
        #
        # Shortening custom extensions would require:
        # 1. Scope context (not available in this function signature)
        # 2. Different scoping rules than types (skip current message scope)
        # 3. Special handling for nested extensions (often require qualification,
        #    see GitHub issue protocolbuffers/protobuf#9550)
        # 4. Message value context detection (skip all message scopes)
        #
        # Since protoc itself often requires qualification for nested extensions,
        # using full FQDNs is the safest approach and matches protoc's behavior
        # in most cases.
        #
        # See tests/fixtures/name_resolution.proto for test cases demonstrating
        # why nested extensions require qualification.

        node = ctx.find_node(Fqdn(FIELD + ':' + n))
        if node is None:
            # Extension not found - treat as orphan
            return n, True
        if node.is_summoned:
            # Extension is defined/imported - safe to use
            return n, False
        # Extension exists but not summoned - orphan
        return n, True
    match = builtin_option_pattern(ctx.options_package).fullmatch(n)
    if not match:
        # NOTE: For unrecognized option patterns (not a field of the options
        # types), we conservatively return the full name unchanged
        # to avoid breaking custom options. These will always use their FQDN.
        return n, False
    kind = match.group(4)
    short_name = match.group(5)
    assert isinstance(kind, str) and isinstance(short_name, str)

    is_orphan = short_name in ctx.variant_orphans.get(f'{kind}Options', [])

    if short_name == 'features' or short_name == 'feature_support':
        short_name = ''

    return short_name, is_orphan



def canonize_dependency(ctx: Context, name: str) -> str:
    """Canonize an import path according to the active variant's import rules.

    Iterates ctx.variant_import_rules in order; the first matching rule wins
    (unless rule has continue: true).  Rules with action 'keep' leave the name
    unchanged and stop; rules with action 'rewrite' substitute the prefix.

    When keep_variant_descriptor is True, the descriptor_proto path is not
    rewritten (the variant-specific path is preserved in the output).

    Args:
        ctx: Build context (variant_import_rules, keep_variant_descriptor)
        name: Import path to canonize

    Returns:
        Canonized path
    """
    if ctx.keep_variant_descriptor and name == ctx.variant_descriptor_proto:
        return name
    for rule in ctx.variant_import_rules:
        if name.startswith(rule['match']):
            if rule['action'] == 'rewrite':
                name = rule['to'] + name[len(rule['match']):]
            # action == 'keep': leave name unchanged
            if not rule.get('continue', False):
                return name
    return name



#def shorten(
