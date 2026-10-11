<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0413 — every installed program answers `--version`

Status: implemented, except S6 (at the next release)
Implemented in: 2026-10-11
App: reproto, prototext
Refs: docs/specs/0402-a-nixpkgs-friendly-build.md (S8: the version check,
      on `mainProgram` only); docs/specs/0407-protoscan-in-rust.md (S4 and
      "A `protoscan-gen-man` binary": man pages from an environment
      variable, no helper in `bin/`); docs/specs/0405-prototools-in-nixpkgs-one-small-pr-at-a-time.md
      (S9: the nixpkgs PRs this changes)

## Background

Reported from the downstream internal package, while it moved to v0.3.2.
Each program each package installs, run with `--version` on the
Nix-built 0.3.2 with an empty environment:

| Program | Prints |
|---|---|
| `reproto` | `reproto, version 0.3.2` |
| `reproto-instantiate-schema` | `RuntimeError: 'reproto' is not installed. Try passing 'package_name' instead.` |
| `reproto-gen-man` | `wrote --version/reproto.1` |
| `reproto-instantiate-schema-gen-man` | `wrote --version/reproto-instantiate-schema.1` |
| `prototext` | `prototext 0.3.2` |
| `prototext-gen-man` | `wrote --version/prototext.1` |

- **`reproto-instantiate-schema`:** a bare `@click.version_option()`
  (`instantiate_cli.py`). click looks the version up under the top-level
  package's name, `reproto`, but the distribution is `prototext-reproto`.
- **The three `*-gen-man` programs** read no options. Their one argument
  is an output directory, so `--version` creates a directory of that name
  in the current one and writes a man page into it.
- **Why nothing caught them:** the install check runs `--version` on
  `mainProgram` only (spec 0402 S8).
- **Version lookups disagree:** `cli.py` falls back to `dev` on any
  exception, the man-page modules on `PackageNotFoundError` only. A
  lookup bug in `cli.py` would print `dev` rather than fail.

The `*-gen-man` programs exist only for the build, which runs them once to
write man pages, and for the website. They are the helper binaries spec
0407 declined for protoscan: users never run them, and a nixpkgs reviewer
may well ask why `bin/` ships them. protoscan and protolens write their
man pages when an environment variable names a directory
(`PROTOSCAN_GEN_MAN`, `PROTOLENS_GEN_MAN`).

## Goals

- **G1.** Every program a package installs prints its version with
  `--version`, and the build checks every one, not only `mainProgram`.
- **G2.** No program in `bin/` exists only for the build.
- **G3.** One version lookup per package, failing loudly on anything but
  "not installed" (running from source).

## Non-goals

- **N1.** Changing the man pages' content.
- **N2.** reproto's static bash completion, or how completions are
  generated (spec 0406).

## Specification

- **S1. reproto's version** comes from one function,
  `reproto.version()`: `importlib.metadata.version('prototext-reproto')`,
  or `dev` on `PackageNotFoundError` only. `cli.py`, `instantiate_cli.py`
  and the man-page code use it; each `click.version_option` passes it,
  with the program's name.
- **S2. reproto's man pages from an environment variable:**
  `REPROTO_GEN_MAN=<dir> reproto` writes `reproto.1` and
  `reproto-instantiate-schema.1` into `<dir>` and exits, before parsing
  arguments, as `PROTOSCAN_GEN_MAN` does. `reproto-gen-man` and
  `reproto-instantiate-schema-gen-man` leave `[project.scripts]`; their
  modules stay, as what the variable calls.
- **S3. prototext's man page from an environment variable:**
  `PROTOTEXT_GEN_MAN=<dir> prototext` writes `prototext.1` and exits. The
  `prototext-gen-man` binary target goes.
- **S4. Every caller follows:**
  - `nix/python.nix`, `nix/rust.nix`;
  - the dev-shell's man hook (`nix/shells.nix`);
  - the website (`.github/workflows/deploy.yml`);
  - the staged recipes for prototext and reproto.

  The man pages each tool documents list the new variable, as protoscan's
  does.
