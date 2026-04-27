# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

from __future__ import annotations

from typing import Any

from google.protobuf.descriptor_pb2 import FileDescriptorProto, FileDescriptorSet
from google.protobuf.message import Message

from .load import QualFile


class Topology:
    def __init__(self) -> None:
        self.files: dict[str, ReFile] = dict()
        self.new_files: dict[str, ReFile] = dict()
    
    def merge_files(self) -> None:
        self.files.update(self.new_files)
        self.new_files = dict()
    
    def has_file(self, fqdn: str) -> bool:
        return fqdn in self.files or fqdn in self.new_files
    
    def find_file(self, fqdn: str) -> Any:
        if fqdn in self.files:
            return self.files[fqdn]
        if fqdn in self.new_files:
            return self.new_files[fqdn]
        return None

class Contents:
    def __init__(self, value: str | bytes) -> None:
        assert isinstance(value, (str, bytes))
        self.value = value

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
        self._qfile: QualFile
        self.is_reachable = False

    def is_ref(self) -> bool:
        return not hasattr(self, '_qfile')
    
    @property
    def qfile(self) -> QualFile:
        return self._qfile

    @qfile.setter
    def qfile(self, v: QualFile) -> None:
        self._qfile = v


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
                assert False
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

    @classmethod
    def from_name(
            cls,
            topo: Topology,
            name: str,
    ) -> ReFile:
        # Check if instance already exists
        instance = topo.find_file(name)
        if instance is not None:
            assert(isinstance(instance, ReFile))
            return instance
        # Instance does not exist, we create one
        return ReFile(topo, name)

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

        # --- File dependencies ------------------------------------------------

        # ...
        # import "path/to/dependendy.proto";
        # ...

        match qfile_or_name.desc:
            case FileDescriptorSet():
                fdp = qfile_or_name.desc.file[0]
            case FileDescriptorProto():
                fdp = qfile_or_name.desc
            case Message():
                assert False

        for index, dep in enumerate(fdp.dependency):
            assert isinstance(dep, str)
            file = ReFile.from_name(topo, dep)  # Gets or creates a ref
            self.targets.add(file)
