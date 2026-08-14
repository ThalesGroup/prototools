<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Where the two walks spend their instructions

*last verified: 2026-08-14*

Two walks carry almost all of this workspace's compute: the **scoring walk**
(`prototext-graph`, `score::walk`) and the **decode walk** (`prototext-core`,
`serialize::render_text`).  This note records what instruction-level profiles
of each one show, and what follows from that.

The motivating case is protolens' startup on the googleapis descriptor set,
which is the largest thing either walk is asked to do:

```sh
time target/release/protolens --descriptor-set $PROTOTEXT_GOOGLEAPIS_SET \
                              $PROTOTEXT_GOOGLEAPIS_SET quit
```

## Executive summary

- **That command is the scoring walk, and almost nothing else.**  68% of its
  141 G instructions fall in three functions of `score::walk`.
- **Over half of the whole startup is one `while` loop**: the packed-varint
  element check in `score_message_multi` (`walk.rs:1444-1454`).  It runs
  `parse_varint` **900 142 298 times** (34.85% of the process) and
  `check_varint_value` **875 123 235 times** (19.27%) — **54.1% together**.
- **Those 900 M parses decode the same bytes over and over.**  The loop sits
  *inside* `for ae in active.iter_mut()`, so one packed payload is re-decoded
  once per active entry group, and `check_varint_value`'s veto is a pure
  function of `(child state, value)` — identical for every group that resolves
  to the same child.
- **The decode walk has the opposite shape**: no single hot function, but 22%
  of its instructions are `memcpy` plus `malloc`/`free`, and a further 8% are
  per-field schema re-lookups through `prost_reflect`.
- The scoring walk's heap traffic is a **fixed cost, not a per-root one**:
  `_int_malloc` measures 911 K / 987 K / 1 149 K instructions at 256 / 1024 /
  4096 roots while the total grows 19x.
- **`benches/score.rs` cannot see any of this.**  Its generator holds the
  active set at the full root count by construction, which makes it a pure
  measurement of the O(active-set) term; `parse_varint` is 0.37% of it even
  with a 20 000-record blob.  See "Why the bench is blind to the hot loop".

The two therefore want different treatments: the scoring walk is a
*redundant-work* problem, the decode walk is an *allocation-and-copy* problem.

## What this document is not evidence for

**Every percentage here is descriptor-set-shaped.**  Both profiles run on a
`FileDescriptorSet` — `googleapis.desc` for the startup, the self-describing
descriptor for the decode bench — and a descriptor set is an unusual protobuf.
It carries `SourceCodeInfo`, whose `path` and `span` are long packed `int32`
runs, and the loop that dominates the startup is gated on exactly that element
type.  A message-heavy or string-heavy blob would rank these entries
differently; one with no packed scalars would not enter the hot loop at all.

So the numbers below establish that a *particular* redundancy is expensive on
a *particular* input.  What generalizes is not the size of any figure but the
shape of the defect: the scoring walk re-derives facts that depend only on the
payload bytes once per active entry, which is wasted work on every input.  Read
the proposals on that basis, and do not quote 54% as an expected win.

## How these numbers were taken

The dev VM exposes **no virtual PMU** — `/sys/bus/event_source/devices/` has no
`cpu` entry and `perf stat -e instructions` reports the event unsupported — so
hardware counters (cycles, cache misses, branch misses) are unavailable here at
any `perf_event_paranoid` setting.  Two tools remain, and they answer different
questions:

- **`perf record -e cpu-clock`** samples the *software* clock event, which does
  work.  Full speed, real threads, so this is the only way to profile the
  actual startup command.  Sampling, so it is statistical.
- **`callgrind`** counts instructions exactly by simulation.  Deterministic and
  repeatable to the instruction, but ~50× slower and it serializes threads, so
  it cannot show a concurrency effect.

Three traps, all of which produced a wrong answer at least once before being
caught:

1. **Callgrind counts the whole process**, including benchmark setup.  For
   `prototext-core --bench codec` the one-time schema parse was **52%** of the
   process — a profile taken without scoping is mostly setup.  Scope it with
   `--collect-atstart=no --toggle-collect='<entry point>'`.
2. **A `--toggle-collect` pattern that matches nothing collects nothing**, and
   says so only after the full simulation has run.  Verify the symbol exists
   before spending the time.
