# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Type names as protoc resolves them (spec 0434, invariant 0001).

A type reference is written in googleapis' convention (S4): a type of the
file's own package by the shortest suffix of its name within the package,
any other package's type by its full name without the leading dot; and,
when protoc would resolve that name to something else, with the leading
dot. Whether a name resolves to its target is decided by simulating
protoc's lookup (``DescriptorBuilder::LookupSymbolNoPlaceholder``,
protobuf's ``descriptor.cc``) over the symbols visible from the file.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import TYPE_CHECKING, Iterable

from google.protobuf.descriptor_pb2 import (
    DescriptorProto,
    EnumDescriptorProto,
    FileDescriptorProto,
)

if TYPE_CHECKING:
    from .context import Context


class Kind(Enum):
    """A symbol's kind, as far as protoc's lookup tells them apart."""
    PACKAGE = 'package'
    MESSAGE = 'message'
    ENUM = 'enum'
    SERVICE = 'service'
    OTHER = 'other'     # field, oneof, extension, enum value, method


_TYPES = (Kind.MESSAGE, Kind.ENUM)
_AGGREGATES = (Kind.MESSAGE, Kind.ENUM, Kind.SERVICE, Kind.PACKAGE)


class Mode(Enum):
    """protoc's ``ResolveMode``: a field's type is looked up among types
    only; an extendee and a method's types among every symbol."""
    TYPES = 'types'
    ALL = 'all'


@dataclass
class _Visible:
    """What protoc sees from one file (``FindSymbol``): the files it may
    read, and the packages they are in, with every parent package."""
    files: frozenset[str]
    packages: frozenset[str]


