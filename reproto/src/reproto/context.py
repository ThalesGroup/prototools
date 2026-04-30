# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from types import CodeType
from typing import TYPE_CHECKING, TypeAlias

from google.protobuf.descriptor import Descriptor
from google.protobuf.descriptor_database import DescriptorDatabase
from google.protobuf.descriptor_pool import DescriptorPool
from google.protobuf.message import Message

from .fake_types import Fqdn

if TYPE_CHECKING:
    from .re_descriptor import ReDescriptorProto
    from .re_enum import ReEnumDescriptorProto
    from .re_field import ReFieldDescriptorProto
    from .re_file import ReFileDescriptorProto
    from .re_method import ReMethodDescriptorProto
    from .re_service import ReServiceDescriptorProto

    NodeTypes: TypeAlias = (
        ReDescriptorProto |
        ReEnumDescriptorProto |
        ReFieldDescriptorProto |
        ReFileDescriptorProto |
        ReMethodDescriptorProto |
        ReServiceDescriptorProto
    )


def _default_variant_orphans() -> dict[str, list[str]]:
    return {
        'EnumOptions':            ['features', 'feature_support'],
        'EnumValueOptions':       ['features', 'feature_support'],
        'ExtensionRangeOptions':  ['features', 'feature_support', 'verification'],
        'FieldOptions':           ['features', 'feature_support'],
        'FileOptions':            ['features', 'feature_support'],
        'MessageOptions':         ['features', 'feature_support', 'map_entry', 'message_set_wire_format'],
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
    fallback_protos: list[str] = field(default_factory=list)
    go_root: str | None = None
    graph: Path | None = None
    keep_variant_descriptor: bool = False
    quiet: bool = False
    redact_comments: bool = False
    redact_orphans: bool = False
    phase2_plugin: CodeType | None = None
    force_proto2_output: bool = False
    # Variant fields (spec 0001) — populated at startup from the variant file.
    # Defaults below reflect the OSS google.protobuf variant.
    variant_descriptor_proto: str = 'google/protobuf/descriptor.proto'
    variant_well_known: dict[str, str] = field(default_factory=dict)
    variant_import_rules: list[dict] = field(default_factory=list)
    variant_ns_rules: list[dict] = field(default_factory=list)
    variant_orphans: dict[str, list[str]] = field(
        default_factory=_default_variant_orphans
    )
    # Resource loading (§8b): None => built-in, path => external filesystem
    variant_file: str | None = None
    variant_stem: str = 'google-protobuf'
    # Modules to import at startup (spec 0018); default empty for OSS variant.
    variant_annotation_modules: list[str] = field(default_factory=list)
    write_variant_descriptor: bool = False

class Context(Options):
    def __init__(
        self,
        pruned_fqdns: set[Fqdn],
        **opts_kwargs,
    ):
        # Initialize Options fields
        super().__init__(**opts_kwargs)

        self.pruned_fqdns = pruned_fqdns

        # Per-file syntax state (updated by re_file.py at the start of each render)
        self.syntax: str = "proto2"         # input file syntax
        self.target_syntax: str = "proto2"  # output syntax

        # Pool and dicts
        self.pool_db: DescriptorDatabase = DescriptorDatabase()
        self.pool: DescriptorPool = DescriptorPool(self.pool_db)
        self.nodes: dict[Fqdn, NodeTypes] = {}
        self.new_nodes: dict[Fqdn, NodeTypes] = {}
        self.files: dict[str, 'ReFileDescriptorProto'] = {}
        self.new_files: dict[str, 'ReFileDescriptorProto'] = {}

        # Built-in protobuf option descriptors and classes

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

    @classmethod
    def from_options(
        cls,
        pruned_fqdns: set[Fqdn],
        options: Options
    ) -> Context:
        return cls(pruned_fqdns, **vars(options))
    
    def merge_nodes(self) -> None:
        self.nodes.update(self.new_nodes)
        self.new_nodes = dict()
    
    def has_node(self, fqdn: Fqdn) -> bool:
        return fqdn in self.nodes or fqdn in self.new_nodes
    
    def find_node(self, fqdn: Fqdn) -> NodeTypes | None:
        if fqdn in self.nodes:
            return self.nodes[fqdn]
        if fqdn in self.new_nodes:
            return self.new_nodes[fqdn]
        return None
    
    def merge_files(self) -> None:
        self.files.update(self.new_files)
        self.new_files = dict()
    
    def has_file(self, fqdn: str) -> bool:
        return fqdn in self.files or fqdn in self.new_files
    
    def find_file(self, fqdn: str) -> 'ReFileDescriptorProto | None':
        if fqdn in self.files:
            return self.files[fqdn]
        if fqdn in self.new_files:
            return self.new_files[fqdn]
        return None