# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""The graph dump: what phases 1 to 6 leave, as text (spec 0428 S1–S6).

`--dump-graph FILE` writes it after phase 6, and the run stops. protoback
writes the same dump; the two, compared byte for byte, point at the first
decision where the tools differ. A set is written sorted, a sequence in its
own order; reproto's node order is reproducible (spec 0428, Background).
"""

from __future__ import annotations

import hashlib
import logging
from pathlib import Path
from typing import TYPE_CHECKING, Any

from .lib.warnings import get_collector

if TYPE_CHECKING:
    from .context import Context
    from .topology import Topology

_SECTIONS = ("files", "pool", "context", "nodes", "seeds", "warnings")

# What a refused pool add warns with: its reason is Python's pool's text,
# which upb does not produce, so the dump keeps its kind and file only
# (spec 0428 S6).
_POOL_REFUSED = "Could not add descriptor from '%s' to pool: %s"


def _list(items: Any) -> str:
    return "[" + ", ".join(str(i) for i in items) + "]"


class Recorder:
    """What phases 1 to 6 warn about, in order: the collector's calls and
    the CLI messages (spec 0428 S6)."""

    _KINDS = (("cli_warn", "warning"), ("cli_info", "info"), ("cli_attn", "attention"))

    def __init__(self) -> None:
        self.events: list[str] = []
        recorder = self

        class _Handler(logging.Handler):
            def emit(self, record: logging.LogRecord) -> None:
                recorder._record(record)

        self._handler = _Handler(level=logging.DEBUG)
        logging.getLogger().addHandler(self._handler)
        collector = get_collector()
        for code in ("w1", "w3", "w4", "w5", "w6", "w7"):
            method = getattr(collector, code)

            def wrapped(*args: Any, _code: str = code, _method: Any = method) -> None:
                self.events.append(" | ".join([_code, *(str(a) for a in args)]))
                _method(*args)

            setattr(collector, code, wrapped)

    def _record(self, record: logging.LogRecord) -> None:
        kind = next((k for attr, k in self._KINDS if getattr(record, attr, False)), None)
        if kind is None:
            return
        if record.msg == _POOL_REFUSED and isinstance(record.args, tuple):
            self.events.append(f"pool-refused | {record.args[0]}")
            return
        self.events.append(f"{kind} | {record.getMessage()}")

    def stop(self) -> None:
        logging.getLogger().removeHandler(self._handler)


def _stripped_records(records: dict[str, list[Any]], tag: str) -> list[str]:
    lines = []
    for owner in sorted(records):
        for r in records[owner]:
            fields = [getattr(r, s) for s in r.__slots__]
            lines.append(" | ".join([f"  {tag}", owner, *(str(f) for f in fields)]))
    return lines


def _files(topo: Topology) -> list[str]:
    lines = []
    for name, f in topo.files.items():
        if f.is_ref():
            lines.append(f"{name} | ref")
            continue
        qf = f.qfile
        lines.append(" | ".join([
            name, "file", f"{qf.root}/{qf.rel_path}",
            "text" if qf.from_text else "binary",
            "seed" if getattr(f, "is_seed", False) else "-",
            "pruned" if f.is_pruned else "-",
            "imports " + _list(sorted(t.name for t in f.targets)),
            "stripped imports " + _list(f.stripped_dependencies),
            "stripped public " + _list(f.stripped_public_dependencies),
        ]))
        lines += _stripped_records(f.stripped_field_types, "stripped field")
        lines += _stripped_records(f.stripped_method_types, "stripped method")
    return lines


def _pool(ctx: Context) -> list[str]:
    lines = []
    for fdp in ctx.pool_db_fdps:
        data = fdp.SerializeToString()
        lines.append(f"{fdp.name} | {len(data)} | {hashlib.sha256(data).hexdigest()}")
    return lines


def _context(ctx: Context) -> list[str]:
    lines = [
        "pruned file names | " + _list(sorted(ctx.pruned_file_names)),
        "pruned fqdns | " + _list(sorted(ctx.pruned_fqdns)),
        "path seed fqdns | " + _list(sorted(ctx.path_seed_fqdns)),
    ]
    for key in sorted(ctx.edition_defaults):
        value = ctx.edition_defaults[key]
        if isinstance(value, dict):
            for name in sorted(value):
                lines.append(f"edition enum | {key} | {name} | {value[name]}")
        else:
            for edition, default in value:
                lines.append(f"edition default | {key} | {edition} | {default}")
    lines.append("schema db extra nodes | "
                 + _list(n.fqdn for n in ctx.schema_db_extra_nodes))
    return lines


def _nodes(ctx: Context) -> list[str]:
    lines = []
    for fqdn, n in ctx.nodes.items():
        flags = ("P" if n.is_pruned else "-") + ("R" if n.is_reachable else "-") \
            + ("S" if n.is_summoned else "-")
        parent = n.stored_parent.fqdn if n.stored_parent is not None else "-"
        lines.append(" | ".join([
            type(n).__name__, fqdn, "present" if n.is_present() else "stub",
            flags, parent,
            "contains " + _list(sorted(c.fqdn for c in n.contains)),
            "targets " + _list(sorted(t.fqdn for t in n.targets)),
        ]))
        for attr, tag in (("stripped_fields", "stripped field"),
                          ("stripped_methods", "stripped method")):
            for r in getattr(n, attr, None) or []:
                fields = [getattr(r, s) for s in r.__slots__]
                lines.append(" | ".join([f"  {tag}", *(str(x) for x in fields)]))
    return lines


def write(ctx: Context, topo: Topology, recorder: Recorder, path: Path) -> None:
    """Write the dump of what phases 1 to 6 left (spec 0428 S1–S6)."""
    recorder.stop()
    sections = {
        "files": _files(topo),
        "pool": _pool(ctx),
        "context": _context(ctx),
        "nodes": _nodes(ctx),
        "seeds": sorted(n.fqdn for n in ctx.seed_nodes),
        "warnings": recorder.events,
    }
    with path.open("w", encoding="utf-8") as out:
        for name in _SECTIONS:
            out.write(f"[{name}]\n")
            for line in sections[name]:
                out.write(line + "\n")
