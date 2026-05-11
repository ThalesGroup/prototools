# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

from __future__ import annotations

import logging
from collections import Counter


def cli_error(message: str, *args, **kwargs) -> None:
    kwargs.setdefault("extra", {}).setdefault("cli_err", True)
    logging.warning(message, *args, stacklevel=2, **kwargs)

def cli_warning(message: str, *args, **kwargs) -> None:
    kwargs.setdefault("extra", {}).setdefault("cli_warn", True)
    logging.warning(message, *args, stacklevel=2, **kwargs)

def cli_info(message: str, *args, **kwargs) -> None:
    kwargs.setdefault("extra", {}).setdefault("cli_info", True)
    logging.warning(message, *args, stacklevel=2, **kwargs)

def cli_attention(message: str, *args, **kwargs) -> None:
    kwargs.setdefault("extra", {}).setdefault("cli_attn", True)
    logging.warning(message, *args, stacklevel=2, **kwargs)


def _occ(count: int) -> str:
    """Format an occurrence count suffix."""
    noun = "occurrence" if count == 1 else "occurrences"
    return f"({count} {noun})"


class WarningCollector:
    """Buffer W4/W5/W6 warnings; print W3 immediately.

    W1 (missing source file during loading) and W5 (missing file dependency
    during rendering) share the same counter in squashed mode — both name
    the same root cause and require the same fix (add the file to -I).

    In squashed mode (default): counts occurrences per distinct message and
    flushes a sorted summary at the end.  In detailed mode: every warning is
    printed immediately, no buffering.

    Usage::

        collector = WarningCollector(detailed=False)
        collector.w1("third_party/foo.proto")   # loading miss — feeds W5 counter
        collector.w4(".some.pkg.Type")
        collector.w5("third_party/bar.proto")   # render miss — feeds W5 counter
        collector.w6("my/file.proto", "field 'x'", "Couldn't find Extension 42")
        collector.w3("file:a.proto pruned — ...")  # always immediate
        collector.flush()
    """

    def __init__(self, detailed: bool = False) -> None:
        self._detailed = detailed
        # Counters for squashed mode (unused in detailed mode)
        self._w4: Counter[str] = Counter()
        self._w5: Counter[str] = Counter()
        self._w6: Counter[str] = Counter()
        # Names of files pruned by reproto itself — W5 for these is suppressed.
        self._pruned_files: set[str] = set()
        # Names of files that will be satisfied by an embedded fallback —
        # W1 (loading-phase miss) for these is suppressed because not finding
        # them on the -I path is expected and intentional.
        self._fallback_files: set[str] = set()

    def register_pruned_file(self, name: str) -> None:
        """Register a file name as pruned so W5 warnings for it are suppressed."""
        self._pruned_files.add(name)

    def register_fallback_file(self, name: str) -> None:
        """Register a file name as provided by an embedded fallback.

        Suppresses the W1 loading-phase warning that would otherwise fire when
        the import-discovery loop fails to find the file on the -I search path.
        """
        self._fallback_files.add(name)

    def w1(self, missing_file: str) -> None:
        """Missing source file (W1 — loading phase).

        In detailed mode: printed immediately with the loading-phase message.
        In squashed mode: feeds the W5 counter so loading and rendering misses
        for the same file are combined into a single summary line.
        """
        if missing_file in self._pruned_files:
            return
        if missing_file in self._fallback_files:
            return
        if self._detailed:
            cli_warning(f"Warning: missing file '{missing_file}' (not found on -I path; skipped)")
        else:
            self._w5[missing_file] += 1

    def w4(self, type_name: str) -> None:
        """Unresolvable type reference (W4)."""
        if self._detailed:
            cli_warning(f"Warning: unresolvable type {type_name}")
        else:
            self._w4[type_name] += 1

    def w5(self, dep_file: str) -> None:
        """Missing file dependency (W5 — rendering phase).

        Suppressed silently if dep_file was pruned by reproto itself.
        """
        if dep_file in self._pruned_files:
            return
        if self._detailed:
            cli_warning(f"Warning: missing dependency file:{dep_file}")
        else:
            self._w5[dep_file] += 1

    def w6(self, proto_file: str, field_ctx: str, error: str) -> None:
        """Option rendering failure (W6) — Couldn't find Extension/message.

        proto_file: the .proto file being rendered
        field_ctx:  e.g. "field 'message_set_extension'"
        error:      the raw error string, e.g. "Couldn't find Extension 42"
        """
        key = f"'{proto_file}' {field_ctx}: {error}"
        if self._detailed:
            cli_warning(f"Warning: {key}")
        else:
            self._w6[key] += 1

    def w3(self, message: str) -> None:
        """Duplicate symbol pruning (W3) — always printed immediately."""
        cli_warning(message)

    def flush(self) -> None:
        """Flush buffered squashed warnings to stderr."""
        if self._detailed:
            return
        # Flush in decreasing order of actionability (spec 0041 §2, §7, §8):
        # W5 (add file to -I; W1 loading misses merged here) →
        # W4 (grep for type) → W6 (rendering degradation, least actionable).
        suppressed = False
        for dep_file, count in sorted(self._w5.items()):
            cli_warning(f"Warning: missing dependency file:{dep_file} {_occ(count)}")
            if count > 1:
                suppressed = True
        for type_name, count in sorted(self._w4.items()):
            cli_warning(f"Warning: unresolvable type {type_name} {_occ(count)}")
            if count > 1:
                suppressed = True
        for key, count in sorted(self._w6.items()):
            cli_warning(f"Warning: {key} {_occ(count)}")
            if count > 1:
                suppressed = True
        if suppressed:
            cli_info("Run with --detailed-warnings to see all warning occurrences.")


# Module-level singleton — configured once at startup by reproto.py/cli.py.
_collector: WarningCollector = WarningCollector(detailed=False)


def get_collector() -> WarningCollector:
    return _collector


def configure_collector(detailed: bool) -> None:
    global _collector
    _collector = WarningCollector(detailed=detailed)

