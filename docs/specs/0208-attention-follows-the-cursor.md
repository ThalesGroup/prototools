<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0208 — attention follows the cursor

Status: implemented
Implemented in: 2026-07-29
App: protolens
Refs: docs/specs/0117-protolens-override-collection.md (§2, per-kind
        origins),
      docs/specs/0124-protolens-manage-pane-navigation.md (G2, the
        manage pane's `z`/`Z` kind rotation),
      docs/specs/0138-protolens-main-pane-inference-heat-cue.md,
      docs/specs/0152-protolens-heat-cue-background-scoring-thread.md,
      docs/specs/0154-protolens-heat-cue-progressive-display.md (G4,
        `heat_cue_resolve` / `recheck_pending_heat_states`),
      docs/specs/0164-protolens-heat-cue-tiered-priority-and-prefetch.md
        (G1/G2/G3 — the tier ladder and the band ends; G2's Visible
        eviction rule is superseded here),
      docs/specs/0189-a-superseded-request-wave-is-discarded-by-the-worker.md,
      docs/specs/0194-the-cursor-is-a-caret.md (S6, caret motion),
      docs/specs/0199-the-arrow-keys-fold-before-they-leave-the-node.md
        (S3, the fold gutter making `0` and `^` coincide; S9, the guard
        that freed `Ctrl-a`),
      docs/specs/0200-the-selection-pane-returns-to-its-caller.md (S3,
        `override_origin_kind`)

## Background

Four items of user feedback (2026-07-28). The last two share a theme —
the scoring queue should serve what the user is looking at *now* — and
the first two are small independent corrections batched in with them.

### 1. There is no readline-style way to reach either end of a line

The main pane binds vim's `^`/`0` and `$` (`key_dispatch.rs:623-624`,
spec 0194 S6 as amended by spec 0199 S3/S5). Both require a shifted or
awkward key on most layouts, and every other text surface the user
touches — the shell, the `:` command line, every terminal input —
answers `Ctrl-a`/`Ctrl-e` for the same two destinations.

`Ctrl-a` is free. It used to fall into the annotation-display toggle
through an unguarded `KeyCode::Char('a')` pattern; spec 0199 S9 added
`if key.modifiers.is_empty()` precisely so that it no longer does
(`key_dispatch.rs:638`). `Ctrl-e` was never bound.

### 2. A new override defaults to a `path:field` origin

`override_origin_for_kind` (`override_apply.rs:3030`) derives a
`PathField` origin for a brand-new override, falling back to `Path`
only at the wrapper root. That was 2026-07-21 feedback, on the grounds
that a `path:field` origin survives sibling reordering and insertion
better than a plain positional `Path`.

In practice the default is surprising. The user points at *one node*
and asks to retype it; the entry that appears is expressed in terms of
that node's **parent** plus a field number, so it reads as being about
somewhere else, and it silently covers every sibling of that node
carrying the same field number. The robustness argument still holds,
but robustness is not what a default should optimize for here — `z`/`Z`
in the management pane (spec 0124 G2) already promote an entry to
`path:field` deliberately, for the user who wants it.

### 3. The line under the cursor is scored at the same tier as the rest of the screen

`heat_cue_resolve` (`heat_cue.rs:272-313`) is the main pane's per-node
heat-cue lookup. It pushes its request — and promotes its two cache
reads — at `Tier::Visible`, uniformly, for every node on screen. The
comment there gives the reason: it runs every frame for every visible
node just to re-check its own pending status, so it must not
repeatedly jump ahead of a request a genuine user action queued.

That reasoning is right for the other forty-odd lines on screen and
wrong for exactly one of them. The node under the cursor is the node
whose type the status line reports, whose cue the user is waiting on,
and the one `t` will open the selection pane on. It is the *definition*
of a user-attention request, and today it queues behind whatever
`Visible`-tier traffic the rest of the viewport generated first.

The selection pane already gets this right for its own requests:
`recompute_override_candidates` (`override_select.rs:414`) and
`upgrade_active_override_to_complete` (`:489`) both push at
`Tier::User`, on the same 2026-07-20 grounds. Only the main pane lacks
the distinction.

### 4. A query's recency does not affect when it is served

`TieredBounded` (`tiered.rs`) inserts `Visible` entries at its band's
*tail* and pops from its head — FIFO, arrival order. Spec 0164 G3 said
this explicitly: "nothing about background re-verification or read-ahead
calls for reordering by recency of the push itself."

