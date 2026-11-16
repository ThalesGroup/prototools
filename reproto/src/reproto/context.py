# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

from __future__ import annotations

import importlib.resources
from dataclasses import dataclass, field
from importlib.resources.abc import Traversable
from pathlib import Path
from types import CodeType
from typing import TYPE_CHECKING

from google.protobuf.descriptor import Descriptor
from google.protobuf.descriptor_database import DescriptorDatabase
from google.protobuf.descriptor_pool import DescriptorPool
from google.protobuf.message import Message

from .fake_types import Fqdn

if TYPE_CHECKING:
    from collections.abc import Callable
    from typing import Any

    from google.protobuf.descriptor_pb2 import FileDescriptorProto

    from .base import NodeBase
    from .feature_resolution import EditionDefaultTable
    from .load import PathPatterns
    from .names import SymbolTable
    from .re_file import ReFileDescriptorProto
    from .text import BlockLine

    # Public alias: any NodeBase subclass.  Replaces the former exhaustive union
    # of Re* types; NodeBase[Any] is both honest and forward-compatible.
    NodeTypes = NodeBase[Any]


@dataclass
class DescOut:
    """Typed output slot for binary descriptor rendering (spec 0076).

    Passed via ctx.out_desc to render() calls to receive the binary descriptor
    counterpart alongside the text output, without changing render() signatures.
    out is None if the node produced no binary descriptor.
    """
    out: Message | None = None


@dataclass
class SourceCodeInfoOut:
    """Pending SourceCodeInfo markers synthesized during rendering (spec 0141).

    Set via ctx.out_sci (internally, by ReFileDescriptorProto.render()) while
    the first (text) rendering pass is in progress. Each entry in `pending` is
    a (path, open_line, close_line) tuple: `path` is the SourceCodeInfo.Location
    path for the covered node, and `open_line`/`close_line` are the actual
    BlockLine objects bracketing its rendered text (identity-matched, not
    positional — see source_info.resolve_source_code_info_locations()).
    """
    pending: list[tuple[list[int], 'BlockLine', 'BlockLine']] = field(
        default_factory=list
    )


def _default_variant_root() -> Traversable:
    return importlib.resources.files('reproto.variants')


def _default_variant_orphans() -> dict[str, list[str]]:
    return {
        'EnumOptions':            ['features', 'feature_support'],
        'EnumValueOptions':       ['features', 'feature_support'],
        'ExtensionRangeOptions':  ['features', 'feature_support', 'verification'],
        'FieldOptions':           ['features', 'feature_support'],
        'FileOptions':            ['features', 'feature_support'],
        'MessageOptions':         ['features', 'feature_support', 'map_entry'],
        'MethodOptions':          ['features', 'feature_support'],
        'ServiceOptions':         ['features', 'feature_support'],
    }


@dataclass
class Options:
    binary: bool = False
    debug: bool = False
    debug_fqdn: bool = False
    descriptor_proto: str = ""
    dry_run: bool = False
    build_schema_db: Path | None = None
    emit_scoring_yaml: bool = False
    # Extension ranges in the scoring graph (spec 0238 S2). Opt-in because a
    # graph built without it declares every message closed, which the SCAN
    # policy must be able to tell apart from "every message really is closed".
    emit_extension_ranges: bool = False
    fallback_protos: list[str] = field(default_factory=list)
    go_root: str | None = None
    emit_scoring_html: Path | None = None
    with_leaf_nodes: bool = False
    pyvis_hide: tuple[str, ...] = field(default_factory=tuple)
    keep_variant_descriptor: bool = False
    quiet: bool = False
    redact_comments: bool = False
    redact_orphans: bool = False
    # Synthesized SourceCodeInfo (spec 0141): when True (default), binary
    # descriptor output (-b/--emit-binary, --schema-db-out) carries a
    # SourceCodeInfo synthesized from reproto's own render pass instead of
    # an empty one.
    source_info: bool = True
    # Caller-supplied FDP patch hook (spec 0369). Compiled source; the
    # callable is resolved once at Context construction into
    # ctx.fdp_plugin_fn. fdp_plugin_legacy records that it came from the
    # deprecated --phase2-plugin, whose entry point takes (ctx, fdp).
    fdp_plugin: CodeType | None = None
    fdp_plugin_legacy: bool = False
    force_proto2_output: bool = False
    force_proto2_for_editions: bool = False
    # Variant fields (spec 0001) — populated at startup from the variant file.
    # Defaults below reflect the OSS google.protobuf variant.
    variant_descriptor_proto: str = 'google/protobuf/descriptor.proto'
    variant_well_known: dict[str, str] = field(default_factory=dict)
    variant_import_rules: list[dict] = field(default_factory=list)
    variant_ns_rules: list[dict] = field(default_factory=list)
    variant_orphans: dict[str, list[str]] = field(
        default_factory=_default_variant_orphans
    )
    # Resource root: Traversable pointing to the directory containing <stem>.yaml.
    # For the built-in variant this is importlib.resources.files('reproto.variants');
    # for external variants it is the parent Path of the YAML file.
    variant_root: Traversable = field(default_factory=_default_variant_root)
    variant_stem: str = 'google-protobuf'
    write_variant_descriptor: bool = True
    # Hidden diagnostic flag (spec 0025): when set to a proto file name, skip
    # rendering and dump resolved FeatureSet YAML for that file to stdout.
    dump_resolved_features: str = ""
    # Hidden diagnostic flag (spec 0428): when set to a path, write there
    # what phases 1 to 6 leave, and stop.
    dump_graph: str = ""
    # Duplicate-symbol pruning (spec 0041): when False (default), files that
    # introduce symbols already registered in the pool are pruned and a warning
    # is emitted.  Set to True to revert to legacy behaviour.
    keep_duplicates: bool = False
    # Warning mode (spec 0041): when True, each warning occurrence is printed
    # immediately; when False (default), W1/W4/W5 are buffered and squashed.
    detailed_warnings: bool = False

