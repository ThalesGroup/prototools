<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0224 — the frame is what notices a scored answer

Status: implemented
Implemented in: 2026-08-01
App: protolens
Refs: docs/specs/0152-protolens-heat-cue-background-scoring-thread.md
        (the worker, the progress event and the recheck this deletes);
        docs/specs/0164-protolens-heat-cue-tiered-priority-and-prefetch.md
        (the bounded request queue whose evictions are the reason the
        recheck set never drains);
        docs/specs/0192-a-frame-costs-the-same-wherever-the-cursor-is.md
        (`heat_dirty` — the deferred repaint that already delivers what
        the recheck was delivering);
        docs/specs/0223-highlighting-yields-to-pending-input.md (the
        monochrome frame, which this restores to meaning what it says)

## Background

Hold `PageUp` for twenty seconds on `googleapis.desc` at the end of the
document, on a hundred-row terminal. Three things happen, and they are
one bug:

- after about ten seconds every frame is drawn without syntax color, and
  stays that way;
- the document keeps scrolling for tens of seconds *after* the key is
  released;
- resizing the terminal then freezes the session — keys ignored, no
  repaint — for as long again, with the activity dot dark.

`pending_heat_recheck` (`tui/mod.rs`) is a `HashSet<usize>` of nodes
awaiting a scoring answer. `heat_cue_resolve` inserts a node when it
draws it unsettled; an entry leaves **only** when that node settles.
Nothing else removes one. But the worker's request queue is capped at
`HEAT_REQUEST_QUEUE_MAX_ENTRIES` (2048, spec 0164), and paging a hundred
rows at eighteen keystrokes a second offers it some thirty-six thousand
nodes: the overwhelming majority of those requests are evicted before
they are ever scored, so their nodes never settle, so the set only
grows.

Every non-`Prefetch` completion sends `AppEvent::HeatWorkerProgress`
(`tui/heat_worker.rs`), and `run_loop`'s arm for it
(`tui/terminal.rs`) answers by scanning that whole set — per entry a
`heat_caches` lock, a `message_payload_range`, a `current_type_key`
(which allocates a `String`), and a `peek_current`.

Measured through a pty at 100x200 on
`/nix/store/qds4bx8dbr64hx474jsk8bvr0dgp05zl-googleapis-db/googleapis.desc`
(driver: `/tmp/resize_freeze.py`; `pending=`/`us=` from a temporary
probe around that match arm, since reverted):

| phase | progress events | set size | median scan | total |
|---|---:|---:|---:|---:|
| during the 20 s hold | 2 590 | 0 → 10 853 | 3.9 ms | 11.6 s |
| after release | 1 888 | ~12 700 | 12.7 ms | **23.6 s of 25 s** |
| after the resize | 1 188 | 12 668 | 7.2 ms | **8.8 s** |

The scan is linear in the set at about 1 µs an entry. Terminal events
share the one `mpsc` channel with those completions, in FIFO order, so
input latency is (queued completions) x (set size). The `Resize` event
in that run was dispatched **8.9 s** after it arrived.

The other two symptoms are consequences of the same arithmetic, not
separate defects:

- **The monochrome screen** is spec 0223 reporting honestly.
  `input_pending` counts unconsumed terminal events, and the queue stops
  reaching empty at the moment one second of wall time no longer holds
  one second of scanning. Per-second, during the hold:

  | s | frames | monochrome | set size |
  |---:|---:|---:|---:|
  | 0-9 | 52 | **0** | 215 → 6 070 |
  | 10 | 43 | 39 | 6 560 |
  | 11+ | 33-56 | **all** | 7 422 → 10 850 |

  For the first ten seconds the loop serviced a continuously repeating
  page key *and still drew in color*. Colour is not lost because the
  user is scrolling; it is lost because the loop stopped keeping up.

- **The dark activity dot** during the freeze is the proof that the
  backlog is on the UI thread: the worker had already drained its queue
  and parked, and only the channel was still full.

## Goals

- **G1.** Holding a page key down does not degrade. Throughput at the
  twentieth second matches throughput at the first, and scrolling stops
  when the key is released rather than seconds later.
- **G2.** A terminal event — key, mouse or resize — is never delayed by
  scoring completions queued ahead of it. Handling one completion is
  O(1) with respect to how far the session has scrolled.
- **G3.** The progressive cue display is unchanged: a row shows `[?]`,
  then `[?/{best}]`, then its cue, as the worker answers, with no user
  action in between.
