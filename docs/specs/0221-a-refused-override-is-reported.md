<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0221 — a refused override is reported, not swallowed

Status: draft
App: protolens
Refs: docs/specs/0202-an-override-is-refused-rather-than-fatal.md (made
        a bad override a refusal instead of a panic; this spec is about
        where that refusal is *reported*),
      docs/specs/0147-protolens-status-message-command-line-split.md
        (G5: every keypress dismisses `self.message` — the lifecycle
        that makes it the wrong channel for a startup refusal),
      docs/specs/0197-the-descriptor-set-is-loaded-on-demand.md (§S3
        prints its fallback warning to stderr from `main.rs` before the
        TUI starts; this spec reuses that placement exactly),
      docs/specs/0198-a-batch-subcommand-that-only-starts-up.md (S2's
        rule that a successful batch subcommand writes nothing to
        stderr, which bounds the fix)

## Background

Spec 0202 made an override that cannot be applied a **refusal**: the
node keeps its previous rendering and the reason goes into
`self.message`. That was the right call — a malformed override should
not take the session down. But `self.message` is the status line, and
the status line is a *transient notice* channel with an aggressive
lifecycle. Three consequences, each verified against the code:

**1. In batch mode the refusal is invisible and the exit code is 0.**
`resettle_node` records the refusal at `override_apply.rs:1071`:

```rust
Err(e) => {
    self.message = format!("cannot apply override: {e}");
    None
}
```

`render_overrides` is the pass that calls it, and `load_overrides` runs
one at `command_line.rs:833`. `main.rs`'s `Export` arm never reads
`app.message` — it checks only `load_overrides`' own `Result`. So

```
protolens blob.pb export / --load-overrides f.yaml
```

can write a rendering in which an override was silently not applied,
and exit `SUCCESS`. For `--format=descriptor-binary` (which *requires*
`--load-overrides`) the exported schema is then wrong in exactly the
way the user was trying to control, with nothing on stderr to say so.

**2. In the TUI a startup refusal is destroyed before the first
frame.** `App::new` runs a full `render_overrides` pass, so it can set
the message. Then `warm_up_heat_cues` overwrites it unconditionally
with its own progress string (`mod.rs:2414`) and clears it
(`mod.rs:2424`):

```rust
app.message = format!(
    "Computing inference cues for the initial view: {}/{} lines scored...",
    i + 1, lines.len()
);
...
if Instant::now().duration_since(start) > WARMUP_FIRST_DRAW_DELAY {
    app.message.clear();
}
```

Even on a document too small to trigger the warmup redraw, the first
keypress clears it: `key_dispatch.rs:361` clears `self.message` ahead
of every dispatch branch, by design (spec 0147 G5).

**3. N refusals collapse into one.** The assignment is `=`, not a
push, so only the last refusal of a pass survives, and the user has no
way to learn there were others.

A fourth, adjacent silence belongs with these: `retain_resolvable`
(`override_pane.rs:483`) drops every loaded entry whose origin no
longer resolves against this blob, and returns nothing. That one is
silent in the TUI too.

The common cause is that a refusal is being reported through a channel
built for "pattern not found: xyz" — something the user just did, which
they will see immediately and which is worthless a second later. A
refusal raised during startup is the opposite: the user did not do it,
it happened during a bulk pass before the first frame, and it stays
true until the overrides change.

**The channel that already fits is stderr, and it is free at that
moment.** Every startup phase already reports there — `main.rs:305`,
`:311`, `:443`, `:456` — and those are plain appended `eprintln!`s, not
a progress bar: nothing on stderr overwrites anything, so the third
consequence above simply does not arise there. Two further facts make
it the right place rather than merely an available one:

- **It is empty in batch.** `announce` (`main.rs:373`) is true only for
  the TUI and for `exit`, so `export`'s stderr carries nothing today —
  spec 0198 S2 and its `tests/batch_export.rs` assertion depend on
  that. A refusal line is therefore the only thing on it.