3. **An inlined function has no symbol to toggle on.**  `score_all` is inlined
   into its caller, so it cannot be scoped directly; scoping to the inner
   `walk::score_subset` instead silently excludes whatever `score_all` does
   around the walk.  This is what the harness below exists to fix.

Machine configuration and pinning: see [bench-process.md](bench-process.md).

## The scoring walk

### What the startup command actually does

`callgrind` over the whole command, unscoped — startup *is* the whole process
here, so nothing needs excluding — pinned to `taskset -c 4`, **140 959 265 308
instructions**:

| self | symbol |
| ---: | --- |
| **35.99%** | `prototext_core::helpers::varint::parse_varint` |
| 17.31% | `score::walk::check_varint_value` |
| 14.93% | `score::walk::score_message_multi` |
| 5.03% | `core::option` inlined into `score_message_multi` |

An earlier `perf record -e cpu-clock -F 999` sample of the `protolens-sweep`
thread put `score_message_multi` first at 43.30% and `parse_varint` second at
33.40%.  The exact counts above supersede it: sampling one thread by
`cpu-clock` and counting every instruction in the process are different
measurements, and the ranking differs.  The 33.40% figure was right about the
magnitude and wrong about the order.

Wall clock for the command, `hyperfine`, 5 runs after a warmup, pinned to
`taskset -c 4-7`: **4.723 s ± 0.034 s**, user 16.3 s across the four pinned
cores.  Within a single hyperfine invocation the σ is 0.7%, so an effect above
roughly 2% is readable.

**Across invocations it is not that stable, and the reason is not known.**  An
earlier run of the identical binary and command, taken the same afternoon,
measured 9.354 s ± 0.033 s with user time 32.2 s — twice the wall clock *and*
twice the CPU, so the program really did about twice the work, and the run was
internally consistent (σ 0.35%) while being wrong.  Two later invocations agree
at 4.72–4.74 s.  Until that is explained, **compare binaries inside one
`hyperfine` invocation** (`hyperfine cmd_a cmd_b`, which interleaves them)
rather than against a figure recorded earlier.  A plausible mechanism, not yet
confirmed: parts of startup are deadline-driven rather than work-driven
(specs 0255, 0257, 0263), so a perturbed run can do a different *amount* of
baking rather than the same amount more slowly.  That would make the command a
poor absolute benchmark and a fine relative one.

### Where the 900 million varints come from

Function-level attribution says `parse_varint`; the call graph says *from
where*, and that is the finding.  `score_message_multi`'s callees:

| inclusive | callee | calls |
| ---: | --- | ---: |
| 34.85% | `helpers::varint::parse_varint` | 900 142 298 |
| 19.27% | `score::walk::check_varint_value` | 875 123 235 |
| 10.62% | `score_message_multi'2` (recursion) | 7 771 |
| 2.17% | `score::walk::propagate_vetoes` | 7 771 |
| 0.18% | `score::walk::record_occurrence` | 6 713 467 |

Both hot edges leave from the same four lines — the per-element check of a
packed varint run, `walk.rs:1444-1454`:

```rust
let node = find_node(ws.graph, child);
let needs_element_check = /* int32/uint32, or a bool/enum range */;
let mut do_veto = false;
if needs_element_check {
    let mut p = 0usize;
    while p < payload.len() {
        let vr = parse_varint(payload, p);      // 34.85% of the process
        p = vr.next_pos;
        if check_varint_value(ws, ae, node, vr.value, vr.overhang) {  // 19.27%
            do_veto = true;
            break;
        }
    }
}
```

That block is inside `for ae in active.iter_mut()`.  So a packed payload is
decoded **once per active entry group**, and every group decodes the same
bytes to the same values.  900 M parses is not 900 M distinct varints in a
25.6 MB blob — it is a much smaller number of varints, multiplied by the
active set.

`check_varint_value` compounds it.  Its **veto verdict is a pure function of
`(node.wire_type, node.range_idx, val)`** — `ae` appears in its body only to
walk `ae.entries` for the `non_canonical` bookkeeping.  Two active entry
groups that resolve to the same `child` state therefore compute the same
answer, element for element, and the loop `break`s at the same element.

The idiom for fixing this is already on the page, eleven lines above: the
`run_ok` test memoizes across the same loop with

```rust
_ => *packed_varints_ok.get_or_insert_with(|| packed_varints_terminate(payload)),
```