Panning is the counter-example. When the viewport moves, the lines that
have just scrolled into view are unscored and blank; the lines that
were already on screen are either scored already or have a request
sitting in the queue from several frames ago. Under FIFO the stale
requests are served first, so the newly-exposed part of the screen —
the part the user just moved to look at — fills in last. Recency is
exactly the right proxy for attention here.

The same holds for a *re-query*. Today a push whose key is already
tracked, at the tier it is already tracked at, updates the payload in
place and does not move (`upsert`'s `else` branch, `tiered.rs:226-231`)
— and this is true of `User` as much as of `Visible`. So "ask again"
carries no information, even though asking again is the most direct
evidence available that somebody still wants the answer. The governing
model this spec adopts is uniform and much simpler to state: **the most
recently asked query is served first, wherever it sits in the ladder.**
Insert at the head, pop at the head, re-ask moves to the head, evict at
the tail.

Two things follow that are worth stating rather than discovering.

**The eviction exception disappears.** `Visible` is the only band that
`evict_one` drains from its **head** rather than its tail (`tiered.rs:
341`), and spec 0164 G2's reason is that tail-eviction under a
tail-insert discipline would discard the entry just pushed, thrashing
under sustained saturation. That argument is entirely a consequence of
the tail-insert choice. Flip insertion to the head and the band's tail
becomes its oldest entry again, so evicting from the tail means what
evicting from the head means today, and all four bands become uniform.

**A lower-tier touch must still not relink.** A background `Visible`
re-check that merges into an entry already tracked at `User` must not
re-prioritize it (spec 0164 G5, pinned by `lower_tier_push_merging_an_
existing_entry_does_not_reorder_it`, `heat_worker.rs:838`). So the rule
is not "any update relinks" but "an update at least as urgent as the
entry's current tier relinks" — which, spelled `tier >= cur_tier`, also
subsumes spec 0189's "re-asking revives a superseded `Prefetch` entry"
as an ordinary instance rather than a special case.

One measurable risk was considered and dismissed. With a re-query
relinking, the `Visible` band's tail is the least-recently-re-asked
entry; since `heat_cue_resolve` re-asks for every *unsettled* visible
node every frame (`heat_cue.rs:273` returns early on `settled()`, so
the re-asking set is exactly the pending set), the tail is the topmost
pending line of the viewport. Under saturation that line would be
evicted and re-pushed on every frame. It cannot arise in practice:
`evict_one` reaches `Visible` only once both `Prefetch` bands are
empty, and the queue holds `HEAT_REQUEST_QUEUE_MAX_ENTRIES` = 512
against a viewport of a few dozen lines.

## Goals

- **G1**: `Ctrl-a` and `Ctrl-e` reach the same two destinations as `^`
  and `$` in the main pane.
- **G2**: A brand-new override, created from the selection pane on a
  bare main-pane node, gets a plain `path` origin.
- **G3**: The main pane's request for the node under the cursor is
  `Tier::User`; every other visible node stays `Tier::Visible`.
- **G4**: One recency rule for the whole structure: the most recently
  asked query is served first. `Visible` joins `User` in inserting and
  popping at the head; asking again at an entry's own tier (or higher)
  moves it to that tier's head; every band evicts at its tail.
- **G5**: A *read* obeys the same rule as a query — re-reading a cached
  result at its own tier, or at a higher one, moves it to that tier's
  head, so a cache evicts least-recently-read rather than
  least-recently-written.

## Non-goals

- **N1**: No new tier, and no change to the three-tier ladder's
  ordering. `User` still outranks `Visible` still outranks `Prefetch`.
- **N2**: The selection pane's passive re-check
  (`poll_pending_override_work`, `override_select.rs:553`) stays at
  `Tier::Visible`. It is not a request about a line — it is a poll for
  a request the pane already pushed at `Tier::User` when it opened, and
  under S4's `tier >= cur_tier` rule a `Visible` touch of a `User`
  entry neither promotes nor relinks it, which is the intended
  behavior.
- **N3**: `close_override`'s cache demotion (`override_select.rs:278`/
  `:293`) stays at `Tier::Visible`. It is a write of a *result* on the
  way out of a pane, not a query.
- **N4**: `Prefetch` keeps inserting at `prefetch_current`'s **tail**.
  Read-ahead position there encodes distance from the cursor, not age
  (spec 0189), so head-insertion would serve the farthest-away
  read-ahead first. Its re-ask behavior is unchanged too: spec 0189's
  revival rule already relinks a re-asked `Prefetch` entry, and S4's
  condition reproduces it exactly.
- **N5**: `start_new_wave`, `discard_one_superseded` and the
  `prefetch_current`/`prefetch_previous` split are untouched.
