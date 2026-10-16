<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0420 — protolens opens the user's editor

Status: implemented
Implemented in: 2026-10-14
App: protolens, Nix packaging, demo shells
Refs: docs/specs/0144-protolens-neovim-jump-to-definition.md (the `v` key,
      the `--listen` handoff this keeps for Neovim);
      docs/specs/0145-protolens-neovim-lsp-config.md (the bundled config
      this removes);
      docs/specs/0410-protolens-builds-and-tests-anywhere.md (S2's
      messages, generalized here; S3, the recipe options this removes);
      docs/specs/0395-the-teleprompt-deck-reads-more-easily.md (the demo
      config that takes over);
      docs/specs/0146-reproto-emit-buf-yaml.md (why `buf lsp serve` finds
      its root without help);
      docs/specs/0405-prototools-in-nixpkgs-one-small-pr-at-a-time.md (the
      shared version)

## Background

The editor key (`v`) opens the `.proto` declaring the type under the
caret. Today it always runs Neovim, with protolens's own config:

- the Nix wrapper sets `PROTOLENS_NVIM_CONFIG` to a bundled `init.lua`,
  and protolens passes it with `-u`, so the user's own `init.lua` is
  never read in that session;
- the bundled config is the demos' taste, not protolens's business: the
  `desert` colorscheme, line numbers, `h`/`l`/arrow keys remapped to wrap
  across lines, hand-written `.proto` syntax rules (Neovim ships its own),
  and the buf language server with `gd`/`gr`/`K`;
- our Nix outputs wrap protolens with a Neovim and buf on its PATH, and
  the nixpkgs recipe offers `withNeovim` and `withBuf` for the same. A
  nixpkgs review asked what the two options are for; the honest answer
  was the demos.

Every tool that opens an editor solves this the same way: run the
editor the user chose (`VISUAL`, then `EDITOR`), positioned at the line,
and leave its configuration to the user. The demos already have their
own Neovim config, `nix/demo-nvim/init.lua`, with the same colorscheme,
syntax and buf client; reproto writes a `buf.yaml` at the root of every
`.proto` tree it generates (spec 0146), so any buf setup finds its
workspace.

## Goals

- **G1.** `v` runs the user's editor, chosen as most Unix tools choose
  it, with the user's own configuration, positioned at the declaration.
- **G2.** protolens ships no editor configuration, and its packages
  bundle no editor: the nixpkgs recipe has no editor options and no
  wrapper.
- **G3.** Neovim keeps what it has today: one instance reused across `v`
  presses, exact line and column.
- **G4.** The demos look and behave as today: their shells and image
  provide Neovim, buf and the demo config themselves.

## Non-goals

- **N1.** A protolens-specific editor variable (`PROTOLENS_EDITOR`).
  `VISUAL` and `EDITOR` are what users already set; a third is one more
  thing to document for the rare user who wants a different editor here
  than everywhere else.
- **N2.** Per-editor line-and-column syntax beyond Neovim and Vim
  (Helix's `file:L:C`, kakoune's `+L:C`). `+LINE` reaches the line
  everywhere; a table can come later if someone asks.
- **N3.** Reusing a session of any editor but Neovim. It needs a remote
  API; Vim's (`--servername`) exists only in builds with clientserver
  support, usually X11.
- **N4.** Language support of any kind. It is the user's editor setup's
  job, or the demo's.

## Specification

- **S1. Which editor.** The value of `VISUAL` if set and non-empty, else
  of `EDITOR`, else `nvim` if it is on PATH, else `vi`. The value is a
  command, possibly with arguments (`EDITOR="code --wait"`), run as git
  runs it: executed directly when it holds no shell metacharacter or
  space, else as `sh -c '<value> "$@"' <value> <args>…`.
- **S2. Which kind,** from the basename of the command's first word,
  without running it: *Neovim* for `nvim`; *Vim* for `vim`; *other*
  otherwise, `vi` included, which may be nvi or BusyBox vi, neither of
  which understands `+call cursor(…)`, while both take `+LINE`.
- **S3. Neovim,** as today minus the config: the command, then
  `--listen <socket> "+call cursor(L,C)" <file>`, in its own process
  group; when suspended, protolens takes the terminal back and keeps the
  instance; the next `v` opens the file in it with `--server <socket>
  --remote-tab` and `--remote-send` of the cursor call, using the
  command's first word rather than a hard-coded `nvim`, then resumes it.
  No `-u`.
- **S4. Vim and others:** the command, then `"+call cursor(L,C)" <file>`
  for Vim, `+L <file>` otherwise. The editor runs in protolens's own
  process group, with the terminal restored to cooked mode and the
  alternate screen left, and protolens waits for it to exit, then
  redraws. Ctrl-Z in the editor suspends both, back to the user's shell,
  as with `git commit`; `fg` resumes both. While it waits, protolens
  ignores SIGINT and SIGQUIT, as git does, so a Ctrl-C meant for the
  editor does not end protolens; it restores them afterwards.
