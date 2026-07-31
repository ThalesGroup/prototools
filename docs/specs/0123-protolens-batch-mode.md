<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0123 — protolens: non-interactive batch mode (`extract` subcommand)

Status: implemented — but read `extract` as `export` throughout. Spec
        0156 renamed the subcommand, the TUI command and the format
        flags; the variant is `Command::Export` (`main.rs:167`) and its
        options are `--load-overrides`, `--format` and `-o/--output`.
        The design below is unaffected — only every name in it changed.
Implemented in: 2026-07-15
Refs: docs/specs/0156-protolens-export-rename-and-export-format-chord.md
        (renames everything this spec calls `extract`),
      docs/specs/0111-protolens-extract.md (`extract`/`ExtractFormat`,
      `:extract` TUI command),
      docs/specs/0113-protolens-tui-refinements.md (D25's positional-path
      notation, `positional_path`/`sibling_position`),
      docs/specs/0117-protolens-override-collection.md (§4's
      `:save-overrides`/`:restore-overrides` YAML persistence format,
      `run_save_overrides`/`run_restore_overrides`, `resolve_path`/
      `origin_resolves`),
      docs/specs/0122-protolens-override-header-patching.md (fixes the
      round-trip losslessness bug this batch mode exists to regression-
      test against a representative set of fixtures)
App: protolens

## Background

Round-trip losslessness (binary → protolens override/extract → `#@
prototext` text → `prototext encode` → binary, byte-for-byte identical
to the original) is a property that has already broken silently once
(spec 0122's group-framing bug) and is exactly the kind of regression a
one-off interactive TUI session cannot catch systematically. Checking it
against a representative set of fixtures needs `protolens` to be
scriptable: given a blob, a descriptor set, and (optionally) a
previously-saved override collection, produce the extracted `#@
prototext` (or raw binary) rendering of a chosen node and exit — with no
terminal, no event loop, and no user interaction.

`protolens` today has no such mode: `main()` (`protolens/src/main.rs`)
unconditionally constructs a `tui::App` and calls `tui::run(&mut app)`,
which owns the whole program until the user quits interactively. The
pieces batch mode needs already exist independently, unused outside the
TUI event loop:

- `App::new` already performs a full decode and (via its own trailing
  `render_overrides(cursor)` call) the initial override-driven render
  pass, with no TUI involved yet.
- `run_restore_overrides` (spec 0117 §4) already loads a saved YAML
  override collection, validates it against the currently-loaded blob/
  descriptor-set hashes (warning, not blocking, on mismatch — same
  policy this spec reuses unchanged), replaces `App`'s override
  collection wholesale, and re-runs `render_overrides` — exactly "apply
  a previously-saved set of overrides" batch mode needs, verbatim.
- `positional_path`/`resolve_path` (spec 0113 D25) already convert
  between a tree node and a canonical, purely-structural `/1/2/3`-style
  path string (root = bare `/`) — `resolve_path` is already, precisely,
  "the inverse of `positional_path`" (its own doc comment) — exactly
  what's needed to resolve a CLI-supplied path argument to a node.
- `extract::extract`/`extract_binary` (spec 0111) already produce
  either raw-binary or self-contained `#@ prototext`-text output for a
  given node — the exact output batch mode needs to write.

Batch mode is therefore primarily new CLI plumbing tying these four
existing pieces together in sequence, not new rendering, persistence, or
path-resolution logic.

## Goals

- G1: `protolens` gains a `clap` subcommand, `extract` (`protolens/src/
  main.rs`), rather than a flat `--extract <path>` flag — resolving the
  open design question raised during review. Rationale: `prototext`, this
  repo's own sibling CLI, already uses subcommands (`decode`, `encode`,
  `list-schemas`, `score`) for exactly this kind of "pick one distinct
  action, with its own action-specific flags" shape, so a subcommand is
  both more idiomatic (matches existing in-repo precedent) and more
  future-proof (each future batch action, e.g. a hypothetical
  `list-overrides` or `validate`, becomes its own `Command` variant with
  its own flags, rather than accumulating more and more `--xxx`/`--yyy`
  flags at the top level whose *combination* implicitly selects a mode).
  `Cli` gains `#[command(subcommand)] command: Option<Command>` —
  `Option`, not required, so today's zero-subcommand invocation
  (`protolens --descriptor-set ... <blob>`, launching the TUI) keeps
  working completely unchanged (G2). `--descriptor-set`, `--type`,
  `--indent`, `--no-annotations`, `--theme`, and the `blob` positional
  stay exactly where they are today, at the top level, since both the
  TUI and every batch subcommand need them equally.
