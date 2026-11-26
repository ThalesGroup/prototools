<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0489 — low parallelism by default, for modest machines

Status: implemented
Implemented in: 2026-11-26
App: nix
Refs: docs/specs/0485-a-cargo-profile-argument-and-bounded-pytest.md
        (the three kinds of builder; README "Building with Nix", which
        gives the bounded command but leaves it to be found);
      prototools' assessment, 2026-11-26 (the root, the announcement, the
        system's Nix first, the setup-hooks' trap: S1)

## Background

A colleague who builds prototools to use them, on a laptop, gets an
out-of-memory kill or a machine that stops answering. Nothing in the
repository bounds the build; the bounds are Nix's and cargo's defaults:

- **Nix:** NixOS's default `nix.conf` has `max-jobs = auto` (one build
  per CPU) and `cores = 0` (each build uses every CPU). Measured on the
  development VM (NixOS, Nix 2.34.8, 12 CPUs, 9 GB): `max-jobs = 12`,
  `cores = 0`, so up to 12 builds, each running up to 12 compilers.
- **Inside a build:** crane sets `CARGO_BUILD_JOBS` from
  `NIX_BUILD_CORES` (spec 0485), so a workspace build runs as many rustc
  as `--cores` allows, each with thin LTO's memory in the `release`
  profile.
- **`nix-shell`** (`shell.nix`, the user shell): the tools it puts on
  PATH are Nix builds, the same workspace build as `nix-build`'s, bounded
  only by `nix.conf`.

