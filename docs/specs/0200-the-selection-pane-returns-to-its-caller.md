<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0200 — the selection pane returns to its caller

Status: implemented
Implemented in: 2026-07-28
App: protolens
Refs: docs/specs/0114-protolens-range-type-override.md (§2/§3/§4, the
        selection pane and its keys),
      docs/specs/0117-protolens-override-collection.md (§2, per-kind
        origins),
      docs/specs/0119-protolens-override-fidelity-and-workflow.md
        (G3 — superseded here),
      docs/specs/0124-protolens-manage-pane-navigation.md (G2, the
        manage pane's `z`/`Z` kind rotation),
      docs/specs/0134-protolens-override-kind-mutation-rework.md,
      docs/specs/0185-the-preview-is-an-overlay.md (S5, the focus lock)

## Background

Three unrelated complaints about the override *selection* pane, sharing
one theme: the pane does not behave like a dialog that was opened by
somebody.

### 1. `q` closes the pane

`key_dispatch.rs:35` binds `Esc`, `t` and `q` all to `close_override`.
In the main pane `q` is `request_quit` (`key_dispatch.rs:504`) — the
application's quit key, with a confirmation prompt behind it. The
selection pane locks focus (spec 0185 S5), so a `q` typed out of habit to
leave protolens does not quit; it silently discards the candidate the
user had just highlighted, with no prompt and no message. Two ways out
(`Esc`, `t`) already exist; the third earns nothing and costs a
collision with the one key in the application whose meaning is
destructive.

### 2. `Enter` always lands in the management pane

Spec 0119 G3 made `Enter` open the management pane with the
just-created entry highlighted, "instead of just closing the pane".
`handle_override_key`'s `Enter` arm (`key_dispatch.rs:169-175`) does it
unconditionally.

The pane already knows who opened it. `override_opened_from_manage` is
set by `open_override_from_manage` (`override_select.rs:350`) and
honored by `close_override` (`override_select.rs:315-319`), so `Esc` and
`t` return to the management pane when that is where they came from, and
to the main pane otherwise. `Enter` is the one exit that ignores it.

For the workflow it was written for — retype an entry from the entry
list — G3 is right, and it stays right. For the other one — `t` on a
node in the main pane, pick a type, look at the result — it is a detour:
the user gets a pane they did not ask for, covering the document they
changed. And the two exits disagreeing about where they land is itself
the defect, independently of which destination is nicer.

### 3. A retype from the management pane changes the entry's kind

Confirming always derives the origin with `override_origin_for_kind`
(`override_apply.rs:2808`), which is `PathField` with a `Path` fallback.
That is the right default for a *new* override — `path:field` origins
survive sibling reordering better than positional `path` ones.

It is wrong when the pane was opened on an existing entry. An entry's
kind is deliberate: the management pane's `z`/`Z` rotate it (spec 0124
G2, spec 0134), and a user who has moved an entry to `fqdn:field` has
said something specific about how that override should match. Retyping it
from the selection pane derives a `path:field` origin instead, and
`OverrideCollection::activate` (`override_pane.rs:295-318`) deactivates
only entries with the *same* origin — so the `fqdn:field` entry stays
active and a second, `path:field` entry is added beside it. One user
action, two active overrides, and the kind the user chose silently
demoted.

## Goals

- **G1.** `q` is not bound in the override selection pane.
- **G2.** `Enter` returns to whichever pane opened the selection pane —
  the management pane when opened from there, the main pane when opened
  with `t`. `Esc` and `t` already do; all three now agree.
- **G3.** When the management pane opened the selection pane, returning
  to it highlights the entry that was just retyped, exactly as spec 0119
  G3 specified.
- **G4.** Confirming an override uses the origin kind of the entry the
  caller was editing, when there is one, and `path:field` (falling back
  to `path`) when there is not.

## Non-goals

- **N1.** Binding `q` to quit in the selection pane. Unbinding is not
  rebinding: whether a modal pane should offer a global quit at all is a
  separate question, and answering it by accident here would be worse
  than leaving `Esc` as the only fast exit.
- **N2.** The management pane's own `q` (`manage_pane.rs:363`), which
  also closes. Out of scope — it is reachable from the main pane by
  `o`, is not focus-locked, and nobody reported it.
- **N3.** Changing what `Esc` or `t` do. They are already correct; G2
  makes `Enter` match them, not the other way round.
