<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0021 — Variant bundle layout: annotation modules and uniform resource loading

**Status:** implemented
**App:** reproto
**Implemented in:** 2026-05-01

---

## Problem

### P1 — No convention for annotation module files in a variant bundle

The `annotation_modules` key (spec 0018) lists Python modules that reproto
imports at startup via `importlib.import_module()`.  The modules are expected
to be importable from the existing Python path.

For a variant delivered as an external YAML file, the associated `*_pb2.py`
files naturally live alongside the `.pb` fallback files in the variant's
resource directory (the sibling directory `<variant-stem>/` next to the YAML
file).  But that directory is not on `sys.path` by default, so the import
fails.

There is no documented convention for where annotation module files should
reside relative to the variant YAML, and no mechanism for reproto to make them
importable automatically.

### P2 — The built-in variant is a special case

`variant.py` stores `variant_file = None` for the built-in `google-protobuf`
variant, and `load_embedded_proto_fallback()` in `reproto.py` branches on
`ctx.variant_file is None` to choose between two completely different loading
strategies:

- `None` → traverse `reproto.variants.<stem>/…` via `importlib.resources`.
- path → read `<variant_file_dir>/<stem>/<pb_name>` from the filesystem.

The result is duplicated logic and a `None` sentinel visible at every call
site.  The built-in variant should be treated as an ordinary variant: the same
code loads resources for both, working transparently whether the package is
installed as a directory or inside a zip/wheel.

---

## Goals

1. Define a convention: annotation module files live under the variant's
   resource root (the `<variant-stem>/` sibling directory), following their
   Python package path.
2. When `annotation_modules` is non-empty, automatically prepend the resource
   root to `sys.path` before importing the modules.  This applies to all
   variants uniformly.
3. A variant directory tree containing both `.pb` and `*_pb2.py` files can be
   zipped and used by another reproto installation with no extra setup.
4. Replace `variant_file: str | None` with `variant_root: Traversable` — a
   single field that points to the parent directory of the variant YAML for
   both built-in and external variants, using `importlib.resources` throughout.
   Remove the `None` sentinel and the two-branch dispatch in
   `load_embedded_proto_fallback()`.

---

## Non-goals

- Supporting annotation modules installed as regular Python packages (already
  works today via the existing Python path).
- Supporting absolute filesystem paths in `annotation_modules` (Python import
  names only).
- Automating zip creation or distribution.

---

## Specification

### 1. Convention: annotation files in the resource root

For a variant whose YAML is `<stem>.yaml`, the resource root is the sibling
directory `<stem>/`.  Annotation module files must reside at:

```
<stem>/<module/path/as/directories>/<module_leaf>.py
```

For example, a module named `foo.bar.baz_pb2` must be at:

```
<stem>/foo/bar/baz_pb2.py
```

This mirrors Python's standard package layout and allows
`sys.path.insert(0, resource_root)` to make all listed modules importable in
one step.  Python 3.3+ namespace packages do not require `__init__.py` files;
the resource root may contain a single `__init__.py` as a cosmetic marker, but
this is not required for correctness.

No `pb/` or `py/` subdirectory is introduced.  `.pb` and `*_pb2.py` files
share the same flat namespace; no basename collision is possible because the
suffixes differ.

This convention applies to the built-in variant identically: any future
`*_pb2.py` files for the built-in `google-protobuf` variant would live under
`reproto/src/reproto/variants/google-protobuf/` as package data alongside the
existing `.pb` files.

### 2. `variant_root: Traversable` replaces `variant_file: str | None`

In `context.py`, replace:

```python
variant_file: str | None = None
variant_stem: str = 'google-protobuf'
```

with:

```python
from importlib.resources.abc import Traversable

variant_root: Traversable   # parent dir of the variant YAML (no default)
variant_stem: str = 'google-protobuf'
```

`variant_root` is a `Traversable` pointing to the directory that contains
`<stem>.yaml` and the `<stem>/` resource subdirectory.  For the built-in
variant this is `importlib.resources.files('reproto.variants')`; for an
external variant it is `importlib.resources.files(Path(path).parent)`.

Both are `Traversable` instances.  All subsequent resource access goes through
`variant_root.joinpath(...)`, making the loading code identical for both cases.

### 3. Uniform resource loading in `variant.py`

Replace `_load_builtin()` and the two-branch `load()` with a single path:

```python
def load(path: str | None = None) -> dict:
    resolved = path or os.environ.get('REPROTO_VARIANT')
    if resolved is None:
        root: Traversable = importlib.resources.files('reproto.variants')
        stem = 'google-protobuf'
    else:
        root = importlib.resources.files(Path(resolved).parent)
        stem = Path(resolved).stem
    text = root.joinpath(f'{stem}.yaml').read_text(encoding='utf-8')
    raw = yaml.safe_load(text)
    return _parse(raw if isinstance(raw, dict) else {}, root, stem)
```

`_parse()` receives `root` and `stem` instead of a `resolved` path string.
It stores them as `variant_root` and `variant_stem` in the returned dict, and
no longer needs any `None`-guarded branch.

### 4. Uniform `.pb` loading in `load_embedded_proto_fallback()`

In `reproto.py`, the function becomes:

```python
pb_name = proto_name[:-len('.proto')] + '.pb'
node: Traversable = ctx.variant_root
for part in [ctx.variant_stem] + pb_name.split('/'):
    node = node.joinpath(part)
data = node.read_bytes()
```

No `if ctx.variant_file is None` branch.  The `Traversable` API handles both
directory and zip/wheel installs transparently.

### 5. `sys.path` injection in `import_annotations()`

Change `import_annotations()` in `reproto.py` to accept the resource root
path as an optional argument:

```python
def import_annotations(modules: list[str], resource_root: str | None = None) -> None:
    """Import annotation modules declared by the active variant.

    If resource_root is given and not already on sys.path, it is prepended
    before any import is attempted.  This allows *_pb2.py files that live
    inside the variant bundle to be found without any extra setup.

    Does nothing when modules is empty.  Logs a warning for each module
    that cannot be imported, but continues execution.
    """
    if not modules:
        return
    if resource_root is not None and resource_root not in sys.path:
        sys.path.insert(0, resource_root)
    for full_module_name in modules:
        try:
            importlib.import_module(full_module_name)
            cli_info(f"Module '{full_module_name}' imported successfully.")
        except ModuleNotFoundError:
            cli_warning(f"Module '{full_module_name}' not found.")
```

The call site in `reproto()` becomes:

```python
resource_root = str(ctx.variant_root.joinpath(ctx.variant_stem))
import_annotations(ctx.variant_annotation_modules, resource_root)
```

This is identical for built-in and external variants.  For the built-in
variant `annotation_modules` is `[]`, so `import_annotations` returns
immediately without touching `sys.path`.  For a zip/wheel install `str()` on a
`Traversable` may not yield a usable filesystem path; this is acceptable
because Python packages that contain importable `.py` modules must be installed
as directories in practice.

### 6. `sys.path` entry lifetime

The inserted path entry persists for the lifetime of the process.  This is
acceptable: reproto is a short-lived CLI tool, not an embedded library.

### 7. Example bundle layout

A hypothetical variant `acme` would be distributed as:

```
acme.yaml
acme/
  __init__.py                ← cosmetic only
  google/protobuf/
    descriptor.pb
    any.pb
    ...
  com/acme/proto/
    options_pb2.py
```

The corresponding `acme.yaml` would use identity mappings in `well_known` so
that reproto loads the `.pb` files from the bundle rather than falling back to
the protoc installation:

```yaml
well_known:
  google/protobuf/any.proto: google/protobuf/any.proto
  ...
```

The recipient places `acme.yaml` and `acme/` in the same directory and passes
`--proto_variant /path/to/acme.yaml`.  reproto finds the `.pb` files and
imports `com.acme.proto.options_pb2` without any further configuration.

---

## Test coverage

- Existing roundtrip tests pass unchanged (`annotation_modules: []`, no
  `sys.path` modification).
- A new unit test for `import_annotations()` verifies that `resource_root` is
  prepended to `sys.path` when supplied and not already present, and that a
  second call with the same root does not add a duplicate entry.
- A new unit test for `variant.load()` (no arguments) verifies that
  `variant_root` is a `Traversable`, that `variant_stem` is
  `'google-protobuf'`, and that `variant_root.joinpath('google-protobuf.yaml')`
  is readable.

---

## Open questions

- Should reproto log the `sys.path` insertion at `--debug` level?

---

## References

- Spec 0018 — `annotation_modules` variant YAML key and `import_annotations()`.
- Spec 0001 §8a–8b — variant resource directory convention and `.pb` loading.
