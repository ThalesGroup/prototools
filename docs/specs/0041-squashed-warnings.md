<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# 0041 — reproto: warning clarity and squashed mode

**Status:** implemented
**Implemented in:** 2026-05-08 (initial); 2026-05-09 (§6, §7, §8 + TC-P*/TC-A*/TC-B*/TC-C*/TC-D* tests)
**App:** reproto

---

## Background

When running reproto against a large corpus (e.g. a google3 checkout with
`--use-variant all`), thousands of warnings are emitted to stderr.  A sample
run produced 10,309 lines for a corpus where only 24 distinct source files
were missing from the `-I` path.  The explosion comes from per-field
repetition: a single missing type can fire once for every field that carries
an option referencing it.

This spec addresses two orthogonal but related concerns:

1. **Warning clarity** — some messages are unclear or carry Python
   implementation noise; fixing them benefits all modes.
2. **Squashed mode** — by default, repeated warnings are collapsed to one line
   per root cause; `--detailed-warnings` restores the full per-occurrence
   stream.

---

## Warning taxonomy

Five distinct warning classes have been identified.  W4 and W5 are kept
distinct because they surface different actionable information: W4 names a
type FQDN (useful for searching the codebase), W5 names a file path (directly
tells the user what to add to `-I`).

### W1 — Missing source file

Emitted during the "Discovering and loading imported files" phase for each
`.proto` that could not be found on the `-I` search path.

Currently fires **two lines per file** — always together, never separately.
The first comes from `load.py` (returns an empty list); the second from
`phases.py` (observes the empty result):

```
File 'third_party/envoy/.../address.proto' is missing, skipping
Skipping unreadable file: third_party/envoy/.../address.proto
```

24 distinct missing files → 48 lines in the sample run.

### W2 — Embedded fallback used

Informational notice emitted when reproto substitutes a built-in copy of a
well-known proto (e.g. `google/protobuf/timestamp.proto`):

```
Using embedded fallback: google/protobuf/timestamp.proto
```

7 lines in the sample run.  Not a warning — an informational status line.

### W3 — Duplicate file / symbol conflict

When two `.proto` files in the corpus define the same fully-qualified symbol,
Python's `descriptor_database` fires one `RuntimeWarning` per conflicting
symbol.  The subsequent `pool_db.Add()` call raises a `TypeError`, which
reproto catches.

#### Two-pool architecture

`ctx.pool_db` is a pure-Python `DescriptorDatabase` — a keyed store of
`FileDescriptorProto` objects, indexed by file name and symbol name.
`ctx.pool` is the C-extension `DescriptorPool`, constructed as
`DescriptorPool(pool_db)`.  This makes `pool_db` the *backing store*: when
`ctx.pool` is asked for a file or symbol it has not yet built into a live
`FileDescriptor`, it fetches the raw `FileDescriptorProto` from `pool_db`
and builds it on demand.  `pool_db.Add()` therefore serves two purposes:

1. **Backing store for lazy pool resolution** — `ctx.pool.FindFileByName()`
   falls back to `pool_db` on a cache miss, triggering deferred descriptor
   construction.
2. **Raw FDP retrieval in phase 3** — `ctx.pool_db.FindFileByName(desc.name)`
   (line 623) retrieves the original `FileDescriptorProto` for building the
   FQDN graph, bypassing the C pool entirely.

#### How the duplicate symbol problem actually works for binary files

The flow for a conflicting pair of `.pb` files is:

1. File A is processed first.  `pool_db.Add(fdp_A)` registers all of A's
   symbols in `pool_db._file_desc_protos_by_symbol`.  No error.
2. File B is processed.  `pool_db.Add(fdp_B)` iterates B's symbols; for each
   one already present it emits a `RuntimeWarning` (via `_AddSymbol`) and
   then **overwrites** the entry — pointing it at B's FDP.  No exception is
   raised from `pool_db.Add()`; the `try/except TypeError` in reproto's binary
   path is **never triggered** by this.  Both files are now in `pool_db`.
3. During rendering, `ctx.pool.FindFileByName('A')` succeeds (A's FDP is in
   `pool_db`; the C pool builds A's `FileDescriptor` and caches it, claiming
   the symbols as belonging to A).
4. `ctx.pool.FindFileByName('B')` is called for rendering B.  The C pool
   tries to build B's `FileDescriptor` from B's FDP (retrieved from
   `pool_db`), but the symbols `pkg.MyMessage` etc. are already registered in
   the C pool's internal symbol table as belonging to A.  The C extension
   raises `TypeError: Couldn't build proto file into descriptor pool:
   duplicate symbol '...'`.  This is the exception caught by the
   `try/except (... TypeError ...)` in `re_file.py` (anomaly A3) and the
   fallback in `phases.py:862`.

So the `RuntimeWarning` storm from `pool_db.Add()` and the `TypeError` during
rendering are **two separate events** from two separate phases — not cause and
effect of the same call.  The `try/except TypeError` in the binary loading
loop (lines 554, 567) is defensive code for a *different* class of TypeError
(e.g. from `phase2_plugin` or `patch_go_package`) and is **not** what catches
the duplicate-symbol failure.

#### `pool_db` symbol-index internals and pre-check cost