The README's "Building with Nix" gives the bounded command (`nix-build -A
prototools --max-jobs 1 --cores 2`), but a colleague must find it before
the first build, and `nix-shell` takes no such flag to be told about.

What the repository cannot do: `--max-jobs` and `--cores` are read by Nix
before it evaluates any expression, and an expression cannot read them
back; so a build cannot be bounded from `default.nix`. Bounding it from
inside each derivation (a `CARGO_BUILD_JOBS` set in it) changes the
derivation: the bounded tools would be other store paths than
`nix-build`'s, built a second time, which is ruled out (the maintainer,
2026-11-26).

## Goals

- **G1. `nix-shell` low by default:** entering the user shell
  (`nix-shell`, `shell.nix`) on a modest machine does not exhaust it,
  with no flag to know.
- **G2. `nix-build` says how:** at its start, an explicit message gives
  the command line that bounds the build.
- **G3. One set of store paths:** the shell's tools are `nix-build`'s,
  derivation for derivation: built by either, reused by the other.
- **G4. Nothing else changes:** `nix-build`, the CI, `scripts/ci` and
  the dev shell build as now, at the parallelism they are given.

## Non-goals

- **N1. Detecting the machine** (its memory, its CPUs) to pick a bound:
  Nix evaluation is meant to be machine-independent; a bound read from
  `/proc` would make one expression build differently on two machines.
- **N2. Changing `nix.conf`** for the colleague: a system setting, theirs.
- **N3. The profile:** `release` stays the default (spec 0485); the bound
  is on parallelism, not on what is built.
- **N4. The dev shell** (`dev-shell.nix`): unchanged, its hook's cargo
  builds included (the maintainer, 2026-11-26). S2's message may reach
  it: it is for development.

## Specification

- **S1. The user shell builds its tools itself, bounded:** its tools
  (prototext, protolens, protoback, protoscan, and the well-known-types
  DB) leave its `buildInputs`. `user-shell-tools` (`default.nix`) holds
  them alone, a `symlinkJoin` of the derivations `nix-build` builds
  (`prototools` has git-seriate too, which the shell never had).
  - **One evaluation:** the shell's evaluation gives the hook the tools'
    derivation and output paths as text, their context discarded
    (`unsafeDiscardStringContext`), so that `nix-shell` itself builds
    nothing; the hook realises the derivation, `nix-store --realise <drv>
    --max-jobs 1 --cores 2`, which evaluates nothing. (A hook running
    `nix-build` evaluated everything a second time, in its own process:
    2.5 to 4 s more per entry.)
  - **Rooted:** `--add-root "$NIXSHELL_REPO/result-user-shell"`, the
    root `nix-build -o result-user-shell` would make, at the checkout's
    root, so that a garbage collection keeps the tools (else a laptop
    rebuilds them after each, at two compilers; nothing of ours is on a
    public cache). In plain view: the checkout shows what holds the tools,
    `rm result-user-shell` frees them, deleting the checkout frees them
    with it. Not `result`, which a `nix-build` in the checkout would
    overwrite. Ignored by git (`/result-*`), so out of every Nix source.
    (`nix-shell` never rooted its inputs: before this spec, a collection
    removed the shell's tools too.)
  - **Said when it builds:** when the output path is missing, one line
    says that it builds, bounded, and where its log is
    (`result-user-shell.log`, beside the root, the build's output teed to
    it), so that a
    long first entry under direnv or an IDE reads as building, not hung.
  - **Then:** the result's `bin` on `PATH`, its man pages on `MANPATH`,
    its bash completions sourced, `PROTOTEXT_DESCRIPTOR_SET` exported, at
    the DB's own path as its setup-hook gave it. **The pattern's trap:**
    the hook must export whatever the setup-hooks of what left
    `buildInputs` exported (a Python package's `PYTHONPATH`, say); here,
    the DB's variable alone.
  - **Override:** `PROTOTOOLS_BUILD_FLAGS` replaces the two flags (for a
    large machine: `--max-jobs auto --cores 0`).
  - **Nix itself:** the system's `nix-store`, looked for on `PATH`, then
    where `nix-shell --pure` leaves it off `PATH`
    (`/run/current-system/sw/bin`, `~/.nix-profile/bin`,
    `/nix/var/nix/profiles/default/bin`); the pinned one
    (`${pkgs.nix}`) last, a dependency of the shell, fetched once (35
    paths, 10.9 MiB).
  - **Failure:** a build that fails, or gives no directory, is said with
    its log and the command to retry, and the shell opens without the
    tools.
- **S2. The message, on `nix-build`:** `default.nix` prints, once per
  evaluation, through `builtins.trace`:

  ```
  prototools: on a machine with little memory, bound the build:
    nix-build -A prototools --max-jobs 1 --cores 2
  (README, "Building with Nix"; PROTOTOOLS_QUIET=1 hides this)
  ```

  unless `PROTOTOOLS_QUIET` is set (`builtins.getEnv`, which `nix-build`
  and `nix-shell` evaluate with). It comes from one shared expression the
  outputs depend on (`builtins.seq`, a `mapAttrs` over `default.nix`'s
  final set), the user shell's attribute and `protobufRelease` (data that
  downstream builds import) not: entering `nix-shell` prints nothing, its
  tools being bounded already (S1). `scripts/ci` and the workflow set
  `PROTOTOOLS_QUIET`.
- **S3. The README** says, at "Building with Nix", that `nix-shell`
  builds its tools at low parallelism, rooted, and how to raise it
  (`PROTOTOOLS_BUILD_FLAGS`).

## Open questions

- **Q1. The bound:** 2 compilers, the default until measured. prototools
  measures the peak memory of `user-shell-tools`' build at `--max-jobs
  1` with `--cores` 1, 2 and 4 (each in a fresh `--store`, inside a
  `systemd-run --user --scope -p MemoryAccounting=yes` cgroup, its
  `memory.peak`), and sends the figures.

## Alternatives considered

- **Bounding `--max-jobs` and `--cores` from the expression:** not
  possible; Nix reads them before evaluating (Background).
- **A wrapper script (`scripts/build`) passing the flags:** bounds a
  build only for who knows to run it, which is what the README already
  achieves; kept as the README's command.
- **Lowering parallelism for every build by default:** slows the CI and
  the release build, against G3; the colleague's case is the one that
  needs a bound, and `nix-shell` is its entry point.
- **A bound inside each derivation** (`CARGO_BUILD_JOBS`, from a
  `default.nix` argument the shell sets): other store paths than
  `nix-build`'s, so the tools built twice (G3).
- **A plain `builtins.trace` at `default.nix`'s top:** printed on
  entering `nix-shell` too, whose build is bounded already; S2's shared
  expression reaches the build outputs alone.

## Test plan

1. `nix-instantiate -A prototools`' and each tool's `.drv` are those
   before this spec (compared with a clean worktree of the previous
   commit); `user-shell-tools`' references are those five derivations;
   the user shell's references hold none of them.
2. Entering `nix-shell`, the tools missing: they are built at
   `--max-jobs 1 --cores 2`, said with the log's path; then on `PATH`,
   `PROTOTEXT_DESCRIPTOR_SET` at the DB's own path, `MANPATH` and the
   completions set; `result-user-shell` registered as a root (`nix-store
   --gc --print-roots`), and not shown by `git status`.
3. Entering it again, the tools built: nothing built or said; with
   `--pure`, the system's `nix-store` used, the tools there.
4. `PROTOTOOLS_BUILD_FLAGS` given: those flags passed; a flag
   `nix-store` refuses: the failure said, with the retry command, the
   shell opened without the tools.
5. `nix-instantiate -A prototools` prints S2's message once; with
   `PROTOTOOLS_QUIET=1`, nothing; entering `nix-shell`, nothing;
   `scripts/ci --preflight`, nothing.

## Measured outcome

On the development VM (NixOS, Nix 2.34.8, 12 CPUs, 9 GB), 2026-11-26:

- **Store paths unchanged** (test 1): `prototools` and the five tools'
  `.drv` identical to the previous commit's; `HEAD`'s shell, entered
  after the new one built the tools, found them in place.
- **First entry, tools missing:** 8 derivations built at `--max-jobs 1
  --cores 2`, 394.7 s, then the shell open with its tools.
- **Entry, tools built,** in clean checkouts, alternated six times:
  3.3 to 3.6 s before, 3.6 to 4.0 s after; the difference, the
  realisation of a present path (0.2 s) and the pinned Nix's evaluation
  (66 thousand of the shell's 0.89 million function calls,
  `NIX_SHOW_STATS`). A checkout with uncommitted changes evaluates its
  sources at about twice the cost, before and after alike.
- **Tests 2 to 5:** as stated; the bogus-flag failure and `--pure` (the
  system's `/run/current-system/sw/bin/nix-store` used) included.
- **Not measured:** the build's peak memory at each bound (Q1).