- **S5. The check covers every program:**
  - Our builds of reproto and prototext run `--version` on each program in
    `$out/bin` and require the version Nix assigns: a loop, in
    `postInstall` (prototext, beside the existing check) and
    `postInstallCheck` (reproto).
  - The staged recipes do the same with `versionCheckHook`, which checks
    one program, plus a `postInstallCheck` loop for reproto's second one.
  - A reproto pytest case runs `--version` on every entry in
    `[project.scripts]` from source and expects `dev` or the installed
    version, so the bug fails reproto's own tests too.
- **S6. nixpkgs:**
  - **prototext** is in nixpkgs at 0.3.0, and its recipe runs
    `prototext-gen-man`. The release carrying this spec breaks the
    automatic update to it, so that update is ours: `prototext: 0.3.0 ->
    X.Y.Z`, with the recipe's `postInstall` following S3.
  - **reproto** (NixOS/nixpkgs#572923, a draft) moves to the same
    release before it leaves draft.

## Alternatives considered

### Fix `instantiate_cli.py` alone

One line fixes the crash, the report's first suggestion. It leaves the
`*-gen-man` programs misreading `--version`, and nothing checking the
programs besides `mainProgram`.

### Keep the `*-gen-man` programs, with option parsing

They would answer `--version`, and the build could check them. But they
would still be programs in `bin/` that no user runs, which spec 0407
already chose against for protoscan, and the two tools would keep two
mechanisms for one job.

### Keep `prototext-gen-man` for the nixpkgs recipe's sake

Avoids touching a merged recipe. But the recipe would carry the helper
indefinitely, and the update has to be reviewed anyway; making it
ourselves lets the change land with its reason.

## Test plan

1. Every program in the built reproto and prototext prints the package's
   version with `--version`, under `env -i`. `reproto-instantiate-schema`
   prints `reproto-instantiate-schema, version X.Y.Z`.
2. `REPROTO_GEN_MAN=<dir> reproto` writes the two man pages and exits 0;
   `PROTOTEXT_GEN_MAN=<dir> prototext` writes `prototext.1`. Each is
   identical to what the helper wrote, apart from the documented
   variable.
3. The new pytest case fails on today's `instantiate_cli.py` and passes
   after S1.
4. Undoing S1 fails the build's check, naming the program.
5. `ci`, `nixpkgs-staging`, the image and its smoke test pass; the
   website builds its man pages.

## Measured outcome

Implemented 2026-10-11, up to the release that S6 waits for.

- **S1:** `reproto.version()` in `reproto/__init__.py`, used by
  `cli.py`, `instantiate_cli.py` and both man pages.
- **S2:** `reproto` is now `cli.run()`, which writes both man pages when
  `REPROTO_GEN_MAN` names a directory and otherwise runs the click
  command. `python -m reproto` runs it from a source tree, which is what
  the website uses. `reproto.1` documents the variable.
- **S3:** `prototext/src/gen_man.rs` became `man.rs`, a module of the
  `prototext` binary, called first in `main`.
  - **Test plan 2:** the page it writes equals v0.3.2's, apart from the
    version and the new `PROTOTEXT_GEN_MAN` entry.
- **S4:** the website no longer installs `lark`, which spec 0411 found
  reproto never imports.
- **S5:**
  - **Test plan 3:** `test_entry_points.py` reads `[project.scripts]`. It
    fails on the old decorator, with the reported `RuntimeError`, and
    passes after S1.
  - **Test plan 4:** run against the Nix-built reproto 0.3.2, the
    build's new loop fails and names `reproto-gen-man`.
- **Tests:**
  - `ci`: reproto's 286 tests, two of them new; prototext and reproto
    check `--version` on every program they install;
  - `nixpkgs-staging`: the eight recipes;
  - the image: 27 of 27 smoke checks. Inside it, `prototext`, `reproto`
    and `reproto-instantiate-schema` print the version, and no
    `*-gen-man` program exists;
  - `reuse lint`, `ruff` and `pyright` on the changed files.
- **Found on the way:** the reproduction itself created a `--version/`
  directory in the repository root, twice, by running the old
  `reproto-gen-man --version`: the bug, as described.
- **S6:** at the next release.
  - prototext's nixpkgs update is ours, since the merged recipe calls
    `prototext-gen-man`.
  - NixOS/nixpkgs#572923 moves to that release before leaving draft.