`DescriptorDatabase.Add()` indexes symbols by iterating `message_type`,
`enum_type`, `extension`, and `service` fields of the `FileDescriptorProto`,
recursing into nested types via `_ExtractSymbols`.  The `RuntimeWarning`
fires inside `_AddSymbol` for each already-present symbol, then the entry is
overwritten.

A pre-check for `--keep-duplicates` would perform the same iteration and
query `pool_db.FindFileContainingSymbol()` for each symbol *before* calling
`pool_db.Add()`.  The cost is O(S) dict lookups where S is the number of
symbols — the same work `Add()` already does.  Roughly a 2× constant factor
on the symbol-scan phase per file.

In practice this is negligible: typical `.proto` files have tens to low
hundreds of symbols, and the total symbol count is bounded by the corpus size
which reproto already fully iterates in phase 2.  There is no algorithmic
complexity increase; the overhead is unmeasurable against I/O and protobuf
parsing costs.

#### Upstream detection option

A `--keep-duplicates` mode can eliminate both the `RuntimeWarning` storm and
the deferred rendering `TypeError`.

The key insight is that `qf.desc` — already populated by `parse_qfile` in
phase 1 for both binary and text-format files — contains the pool-free
`FileDescriptorProto` with `package`, `message_type`, `enum_type`,
`extension`, and `service` fully populated (these are standard fields, not
extensions, so they parse correctly without a pool).  Symbol extraction
requires no additional parsing.

Before calling `pool_db.Add()` for a file, reproto iterates all symbols the
file defines and queries `pool_db.FindFileContainingSymbol()` for each.  That
method returns the `FileDescriptorProto` of the already-registered file
(giving its `.name` directly), or raises `KeyError` if not present.  All
conflicts are collected — not just the first — to produce a complete picture:

```python
conflicts: dict[str, str] = {}  # symbol → conflicting file name
for sym in extract_symbols(fdp):
    try:
        existing = pool_db.FindFileContainingSymbol(sym)
        conflicts[sym] = existing.name
    except KeyError:
        pass
```

If `conflicts` is non-empty, the file is marked `is_pruned = True` and
`pool_db.Add()` is skipped — preventing both the `RuntimeWarning` storm and
the C-pool `TypeError` at render time.  For text-format files, the phase 2
re-parse with the pool is also skipped, preventing `ctx.pool` corruption.

The warning groups conflicts by the file they originate from:

```
Warning: file:src/proto/grpc/channelz/channelz.proto pruned —
    duplicate symbols with file:third_party/grpc_proto/grpc/channelz/v1/channelz.proto
    (42 symbols)
```

If symbols conflict with more than one file (unusual but possible):

```
Warning: file:foo.proto pruned — duplicate symbols with:
    file:bar.proto (12 symbols)
    file:baz.proto (3 symbols)
```

The file that loaded first (topo-sort order) wins.  Well-known type fallbacks
are loaded first as leaf files, so they always win over any user file
redefining them — correctly.

In the sample run: 42 `RuntimeWarning` lines for a single file pair
(`channelz`), totalling 47 lines across 4 conflicting files.

```
RuntimeWarning: Conflict register for file "src/proto/grpc/channelz/channelz.proto":
    grpc.channelz.v1.Channel is already defined in
    "third_party/grpc_proto/grpc/channelz/v1/channelz.proto"
...  (one line per conflicting symbol)
'src/proto/grpc/channelz/channelz.proto': failed to render file options:
    TypeError: Couldn't build proto file into descriptor pool:
    duplicate symbol 'grpc.channelz.v1.Channel'
```

The `RuntimeWarning` lines come from the protobuf library; they can be
intercepted via a custom `warnings.showwarning` hook to participate in
squashing even when `--keep-duplicates` is not active.

### W4 — Option rendering failure: unresolvable type

When a field or file carries a custom option whose type cannot be resolved
(because the defining `.proto` was not loaded), reproto currently fires at
three levels, from two separate call sites:

- **File options** (anomaly A3, emitted from `re_file.py`) — one per affected
  file
- **Field options** (anomaly C5, emitted from `re_field.py`) — one per
  affected field in that file
- **File summary** (`phases.py:862`) — one per affected file, if the rendering
  exception propagates past the anomaly handler

85 distinct unresolvable type names in the sample run.  The top offender
(`.net_model_unm.proto.DrainType`) fired **2,147 times**, all traceable to
a single missing dependency.  Total: ~9,800 lines.

The `TypeError: Couldn't build proto file into descriptor pool:` prefix is a
Python implementation detail leaking into user-facing output.  Field-level
messages do not identify which `.proto` file the field belongs to.

Note on FQDN format: in reproto's internal FQDN scheme, type descriptors are
written as `desc:.some.package.TypeName` (prefix `desc:`).  The unresolvable
name appearing in W4 messages (e.g. `.net_model_unm.proto.DrainType`) is the
raw protobuf type reference, not a reproto FQDN — it has no `desc:` prefix.
The squashed warning format should present it as-is, since that is what the
user would search for.

### W5 — Option rendering failure: missing file dependency

Same call sites as W4 (A3 in `re_file.py`, C5 in `re_field.py`), but the
underlying error names the missing file rather than a missing symbol:

```
field 'name': failed to render options: TypeError: Couldn't build proto file
    into descriptor pool: Depends on file
    'third_party/protoc_gen_validate/validate/validate.proto',
    but it has not been loaded
```

11 distinct missing files; the top offender fired 341 times.  Total: ~684
lines.

