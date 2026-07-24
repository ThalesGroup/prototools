<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0166 — reproto: `hopcroft.rkyv` names diverge from `.desc` under variant namespace rewriting

Status: implemented
Implemented in: 2026-07-24
App: reproto
Refs: docs/specs/0159-reproto-namespace-rewrite-package-consistency.md
      (established the `apply_variant_namespace`/`ctx.keep_variant_descriptor`
      contract this spec reuses), docs/specs/0080-schema-db-wkt-completion.md
      (established the WKT/extra-node transitive-closure promotion this
      spec's collection-loop relocation depends on), docs/specs/0158-reproto-schema-db-canonical-name-collision-error.md
      (adjacent code in `_phase_build_schema_db`, unchanged by this spec)

## Background

`--build-schema-db` (`_phase_build_schema_db`, `phases.py:1485`) writes two
paired artifacts that a schema-db consumer (protolens, prototext) loads
together and expects to be name-for-name consistent:

- `db_path` (`.desc`) — a `FileDescriptorSet`, built from
  `ctx.schema_db_fdps`, the list of rendered `FileDescriptorProto`s
  accumulated during phase 7's render loop plus phase 3b's WKT/extra-node
  promotion (`phases.py:1599-1638`, spec 0080). Every name in `.desc` —
  `package`, `type_name`, `extendee`, `input_type`, `output_type` — has
  already passed through the variant's `namespace_rewrites`
  (`apply_variant_namespace`) and `import_rewrites`
  (`canonize_dependency`) rules, applied at render time by
  `re_field.py`/`re_method.py`/`re_file.py` (spec 0159).
- `hopcroft.rkyv` — a compiled Hopcroft scoring graph, built by
  `prototext_graph_lib.build_graph()` (`phases.py:1586`) from
  `scoring_graphs`, a list of YAML strings assembled by a **separate**
  walk: the inner `_collect` closure (`phases.py:1502-1526`) plus its
  driving loop (`phases.py:1530-1557`). This walk reads names directly off
  `desc.full_name` / `field.message_type.full_name`, obtained via
  `ctx.pool.FindFileByName(proto_name)` (`phases.py:1539`) — i.e. from
  `ctx.pool`, which is populated once, in phase 2, straight from the
  parsed input `.pb` files (`context.py:198-199`; `pool_db.Add(fdp)` at
  `phases.py:925,941`), **before** any variant rewriting exists. Nothing
  in this walk ever calls `apply_variant_namespace` or
  `apply_variant_namespace_to_package`.

Consequently, under a variant with a non-trivial `namespace_rewrites`
rule, `hopcroft.rkyv`'s node names (`messages` keys, `entries`, and every
`child` reference) are the **original, pre-rewrite** FQDNs, while
`.desc`'s equivalent names are the **rewritten** ones. A schema-db
consumer that scores a message by rewritten FQDN (as read from `.desc`)
and looks it up in the compiled graph by that same name gets a miss —
reported upstream as "type not found in descriptor set" for entries that
are, in fact, present, just under their old name.

A second, narrower divergence: the file selection for the `scoring_graphs`
walk is `summoned_files` (`phases.py:1530-1533`), computed **before**
step 3b's WKT/extra-node promotion runs (`phases.py:1599-1638`). `.desc`
includes phase-3b's promoted files (that is the whole point of spec
0080 — a self-contained, transitively-closed schema DB); `hopcroft.rkyv`
currently does not. So even a variant with no rewrite rules at all can
have WKT types present in `.desc` but absent from `hopcroft.rkyv`.

### Rejected alternative: a second, rewritten `DescriptorPool`