@dataclass
class SymbolTable:
    """Every symbol of the loaded files, by full name (no leading dot):
    its kind in each file defining it (S1)."""
    symbols: dict[str, dict[str, Kind]] = field(default_factory=dict)
    packages: dict[str, str] = field(default_factory=dict)
    imports: dict[str, tuple[list[str], list[str]]] = field(default_factory=dict)
    _visible: dict[str, _Visible] = field(default_factory=dict)

    @classmethod
    def build(cls, fdps: Iterable[FileDescriptorProto]) -> SymbolTable:
        table = cls()
        for fdp in fdps:
            table._add_file(fdp)
        return table

    def _add(self, name: str, kind: Kind, file: str) -> None:
        self.symbols.setdefault(name, {}).setdefault(file, kind)

    def _add_file(self, fdp: FileDescriptorProto) -> None:
        file = fdp.name
        if file in self.packages:
            return
        self.packages[file] = fdp.package
        public = [fdp.dependency[i] for i in fdp.public_dependency
                  if i < len(fdp.dependency)]
        self.imports[file] = (list(fdp.dependency), public)
        prefix = fdp.package
        for m in fdp.message_type:
            self._add_message(prefix, m, file)
        for e in fdp.enum_type:
            self._add_enum(prefix, e, file)
        for x in fdp.extension:
            self._add(_join(prefix, x.name), Kind.OTHER, file)
        for s in fdp.service:
            name = _join(prefix, s.name)
            self._add(name, Kind.SERVICE, file)
            for meth in s.method:
                self._add(_join(name, meth.name), Kind.OTHER, file)

    def _add_message(self, prefix: str, m: DescriptorProto, file: str) -> None:
        name = _join(prefix, m.name)
        self._add(name, Kind.MESSAGE, file)
        for f in [*m.field, *m.extension]:
            self._add(_join(name, f.name), Kind.OTHER, file)
        for o in m.oneof_decl:
            self._add(_join(name, o.name), Kind.OTHER, file)
        for n in m.nested_type:
            self._add_message(name, n, file)
        for e in m.enum_type:
            self._add_enum(name, e, file)

    def _add_enum(self, prefix: str, e: EnumDescriptorProto, file: str) -> None:
        self._add(_join(prefix, e.name), Kind.ENUM, file)
        # An enum value is a symbol of the enum's own scope's parent.
        for v in e.value:
            self._add(_join(prefix, v.name), Kind.OTHER, file)

    def visible(self, file: str) -> _Visible:
        """S2: the file, its imports and their ``import public`` chains,
        and the packages these files are in, every parent included."""
        cached = self._visible.get(file)
        if cached is not None:
            return cached
        files = {file}

        def public_closure(dep: str) -> None:
            if dep in files:
                return
            files.add(dep)
            for p in self.imports.get(dep, ([], []))[1]:
                public_closure(p)

        for dep in self.imports.get(file, ([], []))[0]:
            public_closure(dep)
        packages: set[str] = set()
        for f in files:
            package = self.packages.get(f)
            if package:
                parts = package.split('.')
                for i in range(1, len(parts) + 1):
                    packages.add('.'.join(parts[:i]))
        visible = _Visible(frozenset(files), frozenset(packages))
        self._visible[file] = visible
        return visible

    def find(self, name: str, visible: _Visible) -> Kind | None:
        """``FindSymbol``: the symbol `name`, if `visible` sees it."""
        if name in visible.packages:
            return Kind.PACKAGE
        for f, kind in self.symbols.get(name, {}).items():
            if f in visible.files:
                return kind
        return None

    def resolve(self, name: str, scope: str, mode: Mode, visible: _Visible) -> str | None:
        """``LookupSymbolNoPlaceholder``: the full name protoc resolves
        `name` to from `scope`, the innermost scope enclosing the
        reference, or None."""
        if name.startswith('.'):
            return name[1:] if self.find(name[1:], visible) else None
        first = name.split('.', 1)[0]
        compound = first != name
        s = scope
        while s:
            candidate = f'{s}.{first}'
            kind = self.find(candidate, visible)
            if kind is not None:
                if compound:
                    if kind in _AGGREGATES:
                        full = f'{s}.{name}'
                        return full if self.find(full, visible) else None
                elif mode is Mode.ALL or kind in _TYPES:
                    return candidate
            s = s.rpartition('.')[0]
        return name if self.find(name, visible) else None

    def type_name(self, target: str, file: str, scope: str, mode: Mode) -> str:
        """S4: the name to write for `target` (a full name, with its
        leading dot) in `file`, from `scope`."""
        full = target[1:] if target.startswith('.') else target
        package = self.packages.get(file, '')
        defined_in = self.symbols.get(full)
        if defined_in:
            # The package of a file defining the target, not a prefix of its
            # name: a sub-package's type is another package's.
            own = any(self.packages.get(f) == package for f in defined_in)
        else:
            own = package == '' or full.startswith(f'{package}.')
        rel = full[len(package) + 1:] if own and package else full
        if not defined_in:
            # A target not loaded (a missing import, or a group the output
            # synthesizes): nothing to check it against. Relative to the
            # innermost scope enclosing it, as its declaration nests; else
            # by the convention alone.
            if own:
                s = scope
                while s and s != package:
                    if full.startswith(f'{s}.'):
                        return full[len(s) + 1:]
                    s = s.rpartition('.')[0]
            return rel
        visible = self.visible(file)
        if own:
            parts = rel.split('.')
            for i in range(len(parts) - 1, -1, -1):
                candidate = '.'.join(parts[i:])
                if self.resolve(candidate, scope, mode, visible) == full:
                    return candidate
        elif self.resolve(full, scope, mode, visible) == full:
            return full
        return f'.{full}'


def _join(prefix: str, name: str) -> str:
    return f'{prefix}.{name}' if prefix else name


def symbol_table(ctx: Context) -> SymbolTable:
    """The run's table (S1), built on first use from every loaded file."""
    table = ctx.symbol_table
    if table is None:
        from .re_file import ReFileDescriptorProto
        fdps = []
        for re_fdp in [*ctx.files.values(), *ctx.new_files.values()]:
            assert isinstance(re_fdp, ReFileDescriptorProto)
            this = getattr(re_fdp, 'this', None)
            if this is not None:
                fdps.append(this)
        table = SymbolTable.build(fdps)
        ctx.symbol_table = table
    return table