- **N6**: The `path:field` kind itself is not deprecated. `z`/`Z` in the
  management pane still rotate an entry onto it, and the selection pane
  still honors an existing entry's own kind (spec 0200 S3).
- **N7**: No change to what `Ctrl-a` used to do. Spec 0199 S9 already
  detached it from the annotation toggle; this spec only fills the
  vacancy.

## Specification

### S1 — `Ctrl-a`/`Ctrl-e` join the line-end motions

In `key_dispatch.rs`'s main-pane match, extend the two existing arms:

```rust
KeyCode::Char('0') | KeyCode::Char('^') => self.caret_to_line_start(),
KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
    self.caret_to_line_start()
}
KeyCode::Char('$') => self.caret_to_line_end(),
KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
    self.caret_to_line_end()
}
```

Separate guarded arms rather than `|`-alternatives, because the
unmodified spellings must stay unguarded: a bare `0`/`^`/`$` is
accepted with any modifier state today and there is no reason to
tighten that here.

Placement relative to the plain-`a` annotation toggle
(`key_dispatch.rs:638`) does not matter — that arm carries
`if key.modifiers.is_empty()` and cannot catch `Ctrl-a` — but the new
arms belong next to the motions they alias, not next to the toggle.

`e` has no unmodified main-pane binding, so `Ctrl-e` introduces no
collision either.

### S2 — the default origin kind is `path`

`override_origin_for_kind` becomes a single infallible derivation:

```rust
pub(super) fn override_origin_for_kind(&self, idx: usize) -> Result<OverrideOrigin, String> {
    self.origin_for_kind(idx, OverrideKind::Path)
}
```

The `Result` stays, even though `OverrideKind::Path` cannot fail: the
one call site (`key_dispatch.rs:166-169`) sits opposite
`origin_for_kind(idx, kind)`, which genuinely can, and both arms must
produce the same type. The wrapper-root fallback the old body carried
disappears with the `PathField` attempt it was guarding.

The doc comment on `override_origin_kind` (`mod.rs:1105-1108`) and the
comment block at `key_dispatch.rs:152-165` both name the "`path:field`
default" and must be reworded. The substance of spec 0200 S3 is
unchanged and still needs stating — an entry opened from the management
pane is still retyped under its own kind, and deriving the default
instead would still leave the old entry active with a second one beside
it. Only the name of the default changes.

### S3 — the cursor's node asks at `Tier::User`

Add one private helper on `App`, in `heat_cue.rs`:

```rust
/// The tier a main-pane heat request for node `idx` is asked at
/// (S3): `User` for the node under the cursor, `Visible` for every
/// other node on screen.
fn heat_tier_for(&self, idx: usize) -> Tier {
    if idx == self.cursor {
        Tier::User
    } else {
        Tier::Visible
    }
}
```

`self.cursor` is a node index, not a line index (`mod.rs:961`), and it
names the node whose bracket pair the caret belongs to whether the
caret rests on the header line or the footer (`mod.rs:962-967`). So the
comparison is exact and needs no line-map lookup.

Apply it at all four `Tier::Visible` sites that are main-pane per-node
queries:

- `heat_cue_resolve`'s queue push (`heat_cue.rs:289-295`);
- `heat_cue_resolve`'s two cache reads (`:301`, `:310`);
- `recheck_pending_heat_states`' two cache reads (`:441`, `:450`).

The cache reads are included, not just the push, because `peek` is a
*promoting* read (spec 0164 G9): a `Visible` peek retags the entry, and
retagging the cursor node's own result at `Visible` would undo the
promotion its request earned and hand it back to eviction ahead of a
result the user is no longer looking at.

`recheck_pending_heat_states` pushes nothing, by design — it only
peeks — so this changes eviction ranking there and nothing else.

The per-frame re-push concern that motivated `Tier::Visible` in the
first place does not return, even though S4 makes a re-push relink.
`heat_cue_resolve` re-pushes only while the node is unsettled, and the
cursor is one node: the `User` band holds its request plus at most the
selection pane's own one or two, so re-asking it each frame moves it
between the head and the second position of a band with a handful of
entries. It cannot starve anything, because a genuinely newer `User`
request is itself head-inserted ahead of it on the frame it arrives.

### S4 — one recency rule for the whole structure

Four edits in `tiered.rs`, which must land together.

**(a) `Visible` inserts at the head.**

```rust
fn link_at_insertion_end(&mut self, tier: Tier, idx: usize) {
    match tier {
        Tier::User => link_at_head(&mut self.slots, &mut self.user, idx),
        Tier::Visible => link_at_head(&mut self.slots, &mut self.visible, idx),
        Tier::Prefetch => link_at_tail(&mut self.slots, &mut self.prefetch_current, idx),
    }
}
```

