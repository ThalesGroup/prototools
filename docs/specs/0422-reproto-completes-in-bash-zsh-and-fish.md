<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0422 — reproto completes in bash, zsh and fish

Status: implemented
Implemented in: 2026-10-24
App: reproto, Nix packaging
Refs: docs/specs/0406-bash-completion-from-the-raw-command-line.md (the
      bash behavior kept here, and the generated-script form the Rust
      tools' recipes use);
      docs/specs/0243-a-blob-is-a-root-and-the-defaults-fill-the-rest.md
      (S12, the blob members completed after `-I`);
      docs/specs/0411-reproto-and-its-grammar-complete-on-pypi-and-staged-for-nixpkgs.md
      (S6, the recipe this changes);
      NixOS/nixpkgs#572923 (the review comment that asked)

## Background

reproto installs a completion for bash only: a hand-written script,
`src/reproto/completions.sh`, which nixpkgs installs from the source
tree. prototext, protoscan and protolens install all three shells with
one generated form:

```nix
installShellCompletion --cmd prototext \
  --bash <(PROTOTEXT_COMPLETE=bash $out/bin/prototext) \
  --zsh <(PROTOTEXT_COMPLETE=zsh $out/bin/prototext) \
  --fish <(PROTOTEXT_COMPLETE=fish $out/bin/prototext)
```

reproto's CLI is Click, which generates scripts the same way
(`_REPROTO_COMPLETE=zsh_source reproto`), but its scripts understand
only three candidate types: `plain`, `dir` and `file`. reproto's own
completers answer with two more, which only `completions.sh` reads:

- `arg`: paths relative to the current directory, filtered by suffix
  (`--proto-variant`'s `.yaml`, the plugins' `.py`, `--output-root`'s
  directories);
- `arg_I`: the descriptor files, relative to the `-I` directories or
  members of an `-I` blob, which no shell can list by itself.

Measured with Click 8.3.1 and reproto from `main`, 2026-10-24: in zsh,
`reproto --pro` offers the options with their help; `reproto gre`
answers `arg_I grehack2026/`, which Click's zsh script drops, and fish's
does the same; neither falls back to file names (zsh adds none, fish
registers with `--no-files`). So the arguments reproto is mostly given
complete to nothing.

`completions.sh` is not package data either: a `pip install reproto`
has no copy, and its `bash_source` prints Click's generic script, with
the same gaps in bash.

## Goals

- **G1.** zsh and fish complete what bash does: options, their values,
  the `arg` and `arg_I` paths, a directory without a trailing space.
- **G2.** reproto prints its own script for each shell, so any install,
  pip's included, gets it, and the nixpkgs recipe installs the three
  shells in the generated form above.
- **G3.** bash behaves exactly as today.

## Non-goals

- **N1.** Changing Click. Its scripts are frozen on purpose (see the
  comment above `_SOURCE_ZSH` in `click/shell_completion.py`), and
  Click's documented extension point does what we need.
- **N2.** `reproto-instantiate-schema`'s completion. It has no custom
  completer, so Click's scripts serve it fully in all three shells; it
  is a test tool, and gets nothing more here.
- **N3.** Other shells (PowerShell, nushell, elvish).

## Specification

- **S1. Registration.** `reproto.cli` registers three
  `click.shell_completion.ShellComplete` subclasses with
  `add_completion_class`, under Click's own names `bash`, `zsh` and
  `fish`, so `_REPROTO_COMPLETE={bash,zsh,fish}_source` prints reproto's
  scripts and `…_complete` keeps Click's response formats. The registry
  is per process, and `reproto-instantiate-schema`'s entry point does
  not import `reproto.cli`, so it keeps Click's classes (N2).
- **S2. bash.** The class's `source()` returns `completions.sh`, read
  with `importlib.resources`; the script becomes package data in
  `pyproject.toml`. The script itself is unchanged but for its
  commented-out debug lines, which go. The dev shell's hook loads it as
  it loads the other tools' scripts, `source <(_REPROTO_COMPLETE=bash_source
  reproto)`, rather than reading the file: `reproto` there runs the
  working tree, so the shell gets what reproto prints, and the call
  costs 0.4 s (measured 2026-10-24), nothing beside entering the shell.
- **S3. zsh.** A script derived from Click's `_SOURCE_ZSH`, kept as
  package data beside `completions.sh`, which also handles:
  - `arg`: added with their last path component as display
    (`compadd -d`), so the listing reads as for any file;
  - `arg_I`: added as plain words (`compadd`), since they may be
    outside the current directory;
  - in both, a candidate ending in `/` is added with no suffix
    (`-S ''`), so Tab descends into it;
  - in `--option=value`, Click completes the value alone, and
    `compadd -U` replaces the whole word: every `compadd` gives the
    option back with `-p`, and `_path_files`, which matches instead,
    takes it as an ignored prefix (`compset -P '*='`). Click's own script
    loses the option.
- **S4. fish.** A script derived from Click's `_SOURCE_FISH`, also as
  package data, which echoes `arg` and `arg_I` values like `plain`
  ones; fish adds no space after a candidate ending in `/`. fish keeps
  only the candidates that start with the whole token, so in
  `--option=value` each candidate is prefixed with `--option=`; Click's
  own script completes nothing there.
- **S4a. Licensing.** Both scripts derive from Click's (BSD-3-Clause,
  Copyright 2014 Pallets): `REUSE.toml` records them as `MIT AND
  BSD-3-Clause`, and each says so in its comment, which the printed
  script keeps. No SPDX header: zsh reads `#compdef` on the first line.
