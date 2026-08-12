<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0228 — the well-known types are the default descriptor set

Status: implemented
Implemented in: 2026-08-04
App: prototext, protolens, reproto (packaging)
Refs: docs/specs/0090-cli-review.md (`PROTOTEXT_DESCRIPTOR_SET`, the one
        env var shared by the whole toolset); docs/specs/0155-… (the
        `<stub>/proto` layout `-O` is allowed to write into);
        docs/specs/0144-protolens-neovim-jump-to-definition.md
        (`--proto-root` and its fallback to `<stub>/proto`);
        docs/specs/0226-a-fixture-shows-every-anomaly.md (the demo the
        convenience is for)

## Background

`PROTOTEXT_DESCRIPTOR_SET` is read by every tool in the repo —
`prototext` (`src/lib.rs:100`), `protolens` (`src/main.rs:51`) and
`reproto-instantiate-schema` (`instantiate_cli.py:96`) — and is set by
nothing in it. A user who enters `nix-shell` gets four binaries and no
schema, so every demo command has to spell out a `--descriptor-set` path,
and the paths that exist in-repo (`prototext-core/fixtures/descriptor.pb`)
are test fixtures with no scoring graph and no `.proto` sources beside
them: no auto-inference, no jump-to-definition.

The sibling private repo `prototools` does set it, from a `protodb`
derivation, via a package `setup-hook` and a shell hook. This repo already
builds most of the same artifact: `default.nix`'s `wktRkyv` compiles the
well-known types listed in `prototext/wkt/SOURCES` into `wkt.desc` and
runs `reproto --schema-db-out` over it, producing
`schemas.desc` + `schemas/hopcroft.rkyv` + `schemas/index.rkyv`. It exists
only to feed `prototext`'s `build.rs` fast path, and it does not pass
`-O`, so there is no decompiled `.proto` tree.

The same review turned up one gap on the packaging side proper. Shell
completions are installed for all four tools, and man pages for three:
`prototext` (`nix/rust.nix:167`), `reproto` (`nix/python.nix:180`) and
`protoscan` (`nix/python.nix:226`). `protolens` has none — there is no
generator for it at all — so `man/man1/` holds three pages and
`user-shell`'s `MANPATH` (`nix/shells.nix:64`) names three packages.

## Goals

- **G1.** The build produces one fully-indexed well-known-types descriptor
  set: the `.desc`, the `hopcroft.rkyv` scoring graph, the `index.rkyv`
  lazy-load index, and the decompiled `.proto` sources.
- **G2.** The `prototools` package exports `PROTOTEXT_DESCRIPTOR_SET`
  pointing at it.
- **G3.** `nix-shell` (`shell.nix`) and `nix-shell dev-shell.nix` both
  have the variable set.
- **G4.** Auto-inference and jump-to-definition work out of the box, with
  no second environment variable: `protolens some.pb` infers a root type,
  and `v` opens the `.proto` source.
- **G5.** Setting it does not change what the test suite tests.
- **G6.** `protolens` ships a man page, like the other three tools, and
  `man protolens` works in both shells.

## Non-goals

- **N1.** No new environment variable. `PROTOTEXT_PROTO_ROOT` stays unset;
  the `.proto` tree is found through `--proto-root`'s existing fallback to
  `<stub>/proto` (`protolens/src/main.rs:256`), which is exactly what the
  layout in S1 is chosen to satisfy.
- **N2.** Not a corpus. This is the well-known types only — the set every
  `.proto` file imports and no user's schema. A user with their own
  descriptor set overrides the variable or passes `--descriptor-set`.
- **N3.** No fallback for `nix-env -i`. A setup-hook only fires inside a
  Nix build or shell environment; a globally installed binary run from a
  plain shell still gets prototext's built-in `google.protobuf.*`
  fallback, which is what it gets today.

## Specification

### The artifact (G1, G4)