One candidate fix builds a throwaway second `DescriptorPool` from
`ctx.schema_db_fdps` (the already-rewritten FDPs that feed `.desc`) and
runs the `_collect` walk against that instead of `ctx.pool`. This
duplicates the whole schema-db-relevant slice of the pool in memory for
the sole purpose of a name lookup, and requires re-deriving
`summoned_files` from the second pool's own topology (whose insertion
order is explicitly documented as non-topological — `phases.py:1595-1597`
— unlike `ctx.nodes`, whose `is_summoned`/`is_pruned` flags the existing
pruning logic in `_collect` already depends on). Rejected: unnecessary
memory duplication, and a real risk of pruning-lookup mismatches between
the two identity spaces (`ctx.nodes.get(Fqdn(...))` calls at
`phases.py:1503,1508` are keyed on the *original* FQDN and must keep
working exactly as today).

### Rejected alternative: rewrite `FileDescriptorProto`s at load time

A second candidate applies `namespace_rewrites`/`import_rewrites` to
every `FileDescriptorProto` **before** it is added to `ctx.pool`
(`phases.py:925,941`), so `ctx.pool` and `ctx.nodes` themselves carry
rewritten names throughout the pipeline. This is mechanically feasible —
`apply_variant_namespace`, `apply_variant_namespace_to_package`, and
`canonize_dependency` (`mappings.py:61-224`) are pure functions of a
dotted-name string and `ctx`'s rule lists, independent of the pool.
Rejected: `ctx.nodes`' FQDNs are the identity space the `-p`/`--prune`/
`--seed` CLI flags match against, pipeline-wide, not just inside schema-db
building (`_find_matching_nodes`/`_fuzzy_suggest`; the FQDN format is
deliberately "pasteable directly into `--seed`/`--prune`",
`phases.py:303`). Rewriting at load time would silently change what
those flags match against for every user-facing invocation, not only
`--build-schema-db` — a much larger blast radius than this bug warrants,
for a benefit (avoiding a second pool) achievable more directly below.

## Goals

- **G1**: `hopcroft.rkyv`'s node names (`messages` keys, `entries`, every
  `child` reference) are in lockstep with `.desc`'s names — same variant
  `namespace_rewrites` applied, same result.
- **G2**: `hopcroft.rkyv`'s file coverage matches `.desc`'s file coverage,
  including files promoted by phase 3b's WKT/extra-node closure (spec
  0080) — a scoring lookup against any type present in `.desc` never
  misses `hopcroft.rkyv` for lack of coverage.
- **G3**: `--keep-descriptor-path` symmetry: when set, `.desc` keeps
  original (unrewritten) names (spec 0159 G2); `hopcroft.rkyv` must match
  — also unrewritten, not partially rewritten.
- **G4**: `ctx.pool`/`ctx.nodes` traversal, `is_pruned`/`is_summoned`
  lookups, and pruning semantics inside `_collect` are unchanged —
  zero risk to existing pruning correctness, no second pool.

## Non-goals

- **N1**: No rewriting at load time — `ctx.pool`/`ctx.nodes`/`-p`/
  `--prune`/`--seed` matching semantics are untouched anywhere in the
  pipeline outside `_phase_build_schema_db`.
- **N2**: No change to `_phase_emit_scoring_graphs` (the standalone
  `--emit-scoring-graphs` CLI command, `phases.py:1846+`) — it has no
  paired `.desc` and is a different, pre-existing consistency contract,
  out of scope here.
- **N3**: No change to `_synthesize_message_set_item`'s behavior for its
  `_phase_emit_scoring_graphs` call site (unrewritten names there today,
  unchanged by this spec).
- **N4**: No change to the Kahn's-algorithm topological sort
  (`phases.py:1662-1683`) or the spec-0158 collision check
  (`phases.py:1645-1660`) — both operate purely on `.desc`'s own file
  ordering/collision detection and are unaffected by scoring-graph naming.
- **N5**: No change to the `.rkyv` binary format, the Hopcroft
  minimization algorithm, or `prototext_graph_lib`'s Rust/PyO3 surface.

## Specification

### §1 — `_canonical_scoring_name` helper

