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
from pathlib import Path

from reproto import Context, Fqdn, Options

from .load import QualFile
from .phases import (
    DescriptorProtoHasTargetsError,
    DescriptorProtoUnbuildableError,
    RewriteCollisionError,
    DescriptorProtoMissingError,
    DescriptorProtoUnresolvedError,
    # WellKnownTypeHasTargetsError removed by spec 0052
    _dump_resolved_features_yaml,
    _make_context,
    _phase1_load_files,
    _phase2_build_pool,
    _phase3_build_graph,
    _phase4_pruning,
    _phase5_reachability,
    _phase6_summoning,
    _phase7_output,
    _resolve_rewrite_collisions,
    _phase_build_schema_db,
    _phase_emit_scoring_graphs,
)
from .topology import Topology

__all__ = [
    "reproto",
    "matches_any_pattern",
    "DescriptorProtoMissingError",
    "DescriptorProtoUnresolvedError",
    "DescriptorProtoHasTargetsError",
    "DescriptorProtoUnbuildableError",
    "RewriteCollisionError",
    # "WellKnownTypeHasTargetsError",  # removed by spec 0052
]


def matches_any_pattern(fqdn: Fqdn, patterns: list[Fqdn]) -> bool:
    """Return True if fqdn matches any pattern (exact or glob) in patterns."""
    return any(fqdn == p or fnmatch.fnmatch(fqdn, p) for p in patterns)


def reproto(
        in_repo: list[Path],
        seed_paths: list[Path] | list[QualFile],
        seeds: list[Fqdn],
        prunings: list[Fqdn],
        out_repo: Path | None,
        options: Options | None = None,
        path_seeds: list[str] | None = None,
        path_prunings: list[str] | None = None,
) -> Context | None:
    """Reconstruct .proto files from descriptor sets."""
    import warnings as _warnings
    from .lib.warnings import get_collector

    ctx = _make_context(options, prunings, path_prunings, path_seeds)

    if ctx.emit_scoring_html is not None and ctx.build_schema_db is None:
        import sys
        sys.stderr.write('--emit-scoring-html requires --build-schema-db\n')
        sys.exit(1)

    topo = Topology()

    # Suppress RuntimeWarnings from descriptor_database about symbol conflicts.
    # _prune_if_duplicate() handles these proactively; the warnings are redundant.
    _warnings.filterwarnings(
        'ignore',
        message=r'.*already defined in file.*|.*Please remove the leading.*',
        category=RuntimeWarning,
        module=r'google\.protobuf\.descriptor_database',
    )

    from . import dump_graph
    recorder = dump_graph.Recorder() if ctx.dump_graph else None

    try:
        seed_files = _phase1_load_files(ctx, in_repo, seed_paths, topo)
        if not topo.files:
            return None

        _phase2_build_pool(ctx, topo, seed_files, prunings)
        _phase3_build_graph(ctx, topo)

        if ctx.dump_resolved_features:
            _dump_resolved_features_yaml(ctx, ctx.dump_resolved_features)
            return ctx

        _phase4_pruning(ctx, topo, prunings)
        # Seed-by-path (spec 0149 G4): path_seed_fqdns is populated by
        # load_from_path() during phase 1 for every FDP produced by a
        # physical candidate matching a path seed pattern. Sorted: a set of
        # strings iterates in an order PYTHONHASHSEED sets.
        _phase5_reachability(ctx, seeds + sorted(ctx.path_seed_fqdns), topo)
        _phase6_summoning(ctx)

        if recorder is not None:
            dump_graph.write(ctx, topo, recorder, Path(ctx.dump_graph))
            return ctx

        _resolve_rewrite_collisions(ctx)

        # Phase 7 runs whenever there is file output OR when --build-schema-db
        # is active (even without -O, it needs the fully-initialised render
        # context to accumulate ctx.schema_db_fdps).  When out_repo is None
        # (schema-DB-only mode, no -O given), force dry_run so phase 7 renders
        # FDPs into ctx.schema_db_fdps without writing any .proto files to disk.
        if out_repo is not None or ctx.build_schema_db is not None:
            saved_dry_run = ctx.dry_run
            if out_repo is None:
                ctx.dry_run = True
            _phase7_output(ctx, out_repo or Path('.'))
            ctx.dry_run = saved_dry_run
            if ctx.emit_scoring_yaml and out_repo is not None:
                _phase_emit_scoring_graphs(ctx, out_repo)

        if ctx.build_schema_db is not None:
            _phase_build_schema_db(ctx, ctx.build_schema_db)

    finally:
        get_collector().flush()

    return ctx