- **G4.** A monochrome frame again means what spec 0223 says it means —
  the user is outrunning the app — rather than being the permanent
  resting state of a long session.

## Non-goals

- **N1.** Making an evicted request eventually settle. The 2048-entry
  cap is deliberate (spec 0164): a reader who flies past thirty-six
  thousand nodes must be allowed to abandon most of them. Every one of
  those nodes is re-offered the moment it is drawn again.
- **N2.** Changing spec 0223's rule for when a frame is monochrome. G4
  is reached by making the app keep up, not by loosening the rule.
- **N3.** Prioritising terminal events over worker completions, or
  splitting them onto separate channels. See "Alternatives considered".
- **N4.** Any change to the request queue, the caches, `HeatState`, or
  what a cue means.

## Specification

- **S1. Delete `pending_heat_recheck` and `recheck_pending_heat_states`.**
  `run_loop`'s `HeatWorkerProgress` arm keeps only
  `poll_pending_override_work()` — which returns immediately unless the
  override pane is open — beside the `heat_dirty = true` it already
  sets. The arm becomes O(1).

- **S2. The frame is the recheck.** This is the load-bearing claim, and
  it holds because `heat_cue_resolve` already does, per drawn row per
  frame, exactly what the deleted scan did:

  ```rust
  if self.heat_states[idx].settled() { return heat_display(...); }   // memo
  ...
  self.heat_lookup(&range, current_key.as_deref(), ...);             // push
  let state = self.read_heat_state(start, current_key.as_deref(), tier);
  if state.settled() || self.heat_worker.is_some() {
      self.heat_states[idx] = state;                                 // refresh
  ```

  An unsettled node re-reads the shared cache and rewrites its own
  `heat_states` entry on **every** frame in which it is drawn. The scan
  therefore only ever anticipated, by at most one repaint interval, a
  read the very next frame was going to perform anyway — on the same
  node, through the same `read_heat_state`, under the same key
  (`heat_scored_range`, whose doc comment already names both callers as
  the two that must agree).

  Nothing else consumes the result. `heat_states` has exactly three
  readers: `heat_cue_resolve`'s memo above, the two resets in
  `override_apply.rs`, and `prefetch_step_inner`'s skip (S3). No status
  line, footer or pane reads it; `heat_cue_for`/`heat_cue_at` are
  reached only from `render` and from `warm_up_heat_cues`.

  And the repaint that carries it is already owed: spec 0192's
  `heat_dirty` marks the frame and contributes a deadline to
  `recv_timeout`, so a completion is shown within `HEAT_REPAINT_INTERVAL`
  (50 ms) whether or not anything else happens. That is what G3 rests
  on, and it is untouched.

- **S3. Prefetch keeps its skip, and now maintains it itself.**
  `prefetch_step_inner` uses `heat_states[idx].settled()` to skip a node
  without touching the cache, but it never *writes* `heat_states` — it
  relied on the deleted scan to keep that memo warm. Losing the memo
  would not be incorrect (`heat_lookup_ex` would return a hit and push
  nothing) but it would be *slow in a way that matters*: a hit
  `return`s `PrefetchStep::Progressed`, so an already-answered node
  would consume one of the eight guaranteed steps per loop iteration
  instead of being stepped over, and the read-ahead walk would advance
  more slowly the more the worker has answered — precisely backwards.

  So: on a cache hit, prefetch records the state it just proved exists.
  A hit from `heat_lookup` means the window *and* the current type's
  score are both cached (`current_key: None` requires only the window),
  which is exactly `HeatState::settled()`, so one `read_heat_state` at
  the hit turns that node into a permanent skip. One extra lock per
  node, once in its lifetime, against an unbounded set scanned per
  completion.

- **S4. Correct the doc comments the deletion falsifies.**
  `heat_states`' own comment says resolution is "only ever attempted on
  a worker-progress wakeup or a real redraw-triggering input event";
  after S1 only the second half is true. `heat_scored_range`'s comment
  names `recheck_pending_heat_states` as the second caller that must
  agree on the key; after S1 the callers are `heat_cue_resolve` and
  `prefetch_step_inner`.

## Alternatives considered

**Prune the set to the drawn window each frame.** The obvious fix, and
it does bound the scan to a page. Rejected because S2 makes it
pointless: having pruned the set to the rows on screen, the scan then
performs, for those rows, the identical `read_heat_state` that the
repaint arriving milliseconds later performs for the same rows. It buys
a shorter O(page) duplicate in place of an O(session) one. Deleting the
duplicate is strictly better and removes a field.