- **S5. The environment.** protolens keeps exporting
  `PROTOTEXT_PROTO_ROOT` to the editor (a user's config may use it), and
  nothing else of its own.
- **S6. Messages** (spec 0410 S2, generalized): an editor that cannot
  be started is named, with the variable it came from: `nvim not found
  (from EDITOR): set VISUAL or EDITOR to your editor`. No buf notice:
  protolens no longer knows about buf.
- **S7. Removals:** `protolens/nvim/init.lua`; `PROTOLENS_NVIM_CONFIG`
  and its dev-build fallback in `neovim.rs`; the wrappers in
  `nix/rust.nix` (`wrapProtolens`, `protolens`, `protolensLean`), so our
  `protolens` output is the plain binary; `withNeovim`, `withBuf`, the
  `neovim` and `buf` inputs and the wrapper in the staged nixpkgs recipe,
  and the check of their combinations in `default.nix`.
- **S8. The demos** provide what the wrapper did:
  - the GreHack and GRPConf demos' `demoEnv` (exported by their native
    shells, `nix/demo-shells.nix`, and by the workshop image,
    `nix/grehack2026.nix`) gain `VISUAL=nvim`, `EDITOR=nvim` and `VIMINIT`
    sourcing `nix/demo-nvim/init.lua`; their tools already include `neovimLean`
    and `bufLean`, and `protolensLean` gives way to `protolens`.
    `VIMINIT` rather than a session-wide `XDG_CONFIG_HOME`: only Vim and
    Neovim read it, so `gh`, git and other programs in a demo shell keep
    the presenter's own configuration. `neovimLean` sets no `VIMINIT` of
    its own (`wrapRc = false`). teleprompt's wrapper keeps its
    `XDG_CONFIG_HOME`, which `VIMINIT` takes precedence over for Neovim,
    with the same file. `VISUAL` as well as `EDITOR`, since a
    presenter's own `VISUAL` would otherwise win (S1); and `VIMINIT` is
    `if has('nvim') | source … | endif`, since Vim reads `VIMINIT` too
    and cannot source the Lua config;
  - the dev shell's `_hook_nvim` already writes the config and exports
    `XDG_CONFIG_HOME`; it gains `EDITOR=nvim` only if unset.
  - `grehack2026/smoke-test.sh` reads Neovim and buf from the image's
    PATH and the config from `VIMINIT`, not from the protolens wrapper.
- **S9. Docs:** the F1 help line for `v`, the man page, the README's
  editor sentence and the CHANGELOG say: opens `VISUAL`/`EDITOR`,
  Neovim reused across presses.
- **S10. Release.** Implemented on `release-0.3`, a branch from the
  v0.3.3 tag, and released as v0.3.4 (protolens's change only; the
  other tools republish unchanged under the shared version, spec 0405),
  then merged into
  `main`. The nixpkgs protolens PR moves to 0.3.4 with S7's recipe. The
  version bump, the tag and the recipe's source hash belong to the
  release; until then the staged recipe loses its options and wrapper
  and keeps 0.3.3.

## Alternatives considered

### Keep the bundled config, opt-in

A `PROTOLENS_NVIM_CONFIG` the user may set. It keeps a demo's taste in
the package for nobody but the demo, which already has its own copy
(`nix/demo-nvim/init.lua`).

### Add to the user's config instead of replacing it

Drop `-u` but prepend a protolens runtime directory whose `ftplugin`
starts buf only when no language server is attached. It would serve both
kinds of user, at the price of guessing at the user's setup, and it
still ships editor configuration (G2).

### `NVIM_APPNAME=protolens`

Neovim's own way to give a tool a separate profile. Clean, but it is
still protolens choosing a Neovim configuration for the user.

## Test plan

1. Unit tests of S1–S2: each variable order, an empty `VISUAL`, a
   command with arguments, basenames (`/usr/bin/nvim`, `vim`, `vi` as
   *other*, `nano`), and the arguments built for each kind.
2. The handoff tests that exist (`neovim.rs`, `key_dispatch.rs`), with
   the editor program overridden as today, still pass; one more runs a
   stub "other" editor and checks protolens waits for it and redraws.
3. The spawn-failure test checks S6's message names the variable.
4. `nix-build -A ci`; the workshop image's smoke test (S8) opens a
   googleapis `.proto` and sees `buf lsp serve` attach, as today.
5. By hand, in the dev shell and the image: `v` opens Neovim with the
   demo colors and `gd` works; with `EDITOR=vim` it opens Vim at the
   line and column; with `EDITOR=nano` at the line.
6. The staged nixpkgs recipe builds, with no editor in its closure.

## Measured outcome

Implemented 2026-10-14 on `release-0.3`, from v0.3.3.

- **S1–S4:** `neovim.rs` chooses the editor, its kind and its command,
  pure functions over the variables' values; the Neovim handoff is the
  former one without `-u` and with the editor's own program for the
  `--server` calls; other editors run to their end with SIGINT and
  SIGQUIT ignored once the editor has started. A missing editor run
  through `sh` is `sh`'s status 127, reported as missing.
- **Tests (plan 1–3):** 11 new or rewritten: the choice (each variable,
  an empty `VISUAL`, the default with and without `nvim` on PATH), the
  kind (`vi` as *other*), the command built directly or through `sh`,
  the PATH lookup, and through `open_editor`: a missing editor, a
  missing Neovim, a missing editor behind `sh`, another launch failure,
  an editor that exits cleanly and one that fails. protolens: 1,331 and
  29 tests pass.
- **S7:** our `protolens` output is the plain binary; `protolensLean`,
  `wrapProtolens` and the bundled `init.lua` are gone, as are the staged
  recipe's options, `neovim` and `buf` inputs and wrapper, and their
  check in `default.nix`.
- **S8:** `EDITOR` and `VIMINIT` in both demos' `demoEnv`. Checked with
  the demos' `neovimLean` in an empty environment: with `VIMINIT`, the
  demo config loads (desert, line numbers); without it, neither.
- **Plan 4:** `nix-build -A ci` passes; the workshop image's smoke test
  passes every check, among them the editor path as the default user and
  as uid 4242: Neovim from the image's PATH, the demo config through
  `VIMINIT`, `buf lsp serve` attached.
- **S9:** the F1 help line, the README and the CHANGELOG. The man page
  is generated from the command line, which does not mention the editor.
- **Plan 5:** tried by hand in the dev shell with a release build,
  2026-10-15.
- **Not done here:** S10's release (version, tag, the recipe's hashes,
  the nixpkgs PR, the merge into `main`).
