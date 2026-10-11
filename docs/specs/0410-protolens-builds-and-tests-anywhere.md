<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0410 — protolens builds and tests anywhere

Status: draft
App: protolens
Refs: docs/specs/0405-prototools-in-nixpkgs-one-small-pr-at-a-time.md
      (S9 step 4: protolens's nixpkgs PR); docs/specs/0408-the-textproto-grammar-ships-its-parser.md
      (S5: build.rs compiles the committed parser, a prerequisite);
      docs/specs/0145-protolens-neovim-lsp-config.md (the editor key:
      Neovim with buf's language server); docs/specs/0146-reproto-emit-buf-yaml.md

## Background

Three things stand between protolens and a plain nixpkgs recipe, beyond
the grammar (spec 0408):

1. **A test depends on the environment.** With an empty environment, as
   in the nixpkgs build sandbox,
   `tui::tests::heat_cue::a_negative_best_mismatch_is_amber` fails
   (checked 2026-10-11 with `env -i`): it assumes a true-colour terminal,
   which `supports_rgb()` (`theme.rs`) detects from `COLORTERM` and the
   terminal type. `nix/rust.nix` sets `COLORTERM=truecolor` for the tests
   to compensate; a nixpkgs recipe would have to as well.
2. **The editor key needs Neovim and buf at run time.** protolens opens
   the descriptor in Neovim, with buf's language server for navigation
   (`gd`, `gr`, `K`; specs 0145, 0146). Our wrapper (`nix/rust.nix`,
   `wrapProtolens`) puts both on `PATH` with `makeWrapper --prefix PATH`
   and sets `PROTOLENS_NVIM_CONFIG` to the shipped `init.lua`: the
   standard nixpkgs pattern, but both are large, and the editor key is an
   optional feature of a viewer. Their closures (nixos-26.05, measured
   2026-10-11): Neovim 0.12.4, 255 MiB; buf 1.73.0, 141 MiB.

   Without buf, things go badly today: `init.lua` starts `buf lsp serve`
   unconditionally, so Neovim reports an error each time it opens a
   `.proto`, and the navigation keys silently do nothing.
3. **Its completion and man page** already follow prototext's and
   protoscan's model (spec 0406's bash completion, `PROTOLENS_GEN_MAN`),
   so nothing is needed there.

## Goals

- **G1.** protolens's tests pass in an empty environment, with no
  variable set by the recipe.
- **G2.** Its nixpkgs recipe has prototext's shape plus one
  `wrapProgram` line, whose dependencies a reviewer can accept.
- **G3.** The editor key works with whatever is installed: fully with
  Neovim and buf, as a viewer with Neovim alone, and with a clear message
  naming what to install when something is missing.

## Non-goals

- **N1.** Changing how protolens detects colour support for users
  (`supports_rgb()` and its Vim-compatible order stay).
- **N2.** Supporting editors other than Neovim.
- **N3.** The grammar's build (spec 0408).

## Specification

- **S1. Tests set the colour mode they check.** A test that depends on
  true colour sets it explicitly, through the theme's existing seam or a
  test-only override of `supports_rgb()`, instead of reading the
  environment. Every test passes under `env -i`, and the
  `COLORTERM=truecolor` line in `nix/rust.nix` goes.
- **S2. The editor key degrades cleanly.**
  - **No Neovim:** today a missing `nvim` already shows a status-line
    message, `cannot launch nvim: <os error>` (`tui/neovim.rs`), which
    says what failed but not what to do. When the spawn fails with "not
    found", the message names the remedy instead: `nvim not found on
    PATH: install Neovim to use the editor key`.
  - **Neovim without buf:** `init.lua` starts the language server only if
    `vim.fn.executable("buf") == 1`, and otherwise shows one notice, `buf
    not found: go-to-definition unavailable`. protolens checks for `buf`
    before launching, still opens the file, and says on its status line
    that navigation needs buf. Viewing works; only navigation is missing.
- **S3. In nixpkgs, both tools are opt-in, separately.** The recipe takes
  two arguments, both off by default, named after what they add to the
  closure, as nixpkgs names such options (`withSystemd`, `withPCRE2`):
  - `withNeovim ? false` adds Neovim to the wrapper's `PATH`;
  - `withBuf ? false` adds buf, for go-to-definition and references in
    the editor (a comment next to it says so).

  By default the recipe wraps neither, and protolens finds `nvim` and
  `buf` on the user's `PATH`, with S2's messages when they are missing.
  It always sets `PROTOLENS_NVIM_CONFIG` to the shipped `init.lua`.
  - **Why not wrap both by default:** the editor key is optional, and
    the two would add about 400 MiB to every protolens install. A user
    who wants the key almost always has Neovim already; a second copy on
    the wrapper's `PATH` would only shadow it.
  - **Why two arguments, not one:** the tools are independent and users
    are in every combination; Neovim without buf is the common one. One
    `withEditor` would make such a user pull in a second Neovim to get
    buf.
  - Our own `protolens`, and `protolensLean` in the workshop image, keep
    wrapping both (the trimmed `neovimLean` and `bufLean` in the image),
    since the image must work out of the box with nothing installed.
- **S4. The recipe** is staged as `nixpkgs/pkgs/by-name/pr/protolens/package.nix`
  and built by the `nixpkgs-staging` workflow, like the others.

## Alternatives considered

### Keep `COLORTERM` in the recipe

One line, but a reviewer will ask why a test needs a terminal variable,
and the honest answer is that the test reads the user's environment.

### Wrap both in nixpkgs by default

Out of the box for everyone, but every protolens install then pulls in
about 400 MiB for one optional key. S3's arguments give it to those who
want it.

### Wrap buf only

What this spec first proposed, assuming buf was small. It is not (141
MiB), and Neovim without buf is the more common situation to serve.

### Name the buf argument after its purpose (`withProtoNavigation`)

Clearer about what it is for, but it hides that it means a 141 MiB
download, and nixpkgs names such arguments after the dependency.

## Test plan

1. `env -i PATH=... cargo test -p protolens` passes (S1).
2. With `nvim` absent from `PATH`, the editor key shows the message naming
   it. With Neovim but no `buf`, the file opens without a Neovim error,
   the notice appears once, and protolens's status line says navigation
   needs buf (S2).
3. The staged recipe builds with each combination of `withNeovim` and
   `withBuf`, and the wrapper's `PATH` holds exactly what was asked.
4. The staged protolens recipe builds in `nixpkgs-staging` on the three
   runners, and its binary's editor key works with Neovim on `PATH`.
5. `ci`, the image and its smoke test pass.

## Measured outcome

Filled in at implementation.
