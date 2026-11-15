# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

from __future__ import annotations

from typing import Any


from .load import QualFile


class Topology:
    def __init__(self) -> None:
        self.files: dict[str, ReFile] = {}
        self.new_files: dict[str, ReFile] = {}
    
    def merge_files(self) -> None:
        self.files.update(self.new_files)
        self.new_files = {}

    def has_file(self, fqdn: str) -> bool:
        return fqdn in self.files or fqdn in self.new_files
    
    def find_file(self, fqdn: str) -> Any:
        if fqdn in self.files:
            return self.files[fqdn]
        if fqdn in self.new_files:
            return self.new_files[fqdn]
        return None

class File:
    def __init__(
            self,
            topo: Topology,
            name: str
    ) -> None:
        self.topo = topo
        self.name = name
        self.is_pruned = False
        self.targets: set[File] = set()
        self._qfile: QualFile | None = None
        self.is_reachable = False
        # Dependencies stripped because they are unresolvable (pruned duplicates
        # or absent from the input); recorded for orphan rendering.
        self.stripped_dependencies: list[str] = []
        self.stripped_public_dependencies: list[str] = []
        # Fields stripped because their type_name was not in pool_db at Add()
        # time (spec 0087).  Keyed by message FQDN (e.g. ".pkg.Msg").
        self.stripped_field_types: dict[str, list[object]] = {}
        # Service methods stripped because their input_type or output_type was
        # not in pool_db (spec 0087).  Keyed by service FQDN.
        self.stripped_method_types: dict[str, list[object]] = {}

    def is_ref(self) -> bool:
        return self._qfile is None

    @property
    def qfile(self) -> QualFile:
        assert self._qfile is not None
        return self._qfile

    @qfile.setter
    def qfile(self, v: QualFile) -> None:
        self._qfile = v

    def forget(self) -> None:
        """Back to a reference: the file turned out unreadable, and is
        missing from the input from then on (spec 0427 S3)."""
        self._qfile = None


class ReFile(File):
    def __new__(
            cls,
            topo: Topology,
            qfile_or_name: QualFile | str,
        ) -> ReFile:
        match qfile_or_name:
            case QualFile():
                name = qfile_or_name.name
            case str():
                name = qfile_or_name
            case _:
                raise AssertionError(f"Expected QualFile or str, got {type(qfile_or_name)}")
        # Check if instance already exists
        instance = topo.find_file(name)
        if instance is None:
            # Create a new instance
            instance = super().__new__(cls)
            File.__init__(instance, topo, name)
            topo.new_files[name] = instance
        else:
            assert(isinstance(instance, ReFile))
        return instance

    def __init__(
            self,
            topo: Topology,
            qfile_or_name: QualFile | str,
        ) -> None:
        if isinstance(qfile_or_name, str):
            # We are just creating a reference
            assert(topo.has_file(qfile_or_name))
            return
        assert isinstance(qfile_or_name, QualFile)
        if not self.is_ref():
            # We have already been fully initialized
            assert isinstance(self.qfile, QualFile)
            return
        # Ah! Now we can properly finalize the initialization
        assert isinstance(qfile_or_name, QualFile)
        self.qfile = qfile_or_name
        self.is_seed = False

        # Its imports only: a text file is not parsed before the input is
        # read (spec 0427 S1).
        for dep in qfile_or_name.imports:
            assert isinstance(dep, str)
            file = ReFile(topo, dep)
            self.targets.add(file)