- **The TUI has not taken the terminal yet.** `App::new` is called at
  `main.rs:460`; `tui::run` enters the alternate screen afterwards. A
  refusal raised by `App::new`'s pass can be printed by `main.rs`
  before any frame exists, which is precisely where spec 0197 §S3 puts
  its fallback warning (`main.rs:338`).

A refusal raised *during* the session is a different case and needs no
change: the user just ran `:load-overrides` or edited an entry, they
are looking at the screen, and `self.message`'s dismiss-on-keypress
lifecycle is correct for it. Only the startup pass is broken.

## Goals

- **G1.** A batch `export` that could not apply every override it was
  given says so on stderr and does not report success.
- **G2.** A refusal raised while starting the TUI reaches stderr too,
  where the progress lines cannot overwrite it and the user can scroll
  back to it after quitting.
- **G3.** Every refusal is reported, not just the last one.
- **G4.** A refusal line is self-explanatory: it names the overrides
  file, the node, the type that was asked for, and why it could not be
  applied. A user who has never read this spec must be able to act on
  it.

## Non-goals

- **N1.** Turning `self.message` into a queue, a log, or a scrollback.
  Its lifecycle is correct for what it is for (spec 0147 G5), and for
  a refusal the user just caused it is the right channel unchanged.
- **N2.** Making a refusal fatal again. Spec 0202 settled that, and the
  interactive session must keep running. G1 changes the *exit code of a
  batch export*, which is a different question from whether the
  in-process pass aborts.
- **N3.** Reporting refusals per node in the main pane. A refused node
  renders as whatever it was before, which is already the honest
  display; annotating every one of them is a bigger feature than the
  defect warrants.
- **N4.** Fixing `warm_up_heat_cues`' unconditional
  `app.message.clear()` (`mod.rs:2424`). It is a real clobber of any
  message set before it, but once startup refusals go to stderr no
  known message reaches it, so there is nothing to fix in service of
  this spec. Noted here so the next reader does not think it was
  missed.
- **N5.** A stderr line for a refusal raised *mid-session*. The TUI
  owns the terminal then, and the status line already reports it.

## Specification

- **S1.** `App` gains `refusals: Vec<String>`, cleared at the start of
  each `render_overrides` pass and pushed to by `resettle_node`'s `Err`
  arm in place of the `self.message` assignment. One entry per refused
  node.

  `render_overrides` must not print: it also runs mid-session with the
  terminal in raw mode. Collecting lets the caller decide, which is
  what S3 and S4 exercise.

- **S2.** A refusal line is written for a reader who does not know
  protolens's internals (G4). Its shape:

  ```
  error: --load-overrides 'theme.yaml': google.protobuf.FileDescriptorSet.file[3].options:
    cannot render as 'google.protobuf.FileOptions': <reason from the splice>
  ```

  Four parts, each earning its place: the **flag and file**, because a
  batch caller may pass one of several and the message is the only
  thing it gets; the **node path**, because the file names nodes by
  origin and the user must find the offending entry in it; the
  **requested type**, because that is what they wrote and what is
  wrong; and the **reason**, which is `splice_override`'s existing
  error. In the TUI the flag-and-file part is replaced by whatever the
  user typed (`:load-overrides theme.yaml`).

