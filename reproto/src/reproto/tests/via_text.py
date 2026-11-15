# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""reproto, every binary descriptor set it is given turned into text first.

Spec 0427: the text path checked against the binary one. Run as the
suite's command, ``REPROTO="python -m reproto.tests.via_text" pytest
reproto/src/reproto/tests``, every CLI test reads protoc's text form of
its inputs, ``protoc --decode`` with ``descriptor.proto`` alone, and must
pass as it does on the binary sets.

Explicit binary arguments become ``.textpb`` files in a temporary
directory; each ``-I`` directory is mirrored, its binary sets as
``.textpb``; a root-relative argument and a ``--filter-config`` follow the
renaming. A blob root, a file protoc cannot decode, and everything else
are passed as they are.
"""

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

BINARY = {".pb", ".binpb", ".pbset", ".protoset", ".desc"}


class _Converter:
    def __init__(self, tmp: Path) -> None:
        self.tmp = tmp
        self.mirrors: list[Path] = []
        self.count = 0

    def _fresh(self, prefix: str) -> Path:
        self.count += 1
        return self.tmp / f"{prefix}{self.count}"

    @staticmethod
    def to_text(src: Path, dst: Path) -> bool:
        """``src``'s text form into ``dst``, when protoc can decode it."""
        with src.open("rb") as data:
            result = subprocess.run(
                ["protoc", "--decode=google.protobuf.FileDescriptorSet",
                 "google/protobuf/descriptor.proto"],
                stdin=data, capture_output=True,
            )
        if result.returncode != 0 or not result.stdout.strip():
            return False
        dst.write_bytes(result.stdout)
        return True

    def file(self, path: Path) -> str:
        d = self._fresh("f")
        d.mkdir()
        dst = d / (path.stem + ".textpb")
        return str(dst) if self.to_text(path, dst) else str(path)

    def directory(self, path: Path) -> str:
        root = self._fresh("d")
        root.mkdir()
        for f in path.rglob("*"):
            if f.is_dir():
                continue
            out = root / f.relative_to(path)
            out.parent.mkdir(parents=True, exist_ok=True)
            if f.suffix in BINARY and self.to_text(f, out.with_suffix(".textpb")):
                continue
            shutil.copy2(f, out)
        self.mirrors.append(root)
        return str(root)

    def mirrored(self, rel: str) -> bool:
        """Whether a root-relative path has a text form in a mirror."""
        return any((m / rel).with_suffix(".textpb").is_file() for m in self.mirrors)

    def filter_config(self, path: Path) -> str:
        if not path.is_file():
            return str(path)
        copy = self._fresh("filter").with_suffix(".yaml")
        copy.write_text(path.read_text().replace(".pb", ".textpb"))
        return str(copy)


def _arguments(argv: list[str], c: _Converter) -> list[str]:
    args: list[str | tuple[str]] = []
    it = iter(argv)
    for a in it:
        if a in ("-I", "--desc-root"):
            args += [a, c.directory(Path(next(it)))]
        elif a.startswith("-I") and Path(a[2:]).is_dir():
            args.append("-I" + c.directory(Path(a[2:])))
        elif a.startswith("--desc-root=") and Path(a.split("=", 1)[1]).is_dir():
            args.append("--desc-root=" + c.directory(Path(a.split("=", 1)[1])))
        elif a == "--filter-config":
            args += [a, c.filter_config(Path(next(it)))]
        elif Path(a).suffix in BINARY and Path(a).is_file():
            args.append(c.file(Path(a)))
        elif Path(a).suffix in BINARY and not a.startswith("-"):
            # Relative to the roots: renamed once every mirror is made.
            args.append((a,))
        else:
            args.append(a)
    return [
        a if isinstance(a, str)
        else str(Path(a[0]).with_suffix(".textpb")) if c.mirrored(a[0])
        else a[0]
        for a in args
    ]


def main() -> int:
    tmp = Path(tempfile.mkdtemp(prefix="via-text-"))
    try:
        args = _arguments(sys.argv[1:], _Converter(tmp))
        return subprocess.run([sys.executable, "-m", "reproto.cli", *args]).returncode
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
