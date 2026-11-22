# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Build the layouts fdp-scan's regression suite scores (spec 0448 S8).

Contributed by a downstream user, who compares their own scanner on the
same layouts; the committed files beside this one are its `public` output,
regenerated and compared by the `fdp-scan-corpus-check` Nix check.

    generate.py public OUT    the public corpus: the well-known types Python's
                            protobuf carries, as they are and mutated
    generate.py from-dir DIR OUT
                            binary FDPs found under DIR (`*.pb`), e.g. a
                            snapshot repository encoded back to binary with
                            `prototext encode`

Each layout is written as OUT/<layout>.bin, with OUT/<layout>.truth.json:
the true records, `[name, start, end]`, in order. A decoy layout's truth is
empty: any record found in it is spurious. OUT/corpus.json records what
generated the corpus (the protobuf version, for `public`).

Layouts:
    isolated     each record between runs of NUL bytes (C++ string literals)
    consecutive  groups of 20 records back to back, no separator
    fds          groups of 20 framed as a FileDescriptorSet (`0A <length>`
                 before each record), as Go embeds them
    noise-N      each record between random bytes, 16, 64 or 256 of them,
                 seed N (0, 1, 2)
    crafted      each record followed by a hand-built neighbor that starts
                 like a field: a valid tag with an overlong length, `0A` with
                 a plausible length, an invalid wire type
    trailer      each record followed by bytes framing a valid undeclared
                 top-level field (171), then random bytes: the record ends
                 before them
    decoys       bytes that start like a record, and aren't

A record ending in an undeclared top-level field (`#top-level-171`) is in
the layouts where a clean boundary follows it (NULs, the next record, the
end), and in no other: followed by random bytes, the field can't be told
from the neighbor's (`trailer` tests that case).
"""

from __future__ import annotations

import importlib
import json
import random
import sys
from collections.abc import Sequence
from pathlib import Path

import google.protobuf as protobuf
from google.protobuf import descriptor_pb2

Record = tuple[str, bytes]

_WKT_MODULES = [
    "any_pb2",
    "api_pb2",
    "descriptor_pb2",
    "duration_pb2",
    "empty_pb2",
    "field_mask_pb2",
    "source_context_pb2",
    "struct_pb2",
    "timestamp_pb2",
    "type_pb2",
    "wrappers_pb2",
    "compiler.plugin_pb2",
    "cpp_features_pb2",
    "java_features_pb2",
    "go_features_pb2",
]


def _varint(n: int) -> bytes:
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        if n:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def _field(number: int, payload: bytes) -> bytes:
    """A length-delimited field."""
    return _varint(number << 3 | 2) + _varint(len(payload)) + payload


def wkt_records() -> list[Record]:
    """The well-known types' FDPs, as serialized in Python's protobuf."""
    records: list[Record] = []
    for name in _WKT_MODULES:
        try:
            module = importlib.import_module(f"google.protobuf.{name}")
        except ImportError:
            continue
        serialized: bytes = module.DESCRIPTOR.serialized_pb
        fdp = descriptor_pb2.FileDescriptorProto.FromString(serialized)
        records.append((fdp.name, serialized))
    return records


def _with_file_option(serialized: bytes, raw: bytes) -> bytes:
    """`serialized` with `raw` appended to its FileOptions (created if absent)."""
    fdp = descriptor_pb2.FileDescriptorProto.FromString(serialized)
    fdp.options.MergeFromString(raw)
    return fdp.SerializeToString()


def mutations(records: list[Record]) -> list[Record]:
    """Variants of `records` reproducing, without private data, what a
    private corpus showed: fields the stock descriptor.proto doesn't
    declare."""
    out: list[Record] = []
    for name, b in records:
        # FileOptions field 5: not declared, outside the extension ranges.
        out.append((f"{name}#file-option-5", _with_file_option(b, _field(5, b"x"))))
        # A custom option: an extension (>= 1000), tolerated by both.
        out.append((f"{name}#custom-option", _with_file_option(b, _field(50000, b"x"))))
        # An unknown top-level field.
        out.append((f"{name}#top-level-171", b + _field(171, b"x")))
    # A method option the stock MethodOptions doesn't declare (field 8).
    svc = descriptor_pb2.FileDescriptorProto(name="svc/s.proto", package="svc")
    svc.message_type.add(name="R")
    method = svc.service.add(name="S").method.add(
        name="M", input_type=".svc.R", output_type=".svc.R"
    )
    method.options.MergeFromString(_field(8, b"x"))
    out.append(("svc/s.proto#method-option-8", svc.SerializeToString()))
    # protoc's FDP for an empty proto2 file: its name and nothing else.
    out.append(("empty/e.proto#name-only", _field(1, b"empty/e.proto")))
    # A java_package ending in .proto: field 1 of FileOptions, inside a real
    # record, which must not yield a record of its own; alone, and with an
    # undeclared option after it.
    java = _field(1, b"com.example.widgets.proto")
    first = records[0][1]
    out.append((f"{records[0][0]}#java-package", _with_file_option(first, java)))
    out.append(
        (
            f"{records[0][0]}#java-package+file-option-5",
            _with_file_option(first, java + _field(5, b"x")),
        )
    )
    return out


def decoys() -> list[bytes]:
    """Bytes that aren't FDPs but start like one."""
    options = descriptor_pb2.FileOptions(
        java_package="com.example.decoy.proto", java_multiple_files=True
    ).SerializeToString()
    rng = random.Random(4)
    return [
        # A Java codegen template, as in libprotoc: field 1, length 36, a
        # "name" ending in .proto once cut at 36 bytes.
        b"\n$deprecation$private com.google.protobuf.MapFieldBuilder<"
        b"$builder_type_parameters$>\n    internalGet$capitalized_name$() {\n",
        # A java_package option on its own: field 1 of FileOptions.
        _field(1, b"com.example.widgets.proto") + b"\x42\x05Proto",
        # A Python module's separately serialized FileOptions, as a .pyc
        # holds it: java_package and java_multiple_files read as a name and
        # a public_dependency with no import (spec 0449 S8). Alone, and
        # between random bytes.
        options,
        rng.randbytes(32) + options + rng.randbytes(32),
    ]