- **S1.** The installed layout is a `.desc` with a same-stem sibling
  directory:

  ```
  <pkg>/share/prototools/wkt.desc
  <pkg>/share/prototools/wkt/hopcroft.rkyv
  <pkg>/share/prototools/wkt/index.rkyv
  <pkg>/share/prototools/wkt/proto/google/protobuf/*.proto
  ```

  This shape is not decorative — it is the only one that makes G4 free.
  Every consumer derives its sidecar from the descriptor path with the
  extension stripped: `decode.rs:165-176` reads `<stub>/hopcroft.rkyv` and
  `<stub>/index.rkyv`, `complete.rs:88` reads `<stub>/hopcroft.rkyv`, and
  `main.rs:256` falls back to `<stub>/proto`. One variable therefore
  delivers four capabilities.

- **S2.** `wktRkyv` (`default.nix:321`) writes S1's layout directly. Its
  `reproto` invocation becomes `--schema-db-out="$out/wkt-db.desc"` with
  `-O "$out/wkt-db/proto"`, so the `.desc`, both `.rkyv` sidecars and the
  decompiled sources land in their final relative arrangement in one run;
  nothing is moved or renamed afterwards. `reproto` reserves the
  `--schema-db-out` stub directory for its own `.rkyv` files but
  explicitly allows an immediate `proto` child there (`cli.py:754-770`),
  which is precisely this path.

  **Amended 2026-08-12.** The invocation also passes `--emit-descriptor`.
  reproto suppresses `google/protobuf/descriptor.proto` from `-O` by
  default (spec 0150 N1), which left the one gap in G4: the file is
  compiled into `wkt.desc` like every other WKT, so `v` on a
  `--type google.protobuf.FileDescriptorProto` session resolved the
  declaration and then failed the last check with "proto source not
  found". `googleapisDb` and `customDb` already passed the flag for this
  reason; `wktRkyv` did not.

  The stub is named `wkt-db`, not `wkt`, because `$out/wkt.desc` is
  already taken by the raw `protoc` output that `reproto` consumes, and
  `-I "$out"` hands `reproto` that whole directory.

  The existing `cp` of `hopcroft.rkyv` → `wkt.rkyv` and `index.rkyv` →
  `wkt_index.rkyv` stays: those two names are `nix/rust.nix:210-211`'s
  `WKT_RKYV`/`WKT_INDEX` build-time contract and are unrelated to S1.

- **S3.** A new `wktDb` derivation copies S1's layout into
  `share/prototools/` under its user-facing names (`wkt.desc`, `wkt/`) and
  writes `nix-support/setup-hook` exporting
  `PROTOTEXT_DESCRIPTOR_SET`.

  It exists **only** to carry the hook. The hook cannot live on `wktRkyv`
  itself, because `wktRkyv` is a build input of `prototext` (full)
  (`nix/rust.nix:210`): its setup-hook would then fire inside that
  derivation's build environment, which is the leak S8 exists to prevent.
  Separating them keeps the artifact and the environment contract in
  different derivations, which is what lets `nix-build -A ci` stay a clean
  check.

  It is reproto's schema-DB output that is installed, not the raw `protoc`
  `wkt.desc`, because it is the one whose stem directory holds the
  sidecars.

### Wiring (G2, G3)

- **S4.** `wktDb` joins the `prototools` symlinkJoin's `paths`. Its
  `nix-support/setup-hook` is the only one among them, so the join does
  not have to resolve a conflict.

- **S5.** `user-shell` (`nix/shells.nix:57`) gains `wktDb` in
  `buildInputs`. `nix-shell` sources each build input's setup-hook, so G3
  holds for `shell.nix` with no shellHook change.

- **S6.** `dev-shell` has no toolset packages in `nativeBuildInputs` — it
  builds them from source — so it gets an explicit export in `_hook_env`,
  named in that function's one-line recap alongside `NIXSHELL_REPO`.

