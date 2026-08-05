# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

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

_GLOB_CHARS = set('*?[')


# Memo for blob_members(), keyed on the root path. A blob is scanned in
# full, and several DESCRIPTOR_FILES arguments may resolve against the
# same root (spec 0243 S7).
_blob_members: dict[Path, list[tuple[Path, bytes]]] = {}


def blob_members(blob: Path) -> list[tuple[Path, bytes]]:
    """Read a file-shaped -I root as protoscan would have expanded it.

    Returns (root-relative path, FDP bytes) pairs — one per
    FileDescriptorProto the scanner accepts inside `blob`, named after
    the FDP itself with a '.proto' suffix rewritten to '.pb'. That is
    byte-for-byte the tree `protoscan --proto_out` writes, which is what
    lets everything keyed on a root-relative path (the '.' argument,
    -s/-p patterns, the W7 dedup) work unchanged (spec 0243 S5/S6).
    """
    cached = _blob_members.get(blob)
    if cached is not None:
        return cached

    # Deferred import (spec 0243 S10). fdp_scan_lib embeds the WKT
    # scoring graph, and that graph is built by running reproto: a
    # top-level import would put fdp_scan_lib into reprotoBare's closure
    # and close an eval-time cycle in the Nix build.
    from fdp_scan_lib import scan

    data = blob.read_bytes()
    view = memoryview(data)
    members: list[tuple[Path, bytes]] = []
    for start, end in scan(data):
        fragment = bytes(view[start:end])
        # scan() returns only candidates it has already accepted as a
        # FileDescriptorProto (spec 0239 G4); parsing here is not a
        # filter, it is how the member's name is read.
        fdp = FileDescriptorProto()
        fdp.ParseFromString(fragment)
        if not fdp.name:
            continue
        rel_path = Path(fdp.name)
        if rel_path.suffix == PROTO_EXTENSION:
            rel_path = rel_path.with_suffix('.pb')
        members.append((rel_path, fragment))

    _blob_members[blob] = members
    return members


class PathPatterns:
    """A set of root-relative path patterns, partitioned for fast lookup.

    Literal patterns (no glob metacharacters) are matched via O(1) set
    membership (spec 0149 G8). Glob patterns are matched via fqdn_match()
    using a synthesized 'path:' pseudo-FQDN — the exact same
    PurePosixPath.full_match()-based engine already used for -s/-p FQDN
    patterns (spec 0074): '*' matches one path segment, '**' matches any
    number of segments including zero.
    """
    def __init__(self, patterns: set[str]) -> None:
        self.literals = {p for p in patterns if not (_GLOB_CHARS & set(p))}
        self.globs = [p for p in patterns if _GLOB_CHARS & set(p)]

    def matches(self, rel_path: Path) -> bool:
        s = rel_path.as_posix()
        if s in self.literals:
            return True
        if not self.globs:
            return False
        from .phases import fqdn_match  # deferred: avoids load.py/phases.py import cycle
        return any(
            fqdn_match(Fqdn(f'path:{p}'), Fqdn(f'path:{s}'))
            for p in self.globs
        )


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


def _blob_files(
    ctx: Context,
    root: Path,
    rel_path: Path,
) -> list[QualFile]:
    """Select the members of a blob root named by `rel_path`."""
    members = blob_members(root)

    # A '.proto' argument names the member the same way a directory root
    # would: by the source name, with the extension resolved for it.
    wanted = rel_path
    if wanted.suffix == PROTO_EXTENSION:
        wanted = wanted.with_suffix('.pb')
    whole_blob = rel_path == Path('.')

    selected: list[QualFile] = []
    for member, fragment in members:
        if not whole_blob and member != wanted and wanted not in member.parents:
            continue
        if ctx.pruned_paths.matches(member):
            continue
        selected.append(QualFile(root, member, fragment))
    return selected


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
        # A file-shaped root is a blob: it stands for the FDPs inside it
        # (spec 0243 S5), so it is not joined with rel_path at all.
        if root.is_file():
            loaded_files.extend(_blob_files(ctx, root, rel_path))
            continue

        res_path = root / rel_path

        if res_path.is_dir():
            # Recursively collect all files with recognized extensions.
            # Do not stop here (spec 0148 G1): a directory-shaped seed
            # argument (e.g. ".") must be scanned under every -I root, not
            # just the first one that happens to resolve as a directory.
            for f in res_path.rglob('*'):
                if f.suffix in ALL_EXTENSIONS:
                    f_rel = f.relative_to(root)
                    if ctx.pruned_paths.matches(f_rel):
                        continue
                    loaded_files.append(QualFile(
                        root,
                        f_rel,
                        f.read_text() if f.suffix in TEXT_EXTENSIONS
                        else f.read_bytes(),
                    ))
            continue

        else:
            # Look for a single file
            if rel_path.suffix in TEXT_EXTENSIONS:
                if not res_path.is_file():
                    continue
                if ctx.pruned_paths.matches(rel_path):
                    continue
                loaded_files.append(
                    QualFile(root, rel_path, res_path.read_text()))
                return loaded_files
            elif rel_path.suffix in BINARY_EXTENSIONS:
                if not res_path.is_file():
                    continue
                if ctx.pruned_paths.matches(rel_path):
                    continue
                loaded_files.append(
                    QualFile(root, rel_path, res_path.read_bytes()))
                return loaded_files
            elif rel_path.suffix == PROTO_EXTENSION:
                # Try all extensions automatically
                for ext in TEXT_EXTENSIONS:
                    res_path = root / (rel_path.with_suffix(ext))
                    if res_path.is_file():
                        if ctx.pruned_paths.matches(rel_path):
                            continue
                        loaded_files.append(
                            QualFile(root, rel_path, res_path.read_text()))
                        return loaded_files
                for ext in BINARY_EXTENSIONS:
                    res_path = root / (rel_path.with_suffix(ext))
                    if res_path.is_file():
                        if ctx.pruned_paths.matches(rel_path):
                            continue
                        loaded_files.append(
                            QualFile(root, rel_path, res_path.read_bytes()))
                        return loaded_files
    if loaded_files:
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

    When the same FileDescriptorProto.name is produced by more than one
    source (e.g. present under two different -I roots), only the first
    occurrence (in root order, then file-discovery order) is kept; the
    rest are dropped with a W7 warning (spec 0148 G2/G3/G4).

    Returns a list of fully-initialised QualFiles (one per FDP found).
    """
    loaded_files = _load_files(ctx, roots, file_or_dir_path)
    qual_files: list[QualFile] = []
    for file in loaded_files:
        parsed = parse_qfile(ctx, file)
        qual_files.extend(parsed)
        # Seed-by-path (spec 0149 G4): every FDP produced by a physical
        # candidate whose root-relative path matches a path seed pattern
        # becomes an ordinary file:<name> FQDN seed, regardless of whether
        # it goes on to survive the G2 dedup pass below.
        if ctx.path_seeds.matches(file.rel_path):
            for qf in parsed:
                ctx.path_seed_fqdns.add(Fqdn('file:' + qf.name))

    from .lib.warnings import get_collector
    collector = get_collector()
    seen: dict[str, QualFile] = {}
    deduped: list[QualFile] = []
    for qf in qual_files:
        first = seen.get(qf.name)
        if first is None:
            seen[qf.name] = qf
            deduped.append(qf)
        else:
            collector.w7(
                qf.name,
                str(qf.root / qf.rel_path),
                str(first.root / first.rel_path),
            )
    return deduped


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
