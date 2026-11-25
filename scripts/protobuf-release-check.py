# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Spec 0462 S3: in ci's build closure, each derivation referencing a
protobuf other than the release's must be another package's, listed in
ALLOWED; and each listed package must still reference one.

Usage: protobuf-release-check.py GRAPH RELEASE
  GRAPH    `nix-store --query --graph` of ci's .drv: edges from a
           dependency to the derivation referencing it
  RELEASE  nix/protobuf.nix's release, e.g. 36.2

Run by scripts/protobuf-release-check, which reads .drv files only:
nothing is built or fetched.
"""

import re
import sys

# Other packages whose own builds take another protobuf (spec 0462 N1),
# by pname, with the reason.
ALLOWED: dict[str, str] = {
    # buf was one until reproto's tests, which ran it, were retired (spec
    # 0487).
}

CPP = re.compile(r"^protobuf-(\d[\d.]*)\.drv$")
PYTHON = re.compile(r"^python3\.\d+-protobuf-(\d[\d.]*)\.drv$")
PNAME = re.compile(r"^(.*?)-\d")


EDGE = re.compile(r'^"([^"]+)" -> "([^"]+)"')


def name(node: str) -> str:
    return node.split("-", 1)[1]


def read_graph(path: str) -> dict[str, list[str]]:
    """Each derivation's references, from the graph's edges."""
    refs: dict[str, list[str]] = {}
    for line in open(path, encoding="utf-8"):
        if m := EDGE.match(line):
            dep, node = m.groups()
            refs.setdefault(node, []).append(dep)
            refs.setdefault(dep, [])
    return refs


def main() -> None:
    graph, release = sys.argv[1], sys.argv[2]
    refs = read_graph(graph)
    other = {}
    for node in refs:
        n = name(node)
        if (m := CPP.match(n)) and m.group(1) != release:
            other[node] = n
        if (m := PYTHON.match(n)) and m.group(1) != f"7.{release}":
            other[node] = n
    problems = []
    seen = set()
    for node, node_refs in sorted(refs.items(), key=lambda kv: name(kv[0])):
        taken = sorted(other[r][: -len(".drv")] for r in node_refs if r in other)
        if not taken:
            continue
        n = name(node)[: -len(".drv")]
        pname = (m.group(1) if (m := PNAME.match(n)) else n)
        if pname in ALLOWED:
            seen.add(pname)
        else:
            problems.append(f"{n} takes {', '.join(taken)}, not protobuf {release}")
    for pname in sorted(set(ALLOWED) - seen):
        problems.append(f"{pname} is listed in scripts/protobuf-release-check.py but takes "
                        f"no other protobuf any more: remove it")
    if problems:
        print("protobuf-release-check (spec 0462):", *problems, sep="\n  ", file=sys.stderr)
        print("A derivation of ours must take nix/protobuf.nix's protoc (default.nix's "
              "`protoc`) and pythonPkgs.protobuf.", file=sys.stderr)
        sys.exit(1)


main()
