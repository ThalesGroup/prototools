# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

import fnmatch
import logging
from pathlib import Path

from google.protobuf import text_format
from google.protobuf.descriptor_pb2 import FileDescriptorProto

from reproto import Context, Fqdn
from reproto.split_fdps import split_fdps

logger = logging.getLogger(__name__)
logger.propagate = True  # default is True, usually fine

# Common protobuf file extensions
BINARY_EXTENSIONS = ['.pb', '.binpb', '.pbset', '.protoset', '.desc']
TEXT_EXTENSIONS = ['.textpb', '.pbtxt', '.prototxt', '.ascii_proto']
PROTO_EXTENSION = '.proto'
ALL_EXTENSIONS = BINARY_EXTENSIONS + TEXT_EXTENSIONS


def matches_any_pattern(fqdn: Fqdn, patterns: set[Fqdn]) -> bool:
    """
    Check if an FQDN matches any of the given glob patterns.

    Args:
        fqdn: The fully qualified domain name to match
        patterns: Set of glob patterns (e.g., '.internal.*', '*.Service')

    Returns:
        True if fqdn matches any pattern (exact or glob), False otherwise
    """
    for pattern in patterns:
        # Support both exact matches and glob patterns
        if fqdn == pattern or fnmatch.fnmatch(fqdn, pattern):
            return True
    return False


def is_pruned(
        ctx: Context,
        rel_path: Path,
) -> bool:
    if rel_path.suffix in ALL_EXTENSIONS:
        target = Fqdn('file' + ':' + str(rel_path.with_suffix('.proto')))
    else:
        target = Fqdn('file' + ':' + str(rel_path))
    # FIXME: do we really want to match against pruned_fqdns with xxx:?
    return matches_any_pattern(target, ctx.pruned_fqdns)
        

class QualFile:
    def __init__(
            self,
            root: Path,
            rel_path: Path,
            contents: str | bytes,
    ) -> None:
        """
        Note: it is assumed that if given a file descriptor set, this file
        descriptor set contains exactly one file descriptor proto.
        """
        self.root = root
        self.rel_path = rel_path
        self.contents = contents
        self._name: str
        self._desc: FileDescriptorProto

    @property
    def name(self) -> str:
        return self._name

    @name.setter
    def name(self, v: str) -> None:
        self._name = v

    @property
    def desc(self) -> FileDescriptorProto:
        return self._desc

    @desc.setter
    def desc(self, v: FileDescriptorProto) -> None:
        self._desc = v


def _load_files(
    ctx: Context,
    roots: list[Path],
    rel_path: Path
) -> list[QualFile]:
    """
    Determine which files to load and whether they are text format.

    Returns a list of tuples: (res_path, is_text, rel_path)
    """

    loaded_files: list[QualFile] = []

    for root in roots:
        res_path = root / rel_path
        
        if res_path.is_dir():
            # Recursively collect all files with recognized extensions
            for f in res_path.rglob('*'):
                if f.suffix in ALL_EXTENSIONS:
                    loaded_files.append(QualFile(
                        root,
                        f.relative_to(root),
                        f.read_text() if f.suffix in TEXT_EXTENSIONS
                        else f.read_bytes(),
                    ))
            return loaded_files
    
        else:
            # Look for a single file
            if rel_path.suffix in TEXT_EXTENSIONS:
                if not res_path.is_file():
                    continue
                loaded_files.append(
                    QualFile(root, rel_path, res_path.read_text()))
                return loaded_files
            elif rel_path.suffix in BINARY_EXTENSIONS:
                if not res_path.is_file():
                    continue
                loaded_files.append(
                    QualFile(root, rel_path, res_path.read_bytes()))
                return loaded_files
            elif rel_path.suffix == PROTO_EXTENSION:
                # Try all extensions automatically
                for ext in TEXT_EXTENSIONS:
                    res_path = root / (rel_path.with_suffix(ext))
                    if res_path.is_file():
                        loaded_files.append(
                            QualFile(root, rel_path, res_path.read_text()))
                        return loaded_files
                for ext in BINARY_EXTENSIONS:
                    res_path = root / (rel_path.with_suffix(ext))
                    if res_path.is_file():
                        loaded_files.append(
                            QualFile(root, rel_path, res_path.read_bytes()))
                        return loaded_files
    from .lib.warnings import get_collector
    get_collector().w1(str(rel_path))
    return []


def load_from_path(
    ctx: Context,
    roots: list[Path],
    file_or_dir_path: Path
) -> list[QualFile]:
    """
    Load protobuf descriptors from a file or directory.

    - If a directory is given, recursively loads all descriptor files with
      supported extensions.
    - If a single file is given:
      - If extension is .proto → tries all binary/text extensions automatically
      - Otherwise → loads the file directly

    Returns a list of fully-initialised QualFiles (one per FDP found).
    """
    loaded_files = _load_files(ctx, roots, file_or_dir_path)
    qual_files: list[QualFile] = []
    for file in loaded_files:
        qual_files.extend(parse_qfile(ctx, file))
    return qual_files


def parse_qfile(
    ctx: Context,
    file: QualFile,
) -> list[QualFile]:
    """Split a raw QualFile into one QualFile per contained FDP.

    Delegates format detection, decapsulation, and splitting to split_fdps().
    Returns an empty list and logs a warning if the input cannot be parsed.
    """
    try:
        fragments = split_fdps(file.contents, file.rel_path.suffix)
    except ValueError:
        logger.warning(
            "Cannot parse '%s'", file.rel_path,
            extra={"cli_warn": True},
        )
        return []

    result: list[QualFile] = []
    for name, fragment in fragments:
        qf = QualFile(file.root, file.rel_path, fragment)
        qf.name = name
        if isinstance(fragment, bytes):
            fdp = FileDescriptorProto()
            fdp.ParseFromString(fragment)
            qf.desc = fdp
        else:
            qf.desc = text_format.Parse(
                fragment,
                FileDescriptorProto(),
                allow_unknown_field=True,
                allow_unknown_extension=True,
            )
        if ctx.debug:
            logger.warning(
                "Parsing file '%s'", file.rel_path,
                extra={"cli_warn": True},
            )
        result.append(qf)
    return result