**(b) Every band evicts at its tail.**

```rust
fn evict_one(&mut self) -> Option<(K, V)> {
    let idx = self
        .prefetch_previous
        .tail
        .or(self.prefetch_current.tail)
        .or(self.visible.tail)
        .or(self.user.tail)?;
    Some(self.remove_by_idx(idx))
}
```

(a) and (b) are one change, not two. Head-insertion with the existing
head-eviction would evict the entry that had just been pushed — the
exact thrash spec 0164 G2 was avoiding, merely relocated. Together they
leave the *set* of eviction victims unchanged: under tail-insert the
head was the band's oldest entry, under head-insert the tail is. Only
`pop_highest`'s choice changes, which is the point.

**(c) `upsert` relinks whenever the caller asks at least as urgently as
the entry is already ranked.**

```rust
if let Some(&idx) = self.index.get(&key) {
    let cur_tier = self.slots[idx].as_ref().expect(..).tier;
    if tier >= cur_tier {
        self.unlink(idx);
        {
            let s = self.slots[idx].as_mut().expect(..);
            s.tier = tier;
            s.value = value;
        }
        self.link_at_insertion_end(tier, idx);
    } else {
        self.slots[idx].as_mut().expect(..).value = value;
    }
    return UpsertOutcome::Applied { evicted: None };
}
```