Add a small helper near `_collect` (`phases.py`, inside
`_phase_build_schema_db`, before the "1. Collect" section) that reuses
spec 0159's existing rewrite machinery, guarded exactly like its other
call sites (`re_field.py:482`, `re_method.py:125`):

```python
def _canonical_scoring_name(ctx: 'Context', full_name: str) -> str:
    """Rewrite a scoring-graph node/child FQDN through the active
    variant's namespace_rewrites rules, mirroring the same rewrite
    .desc's own type_name/extendee fields already receive at render
    time (spec 0159) — keeps hopcroft.rkyv's names in lockstep with
    .desc's names.
    """
    if ctx.keep_variant_descriptor:
        return full_name
    from .mappings import apply_variant_namespace
    from .fake_types import Ref as _Ref
    return str(apply_variant_namespace(ctx, _Ref(f'.{full_name}'))).lstrip('.')
```

### §2 — `_collect`: rewrite `child` and the node's own name at emission

`ctx.nodes.get(Fqdn(...))` pruning lookups (`phases.py:1503,1508`) and
`node_kind`'s `group_fqdns` membership check (`phases.py:1522`, which
compares against `_collect_group_fqdns`'s own unrewritten output) stay
keyed on the **original** `desc.full_name` — computed first, unchanged.
Only the names actually written into `messages`/`entries`/`child` are
rewritten, at the point of writing:

```python
def _collect(desc: Any, messages: dict, group_fqdns: 'set[str]', entries: list[str]) -> None:
    msg_node = ctx.nodes.get(Fqdn(f'desc:.{desc.full_name}'))
    if msg_node is not None and msg_node.is_pruned:
        return
    fields_out = []
    for f in sorted(desc.fields_by_number.values(), key=lambda f: f.number):
        field_node = ctx.nodes.get(Fqdn(f'fdsc:.{f.full_name}'))
        if field_node is not None and field_node.is_pruned:
            continue
        type_str, child, range_ = _scoring_kind(f)
        entry: dict = {'number': f.number, 'type': type_str}
        if child is not None:
            entry['child'] = _canonical_scoring_name(ctx, child)
        if range_ is not None:
            entry['range'] = list(range_)
        label = _field_label(f)
        if label != 'optional':
            entry['label'] = label
        fields_out.append(entry)
    node_kind = 'GROUP' if desc.full_name in group_fqdns else 'LENDEL'
    canonical_name = _canonical_scoring_name(ctx, desc.full_name)
    _synthesize_message_set_item(desc, messages, fields_out, canonical_name)
    messages[canonical_name] = {'kind': node_kind, 'fields': fields_out}
    entries.append(canonical_name)
    for nested in desc.nested_types:
        _collect(nested, messages, group_fqdns, entries)
```

Note `_synthesize_message_set_item`'s call moves after `node_kind`/
`canonical_name` are computed, and now takes `canonical_name` (§3) so its
own synthesized `Item` FQDN (`f'{desc.full_name}.Item'`) lands in the
rewritten namespace too, consistent with every other node.

### §3 — `_synthesize_message_set_item`: optional canonical-name parameter

```python
def _synthesize_message_set_item(
    desc: Any, messages: dict, fields_out: list,
    canonical_full_name: str | None = None,
) -> None:
    """... (docstring unchanged) ..."""
    if not desc.GetOptions().message_set_wire_format or fields_out:
        return
    base_name = canonical_full_name if canonical_full_name is not None else desc.full_name
    item_fqdn = f'{base_name}.Item'
    messages[item_fqdn] = {
        'kind': 'GROUP',
        'fields': [
            {'number': 2, 'type': 'int32'},
            {'number': 3, 'type': 'bytes'},
        ],
    }
    fields_out.append({
        'number': 1, 'type': 'message', 'child': item_fqdn, 'label': 'repeated',
    })
```

Default `None` preserves `_phase_emit_scoring_graphs`'s existing
(unrewritten, single-argument) call site untouched (N3).

### §4 — Relocate the collection loop after phase 3b, widen file selection