`packed_varints_ok` is a per-token `Option` computed at most once no matter
how many entry groups ask for it.  The element check wants exactly the same
treatment, keyed on `child` rather than on nothing.

### What parse_varint costs per call

The redundancy above is the first-order problem; the per-call price is the
second.  It is a small function by intent, and the source says so — the `#[inline]` on
it carries a comment explaining that inlining lets the shift sequence fold into
the caller's loop.  That is not what the compiler did:

```
parse_varint out-of-line body: 0x233 = 563 bytes, 3 copies in the binary
56 call sites, of which 8 are inside score_message_multi
```

563 bytes is well past LLVM's inline threshold, so `#[inline]` — a hint, not a
mandate — is declined, and the hot walk makes a real call.  The body is large
because it handles four concerns at once: the ordinary value, truncation and
overflow (returning the offending bytes), non-canonical overhang counting, and
absurdly long varints.

The return type compounds it:

```rust
pub struct VarintResult<'a> {
    pub next_pos: usize,           //  8
    pub varint_gar: Option<&'a [u8]>, // 16
    pub varint: Option<u64>,       // 16
    pub varint_ohb: Option<u64>,   // 16
}                                  // = 56 bytes, returned via hidden pointer
```

Every varint in a 25 MB blob is decoded into 56 bytes of memory that the
caller must then read back.

And the scoring walk does not want any of it.  `walk.rs` already defines its
own 32-byte result and an adapter that immediately discards the borrow and
flattens the options:

```rust
fn parse_varint(buf: &[u8], start: usize) -> VarintResult {
    let vr = prototext_core::helpers::parse_varint(buf, start);
    VarintResult {
        next_pos: vr.next_pos,
        garbage: vr.varint_gar.map(|_| ()),   // borrow dropped
        value: vr.varint.unwrap_or(0),        // option flattened
        overhang: vr.varint_ohb.unwrap_or(0), // option flattened
    }
}
```

The comment above it is explicit that scoring never reproduces bytes and only
needs to know *that* they were garbage.  The adaptation is correct; it just
happens **after** the full price has been paid, not instead of it.

Note also what the walk pays on the success path that it never reads: the
overhang count is computed by scanning *backwards* over the varint's bytes on
every successful parse, and it exists for byte-identical re-export — a
rendering concern, not a scoring one.

The bill comes to **54.6 instructions inside `parse_varint` per call**, plus a
further 5.0 in the adapter (`walk.rs:535`, 3.21% of the process on its own) for
the call, the 56-byte store and the read-back.  A varint whose value fits in
one byte — which most tags and most packed elements are — should cost a load, a
test and a branch.

### Why the bench is blind to the hot loop

`benches/score.rs` and `examples/profile_score.rs` share a generator whose
comment states its purpose plainly: fields 1-3 are common to every root and
field `100 + i` is unique and never encoded, "which keeps Hopcroft from merging
the roots without vetoing any of them, holding the active set at the full root
count for the whole walk."

That is a deliberate worst case for the O(active-set) term, and it works.  It
also means the blob never grows a packed varint run, so the loop that is 54% of
a real startup is never entered.  Measured, `bin/profile score`, 3 iterations:

| roots | records | Ir collected | Ir / root / iteration | `parse_varint` |
| ---: | ---: | ---: | ---: | ---: |
| 256 | 64 | 77 069 876 | 100 352 | 0.08% |
| 1024 | 64 | 336 571 210 | 109 561 | 0.02% |
| 4096 | 64 | 1 494 296 663 | 121 606 | 0.00% |
| 64 | 20 000 | 1 888 674 655 | — | 0.37% |

Three things fall out of it:

- **The scaling question is answered: the walk is O(A), not O(A²).**  Four
  times the roots costs 4.37x then 4.44x the instructions; per-root cost rises
  9% then 11% per quadrupling, which is a slowly growing constant and not a
  second factor of A.
- **`parse_varint` is 0.37% of the bench even with a 20 000-record blob**,
  against 35.99% of a real startup.  The bench holds tokens fixed and grows the
  active set; googleapis does the reverse inside the packed loop.  Any change
  to the varint path will therefore look free in `--bench score`, and it is
  not.  A workload with packed repeated scalars is missing.
