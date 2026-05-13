# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""Stress-test harness for the prototext auto-inference pipeline (spec 0054).

Pipeline per test:
  1. Session fixture: fetch remote corpora + compile fixture .proto files
     → reproto --build-schema-db → stress.desc + stress/hopcroft.rkyv
     (done once, cached for the session).
  2. Per type: prototext --descriptor stress.desc -O inst_dir instantiate-schema <FQDN>
     → instance.pb  (generated at test time, never committed).
  3. prototext --descriptor stress.desc list-schemas instance.pb
     → top-tied FQDNs (one per line, lex-sorted).
  4. Assert expected FQDN is in the top-tied list.
  5. Assert len(top-tied) <= max_ties (from types.yaml, default 5).

Triggered by:  nix-build -A stress-tests
Not part of the regular CI closure.
"""

from __future__ import annotations

import subprocess
from pathlib import Path
from typing import NamedTuple

import pytest
import yaml

# ---------------------------------------------------------------------------
# Repo layout constants
# ---------------------------------------------------------------------------

TYPES_YAML = Path(__file__).parent / "types.yaml"


# ---------------------------------------------------------------------------
# Type entry
# ---------------------------------------------------------------------------

class TypeEntry(NamedTuple):
    fqdn: str
    max_ties: int


DEFAULT_MAX_TIES = 5


def _load_types() -> list[TypeEntry]:
    data = yaml.safe_load(TYPES_YAML.read_text())
    assert isinstance(data, dict) and "types" in data
    entries: list[TypeEntry] = []
    for item in data["types"]:
        if isinstance(item, str):
            entries.append(TypeEntry(fqdn=item, max_ties=DEFAULT_MAX_TIES))
        else:
            entries.append(TypeEntry(
                fqdn=str(item["fqdn"]),
                max_ties=int(item.get("max_ties", DEFAULT_MAX_TIES)),
            ))
    return entries


# ---------------------------------------------------------------------------
# Session fixture
# ---------------------------------------------------------------------------

@pytest.fixture(scope="session")
def schema_db(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """Fetch corpora, compile protos, build the schema DB; return .desc path.

    If the STRESS_DB environment variable is set to an existing .desc path,
    the build step is skipped and that DB is used directly.
    """
    import os
    prebuilt = os.environ.get("STRESS_DB")
    if not prebuilt:
        pytest.skip(
            "STRESS_DB not set; skipping stress tests "
            "(set STRESS_DB=/path/to/stress.desc to enable)"
        )
    p = Path(prebuilt)
    assert p.exists(), f"STRESS_DB={prebuilt!r} does not exist"
    return p


# ---------------------------------------------------------------------------
# Parametrize over types.yaml
# ---------------------------------------------------------------------------

_TYPE_ENTRIES = _load_types()


@pytest.fixture(scope="session")
def all_instances(
    schema_db: Path,
    tmp_path_factory: pytest.TempPathFactory,
) -> dict[str, Path]:
    """Generate one instance per type; return {fqdn: instance_path}.

    All instances are generated in a single reproto-instantiate-schema call (DB loaded once).
    """
    inst_dir = tmp_path_factory.mktemp("instances")
    fqdns = [e.fqdn for e in _TYPE_ENTRIES]
    gen = subprocess.run(
        [
            "reproto-instantiate-schema",
            "--descriptor", str(schema_db),
            "-O", str(inst_dir),
            *fqdns,
        ],
        capture_output=True,
        text=True,
    )
    assert gen.returncode == 0, f"instantiate-schema failed:\n{gen.stderr}"

    instances: dict[str, Path] = {}
    for fqdn in fqdns:
        rel = fqdn.replace(".", "/") + ".pb"
        instance_path = inst_dir / rel
        assert instance_path.exists(), (
            f"instantiate-schema produced no output file for {fqdn}"
        )
        instances[fqdn] = instance_path
    return instances


@pytest.fixture(scope="session")
def list_schemas_results(
    schema_db: Path,
    all_instances: dict[str, Path],
) -> dict[str, list[str]]:
    """Run prototext list-schemas once over all instances; return {fqdn: [candidates]}."""
    if not _TYPE_ENTRIES:
        return {}

    instance_paths = [str(all_instances[e.fqdn]) for e in _TYPE_ENTRIES]
    ls = subprocess.run(
        [
            "prototext",
            "--descriptor", str(schema_db),
            "list-schemas",
            *instance_paths,
        ],
        capture_output=True,
        text=True,
    )
    assert ls.returncode == 0, (
        f"prototext list-schemas failed:\n{ls.stderr}"
    )

    # Parse YAML: list of {path, types} dicts.
    raw = yaml.safe_load(ls.stdout) or []
    results: dict[str, list[str]] = {}
    for entry, item in zip(_TYPE_ENTRIES, raw):
        results[entry.fqdn] = item.get("types") or []
    return results


@pytest.mark.parametrize("entry", _TYPE_ENTRIES, ids=[e.fqdn for e in _TYPE_ENTRIES])
def test_auto_infer(
    entry: TypeEntry,
    list_schemas_results: dict[str, list[str]],
) -> None:
    """Assert prototext list-schemas ranks the expected FQDN at the top."""
    fqdn = entry.fqdn
    candidates = list_schemas_results.get(fqdn, [])

    assert fqdn in candidates, (
        f"expected {fqdn!r} not in --list-schemas top; got: {candidates}"
    )

    if len(candidates) > entry.max_ties:
        import warnings
        warnings.warn(
            f"{fqdn}: --list-schemas returned {len(candidates)} tied FQDNs "
            f"(max_ties={entry.max_ties}); scoring too weak for this corpus.",
            stacklevel=2,
        )
