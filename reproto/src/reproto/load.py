# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

import fnmatch
import logging
from pathlib import Path

from google.protobuf import text_format
from google.protobuf.descriptor_pb2 import FileDescriptorProto, FileDescriptorSet
from google.protobuf.message import DecodeError, Message

import prototext_codec_lib as _pt_codec

from reproto import Context, Fqdn

logger = logging.getLogger(__name__)
logger.propagate = True  # default is True, usually fine

# Common protobuf file extensions
BINARY_EXTENSIONS = ['.pb', '.binpb', '.pbset', '.protoset', '.desc']
TEXT_EXTENSIONS = ['.textpb', '.pbtxt', '.prototxt', '.ascii_proto']
PROTO_EXTENSION = '.proto'
ALL_EXTENSIONS = BINARY_EXTENSIONS + TEXT_EXTENSIONS

def decapsulate(text: str) -> str:
    """
    Remove enclosing `entry {}` from a .textpb string, handling surrounding spaces.
    Note: This is a pure slicing function (no string copy involved)
    """

    # Strip leading whitespace to check for 'entry {'
    start = 0
    while start < len(text) and text[start].isspace():
        start += 1

    # Check if we have 'entry' followed by optional spaces and '{'
    if text.startswith("entry", start):
        i = start + len("entry")
        while i < len(text) and text[i].isspace():
            i += 1
        if i < len(text) and text[i] == "{":
            # Found opening 'entry {', slice past it
            start = i + 1
            # Strip any additional whitespace after '{'
            while start < len(text) and text[start].isspace():
                start += 1

            # Now find the matching closing '}' at the end
            end = len(text) - 1
            while end >= start and text[end].isspace():
                end -= 1
            if end >= start and text[end] == "}":
                end -= 1  # Exclude the '}'
                # Strip any whitespace before '}'
                while end >= start and text[end].isspace():
                    end -= 1
                end += 1  # Make the slice inclusive of last content

            return text[start:end]

    # No enclosing 'entry { ... }' found
    return text


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
        self._desc: FileDescriptorProto | FileDescriptorSet  # | Message

    @property
    def name(self) -> str:
        return self._name

    @name.setter
    def name(self, v: str) -> None:
        self._name = v

    @property
    def desc(self) -> FileDescriptorProto | FileDescriptorSet | Message:
        return self._desc

    @desc.setter
    def desc(self, v: FileDescriptorProto | FileDescriptorSet) -> None:
        assert not isinstance(v, FileDescriptorSet) or len(v.file) == 1
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
                if (f.suffix in ALL_EXTENSIONS):
                    #and not is_pruned(ctx, f.relative_to(root))):
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
                #if is_pruned(ctx, rel_path):
                #    continue
                loaded_files.append(
                    QualFile(root, rel_path, res_path.read_text()))
                return loaded_files
            elif rel_path.suffix in BINARY_EXTENSIONS:
                if not res_path.is_file():
                    continue
                #if is_pruned(ctx, rel_path):
                #    continue
                loaded_files.append(
                    QualFile(root, rel_path, res_path.read_bytes()))
                return loaded_files
            elif rel_path.suffix == PROTO_EXTENSION:
                #if is_pruned(ctx, rel_path):
                #    continue
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
    logger.warning(
        "File '%s' is missing, skipping", rel_path,
        extra={"cli_warn": True}
    )
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

    Returns a set of pre-initialized QualFiles.
    """

    loaded_files = _load_files(ctx, roots, file_or_dir_path)


    qual_files: list[QualFile] = list()
    for file in loaded_files:
        qfile = parse_qfile(ctx, file)
        if qfile is not None:
            qual_files.append(file)
    return qual_files


def parse_qfile(
    ctx: Context,
    file: QualFile
) -> QualFile | None:
    """
    Load protobuf descriptors from a file or directory.

    - If a directory is given, recursively loads all descriptor files with
    supported extensions.
    - If a single file is given:
    - If extension is .proto → tries all binary/text extensions automatically
    - Otherwise → loads the file directly

    Returns a set of pre-initialized QualFiles.
    """

    ok = False
    match file.contents:
        case str():
            data = decapsulate(file.contents)
            if not ok:
                try:
                    fds = text_format.Parse(
                        data,
                        FileDescriptorSet(),
                        allow_unknown_field=True,
                        allow_unknown_extension=True,
                    )
                    if fds.file and len(fds.file) == 1 and fds.file[0].name:
                        file.desc = fds
                        file.name = fds.file[0].name
                        ok = True
                except (text_format.ParseError, IndexError, AttributeError):
                    pass
            if not ok:
                try:
                    fdp = text_format.Parse(
                        data,
                        FileDescriptorProto(),
                        allow_unknown_field=True,
                        allow_unknown_extension=True,
                    )
                    if fdp.name:
                        file.desc = fdp
                        file.name = fdp.name
                        ok = True
                except (text_format.ParseError, AttributeError):
                    pass
        case bytes():
            data = _pt_codec.format_as_bytes(file.contents)
            file.contents = data  # normalise once; Phase 2 reuses qf.contents
            if not ok:
                try:
                    fds = FileDescriptorSet()
                    fds.ParseFromString(data)
                    if fds.file and len(fds.file) == 1 and fds.file[0].name:
                        file.desc = fds
                        file.name = fds.file[0].name
                        ok = True
                except (DecodeError, IndexError, AttributeError):
                    pass
            if not ok:
                try:
                    fdp = FileDescriptorProto()
                    fdp.ParseFromString(data)
                    if fdp.name:
                        file.desc = fdp
                        file.name = fdp.name
                        ok = True
                except (DecodeError, AttributeError):
                    pass
    if ok:
        if ctx.debug:
            logger.warning(
                "Parsing file '%s'", file.rel_path,
                extra={"cli_warn": True}
            )
        return file
    
    else:
        logger.warning(
            "Cannot parse '%s'", file.rel_path,
            extra={"cli_warn": True}
        )
        return None
