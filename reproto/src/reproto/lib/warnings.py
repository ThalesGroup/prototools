# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""Warnings: one code each (spec 0438), counted, and printed as
``Warning [CODE]: message``; squashed ones summarized at the end."""

from __future__ import annotations

import logging
from collections import Counter

# Spec 0438 S1, in its order: what each code means. --fail-on takes these,
# the anomalies' codes (anomalies.ANOMALIES) besides.
CODES: dict[str, str] = {
    "W1": "a file not found on the -I path; skipped (summarized with W5)",
    "W2": "an editions file written as proto2 (--force-proto2-for-editions)",
    "W3": "a duplicate symbol pruned",
    "W4": "an unresolvable type",
    "W5": "a missing dependency",
    "W6": "an option value or a file that cannot be rendered",
    "W7": "a file shadowed by one of the same name under another -I root",
    "W8": "a descriptor the pool refuses while loading",
    "W9": "an unparseable file skipped while loading",
    "W10": "a circular dependency",
    "W11": "a prune pattern matching nothing",
    "W12": "a seed pattern matching nothing",
    "W13": "two files rewritten onto one path, the variant's descriptor.proto kept",
    "W14": "--dump-resolved-features: a file not loaded",
    "W15": "an input that cannot be parsed",
    "W16": "a self-dependency stripped",
    "W17": "a variant key no tool reads",
    "W18": "a deprecated option or variable",
    "W19": "a file the descriptor pool refuses: its options not rendered",
    "W20": "a schema-database entry that cannot be built",
    "W21": "a scoring-graph entry that cannot be built",
    "W22": "an option field the options type does not declare: an orphan",
    "T1": "text: a field by number declared nowhere, read as length-delimited",
    "T2": "text: a string protoc printed as a message, rebuilt",
    "T3": "text: an [extension] declared nowhere, skipped",
    "T4": "text: a field name declared nowhere, skipped",
}


def cli_error(message: str, *args, **kwargs) -> None:
    kwargs.setdefault("extra", {}).setdefault("cli_err", True)
    logging.warning(message, *args, stacklevel=2, **kwargs)

def cli_warning(message: str, *args, **kwargs) -> None:
    """A diagnostic on the warning stream, with no code: `--debug` output.
    A warning goes through `warn`."""
    kwargs.setdefault("extra", {}).setdefault("cli_warn", True)
    logging.warning(message, *args, stacklevel=2, **kwargs)

def cli_info(message: str, *args, **kwargs) -> None:
    kwargs.setdefault("extra", {}).setdefault("cli_info", True)
    logging.warning(message, *args, stacklevel=2, **kwargs)

def cli_attention(message: str, *args, **kwargs) -> None:
    kwargs.setdefault("extra", {}).setdefault("cli_attn", True)
    logging.warning(message, *args, stacklevel=2, **kwargs)


def _line(code: str, message: str) -> None:
    """Print one warning line, its code first (spec 0438 S2)."""
    logging.warning("%s", f"Warning [{code}]: {message}", stacklevel=3,
                    extra={"cli_warn": True})


def warn(code: str, message: str) -> None:
    """A warning: counted under `code`, printed now."""
    get_collector().count(code)
    _line(code, message)


def _occ(count: int) -> str:
    """Format an occurrence count suffix."""
    noun = "occurrence" if count == 1 else "occurrences"
    return f"({count} {noun})"


# The squashed codes, in the order their summary lines are printed: the
# most actionable first (spec 0041 §2, §7, §8), then by code.
_SQUASHED_ORDER = ("W5", "W4", "W6", "W19", "W20", "W21", "W22", "T1", "T2", "T3", "T4")


class WarningCollector:
    """Every warning counted by code (for --fail-on); the squashed ones
    buffered and summarized at the end, one line per message with its
    count; in detailed mode printed as they occur.

    W1 (a file missing while loading) and W5 (missing while rendering) share
    W5's summary line: the same cause, the same fix (add the file to -I).
    Each is counted under its own code.
    """

    def __init__(self, detailed: bool = False) -> None:
        self._detailed = detailed
        self.counts: Counter[str] = Counter()
        # (code, message) → occurrences, for squashed mode.
        self._squashed: dict[str, Counter[str]] = {c: Counter() for c in _SQUASHED_ORDER}
        self._prost: list[str] = []   # --force-proto2-for-editions patched file names
        # Names of files pruned by reproto itself — W5 for these is suppressed.
        self._pruned_files: set[str] = set()
        # Names of files that will be satisfied by an embedded fallback —
        # W1 (loading-phase miss) for these is suppressed because not finding
        # them on the -I path is expected and intentional.
        self._fallback_files: set[str] = set()

    def count(self, code: str) -> None:
        assert code in CODES or code[0] in "ABCD", code
        self.counts[code] += 1

    def register_pruned_file(self, name: str) -> None:
        """Register a file name as pruned so W5 warnings for it are suppressed."""
        self._pruned_files.add(name)

    def register_fallback_file(self, name: str) -> None:
        """Register a file name as provided by an embedded fallback.

        Suppresses the W1 loading-phase warning that would otherwise fire when
        the import-discovery loop fails to find the file on the -I search path.
        """
        self._fallback_files.add(name)

    def squash(
        self, code: str, message: str, summary_code: str | None = None,
        detailed: str | None = None,
    ) -> None:
        """A warning of a squashed code: counted under `code`; printed now in
        detailed mode (as `detailed`, default `message`), else summarized by
        `message` under `summary_code` (default `code`)."""
        self.count(code)
        if self._detailed:
            _line(code, detailed or message)
        else:
            self._squashed[summary_code or code][message] += 1

    def w1(self, missing_file: str) -> None:
        """Missing source file (W1 — loading phase); summarized with W5."""
        if missing_file in self._pruned_files or missing_file in self._fallback_files:
            return
        if self._detailed:
            self.count("W1")
            _line("W1", f"missing file '{missing_file}' (not found on -I path; skipped)")
        else:
            self.squash("W1", f"missing dependency file:{missing_file}", summary_code="W5")

    def w4(self, type_name: str) -> None:
        """Unresolvable type reference (W4)."""
        self.squash("W4", f"unresolvable type {type_name}")

    def w5(self, dep_file: str) -> None:
        """Missing file dependency (W5 — rendering phase).

        Suppressed silently if dep_file was pruned by reproto itself.
        """
        if dep_file in self._pruned_files:
            return
        self.squash("W5", f"missing dependency file:{dep_file}")

    def w6(self, proto_file: str, field_ctx: str, error: str, code: str = "W6") -> None:
        """Something that cannot be rendered or built (W6, W19, W20, W21).

        proto_file: the .proto file being rendered
        field_ctx:  e.g. "field 'message_set_extension'"
        error:      the raw error string, e.g. "Couldn't find Extension 42"
        """
        self.squash(code, f"'{proto_file}' {field_ctx}: {error}")

    def w3(self, message: str) -> None:
        """Duplicate symbol pruning (W3) — always printed immediately."""
        warn("W3", message)

    def w7(self, fdp_name: str, shadowed_path: str, kept_path: str) -> None:
        """Duplicate FDP name across -I roots (W7, spec 0148) — always
        printed immediately, not squashed."""
        warn("W7", f"{shadowed_path}: definition of file:{fdp_name} shadowed by {kept_path}")

    def w_prost(self, file_name: str) -> None:
        """--force-proto2-for-editions: an editions file written as proto2
        (W2). Printed now in detailed mode; else counted in one line."""
        self.count("W2")
        if self._detailed:
            _line("W2", f"'{file_name}' is an editions file; written as proto2 "
                        "(--force-proto2-for-editions)")
        else:
            self._prost.append(file_name)

    def flush(self) -> None:
        """Flush buffered squashed warnings to stderr."""
        if self._detailed:
            return
        suppressed = False
        for code in _SQUASHED_ORDER:
            for message, count in sorted(self._squashed[code].items()):
                _line(code, f"{message} {_occ(count)}")
                if count > 1:
                    suppressed = True
        if self._prost:
            n = len(self._prost)
            noun = "file" if n == 1 else "files"
            _line("W2", f"{n} editions {noun} written as proto2 "
                        "(--force-proto2-for-editions); run with --detailed-warnings "
                        "to list them.")
        if suppressed:
            cli_info("Run with --detailed-warnings to see all warning occurrences.")

    def failed_on(self, codes: list[str]) -> str | None:
        """The --fail-on codes that occurred, `T4 (3 occurrences), W5 (1
        occurrence)` in the codes' order (spec 0438 S4), or None."""
        from ..anomalies import ANOMALIES
        order = [*CODES, *ANOMALIES]
        hit = [c for c in order if c in codes and self.counts[c]]
        if not hit:
            return None
        return ", ".join(f"{c} {_occ(self.counts[c])}" for c in hit)


# Module-level singleton — configured once at startup by reproto.py/cli.py.
_collector: WarningCollector = WarningCollector(detailed=False)


def get_collector() -> WarningCollector:
    return _collector


def configure_collector(detailed: bool) -> None:
    global _collector
    _collector = WarningCollector(detailed=detailed)