W4 and W5 originate at the same code locations but carry different actionable
information:

- W4 names a **type reference** (e.g. `.net_model_unm.proto.DrainType`) —
  useful for grepping the codebase to find where the type is defined.
- W5 names a **file path** (e.g. `third_party/.../validate.proto`) — directly
  tells the user what to add to `-I`.

They are kept as separate warning kinds in both detailed and squashed output.
In reproto FQDN terms, a missing file would be `file:third_party/.../validate.proto`;
the squashed warning should use that form for consistency with `--seed` / `--prune`.

---

## Goals

1. Improve warning message clarity for all five classes in all modes.
2. Make squashed mode the default: each distinct root cause is shown once,
   with an occurrence count.
3. Provide `--detailed-warnings` to restore the original per-occurrence stream.
4. In squashed mode, flush a summary hint at the end when warnings were
   suppressed.
5. By default, prune duplicate-symbol files upfront (before pool population);
   provide `--keep-duplicates` to revert to the legacy behaviour.

---

## Non-goals

- Suppressing warnings entirely (a separate `--quiet` concern).
- Resolving missing dependencies automatically.
- Changing the set of conditions that produce warnings.

---

## Specification

### 1. Warning message clarity (applied in all modes)

**W1:** The two-line pair collapses to one line.  The `load.py` warning is
kept; the `phases.py` `"Skipping unreadable file: ..."` line is removed
(it is redundant — `load.py` already fired for the same file).

W1 and W5 are merged into a single counter (see §8).

**W2:** Keep as-is.  These are useful status lines, not warnings.

**W3:** The per-symbol `RuntimeWarning` lines from `descriptor_database` are
intercepted via a `warnings.showwarning` hook and fed into reproto's warning
collector (see §4), rather than being printed directly.  This removes the
`/nix/store/.../descriptor_database.py:152: RuntimeWarning:` prefix, which
gives the user a path into reproto's internal call site and is not actionable.

Reproto's own `"Could not add descriptor from '...' to pool: ..."` line is
replaced with a single line per conflicting file pair:

```
Warning: file:src/proto/grpc/channelz/channelz.proto conflicts with
    file:third_party/grpc_proto/grpc/channelz/v1/channelz.proto (file skipped)
```

**W4/W5:** Strip the `TypeError: Couldn't build proto file into descriptor
pool:` prefix.  Add the containing `.proto` file to field-level messages.
W4 and W5 keep distinct formats to preserve their different actionable content:

```
# W4 — before
field 'device_name': failed to render options: TypeError: Couldn't build proto
    file into descriptor pool: couldn't resolve name '.net_model_unm.proto.DrainType'

# W4 — after
Warning: 'net/model/foo.proto' field 'device_name': unresolvable type '.net_model_unm.proto.DrainType'

# W5 — before
field 'name': failed to render options: TypeError: Couldn't build proto file
    into descriptor pool: Depends on file 'third_party/.../validate.proto',
    but it has not been loaded

# W5 — after
Warning: 'net/model/foo.proto' field 'name': missing file dependency
    'third_party/protoc_gen_validate/validate/validate.proto'
```

### 2. Default mode (squashed)

Warnings are buffered during the run and flushed at the end, grouped by root
cause.  Each distinct root cause appears exactly once, with an occurrence
count.  If an error or unhandled exception occurs, the buffer is flushed
before exiting so warnings are not lost.

#### Flush order

Categories are printed in decreasing order of actionability — the most
directly fixable problems appear first:

1. **W5** — missing file dependencies (merged with W1, see §8): the user
   can immediately add the named file to `-I`.
2. **W4** — unresolvable types: requires grepping the codebase to identify
   which file to add; one step less direct than W5.
3. **W6** — option rendering failures ("Couldn't find Extension/message"):
   a rendering-level degradation, typically a consequence of a missing file
   already reported by W5; least actionable on its own.

**W2** is displayed as emitted (not buffered; no repetition).

**W3** is printed immediately as emitted, in both squashed and detailed modes
(one occurrence per pruned file; no buffering needed).

#### Format

Each squashed line always includes an occurrence count, even for N=1, so
that the flush block is visually distinct from immediate emissions:

**W5 squashed:**
```
Warning: missing dependency file:third_party/protoc_gen_validate/validate/validate.proto (341 occurrences)
Warning: missing dependency file:third_party/envoy/.../base.proto (164 occurrences)
```

**W4 squashed:**
```
Warning: unresolvable type .net_model_unm.proto.DrainType (2147 occurrences)
Warning: unresolvable type .net_model_unm.proto.Size (613 occurrences)
```

**W6 squashed:**
```
Warning: 'security/loas/.../thinmint_cert_extensions.proto' field 'message_set_extension': "Couldn't find Extension 43796541" (1 occurrence)
```

After the squashed block, if any warnings had more than one occurrence, append:

```
Run with --detailed-warnings to see all warning occurrences.
```

### 3. Detailed mode (`--detailed-warnings`)

Each warning is printed immediately as it is emitted, in the format described
in §1.  No buffering, no summary.  Behaviour is identical to today's output,
but with the cleaner message format.

### 4. Duplicate pruning (default) and `--keep-duplicates`

By default, reproto checks each file for symbol conflicts before calling
`pool_db.Add()`.  The check reads directly from `qf.desc` — the pool-free
`FileDescriptorProto` already populated in phase 1 for both binary and
text-format files — so no additional parsing is required.