- **Heap traffic is fixed, not per-root**: `_int_malloc` is 911 K / 987 K /
  1 149 K instructions while the totals grow 19x, so the walk really does
  allocate nothing in steady state, as spec 0179 intended.  It is not zero,
  which is why a 64-root smoke run shows it at 2%: a constant against a small
  denominator.  One caveat — `memcpy` is 196 K / 659 K / **12 371 K** across
  the three root counts, an 18.8x jump on the last step alone.  Something
  spills its inline capacity between 1024 and 4096 roots.  Not chased.

The `from_utf8` entry (5.26–6.37% of the bench, absent from the startup top)
is worth a second look on its own: UTF-8 validation of candidate string fields
is a real cost, and a score only needs to know whether the bytes *are* valid,
which is what `from_utf8` returns — but it is being called on every candidate
string of every surviving root.

## The decode walk

`callgrind`, scoped to `decode_and_render`, `prototext-core --bench codec`,
path A2 (schema + annotations) over the 18.7 KB self-describing descriptor,
117.7 M instructions:

| self | symbol |
| ---: | --- |
| 12.27% | `serialize::common::escape::escape_string_into` |
| 12.21% | `render_text::helpers::annotations::AnnWriter::push_field_decl` |
| 11.40% | `render_text::render_message` |
| **10.38%** | `__memcpy_avx_unaligned_erms` |
| 8.76% | `render_text::sink::TextSink::scalar_field` |
| 7.29% | `helpers::varint::parse_varint` |
| 6.05% | `prost_reflect::FieldDescriptor::kind` |
| 5.25% | `_int_free` |
| 4.48% | `render_text::helpers::len_field::render_len_field` |
| 3.72% | `malloc` |
| 2.74% | `free` |
| 2.73% | `core::str::converts::from_utf8` |
| 2.05% | `prost_reflect::FieldDescriptor::name` |
| 1.94% | `render_text::helpers::output::wfl_prefix_n` |

Two structural facts, neither of them algorithmic:

- **21.9% is memory traffic**: `memcpy` 10.38% plus `malloc` 3.72%, `free`
  2.74% and `_int_free` 5.25%.  A fifth of the decode path is moving bytes
  around and managing the heap rather than decoding protobuf.
- **8.1% is schema re-lookup**: `FieldDescriptor::kind` 6.05% and
  `::name` 2.05%, resolved once per field per render rather than once per
  field.  These are `prost_reflect` accessors that walk back into the
  descriptor pool on each call.

`parse_varint` appears here too, at 7.29% — the same un-inlined body, called
from a walk with a different balance of work around it.

## What follows

Ordered by expected return over risk.  None of these are implemented; each
needs its own before-and-after under
[bench-process.md](bench-process.md)'s protocol.

P0, P1, P7 and P8 are specified together in
[spec 0288](../specs/0288-the-same-bytes-are-read-once-whoever-asks.md), which
also records why the blob-wide cache and the hand-written assembly were
rejected.  P4 is closed there as a non-goal.

**P0 — memoize the packed element check across the active set.**  The largest
item on the board by a wide margin: it targets **54.1%** of startup, and it
removes work rather than making work cheaper.  Inside the `for ae in
active.iter_mut()` loop, `walk.rs:1444-1454` re-decodes one packed payload and
re-runs `check_varint_value` over it once per active entry group.  The verdict
depends only on the payload bytes.  `packed_varints_terminate` — memoized
eleven lines above by `packed_varints_ok` — already decodes every element of
that payload once per token and throws the values away.  Keep them: the decode
this needs is the pass the walk already pays for, and the element check then
reads values instead of calling `parse_varint`.

The prize is measured in situ rather than estimated.  The two loops have the
same shape over the same payloads; the memoized one costs 61 064 771
instructions, the unmemoized one 1 761 775 748 — **28.9x**.

Two other payload-only facts sit in the same loop, and P7 is one of them.  See
[spec 0288](../specs/0288-the-same-bytes-are-read-once-whoever-asks.md) for the
buffer's constraints and for why a verdict memo keyed on `child`, and a
blob-wide table of decoded varints, were both rejected.

