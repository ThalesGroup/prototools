# Briefing: nixpkgs PR `protoback: init at 0.4.0`

Notes for the maintainer before marking the PR ready, so that reviewer
questions can be answered first-hand (nixpkgs automation/AI policy: the
contributor must understand the change and answer without relaying to an
AI tool).

## The reviewer's points on the earlier PRs, checked here

The same reviewer approved prototext, protoscan, protolens and reproto
(#572580, #572904, #572911, #572923, #573094). What they asked for there,
and where this recipe stands:

- **`changelog` from `finalAttrs.src.rev`, not the tag:** "it behaves
  better when users override `src`" (#573094). Done.
- **Arguments grouped, `# passthru` above `nix-update-script`** (#572580).
  Done.
- **Completions for every shell the program supports,** not bash alone
  (#572923). bash, zsh and fish.
- **Nothing that doesn't belong in nixpkgs,** such as build options to
  combine (#572911). None: no features, no flags.
- **Built on x86_64-linux, aarch64-linux and aarch64-darwin:** the
  reviewer runs `nixpkgs-review` on all three. Upstream CI builds the
  staged recipe on the same three.

## What the recipe does, line by line

- **`src`**: the `v0.4.0` tag of douzebis/prototools, as for the other
  prototools packages.
- **`cargoBuildFlags`/`cargoTestFlags = [ "-p" "protoback" ]`**: one crate
  of the workspace. protoback embeds a few data files from the repository
  (the well-known types' descriptors and their variant, under `reproto/`),
  read from the same source tarball. It is not published on crates.io
  (`publish = false`), which nixpkgs doesn't need.
- **`nativeCheckInputs = [ protobuf python3 ]`**: some tests compile
  protoback's output with `protoc` and skip without it; the plugin tests
  run their test plugins as `python3 plugin.py`, and fail without it.
  Neither is a runtime dependency.
- **`postInstall`**, behind `canExecute`:
  - completions: `PROTOBACK_COMPLETE=<shell> protoback` prints the script,
    for bash, zsh and fish;
  - man pages: `PROTOBACK_GEN_MAN=<dir> protoback` writes `protoback.1` and
    `protoback-instantiate.1`.
- **`versionCheckHook`**: `protoback --version` prints the version.
- **74 tests** in 11 test binaries run in the check phase, none skipped
  (x86_64-linux, upstream's nixpkgs pin, 2026-11-25).

## Questions a reviewer may ask

- **"How does this relate to `reproto`?"** protoback is its Rust
  successor, compared against reproto case by case in upstream CI (1 655
  cases). reproto stays in nixpkgs for now; upstream plans to retire it,
  and nixpkgs' side of that is a later PR.
- **"Why `python3` for the tests?"** The plugin interface sends descriptors
  to an external program; its tests use small Python plugins. Upstream
  could make them skip without Python, as the `protoc` tests do.
- **"The repository contains binary files (`*.pb`)?"** Data: the
  well-known types' descriptor sets, which protoback embeds; upstream's CI
  regenerates them and fails if the committed copies differ.

## What was checked before opening

- Built from the local tree through nixpkgs' `callPackage`, with the
  vendored crates of the local `Cargo.lock`
  (`nix-build -A nixpkgs-staging-rust.protoback`): tests, completions,
  man pages and version check.
- To do at the release: in a nixpkgs checkout of `master`, `nix-update`
  for `hash` and `cargoHash`, then build the package alone. Not
  `nixpkgs-review` on the development host, which runs out of memory there.