Chunk = tuple[str | None, bytes]

# Neighbors that start like a field.
_CRAFTED = [
    b"\x12\xff\xff\xff\x0f" + b"short",  # LEN field 2, length past the end
    b"\x0a\x10" + b"not-a-name\x01\x02\x03\x04\x05\x06",  # 0A, plausible length
    b"\x0f\x01\x02",  # wire type 7: invalid
    b"\x08\x96\x01" + b"\xff" * 4,  # a varint field, then junk
]


def write_layouts(records: list[Record], out: Path, meta: dict[str, str]) -> None:
    out.mkdir(parents=True, exist_ok=True)

    def emit(layout: str, chunks: Sequence[Chunk]) -> None:
        buf = bytearray()
        truth: list[tuple[str, int, int]] = []
        for name, chunk in chunks:
            if name is not None:
                truth.append((name, len(buf), len(buf) + len(chunk)))
            buf += chunk
        (out / f"{layout}.bin").write_bytes(bytes(buf))
        (out / f"{layout}.truth.json").write_text(json.dumps(truth))

    nul: Chunk = (None, b"\0" * 16)
    # Records with nothing undeclared at their end: for the layouts where
    # random bytes follow them.
    plain = [(n, b) for n, b in records if not n.endswith("#top-level-171")]

    isolated: list[Chunk] = []
    consecutive: list[Chunk] = []
    fds: list[Chunk] = []
    for i, (name, b) in enumerate(records):
        isolated += [nul, (name, b)]
        if i % 20 == 0:
            consecutive.append(nul)
            fds.append(nul)
        consecutive.append((name, b))
        fds += [(None, b"\x0a" + _varint(len(b))), (name, b)]
    for layout, chunks in (
        ("isolated", isolated),
        ("consecutive", consecutive),
        ("fds", fds),
    ):
        emit(layout, chunks + [nul])

    for seed in range(3):
        rng = random.Random(seed)
        noise: list[Chunk] = []
        for name, b in plain:
            noise += [(None, rng.randbytes(rng.choice((16, 64, 256)))), (name, b)]
        emit(f"noise-{seed}", noise + [(None, rng.randbytes(64))])

    rng = random.Random(3)
    crafted: list[Chunk] = []
    trailer: list[Chunk] = []
    for i, (name, b) in enumerate(plain):
        crafted += [nul, (name, b), (None, _CRAFTED[i % len(_CRAFTED)])]
        trailer += [nul, (name, b), (None, _field(171, b"x") + rng.randbytes(16))]
    emit("crafted", crafted + [nul])
    emit("trailer", trailer + [nul])

    emit("decoys", [c for d in decoys() for c in (nul, (None, d))] + [nul])
    (out / "corpus.json").write_text(json.dumps(meta, indent=1) + "\n")


def from_dir(root: Path) -> list[Record]:
    return sorted(
        (str(p.relative_to(root)), p.read_bytes()) for p in root.rglob("*.pb")
    )


def main(argv: list[str]) -> int:
    if len(argv) == 2 and argv[0] == "public":
        base = wkt_records()
        meta = {
            "source": "public",
            "protobuf": protobuf.__version__,
            "well_known_types": str(len(base)),
        }
        write_layouts(base + mutations(base), Path(argv[1]), meta)
    elif len(argv) == 3 and argv[0] == "from-dir":
        write_layouts(from_dir(Path(argv[1])), Path(argv[2]), {"source": "from-dir"})
    else:
        print(__doc__, file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