Move the entire "── 1. Collect …" block — the `_collect` def
(`phases.py:1502-1526`), `scoring_graphs` initialization
(`phases.py:1528`), and its driving loop (`phases.py:1530-1557`) — to run
**after** phase 3b's WKT/extra-node promotion (currently
`phases.py:1599-1638`), i.e. immediately before the spec-0158 collision
check (currently `phases.py:1640`). `_collect`'s own body is unchanged
except as shown in §2; only its position moves, plus the file-selection
source:

```python
    # ── (relocated) Collect per-file scoring-graph YAML strings, now run
    # after WKT/extra-node promotion so file coverage matches .desc's
    # (spec 0080's closure) exactly instead of only pre-promotion
    # summoned_files (goal G2).
    scoring_files = list(dict.fromkeys(ctx.schema_db_fdp_origins))
    total_scoring = 0 if ctx.quiet else len(scoring_files)
    with _progress('Collecting scoring data', total_scoring, quiet=ctx.quiet) as advance:
        for proto_name in scoring_files:
            try:
                fd = ctx.pool.FindFileByName(proto_name)
            except (KeyError, TypeError) as e:
                from .lib.warnings import get_collector
                get_collector().w6(proto_name, "schema db", str(e))
                advance()
                continue

            group_fqdns = _collect_group_fqdns(fd)
            messages: dict = {}
            entries: list[str] = []
            for msg_desc in fd.message_types_by_name.values():
                _collect(msg_desc, messages, group_fqdns, entries)
            entries.sort()

            scoring_graphs.append(
                str(yaml.dump({'entries': entries, 'messages': messages},
                              sort_keys=False, allow_unicode=True))
            )
            advance()
```

