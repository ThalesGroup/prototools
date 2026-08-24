# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Phase functions for the reproto reconstruction algorithm.

Each of the 7 algorithm phases is implemented here as a private function.
The public entry point reproto() in reproto.py calls these in sequence.
"""

from __future__ import annotations

import importlib.resources
import itertools
from collections.abc import Callable
from typing import Any
import logging
import sys
from pathlib import Path

import re2 as re
from google.protobuf import message, text_format
from google.protobuf.descriptor_pb2 import (
    DescriptorProto,
    EnumDescriptorProto,
    FeatureSet,
    FieldDescriptorProto,
    FileDescriptorProto,
    FileDescriptorSet,
)
from google.protobuf.message import DecodeError
from rapidfuzz import fuzz

from .lib.warnings import cli_attention, cli_error, cli_info, cli_warning
from reproto import Context, Fqdn, Node, Options

from .fake_types import parse_fqdn
from .feature_resolution import ResolvedFeatures, build_edition_defaults
from .globals import FILE
from .load import QualFile, load_from_path
from .option_messages import create_option_message_classes
from .re_file import ReFileDescriptorProto
from .topology import File, ReFile, Topology

logger = logging.getLogger(__name__)

# EDITIONS_COMPAT_REQUIRED: set to False once prost-reflect supports editions
# syntax natively (upstream PR pending).  While True, _phase_build_schema_db
# forces editions files to proto2 output unconditionally so that the emitted
# descriptor.desc is consumable by prost-reflect.
EDITIONS_COMPAT_REQUIRED = True


class DescriptorProtoMissingError(Exception):
    """Raised when 'descriptor.proto' is missing."""

class DescriptorProtoUnresolvedError(Exception):
    """Raised when 'descriptor.proto' is referenced but not loaded."""

class DescriptorProtoHasTargetsError(Exception):
    """Raised when 'descriptor.proto' itself has dependencies (corrupted)."""


# ---------------------------------------------------------------------------
# Spec 0087 — stripped unresolvable field/method type records
# ---------------------------------------------------------------------------

class StrippedField:
    """Field stripped from a DescriptorProto before pool_db.Add() because its
    type_name was not resolvable in pool_db at that time (spec 0087)."""
    __slots__ = ('number', 'name', 'type_name', 'label')

    def __init__(self, number: int, name: str, type_name: str, label: str) -> None:
        self.number = number
        self.name = name
        self.type_name = type_name
        self.label = label  # "optional" / "required" / "repeated"


class StrippedMethod:
    """Method stripped from a ServiceDescriptorProto before pool_db.Add()
    because its input_type or output_type was not resolvable (spec 0087)."""
    __slots__ = ('name', 'input_type', 'output_type')

    def __init__(self, name: str, input_type: str, output_type: str) -> None:
        self.name = name
        self.input_type = input_type
        self.output_type = output_type


_LABEL_NAMES: dict[int, str] = {
    FieldDescriptorProto.LABEL_OPTIONAL: 'optional',
    FieldDescriptorProto.LABEL_REQUIRED: 'required',
    FieldDescriptorProto.LABEL_REPEATED: 'repeated',
}


def _strip_unresolvable_field_types(
    ctx: 'Context',
    topo_file: 'ReFile',
    fdp: FileDescriptorProto,
) -> None:
    """Strip fields/methods whose types are unresolvable due to pruning (spec 0087).

    Called immediately after _strip_unresolvable_dependencies and before
    every pool_db.Add() call.  Stripped records are accumulated on topo_file
    keyed by FQDN; phase 3 dispatches them to the matching Re* nodes.

    A type_name is unresolvable iff pool_db.FindFileContainingSymbol() raises
    KeyError AND the symbol is not defined within fdp itself.

    Correctness relies on two invariants:
    - Topo-sort: all of fdp's surviving dependencies were already Add()ed to
      pool_db in earlier ranks, so their symbols are present.
    - _strip_unresolvable_dependencies ran first: pruned/absent imports are
      already removed from fdp.dependency, so their symbols are the only ones
      missing from pool_db at this point.
    - Own-file symbols (types defined in fdp itself) are not in pool_db yet;
      they are excluded via own_symbols to avoid false positives.
    """
    pkg = fdp.package
    prefix = f'.{pkg}' if pkg else ''
    own_symbols = set(_extract_fdp_symbols(fdp))
    stripped_count = 0

    def _is_unresolvable(type_name: str) -> bool:
        if not type_name:
            return False  # primitive field
        name = type_name.lstrip('.')
        if name in own_symbols:
            return False  # defined in the same file
        try:
            ctx.pool_db.FindFileContainingSymbol(name)
            return False
        except KeyError:
            return True

    from .lib.warnings import get_collector
    collector = get_collector()

    def _walk_messages(messages: Any, parent_fqdn: str) -> None:
        nonlocal stripped_count
        for msg in messages:
            msg_fqdn = f'{parent_fqdn}.{msg.name}'
            to_remove = []
            for field in msg.field:
                if _is_unresolvable(field.type_name):
                    label_str = _LABEL_NAMES.get(field.label, 'optional')
                    record = StrippedField(field.number, field.name, field.type_name, label_str)
                    topo_file.stripped_field_types.setdefault(msg_fqdn, []).append(record)
                    to_remove.append(field)
                    stripped_count += 1
                    if not ctx.quiet:
                        collector.w4(field.type_name)
            for field in to_remove:
                msg.field.remove(field)
            _walk_messages(msg.nested_type, msg_fqdn)

    _walk_messages(fdp.message_type, prefix)

    for svc in fdp.service:
        svc_fqdn = f'{prefix}.{svc.name}'
        to_remove = []
        for method in svc.method:
            unresolvable_types = [
                t for t in (method.input_type, method.output_type)
                if _is_unresolvable(t)
            ]
            if unresolvable_types:
                record = StrippedMethod(method.name, method.input_type, method.output_type)
                topo_file.stripped_method_types.setdefault(svc_fqdn, []).append(record)
                to_remove.append(method)
                stripped_count += 1
                if not ctx.quiet:
                    for t in unresolvable_types:
                        collector.w4(t)
        for method in to_remove:
            svc.method.remove(method)

# WellKnownTypeHasTargetsError: removed by spec 0052 — fallback WKTs may now
# import other fallback files (e.g. type.proto imports any + source_context).
# class WellKnownTypeHasTargetsError(Exception):
#     """Raised when a well-known type proto has targets (imports)."""


def import_annotations(modules: list[str], resource_root: str | None = None) -> None:
    """Import annotation modules declared by the active variant.

    If resource_root is given and not already on sys.path, it is prepended
    before any import is attempted.  This allows *_pb2.py files that live
    inside the variant bundle to be found without any extra setup.

    Does nothing when modules is empty.  Logs a warning for each module
    that cannot be imported, but continues execution.
    """
    if not modules:
        return
    if resource_root is not None and resource_root not in sys.path:
        sys.path.insert(0, resource_root)
    for full_module_name in modules:
        try:
            importlib.import_module(full_module_name)
        except ModuleNotFoundError:
            cli_warning(f"Module '{full_module_name}' not found.")


def fqdn_to_path(fqdn: Fqdn) -> Path:
    """Convert file FQDN to Path object.

    Args:
        fqdn: File FQDN in format 'file:path/to/file.proto'

    Returns:
        Path object extracted from FQDN

    Raises:
        AssertionError: If FQDN doesn't match expected file format
    """
    match = re.fullmatch(r"file:(.+)", str(fqdn))
    assert match, f"String does not match expected format: {fqdn}"
    g = match.group(1)
    assert isinstance(g, str)
    return Path(g)


def patch_go_package(ctx: Context, fdp: FileDescriptorProto) -> None:
    """Patch go_package option for Go stub generation.

    Replaces 'internal' path components with 'intern4l' to avoid
    Go reserved keyword conflicts, if ctx.go_root is specified.

    Args:
        ctx: Build context with go_root setting
        fdp: File descriptor proto to patch
    """
    # Patch the go_package option to fix go_lang stub issues
    if ctx.go_root is not None:
        go_package = f'{ctx.go_root}/{fdp.name}'
        go_package = re.sub(
            r'(^|/)(internal)(/|$)',
            r'\1intern4l\3',
            go_package
        )
        fdp.options.go_package = go_package


def _normalise_fqdn_name(prefix: str, name: str) -> str:
    """Normalise the name part of an FQDN for path-aware matching.

    For non-file/path prefixes, replace '.' with '/' and strip any leading
    '/'. For file: and path: prefixes, leave the name as-is (both already
    use '/' as their natural separator; see spec 0149).
    """
    if prefix in ('file', 'path'):
        return name
    return name.replace('.', '/').lstrip('/')


def fqdn_match(pattern: Fqdn, subject: Fqdn) -> bool:
    """Return True iff subject matches pattern.

    Normalises both sides before matching (. → / for non-file FQDNs).
    Matching is anchored (full path); * matches one segment, ** matches any
    number of segments (including zero).  Uses PurePosixPath.full_match()
    (Python 3.13+).
    """
    from pathlib import PurePosixPath
    if ':' not in pattern or ':' not in subject:
        return pattern == subject
    p_prefix, p_name = pattern.split(':', 1)
    s_prefix, s_name = subject.split(':', 1)
    if p_prefix != s_prefix:
        return False
    p_norm = _normalise_fqdn_name(p_prefix, p_name)
    s_norm = _normalise_fqdn_name(s_prefix, s_name)
    return PurePosixPath(f'/{s_norm}').full_match(f'/{p_norm}')


def fqdn_matches_any(subject: Fqdn, patterns: list[Fqdn]) -> bool:
    return any(fqdn_match(p, subject) for p in patterns)


def _find_matching_nodes(
    pattern: Fqdn,
    ctx: Context,
) -> list[tuple[Fqdn, Node]]:
    """Return all (fqdn, node) pairs whose fqdn matches pattern (exact or glob).

    Uses fqdn_match for all patterns so that normalisation (. → / for non-file
    FQDNs) is applied consistently on both sides regardless of whether the
    pattern contains glob characters.
    """
    return [
        (fqdn, node)
        for fqdn, node in ctx.nodes.items()
        if fqdn_match(pattern, fqdn)
    ]


def _fuzzy_suggest(pattern: str, ctx: Context) -> str | None:
    """Return the closest matching FQDN from ctx.nodes, or None if none is close enough.

    The suggestion strips the leading dot after the prefix (e.g. 'desc:.a.B' →
    'desc:a.B') so it can be pasted directly into --seed / --prune without
    modification.
    """
    best_match = None
    best_score = 85
    for node in ctx.nodes.values():
        score = fuzz.ratio(pattern, node.fqdn)
        if score > best_score:
            best_score = score
            best_match = node.fqdn
    if best_match is None:
        return None
    # Strip leading dot from the name part for user-facing display.
    if ':' in best_match:
        prefix, rest = best_match.split(':', 1)
        if rest.startswith('.'):
            return f'{prefix}:{rest[1:]}'
    return best_match


def _dump_resolved_features_yaml(ctx: Context, target_file: str) -> None:
    """Dump resolved FeatureSet for every element in target_file as YAML.

    Called when --dump-resolved-features is set.  Output goes to stdout.
    Only edition files produce meaningful output; for proto2/proto3 files the
    edition_defaults section will be empty (no FeatureSet in their descriptor).
    """
    import yaml  # lazy: only needed for this diagnostic path
    from .feature_resolution import feature_value_name, resolve_features

    # Locate the target ReFileDescriptorProto.
    # ctx.files/new_files are keyed by Fqdn("FILE:<filename>").
    file_fqdn = Fqdn(f'{FILE}:{target_file}')
    re_fdp = ctx.find_file(file_fqdn)
    if re_fdp is None:
        cli_warning(f"--dump-resolved-features: file '{target_file}' not found in loaded set")
        return

    fdp = re_fdp.this
    ed = fdp.edition     # int; 0 for proto2/proto3
    defaults = ctx.edition_defaults

    def _feat_name(feature: str, value: int) -> str:
        return feature_value_name(defaults, feature, value)

    def _resolve(*fsets: FeatureSet | None) -> ResolvedFeatures:
        return resolve_features(defaults, ed, *fsets)

    def _feat_dict(resolved: ResolvedFeatures) -> dict:
        from dataclasses import asdict
        return {k: _feat_name(k, v) for k, v in asdict(resolved).items()}

    def _overrides_dict(fs: FeatureSet | None) -> dict:
        """Return only the explicitly-set fields of a FeatureSet message."""
        if fs is None:
            return {}
        result = {}
        for fname in ("field_presence", "enum_type", "repeated_field_encoding",
                      "utf8_validation", "message_encoding", "json_format"):
            try:
                if fs.HasField(fname):
                    result[fname] = _feat_name(fname, getattr(fs, fname))
            except ValueError:
                pass
        return result

    def _file_features_dict(fs: FeatureSet | None) -> dict:
        return _overrides_dict(fs)

    # edition_defaults: resolved from defaults table only (no overrides)
    file_fs = fdp.options.features if fdp.options.HasField('features') else None
    defaults_resolved = _resolve()   # no overrides → pure edition defaults
    edition_defaults_dict = _feat_dict(defaults_resolved)

    # file-level resolved and overrides
    file_resolved = _resolve(file_fs)

    def _render_field(field_proto: FieldDescriptorProto, msg_fs: FeatureSet | None) -> dict:
        field_fs = field_proto.options.features if field_proto.options.HasField('features') else None
        resolved = _resolve(file_fs, msg_fs, field_fs)
        return {
            "resolved":  _feat_dict(resolved),
            "overrides": _overrides_dict(field_fs),
        }

    def _render_enum(enum_proto: EnumDescriptorProto) -> dict:
        enum_fs = enum_proto.options.features if enum_proto.options.HasField('features') else None
        resolved = _resolve(file_fs, enum_fs)
        return {
            "resolved":  _feat_dict(resolved),
            "overrides": _overrides_dict(enum_fs),
        }

    def _render_message(msg_proto: DescriptorProto) -> dict:
        msg_fs = msg_proto.options.features if msg_proto.options.HasField('features') else None
        resolved = _resolve(file_fs, msg_fs)
        entry: dict = {
            "resolved":  _feat_dict(resolved),
            "overrides": _overrides_dict(msg_fs),
        }
        if msg_proto.field:
            entry["fields"] = {
                f.name: _render_field(f, msg_fs) for f in msg_proto.field
            }
        if msg_proto.nested_type:
            entry["messages"] = {
                m.name: _render_message(m) for m in msg_proto.nested_type
            }
        if msg_proto.enum_type:
            entry["enums"] = {
                e.name: _render_enum(e) for e in msg_proto.enum_type
            }
        return entry

    doc: dict = {
        "file":             fdp.name,
        "edition":          ed,
        "edition_defaults": edition_defaults_dict,
        "file_features":    _file_features_dict(file_fs),
        "file_resolved":    _feat_dict(file_resolved),
    }
    if fdp.message_type:
        doc["messages"] = {m.name: _render_message(m) for m in fdp.message_type}
    if fdp.enum_type:
        doc["enums"] = {e.name: _render_enum(e) for e in fdp.enum_type}

    print(yaml.dump(doc, sort_keys=False, allow_unicode=True), end="")


def _make_context(
    options: Options | None,
    prunings: list[Fqdn],
    path_prunings: list[str] | None = None,
    path_seeds: list[str] | None = None,
) -> Context:
    from .load import PathPatterns
    pruned_paths = PathPatterns(set(path_prunings or []))
    seed_paths = PathPatterns(set(path_seeds or []))
    if options is None:
        ctx = Context(set(prunings), pruned_paths, seed_paths)
    else:
        ctx = Context.from_options(set(prunings), pruned_paths, seed_paths, options)
    import_annotations(
        ctx.variant_annotation_modules,
        str(ctx.variant_root.joinpath(ctx.variant_stem)),
    )
    return ctx


def _phase1_load_files(
    ctx: Context,
    in_repo: list[Path],
    seed_paths: list[Path] | list[QualFile],
    topo: Topology,
) -> set[ReFile]:
    """Phase 1: Load seed files and recursively discover all imported dependencies."""

    # Pre-register fallback protos so W1 is suppressed for them.
    # The import-discovery loop calls load_from_path for every dependency; if a
    # dependency is a well-known type that will be satisfied by an embedded
    # fallback, it won't be found on the -I path and w1() would fire a spurious
    # warning.  register_fallback_file() tells w1() to suppress those misses
    # without affecting W5 (render-phase) suppression.
    from .lib.warnings import get_collector as _get_collector
    for _fp in ctx.fallback_protos:
        _get_collector().register_fallback_file(_fp)

    seed_files: set[ReFile] = set()

    if seed_paths and isinstance(seed_paths[0], QualFile):
        # Ingest pre-loaded QualFiles into the topology
        for f in seed_paths:
            assert isinstance(f, QualFile)
            file = ReFile(topo, f)
            file.is_seed = True
            seed_files.add(file)
        topo.merge_files()

    else:
        # Load files from the CLI's seed_paths
        qual_files: list[QualFile] = []
        for path in seed_paths:
            assert isinstance(path, Path)
            qual_files.extend(load_from_path(ctx, in_repo, path))

        # Ingest loaded files into the topology
        known_files: dict[str, ReFile] = {}

        # Read seed paths (if not pruned)
        for qual_file in qual_files:
            file = ReFile(topo, qual_file)
            file.is_seed = True
            seed_files.add(file)
            name = file.name
            if name not in known_files:
                known_files[name] = file

        from .lib.console import progress_lazy as _progress_lazy
        # Read imported paths (if not pruned)
        with _progress_lazy('Discovering imported files', quiet=ctx.quiet) as (advance, _):
            while topo.new_files:
                advance()
                topo.merge_files()
                for file in topo.files.values():
                    assert isinstance(file, ReFile)
                    if file.is_ref():
                        if file not in known_files:
                            name = file.name
                            known_files[name] = file
                            qfiles: list[QualFile] = load_from_path(ctx, in_repo, Path(name))
                            if len(qfiles) != 1:
                                pass  # W1 warning already emitted by load_from_path
                            else:
                                ReFile(topo, qfiles[0])
                                if ctx.debug:
                                    cli_info(f"  Loading: {name}")
            topo.merge_files()

    # Helper function to load embedded proto fallbacks
    def load_embedded_proto_fallback(proto_name: str) -> 'QualFile | None':
        """
        Load a proto fallback from the variant's resource directory.

        Traverses ctx.variant_root / ctx.variant_stem / <pb_name> via the
        Traversable API, which works for both directory and zip/wheel installs.

        Args:
            proto_name: Proto file name (e.g., "google/protobuf/any.proto")

        Returns:
            The QualFile built from the embedded data, or None on failure.
        """
        try:
            pb_name = proto_name[:-len('.proto')] + '.pb'

            node = ctx.variant_root
            for part in [ctx.variant_stem] + pb_name.split('/'):
                node = node.joinpath(part)
            data = node.read_bytes()

            fds = FileDescriptorSet()
            fds.ParseFromString(data)
            fdp = fds.file[0]
            qual_file = QualFile(Path('internal'), Path(proto_name), fdp.SerializeToString())
            qual_file.name = fdp.name
            qual_file.desc = fdp
            ReFile(topo, qual_file)

            if not ctx.quiet:
                cli_attention(f"Using embedded fallback: {proto_name}")
            return qual_file
        except (ImportError, FileNotFoundError, DecodeError, IndexError, AttributeError) as e:
            if ctx.debug:
                cli_warning(f"Failed to load embedded fallback for {proto_name}: {e}")
            return None

    # Load requested fallbacks (well-known types and descriptor.proto)
    # These fallbacks replace any versions that might exist in the input files.
    # We keep them in the reconstructed files so protoc can compile them.
    for fallback_proto in ctx.fallback_protos:
        proto_fqdn = Fqdn(FILE + ':' + fallback_proto)
        ctx.pruned_fqdns.discard(proto_fqdn)  # safe remove (no KeyError)
        # Do NOT pop from topo.files here.  If the import-discovery loop already
        # created a ref ReFile for this name (e.g. any.proto listed as a
        # dependency of a seed file), that ref is held in the importing file's
        # targets set.  Popping it and creating a new instance breaks topo-sort
        # object-identity: the importing file appears to have no unresolved
        # dependencies and lands in the same topo rank as the fallback, causing
        # pool_db.Add to fail silently and rendering to emit a spurious W5.
        # Instead we let ReFile.__new__ return the existing instance (ref or
        # full) so that all targets references remain valid, then force-overwrite
        # qfile below so the fallback content always wins (spec 0051).

        fallback_qfile = load_embedded_proto_fallback(fallback_proto)
        topo.merge_files()

        # Force the fallback QualFile to win regardless of what was previously
        # loaded from disk (covers the case where a full ReFile already existed).
        if fallback_qfile is not None:
            proto_file: ReFile = topo.files[fallback_proto]
            proto_file.qfile = fallback_qfile

        # Note: fallback files may have imports (e.g. type.proto imports any
        # and source_context; api.proto imports timestamp and source_context).
        # Those imports must appear earlier in ctx.fallback_protos so the
        # topo-sort sees them in the right order — enforced by cli.py ordering.

    return seed_files


def _strip_self_dependency(fdp: FileDescriptorProto) -> None:
    """Remove any self-referential entries from fdp.dependency in-place.

    Some malformed .pb files in the wild list their own name as a dependency
    (likely an artifact of a broken descriptor-set extraction script).  The
    protobuf C extension segfaults when such a descriptor is added to the pool,
    so we strip the bogus entry before calling pool_db.Add().
    """
    self_deps = [d for d in fdp.dependency if d == fdp.name]
    for _ in self_deps:
        fdp.dependency.remove(fdp.name)
        cli_warning(
            "Stripped self-dependency from '%s' (malformed descriptor)", fdp.name
        )


def _strip_unresolvable_dependencies(
    ctx: Context,
    topo_file: 'ReFile',
    fdp: FileDescriptorProto,
) -> None:
    """Remove unresolvable entries from fdp.dependency in-place.

    Two cases are handled:

    1. Pruned-duplicate files (spec 0053): P's symbols conflict with an
       already-loaded file; P's path is in ctx.pruned_file_names.
       Emits W3.

    2. Absent files: P was listed as a dependency but was never provided
       in any input .pb (its topo entry is still a ref).  Without stripping,
       pool.FindFileByName on the importer raises TypeError and prevents the
       importer from rendering at all.  Emits W1.

    In both cases the stripped path is recorded on the topo-layer ReFile so
    phase 3 can copy it to the ReFileDescriptorProto node for orphan rendering
    (a commented-out import vestige in the output .proto).
    """
    from .lib.warnings import get_collector
    collector = get_collector()
    public_indices = set(fdp.public_dependency)
    to_strip: list[tuple[int, str]] = []
    for i, dep in enumerate(fdp.dependency):
        if dep in ctx.pruned_file_names:
            to_strip.append((i, dep))
        else:
            topo_dep = topo_file.topo.files.get(dep)
            if topo_dep is not None and topo_dep.is_ref():
                to_strip.append((i, dep))

    if not to_strip:
        return

    for i, dep in reversed(to_strip):
        if dep in public_indices:
            topo_file.stripped_public_dependencies.append(dep)
        else:
            topo_file.stripped_dependencies.append(dep)
        fdp.dependency.remove(dep)
        if dep in ctx.pruned_file_names:
            if not ctx.quiet:
                collector.w3(
                    f"Warning: stripped pruned dependency \"{dep}\" from {fdp.name}"
                )
        else:
            collector.w1(dep)


def _extract_fdp_symbols(fdp: FileDescriptorProto) -> list[str]:
    """Return all fully-qualified symbol names defined in fdp.

    Mirrors the logic of descriptor_database._ExtractSymbols, which is not
    part of the public API.  We need this to probe the pool_db for conflicts
    before calling pool_db.Add().
    """
    pkg = fdp.package
    prefix = pkg if pkg else ''

    def _collect(
        messages: list[DescriptorProto],
        enums: list[EnumDescriptorProto],
        parent: str,
    ) -> list[str]:
        syms: list[str] = []
        for m in messages:
            fqn = f'{parent}.{m.name}' if parent else m.name
            syms.append(fqn)
            syms.extend(_collect(list(m.nested_type), list(m.enum_type), fqn))
        for e in enums:
            syms.append(f'{parent}.{e.name}' if parent else e.name)
        return syms

    return _collect(list(fdp.message_type), list(fdp.enum_type), prefix)


def _translate_fdp_to_proto2(fdp: FileDescriptorProto) -> FileDescriptorProto:
    """Return a deep copy of fdp with editions→proto2 translation applied.

    Used by _phase_build_schema_db to produce semantically-correct proto2 FDPs
    for consumption by prost-reflect (which does not support editions syntax).

    Translation: clear syntax/edition fields (proto2 convention = unset), then
    recursively clear all features entries from options at every level.  This
    does NOT apply label/type/packed translations — those are only needed for
    the --emit-binary render path.  prost-reflect reads the pool FDPs, which
    already carry the correct label/type values from protoc compilation; only
    the editions metadata (syntax, edition, features) needs to be stripped.
    """
    out = FileDescriptorProto()
    out.CopyFrom(fdp)
    # Strip editions metadata so prost-reflect sees a valid proto2 descriptor.
    out.ClearField('syntax')
    out.ClearField('edition')
    if out.HasField('options'):
        out.options.ClearField('features')
    for msg in out.message_type:
        _clear_features_recursive(msg)
    for enum in out.enum_type:
        if enum.HasField('options'):
            enum.options.ClearField('features')
        for val in enum.value:
            if val.HasField('options'):
                val.options.ClearField('features')
    for svc in out.service:
        if svc.HasField('options'):
            svc.options.ClearField('features')
        for method in svc.method:
            if method.HasField('options'):
                method.options.ClearField('features')
    for ext in out.extension:
        if ext.HasField('options'):
            ext.options.ClearField('features')
    return out


def _clear_features_recursive(msg: 'DescriptorProto') -> None:
    """Recursively clear features from all options in a DescriptorProto tree."""
    if msg.HasField('options'):
        msg.options.ClearField('features')
    for field in msg.field:
        if field.HasField('options'):
            field.options.ClearField('features')
    for oneof in msg.oneof_decl:
        if oneof.HasField('options'):
            oneof.options.ClearField('features')
    for nested in msg.nested_type:
        _clear_features_recursive(nested)
    for enum in msg.enum_type:
        if enum.HasField('options'):
            enum.options.ClearField('features')
        for val in enum.value:
            if val.HasField('options'):
                val.options.ClearField('features')
    for ext in msg.extension:
        if ext.HasField('options'):
            ext.options.ClearField('features')


def _prune_if_duplicate(
    ctx: Context,
    n: ReFile,
    fdp: FileDescriptorProto,
) -> bool:
    """Check fdp for symbols already registered in pool_db.

    If any conflicts are found, emit a W3 warning, mark n as pruned, and
    return True (caller should skip pool_db.Add).  Returns False otherwise.
    """
    conflicts: dict[str, str] = {}
    for sym in _extract_fdp_symbols(fdp):
        try:
            existing = ctx.pool_db.FindFileContainingSymbol(sym)
            conflicts[sym] = existing.name
        except KeyError:
            pass
    if not conflicts:
        return False
    by_file: dict[str, int] = {}
    for fname in conflicts.values():
        by_file[fname] = by_file.get(fname, 0) + 1
    parts = [f"Warning: file:{n.name} pruned — duplicate symbols with:"]
    for fname, count in sorted(by_file.items()):
        noun = "symbol" if count == 1 else "symbols"
        parts.append(f"    file:{fname} ({count} {noun})")
    from .lib.warnings import get_collector
    get_collector().w3('\n'.join(parts))
    n.is_pruned = True
    ctx.pruned_file_names.add(n.name)
    return True


def _phase2_build_pool(
    ctx: Context,
    topo: Topology,
    seed_files: set[ReFile],
    prunings: list[Fqdn],
) -> None:
    """Phase 2: Build the descriptor pool in topological order."""
    # === Load FileDescriptors in the pool_db =================================

    # --- Mark seeds ----------------------------------------------------------

    reachable_files: set[ReFile] = set()
    seeding_files: set[ReFile] = set()
    fresh_files: set[ReFile] = set()

    for file in seed_files:
        assert isinstance(file, ReFile)
        if not file.is_ref():  # and not file.is_pruned:
            file.is_reachable = True
            seeding_files.add(file)

    # --- Propagates forwards (targeted nodes) --------------------------------

    while seeding_files:
        for seed in seeding_files:
            for child in seed.targets:
                assert isinstance(child, ReFile)
                # propagate to chldren, if they are not dangling and not pruned
                #if child.is_pruned:
                #    continue
                if child.is_ref():
                    continue
                if child not in reachable_files and child not in seeding_files:
                    child.is_reachable = True
                    fresh_files.add(child)
        reachable_files.update(seeding_files)
        seeding_files = fresh_files
        fresh_files = set()

    # --- Display the results (debug only) ------------------------------------

    if ctx.debug_fqdn:
        for name in topo.files:
            file = topo.find_file(name)
            assert isinstance(file, File)
            cli_info(
                "FILE %s %s %s %s",
                '-' if file not in seed_files else 'S',
                '-' if file.is_ref() else 'P',
                '-' if file.is_ref() or not file.is_reachable else 'R',
                name
            )

    # -------------------------------------------------------------------------

    from .lib.console import progress as _progress

    # --- Locate descriptor.proto ---------------------------------------------

    if ctx.variant_descriptor_proto not in topo.files:
        cli_error(
            f"descriptor.proto not found in input files "
            f"({ctx.variant_descriptor_proto}); "
            f"add it to your input set or use --use-variant to load the "
            f"embedded fallback"
        )
        raise DescriptorProtoMissingError
    descriptor_proto: ReFile = topo.files[ctx.variant_descriptor_proto]
    if descriptor_proto.is_ref():
        raise DescriptorProtoUnresolvedError
    if descriptor_proto.targets:
        raise DescriptorProtoHasTargetsError

    # --- Merging the file descriptor sets into pool_db ----------------------

    files: set[ReFile] = {
        file for file in topo.files.values()
    }
    leaves: set[ReFile] = set()
    non_leaves: set[ReFile] = set()

    if ctx.phase2_plugin:
        exec_context = {}
        exec(ctx.phase2_plugin, exec_context)
        phase2_plugin = exec_context["phase2_plugin"]
    else:
        def phase2_plugin(
            _ctx: Context,
            _fdp: FileDescriptorProto,
        ) -> None:
            pass

    total_files = 0 if ctx.quiet else len(topo.files)
    with _progress('Loading descriptors', total_files, quiet=ctx.quiet) as advance:
        for i in itertools.count(start=1):
            # Find leaf-files (i.e.: do not target other files)
            for n in files:
                is_leaf = all(t not in files for t in n.targets)
                if is_leaf:
                    leaves.add(n)
                else:
                    non_leaves.add(n)
            if not leaves:
                # No more leaf-files, exit the loop
                break
            # Merge the leaf-FDPs into the descriptors pool
            if ctx.debug:
                cli_info(f"  Rank {i}: processing {len(leaves)} files")
            for n in leaves:
                if ctx.debug:
                    cli_info(f"  Merging: {n.name}")
                if n.is_ref():
                    advance()
                    continue
                if fqdn_matches_any(Fqdn(f'file:{n.name}'), prunings):
                    n.is_pruned = True
                    ctx.pruned_file_names.add(n.name)
                    advance()
                    continue
                qf = n.qfile
                contents = qf.contents
                try:
                    match contents:
                        case str():
                            # Text format — contents is a bare FDP fragment (already
                            # decapsulated by split_fdps; no entry{} wrapper present)
                            fdp = text_format.Parse(
                                contents,
                                FileDescriptorProto(),
                                allow_unknown_field=True,
                                allow_unknown_extension=True,
                                descriptor_pool=ctx.pool,
                            )
                            phase2_plugin(ctx, fdp)
                            patch_go_package(ctx, fdp)
                            _strip_self_dependency(fdp)
                            if not ctx.keep_duplicates and _prune_if_duplicate(ctx, n, fdp):
                                advance()
                                continue
                            _strip_unresolvable_dependencies(ctx, n, fdp)
                            _strip_unresolvable_field_types(ctx, n, fdp)
                            ctx.pool_db.Add(fdp)
                            ctx.pool_db_fdps.append(fdp)

                        case bytes():
                            # Binary format — contents is a serialised single FDP
                            fdp = FileDescriptorProto()
                            fdp.ParseFromString(contents)
                            try:
                                phase2_plugin(ctx, fdp)
                                patch_go_package(ctx, fdp)
                                _strip_self_dependency(fdp)
                                if not ctx.keep_duplicates and _prune_if_duplicate(ctx, n, fdp):
                                    advance()
                                    continue
                                _strip_unresolvable_dependencies(ctx, n, fdp)
                                _strip_unresolvable_field_types(ctx, n, fdp)
                                ctx.pool_db.Add(fdp)
                                ctx.pool_db_fdps.append(fdp)
                            except TypeError as e:
                                # NOTE: TypeError can occur when attempting to add a descriptor
                                # that conflicts with an existing one in the pool (e.g., duplicate
                                # symbol names or incompatible descriptor types).
                                cli_warning("Could not add descriptor from '%s' to pool: %s", n.name, e)
                except message.DecodeError:
                    cli_warning(f"Skipping unparseable file: {n.name}")
                    n.is_pruned = True
                advance()
            # Prepare for next loop iteration
            files = non_leaves
            non_leaves = set()
            leaves = set()

    if non_leaves:
        # The remaining files have a circular dependency; mark them pruned so
        # that phase 3 skips them (they were never added to the pool).
        for n in non_leaves:
            cli_warning(f"Circular dependency detected: {n.name}")
            n.is_pruned = True

    # --- Build edition-default table from the variant's descriptor.pb --------
    # pool_db.FindFileByName returns the FileDescriptorProto directly.
    try:
        descriptor_fdp = ctx.pool_db.FindFileByName(ctx.variant_descriptor_proto)
        ctx.edition_defaults = build_edition_defaults(descriptor_fdp)
    except KeyError:
        ctx.edition_defaults = {}


def _phase3_build_graph(
    ctx: Context,
    topo: Topology,
) -> None:
    """Phase 3: Build FQDN graph — create Re* wrapper nodes and populate ctx.nodes."""
    from .lib.console import progress as _progress

    total_files = 0 if ctx.quiet else len(topo.files)
    with _progress('Building type graph', total_files, quiet=ctx.quiet) as advance:
        # Create ReFileDescriptorProto nodes for all loaded files
        for file in topo.files.values():
            if not (file.is_ref() or file.is_pruned):
                desc = file.qfile.desc
                proto = ctx.pool_db.FindFileByName(desc.name)
                ReFileDescriptorProto(ctx, proto)
            advance()

    ctx.merge_nodes()

    # Propagate topology-level is_pruned to context nodes.  Pruned files are
    # skipped above, but stubs for them are created lazily by _initialize_from_message
    # when non-pruned files list them as dependencies.  Without this pass those
    # stubs have is_pruned=False, which causes phase 5 to mark them reachable and
    # phase 6 to mark them summoned, leading to a crash in phase 7 when render
    # calls .name on an uninitialised node.
    # Also copy stripped_dependencies recorded in phase 2 onto the Re* nodes
    # so the render path can emit them as orphan import lines (spec 0053).
    # Dispatch stripped field/method types to their Re* nodes (spec 0087).
    from .re_descriptor import ReDescriptorProto as _ReDesc
    from .re_service import ReServiceDescriptorProto as _ReSvc
    from .fake_types import Ref as _Ref
    for name, file in topo.files.items():
        fqdn = ReFileDescriptorProto.fqdn_from_ref(name)
        node = ctx.find_file(fqdn)
        if file.is_pruned and node is not None:
            node.is_pruned = True
        if node is not None and (file.stripped_dependencies
                                 or file.stripped_public_dependencies):
            node.stripped_dependencies = list(file.stripped_dependencies)
            node.stripped_public_dependencies = list(file.stripped_public_dependencies)
        # spec 0087: dispatch stripped field records to ReDescriptorProto nodes
        for msg_fqdn, fields in file.stripped_field_types.items():
            msg_node_fqdn = _ReDesc.fqdn_from_ref(_Ref(msg_fqdn))
            msg_node = ctx.find_node(msg_node_fqdn)
            if isinstance(msg_node, _ReDesc):
                msg_node.stripped_fields.extend(fields)
        # spec 0087: dispatch stripped method records to ReServiceDescriptorProto nodes
        for svc_fqdn, methods in file.stripped_method_types.items():
            svc_node_fqdn = _ReSvc.fqdn_from_ref(_Ref(svc_fqdn))
            svc_node = ctx.find_node(svc_node_fqdn)
            if isinstance(svc_node, _ReSvc):
                svc_node.stripped_methods.extend(methods)


def _phase4_pruning(ctx: Context, topo: Topology, prunings: list[Fqdn]) -> None:
    """Phase 4: Mark user-specified prunings and propagate to contained children."""
    from .lib.console import spinning as _spinning
    label = 'Processing exclusions' if prunings else 'Applying pruning rules'
    with _spinning(label, quiet=ctx.quiet):
        transitive_prunnings: set[Node] = set()
        current_prunings: set[Node] = set()

        # Start from user-specified prunings
        for pruning_pattern in prunings:  # the user-specified prunings
            matched_nodes = _find_matching_nodes(pruning_pattern, ctx)

            if not matched_nodes:
                # Suppress the warning if the pattern was already satisfied at
                # phase 2 (file was loaded but skipped into ctx.pruned_file_names
                # before it could be added to ctx.nodes).
                pat_str = str(pruning_pattern)
                if pat_str.startswith('file:'):
                    already_pruned = pat_str[len('file:'):] in ctx.pruned_file_names
                else:
                    already_pruned = False
                if not already_pruned:
                    cli_warning(f"Pruning target not found: {pruning_pattern}")
                    if '*' not in pruning_pattern and '?' not in pruning_pattern:
                        suggestion = _fuzzy_suggest(pruning_pattern, ctx)
                        if suggestion is not None:
                            cli_attention(f"  Did you mean: {suggestion}?")
                continue

            for matched_fqdn, matched_node in matched_nodes:
                assert isinstance(matched_node, Node)
                if not matched_node.is_present():
                    continue
                matched_node.is_pruned = True
                # Enforce "prune overrides seed": if this is a file node, clear
                # is_seed so that phase 5's default-seed path does not mark it
                # reachable.
                if isinstance(matched_node, ReFileDescriptorProto):
                    topo_file = topo.files.get(matched_node.name)
                    if topo_file is not None:
                        topo_file.is_seed = False
                current_prunings.add(matched_node)

        # --- Propagate stumps (contains relation) --------------------------------

        while current_prunings:
            transitive_prunnings.update(current_prunings)
            new_prunings: set[Node] = set()
            for node in current_prunings:
                for child in node.contains:
                    assert (isinstance(child, Node)
                            and child is not None
                            and not child.is_pruned)
                    if not child.is_present():
                        continue
                    if child not in transitive_prunnings:
                        child.is_pruned = True
                        new_prunings.add(child)
            current_prunings = new_prunings

        # Register all pruned file names (both topology-pruned from phase 2/3 and
        # user-pruned here) with the warning collector, so that W5 warnings for
        # their importers are suppressed — their absence from pool_db is intentional.
        from .lib.warnings import get_collector
        _collector = get_collector()
        for name, file in topo.files.items():
            if file.is_pruned:
                _collector.register_pruned_file(name)


def _phase5_reachability(
    ctx: Context,
    seeds: list[Fqdn],
    topo: Topology,
) -> None:
    """Phase 5: Mark all nodes transitively reachable from seeds (forward propagation)."""
    from .lib.console import progress as _progress, progress_lazy as _progress_lazy

    transitive_reachables: set[Node] = set()
    current_reachables: set[Node] = set()

    with _progress_lazy('Marking seeds', quiet=ctx.quiet) as (advance, _):
        # Did the user explicitly specify seeds?
        if seeds:
            for seed_pattern in seeds:
                matched_nodes = _find_matching_nodes(seed_pattern, ctx)

                if not matched_nodes:
                    cli_warning(f"Seed not found: {seed_pattern}")
                    if '*' not in seed_pattern and '?' not in seed_pattern:
                        suggestion = _fuzzy_suggest(seed_pattern, ctx)
                        if suggestion is not None:
                            cli_attention(f"  Did you mean: {suggestion}?")
                    continue

                for matched_fqdn, matched_node in matched_nodes:
                    assert isinstance(matched_node, Node)
                    if not matched_node.is_present():
                        continue
                    if matched_node.is_pruned:
                        cli_info(f"  Skipping pruned seed: {matched_fqdn}")
                        continue
                    matched_node.is_reachable = True
                    current_reachables.add(matched_node)

        # If not, use the seed-files as seeds
        else:
            # Re-mark seed files
            for fqdn, node in ctx.nodes.items():
                prefix, ref = parse_fqdn(fqdn)
                if prefix != FILE:
                    continue
                # Extract file name from fqdn
                # fqdn format: "file:path/to/file.proto"
                # parse_fqdn returns ref as "path/to/file.proto"
                name = ref
                assert name in topo.files
                file = topo.files[name]
                if not file.is_ref() and file.is_seed:
                    node.is_reachable = True
                    current_reachables.add(node)

    initial_nodes = current_reachables  # for --debug

    # --- Propagates forwards (targeted nodes) --------------------------------

    total_nodes = 0 if ctx.quiet else len(ctx.nodes)
    with _progress('Computing reachability', total_nodes, quiet=ctx.quiet) as advance:
        while current_reachables:
            transitive_reachables.update(current_reachables)
            new_reachables: set[Node] = set()
            for node in current_reachables:
                advance()
                for target in node.targets:
                    assert (isinstance(target, Node)
                            and target is not None)
                    if target.is_pruned:
                        continue
                    if not target.is_present():
                        continue
                    if target not in transitive_reachables:
                        if target.seeder is None:
                            target.seeder = node
                        target.is_reachable = True
                        new_reachables.add(target)
            current_reachables = new_reachables

    # --- Display the results (debug only) ------------------------------------
    if ctx.debug_fqdn:
        for fqdn in ctx.nodes:
            node = ctx.find_node(fqdn)
            assert isinstance(node, Node)
            cli_info(
                "FQDN %s %s %s %s",
                'S' if node in initial_nodes else '-',
                'P' if node.is_present else '-',
                'R' if node.is_present and node.is_visible() else '-',
                fqdn,
            )


def _shortest_lex_path(
    start: 'ReFileDescriptorProto',
    end: 'ReFileDescriptorProto',
    import_graph: 'dict[ReFileDescriptorProto, list[ReFileDescriptorProto]]',
) -> 'list[ReFileDescriptorProto]':
    """Return the shortest import path from start to end, lex-smallest if tied.

    Shortest = fewest hops.  Among all shortest paths, choose the one whose
    sequence of intermediate node names is lexicographically smallest.
    This is achieved by BFS with neighbours pre-sorted by name: the first
    time a node is reached is always via the lex-smallest shortest path.

    Returns the full path [start, ..., end], or [] if no path exists.
    import_graph[F] must be F's direct imports sorted by name.
    """
    from collections import deque
    if start is end:
        return [start]
    prev: dict[ReFileDescriptorProto, ReFileDescriptorProto] = {start: start}
    queue: deque[ReFileDescriptorProto] = deque([start])
    while queue:
        node = queue.popleft()
        for neighbour in import_graph.get(node, []):
            if neighbour in prev:
                continue
            prev[neighbour] = node
            if neighbour is end:
                path: list[ReFileDescriptorProto] = []
                cur: ReFileDescriptorProto = end
                while cur is not start:
                    path.append(cur)
                    cur = prev[cur]
                path.append(start)
                path.reverse()
                return path
            queue.append(neighbour)
    return []


def _phase6_summoning(ctx: Context) -> None:
    """Phase 6: Mark container nodes of reachable nodes (backward propagation)."""
    from .lib.console import progress as _progress, progress_lazy as _progress_lazy

    # --- Sub-pass 1: seed summoning ------------------------------------------
    # Mark every reachable node as summoned, then propagate upward through the
    # parent relation until file nodes are reached.

    current_summoned: set[Node] = set()

    total_nodes = 0 if ctx.quiet else len(ctx.nodes)
    with _progress('Marking summoned nodes', total_nodes, quiet=ctx.quiet) as advance:
        for node in ctx.nodes.values():
            advance()
            if not node.is_reachable:
                continue
            node.is_summoned = True
            current_summoned.add(node)

    while current_summoned:
        new_summoned: set[Node] = set()
        for node in current_summoned:
            if node.parent is None:
                continue
            if node.parent.is_summoned:
                continue
            node.parent.is_summoned = True
            new_summoned.add(node.parent)
        current_summoned = new_summoned

    # --- Sub-pass 2: import bridging (spec 0046) ------------------------------
    # For each summoned file A and each of A's type-level targets T whose host
    # file C is also summoned, find the shortest (lex-smallest if tied) import
    # path from A to C and summon all intermediate files on it.
    # Repeat until stable: newly summoned bridge files may themselves need
    # bridges for their own type-level targets.

    # Build import graph: file node -> sorted list of directly imported file nodes.
    import_graph: dict[ReFileDescriptorProto, list[ReFileDescriptorProto]] = {}
    for node in ctx.nodes.values():
        if not isinstance(node, ReFileDescriptorProto):
            continue
        if not node.is_present() or node.is_pruned:
            continue
        imports = sorted(
            (t for t in node.targets
             if isinstance(t, ReFileDescriptorProto)
             and t.is_present()
             and not t.is_pruned),
            key=lambda f: f.name,
        )
        import_graph[node] = imports

    def _host_file(node: Node) -> 'ReFileDescriptorProto | None':
        """Walk parent chain to find the hosting ReFileDescriptorProto."""
        cur: Node = node
        while cur.parent is not None:
            cur = cur.parent
        if isinstance(cur, ReFileDescriptorProto):
            return cur
        return None

    def _all_type_targets(file_node: ReFileDescriptorProto) -> 'set[Node]':
        """Collect all non-file targets reachable via the contains tree of file_node."""
        result: set[Node] = set()
        stack: list[Node] = [file_node]
        visited: set[Node] = set()
        while stack:
            n = stack.pop()
            if n in visited:
                continue
            visited.add(n)
            for t in n.targets:
                if not isinstance(t, ReFileDescriptorProto) and t.is_present():
                    result.add(t)
            for c in n.contains:
                stack.append(c)
        return result

    with _progress_lazy('Bridging import paths', quiet=ctx.quiet) as (advance, _):
        changed = True
        while changed:
            changed = False
            advance()
            summoned_files = [
                node for node in ctx.nodes.values()
                if isinstance(node, ReFileDescriptorProto)
                and node.is_summoned
            ]
            for file_a in summoned_files:
                for target in _all_type_targets(file_a):
                    file_c = _host_file(target)
                    if file_c is None or not file_c.is_summoned:
                        continue
                    if file_c is file_a:
                        continue
                    path = _shortest_lex_path(file_a, file_c, import_graph)
                    if not path:
                        continue
                    # Summon all intermediate files (exclude start and end)
                    for intermediate in path[1:-1]:
                        if not intermediate.is_summoned:
                            intermediate.is_summoned = True
                            changed = True

    # --- Sub-pass 3: DB dependency closure (only when --build-schema-db) ------
    # Identify fallback/WKT files that are transitive dependencies of summoned
    # files but not themselves summoned.  Collect them into
    # ctx.schema_db_extra_nodes for later binary rendering inside
    # _phase_build_schema_db.  Do NOT modify is_summoned or is_reachable here —
    # phase 7 must run unmodified.
    if ctx.build_schema_db:
        from .re_file import ReFileDescriptorProto as _ReFileFDP
        from .fake_types import Ref as _Ref

        with _progress_lazy('Pulling in WKT dependencies', quiet=ctx.quiet) as (advance, _):
            seen: set[str] = {
                node.name for node in ctx.nodes.values()
                if isinstance(node, _ReFileFDP) and node.is_summoned
            }
            work: list[_ReFileFDP] = [
                node for node in ctx.nodes.values()
                if isinstance(node, _ReFileFDP) and node.is_summoned
            ]
            while work:
                advance()
                file_node = work.pop()
                for dep_name in file_node.dependency:
                    if dep_name in seen:
                        continue
                    seen.add(dep_name)
                    fqdn = _ReFileFDP.fqdn_from_ref(_Ref(dep_name))
                    dep_node = ctx.find_node(fqdn)
                    if dep_node is None or not isinstance(dep_node, _ReFileFDP):
                        continue
                    if not dep_node.is_present() or dep_node.is_pruned:
                        continue
                    ctx.schema_db_extra_nodes.append(dep_node)
                    work.append(dep_node)


def _phase7_output(ctx: Context, out_repo: Path) -> None:
    """Phase 7: Generate and write reconstructed .proto files."""
    create_option_message_classes(ctx)

    from .lib.console import progress as _progress

    summoned = [
        re_fdp for re_fdp in ctx.nodes.values()
        if isinstance(re_fdp, ReFileDescriptorProto)
        and re_fdp.is_present()
        and re_fdp.is_summoned
    ]

    from .mappings import canonize_dependency
    with _progress('Rendering proto files', 0 if ctx.quiet else len(summoned), quiet=ctx.quiet) as advance:
        for re_fdp in summoned:
            # --no-emit-descriptor suppresses disk output only (spec 0150):
            # descriptor.proto (or a variant's equivalent) is otherwise
            # rendered and binary-accumulated exactly like any other WKT,
            # so --schema-db-out/--build-schema-db stays transitively
            # complete even when it is not written to --proto-out.
            suppress_disk_output = (
                re_fdp.name == ctx.variant_descriptor_proto
                and not ctx.write_variant_descriptor
            )
            canonical_name = canonize_dependency(ctx, re_fdp.name)
            res_path = out_repo / Path(canonical_name)
            if ctx.debug and not suppress_disk_output:
                cli_info(f"  Writing: {res_path}")

            # Make sure all parent directories exist
            if not ctx.dry_run and not suppress_disk_output:
                res_path.parent.mkdir(parents=True, exist_ok=True)

            # Render whenever we need output: text write, --emit-binary, or
            # --build-schema-db (which may run without -O, i.e. with dry_run).
            need_render = not ctx.dry_run or ctx.binary or bool(ctx.build_schema_db)
            if need_render:
                from .context import DescOut
                from .syntax import fdp_syntax

                need_slot = ctx.binary or bool(ctx.build_schema_db)
                # For schema-db: temporarily force proto2 for editions files.
                saved_force = ctx.force_proto2_for_editions
                if ctx.build_schema_db and EDITIONS_COMPAT_REQUIRED:
                    if fdp_syntax(re_fdp.this) == "editions":
                        ctx.force_proto2_for_editions = True
                slot = DescOut() if need_slot else None
                ctx.out_desc = slot
                try:
                    content = re_fdp.render(ctx)[0].flush(ctx)
                except (KeyError, ValueError, TypeError, AttributeError) as e:
                    ctx.out_desc = None
                    ctx.force_proto2_for_editions = saved_force
                    from .lib.warnings import get_collector
                    from .anomalies import _classify_exc
                    clean_msg, w4, w5 = _classify_exc(str(e))
                    if w4 is not None:
                        get_collector().w4(w4)
                    elif w5 is not None:
                        get_collector().w5(w5)
                    else:
                        get_collector().w6(re_fdp.name, '', clean_msg)
                    if ctx.debug:
                        cli_warning(str(re_fdp.this))
                    advance()
                    continue
                ctx.out_desc = None
                ctx.force_proto2_for_editions = saved_force

                # Write binary (.pb) if requested (skipped in dry-run mode)
                if ctx.binary and not ctx.dry_run and not suppress_disk_output and slot is not None and slot.out is not None:
                    assert isinstance(slot.out, FileDescriptorProto)
                    try:
                        res_path.with_suffix(".pb").write_bytes(
                            slot.out.SerializeToString())
                    except (IOError, OSError, PermissionError, UnicodeEncodeError) as e:
                        cli_error(f"Failed to write {res_path.with_suffix('.pb')}: {type(e).__name__}: {e}")
                        cli_error("Cannot continue due to I/O error.")
                        sys.exit(1)

                # Accumulate for --build-schema-db
                if ctx.build_schema_db and slot is not None and slot.out is not None:
                    assert isinstance(slot.out, FileDescriptorProto)
                    ctx.schema_db_fdps.append(slot.out)
                    ctx.schema_db_fdp_origins.append(re_fdp.name)

                # Write .proto text (skipped in dry-run mode)
                if not ctx.dry_run and not suppress_disk_output:
                    try:
                        res_path.write_text(content)
                    except (IOError, OSError, PermissionError, UnicodeEncodeError) as e:
                        cli_error(f"Failed to write {res_path}: {type(e).__name__}: {e}")
                        cli_error("Cannot continue due to I/O error.")
                        sys.exit(1)

            advance()

    # spec 0146: drop a minimal buf.yaml at the -O root so `buf lsp serve`
    # (used by protolens's Neovim integration, spec 0145) can resolve
    # imports rooted at this directory — buf ignores the LSP root_dir
    # entirely and otherwise only resolves relative to the opened file's
    # own directory. Skipped in dry-run mode; never overwrites an
    # existing buf.yaml (the user may have hand-customized it).
    if not ctx.dry_run:
        buf_yaml_path = out_repo / "buf.yaml"
        if not buf_yaml_path.exists():
            buf_yaml_path.write_text(
                "# Auto-generated by reproto — lets `buf lsp serve` (used by\n"
                "# protolens's Neovim integration, spec 0145) resolve imports\n"
                "# rooted at this directory. Safe to edit or delete.\n"
                "version: v2\n"
                "modules:\n"
                "  - path: .\n"
            )


def _phase_build_schema_db(ctx: 'Context', db_path: Path) -> None:
    """Build the full schema DB at db_path (spec 0056, 0068).

    Produces:
      db_path                      — FileDescriptorSet of all loaded FDPs (.desc)
      db_path.stem/hopcroft.rkyv   — compiled (baked) Hopcroft scoring graph
      db_path.stem/index.rkyv      — lazy-loading FDS index (spec 0068)

    YAML content is generated in-memory from the same logic as
    _phase_emit_scoring_graphs; no intermediate files are written to disk.
    """
    from .lib.console import progress as _progress, progress_lazy as _progress_lazy
    import yaml
    from google.protobuf.descriptor_pb2 import FileDescriptorSet

    # ── 1. Render binary FDPs for WKT/fallback dependencies (spec 0080) ──────
    #
    # ctx.schema_db_extra_nodes was populated by phase 6 sub-pass 3.  These
    # nodes are transitive dependencies of summoned files but were not rendered
    # by phase 7 (they were not summoned).  Promote them now, render their
    # binary FDPs, and append to ctx.schema_db_fdps so the DB is self-contained.
    # Phase 7 is complete at this point so promoting is_summoned here is safe.
    # Runs before the scoring-graph collection below so hopcroft.rkyv's file
    # coverage matches .desc's exactly, WKT promotion included (spec 0166 G2).
    from .context import DescOut as _DescOut
    from .syntax import fdp_syntax as _fdp_syntax

    def _promote_subtree(node: Node) -> None:
        """Promote node and all contains-descendants to summoned+reachable."""
        stack: list[Node] = [node]
        while stack:
            n = stack.pop()
            n.is_summoned = True
            n.is_reachable = True
            for child in n.contains:
                stack.append(child)

    with _progress_lazy('Rendering WKT proto files', quiet=ctx.quiet) as (advance, _):
        for extra_node in ctx.schema_db_extra_nodes:
            advance()
            _promote_subtree(extra_node)
            saved_force = ctx.force_proto2_for_editions
            if EDITIONS_COMPAT_REQUIRED and _fdp_syntax(extra_node.this) == "editions":
                ctx.force_proto2_for_editions = True
            slot = _DescOut()
            ctx.out_desc = slot
            try:
                extra_node.render(ctx)
            except (KeyError, ValueError, TypeError, AttributeError):
                pass
            finally:
                ctx.out_desc = None
                ctx.force_proto2_for_editions = saved_force
            if slot.out is not None:
                assert isinstance(slot.out, FileDescriptorProto)
                ctx.schema_db_fdps.append(slot.out)
                ctx.schema_db_fdp_origins.append(extra_node.name)

    # ── 2. Collect per-file scoring-graph YAML strings (shares the collector
    #      with _phase_emit_scoring_graphs; see _collect_scoring_messages)

    scoring_graphs: list[str] = []

    scoring_files = list(dict.fromkeys(ctx.schema_db_fdp_origins))
    total_scoring = 0 if ctx.quiet else len(scoring_files)
    with _progress('Collecting scoring data', total_scoring, quiet=ctx.quiet) as advance:
        for proto_name in scoring_files:
            try:
                fd = ctx.pool.FindFileByName(proto_name)
            except (KeyError, TypeError) as e:
                from .lib.warnings import get_collector
                get_collector().w6(proto_name, "schema db", str(e))
                advance()
                continue

            group_fqdns = _collect_group_fqdns(fd)
            messages: dict = {}
            entries: list[str] = []
            for msg_desc in fd.message_types_by_name.values():
                _collect_scoring_messages(
                    ctx, msg_desc, messages, group_fqdns, entries,
                    lambda n: _canonical_scoring_name(ctx, n),
                )
            entries.sort()

            scoring_graphs.append(
                str(yaml.dump(_scoring_yaml_doc(ctx, entries, messages),
                              sort_keys=False, allow_unicode=True))
            )
            advance()

    if not scoring_graphs:
        from .lib.warnings import get_collector
        get_collector().w6('--build-schema-db', 'schema db', 'no scoring graphs generated; skipping')
        return

    # ── 3. Build the baked graph via the prototext_graph_lib PyO3 extension

    try:
        from prototext_graph_lib import build_graph
    except ImportError as e:
        raise RuntimeError(
            f'--build-schema-db requires the prototext_graph_lib extension: {e}'
        ) from e

    last_current = 0
    total_set = False

    def _on_progress(current: int, total: int) -> None:
        nonlocal last_current, total_set
        if not total_set:
            set_total(total)
            total_set = True
        advance(current - last_current)
        last_current = current

    want_pyvis = ctx.emit_scoring_html is not None
    with _progress_lazy('Compiling scoring graph', quiet=ctx.quiet) as (advance, set_total):
        baked_graph, compiled_yaml, initial_yaml = build_graph(
            scoring_graphs=scoring_graphs,
            emit_yaml=want_pyvis,
            emit_initial_yaml=want_pyvis,
            on_progress=_on_progress,
        )

    # ── 4. Assemble schemas.pb from FDPs collected by _phase7_output (spec 0076 §7)
    #
    # ctx.schema_db_fdps was populated during phase 7's render loop and step 1
    # above.  The loop iterates ctx.nodes in insertion order, which is not
    # guaranteed topological.  pool.Add() requires dependencies before
    # dependents, so sort first.

    # spec 0158: detect two entries that canonize to the same schema-db
    # name but carry different content (e.g. an import_rewrites rule
    # colliding a rewritten file's name with an already-canonical file of
    # the same name) and abort loudly instead of silently keeping
    # whichever entry happens to appear last.
    seen: dict[str, tuple[str, FileDescriptorProto]] = {}
    for origin, fdp in zip(ctx.schema_db_fdp_origins, ctx.schema_db_fdps):
        prior = seen.get(fdp.name)
        if prior is not None:
            prior_origin, prior_fdp = prior
            if prior_fdp.SerializeToString() != fdp.SerializeToString():
                cli_error(
                    f"error: schema-db name collision: '{prior_origin}' and "
                    f"'{origin}' both canonize to '{fdp.name}' with "
                    "different content — cannot build a valid "
                    "FileDescriptorSet. Use -p/--prune to remove one of "
                    "the colliding sources."
                )
                sys.exit(1)
            continue
        seen[fdp.name] = (origin, fdp)
    fdp_by_name = {name: fdp for name, (_origin, fdp) in seen.items()}
    # Kahn's algorithm: process files whose deps are already placed.
    in_set: set[str] = set()
    sorted_fdps = []
    remaining = list(ctx.schema_db_fdps)
    # Limit iterations to avoid infinite loop on cycles (shouldn't happen).
    for _ in range(len(remaining) + 1):
        if not remaining:
            break
        next_remaining = []
        for fdp in remaining:
            deps_ok = all(
                d not in fdp_by_name or d in in_set
                for d in fdp.dependency
            )
            if deps_ok:
                sorted_fdps.append(fdp)
                in_set.add(fdp.name)
            else:
                next_remaining.append(fdp)
        remaining = next_remaining
    # Append any unresolved (shouldn't happen in a well-formed FDS).
    sorted_fdps.extend(remaining)

    fds = FileDescriptorSet()
    fds.file.extend(sorted_fdps)

    # ── 5. Write both outputs
    #
    # db_path           → FileDescriptorSet (.desc)
    # db_path.stem/     → sibling directory
    #   hopcroft.rkyv   → compiled Hopcroft scoring graph

    with _progress_lazy('Writing schema DB', quiet=ctx.quiet) as (advance, _):
        raw_pb_bytes = fds.SerializeToString()
        advance()
        db_path.parent.mkdir(parents=True, exist_ok=True)
        db_path.write_bytes(raw_pb_bytes)
        advance()

        schema_db_dir = db_path.with_suffix('')
        schema_db_dir.mkdir(parents=True, exist_ok=True)
        (schema_db_dir / 'hopcroft.rkyv').write_bytes(baked_graph)
        advance()

        # ── 6. Build and write index.rkyv (spec 0068)
        from .build_index import write_fds_index
        write_fds_index(raw_pb_bytes, fds, schema_db_dir / 'index.rkyv')
        advance()

    if not ctx.quiet:
        eprintln = __import__('sys').stderr.write
        eprintln(f'  descriptor: {db_path}\n')
        eprintln(f'  graph:      {schema_db_dir / "hopcroft.rkyv"}\n')
        eprintln(f'  index:      {schema_db_dir / "index.rkyv"}\n')

    # ── 7. Write pyvis HTML visualisations (spec 0083) ────────────────────────
    if want_pyvis and initial_yaml is not None and compiled_yaml is not None:
        from .show import render_scoring_graph
        pyvis_path = ctx.emit_scoring_html
        assert pyvis_path is not None
        # Strip trailing .html to compute stem, then build the two output paths.
        p = str(pyvis_path)
        stem = p[:-5] if p.endswith('.html') else p
        raw_path = Path(stem + '.html')
        hop_path = Path(stem + '-hopcroft.html')
        render_scoring_graph(
            compiled_yaml=initial_yaml,
            output_path=raw_path,
            title='Scoring graph (raw)',
            with_leaf_nodes=ctx.with_leaf_nodes,
            hide=ctx.pyvis_hide,
        )
        render_scoring_graph(
            compiled_yaml=compiled_yaml,
            output_path=hop_path,
            title='Scoring graph (Hopcroft minimised)',
            with_leaf_nodes=ctx.with_leaf_nodes,
            hide=ctx.pyvis_hide,
        )
        if not ctx.quiet:
            eprintln = __import__('sys').stderr.write
            eprintln(f'  pyvis raw:       {raw_path}\n')
            eprintln(f'  pyvis hopcroft:  {hop_path}\n')


def _canonical_scoring_name(ctx: 'Context', full_name: str) -> str:
    """Rewrite a scoring-graph node/child FQDN through the active variant's
    namespace_rewrites rules, mirroring the same rewrite .desc's own
    type_name/extendee fields already receive at render time (spec 0159)
    — keeps hopcroft.rkyv's names in lockstep with .desc's names (spec
    0166).
    """
    if ctx.keep_variant_descriptor:
        return full_name
    from .mappings import apply_variant_namespace
    from .fake_types import Ref as _Ref
    return str(apply_variant_namespace(ctx, _Ref(f'.{full_name}'))).lstrip('.')


def _collect_group_fqdns(fd: Any) -> 'set[str]':
    """Return the set of FQDNs that are group message types in fd (spec 0058)."""
    from google.protobuf.descriptor import FieldDescriptor as FD

    group_fqdns: set[str] = set()

    def _scan(msg: Any) -> None:
        for f in msg.fields_by_number.values():
            if f.type == FD.TYPE_GROUP:
                group_fqdns.add(f.message_type.full_name)
        for nested in msg.nested_types:
            _scan(nested)

    for msg_desc in fd.message_types_by_name.values():
        _scan(msg_desc)
    return group_fqdns


def _synthesize_message_set_item(
    desc: Any, messages: dict, fields_out: list, full_name: str,
) -> None:
    """Synthesize the protocol-fixed `Item` group for a MessageSet type (spec 0108).

    A message with `message_set_wire_format=true` has no declared fields
    (descriptor.proto invariant): its wire payload is `repeated group Item = 1
    { optional int32 type_id = 2; optional bytes message = 3; }`.  Declaring
    `Item` as an ordinary GROUP node with ordinary fields gives real match
    credit for `type_id`/`message` instead of falling to blind-group-skip
    unknowns, and lets custom-named MessageSet types be recognized
    structurally (no FQDN heuristic needed).

    full_name is the caller's already-renamed name for desc, used to build the
    synthesized Item's FQDN so that it lands in the same (possibly
    variant-namespace-rewritten) namespace as the node's own entry (spec 0166).
    """
    if not desc.GetOptions().message_set_wire_format or fields_out:
        return
    item_fqdn = f'{full_name}.Item'
    messages[item_fqdn] = {
        'kind': 'GROUP',
        'fields': [
            {'number': 2, 'type': 'int32'},
            {'number': 3, 'type': 'bytes'},
        ],
    }
    fields_out.append({
        'number': 1, 'type': 'message', 'child': item_fqdn, 'label': 'repeated',
    })


def _field_label(field: Any) -> str:
    """Return 'required', 'repeated', or 'optional' for a FieldDescriptor (spec 0045)."""
    from google.protobuf.descriptor import FieldDescriptor as FD
    label = field.label
    if label == FD.LABEL_REQUIRED:
        return 'required'
    if label == FD.LABEL_REPEATED:
        return 'repeated'
    return 'optional'


def _scoring_kind(field: Any) -> 'tuple[str, str | None, tuple[int, int] | None]':
    """Map a FieldDescriptor to a (type_str, child_fqdn, range) tuple (spec 0077 §5).

    `field.is_packed` is deliberately not consulted (spec 0175). Every repeated
    scalar has two legal wire encodings — packed and expanded — and a reader
    must accept both whatever the option says, so the option carries no
    information a scorer may act on. What the scorer does need is the element
    type, which the old `LEN_PACKED` collapse discarded.
    """
    from google.protobuf.descriptor import FieldDescriptor as FD
    TYPE = field.type
    if TYPE == FD.TYPE_MESSAGE:
        return 'message', field.message_type.full_name, None
    if TYPE == FD.TYPE_GROUP:
        return 'group', field.message_type.full_name, None
    if TYPE == FD.TYPE_STRING:
        return 'string', None, None
    if TYPE == FD.TYPE_BYTES:
        return 'bytes', None, None
    if TYPE in (FD.TYPE_DOUBLE, FD.TYPE_FIXED64, FD.TYPE_SFIXED64):
        return 'double', None, None
    if TYPE in (FD.TYPE_FLOAT, FD.TYPE_FIXED32, FD.TYPE_SFIXED32):
        return 'float', None, None
    if TYPE == FD.TYPE_BOOL:
        return 'bool', None, (0, 1)
    if TYPE == FD.TYPE_ENUM:
        # An *open* enum has no range: every 32-bit value is legal, is
        # preserved on round-trip, and is exactly what a newer sender emits to
        # an older reader (spec 0176). `int32` states precisely that, and is
        # what an enum is on the wire — a 32-bit two's-complement varint, with
        # negatives sign-extended to ten bytes. `is_closed` resolves the
        # edition feature `features.enum_type`, so it is right for proto2,
        # proto3 and editions alike; a test on the declaring file's `syntax`
        # would silently get editions wrong.  It is a property, not a method.
        if not field.enum_type.is_closed:
            return 'int32', None, None
        values = list(field.enum_type.values_by_number.keys())
        return 'enum', None, (min(values), max(values))
    if TYPE == FD.TYPE_INT32:
        return 'int32', None, None
    if TYPE in (FD.TYPE_UINT32, FD.TYPE_SINT32):
        return 'uint32', None, None
    if TYPE in (FD.TYPE_INT64, FD.TYPE_UINT64, FD.TYPE_SINT64):
        return 'uint64', None, None
    raise ValueError(f'Unknown field type: {TYPE}')


def _scoring_yaml_doc(ctx: 'Context', entries: list[str], messages: dict) -> dict:
    """Assemble one scoring-graph YAML document (spec 0045 format).

    The `extension_ranges: true` marker (spec 0238 S2) declares that this file
    was produced by a run that was *asked* for extension ranges, which is what
    makes the absence of an `ext_ranges` key on a message mean "closed" rather
    than "not recorded".  Without the marker the SCAN policy would read a graph
    built by an ordinary run as one in which every message is closed, and
    terminate on the first custom option of every descriptor.

    Emitted only when the flag is set, so the document is byte-identical to
    the pre-0238 one otherwise.
    """
    doc: dict = {}
    if ctx.emit_extension_ranges:
        doc['extension_ranges'] = True
    doc['entries'] = entries
    doc['messages'] = messages
    return doc


def _canonical_extension_ranges(desc: Any) -> 'list[list[int]]':
    """Return `desc`'s extension ranges in canonical form (spec 0238 S3).

    The canonical form is the unique minimal list of maximal, disjoint,
    **non-adjacent**, ascending intervals, inclusive at both ends.  Uniqueness
    is the whole requirement: the compiled graph interns range sets and uses
    the intern index as the equality test, so two messages admitting exactly
    the same extension field numbers must reach the same index however their
    `.proto` spelled it.

    Three things make that non-trivial:

    - `extension_ranges` is **half-open** — `extensions 1000 to max` arrives as
      `(1000, 536870912)` and `extensions 10000` as `(10000, 10001)`.
    - Clauses need not be sorted or disjoint.
    - Adjacent clauses must merge.  This is not hypothetical: `FeatureSet`
      declares `1000 to 9994`, `9995 to 9999`, `10000`, which touch and so
      canonicalize to the single `[1000, 10000]` that one
      `extensions 1000 to 10000` clause would have given.

    `max` needs no handling here: protoc has already materialized it as the
    concrete 536870912 by the time reproto sees the descriptor.
    """
    spans = sorted((s, e - 1) for s, e in desc.extension_ranges if e > s)
    merged: list[list[int]] = []
    for start, end in spans:
        if merged and start <= merged[-1][1] + 1:
            merged[-1][1] = max(merged[-1][1], end)
        else:
            merged.append([start, end])
    return merged


def _collect_scoring_messages(
    ctx: 'Context',
    desc: Any,
    messages: dict,
    group_fqdns: 'set[str]',
    entries: list[str],
    rename: 'Callable[[str], str]',
) -> None:
    """Collect `desc` and its nested types into a scoring-graph YAML mapping.

    Shared by `_phase_emit_scoring_graphs` and `_phase_build_schema_db`, which
    differ only in `rename` — applied to every *emitted* name.  The emit path
    passes the identity; the schema-db path passes `_canonical_scoring_name`,
    so that `hopcroft.rkyv`'s names stay in lockstep with `.desc`'s own
    rewritten names (spec 0159, spec 0166 G1).

    Renaming is confined to emitted names deliberately: the pruning lookups
    (`ctx.nodes.get`) and `node_kind`'s `group_fqdns` membership test stay
    keyed on the original, pre-rewrite FQDN.

    Known transitions include both the message's own declared fields and every
    extension targeting it elsewhere (`extend desc { ... }`, e.g. FieldOptions
    extensions) — the compiled scoring graph must recognize extension field
    numbers as matches, not as unknowns (2026-07-25 fix).  MessageSet
    extensions are the one exception: `message_set_wire_format` messages never
    carry their extensions' own field numbers on the wire (everything is
    wrapped in the synthesized `Item` group), so including them here would
    wrongly claim those field numbers as direct transitions.
    """
    msg_node = ctx.nodes.get(Fqdn(f'desc:.{desc.full_name}'))
    if msg_node is not None and msg_node.is_pruned:
        return
    fields_out: list[dict] = []
    known_fields = list(desc.fields_by_number.values())
    if not desc.GetOptions().message_set_wire_format:
        known_fields += list(ctx.pool.FindAllExtensions(desc))
    for f in sorted(known_fields, key=lambda f: f.number):
        field_node = ctx.nodes.get(Fqdn(f'fdsc:.{f.full_name}'))
        if field_node is not None and field_node.is_pruned:
            continue
        type_str, child, range_ = _scoring_kind(f)
        entry: dict = {'number': f.number, 'type': type_str}
        if child is not None:
            entry['child'] = rename(child)
        if range_ is not None:
            entry['range'] = list(range_)
        label = _field_label(f)
        if label != 'optional':
            entry['label'] = label
        fields_out.append(entry)
    name = rename(desc.full_name)
    # Must precede `messages[name]`: `yaml.dump(sort_keys=False)` makes the
    # insertion order of `messages` the emitted order.
    _synthesize_message_set_item(desc, messages, fields_out, name)
    body: dict = {
        'kind': 'GROUP' if desc.full_name in group_fqdns else 'LENDEL',
        'fields': fields_out,
    }
    if ctx.emit_extension_ranges:
        ext_ranges = _canonical_extension_ranges(desc)
        if ext_ranges:
            body['ext_ranges'] = ext_ranges
    messages[name] = body
    entries.append(name)
    for nested in desc.nested_types:
        _collect_scoring_messages(ctx, nested, messages, group_fqdns, entries, rename)


def _phase_emit_scoring_graphs(ctx: 'Context', out_dir: Path) -> None:
    """Emit one scoring-graph YAML file per FileDescriptorProto (spec 0045)."""
    import yaml

    from .mappings import canonize_dependency
    for re_file in ctx.nodes.values():
        if not isinstance(re_file, ReFileDescriptorProto):
            continue
        if not re_file.is_present():
            continue
        if not re_file.is_summoned:
            continue
        proto_name = re_file.name
        canonical_name = canonize_dependency(ctx, proto_name)

        try:
            fd = ctx.pool.FindFileByName(proto_name)
        except KeyError:
            from .lib.warnings import get_collector
            get_collector().w6(proto_name, "scoring graph", "not in descriptor pool")
            continue
        except TypeError as e:
            from .lib.warnings import get_collector
            get_collector().w6(proto_name, "scoring graph", str(e))
            continue

        group_fqdns = _collect_group_fqdns(fd)
        messages: dict = {}
        entries: list[str] = []
        for msg_desc in fd.message_types_by_name.values():
            # Identity rename: this path emits unrewritten FQDNs.
            _collect_scoring_messages(
                ctx, msg_desc, messages, group_fqdns, entries, lambda n: n,
            )
        entries.sort()

        yaml_path = out_dir / Path(canonical_name).with_suffix('.yaml')
        yaml_path.parent.mkdir(parents=True, exist_ok=True)
        with open(yaml_path, 'w', encoding='utf-8') as fh:
            yaml.dump(_scoring_yaml_doc(ctx, entries, messages), fh,
                      sort_keys=False, allow_unicode=True)