All conflicts are collected by iterating every symbol the file defines and
calling `pool_db.FindFileContainingSymbol()` for each.  The result is a map
from symbol to the name of the already-registered file.  If any conflicts are
found, the file is marked `is_pruned = True`, `pool_db.Add()` is skipped, and
for text-format files the phase 2 re-parse with `ctx.pool` is also skipped.
This prevents both the per-symbol `RuntimeWarning` storm and the deferred
C-pool `TypeError` at render time.

W3 warnings are printed immediately as they occur (during pool-building, before
rendering), in both squashed and detailed modes — there is no per-field
explosion, so buffering adds no value:

```
Warning: file:src/proto/grpc/channelz/channelz.proto pruned —
    duplicate symbols with file:third_party/grpc_proto/grpc/channelz/v1/channelz.proto
    (42 symbols)
```

If symbols conflict with more than one file (unusual but possible in a large
corpus):

```
Warning: file:foo.proto pruned — duplicate symbols with:
    file:bar.proto (12 symbols)
    file:baz.proto (3 symbols)
```

The file that loaded first (topo-sort order) wins.  Well-known type fallbacks
are loaded first as leaf files and therefore always win over any user file
redefining them.

`--keep-duplicates` disables duplicate pruning entirely, reverting to the
legacy behaviour: both conflicting files are added to `pool_db`, producing
the per-symbol `RuntimeWarning` storm and the deferred C-pool `TypeError` at
render time.  This flag exists primarily for debugging or to observe legacy
behaviour.

### 5. Implementation notes

- Introduce a `WarningCollector` class.  In squashed mode it buffers warnings
  keyed by `(category, root_cause)`; in detailed mode it prints immediately.
- Register it at startup; expose a `ctx.warn(category, root_cause, detail)`
  call site used throughout rendering code.  The two existing anomaly call
  sites (A3 in `re_file.py`, C5 in `re_field.py`) and the file-summary site
  (`phases.py:862`) are updated to route through `WarningCollector`.
- Intercept `descriptor_database` `RuntimeWarning` lines via
  `warnings.showwarning` so they participate in squashing.
- Flush the buffer: (a) at normal end-of-run, after "Writing reconstructed
  .proto files."; (b) in `except`/`finally` blocks on abnormal exit.

### 6. Log output improvements (follow-up)

A second corpus run (`--proto-variant ... --use-variant all`) revealed four
remaining output quality issues.

#### 6.1 C5 field-option warnings not routed through `WarningCollector`

The "Couldn't find Extension/message" lines emitted during field-option
rendering (anomaly C5, `re_field.py`) currently bypass `WarningCollector`
and are printed raw to stderr as they occur.  In the sample run this
produces ~60 unsorted lines interleaved mid-phase-7, e.g.:

```
'security/loas/l2/internal/certs/thinmint_cert_extensions.proto' field 'message_set_extension': failed to render options: "Couldn't find Extension 43796541"
'security/loas/l2/internal/certs/thinmint_cert_extensions.proto' field 'message_set_extension': failed to render options: "Couldn't find Extension 43673850"
...  (12 more lines for the same file)
```

**Fix:** Route C5 through `WarningCollector` using a new `w6()` method (to
keep it distinct from W4/W5 which originate from the option-type resolution
path).  The squash key is `(proto_file, error_description)`.  In squashed
mode, all C5 lines are deferred and printed sorted at flush time, one line
per distinct `(file, error)` pair with an occurrence count.  In
`--detailed-warnings` mode, each fires immediately as before.

Squashed format:

```
Warning: 'security/loas/l2/internal/certs/thinmint_cert_extensions.proto' field 'message_set_extension': Couldn't find Extension 43796541
```

Since each `(file, field, extension-id)` combination is already unique, the
occurrence count for C5 will typically be 1 — it is included for consistency
but not printed when it equals 1 (matching W1/W4/W5 behavior).

#### 6.2 W5 warnings for importers of pruned files

In the sample run, `file:privacy/rollouts/proto/request_context_enforceable_mode_config.proto`
generates **850 W5 occurrences** solely because it was pruned by
duplicate-symbol detection.  Its importers render it as an orphan import
and each one fires W5 when the file is not found in `pool_db`.

This is noise generated by reproto's own pruning, not by a genuine user
error.  Files that were pruned (whether by `--prune` or duplicate-symbol
detection) should be silently skipped when generating W5: if reproto itself
decided to exclude the file, its absence from `pool_db` is expected and
should not be reported.

**Fix:** Before emitting W5, check whether the missing dependency's FQDN
(`file:<name>`) corresponds to a pruned node.  If so, suppress the W5
entirely.  Both squashed and detailed modes respect this suppression — the
warning is never generated, not merely hidden in the summary.

#### 6.3 `Skipping descriptor.proto` indentation

The line:

```
  Skipping net/proto2/proto/descriptor.proto (use --emit-descriptor to include)
```

is indented with two leading spaces, making it look visually subordinate
to the preceding "Writing reconstructed .proto files." phase header.  This
is inconsistent with all other `cli_info` lines which carry no indentation.

**Fix:** Remove the two leading spaces from the format string in `phases.py`.

#### 6.4 Squashed output should be sorted