- **S5. Packaging.** The nixpkgs recipe and `nix/python.nix` install the
  three shells in the generated form, under the `canExecute` guard that
  already covers the man pages:

  ```nix
  installShellCompletion --cmd reproto \
    --bash <(_REPROTO_COMPLETE=bash_source $out/bin/reproto) \
    --zsh <(_REPROTO_COMPLETE=zsh_source $out/bin/reproto) \
    --fish <(_REPROTO_COMPLETE=fish_source $out/bin/reproto)
  ```

  Neither refers to `src/reproto/completions.sh` any more, nor does the
  dev shell (S2).
- **S6. Docs.** The man page's completion section (`gen_man.py`) gives
  the three `source` lines instead of `completions.sh`'s path; the
  README, if it mentions completion, and the CHANGELOG likewise.
- **S7. Release.** Released as v0.3.5. NixOS/nixpkgs#572923 moves to
  0.3.5 with S5's recipe before it is merged, so that the review covers
  it once.

## Alternatives considered

### Click's zsh and fish scripts as they are

Works with 0.3.4 and no change to reproto, but the descriptor files and
every `arg` path complete to nothing (Background), and a later switch
would be a second recipe change needing a committer's approval.

### Map reproto's types to Click's for zsh and fish

The completers would answer `file` or `plain` outside bash. `file`
makes the shell complete relative to the current directory, which is
wrong for `-I`'s files and loses the blob members; `plain` makes zsh
add a space after a directory, so Tab cannot descend.

### Ship one static file per shell, installed by path

What the recipe does today for bash. It keeps the recipe tied to the
source layout, and pip users still have no script.

## Test plan

1. `test_completion_scripts.py`, each case in a fresh process:
   `bash_source` prints `completions.sh`, trailing newlines aside (spec
   0423 S2); `zsh_source` and
   `fish_source` print reproto's scripts, every placeholder filled;
   `reproto-instantiate-schema`'s are still Click's.
2. In the same file: after `-I <dir>`, `zsh_complete` and
   `fish_complete` answer the `arg_I` candidates in Click's formats.
3. By hand, in interactive zsh and fish, from the installed scripts:
   options with their help; `--proto-variant <Tab>` lists `.yaml` files
   and directories; `-I <dir> <Tab>` lists the descriptor files under
   it, and an `-I` blob's members; a directory is completed with `/` and
   no space. bash, with the `attic/run-reproto-completion-test` cases:
   as today.
4. `nix-build -A ci`, and the staged recipe in `nixpkgs-staging`: the
   three files land in `share/{bash-completion,zsh,fish}`.

## Measured outcome

Implemented 2026-10-24, for v0.3.5.

- **S1–S4a** as specified. The implementation added `--option=value`
  in zsh and fish (S3, S4), where Click's scripts lose the option or
  complete nothing. It also dropped each script's last newline, to make
  up for the one Click 8.3 prints after it; Click 8.4 prints none, and
  spec 0423 S1–S2 removed that, the scripts now being printed as is.
- **Plan 1, 2:** `test_completion_scripts.py`, 4 tests, pass.
- **Plan 3,** in zsh 5.9.1, fish 4.7.1 and bash 5.3, typed through tmux:
  the options with their help; `--proto-variant v` lists `variant.yaml`
  and `vdir/`, and `vdir/` gives `vdir/v.yaml`; `-I descs` lists `a.pb`
  and `sub/`, not `notes.txt`; `-I descs su` gives `sub/` with no
  space; `-I blob` lists the blob's `extra/second.pb` and `first.pb`;
  `--proto-variant=vd`, `--use-variant=desc` and `--desc-root=de` keep
  the option. Identical in the three shells, bash as before.
- **Plan 4:** `nix-build -A ci` and `nixpkgs-staging` pass; the reproto
  package holds `share/bash-completion/completions/reproto.bash`,
  `share/zsh/site-functions/_reproto` (first line `#compdef reproto`)
  and `share/fish/vendor_completions.d/reproto.fish`.