- **S7.** `default.nix` passes `wktDb` into `nix/shells.nix`, and adds it
  to `ci` (and `ci-no-clippy`). The hook is the deliverable, so the
  derivation that carries it has to be one `nix-build -A ci` builds;
  otherwise the only thing that ever forces it is a developer entering a
  shell.

### Keeping the tests honest (G5)

- **S8.** Every test that spawns a toolset binary clears
  `PROTOTEXT_DESCRIPTOR_SET` from the child's environment. Without this a
  `cargo test` run *inside the dev-shell* would exercise a different
  descriptor set from the same run under `nix-build`, where the variable
  is unset — a difference that shows up as a test that passes in CI and
  fails on the developer's machine, or worse the reverse.

  Sites: `protolens/tests/batch_export.rs`'s `run` helper covers that
  file. `prototext/tests/e2e.rs` spawns the binary from five places, so it
  gains a `prototext_cmd()` helper that builds the `Command` with both
  variables already removed, and the five sites call it — cheaper to keep
  honest than five separate `env_remove` pairs. Also
  `PROTOTEXT_DEFAULT_DESCRIPTOR`, the deprecated alias
  (`prototext/src/run.rs:400`), so that a stale value in a user's
  environment cannot leak in either.

- **S9.** `reproto/src/reproto/tests/conftest.py` gains an autouse fixture
  deleting both variables. No Python test relies on the fallback today;
  the fixture is there so that none acquires the dependency by accident.

### The man page (G6)

- **S10.** `PROTOLENS_GEN_MAN=<dir> protolens` renders `<dir>/protolens.1`
  from the live clap definition and exits, checked in `main` immediately
  after the `PROTOLENS_COMPLETE` handler and before `Cli::parse()`.

  An environment variable rather than a `protolens-gen-man` binary, which
  is what `prototext` uses. `protolens` is a bin-only crate whose `Cli` is
  private to `main.rs`, so a second `[[bin]]` would have to redeclare
  every module and would compile the whole TUI a second time. A hidden
  subcommand does not work either: `blob` is a required positional on the
  root command, so `protolens gen-man <dir>` would still demand a blob.
  The variable is the mechanism the binary already uses for
  `PROTOLENS_COMPLETE` — print a thing and exit, before parsing — and
  costs nothing to compile. It is documented in the man page's own
  ENVIRONMENT section, beside `PROTOLENS_COMPLETE`.

- **S11.** `nix/rust.nix`'s `protolensPostInstall` generates the page into
  `$out/share/man/man1`, before `wrapProgram`. `user-shell`'s `MANPATH`
  gains `${protolens}/share/man`, and `dev-shell`'s `_hook_man` gains the
  working-tree invocation beside the other three.

## Alternatives considered

**Point the variable at a working-tree copy, as the sibling repo does.**
`prototools`'s `_hook_protodb` installs `protodb.desc` into the repo root
and exports a `${repoRoot}` path. Rejected here: it puts a generated,
multi-megabyte artifact in the working tree that has to be gitignored and
that goes stale silently when `SOURCES` changes. The store path has
neither problem, and nothing in this repo needs to write to the descriptor
set.

**Attach the setup-hook to `wktRkyv` or to `rust.prototext`, and skip the
`wktDb` derivation.** Rejected — see S3. `wktRkyv` is a build input of
`prototext` (full), and `prototext` is a build input of the Python test
derivations, so a setup-hook on either would export
`PROTOTEXT_DESCRIPTOR_SET` inside builds that must not see it.

**Add a second derivation for the `-O` run, leaving `wktRkyv` untouched.**
Rejected: it would run `protoc` and `reproto` over the same inputs twice
and could silently drift from the `.rkyv` the Rust fast path is built
against. Extending `wktRkyv` changes its hash, so `prototext` (full) and
the Python test derivations below it rebuild once — but `prototext` takes
`cargoArtifacts = prototextBare` (`nix/rust.nix:207`), so that is a relink
of one crate with `--features wkt-db`, not a recompile of the workspace,
and it happens once.