- **N4.** A kind rotation in the *selection* pane. Kind mutation stays
  the management pane's `z`/`Z` (spec 0124 G2). G4 propagates a kind; it
  does not let the user pick one here.
- **N5.** Changing `override_origin_for_kind`'s `PathField` → `Path`
  fallback, or the default for a brand-new override.

## Specification

### S1. `q` is unbound

```rust
KeyCode::Esc | KeyCode::Char('t') => self.close_override(),
```

`q` then reaches the arm list's `_ => {}` and does nothing at all, which
is the intended outcome — the selection pane deliberately swallows keys
it does not use, exactly as `Tab` is swallowed with a message (spec 0185
S5). No message is added for `q`: `Tab` gets one because a user has a
specific expectation of it that the focus lock defeats, whereas `q` has
no meaning here to explain.

### S2. `Enter` returns to the caller

The tail of the `Enter` arm (`key_dispatch.rs:159-175`) becomes:

```rust
// Spec 0119 G3, narrowed by spec 0200 G2: land in the management
// pane and highlight the entry just created/reactivated — but only
// when the management pane is where this pane was opened from.
// `activate` guarantees at most one entry per origin is active, so
// this origin/type pair unambiguously identifies it.
let target_highlight = self
    .overrides
    .entries()
    .iter()
    .position(|e| e.origin == origin && e.r#type == new_fqdn);
let returning_to_manage = self.override_opened_from_manage;
self.close_override();
if returning_to_manage {
    self.manage_highlight = target_highlight.unwrap_or(0);
    self.manage_scroll = 0;
    self.last_manage_highlight = None;
    self.manage_pan_offset = 0;
}
```

`manage_open` and `manage_focus` are no longer set here at all:
`close_override` already sets them from `override_opened_from_manage`,
and it *clears* that flag as it goes — which is why it has to be read
into `returning_to_manage` first.

`close_override`'s doc comment currently says the `Enter` call site
"already sets these same three fields itself right after calling this,
so setting them here too is harmless there". After S2 that is no longer
true in the harmless direction — it is the only thing that sets them —
so the comment is rewritten to say `close_override` owns the return, and
`Enter` only adds the highlight.

`target_highlight` is computed *before* `close_override` for the same
reason it is today: it is a position in `overrides.entries()`, which
`close_override` does not touch, but keeping the two adjacent to the
`activate` call that created the entry is what makes the position
readable.

### S3. The caller's origin kind

A new field on `App`, beside `override_opened_from_manage`:

```rust
/// The origin kind to confirm a new type under, when the selection
/// pane was opened on an *existing* entry (spec 0200 G4). `None` — the
/// pane opened on a bare node — means the `path:field` default of
/// `override_origin_for_kind`.
override_origin_kind: Option<OverrideKind>,
```

- `open_override_from_manage` sets it to `Some(entry.origin.kind())`,
  read from the same `entry` it already reads `origin` and `r#type` from.
- `close_override` clears it to `None`, in the same place it clears
  `override_opened_from_manage` — except that it must clear
  unconditionally, since the flag it sits beside is only *read* inside an
  `if`.
- No other opener sets it. `toggle_override` and the other main-pane
  entry points leave it `None`, which is the default.

The `Enter` arm's origin derivation becomes:

```rust
let origin = match self.override_origin_kind {
    Some(kind) => self.origin_for_kind(idx, kind),
    None => self.override_origin_for_kind(idx),
};
let origin = match origin {
    Ok(origin) => origin,
    Err(e) => {
        self.message = format!("cannot create override: {e}");
        return;
    }
};
```

The error path is unchanged and deliberately not softened into a
fallback: if `origin_for_kind(idx, FqdnField)` now fails, the parent's
type has become unresolved since the entry was created, and silently
retyping under a different kind is precisely the defect Background 3
describes. Saying so and doing nothing is the correct answer.

#### Why a kind rather than the whole origin

Storing `Some(entry.origin.clone())` and using it verbatim would also
fix Background 3, with no rederivation at all. The kind is stored
instead because `Enter` from the main pane has to rederive anyway, so
carrying a kind keeps one code path (`origin_for_kind`) for every
confirm, and because the rederivation is what keeps the origin agreeing
with the *current* tree: the selection pane's target node is chosen by
`open_override_from_manage` from the nodes the entry currently affects,
and deriving from that node is the same operation that produced the
entry in the first place.

