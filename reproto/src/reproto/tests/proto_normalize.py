# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""
Proto source normalizer for roundtrip test comparisons.

Provides comment stripping (via tree-sitter) and canonical formatting
(via buf) for .proto source text.  Test-only — not part of the
installable reproto package.
"""

import difflib
import os
import shutil
import subprocess
import tempfile
from pathlib import Path

from tree_sitter import Parser
from tree_sitter_language_pack import get_language

_PARSER = Parser(get_language("proto"))


def uncomment(text: str) -> str:
    """Remove all comment nodes from proto source text.

    Uses tree-sitter for syntax-aware parsing: // inside a string literal
    is part of a string_lit node, never a comment node.  Both // line
    comments and /* */ block comments are removed.

    Residual blank lines and trailing whitespace are cleaned up after
    splicing out the comment byte ranges.
    """
    src = text.encode()
    tree = _PARSER.parse(src)
    ranges: list[tuple[int, int]] = []
    stack = [tree.root_node]
    while stack:
        node = stack.pop()
        if node.type == "comment":
            ranges.append((node.start_byte, node.end_byte))
        else:
            stack.extend(node.children)
    out = bytearray(src)
    for start, end in sorted(ranges, reverse=True):
        del out[start:end]
    lines = out.decode().splitlines()
    lines = [line.rstrip() for line in lines if line.strip()]
    return '\n'.join(lines) + '\n'


def buf_format_batch(texts: dict[str, str]) -> dict[str, str]:
    """Run buf format on multiple proto texts in a single subprocess.

    Writes each text to a temp file under a shared tmpdir with a minimal
    buf.yaml, runs buf format --write on the directory, and reads results
    back.  Labels must be valid filename stems (no path separators).
    """
    with tempfile.TemporaryDirectory() as td:
        root = Path(td)
        (root / "buf.yaml").write_text("version: v2\n")
        paths: dict[str, Path] = {}
        for label, text in texts.items():
            p = root / f"{label}.proto"
            p.write_text(text)
            paths[label] = p
        subprocess.run(
            ["buf", "format", "--write", str(root)],
            check=True,
            env={**os.environ, "HOME": td},
        )
        return {label: paths[label].read_text() for label in texts}


def normalize_proto_batch(texts: dict[str, str]) -> dict[str, str]:
    """Uncomment then buf-format multiple proto source strings.

    All buf formatting happens in a single subprocess call.
    """
    uncommented = {k: uncomment(v) for k, v in texts.items()}
    return buf_format_batch(uncommented)


def normalize_proto(text: str) -> str:
    """Uncomment then buf-format a single proto source string."""
    return normalize_proto_batch({"f": text})["f"]


# Resolve the prototext binary: prefer PATH (nix-build sandbox), fall back to
# the repo-relative path used in the nix-shell development environment.
_PROTOTEXT = Path(
    shutil.which("prototext")
    or str(Path(__file__).parents[4] / "bin" / "prototext")
)


def _decode_pb(data: bytes) -> list[str]:
    result = subprocess.run(
        [str(_PROTOTEXT), "decode", "--type", "google.protobuf.FileDescriptorSet"],
        input=data,
        capture_output=True,
    )
    return result.stdout.decode(errors="replace").splitlines(keepends=True)


def pb_diff(pb1: bytes, pb2: bytes) -> str:
    """Decode both .pb blobs via prototext and return a unified diff.

    Returns an empty string if the blobs are equal.
    """
    if pb1 == pb2:
        return ""
    left  = _decode_pb(pb1)
    right = _decode_pb(pb2)
    return "".join(difflib.unified_diff(left, right, fromfile="expected", tofile="actual"))


def pb_diff_fields(pb1: bytes, pb2: bytes) -> set[str]:
    """Return the set of field names that differ between two .pb blobs.

    Parses the prototext output and collects field names from lines that
    appear only in pb1 or only in pb2 (i.e. lines starting with '-' or '+'
    in the unified diff, excluding the diff header lines).
    """
    import re
    if pb1 == pb2:
        return set()
    left  = _decode_pb(pb1)
    right = _decode_pb(pb2)
    diff = difflib.unified_diff(left, right)
    fields: set[str] = set()
    for line in diff:
        if line.startswith(('---', '+++')):
            continue
        if line.startswith(('+', '-')):
            m = re.match(r'[+-]\s*(\w+)', line)
            if m:
                fields.add(m.group(1))
    return fields