In the current implementation, W3 warnings are printed in topo-sort order
(the order files are processed in phase 2) and W1/W4/W5 are sorted
alphabetically at flush time.  C5 (§6.1 above) should also be sorted at
flush time.

Additionally, the W1 single-occurrence format currently prints:

```
Warning: missing file 'third_party/foo.proto' (not found on -I path; skipped)
```

while the multi-occurrence format prints `(N occurrences)`.  For
consistency, single-occurrence lines should also carry `(1 occurrence)` —
or, equivalently, the occurrence count should always be appended.  This
makes it unambiguous that the line is from the squashed summary rather than
an immediate emission.

**Update:** Always append `(N occurrence[s])` in the squashed block,
even for N=1.  This applies to W1, W4, W5, and the new W6.

### 7. Squashed flush order

Corpus analysis showed that the original flush order (W1 → W6 → W4 → W5)
interleaved "easy to fix" and "hard to fix" warnings arbitrarily.

**Fix:** Reorder the flush block by decreasing actionability: W5 → W4 → W6
(W1 is merged into W5 per §8; see §2 for rationale).  The
`--detailed-warnings` stream is unaffected.

### 8. W1/W5 merge

A second corpus run revealed that the same file path often appeared in
both the W1 block and the W5 block:

```
Warning: missing dependency file:third_party/envoy/.../accesslog.proto (12 occurrences)
Warning: missing file 'third_party/envoy/.../accesslog.proto' (1 occurrence)
```

These are not the same occurrences — W1 counts the single loading-phase
miss; W5 counts rendering-phase pool failures — but they name the same
root cause and require the same fix (add the file to `-I`).  Showing both
is redundant and forces the user to correlate two entries mentally.

**Fix:** `w1()` feeds the same counter as `w5()`.  A missing file
accumulates one count when first looked up during loading (the W1 event)
and additional counts for each rendering reference that fails (W5 events).
The single flushed line is:

```
Warning: missing dependency file:third_party/envoy/.../accesslog.proto (13 occurrences)
```

Where 13 = 1 (loading) + 12 (rendering).  For files that are missing but
never referenced during rendering, the count is 1 — identical to what W1
alone would have shown.

In `--detailed-warnings` mode, W1 prints immediately (as before) using the
loading-phase message format, and W5 prints immediately at render time —
the two are kept distinct in detailed mode since they occur at different
phases and the user sees them in chronological order anyway.

---

## Test coverage

Tests live in `reproto/src/reproto/tests/`.  New unit tests for
`WarningCollector` go in `test_warnings.py` (`_classify_exc`, which
parsed exception text into W4/W5/W6, and its TC-C tests were removed by
spec 0429 S16: a render failure is W6 under a fixed reason); new
integration/regression tests extend `test_pruned_dependency.py`.

### Unit tests — `WarningCollector` (`test_warnings.py`)

**TC-W1  Squashed flush format and order.**
Feed one `w5()`, two `w4()` for the same type, and one `w6()` into a
collector in squashed mode.  Call `flush()` and capture stderr.  Assert:
- W5 line appears before W4 line (flush order per §7).
- W4 line appears before W6 line.
- W5 line carries `(1 occurrence)`.
- W4 line carries `(2 occurrences)`.
- W6 line carries `(1 occurrence)`.

**TC-W2  "Run with --detailed-warnings" hint.**
Feed two events for the same W4 key (count > 1).  Assert that `flush()`
output ends with the hint line `"Run with --detailed-warnings to see all
warning occurrences."`.  Then repeat with a single unique event (count = 1)
and assert the hint is absent.

**TC-W3  Detailed mode: events fire immediately, flush is a no-op.**
Create a collector with `detailed=True`.  Emit `w4()`, `w5()`, `w6()` and
capture stderr for each call.  Assert each produces a line immediately.
Call `flush()` and assert it produces no additional output.

**TC-W4  W1 feeds the W5 counter.**
Call `w1("missing.proto")` once and then `w5("missing.proto")` twice on a
squashed collector.  Call `flush()`.  Assert exactly one W5 line appears
(no separate W1 line) and the occurrence count is 3.

**TC-W5  W5 suppression for pruned files.**
Call `register_pruned_file("pruned.proto")`, then `w5("pruned.proto")` and
`w1("pruned.proto")`.  Call `flush()`.  Assert no line for `pruned.proto`
appears in the output.  Repeat the same check in detailed mode.

**TC-W6  W3 is always immediate in both modes.**
In squashed mode call `w3("some message")` and assert the line appears on
stderr immediately (before `flush()`).  Confirm `flush()` does not repeat it.

**TC-W7  Flush with no events produces no output.**
Create a fresh squashed collector and call `flush()`.  Assert stderr is
empty and the hint line is absent.

**TC-W8  Multiple W5 keys sorted alphabetically.**
Feed `w5("z/z.proto")` and `w5("a/a.proto")` in that order.  Assert the
flushed W5 block is sorted: `a/a.proto` line precedes `z/z.proto` line.

### Regression tests — `test_pruned_dependency.py`

**TC-R1  Zero W5 for importers of a `--prune`-pruned file.**
Extend `test_explicit_prune_no_crash`: after asserting no crash, also assert
that no `"missing dependency file:prune_base.proto"` line appears in stderr.
This verifies §6.2 suppression for the explicit-prune path.