The `new_tier` local disappears: `tier >= cur_tier` is exactly the
condition under which `cur_tier.max(tier) == tier`, so the max is
redundant inside the branch, and outside it the tier does not change.
The condition is *weaker* than today's `new_tier > cur_tier ||
new_tier == Tier::Prefetch` in one case and identical in the rest:

| entry | pushed at | today | with S4 |
| --- | --- | --- | --- |
| `Visible` | `User` | relink (promotion) | relink |
| `Prefetch` | `Prefetch` | relink (0189 revival) | relink |
| `User` | `User` | in place | **relink to head** |
| `Visible` | `Visible` | in place | **relink to head** |
| `User` | `Visible` | in place | in place |

The last row is the one that must not move, and does not: a background
re-check merging into a request the user asked for must not
re-prioritize it (spec 0164 G5). The second row is spec 0189's
"re-asking revives," which stops being a named special case and becomes
an ordinary instance of the rule.

**(d) `peek` takes the identical condition**, since a read is a query
(G5):

```rust
pub(super) fn peek(&mut self, key: &K, tier: Tier) -> Option<V> {
    let idx = *self.index.get(key)?;
    let cur_tier = self.slots[idx].as_ref().expect(..).tier;
    if tier >= cur_tier {
        self.unlink(idx);
        self.slots[idx].as_mut().expect(..).tier = tier;
        self.link_at_insertion_end(tier, idx);
    }
    Some(self.slots[idx].as_ref().expect(..).value.clone())
}
```

This is the edit that reaches the two `HeatCaches` maps, which never
call `pop_highest` and are therefore affected only in their eviction
order: a cached result now survives on the strength of being *read*,
not merely of having been written recently. Spec 0189's separate
paragraph justifying a same-tier `Prefetch` re-read is preserved by the
same condition that preserves it in `upsert`.

Doc comments to rewrite: `TieredBounded`'s (loses the "except
`Visible`" eviction clause and the `Visible`-is-FIFO clause),
`link_at_insertion_end`'s, `upsert`'s bullet list, `peek`'s, and
`evict_one`'s. The invariant they should all state is G4's single
sentence, with `prefetch_current`'s tail-insertion called out as the
one deliberate exception and N4's reason given once.

## Test plan

1. `Ctrl-a` from mid-line moves the caret to the same column a bare `^`
   does, and `Ctrl-e` to the same column `$` does, on a line where the
   two differ from each other.
2. `Ctrl-a` still does not toggle the annotation display
   (`only_an_unmodified_a_toggles_the_annotation_display`, spec 0199's
   item 23, must keep passing unchanged) — it now has a motion to
   perform instead of nothing, and the assertion is unaffected.
3. Confirming a type on a bare main-pane node with a parent creates an
   entry whose `origin.kind()` is `OverrideKind::Path`, and whose path
   is the node's own positional path — not its parent's.
4. Confirming on the wrapper root still yields `Path` (the case the old
   fallback handled) — now by the ordinary route, with no fallback.
5. Spec 0200 S3 still holds: a pane opened from the management pane on
   a `path:field` entry, confirmed, retypes *that* entry and creates no
   second one. (`override_select.rs:2120-2136` — the assertion that no
   `path:field` entry appeared beside it must be re-pointed at the
   entry count, since a `path:field` entry now legitimately exists only
   because the fixture created it.)
6. With the cursor on node A and node B also visible, the request
   pushed for A is at `Tier::User` and the one for B at
   `Tier::Visible` — read back off the queue, not asserted on tier
   assignment in isolation.
7. Moving the cursor from A to B makes B's next request `Tier::User`
   and leaves A's already-queued entry at `Tier::Visible` (a tier never
   moves down, spec 0164 G5, so A is not demoted — it simply stops
   being re-promoted).
8. `TieredBounded`: two `Visible` upserts pop most-recent-first
   (rewriting the FIFO half of `user_pops_lifo_visible_pops_fifo`, and
   renaming it — both bands are LIFO now).
9. `TieredBounded`: at capacity, a third `Visible` upsert evicts the
   *oldest* of the two, not the one just inserted (rewriting
   `visible_evicts_from_its_own_head_not_the_newest` against the tail
   and renaming it).
10. `TieredBounded`: a same-tier re-upsert moves the entry to its
    band's head — asserted for `User` and for `Visible` separately,
    since these are the two rows of S4(c)'s table that change.
    `heat_worker.rs`'s `pop_returns_most_recently_pushed_first_and_
    merges_do_not_reorder` asserts the old `User` behavior directly and
    must be rewritten, not merely renamed.
11. `TieredBounded`: a *lower*-tier upsert of a `User` entry still
    updates in place without relinking or promoting — the row that must
    not move. Already covered end-to-end by `lower_tier_push_merging_
    an_existing_entry_does_not_reorder_it` (`heat_worker.rs:838`),
    which must keep passing unchanged, as must
    `visible_push_of_a_new_key_does_not_preempt_a_queued_user_request`.
12. `TieredBounded`: a same-tier `Visible` `peek` moves the entry to
    the `Visible` head, so a subsequent eviction spends the entry that
    was *not* re-read. The `Prefetch` revival tests
    (`a_prefetch_repush_revives_a_superseded_key`,
    `a_prefetch_peek_revives_a_superseded_key`) must keep passing
    unchanged — they are the regression guard that S4(c)/(d)'s weaker
    condition still covers spec 0189.
13. Full suite: `cargo test --release --no-default-features -p
    protolens --bin protolens`, then `nix-build -A ci`.

## Measured outcome

Implemented 2026-07-29. The protolens bin suite goes from 515 to 518
tests: 518 passed, 0 failed, 19 ignored. `nix-build -A ci` is green,
`cargo clippy --no-default-features --workspace -- -D warnings` and
`cargo fmt --all --check` are clean, and `reuse lint` reports full
compliance.

Three existing tests changed rather than being added to, each pinning a
behavior this spec deliberately reverses:

- `esc_and_enter_land_in_the_same_place_and_the_default_kind_returns`
  asserted `OverrideKind::PathField`; it now asserts `OverrideKind::Path`
  and, more strongly, that the entry's path is the node's own positional
  path rather than its parent's (S2).
- `user_pops_lifo_visible_pops_fifo` became
  `user_and_visible_both_pop_lifo`, and
  `visible_evicts_from_its_own_head_not_the_newest` became
  `visible_evicts_its_oldest_not_the_newest` — the two halves of S4's
  claim that head-eviction was only ever a consequence of tail-insertion,
  so flipping both leaves the *set* of victims unchanged and only
  reorders service (S4a, S4b).
- `heat_worker.rs`'s `pop_returns_most_recently_pushed_first_and_merges_
  do_not_reorder` pinned the old in-place same-tier merge directly; it is
  now `..._and_a_reask_moves_to_the_head`, asserting that the re-asked
  key jumps back ahead while its window is still the union of the two
  asks (S4c).

The `tier >= cur_tier` condition of S4(c)/(d) turned out to subsume spec
0189's `Prefetch`-revival special case exactly: both revival tests
(`a_prefetch_repush_revives_a_superseded_key`,
`a_prefetch_peek_revives_a_superseded_key`) pass unchanged, and the
`new_tier` local disappeared from both `upsert` and `peek`. The one row
that must not relink — a lower-tier touch of a higher-tier entry — is
still excluded, and remains pinned unchanged by
`lower_tier_push_merging_an_existing_entry_does_not_reorder_it`.

The fifth ask discussed alongside this spec (that the override
management pane's `z` rotate `path -> path:field -> fqdn:field`) needed
no code: `OverrideKind::next()` (`override_pane.rs:110-116`) already
rotates in exactly that order.

## Open questions

None.
