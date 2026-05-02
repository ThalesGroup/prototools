# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""
Reproto: Reconstruct .proto files from compiled descriptor sets.

OVERVIEW
========
This module implements the core algorithm for regenerating
human-readable .proto files from binary/text FileDescriptorSets.
It performs dependency analysis, type resolution, and selective
pruning to produce minimal, correct .proto files.

ALGORITHM PHASES
================
The reconstruction process consists of 7 sequential phases:

Phase 1: FILE LOADING
---------------------
Load seed files and recursively discover imported files.
- Parse initial seed paths into QualFile objects
- Build topology graph of file dependencies
- Recursively load transitive imports (breadth-first)
- Merge files into unified topology

Phase 2: DESCRIPTOR POOL BUILDING
----------------------------------
Parse descriptors in dependency order and build type pool.
- Topological sort: process files leaves-first (no unresolved deps)
- Parse FileDescriptorProto from text or binary format
- Add to pool_db for lazy type lookup (supports circular deps)
- Detect and report circular dependencies

Phase 3: FQDN GRAPH CONSTRUCTION
---------------------------------
Build fully-qualified name registry with dependency relationships.
- Create ReFileDescriptorProto nodes for each file
- Populate ctx.nodes with all FQDNs (messages, enums, services)
- Establish graph edges: targets (references), contains (children),
  parent (container)

Phase 4: PRUNING
----------------
Mark excluded nodes and their transitive children.
- Process user-specified prunings (--prune flag)
- Transitive closure: prune all children via contains relation
- Pruned nodes are excluded from output and reachability analysis

Phase 5: REACHABILITY ANALYSIS
-------------------------------
Mark nodes transitively reachable from seeds (forward propagation).
- Identify seed nodes (explicit --seed flags or seed files)
- Propagate forward via targets relation: seed → referenced types
- is_reachable = "transitively needed by at least one seed"
- Tracks seeder for debugging (first node that reached each target)

Phase 6: SUMMONING
------------------
Mark container nodes of reachable nodes (backward propagation).
- Start from all reachable nodes
- Propagate backward via parent relation: child → parent message
- is_summoned = "container of at least one reachable node"
- Special case: files summoned if they target summoned nodes

Phase 7: OUTPUT
---------------
Generate and write reconstructed .proto files.
- Create dynamic option message classes (with extensions)
- For each file with summoned content:
  - Call re_fdp.render(ctx) to generate .proto text
  - Write to out_repo preserving directory structure
- Skip files with no summoned content (minimizes output)

KEY PRINCIPLES
==============

1. Three-State Node Marking
----------------------------
Each node has three independent boolean flags:
- is_pruned: User explicitly excluded this (--prune flag)
- is_reachable: Transitively needed by seeds (forward from seeds)
- is_summoned: Contains reachable content (backward to parents)

A node is written to output if: is_summoned AND NOT is_pruned
(Though typically pruned nodes cannot be summoned, except possibly
when a pruned file imports a summoned file)

2. Topological Processing
--------------------------
Files must be processed in dependency order (leaves-first):
- Leaves = files with no outgoing dependencies
- Process leaves → add to pool_db → remove from graph
- Repeat until no files remain (or circular dependency detected)
- Ensures all referenced types exist in pool before parsing

3. Bidirectional Graph Traversal
---------------------------------
Two complementary propagation passes:
- Forward (Phase 5): seeds → targets (what you reference)
  Example: Message field references another message type
- Backward (Phase 6): reachable → parents (what contains you)
  Example: Reachable field triggers parent message summoning

4. Descriptor Pool with Lazy Lookup
------------------------------------
pool_db provides lazy element lookup infrastructure that allows
adding FileDescriptorProtos with circular dependencies before
they are fully resolved. This is the underlying mechanism for pool.

5. Singleton Pattern for Nodes
-------------------------------
All Re*DescriptorProto objects are singletons identified by FQDN.
Node lifecycle:
- Created as reference: Node exists in ctx.nodes but _this is None
  (dangling reference to type not yet loaded)
- Fully instantiated: _this populated when file containing it
  is loaded (can now be used for rendering)
- Orphaned reference: Remains dangling if file never loaded
  (incomplete FDP set - rendered as comment)

ctx.nodes registry ensures each FQDN maps to exactly one object.

6. Lazy Import Loading
-----------------------
Files loaded on-demand during topology building:
- Start with seed files
- Discover imports from dependency field
- Load imported files recursively (BFS)
- Avoids loading unused files from repository

7. Format Agnostic
------------------
Handles multiple input formats transparently:
- Text format (.textproto) vs Binary (.pb)
- FileDescriptorSet vs FileDescriptorProto
- Auto-detects format and wraps in consistent interface

EXAMPLE FLOW
=============
Given: seed=foo.proto (references bar.Message)

