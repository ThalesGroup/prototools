<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Building prototools

How to build the tools, to use them or to develop them, and how to check
a change. Everything here goes through Nix (`nix-build`, `nix-shell`),
which pins every input.

## Two worlds

Every build belongs to one of two worlds, chosen by `default.nix`'s
`cargoProfile` argument:

| world | `cargoProfile` | what it is |
|---|---|---|
| **release** | `release`, the default | what ships: thin LTO, the fastest code, the slowest to build |
| **quick** | `quick` | no LTO across crates: several times faster to build, the code a little slower |

Each world has its own store paths, and its own tests: a world's tests
test that world's binaries. Where each entry point stands:

| entry point | world | what for |
|---|---|---|
| `nix-shell` (`shell.nix`) | release | using the tools |
| `nix-build -A prototools` | release | using the tools, installed elsewhere |
| `nix-shell dev-shell.nix` | quick | developing |
| `check`, `check --nix` | quick | checking a change |
| `check --release`, the GitHub CI | release | checking what ships, before a push |
| the demo shells (`cd grehack2026 && nix-shell`, the same in `grpconf2026`) | release | demos: some show the tools' performance, and what they show is what ships |

## Using the tools

```shell
nix-shell
```

The shell builds the tools it puts on your `PATH` (prototext, protolens,
protoback, protoscan, and the well-known types' schema database) at low
parallelism, `--max-jobs 1 --cores 2`, so that a modest machine stays
usable (see below). They are the same derivations as `nix-build`'s: what
one built, the other reuses. The first entry builds them, and says so,
with the build's log beside the root (`result-user-shell.log`); later
entries find them built.

The shell roots the tools in the checkout, as `result-user-shell`, so that
a garbage collection keeps them (nothing of ours is on a public binary
cache: a collected tool is rebuilt from source). `rm result-user-shell`
frees them; deleting the checkout frees them with it.

To build them without a shell:

```shell
nix-build -A prototools --max-jobs 1 --cores 2
```

## A modest machine

Depending on its configuration (NixOS's default, for example), Nix can
run several builds at once, each with a compiler per CPU, and the release
world's link-time optimization adds its memory to each: a laptop can run
out of memory. The bounds:

- **`nix-shell`** builds at `--max-jobs 1 --cores 2` already.
  `PROTOTOOLS_BUILD_FLAGS` replaces those flags, on a large machine for
  example: `PROTOTOOLS_BUILD_FLAGS="--max-jobs auto --cores 0" nix-shell`.
- **`nix-build`** cannot be bounded from the repository: Nix reads
  `--max-jobs` and `--cores` before it evaluates anything. It reminds you
  of the bounded command when it starts; `PROTOTOOLS_QUIET=1` hides that
  reminder. `max-jobs` and `cores` in `nix.conf` set the bounds once and
  for all.
- **The dev shell and `check`** use every core by default, the
  development cycle being meant to be fast. `PROTOTOOLS_MODEST=1` bounds
  cargo at two jobs in both, and Nix at `--max-jobs 1 --cores 2` in
  `check --nix` and `check --release`. It cannot bound the Nix builds
  that entering the dev shell makes, for the same reason as `nix-build`'s:
  for those, `nix-shell dev-shell.nix --max-jobs 1 --cores 2`.

## Developing

```shell
nix-shell dev-shell.nix
```

The dev shell is in the quick world throughout:

- **its inputs** (the Python extensions, the schema database, the tools)
  are the quick world's Nix builds, which `check --nix` builds too;
- **its local builds** go to `target/quick/`: its hook rebuilds
  prototext, protolens and protoscan there when their sources are newer,
  and `bin/`'s wrappers run them (`PROTOTOOLS_CARGO_PROFILE=release` makes
  a wrapper run `target/release/` instead);
- **the Python extensions** (`prototext-pyo3`, `fdp-scan-pyo3`,
  `prototext-graph-pyo3`) are deployed from the working tree by
  `scripts/deploy-extensions`, into `target/python/`, which the shell puts
  first on `PYTHONPATH`; an extension not deployed falls through to Nix's
  build. `check` deploys them when a Rust source is newer.

## Checking a change

In the dev shell, `check` calls `scripts/check`, which uses the dev
shell's tools and refuses to run outside it. Each mode runs the cheap steps first and
stops at the first failure; its log goes to
`/tmp/oss-prototools-check-<timestamp>.log`, its path printed first, to
follow with `tail -f`.

| command | when | what it runs |
|---|---|---|
| `check --preflight` | before any long build | `cargo fmt --check`, ruff, nixfmt, REUSE, the evaluation, the protobuf-release check: seconds, no build |
| `check` | after an edit | the preflight, then clippy, the Rust tests, protoback's suite again via text, the extensions deployed if stale, pyright, pytest: in the working tree, at `--profile quick`, incremental |
| `check --nix` | before a push, optimistic | the preflight, then the quick world's CI, `nix-build -A ci-quick`, on the tracked files (it names any untracked file the sandbox will not see) |
| `check --release` | before a push | the preflight, then the release world's CI, the steps GitHub runs, in their order |

Rules:

- **`check` runs `ci`'s checks with `ci`'s options:** a check added to
  `ci` is added to `scripts/check` in the same change.
- **A step added to the GitHub workflow** (`.github/workflows/nix.yml`)
  is added to `scripts/check`'s `nix_steps` too: `ci` fails otherwise
  (`ci-workflow-check` compares them).
- **`check --release --staging`** also builds the staged nixpkgs
  recipes: run it when they, or what they build from, change.

## Importing `default.nix`

A downstream build taking the tools from this repository passes
`boundHint = false`, so that its evaluations do not print this
repository's reminder, which names this repository's attribute and
README:

```nix
import <this repository> { inherit pkgs; boundHint = false; }
```

`protobufRelease` gives the protobuf release every build here takes: its
`protoc` and its Python package set.