- G2: batch mode is entered if and only if `cli.command` is
  `Some(Command::Extract { .. })`; `main()` short-circuits before ever
  calling `tui::run` in that case. `Command::Extract`'s own fields:
  - `path` (positional, required): the field path of the node to
    extract, in the same positional-path notation `positional_path`/
    `resolve_path` already use (`/` = the document root; `/1/2` = the
    root's first child's second child; 1-indexed sibling positions, not
    field numbers). Positional rather than a `--extract <path>` flag,
    since under the subcommand design the verb is already spelled out
    by the subcommand name itself (`protolens ... extract /1/2`) — no
    need to also prefix the path argument with "extract" again. A path
    that doesn't resolve (`resolve_path` returns `None`, e.g. an
    out-of-range sibling position or too-deep a path) is a batch-mode
    error: see G5.
  - `--load-overrides <path>`: loads a previously-saved override
    collection (spec 0117 §4 YAML format, the same file
    `:save-overrides` writes) and applies it before extraction. Named
    `--load-overrides` rather than `--restore-overrides` deliberately —
    see the naming discussion below. Optional; when omitted, extraction
    proceeds against the blob's plain automatic overrides only (spec
    0120's Any/MessageSet auto-expansion, which `render_overrides`
    always seeds regardless).
  - `--format <text|binary>`, `-o`/`--output <path>`: see G3.

  `main()`'s control flow for the `Extract` case: load the descriptor
  set and blob and construct `App::new` exactly as today (performing
  the initial decode and automatic-override render pass), additionally
  apply `--load-overrides` if given (reusing `run_restore_overrides`'s
  existing body verbatim — see G4), resolve `path` to a node via
  `resolve_path`, perform the extraction, write/print the result, and
  return — the TUI event loop (`tui::run`) is never entered, no
  terminal raw-mode setup ever occurs.

  **Naming: `--load-overrides`, not `--restore-overrides`.** The TUI
  command stays `:restore-overrides` — unchanged, still spec 0117 §4's
  original name — but the batch-mode flag applying that exact same
  operation is named `--load-overrides` instead. This is a deliberate,
  context-dependent naming split, not an inconsistency: in the TUI, the
  management pane can already hold a live collection (built up
  interactively, or from an earlier `:restore-overrides`/auto-expansion)
  at the moment the command runs, and "restore" correctly signals
  *replace what's there* — its actual, already-specified semantics
  (`run_restore_overrides`'s own doc comment: "replaces the collection
  wholesale"). "Load" would be genuinely ambiguous in that context —
  replace, or merge/add? — precisely the concern raised in review, so
  the TUI command name is deliberately left as is. In batch mode,
  though, there is no such ambiguity to begin with: `App::new` always
  starts from an empty user-authored collection (only the automatic
  Any/MessageSet entries are ever pre-seeded, and those aren't what a
  saved collection would redundantly duplicate), so there is nothing to
  "restore" *from* — the collection is being populated for the first
  time in this process's lifetime. "Load" describes that accurately and
  reads more naturally for a fresh, one-shot CLI invocation. The same
  underlying operation legitimately gets two different names because
  its surrounding state genuinely differs between the two call sites.
- G3: two further `Command::Extract` fields control extraction's
  output, mirroring existing conventions elsewhere in this repo rather
  than inventing new ones:
  - `--format <text|binary>` (`clap::ValueEnum`): selects between `#@
    prototext` text and raw binary output — the same two
    `ExtractFormat` variants `:extract`'s own TUI command already
    offers. Named `--format`, not `--extract-format`: under the
    subcommand design `protolens extract --format text` is already
    unambiguous (no sibling `--theme`-style flag exists at the
    subcommand level to confuse it with). Its default is *conditional*,
    not a fixed `text`: `text` when `--no-annotations` (a top-level
    flag, shared with the TUI) is absent, `binary` when
    `--no-annotations` is present — text output with no `#@`
    annotations cannot be losslessly round-tripped back to binary
    (annotations are exactly what carries wire-type/anomaly facts a
    schema-only text rendering can't otherwise reconstruct — spec
    0122's whole premise), so defaulting to `binary` there avoids
    silently handing back an unusable rendering. Explicitly passing
    `--format text` *together with* `--no-annotations` is a hard error
    at startup (not merely a bad default) for the same reason —
    unlike an unresolvable `path` (G5), this is a static, purely
    argument-driven inconsistency detectable before any decoding even
    starts.
  - `-o`/`--output <path>` (optional): write the extracted result to
    `<path>` instead of stdout. Mirrors `prototext`'s own global `-o`/
    `--output` flag exactly (same short/long spelling, same "omit for
    stdout" default) for cross-tool consistency, rather than requiring
    an output path unconditionally the way `:extract`'s TUI command does
    (a TUI command always targets an explicit file the user just typed;
    a batch/scripting tool should default to stdout so it composes with
    a shell pipeline, e.g. piping straight into `prototext encode`).
- G4: `--load-overrides`'s application logic reuses
  `run_restore_overrides`'s existing body (hash-mismatch warning printed
  to stderr rather than into `App.message`, since there is no status
  line to display it in outside the TUI) — no new override-loading code
  path, no change to the spec 0117 §4 YAML format or its validation
  rules. One outcome differs from the TUI, though: `run_restore_
  overrides` responds to a file-read failure (missing file) or a
  YAML-parse failure (malformed collection) by setting `App.message`
  and returning, leaving the run continuing with overrides unapplied —
  appropriate for an interactive session where the user can just retry
  the command, but wrong for a scripting tool silently producing
  output that doesn't reflect the overrides its caller explicitly
  asked for. In batch mode, either failure is a hard error: print the
  same diagnostic to stderr and exit with a non-zero `ExitCode`
  (same style as G5's unresolvable-`path` error) instead of proceeding.
  Only the hash-mismatch case keeps the TUI's non-blocking-warning
  policy unchanged — a mismatch alone doesn't mean the file failed to
  load or parse, only that it may not have been captured against
  exactly this blob/descriptor-set.
- G5: a non-existent `path` (`resolve_path` returns `None`) is a batch-
  mode error: print a diagnostic to stderr and exit with a non-zero
  `ExitCode`, same style as `main()`'s other existing error paths
  (`--descriptor-set` missing, blob unreadable, decode failure).
- G6: batch mode's extraction reuses `extract::extract_binary`/
  `extract::extract` (or the minimal refactor needed to return a byte
  vector rather than write straight to a `Path`, if `extract`'s current
  file-writing signature doesn't already suit writing to stdout) —
  no new extraction/dedent/header-prepending logic duplicated in
  `main.rs`.

## Non-goals

- No support for loading a saved override collection outside the
  `extract` subcommand (i.e. pre-loading overrides at the top level and
  then continuing into the ordinary interactive TUI) — out of scope for
  this spec; `--load-overrides` only exists as a field of
  `Command::Extract`. A future spec may independently propose that
  combination (and, per G1's reasoning, could do so cleanly as a
  top-level flag or its own subcommand without disturbing this one).
- No new override-collection YAML schema, no change to
  `origin_resolves`'s silent-drop-unresolvable-entries or hash-mismatch-
  is-a-warning-not-an-error policies (spec 0117 §4) — reused exactly as
  is.
- No batch-mode support for *writing* overrides (`--save-overrides` or
  similar) — batch mode only ever *reads* a previously-saved collection;
  producing one is still an interactive-TUI-only operation
  (`:save-overrides`).
- No change to `positional_path`/`resolve_path`'s notation itself (spec
  0113 D25) — batch mode is purely a new consumer of the existing
  scheme, not a redesign of it.
- No batch-mode equivalent of `--type`/root-type inference, `--indent`,
  `--no-annotations`, or `--theme` — these existing flags continue to
  behave exactly as today (`--theme`/colorization are simply irrelevant
  to batch mode's non-interactive stdout/file output and can be safely
  ignored there, not specially disabled).

## Specification

### `Cli`/`Command` shape (`protolens/src/main.rs`)

```rust
#[derive(Parser)]
#[command(name = "protolens", version, about)]
struct Cli {
    /// Batch action to perform and exit. If omitted, protolens launches
    /// the interactive TUI (unchanged, today's only behavior).
    #[command(subcommand)]
    command: Option<Command>,

    // --descriptor-set, --type, --indent, --no-annotations, --theme,
    // and the `blob` positional stay exactly as they are today, at
    // this top level — shared unchanged by both the TUI and every
    // batch subcommand.
    ...
}

#[derive(clap::Subcommand)]
enum Command {
    /// Extract one node's rendering and exit, without entering the
    /// interactive TUI (spec 0123).
    Extract {
        /// Field path of the node to extract, in positional-path
        /// notation (`/` = document root; `/1/2` = root's 1st child's
        /// 2nd child — same notation the TUI's status line already
        /// displays per node).
        path: String,

        /// Previously-saved override collection (spec 0117 §4 YAML,
        /// the same file `:save-overrides` writes) to apply before
        /// extraction. A target-hash mismatch against the loaded
        /// blob/descriptor-set is a warning (to stderr), not a hard
        /// error — same policy as the `:restore-overrides` TUI
        /// command, whose body this reuses verbatim.
        #[arg(long = "load-overrides")]
        load_overrides: Option<PathBuf>,

        /// Output format. Defaults to `text`, or to `binary` when
        /// `--no-annotations` is set — see G3. Explicitly passing
        /// `text` together with `--no-annotations` is a startup error.
        #[arg(long = "format", value_enum)]
        format: Option<ExtractFormatArg>,

        /// Write to this file instead of stdout.
        #[arg(short = 'o', long = "output")]
        output: Option<PathBuf>,
    },
}
```

`ExtractFormatArg` is a small `clap::ValueEnum` mirroring
`extract::ExtractFormat`'s two variants (`Text`, `Binary`) — introduced
because `ExtractFormat` itself lives in a `protolens`-internal module
with no existing `ValueEnum` derive; whether that's a new wrapper type
or `ExtractFormat` itself grows the derive is an implementation
decision, not fixed here.

### `main()` control flow

```
parse Cli
require --descriptor-set (unchanged)
read blob, auto-convert via render_as_bytes (unchanged)
load DescriptorContext (unchanged)
decode via decode::decode (unchanged)
construct App::new (unchanged) — this already performs the initial
  automatic-override render pass (spec 0120)

match cli.command {
    Some(Command::Extract { path, load_overrides, format, output }) => {
        let format = match format {
            Some(f) => f,   // already validated above, before decoding
            None => if cli.no_annotations { Binary } else { Text },
        };
        if let Some(overrides_path) = load_overrides {
            apply it — same body as run_restore_overrides for the
            hash-mismatch case (warning to stderr instead of
            app.message), but unlike run_restore_overrides, a file-read
            or YAML-parse failure is a hard error here (G4): print to
            stderr and exit non-zero, rather than silently continuing
            with overrides unapplied.
        }
        let idx = app.resolve_path(&path)
            .ok_or("error: extract path does not resolve")?;
        let bytes_or_text = extract at idx, per format;
        write to output, or stdout if None;
        return ExitCode::SUCCESS (or FAILURE on any of the above errors);
    }
    None => {
        // unchanged: theme resolution, tui::run(&mut app)
    }
}
```

The `--format text` + `--no-annotations` conflict (G3) is checked
immediately after `parse Cli`, before `--descriptor-set`/blob handling
or `App::new` — genuinely before any decoding starts, matching G3's
claim, not (as an earlier draft of this pseudocode had it) inside the
`match cli.command` arm after `App::new` has already run:

```
parse Cli
if let Some(Command::Extract { format: Some(ExtractFormatArg::Text), .. }) = &cli.command {
    if cli.no_annotations {
        error("--format text is incompatible with --no-annotations");
        return ExitCode::FAILURE;
    }
}
require --descriptor-set (unchanged)
read blob, auto-convert via render_as_bytes (unchanged)
load DescriptorContext (unchanged)
decode via decode::decode (unchanged)
construct App::new (unchanged) — this already performs the initial
  automatic-override render pass (spec 0120)
```

`resolve_path`/`run_restore_overrides`'s bodies are currently private
methods on `tui::App` (`fn resolve_path`, `fn run_restore_overrides`,
both `tui.rs`) — reused as is (widened to `pub(crate)` if `main.rs`
needs to call them from outside the `tui` module; no behavioral change).

### Output writing

Text format: identical bytes `extract::extract`'s `ExtractFormat::Text`
branch already writes to a file (the `#@ prototext: protoc` header plus
dedented, self-contained node text) — written to `output` if given,
else to stdout.

Binary format: identical bytes `extract::extract_binary` already
returns — written to `output` if given, else to stdout (raw bytes,
not further encoded — a caller wanting binary-to-text or text-to-binary
conversion already has `prototext encode`/`decode` for that).

## Test plan

1. `resolve_path("/")`/`resolve_path("/1/2")` behavior is already
   covered by existing tests (`resolve_path_is_the_inverse_of_
   positional_path`) — no new coverage needed for the resolution logic
   itself.
2. New integration-style test (or a `main.rs`-level test harness, if one
   doesn't already exist for this binary): running with
   `--descriptor-set <fixture>.desc <fixture>.pb extract /` (no
   `--load-overrides`) produces the same `#@ prototext` text as the
   fixture's known-good root-level TUI extract.
3. New test: running with `extract <path> --load-overrides <saved>.yaml`
   against a fixture with a saved MessageSet/Any auto-override
   collection produces output reflecting those overrides (i.e. expanded
   MessageSet `Item`/Any content, not raw bytes).
4. New test: `extract` given a path that doesn't resolve (`resolve_path`
   returns `None`) exits non-zero with a stderr diagnostic, and never
   attempts to enter the TUI (no terminal raw-mode side effects —
   easiest to assert indirectly, e.g. that the process exits promptly in
   a non-TTY test harness rather than blocking on stdin).
4a. New test (G4): `extract <path> --load-overrides <missing-or-
   malformed>` exits non-zero with a stderr diagnostic (not the TUI's
   non-blocking "overrides unapplied" behavior) — covering both a
   nonexistent file and a syntactically invalid YAML file. A
   hash-mismatched-but-otherwise-valid collection, by contrast, still
   applies successfully with only a stderr warning (unchanged from
   `run_restore_overrides`'s existing policy) and exits zero.
5. New test: `extract / --format text` combined with `--no-annotations`
   exits non-zero with a stderr diagnostic before any decoding is
   attempted; `extract / --no-annotations` with no explicit `--format`
   defaults to binary output.
6. Round-trip regression: for a representative fixture set, run batch
   mode's `extract /` on the root, pipe the `#@ prototext`-text output
   into `prototext encode`, and assert byte-for-byte equality with the
   original blob — spec 0122's own motivating regression test, now
   scriptable across many fixtures instead of a single manual repro.
7. `reuse lint` passes (no new files besides this spec).