**TC-R2  Zero W5 for importers of a duplicate-symbol-pruned file.**
Extend `test_duplicate_prune_no_crash`: after asserting no crash, also
assert that no `"missing dependency"` line for either `prune_duplicate_1` or
`prune_duplicate_2` appears in stderr (whichever was pruned produces no W5).
This verifies §6.2 suppression for the duplicate-prune path.

**TC-R3  W1 loading miss appears as a W5 squashed line.**
Compile only `prune_importer.proto` (not `prune_base.proto`) and run reproto
without providing `prune_base.pb` on the command line but with a fresh
`-I` that does not contain `prune_base.proto`.  Assert that stderr contains
exactly one `"missing dependency file:prune_base.proto"` line (the W1 loading
miss merged into the W5 counter) and no separate `"missing file"` line.

### Existing tests

All existing tests must continue to pass unchanged.  Squashed mode is now the
default; message-format changes from §1 are reflected in updated fixture
expectations where reproto's stderr is asserted.

---

## Functional call-site coverage

The section above covers the warning infrastructure itself.  This section
identifies every call site in reproto that emits a warning or error, the
condition that triggers it, and the test gap (if any).

### Source inventory

Every `cli_warning`, `cli_error`, `cli_attention`, `get_collector().wN()`, or
`report()` call in the reproto source, grouped by file.

#### `load.py:188` — `w1()`: import not found on `-I`

Fires when a `.proto` name listed in a descriptor's `dependency` field cannot
be found on any `-I` root.  In squashed mode the event merges into the W5
counter; in detailed mode it prints immediately.

**Already covered by:** TC-R3 (W1→W5 merge end-to-end).

#### `phases.py:394` — `cli_warning`: self-dependency stripped

Fires when a `FileDescriptorProto` lists its own name in its `dependency`
field.  This is a malformed descriptor artifact; reproto strips the entry to
prevent the C extension from segfaulting.

**Test gap:** No test exercises this path.

**TC-P1 (implemented — revised):** Patch a compiled `.pb` to add the
file's own name to `fdp.dependency`.  A self-referential FDP is never a
topo-sort leaf (it perpetually depends on itself), so it reaches the
circular-dependency detector rather than `_strip_self_dependency`.  The
observable behaviour is `"Circular dependency detected"` on stderr; the test
asserts exactly that.  The `_strip_self_dependency` helper is effectively a
dead-code guard for seed files — it is reachable only for dependency files
loaded during phase 2.

#### `phases.py:453` — `w3()`: duplicate symbols (W3)

Fires when two files define the same fully-qualified symbol and the default
duplicate-pruning path activates.

**Already covered by:** `test_duplicate_prune_no_crash` (TC-R2 extension).

**Proposed TC-P2 (single-symbol case):** Create two fixtures that define
exactly one common symbol (simplest possible conflict).  Assert the W3 line
says `"(1 symbol)"` (singular), not `"(1 symbols)"`.

#### `phases.py:622/637` — `cli_warning`: pool Add failure (`--keep-duplicates`)

Fires when `--keep-duplicates` is active and `pool_db.Add()` raises
`TypeError` for a conflicting descriptor.  This is the legacy path; the
default path (duplicate pruning) bypasses it entirely.

**Test gap:** No test passes `--keep-duplicates` with conflicting fixtures.

**Proposed TC-P3:** Run reproto on the two `prune_duplicate_*` fixtures with
`--keep-duplicates`.  Assert reproto still exits 0 (does not crash), and that
no `"pruned"` W3 line appears (the pruning path is disabled).

#### `phases.py:647` — `cli_warning`: unparseable file (DecodeError)

Fires when a binary file with a `.pb`/`.binpb`/etc. extension cannot be
decoded as either a `FileDescriptorSet` or a `FileDescriptorProto`.

**Test gap:** No test passes corrupt binary input.

**Proposed TC-P4:** Write a file with a `.pb` extension whose contents are
random bytes (not valid protobuf).  Pass it as an input to reproto.  Assert
reproto exits 0 and stderr contains `"Skipping unparseable file:"`.

#### `phases.py:658` — `cli_warning`: circular dependency

Fires when the topological sort cannot drain all files because at least two
remaining files depend on each other.

**Test gap:** No test exercises this path.

**Proposed TC-P5:** Construct two `FileDescriptorProto` objects that each list
the other as a dependency (A imports B, B imports A), serialise them to `.pb`,
and run reproto.  Assert stderr contains `"Circular dependency detected:"` and
reproto exits 0.

#### `phases.py:724/728` — pruning target not found

`cli_warning("Pruning target not found: ...")` fires when a `--prune` pattern
matches no FQDN in the loaded graph.  `cli_attention("Did you mean: ...")` is
a follow-up fuzzy suggestion for non-glob patterns.

**Test gap:** No test passes an invalid `--prune` value.

**Proposed TC-P6:** Compile `prune_base.proto` and run reproto with
`--prune file:nonexistent.proto`.  Assert stderr contains `"Pruning target not
found: file:nonexistent.proto"` and reproto exits 0.

**Proposed TC-P7 (fuzzy suggestion):** Use a `--prune` value that is a
near-miss of a real FQDN (e.g. `file:prune_bse.proto` vs `file:prune_base.proto`).
Assert stderr contains a `"Did you mean:"` line.

#### `phases.py:783/787` — seed not found

Same structure as pruning-not-found, but for `--seed`.

**Test gap:** No test passes an invalid `--seed` value.