- **S3. Batch (`main.rs`'s `Export` arm).** After `load_overrides` and
  before writing any output: print each entry of `app.refusals` to
  stderr per S2 and return `ExitCode::FAILURE`. This does not violate
  spec 0198 S2 — that rule binds a *successful* export, and this one is
  not.

  The non-zero exit is deliberate and is a **behavior change**: today
  the same invocation exits 0. It was weighed against warning-and-
  succeeding (see Alternatives) and settled in favor of failing,
  because the exit code is the only signal a script piping
  `export --format=descriptor-binary` into a build receives. Nothing is
  written to the output path when this happens — a half-applied schema
  on disk beside a failed exit is worse than no file.

- **S4. TUI startup.** After `App::new` returns and before `tui::run`
  is called, `main.rs` prints each entry of `app.refusals` to stderr
  the same way, alongside spec 0197 §S3's fallback warning
  (`main.rs:338`) and the phase lines. `App::new` does not print them
  itself — `main.rs` is the only place that knows the terminal is still
  ours. It additionally sets `app.message` to a count, `"N override(s)
  refused — see stderr"`, so a user who never looks at stderr still
  learns the document is not what they asked for.

- **S5.** A refusal raised mid-session keeps today's behavior: the
  status line, now carrying the count when a pass refuses more than
  one. No stderr write (N5).

- **S6.** `retain_resolvable` returns the number of entries it dropped;
  `load_overrides` turns a non-zero count into one more entry in the
  `warnings` vector it already returns, which both callers already
  print (`main.rs:486`) or show (`command_line.rs:852`). This requires
  widening `warnings` from `Vec<&'static str>` to `Vec<String>`. No new
  channel: the hash-mismatch warning already proves this one works.

## Alternatives considered

### Print refusals to stderr from `resettle_node` itself

One line, no new field, and it fixes the batch case immediately.
Rejected because `render_overrides` runs inside the TUI with the
terminal in raw mode and the alternate screen active — writing to
stderr there corrupts the display. The reporting decision has to sit
with the caller, which is what S1 says.

### Carry a startup refusal in the splash pane instead of on stderr

Spec 0197 §S3's warning also reaches a splash pane, which survives
until the user dismisses it — a longer life than a stderr line the user
may have scrolled past. Rejected as a second channel bought for very
little: the refusal is on stderr either way, the splash pane is not
readable after the session ends, and a batch caller cannot see it at
all. The status-line count in S4 already covers the user who never
looks at stderr.

### Save and restore `app.message` across `warm_up_heat_cues`

The original shape of this spec, back when the refusal stayed in the
status line: stop `mod.rs:2424` clearing unconditionally. Rejected
because it fixes only *where the message is destroyed*, not that a
transient line dismissed by the next keypress is the wrong home for a
startup-time refusal, and it does nothing for batch. See N4.

### Make the batch export exit 0 with warnings on stderr

Consistent with the existing hash-mismatch warning, which warns and
succeeds. Rejected for refusals specifically: a hash mismatch means
"this file may not be for this blob, but I applied it"; a refusal means
"I did not apply what you asked for". A script that pipes
`export --format=descriptor-binary` into a build has no way to notice
the difference except through the exit code.

This was the one judgment call in the spec. Settled in favor of
failing; see S3.

### Keep everything in `self.message` and just lengthen the timeout

Does not help batch mode at all (nothing reads the field), does not
survive the keypress clear, and makes every ordinary notice stickier to
fix a case that is not a notice.

## Test plan

1. `export_with_an_unappliable_override_fails_loudly` — G1/S3: a
   `--load-overrides` file whose entry cannot be spliced produces a
   non-empty stderr and `ExitCode::FAILURE`, and writes no output file.
2. `a_refusal_line_names_file_node_type_and_reason` — G4/S2: the
   emitted line contains all four parts, asserted individually so that
   dropping one is a test failure rather than a cosmetic diff.
3. `export_with_every_override_applied_still_says_nothing` — the spec
   0198 S2 contract is intact: the success path's stderr is still
   empty.
4. `refusals_are_collected_not_overwritten` — G3/S1: a pass with two
   unappliable overrides leaves two entries in `refusals`, where today
   the second `self.message` assignment destroys the first.
5. `a_session_refusal_still_uses_the_status_line` — S5/N5: a refusal
   raised after startup sets `self.message` and writes nothing to
   stderr.
6. `load_overrides_warns_about_dropped_entries` — S6: a file with one
   resolvable and one unresolvable origin returns a warning naming the
   dropped count.

## Measured outcome

Filled in at implementation.
