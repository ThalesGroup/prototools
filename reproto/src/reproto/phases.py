# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Phase functions for the reproto reconstruction algorithm.

Each of the 7 algorithm phases is implemented here as a private function.
The public entry point reproto() in reproto.py calls these in sequence.
"""

from __future__ import annotations

import fnmatch
import importlib.resources
import itertools
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
from .load import QualFile, decapsulate, load_from_path
from .option_messages import create_option_message_classes
from .re_file import ReFileDescriptorProto
from .topology import File, ReFile, Topology

logger = logging.getLogger(__name__)


class DescriptorProtoMissingError(Exception):
    """Raised when 'descriptor.proto' is missing."""

class DescriptorProtoHasTargetsError(Exception):
    """Raised when 'descriptor.proto' has targets."""

class WellKnownTypeHasTargetsError(Exception):
    """Raised when a well-known type proto has targets (imports)."""


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


def _find_matching_nodes(
    pattern: Fqdn,
    ctx: Context,
) -> list[tuple[Fqdn, Node]]:
    """Return all (fqdn, node) pairs whose fqdn matches pattern (exact or glob)."""
    if '*' in pattern or '?' in pattern:
        return [
            (fqdn, node)
            for fqdn, node in ctx.nodes.items()
            if fnmatch.fnmatch(fqdn, pattern)
        ]
    node = ctx.find_node(pattern)
    return [(pattern, node)] if node is not None else []


def _fuzzy_suggest(pattern: str, ctx: Context) -> str | None:
    """Return the closest matching FQDN from ctx.nodes, or None if none is close enough."""
    best_match = None
    best_score = 85
    for node in ctx.nodes.values():
        score = fuzz.ratio(pattern, node.fqdn)
        if score > best_score:
            best_score = score
            best_match = node.fqdn
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


def _make_context(options: Options | None, prunings: list[Fqdn]) -> Context:
    if options is None:
        ctx = Context(set(prunings))
    else:
        ctx = Context.from_options(set(prunings), options)
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
    if not ctx.quiet:
        cli_info('Loading seed files')

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

        if not ctx.quiet:
            cli_info('Discovering and loading imported files')

        # Read imported paths (if not pruned)
        while topo.new_files:
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
    def load_embedded_proto_fallback(proto_name: str) -> bool:
        """
        Load a proto fallback from the variant's resource directory.

        Traverses ctx.variant_root / ctx.variant_stem / <pb_name> via the
        Traversable API, which works for both directory and zip/wheel installs.

        Args:
            proto_name: Proto file name (e.g., "google/protobuf/any.proto")

        Returns:
            True if loaded successfully, False otherwise
        """
        try:
            pb_name = proto_name[:-len('.proto')] + '.pb'

            node = ctx.variant_root
            for part in [ctx.variant_stem] + pb_name.split('/'):
                node = node.joinpath(part)
            data = node.read_bytes()

            fd = FileDescriptorSet()
            fd.ParseFromString(data)
            qual_file = QualFile(Path('internal'), Path(proto_name), data)
            qual_file.name = fd.file[0].name
            qual_file.desc = fd
            ReFile(topo, qual_file)

            cli_attention(f"Using embedded fallback: {proto_name}")
            return True
        except (ImportError, FileNotFoundError, DecodeError, IndexError, AttributeError) as e:
            if ctx.debug:
                cli_warning(f"Failed to load embedded fallback for {proto_name}: {e}")
            return False

    # Load requested fallbacks (well-known types and descriptor.proto)
    # These fallbacks replace any versions that might exist in the input files.
    # We keep them in the reconstructed files so protoc can compile them.
    for fallback_proto in ctx.fallback_protos:
        proto_fqdn = Fqdn(FILE + ':' + fallback_proto)
        ctx.pruned_fqdns.discard(proto_fqdn)  # safe remove (no KeyError)
        topo.files.pop(fallback_proto, None)  # safe remove (no KeyError)

        load_embedded_proto_fallback(fallback_proto)
        topo.merge_files()

        # Verify well-known types are leaf files (no imports)
        proto_file: ReFile = topo.files[fallback_proto]
        if proto_file.targets:
            cli_error(f"Unexpected imports in {fallback_proto} (well-known types must be leaf files)")
            raise WellKnownTypeHasTargetsError(f"{fallback_proto} has targets")

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
    return True


def _phase2_build_pool(
    ctx: Context,
    topo: Topology,
    seed_files: set[ReFile],
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

    if not ctx.quiet:
        cli_info('Sorting files topologically')
        cli_info('Merging file descriptors into pool')

    # --- Locate descriptor.proto ---------------------------------------------

    descriptor_proto: ReFile = topo.files[ctx.variant_descriptor_proto]
    if descriptor_proto.is_ref() or descriptor_proto.targets:
        cli_error("Unexpected targets in descriptor.proto, aborting")
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
                continue
            qf = n.qfile
            contents = qf.contents
            try:
                match contents:
                    case str():
                        # Text format
                        match qf.desc:
                            case FileDescriptorSet():
                                # - Update the pool of descriptors
                                f = text_format.Parse(
                                    decapsulate(contents),
                                    FileDescriptorSet(),
                                    allow_unknown_field=True,
                                    allow_unknown_extension=True,
                                    descriptor_pool=ctx.pool,
                                )
                                assert len(f.file) == 1
                                # - Update pool with static FileDescriptorProto
                                fdp = f.file[0]
                                phase2_plugin(ctx, fdp)
                                patch_go_package(ctx, fdp)
                                _strip_self_dependency(fdp)
                                if not ctx.keep_duplicates and _prune_if_duplicate(ctx, n, fdp):
                                    continue
                                ctx.pool_db.Add(fdp)
                            case FileDescriptorProto():
                                # - Update the pool of descriptors
                                fdp = text_format.Parse(
                                    decapsulate(contents),
                                    FileDescriptorProto(),
                                    allow_unknown_field=True,
                                    allow_unknown_extension=True,
                                    descriptor_pool=ctx.pool,
                                )
                                # - Update pool with static FileDescriptorProto
                                phase2_plugin(ctx, fdp)
                                patch_go_package(ctx, fdp)
                                _strip_self_dependency(fdp)
                                if not ctx.keep_duplicates and _prune_if_duplicate(ctx, n, fdp):
                                    continue
                                ctx.pool_db.Add(fdp)

                    case bytes():
                        # Binary format
                        match qf.desc:
                            case FileDescriptorSet():
                                fds = FileDescriptorSet()
                                fds.ParseFromString(contents)
                                assert len(fds.file) == 1
                                try:
                                    fdp = fds.file[0]
                                    phase2_plugin(ctx, fdp)
                                    patch_go_package(ctx, fdp)
                                    _strip_self_dependency(fdp)
                                    if not ctx.keep_duplicates and _prune_if_duplicate(ctx, n, fdp):
                                        continue
                                    ctx.pool_db.Add(fdp)
                                except TypeError as e:
                                    # NOTE: TypeError can occur when attempting to add a descriptor
                                    # that conflicts with an existing one in the pool (e.g., duplicate
                                    # symbol names or incompatible descriptor types).
                                    cli_warning("Could not add descriptor from '%s' to pool: %s", n.name, e)
                            case FileDescriptorProto():
                                fdp = FileDescriptorProto()
                                fdp.ParseFromString(contents)
                                try:
                                    phase2_plugin(ctx, fdp)
                                    patch_go_package(ctx, fdp)
                                    _strip_self_dependency(fdp)
                                    if not ctx.keep_duplicates and _prune_if_duplicate(ctx, n, fdp):
                                        continue
                                    ctx.pool_db.Add(fdp)
                                except TypeError as e:
                                    # NOTE: TypeError can occur when attempting to add a descriptor
                                    # that conflicts with an existing one in the pool (e.g., duplicate
                                    # symbol names or incompatible descriptor types).
                                    cli_warning("Could not add descriptor from '%s' to pool: %s", n.name, e)
                                # Since dyn_fds is a dynamic message,
                                # MergeFromString expects a **serialized
                                # dynamic FDS** So first we need to serialize
                                # `fds` into binary again
                                fds = FileDescriptorSet()
                                fds.file.extend([fdp])  # add the single FDP
                            case _:
                                assert False
            except message.DecodeError:
                cli_warning(f"Skipping unparseable file: {n.name}")
                n.is_pruned = True
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
    if not ctx.quiet:
        cli_info('File descriptor pool complete')

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
    if not ctx.quiet:
        cli_info('Building FQDN dependency graph')

    # Create ReFileDescriptorProto nodes for all loaded files
    for file in topo.files.values():
        if file.is_ref() or file.is_pruned:
            continue
        desc = file.qfile.desc
        match desc:
            case FileDescriptorSet():
                ReFileDescriptorProto(ctx, desc.file[0])
            case FileDescriptorProto():
                proto = ctx.pool_db.FindFileByName(desc.name)
                ReFileDescriptorProto(ctx, proto)
            case _:
                raise AssertionError(f"Unexpected desc type: {type(desc)}")

    ctx.merge_nodes()

    # Propagate topology-level is_pruned to context nodes.  Pruned files are
    # skipped above, but stubs for them are created lazily by _initialize_from_message
    # when non-pruned files list them as dependencies.  Without this pass those
    # stubs have is_pruned=False, which causes phase 5 to mark them reachable and
    # phase 6 to mark them summoned, leading to a crash in phase 7 when render
    # calls .name on an uninitialised node.
    for name, file in topo.files.items():
        if not file.is_pruned:
            continue
        fqdn = ReFileDescriptorProto.fqdn_from_ref(name)
        node = ctx.find_file(fqdn)
        if node is not None:
            node.is_pruned = True


def _phase4_pruning(ctx: Context, topo: Topology, prunings: list[Fqdn]) -> None:
    """Phase 4: Mark user-specified prunings and propagate to contained children."""
    if not ctx.quiet and prunings:
        cli_info('Processing exclusions')

    transitive_prunnings: set[Node] = set()
    current_prunings: set[Node] = set()

    # Start from user-specified prunings
    for pruning_pattern in prunings:  # the user-specified prunings
        matched_nodes = _find_matching_nodes(pruning_pattern, ctx)

        if not matched_nodes:
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
    if not ctx.quiet:
        cli_info('Computing reachability from seeds')

    transitive_reachables: set[Node] = set()
    current_reachables: set[Node] = set()

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

    while current_reachables:
        transitive_reachables.update(current_reachables)
        new_reachables: set[Node] = set()
        for node in current_reachables:
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
    if not ctx.quiet:
        cli_info('Marking containers of reachable nodes')

    # --- Sub-pass 1: seed summoning ------------------------------------------
    # Mark every reachable node as summoned, then propagate upward through the
    # parent relation until file nodes are reached.

    current_summoned: set[Node] = set()

    for node in ctx.nodes.values():
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
                if not isinstance(t, ReFileDescriptorProto):
                    result.add(t)
            for c in n.contains:
                stack.append(c)
        return result

    changed = True
    while changed:
        changed = False
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


def _phase7_output(ctx: Context, out_repo: Path) -> None:
    """Phase 7: Generate and write reconstructed .proto files."""
    if not ctx.quiet:
        cli_info('Writing reconstructed .proto files.')

    create_option_message_classes(ctx)

    for re_fdp in ctx.nodes.values():
        if not isinstance(re_fdp, ReFileDescriptorProto):
            continue
        if not re_fdp.is_present():
            continue
        if not re_fdp.is_summoned:
            continue

        if (re_fdp.name == ctx.variant_descriptor_proto
            and not ctx.write_variant_descriptor):
            cli_info(f"Skipping {re_fdp.name} "
                     f"(use --emit-descriptor to include)")
            continue
        path = Path(re_fdp.name)
        res_path = out_repo / path
        if ctx.debug:
            cli_info(f"  Writing: {res_path}")

        # Make sure all parent directories exist
        res_path.parent.mkdir(parents=True, exist_ok=True)

        # Write content to file
        if not ctx.dry_run:

            # If requested, output the FDP in binary form (.pb)
            if ctx.binary:
                # Get the FileDescriptor
                file_descriptor = ctx.pool.FindFileByName(re_fdp.name)
                # Convert to FileDescriptorProto
                fd_proto = FileDescriptorProto()
                file_descriptor.CopyToProto(fd_proto)
                content = fd_proto.SerializeToString()
                # Write to disk (I/O errors are FATAL)
                try:
                    res_path.with_suffix(".pb").write_bytes(content)
                except (IOError, OSError, PermissionError, UnicodeEncodeError) as e:
                    cli_error(f"Failed to write {res_path}: {type(e).__name__}: {e}")
                    cli_error("Cannot continue due to I/O error.")
                    sys.exit(1)

            # First: render (data anomalies warn, programming errors propagate)
            try:
                content = re_fdp.render(ctx)[0].flush(ctx)
            except (KeyError, ValueError, TypeError, AttributeError) as e:
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
                continue

            # Second: write (I/O errors are FATAL)
            try:
                res_path.write_text(content)
            except (IOError, OSError, PermissionError, UnicodeEncodeError) as e:
                cli_error(f"Failed to write {res_path}: {type(e).__name__}: {e}")
                cli_error("Cannot continue due to I/O error.")
                sys.exit(1)