Phase 1: Load foo.proto → discover import bar.proto → load bar.proto
Phase 2: Process bar.proto first (leaf), then foo.proto
         Add bar.Message to pool_db, then parse foo.proto
Phase 3: Build nodes: file:foo.proto, message:.Foo, message:.bar.Msg
         Edges: .Foo targets .bar.Message
Phase 4: (no prunings in this example)
Phase 5: Mark file:foo.proto reachable (seed)
         Propagate: .Foo reachable, .bar.Message reachable
Phase 6: Backward: .Foo summoned → file:foo.proto summoned
         .bar.Message summoned → file:bar.proto summoned
Phase 7: Write foo.proto and bar.proto to out_repo

DEBUGGING FLAGS
===============
--debug: Verbose logging of each phase
--debug-fqdn: Print all FQDNs with their states (S/P/R flags)
--graph: Generate visual dependency graph (.html file)
"""

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

from lib.warnings import cli_attention, cli_error, cli_info, cli_warning
from reproto import Context, Fqdn, Node, Options

from .fake_types import parse_fqdn
from .feature_resolution import ResolvedFeatures, build_edition_defaults
from .globals import FILE
from .load import QualFile, decapsulate, load_from_path
from .option_messages import create_option_message_classes
from .re_file import ReFileDescriptorProto
from .show import show_graph
from .topology import File, ReFile, Topology

logger = logging.getLogger(__name__)
logger.propagate = True  # default is True, usually fine


def matches_any_pattern(fqdn: Fqdn, patterns: list[Fqdn]) -> bool:
    """Return True if fqdn matches any pattern (exact or glob) in patterns."""
    return any(fqdn == p or fnmatch.fnmatch(fqdn, p) for p in patterns)


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
            cli_info(f"Module '{full_module_name}' imported successfully.")
        except ModuleNotFoundError:
            cli_warning(f"Module '{full_module_name}' not found.")

class DescriptorProtoMissingError(Exception):
    """Raised when 'descriptor.proto' is missing."""

class DescriptorProtoHasTargetsError(Exception):
    """Raised when 'descriptor.proto' has targets."""

class WellKnownTypeHasTargetsError(Exception):
    """Raised when a well-known type proto has targets (imports)."""

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


def reproto(
        in_repo: list[Path],
        seed_paths: list[Path] | list[QualFile],
        seeds: list[Fqdn],
        prunings: list[Fqdn],
        out_repo: Path,
        options: Options | None = None,
) -> Context | None:
    """
    Reconstruct .proto files from descriptor sets.

    Args:
        in_repo: Search paths for locating .proto/.pb files
        seed_paths: Initial files to load (paths or pre-loaded QualFiles)
        seeds: Explicit FQDNs to treat as roots (empty = use seed_paths)
        prunings: FQDNs to exclude from output
        out_repo: Output directory for reconstructed .proto files
        options: Optional configuration (logging, feature flags, etc.)

    The function executes 7 phases (see module docstring for details).
    """
    if options is None:
        ctx = Context(set(prunings))
    else:
        ctx = Context.from_options(set(prunings), options)
    import_annotations(
        ctx.variant_annotation_modules,
        str(ctx.variant_root.joinpath(ctx.variant_stem)),
    )

    # =========================================================================
    # PHASE 1: FILE LOADING
    # =========================================================================
    # Load seed files and recursively discover all imported dependencies.
    # Builds a topology graph where nodes are files and edges are imports.
    #
    # Key operations:
    # - Parse seed_paths into QualFile objects (qualified file metadata)
    # - Create ReFile nodes in topology for each file
    # - Mark seed files with is_seed flag
    # - Iteratively load files referenced in dependency fields (BFS)
    # - Handle special case: fallback descriptor.proto
    # =========================================================================

    if not ctx.quiet:
        cli_info('Phase 1: Loading seed files')

    topo = Topology()
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
            cli_info('Phase 1: Discovering and loading imported files')

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
                            cli_warning(f"Skipping unreadable file: {name}")
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
    
    initial_nodes = seeding_files
    
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

    # =========================================================================
    # PHASE 2: DESCRIPTOR POOL BUILDING
    # =========================================================================
    # Process files in topological order (leaves-first) and merge their
    # FileDescriptorProtos into the descriptor pool. This ensures all
    # referenced types are available before parsing dependent files.
    #
    # Algorithm:
    # 1. Find leaf files (no outgoing file dependencies)
    # 2. Parse each leaf's FileDescriptorProto (text or binary format)
    # 3. Add to pool_db for lazy type lookup (supports circular deps)
    # 4. Remove processed leaves from graph
    # 5. Repeat until no files remain
    #
    # Handles:
    # - Text format (textproto) via text_format.Parse
    # - Binary format (.pb) via ParseFromString
    # - Both FileDescriptorSet and FileDescriptorProto containers
    # - go_package option patching for Go stub generation
    #
    # Terminates when:
    # - All files processed (success), OR
    # - No leaves found but files remain (circular dependency error)
    # =========================================================================

    if not topo.files:
        return

    if not ctx.quiet:
        cli_info('Phase 2: Sorting files topologically')
        cli_info('Phase 2: Merging file descriptors into pool')

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
        # The remaining files have a circular dependency
        for n in non_leaves:
            cli_warning(f"Circular dependency detected: {n.name}")
    if not ctx.quiet:
        cli_info('Phase 2: File descriptor pool complete')

    # --- Build edition-default table from the variant's descriptor.pb --------
    # pool_db.FindFileByName returns the FileDescriptorProto directly.
    try:
        descriptor_fdp = ctx.pool_db.FindFileByName(ctx.variant_descriptor_proto)
        ctx.edition_defaults = build_edition_defaults(descriptor_fdp)
    except KeyError:
        ctx.edition_defaults = {}

    # =========================================================================
    # PHASE 3: FQDN GRAPH CONSTRUCTION
    # =========================================================================
    # Create ReFileDescriptorProto wrapper nodes for all files and populate
    # the ctx.nodes registry with all FQDNs (files, messages, enums, fields,
    # services, methods, etc.).
    #
    # The ReFileDescriptorProto constructor recursively creates child nodes
    # for all contained elements, establishing three key relationships:
    # - targets: nodes this node references (e.g., field → message type)
    # - contains: nodes directly owned by this node (e.g., message → fields)
    # - parent: the containing node (e.g., field → parent message)
    #
    # After this phase, ctx.nodes is a complete graph of all symbols with
    # their dependency relationships.
    #
    # Singleton property: Each FQDN maps to exactly one Re*DescriptorProto
    # object in ctx.nodes. Objects start as references (_this=None) and
    # become fully instantiated when their file is loaded.
    # =========================================================================

    if not ctx.quiet:
        cli_info('Phase 3: Building FQDN dependency graph')

    seed_re_fdp : set[ReFileDescriptorProto] = set()
    known_fqdns: dict[Fqdn, ReFileDescriptorProto] = dict()

    # Create ReFileDescriptorProto nodes for all loaded files
    for file in topo.files.values():
        if file.is_ref():
            continue
        desc = file.qfile.desc
        match desc:
            case FileDescriptorSet():
                re_fdp = ReFileDescriptorProto(ctx, desc.file[0])
            case FileDescriptorProto():
                proto = ctx.pool_db.FindFileByName(desc.name)
                re_fdp = ReFileDescriptorProto(ctx, proto)
            case _:
                raise AssertionError(f"Unexpected desc type: {type(desc)}")
        seed_re_fdp.add(re_fdp)
        fqdn = re_fdp.fqdn
        if fqdn not in known_fqdns:
            known_fqdns[fqdn] = re_fdp

    ctx.merge_nodes()

    # --dump-resolved-features diagnostic mode: emit YAML and return early.
    if ctx.dump_resolved_features:
        _dump_resolved_features_yaml(ctx, ctx.dump_resolved_features)
        return ctx

    # =========================================================================
    # PHASE 4: PRUNING (Transitive Exclusion)
    # =========================================================================
    # Mark user-specified prunings and propagate to all contained children.
    #
    # Algorithm:
    # 1. For each user-provided pruning FQDN:
    #    - Lookup node in ctx.nodes (with fuzzy match suggestions)
    #    - Mark node.is_pruned = True
    # 2. Transitively prune children:
    #    - For each pruned node, mark all node.contains as pruned
    #    - Repeat until no new prunings
    #
    # Effect:
    # - Pruned nodes are excluded from reachability analysis
    # - Pruned nodes are not written to output files
    #
    # Use case: Remove deprecated/internal APIs from output
    # =========================================================================

    if not ctx.quiet and prunings:
        cli_info('Phase 4: Processing exclusions')

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

    # =========================================================================
    # PHASE 5: REACHABILITY ANALYSIS (Forward Propagation)
    # =========================================================================
    # Mark all nodes transitively reachable from seeds by following the
    # targets relation (references).
    #
    # Algorithm:
    # 1. Identify seed nodes:
    #    - If explicit --seed flags: use those FQDNs
    #    - Otherwise: use all FQDNs from seed files
    # 2. Mark seeds as reachable
    # 3. Propagate forward via targets relation:
    #    - For each reachable node, mark all node.targets as reachable
    #    - Track seeder (first node that reached each target) for debugging
    #    - Repeat until no new reachable nodes
    #
    # Semantics:
    # - is_reachable = "transitively needed by at least one seed"
    # - Pruned nodes block propagation (not marked reachable)
    #
    # Example: seed message has field of type Foo → Foo becomes reachable
    # =========================================================================

    if not ctx.quiet:
        cli_info('Phase 5: Computing reachability from seeds')

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
            if hasattr(file, 'qfile') and file.is_seed:
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

    # =========================================================================
    # PHASE 6: SUMMONING (Backward Propagation)
    # =========================================================================
    # Mark all parent/container nodes of reachable nodes by following the
    # parent relation backward. This ensures we write complete definitions.
    #
    # Algorithm:
    # 1. Mark all reachable nodes as summoned (base case)
    # 2. Propagate backward via parent relation:
    #    - For each summoned node with a parent:
    #      Mark node.parent as summoned
    #    - For each summoned node without a parent (file-level):
    #      Check if any sibling file targets this node (imports)
    #      If so, mark that file as summoned
    # 3. Repeat until no new summoned nodes
    #
    # Semantics:
    # - is_summoned = "container of at least one reachable node"
    # - A file is summoned if it contains any summoned symbol OR imports
    #   a summoned file
    #
    # Example: If message field is reachable → parent message is summoned
    #          → parent file is summoned → file gets written to output
    # =========================================================================

    if not ctx.quiet:
        cli_info('Phase 6: Marking containers of reachable nodes')

    transitive_summoned: set[Node] = set()
    current_summoned: set[Node] = set()

    # Pre-identify sibling files
    sibling_files: set[Node] = set()
    for node in ctx.nodes.values():
        if (node.is_present()
            and node.parent is None
            and not node.is_pruned):
            sibling_files.add(node)

    # --- Start from visible nodes --------------------------------------------

    for node in ctx.nodes.values():
        if not node.is_reachable:
            continue
        node.is_summoned = True
        current_summoned.add(node)

    # --- Propagate backwards (parent relation) -------------------------------


    while current_summoned:
        transitive_summoned.update(current_summoned)
        new_summoned: set[Node] = set()
        for node in current_summoned:
            if node.parent is None:
                for sibling_file in sibling_files:
                    sibling_file: Node
                    if sibling_file.is_summoned:
                        continue
                    if node not in sibling_file.targets:
                        continue
                    sibling_file.is_summoned = True
                    new_summoned.add(sibling_file)
                continue
            if node.parent.is_summoned:
                continue
            node.parent.is_summoned = True
            new_summoned.add(node.parent)
        current_summoned = new_summoned

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

    # =========================================================================
    # PHASE 7: OUTPUT (Regenerate .proto Files)
    # =========================================================================
    # Generate and write reconstructed .proto files for all files containing
    # summoned content.
    #
    # Algorithm:
    # 1. Create dynamic option message classes (enables extension parsing)
    # 2. For each ReFileDescriptorProto in ctx.nodes:
    #    - Skip if not a file node
    #    - Skip if no summoned targets (empty file)
    #    - Skip if is_pruned
    #    - Special case: skip descriptor.proto unless flag set
    # 3. Call re_fdp.render(ctx) to generate .proto text:
    #    - Reconstructs syntax, package, imports, options
    #    - Recursively renders summoned messages, enums, services
    #    - Marks orphaned elements (not summoned) as comments
    # 4. Write to out_repo/path preserving directory structure
    #
    # Output format:
    # - Human-readable .proto syntax
    # - Preserves original formatting intent (comments for orphans)
    # - Includes only summoned content (minimizes output size)
    # =========================================================================

    if not ctx.quiet:
        cli_info('Writing reconstructed .proto files.')

    create_option_message_classes(ctx)

    for re_fdp in ctx.nodes.values():
        if not isinstance(re_fdp, ReFileDescriptorProto):
            continue
        if not re_fdp.is_present():
            continue
        if not any(n for n in re_fdp.targets if n.is_summoned):
            continue

        if (re_fdp.name == ctx.variant_descriptor_proto
            and not ctx.write_variant_descriptor):
            cli_info(f"  Skipping {re_fdp.name} "
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
                cli_warning(f"Failed to render {re_fdp.name}: {type(e).__name__}: {e}")
                if ctx.debug:
                    pass
                    cli_warning(str(re_fdp.this))
                continue

            # Second: write (I/O errors are FATAL)
            try:
                res_path.write_text(content)
            except (IOError, OSError, PermissionError, UnicodeEncodeError) as e:
                cli_error(f"Failed to write {res_path}: {type(e).__name__}: {e}")
                cli_error("Cannot continue due to I/O error.")
                sys.exit(1)

    # Generate visual graph for debugging if requested
    if ctx.graph is not None:
        # Produces a visual representation of the FQDN graph (HTML file)
        # showing nodes, their relationships, and their states
        show_graph(ctx, output_path=ctx.graph)

    return ctx