**Coalesce the progress events** — drain consecutive
`HeatWorkerProgress` from the channel and recheck once. Divides the cost
by the burst length, which would have masked this defect rather than
removing it: one scan at 12.7 ms is still a dropped frame, and the set
would still grow without bound, so the constant would keep getting
worse. It is a sound measure in its own right and remains available if a
future event class turns out to be expensive; it is not needed once
nothing on the path is.

**Give terminal events priority, or their own channel.** The FIFO is
only a problem because one event class costs twelve milliseconds. Fix
the cost, not the ordering. A second channel also means two receive
points and a hand-rolled priority select in a loop that is already a
state machine over seven interdependent locals, for no benefit once
S1 lands.

**Cap `pending_heat_recheck` (MRU, like the queue and the caches).**
Bounds the scan with a constant nobody can derive, and makes the app
choose which nodes to be silently wrong about — while still doing the
duplicate work S2 identifies.

**Let the worker report *which* range it answered**, and recheck only
the nodes covering it. Removes the scan honestly, but needs a
range→nodes index that does not exist and would have to be maintained
across splices. All of that to deliver, early, a value the next frame
computes for free.

## Test plan

1. `a_drawn_row_picks_up_a_cache_answer_with_no_recheck` — draw a frame
   with an unsettled node (`HeatWorkerHandle::stub_for_test`, so nothing
   can settle it behind the test's back), assert the row shows `[?]`;
   write the answer straight into `heat_caches`; draw again and assert
   the cue is shown. This is S2 stated as an assertion, and it is the
   only test that has to pass for the spec to be sound.
2. `a_prefetch_cache_hit_settles_the_node_for_the_next_wave` — with the
   answer already in `heat_caches`, one `prefetch_step` over that node
   leaves `heat_states[idx].settled()`, and the following step moves
   past it (S3).
3. `heat_cue_for_resolves_once_a_real_worker_populates_the_cache`
   (existing, `tui/tests/heat_cue.rs`) — rewrite its poll to call
   `heat_cue_for` instead of `recheck_pending_heat_states`. Its
   two-phase structure (queue against a stub, then `start_for_test` on
   that same queue) must be kept: `heat_cue_resolve` pushes and reads
   back under two separate lock acquisitions, so a worker running during
   that window settles the node before the first assertion.
4. The `#[ignore]`d harnesses in `tui/tests/profiling.rs` emulate
   `run_loop`'s match arm and call the deleted function in four places;
   they drop the call.
5. Manual, on the corpus, with `/tmp/resize_freeze.py` and
   `PROTOLENS_TRACE` — the acceptance test for G1/G2/G4, since
   `run_loop` still has no harness:
   - keystrokes processed in the last second of the hold within 10% of
     the first second's (G1);
   - the last `key` trace line no more than one repaint interval after
     the last keystroke sent (G1);
   - `draw term` for the resize within 100 ms of `TIOCSWINSZ` (G2);
   - no monochrome frame (`styles_us=0`) at any point in the run (G4) —
     the ten colored seconds in the Background say this is reachable,
     not merely hoped for.

## Measured outcome

Same harness, same corpus, same terminal (`/tmp/resize_freeze.py`,
100x200 on `googleapis.desc`, `G`, 20 s of `PageUp`, 25 s settle, resize
to 40 rows):

| | before | after |
|---|---:|---:|
| keystrokes serviced, 1st second of the hold | 16-17 | 42 |
| keystrokes serviced, 20th second | 11-12 | **42** |
| `PageUp` keys sent / `draw key` frames | 413 / — | 413 / **413** |
| last keystroke serviced | tens of s late | 36 ms **before** the key stopped |
| scroll frames after release | ~24 s worth | **0** |
| `draw term` after `TIOCSWINSZ` | 8.9 s | **3 ms** |
| monochrome frames in the run | all after 10 s | **0 of 1359** |

- **G1.** Throughput is flat: 42 keystrokes in the first second and 42 in
  the twentieth, against 16-17 falling to 11-12 before. Every one of the
  413 `PageUp` presses produced its own `draw key` frame, and the last
  one was drawn before the driver stopped sending — so there is no
  backlog left to run off. The 307 frames during the settle window are
  all `draw heat`: read-ahead answers arriving, which is the point.
- **G2.** The resize was drawn 3 ms after the ioctl.
- **G4.** Not one frame in the run was monochrome, against "every frame
  after the tenth second".