**Proposed TC-P8:** Run reproto with `--seed file:nonexistent.proto` on a
loaded fixture.  Assert stderr contains `"Seed not found: file:nonexistent.proto"`.

**Proposed TC-P9 (fuzzy suggestion):** Use a near-miss `--seed` value and
assert a `"Did you mean:"` suggestion appears.

#### `phases.py:795` — pruned seed skipped

`cli_info("Skipping pruned seed: ...")` fires when a `--seed` FQDN resolves
to a node that is already pruned.

**Test gap:** No test combines `--seed` and `--prune` targeting the same node.

**Proposed TC-P10:** Compile `prune_base.proto`, then run reproto with both
`--prune file:prune_base.proto` and `--seed file:prune_base.proto`.  Assert
stderr contains `"Skipping pruned seed:"` and that `prune_base.proto` does not
appear in the output.

#### Anomalies in `re_file.py`

**A1 — editions file rendered as proto2 (`--force-proto2-output`)**

Fires when `--force-proto2-output` is used on an editions source file.

**Already covered by:** `test_roundtrip_polyglot` for editions fixture.
No stderr assertion — gap: A1 warning line not asserted.

**Proposed TC-A1:** Run reproto on `editions_rendering.proto` with
`--force-proto2-output`.  Assert stderr contains `"WARNING[editions]:"`.

**A2 — syntax downconverted from proto3 to proto2**

Fires when `--force-proto2-output` is used on a proto3 source file.

**Test gap:** No explicit assertion that A2 fires.

**Proposed TC-A2:** Run reproto on any proto3 fixture with
`--force-proto2-output`.  Assert stderr contains `"WARNING[downconvert]:"`.

**A3 — file options could not be rendered**

Fires when `ctx.pool.FindFileByName()` raises an exception for the file being
rendered (e.g. duplicate symbol, or dependency not loaded).

**Test gap:** Triggered in corpus runs but no fixture test exercises A3.

**Proposed TC-A3:** Construct a `.pb` where the file options reference an
extension type that is not provided in any of the `-I` files.  Assert the
rendered `.proto` contains `"OMITTED[render]:"` or `"WARNING[render]:"` and
that the W4 or W5 squashed line appears on stderr.

**A4 — `import weak` not valid in proto3**

Fires when a proto3 file uses `import weak` (which is proto2-only).  Reproto
renders it as a plain `import`.

**Already covered by:** `test_roundtrip[weak_import_proto2.proto]`.
No stderr assertion — gap: A4 warning not asserted.

**Proposed TC-A4:** Add a fixture `weak_import_proto3.proto` (proto3 syntax,
uses a weak dependency), compile and run reproto, assert stderr contains
`"WARNING[proto3]: 'import weak'"` and the output uses plain `import`.

**A5 — file-level extend block not valid in proto3**

Fires when a proto3 file contains a top-level `extend` block.  Reproto omits
it.

**Test gap:** Not covered (proto3 extend is a proto2 combination edge case
needing a specially crafted descriptor, since `protoc` itself rejects it in
proto3 source).

**Proposed TC-A5:** Programmatically build a `FileDescriptorProto` with
`syntax = "proto3"` and a non-empty `extension` field (proto3 + extension),
serialise to `.pb`, run reproto, assert `"OMITTED[proto3]:"` appears and the
output contains no `extend` block.

#### Anomalies in `re_descriptor.py`

**B1 — nested extend block not valid in proto3**

Same as A5 but for a message-nested extend block.

**Test gap:** No fixture test covers B1.

**Proposed TC-B1:** Same approach as TC-A5 but with the extension field inside
a message's `extension` repeated field rather than at file level.  Assert
`"OMITTED[proto3]:"` in the rendered comment block.

**B2 — extension range not valid in proto3**

Fires when a proto3 message has an `extension_range` (proto2-only).

**Test gap:** No fixture test covers B2 (same programmatic-descriptor approach
needed).

**Proposed TC-B2:** Build a `FileDescriptorProto` with `syntax = "proto3"`,
a message with a non-empty `extension_range`, serialise, run reproto, assert
`"OMITTED[proto3]:"`.

**B3 — `message_set_wire_format` not valid in proto3**

Fires when a proto3 message has `options.message_set_wire_format = true`.

**Test gap:** No fixture test covers B3.

**Proposed TC-B3:** Build a `FileDescriptorProto` with `syntax = "proto3"`
and a message with `MessageOptions.message_set_wire_format = true`, serialise,
run reproto, assert `"WARNING[proto3]:"` and no `message_set_wire_format`
line in the output.

#### Anomalies in `re_field.py`

**C1 — non-canonical map entry**

Fires when a `map_entry = true` message is missing field 1 (key) or field 2
(value).  Reproto falls back to `repeated MessageType field`.

**Test gap:** Requires a programmatically crafted descriptor; no fixture covers
this.

**Proposed TC-C1:** Build a `FileDescriptorProto` whose map-entry message has
only field 1 (missing field 2), serialise, run reproto, assert
`"WARNING[render]:"` and `"repeated"` in the output for that field.

**C2 — group field not valid in proto3**

Fires when a proto3 file has a `TYPE_GROUP` field.  Reproto renders it as a
plain message field.

**Test gap:** Similar to B1/B2 — requires a crafted descriptor.