## Alternatives considered

### A1. Keep `Enter`'s unconditional management-pane landing, and make `Esc` unconditional too

Also makes the exits agree, in the other direction. Rejected: it forces
the pane on the `t`-from-the-main-pane workflow, which is the common one,
and spec 0119 G3's own justification ("instead of just closing the
pane") was about discoverability of a then-new pane rather than about
where the user wants to be.

### A2. A confirm-and-stay key alongside `Enter`

`Enter` closes to the caller, some other key confirms and opens the
management pane. Rejected: a fourth exit from a pane that already has
three, for a destination one keystroke (`o`) away from the main pane.

### A3. Let `Enter` in the selection pane rotate/choose the origin kind

Rejected — N4. The selection pane's job is picking a *type*; the
management pane owns origins, and `z`/`Z` there already do this with a
visible current value. G4 is about not *losing* a kind, not about
setting one.

### A4. Have `activate` deactivate every entry affecting the same node, not just the same origin

Would make Background 3 harmless rather than fixing it, and would be a
large behavioral change to the collection semantics — cross-kind
deactivation is exactly what spec 0117's per-kind origins exist to
avoid. Rejected.

## Test plan

1. **`q` in the selection pane does nothing.** The pane stays open, the
   highlight does not move, and no override is created. Today it closes.
2. **`Enter` on a pane opened with `t` closes it and leaves the
   management pane closed.** G2, and the assertion that fails today.
3. **`Enter` on a pane opened from the management pane returns there,
   highlighting the retyped entry.** G3 — spec 0119 G3's own guarantee,
   now conditional; this is the test that keeps it.
4. **`Esc` from either caller lands where `Enter` does.** The agreement
   G2 is really about, asserted as a pair rather than one side at a
   time.
5. **Confirming from the main pane still produces a `path:field`
   origin.** N5 — the default is untouched.
6. **Retyping an `fqdn:field` entry from the management pane produces an
   `fqdn:field` origin, and exactly one active entry.** G4, and the two
   halves of Background 3's defect: the kind, and the absence of the
   second entry.
7. **Retyping a `path` entry from the management pane produces a `path`
   origin.** The other non-default kind, so the fix is not special-cased
   to `fqdn:field`.
8. **After returning from a management-pane-opened session, a fresh `t`
   from the main pane defaults to `path:field` again.** The clearing half
   of S3, which is what a leaked `override_origin_kind` would break — and
   which no other test would catch, since it only shows up on the
   *second* pane opening.

## Open questions

**Q1. Should the selection pane's status line say which origin kind a
confirm will use?** Not proposed. The kind is visible in the management
pane, which is where it is chosen, and the selection pane's status line
is already carrying the target path, the sort mode and the candidate
count. Worth revisiting if G4 turns out to surprise anyone.

## Measured outcome

Implemented 2026-07-28. `q` dropped from the pane's key arm in
`key_dispatch.rs` (S1); the `Enter` arm now reads
`override_opened_from_manage` into a local *before* `close_override`
clears it, and restores the management pane only if that local is set
(S2); `override_origin_kind: Option<OverrideKind>` added to `App`, set
by `open_override_from_manage` from the entry's own origin and cleared
unconditionally by `close_override` (S3).

Test-plan items 1-8 landed as three new tests plus two rewritten ones
in `protolens/src/tui/tests/override_select.rs`. The two rewrites are
the point of interest: `override_pane_q_closes_pane` became
`override_pane_q_is_unbound`, and
`enter_key_applies_override_and_closes_pane` lost its `manage_open`
assertion, which had pinned spec 0119 G3 as unconditional. Both were
asserting the defects, not guarding against them.

One thing the test plan got wrong. Item 6 asked for "exactly one active
entry", and it was written as a total-count assertion — which fails,
legitimately. `OverrideCollection::activate` stores one entry per
*(origin, type)* pair: retyping an origin deactivates the old entry but
keeps it in the list and pushes a new one, so the total grows by one
every time. The defect was never about the list length. It is about how
many entries are simultaneously *active* for one origin, and about a
`path:field` entry appearing beside an `fqdn:field` one. The test now
asserts exactly those two things, and the count of inactive history
entries is correctly none of its business.