**Set `PROTOTEXT_PROTO_ROOT` as well, explicitly.** Rejected (N1): the
`<stub>/proto` fallback already resolves it, and a second variable is a
second thing to keep in sync with the first.

**Ship a larger descriptor set (e.g. googleapis).** Rejected (N2). It is
25.6 MB before its indices, it is not something every schema imports, and
it would make an unrelated corpus the silent default for every command
that omits `--descriptor-set`.

## Test plan

1. `nix-build -A ci` — unchanged and green; proves the variable does not
   leak into the sealed build.
2. `nix-shell --run 'echo $PROTOTEXT_DESCRIPTOR_SET'` — non-empty, and the
   path exists.
3. `nix-shell dev-shell.nix --run 'echo $PROTOTEXT_DESCRIPTOR_SET'` —
   same value.
4. `nix-shell --run 'protolens prototext-core/fixtures/descriptor.pb export /'`
   — with no flag at all, renders field *names*
   (`file { … FileDescriptorProto = 1`) rather than field numbers. That is
   the end-to-end check of S1 through S5 at once: the variable is found,
   the stub resolves, and the sibling `hopcroft.rkyv` makes inference
   possible.

   Not `grpconf/anomalies.pb`, which is the obvious candidate and the
   wrong one: it is engineered to be maximally anomalous (spec 0226), so
   the sweep declines every candidate and falls back to a raw root —
   correctly. Naming its type needs `--type`, which proves nothing about
   the variable. A blob used to test inference has to be one inference is
   expected to succeed on.
5. In protolens, `v` on a field of that blob opens the decompiled
   `descriptor.proto` — the check that `-O` landed where `main.rs:256`
   looks.
6. `cargo test` inside the dev-shell — identical results to `nix-build -A
   ci`'s `rustTests`, which is what S8 buys.
7. `ls <stub>` — `hopcroft.rkyv`, `index.rkyv` and `proto/` all present,
   and no stray `.desc` inside the stub directory (it is reserved).
8. `nix-shell --run 'man -w protolens'` — resolves, and `mandoc -T lint`
   reports no ERROR. Same in `dev-shell.nix`, where the page comes from
   the working-tree `man/man1/` that `_hook_man` regenerates.

   Not "without warnings": `mandoc -T lint` emits STYLE and WARNING lines
   for every page in this repo, and `man -w` prints an `outdated
   mandoc.db` note for every *store* path, because no nixpkgs derivation
   here runs `makewhatis`. Both are pre-existing and identical for
   `prototext`. ERROR count is the signal; the rest is noise that would
   only teach a future reader to ignore the check.
9. `protolens --help` — unchanged: S10 adds no argument, no subcommand and
   no line of help output.

## Measured outcome

Implemented 2026-08-04. All nine items pass.

- **The variable resolves to the same store path in both shells** —
  `<wkt-db>/share/prototools/wkt.desc`, 25 197 bytes, with
  `hopcroft.rkyv`, `index.rkyv` and `proto/` beside it and no stray
  `.desc` in the stub (items 2, 3, 7).
- **Inference works with no flags at all** (item 4):
  `protolens prototext-core/fixtures/descriptor.pb export /` renders
  `file { … FileDescriptorProto = 1` where `--raw` renders `1 { … message`.
- **`nix-build -A ci` is green** with `wktDb` in it (item 1), so the
  setup-hook is forced by CI rather than only by entering a shell, and it
  still does not leak into the sealed builds.
- **The man page** lints at 27 STYLE/WARNING lines and **0 ERROR**,
  against 17 and 0 for the hand-maintained `prototext.1` — same class of
  output, nothing new introduced (item 8). `--help` gained nothing
  (item 9).

What the plan got wrong, corrected above: item 4 originally named
`grpconf/anomalies.pb`, which inference declines by design. That is the
sweep behaving correctly on a deliberately pathological blob, not a
defect — but it made the item unable to distinguish a working
`PROTOTEXT_DESCRIPTOR_SET` from a broken one, which was its whole
purpose.