**P1 — make `parse_varint` small enough to inline.**  Second, and independent
of P0 — it makes the surviving parses cheaper and helps every other caller,
including the decode walk's 7.29%.  It costs **54.6 instructions per call**
today.  The 563-byte body is large for reasons that
have nothing to do with the common case — truncation and overflow (which return
the offending bytes), absurdly long varints (a second scan loop), and overhang
counting (a backwards scan).  A small `#[inline]` entry handling the common case
— terminator inside the buffer, fits in 64 bits — that tail-calls an
`#[inline(never)] #[cold]` continuation for the rest would shrink the inlinable
body by most of its size.  No varint semantics are duplicated: the cold path is
the existing code, moved.

**P2 — nothing.  Do not hand-write a lean result type for the walk.**  The
obvious companion to P1 is to give the scoring walk a 32-byte result with no
borrow and no `Option`, matching the adapter at `walk.rs:534`.  It is
unnecessary, and that is worth recording so it is not proposed again: once P1
lets the body inline, SROA breaks the 56-byte `VarintResult` into registers so
it never materializes, and dead-code elimination then removes exactly what the
adapter discards — the `varint_gar` slice computation and both `Option`
discriminants.  The optimizer *derives* the lean struct from the caller's
context.  Hand-writing it would buy the same thing at the cost of a second
implementation of truncation, overflow and overhang semantics that must agree
with the first.  Let the compiler do it.

**P3 — do not compute the overhang on the scoring path.**  The backwards scan
for non-canonical `0x80` padding runs on every successful parse and serves
byte-identical re-export, which scoring never performs.  Unlike `varint_gar`
this one does *not* fall out of P1: the walk's adapter reads `overhang`, so
dead-code elimination cannot remove the scan even after inlining.  It needs a
deliberate split — either a variant that skips it, or moving the scan behind
the `last_b == 0x00` test it already guards on so the common case never
enters it.

**P4 — `overflow-checks` is not the problem; measured, and it is 2%.**  The
release profile enables it workspace-wide (`b242a03`, 2026-08-01), which is
wider than the bug that motivated it — that wrap was in protolens'
`structure.rs`, while the setting also covers the two walks.  Measured here by
building protolens twice and interleaving both binaries in one `hyperfine`
invocation: **4.738 s with, 4.644 s without, i.e. 1.02× ± 0.01**.  The commit
that introduced it recorded +11% on the 2026-08-01 shape of the command; on
today's it is 2%.  **Keep it.**  It buys a real class of silent bug for a cost
that is barely above this command's own reproducibility, and narrowing it to
`[profile.release.package.*]` would trade that away for nothing measurable.

**P5 — resolve field descriptors once per field, not once per use.**  The
decode walk's 8.1% in `FieldDescriptor::kind` and `::name` is repeated
resolution of information that does not change during a render.  Hoisting it to
where the field is first identified is a local change.

**P6 — attack the decode walk's copies before its allocations.**  `memcpy` at
10.38% is the single largest decode entry after the two 12% ones, and
`escape_string_into` plus `TextSink::scalar_field` sit right beside it, which
suggests string bodies being built up and moved rather than written once into
the output buffer.  Worth a DHAT run to name the allocation sites before
proposing a shape.

**P7 — validate UTF-8 once per candidate, not once per root.**  `from_utf8` is
5.26–6.37% of the scoring bench.  Whether a byte range is valid UTF-8 is a
property of the range, not of the candidate root being tested against it, so
the same bytes are being validated repeatedly as the active set is explored.
This is P0's argument applied to a different check, and it does not appear in
the startup profile — so measure it on the bench, not on protolens.

**P8 — give the scoring bench a packed-scalar workload.**  Not an optimization;
a prerequisite for measuring P0 and P1.  Today `benches/score.rs` cannot
observe the loop that is half of a real startup, so a change to it would show
as noise there and as a large win on protolens, which is the wrong way round
for a regression gate.  A second generator emitting packed repeated
`int32`/`enum` runs would close it.

## Open questions

- `memcpy` in the scoring bench jumps 18.8x between 1024 and 4096 roots while
  the total grows 4.4x.  Something exceeds an inline capacity there; spec 0179
  sized those structures and the number it chose should be re-read against
  this.
- The `while p < payload.len()` loop is entered under `needs_element_check`,
  which is true only for int32/uint32/bool/enum.  Whether googleapis is
  unusually rich in those, or whether every descriptor set is, decides how
  general P0's win is.
- Why the same binary and command measured 9.354 s once and 4.72–4.74 s twice
  (above) is still unexplained.
