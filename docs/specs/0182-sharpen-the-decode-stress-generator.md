<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0182 — sharpen the decode stress generator, and give it a total oracle

Status: draft
Implemented in:
App: prototext, prototext-core
Refs: docs/prototext/decode-flaws.md (the "not fuzzed" correction, C1, C2,
      C3, C5),
      docs/scoring-flaws.md (C1, C3),
      docs/specs/0171-shared-wire-bounds-and-depth-caps.md,
      docs/specs/0174-render-budget-at-the-input.md

## Background

Every confirmed correctness bug in `prototext-core`'s decode path was
triggered by malformed input: C1 (LEN `pos + len` overflow), C2
(`skip_group` uncapped recursion), C3 (no depth cap), C5 (a thread-local
budget counter shared with `ProbeSink`). Rendering bytes of unknown
provenance is this library's stated purpose, so "the input was malformed"
is its normal operating condition, not an edge case.

### The scaffolding is already right; the generator is not

`selftest_roundtrip` (`prototext/tests/roundtrip.rs:2497`) is better than
the flaws report initially assumed. It has a deterministic xorshift64, a
real schema (`knife_schema`), env-tunable `PROTOTEXT_SELFTEST_N` /
`PROTOTEXT_SELFTEST_SEED`, and the right central oracle: `binary → text →
binary` must be byte-identical. A `cargo-fuzz` target would add nightly
and out-of-CI infrastructure to reach an oracle this test already has.

**What is weak is `random_bytes` (`:2486`), and it is weak in a way that
provably excludes all four bugs.** It returns uniformly random bytes of
length 0-63. Therefore:

- **C1 is unreachable.** It needs a ten-byte varint of `0x80`-continued
  bytes encoding a length near `u64::MAX`. From uniform bytes that is a
  ~2⁻⁷⁰ event.
- **C2 and C3 are unreachable at any N.** A LEN level costs two bytes and
  a group level one, so 63 bytes cannot express more than 63 levels. The
  cap is 1000. No seed reaches it.
- **C5 is unreachable.** It needed budget pressure across *sibling*
  subtrees, which needs a payload large enough to have siblings.
- Most iterations die on the first byte as `INVALID_TAG_TYPE` and consume
  the whole buffer, so the bulk of the run re-tests one path.

The million iterations are real but shallow: the generator samples the
byte space uniformly, while the decoder's interesting behavior lives in a
vanishingly small, highly structured subset of it.

### The two questions this spec exists to settle

The flaws report's recommendation list is sound and is reproduced below
as S2. But it left two things unanswered, and both are the kind of thing
that sinks a stress test six months later:

**(a) The oracle is not total.** `encode(render(b)) == b` is the right
property for well-formed input, and it happens to hold today for
everything the *current* generator can produce. It is not obviously
preserved by the sharpened generator: the depth cap degrades an over-deep
node to an opaque scalar, and the recommendation for a "nesting-depth
mode" (S2.4) targets exactly that boundary. A LEN node degraded to an
opaque byte string does re-encode to the same bytes — its content is
preserved verbatim — but a `START_GROUP` has no length prefix and no
opaque form, so its degradation is not obviously byte-preserving.

The tempting fix is an exclusion list: "skip the round-trip assertion
when the input was over-deep." **That is the wrong answer**, because the
exclusion list is maintained by hand, grows with every new generator
mode, and is precisely where a real bug would come to hide. S1 gives the
suite a *total* oracle instead, so no input is ever excluded.

**(b) It must stay inside `cargo test`.** The whole argument for
sharpening this test rather than adding `cargo-fuzz` is that it runs in
the existing regression suite. That argument is void if the sharpened
generator makes the suite too slow to keep running, and the sharpened
generator is much more expensive per iteration: the current one produces
≤63 bytes and usually dies on byte one, while S2 asks for inputs of a few
KB and nesting depths around 1000. S3 fixes a wall-clock budget rather
than an iteration count, and requires the number to be measured rather
than guessed.

## Goals

- **G1** — a **total** oracle: every generated input is checked, with no
  skip list and no input-shape exclusions.
- **G2** — a generator that reaches the region where the decoder does
  work: structured records, adversarial length prefixes, mutation of
  valid messages, and nesting around the cap.