**Proposed TC-C2:** Build a proto3 `FileDescriptorProto` with a `TYPE_GROUP`
field, serialise, run reproto, assert `"WARNING[proto3]:"`.

**C3 — `required` label not valid in proto3**

Fires when a proto3 field has `LABEL_REQUIRED`.

**Test gap:** Same programmatic approach.

**Proposed TC-C3:** Build a proto3 descriptor with a required field, run
reproto, assert `"WARNING[proto3]:"` and that the output renders the field
without the `required` keyword.

**C4 — explicit default value not valid in proto3**

Fires when a proto3 field has `default_value` set.

**Test gap:** No fixture covers this path (protoc itself rejects it).

**Proposed TC-C4:** Build a proto3 descriptor with `default_value = "hello"`,
run reproto, assert `"WARNING[proto3]:"`.

**C5 — field options could not be rendered**

Fires when `ctx.pool.FindExtensionByNumber()` or `FindFieldByName()` raises
for a field (option type missing from pool).

**Test gap:** Triggered in corpus runs but no fixture test exercises C5
directly, asserting the W4/W5/W6 routing.

**Proposed TC-C5:** Build a descriptor where a field carries a custom option
whose extension is not in the pool.  Run reproto, assert that the rendered
`.proto` contains `"WARNING[render]:"` on the field's line and that a W6
squashed line appears on stderr.

#### Anomalies in `field_descriptor.py`

**D1 — unexpected value type for option scalar**

Fires when the Python value for an option field does not match the field's
proto type (e.g. a float received where an int is expected).

**Test gap:** D1 can only be triggered by a crafted in-process scenario (the
pool enforces type correctness at decode time, so a real `.pb` cannot easily
trigger it).

**TC-D1 (implemented — revised):** The D1 `TypeError` path in
`dump_option()` is unreachable in practice: `get_scalar()` raises `RuntimeError`
(not `TypeError`) for scalar Python-type-vs-proto-field-type mismatches, and the
`TypeError` in its `case _` branch can only fire when a value is not any of
`bool/int/float/str/bytes` — but `dump_option()` already routes those to D2 via
its own `case _` arm.  The test documents the observable behaviour: passing an
`int` to a `str` proto field raises `RuntimeError("Unexpected FieldDescriptor type")`.

**D2 — unrecognised descriptor type**

Fires when the match arm in `render_value()` hits the `case _:` branch.

**Test gap:** Same unit-test approach.

**Proposed TC-D2 (unit test):** Call `render_value()` with a value of a type
not matched by any arm (e.g. a `list`) and assert a `"WARNING[render]:"` `D2`
comment is returned.

### Summary table

| ID | Trigger | File | Test status |
|----|---------|------|-------------|
| TC-P1 | Self-dependency in descriptor | `phases.py:394` | Implemented (see revised note) |
| TC-P2 | W3 singular "1 symbol" | `phases.py:453` | Implemented |
| TC-P3 | `--keep-duplicates` pool failure | `phases.py:622` | Implemented |
| TC-P4 | Unparseable `.pb` file | `phases.py:647` | Implemented |
| TC-P5 | Circular dependency | `phases.py:658` | Implemented |
| TC-P6 | Pruning target not found | `phases.py:724` | Implemented |
| TC-P7 | Pruning fuzzy suggestion | `phases.py:728` | Implemented |
| TC-P8 | Seed not found | `phases.py:783` | Implemented |
| TC-P9 | Seed fuzzy suggestion | `phases.py:787` | Implemented |
| TC-P10 | Pruned seed skipped | `phases.py:795` | Implemented |
| TC-A1 | A1 editions→proto2 warning | `re_file.py` | Implemented |
| TC-A2 | A2 proto3→proto2 warning | `re_file.py` | Implemented |
| TC-A3 | A3 file options fail | `re_file.py` | Skipped (needs real protoc extension; not exercisable via crafted descriptor) |
| TC-A4 | A4 weak import in proto3 | `re_file.py` | Implemented |
| TC-A5 | A5 file-level extend in proto3 | `re_file.py` | Implemented |
| TC-B1 | B1 nested extend in proto3 | `re_descriptor.py` | Implemented |
| TC-B2 | B2 extension range in proto3 | `re_descriptor.py` | Implemented |
| TC-B3 | B3 message_set_wire_format in proto3 | `re_descriptor.py` | Implemented |
| TC-C1 | C1 non-canonical map entry | `re_field.py` | Implemented |
| TC-C2 | C2 group in proto3 | `re_field.py` | Implemented |
| TC-C3 | C3 required in proto3 | `re_field.py` | Implemented |
| TC-C4 | C4 default value in proto3 | `re_field.py` | Implemented |
| TC-C5 | C5 field options fail | `re_field.py` | Skipped (same trigger as A3; requires real compiled extension) |
| TC-D1 | D1 scalar type mismatch (unit) | `field_descriptor.py` | Implemented (see revised note; D1 path is unreachable) |
| TC-D2 | D2 unknown descriptor type (unit) | `field_descriptor.py` | Implemented |

Anomalies A5, B1, B2, B3, C1, C2, C3, C4 all require programmatically crafted
`FileDescriptorProto` objects because `protoc` itself rejects the corresponding
source-level constructs.  These tests should live in a dedicated
`test_anomalies.py` that builds descriptors in-process and calls `reproto()`
directly rather than going through the CLI subprocess.
