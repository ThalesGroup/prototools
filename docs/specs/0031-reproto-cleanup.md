<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0031 — reproto code-quality cleanup

**Status:** in progress (rounds 1–3 implemented 2026-05-02/03)
**App:** reproto

---

## Purpose

Address the findings from the code review documented in `reproto/REVIEW.md`.
The work is split into six sequential rounds, ordered so that each round
creates the preconditions for the next.

---

## Goals

Eliminate code duplication, non-idiomatic constructs, misleading comments,
dead code, and oversized files — without any change in observable behaviour.

---

## Non-goals

- Changing public API surface (`__all__`, `reproto()` signature, CLI flags).
- Adding new features.
- Changing test fixtures or test logic (except where a refactor moves a
  helper that tests import).

---

## Specification

### Round 1 — Trivial, zero-risk cleanup

No design decisions. All changes are mechanical and have no behaviour impact.

1. **Remove unused `Contents` class** (`topology.py`).
2. **Remove unused `_ExtensionFieldDescriptor = Any` alias**
   (`option_renderers.py`).
3. **Remove dead match arms** after the scalar catch-all arm in
   `ReFieldDescriptor.dump_option` (`simple_types.py:188–198`).
   After `case int() | bool() | float() | str() | bytes():` matches and
   returns, the subsequent individual arms `case bool():`, `case int() |
   float():`, `case str():`, `case bytes():` are unreachable.
4. **Replace `assert False`** with `raise AssertionError(...)` in
   `topology.py` and `load.py`.
5. **Replace `dict()` / `list()` constructors** with `{}` / `[]` where
   used for empty literals (`context.py`, `topology.py`).
6. **Fix 2-space indentation** in `option_messages.py` (whole-file;
   use 4 spaces consistently with the rest of the codebase).
7. **Remove redundant `else` after `continue`** in
   `re_descriptor.py:164–167` and `re_descriptor.py:526–529`.
8. **Remove commented-out code blocks:**
   - `reproto.py`: all `# if ... is_pruned` / `# continue` lines
   - `load.py`: all `#if is_pruned` blocks
   - `topology.py:122–125`: the `# ...` placeholder comment
   - `simple_types.py:459–461`: the commented-out `filter_out` block
9. **Fix stale / misleading comments:**
   - `re_simple.py` module docstring: reframe as a permanent facade,
     not a "compatibility wrapper".
   - `base.py:313`: remove the `(Moved from ProtoMessageDelegateMixin)`
     parenthetical.
   - `base.py:96–99`: remove the historical "Benefits over the previous
     mixin approach" bullet list from the `NodeBase` docstring.
   - `reproto.py:647`: remove the `# Ruff does not like lambda` comment.
   - `simple_types.py:386`: replace `# TODO: Needs factoring???` with a
     proper comment or remove it.
   - Remove verbose no-op docstrings from trivial delegating properties
     in `re_file.py`, `re_descriptor.py`, `re_field.py`, `re_enum.py`.
10. **Inline `resource_root`** in `reproto.py:418` — it is used on the
    very next line and the variable name adds no clarity over the
    expression.
11. **Remove duplicate `seed_files` type annotation** in `reproto.py`
    (declared twice in the two branches of `if/else`; declare once
    before the `if`).

### Round 2 — Factor shared utilities

Extract recurring patterns into named helpers. Each helper's signature is
forced by the existing call sites; no design judgement is needed.

1. **Walk-to-file-root helper** — extract the pattern that walks
   `_parent` pointers until a `ReFileDescriptorProto` is found. Used in
   `re_descriptor.py:render_extensions` (lines 100–104),
   `re_descriptor.py:render` (lines 444–448), and
   `re_field.py:_get_file_and_msg` (lines 32–41). Place in `utils.py`
   as `get_file_node(node) -> ReFileDescriptorProto`.
2. **`_fuzzy_suggest` helper** — extract the `best_match` / `fuzz.ratio`
   loop that appears identically in the pruning phase (reproto.py ~870)
   and the seeding phase (~960). Signature:
   `_fuzzy_suggest(pattern: str, nodes: dict) -> str | None`.
3. **`_find_matching_nodes` helper** — extract the `is_pattern` /
   `fnmatch.fnmatch` / `ctx.find_node` / `matched_nodes` scaffold that
   appears in both the pruning and seeding phases. Signature:
   `_find_matching_nodes(pattern: Fqdn, ctx: Context) -> list[tuple[Fqdn, Node]]`.
4. **Eliminate `ReFile.from_name`** — `ReFile.from_name(topo, name)` is
   identical to `ReFile(topo, name)` (both go through `__new__`'s
   registry lookup). Replace all call sites with the direct constructor
   call and delete `from_name`.

### Round 3 — Eliminate the `Re*Options` class family  *(needs dedicated spec)*

The six per-type option renderer classes in `option_renderers.py`
(`ReEnumOptions`, `ReMessageOptions`, `ReServiceOptions`,
`ReMethodOptions`, `ReFieldOptions`, `ReExtensionRangeOptions`) duplicate
logic already present in `render_options_from_message` and `ReOptions`.
A dedicated spec must:

- Audit all call sites and document which rendering path each uses today.
- Define the unified interface (which of the two paths wins).
- Map the `filter_out` mechanism to the `exclude` parameter of
  `render_options_from_message`.
- List every file that changes.

### Round 4 — Deduplicate oneof and extend-block rendering  *(borderline; spec recommended)*

1. Delete the dead `render_oneofs` method (`re_descriptor.py:141–192`);
   the identical logic lives inline in `render`.
2. Deduplicate the extend-block grouping loop between
   `re_file.py:render` and `re_descriptor.py:render_extensions`. A spec
   should document which orphan-tracking behaviour is canonical before
   touching this.

### Round 5 — Split long files  *(needs dedicated spec)*

- `reproto.py` (1 202 lines): extract each of the 7 phases into a named
  internal function; the `reproto()` entry point becomes a readable
  index.
- `base.py` (622 lines): move the three options-rendering methods
  (`render_options_from_message`, `render_options`,
  `format_composite_options`) to module-level functions that accept a
  `NodeBase` instance.
- `simple_types.py` (474 lines): split `ReFieldDescriptor` and its
  `dump_option` / `get_scalar` machinery into a separate module.
- `re_descriptor.py` (595 lines): move the source-code-info helpers
  (`render_message_comments`, `_calculate_source_code_info_path`) to a
  dedicated module or mixin.

A dedicated spec must define the new module layout, import structure, and
whether `re_simple.py`-style re-export shims are kept for each split.

### Round 6 — Behavioural correctness items  *(needs dedicated spec)*

- Fix `filter_out: list[str] = []` mutable default argument (six
  occurrences in `option_renderers.py`).
- Reconsider the `HasField` property on `NodeBase` that returns a
  callable instead of being a real method.
- Replace `is_ref()` sentinel (`hasattr`) with `_qfile: QualFile | None`
  and an `is None` test.
- Investigate and fix (or document as intentional) the `shorten_type_name`
  edge case where `do_more=False` from a recursive call does not prevent
  further prefix stripping (REVIEW.md §6.6).

A dedicated spec must include test cases that pin the intended behaviour
before any code changes.

---

## Implementation order rationale

Each round depends on the previous:
- Round 1 eliminates noise so Round 2 diffs are easier to review.
- Round 2 provides shared utilities that Round 3 and 4 rely on.
- Round 3 must complete before Round 4 (extend/oneof deduplication
  references the unified options path).
- Round 5 (file splitting) is easier after Round 3–4 reduce file sizes.
- Round 6 (behavioural) goes last because it may need new tests written
  before the code changes.