- **G3** — the suite reports a **coverage proxy** and *fails* if coverage
  falls below a floor, so the generator cannot silently regress to
  uselessness while the test stays green.
- **G4** — it stays a `cargo test` test inside `nix-build -A ci`, within
  a stated and measured wall-clock budget.
- **G5** — determinism and reproducibility are preserved: a failure
  prints a seed and an iteration index that reproduce it exactly.

## Non-goals

- **N1** — adding `cargo-fuzz`, a `fuzz/` directory, or any
  nightly-toolchain or out-of-CI infrastructure. The oracle is the
  valuable part and this test already has it. If coverage-guided
  mutation later proves necessary, that is a separate decision with a
  separate cost, and it is better made after G3's coverage numbers exist
  to argue from.
- **N2** — fuzzing `prototext-graph`'s scoring walk. It has its own
  bounds bugs (scoring C1, C3) and deserves the same treatment, but it
  has a different oracle problem (scores are not round-trippable) and
  mixing the two would settle neither.
- **N3** — changing any decode behavior, cap, or annotation. This spec
  adds no fix; it adds the thing that would have found the fixes.
- **N4** — replacing the existing `selftest_roundtrip`. The uniform-byte
  mode stays as one generator among several: it is cheap, it is the only
  mode with no structure bias at all, and a million iterations of it is
  genuine evidence about the paths it does reach.
- **N5** — property-testing crates (`proptest`, `quickcheck`) and their
  shrinkers. Attractive, and a real dependency decision; the hand-rolled
  xorshift is already deterministic and already prints a reproducing
  seed, which is the property that matters. Revisit only if minimization
  of failing cases becomes the bottleneck in practice.

## Specification

### S1. A total oracle, in four layers

Applied to every input from every generator mode. Ordered from strongest
to most general; each is checked when its precondition holds, and at
least one always holds.

**O1 — strict round-trip (conditional, strongest).**
`encode(render(b)) == b`. Required whenever the render degraded nothing.
"Degraded nothing" must be a signal *from the renderer*, not a guess
about the input: the decode side already knows when it capped depth or
emitted a malformity, and the test must read that rather than
re-deriving it. If exposing it requires a small accessor on the render
result, that accessor is in scope for this spec — inferring it from the
rendered text is not, because that reintroduces the hand-maintained
predicate S1 exists to avoid.

**O2 — text fixed point (unconditional, total).**
`render(encode(render(b))) == render(b)`. The *text* must be a fixed
point even when the bytes are not. This is the load-bearing addition:
it is decidable, it is total, and it needs no knowledge of whether the
input was degenerate. It catches every asymmetry between the renderer
and the encoder, and it holds by construction wherever O1 holds, so it
costs one extra render to remove the entire exclusion-list problem.

**O3 — idempotence (unconditional).** `render(b)` twice on the same
thread must give identical output. This is exactly C5's class — a
thread-local counter shared with `ProbeSink` made a second render differ
from the first — and it is the class `tripping_the_depth_cap_does_not_
leak_the_counter` currently pins with one hand-written case.

**O4 — schema monotonicity (unconditional).** Rendering with and without
a schema must re-encode to the same bytes. A schema supplies names and
annotations, never wire content; this is the direct statement of that
promise and is currently asserted nowhere at random.

Nothing here may panic, for any input, ever. A panic is a failure of the
suite regardless of which oracle was being checked — that is what would
have caught C1 and C2 in minutes.

### S2. Generator modes

Each mode is selected per iteration by the PRNG, with a fixed weighting
recorded in the source. In rough order of expected yield:

1. **Records, not bytes.** Emit a sequence of `(field number, wire type,
   payload)` triples, drawing the field number from {schema-known,
   unknown, 0, `u32::MAX`-ish} and the wire type from all eight values
   including the two invalid ones. This lands the generator inside the
   space where the decoder does work.
2. **Adversarial length prefixes.** Sample LEN from a fixed interesting
   set — `0`, `1`, exactly-remaining, remaining+1, `2^31`, `2^32`,
   `u64::MAX` — plus overlong (non-canonical) encodings of small values.
   That set contains C1 by construction, and non-canonical varints are
   already a concept the renderer annotates.