class Context(Options):
    def __init__(
        self,
        pruned_fqdns: set[Fqdn],
        pruned_paths: PathPatterns,
        path_seeds: PathPatterns,
        **opts_kwargs: Any,
    ):
        # Initialize Options fields
        super().__init__(**opts_kwargs)

        self.pruned_fqdns = pruned_fqdns
        self.pruned_paths = pruned_paths
        self.path_seeds = path_seeds
        # Populated by load.py as matching candidate files are discovered
        # (spec 0149 G4); consumed by reproto() after phase 1 to extend the
        # FQDN seed list passed to _phase5_reachability.
        self.path_seed_fqdns: set[Fqdn] = set()
        # The nodes phase 5 started from, for the graph dump (spec 0428 S6).
        self.seed_nodes: set[NodeBase[Any]] = set()

        # Per-file syntax state (updated by re_file.py at the start of each render)
        self.syntax: str = "proto2"         # input file syntax
        self.target_syntax: str = "proto2"  # output syntax
        self.current_file: str = ""         # name of the .proto file being rendered
        # Every loaded file's symbols, for the type names written (spec 0434),
        # built on first use by names.symbol_table.
        self.symbol_table: SymbolTable | None = None

        # Binary output side-channel (spec 0076). When set to a DescOut instance,
        # render() additionally populates out_desc.out with the binary descriptor
        # counterpart. None (the default) means text-only output.
        self.out_desc: DescOut | None = None

        # Synthesized SourceCodeInfo side-channel (spec 0141). Set internally
        # by ReFileDescriptorProto.render() at the start of its first (text)
        # pass, and reset to None before its second (binary) pass begins.
        # None outside of a file render() call, or when synthesis is disabled.
        self.out_sci: SourceCodeInfoOut | None = None

        # Caller's FDP patch hook, resolved from self.fdp_plugin by
        # _make_context (spec 0369 S2).  The None default is load-bearing:
        # Context is also built directly, bypassing _make_context, and
        # apply_fdp_plugin runs on every parse.
        # Returns object, not None: the contract says a plugin returns None,
        # but it is arbitrary user code and nothing verifies that.  We call
        # it and discard the result.
        self.fdp_plugin_fn: Callable[..., object] | None = None

        # FDPs collected by _phase7_output for --build-schema-db (spec 0076 §7).
        # Populated in topological order during the phase 7 render loop when
        # ctx.build_schema_db is set; read by _phase_build_schema_db.
        self.schema_db_fdps: list[FileDescriptorProto] = []

        # Original (pre-canonicalization) .name of each entry in
        # schema_db_fdps, same index correspondence — spec 0158, used only
        # to name both sides of a canonical-name collision in the error
        # message.
        self.schema_db_fdp_origins: list[str] = []

        # WKT/fallback nodes identified by phase 6 sub-pass 3 (spec 0080).
        # These are transitive dependencies of summoned files that were not
        # summoned themselves.  _phase_build_schema_db promotes and renders
        # them after phase 7 to complete the schema DB.
        self.schema_db_extra_nodes: list[Any] = []

        # Edition feature defaults extracted from the variant's descriptor.pb
        # at startup (spec 0025). Maps feature field name -> sorted list of
        # (edition_number, value_name) pairs.
        self.edition_defaults: EditionDefaultTable = {}

        # File names pruned by duplicate-symbol detection (spec 0053).
        # Populated by _prune_if_duplicate in phase 2; used by
        # _strip_pruned_dependencies to patch importer FDPs before pool_db.Add.
        self.pruned_file_names: set[str] = set()

        # Pool and dicts
        self.pool_db: DescriptorDatabase = DescriptorDatabase()
        self.pool: DescriptorPool = DescriptorPool(self.pool_db)
        # FDPs added to pool_db in insertion (topological) order (spec 0056).
        self.pool_db_fdps: list[FileDescriptorProto] = []
        self.nodes: dict[Fqdn, NodeBase[Any]] = {}
        self.new_nodes: dict[Fqdn, NodeBase[Any]] = {}
        self.files: dict[str, 'ReFileDescriptorProto'] = {}
        self.new_files: dict[str, 'ReFileDescriptorProto'] = {}

        # Built-in protobuf option descriptors and classes, of the variant's
        # descriptor.proto, in the package it declares (option_messages).
        self.options_package: str = 'google.protobuf'
        # The *Options a proto3 file may extend (syntax.descriptor_options_fqns).
        self.descriptor_options_fqns: frozenset[str] | None = None

        # EnumOptions
        self.eno_desc: Descriptor
        self.eno_cls: type[Message]
        # EnumValueOptions
        self.evo_desc: Descriptor
        self.evo_cls: type[Message]
        # FieldOptions
        self.fdo_desc: Descriptor
        self.fdo_cls: type[Message]
        # FileOptions
        self.fio_desc: Descriptor
        self.fio_cls: type[Message]
        # MessageOptions
        self.mso_desc: Descriptor
        self.mso_cls: type[Message]
        # MethodOptions
        self.meo_desc: Descriptor
        self.meo_cls: type[Message]
        # OneofOptions
        self.ooo_desc: Descriptor
        self.ooo_cls: type[Message]
        # ServiceOptions
        self.svo_desc: Descriptor
        self.svo_cls: type[Message]
        # ExtensionRangeOptions; None in a variant whose descriptor.proto
        # predates it (proto2).
        self.ero_desc: Descriptor | None
        self.ero_cls: type[Message] | None
        # The pool the option classes above come from: pool_db with maps as
        # the repeated messages they are on the wire (option_messages.py).
        self.options_pool: DescriptorPool
        # Files loaded into options_pool for their extensions.
        self.options_pool_files: set[str]

    @classmethod
    def from_options(
        cls,
        pruned_fqdns: set[Fqdn],
        pruned_paths: PathPatterns,
        path_seeds: PathPatterns,
        options: Options
    ) -> Context:
        return cls(pruned_fqdns, pruned_paths, path_seeds, **vars(options))
    
    def merge_nodes(self) -> None:
        self.nodes.update(self.new_nodes)
        self.new_nodes = {}

    def has_node(self, fqdn: Fqdn) -> bool:
        return fqdn in self.nodes or fqdn in self.new_nodes
    
    def find_node(self, fqdn: Fqdn) -> 'NodeBase[Any] | None':
        if fqdn in self.nodes:
            return self.nodes[fqdn]
        if fqdn in self.new_nodes:
            return self.new_nodes[fqdn]
        return None
    
    def merge_files(self) -> None:
        self.files.update(self.new_files)
        self.new_files = {}

    def has_file(self, fqdn: str) -> bool:
        return fqdn in self.files or fqdn in self.new_files
    
    def find_file(self, fqdn: str) -> 'ReFileDescriptorProto | None':
        if fqdn in self.files:
            return self.files[fqdn]
        if fqdn in self.new_files:
            return self.new_files[fqdn]
        return None


class PluginError(Exception):
    """A caller-supplied FDP plugin raised (spec 0369 S4).

    Wrapping matters: three of the four call sites sit inside handlers
    that would otherwise absorb the throw and blame the input file —
    the embedded-fallback load, the phase-2 DecodeError guard, and the
    pool_db.Add TypeError guard.
    """


def apply_fdp_plugin(ctx: Context, fdp: 'FileDescriptorProto') -> None:
    """Apply the caller's plugin to fdp, in place (spec 0369 S2).

    Called immediately after every FDP is parsed and before anything
    reads it, so that topology, import discovery and ranking all see the
    patched dependency list.  A no-op when no plugin is configured; the
    null check lives here rather than at each call site.
    """
    if ctx.fdp_plugin_fn is None:
        return
    try:
        if ctx.fdp_plugin_legacy:
            ctx.fdp_plugin_fn(ctx, fdp)
        else:
            ctx.fdp_plugin_fn(fdp)
    except Exception as e:
        raise PluginError(
            f"plugin raised {type(e).__name__} on '{fdp.name}': {e}"
        ) from e