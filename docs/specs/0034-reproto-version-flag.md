<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0034 — reproto: add --version flag

**Status:** implemented
**Implemented in:** 2026-05-02
**App:** reproto

---

## Purpose

Expose the package version string via a `--version` flag so scripts and
users can confirm which build is installed.

---

## Goals

1. `reproto --version` prints `reproto <version>` and exits 0.
2. The version string is read from the package metadata at runtime
   (i.e. from `pyproject.toml` via `importlib.metadata`) — not
   hard-coded in source.
3. No new test fixtures are needed; the flag is covered by Click's
   built-in `version_option` behaviour.

---

## Specification

### `cli.py`

Add `importlib.metadata` to the standard-library imports:

```python
from importlib.metadata import version as _pkg_version
```

Add Click's built-in version option decorator to `main`, directly above
the existing options:

```python
@click.version_option(
    version=_pkg_version('reproto'),
    prog_name='reproto',
)
```

The version is resolved at module load time with a `"dev"` fallback for
development environments where the package is not installed:

```python
try:
    _reproto_version = importlib.metadata.version('reproto')
except importlib.metadata.PackageNotFoundError:
    _reproto_version = 'dev'
```

No changes to `Options`, `Context`, or `main()`'s signature are needed —
`version_option` is handled entirely by Click before `main` is called.

---

## Files changed

| File | Change |
|---|---|
| `cli.py` | Import `importlib.metadata.version`; add `@click.version_option(...)` decorator |