`ctx.schema_db_fdp_origins` (phase 7's origin list, extended by phase 3b
at `phases.py:1638`) carries each rendered FDP's **pre-rewrite** proto
name — the same key space `ctx.pool.FindFileByName` already expects
(phase 7's origins are never rewritten; only `slot.out`'s *content* is —
confirmed by phase 7's render call sites, unchanged by this spec).
`dict.fromkeys(...)` de-duplicates while preserving first-seen order (a
file can appear once per phase-7 render and, for an extra node, once more
via phase 3b — though in practice these sets don't overlap since phase
3b only promotes previously-unsummoned nodes).

`build_graph(scoring_graphs=scoring_graphs, ...)` (`phases.py:1586`) and
everything after it (§ "2. Build the baked graph" onward) stays exactly
where it is, unchanged, now simply reading a `scoring_graphs` populated
later and via the relocated loop.

### §5 — Net effect on ordering

Old order: 1 (collect, pre-3b) → 2 (build_graph) → 3 (topo-sort setup
comment) → 3b (WKT promotion) → 3-collision-check → sort → write.

New order: 3b (WKT promotion) → 1 (collect, using post-3b origins) → 2
(build_graph) → 3-collision-check → sort → write.

The topological sort (N4) and collision check operate on
`ctx.schema_db_fdps`/`ctx.schema_db_fdp_origins`, which phase 3b already
finishes populating before either runs, in both the old and new order —
this relocation does not change their inputs.

## Test plan

New test module `reproto/src/reproto/tests/test_scoring_graph_namespace_consistency.py`,
following `test_variant_package_rewrite.py`'s fixture/harness pattern
(`_write_proto`/`_compile`/`_run` helpers, protoc-based end-to-end CLI
invocation):

Verifying `hopcroft.rkyv`'s content itself needs a real reader.
`reproto`'s own test harness drives the CLI via `subprocess.run`
(`test_variant_package_rewrite.py`'s `_run`), so a same-process Python
`monkeypatch` on `prototext_graph_lib.build_graph` (or on `phases`'
deferred, function-local import of it, `phases.py:1567`) would not reach
the subprocess and cannot be used here; no Python-side loader for an
already-baked graph exists either (`prototext_graph_lib`'s only exposed
functions, `build_graph`/`build_fds_index`, are builders, not readers).
Rather than inventing new reader infrastructure, reuse the `prototext`
CLI's existing `score --type <NAME>` subcommand — a real consumer of
`hopcroft.rkyv` that looks `<NAME>` up as a graph root and fails with
`"type '<NAME>' not found in scoring graph"` (`prototext/src/run.rs:1080`)
if absent — which is the exact failure mode the original bug report
observed. All tests below therefore chain two subprocesses: `reproto
--build-schema-db` (producing `db_path`/`db_path.stem/hopcroft.rkyv`),
then `prototext --descriptor-set db_path score --type <NAME>
--assume-binary <payload>` (`prototext` built via `cargo build
--release`, on `PATH` in this repo's dev shell).

- **G1 (name lockstep)**: reuse `test_variant_package_rewrite.py`'s
  `selfref.yaml`/`_setup_fixtures` shape (variant with a
  `namespace_rewrites` rule rewriting `proto2` → `canonical`). Run
  `reproto --build-schema-db`. Serialize a minimal `Outer` payload using
  the fixture's own compiled (pre-rewrite) Python module — wire bytes are
  name-agnostic, so this is valid input for scoring against either name.
  Assert `prototext score --type canonical.Outer --assume-binary
  payload.bin` exits 0, and `prototext score --type proto2.Outer
  --assume-binary payload.bin` exits non-zero with `"not found in
  scoring graph"` in stderr.
- **G2 (file coverage parity)**: no dedicated test. Three real fixture
  scenarios were tried during implementation to isolate a pre-fix/
  post-fix difference — a custom-option dependency pulling in
  `google.protobuf.MessageOptions` (spec 0150's own original fixture
  combo) and a plain unused `import "google/protobuf/timestamp.proto";`
  with no field ever referencing `Timestamp` — and in every case the WKT
  was already present in `hopcroft.rkyv` **before** this spec's fix, not
  just after. Root cause: `re_file.py:229-232` adds every declared
  `dependency` as a `target` of the importing file unconditionally
  (whether or not any type from it is actually used), and phase 5's
  reachability propagation is fully transitive over `.targets` — so in
  practice `ctx.schema_db_extra_nodes` (phase 6 sub-pass 3, spec 0080)
  ends up empty, or a strict subset of files already `is_summoned` via
  ordinary reachability, for every scenario tried. The old `summoned_files`
  filter and the new `ctx.schema_db_fdp_origins` source therefore
  coincide in every fixture found so far, even though they are
  structurally different variables. The fix (§4's relocation) is kept —
  it is still strictly more correct, mirroring `.desc`'s own established
  file source (spec 0080) — but no fixture demonstrating an observable
  G2 regression was found, so no test claims to exercise one. A future
  spec can revisit this if a real triggering scenario turns up (candidate
  suspects not yet tried: variant-path name-canonicalization mismatches
  between a fallback-loaded WKT node and its `self.dependency`-string
  identity).
- **G3 (`--keep-descriptor-path` symmetry)**: same fixture as G1, run
  with `--keep-descriptor-path`. Assert the outcomes invert: `prototext
  score --type proto2.Outer` exits 0, `--type canonical.Outer` exits
  non-zero — mirroring `test_variant_package_rewrite.py`'s
  `test_G2_keep_descriptor_path_stays_fully_unrewritten`.
- **Regression guard**: a fixture with a `MessageSet`-wire-format message
  under a `namespace_rewrites` rule. Serialize a payload whose wire bytes
  populate the synthesized `Item` group's `type_id`/`message` fields.
  Assert `prototext score --type <rewritten-name>` exits 0 and matches
  the `Item` sub-node (covers §3's `canonical_full_name` threading) —
  inspect via `--detailed-score`-equivalent-on-`list-schemas` is not
  needed; a non-zero `matched` count in `score`'s default output over an
  all-required-Item payload is sufficient.