3. **Mutation of valid messages.** Build a well-formed message, then
   apply one damaging edit: truncate at a random offset, flip a wire
   type, delete a length byte, duplicate a tag. **Truncation is the
   single highest-yield mutation**, because every `payload_end`
   rejection path is a truncation.
4. **Nesting depth.** Wrap a leaf in `k` levels of LEN or `START_GROUP`
   with `k` drawn from around the cap — 0, 1, 999, 1000, 1001, 5000.
   This requires lifting the 63-byte ceiling; make the payload length
   log-uniform from 1 byte to a few KB so both the tiny-edge and the
   deep cases get sampled. This mode is by far the most expensive per
   iteration and gets its own much smaller iteration count (S3).
5. **Uniform bytes (existing).** Retained per N4.

### S3. Time budget, measured not guessed

The test must state a wall-clock budget and default its iteration counts
to fit it, per mode, on the CI machine. Implementation must **measure**
each mode's per-iteration cost and record the numbers in the spec's
`Implemented in` pass, rather than picking round numbers.

Two constraints on the shape of the answer:

- The **total** default runtime is the budgeted quantity, not the total
  iteration count. Mode 4 is orders of magnitude more expensive per
  iteration than mode 5, so a single shared `N` cannot be right for
  both; each mode carries its own default.
- `PROTOTEXT_SELFTEST_N` and `PROTOTEXT_SELFTEST_SEED` keep working for
  a deliberate long soak, and the long soak stays a manual operation —
  it is not added to `nix-build -A ci`.

Fix the existing doc drift in the same pass: the comment at `:2495` says
the default `N` is 10 000; the code at `:2502` says `1_000_000`.

### S4. Coverage proxy, with a floor that fails

Count, across the run, how many iterations produced at least one nested
message, at least one malformity of each `MalformedKind`, and at least
one depth-capped node. Print the tally.

**Printing is not enough.** Assert a floor on each counter. A generator
that has silently regressed to producing only `INVALID_TAG_TYPE` on byte
one — which is approximately the current state — must *fail* the suite,
not pass it quietly with a disappointing report nobody reads. The floors
should be set well below the measured values, so they catch regression
rather than noise; the measured values go in the spec.

This is the item that would have made the current generator's weakness
visible instead of leaving it to be inferred three years later.

## Open questions to settle before implementing

- **Q1 — is O1's precondition cheaply available?** The spec assumes the
  renderer can report "nothing was degraded" without a new traversal. If
  it cannot, O1 becomes conditional on something more expensive to
  compute, and the right response is to drop O1 and rely on O2/O3/O4
  rather than to reintroduce an input-shape predicate.
- **Q2 — does a depth-capped `START_GROUP` round-trip?** If it does, O1
  is unconditional and O2 is pure insurance. If it does not, O2 is
  load-bearing and the spec's central claim is confirmed. Either answer
  is fine; the answer should be *known* and written down, because it is
  a real statement about the format's behavior at the cap, not a test
  detail.
- **Q3 — what is `knife_schema`'s coverage?** The generator's
  "schema-known field number" draw is only as good as the schema behind
  it. If `knife_schema` has no groups, no packed repeated fields, no
  Any, or no MessageSet, mode 1 cannot reach the corresponding decoder
  paths and the schema needs extending first.

## Test plan

This spec's deliverable *is* a test, so the plan is about how to know it
works rather than about what to assert.

- **Reproduce a known bug.** Before shipping, check the sharpened
  generator against the pre-fix commits for C1 and C2 (both are in
  history) and confirm it finds them, with the iteration count it took.
  A stress test that cannot find the bugs that motivated it is not
  evidence of anything, and this is the only check that distinguishes a
  real improvement from a longer-running version of the same test.
- **Confirm O2 is not vacuous.** Verify at least one generated input
  where O1's precondition fails and O2 still holds — otherwise the
  fourth layer is untested scaffolding.
- **Seed reproducibility.** A recorded failing `(seed, mode, iteration)`
  must reproduce byte-identically on a rerun.
- **Budget.** Time `nix-build -A ci` before and after; the delta must be
  within S3's stated budget.
