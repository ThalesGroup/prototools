# Briefing: nixpkgs PR `git-seriate: init at 0.4.0`

Notes for the maintainer before marking the PR ready, so that reviewer
questions can be answered first-hand (nixpkgs automation/AI policy: the
contributor must understand the change and answer without relaying to an
AI tool).

## The reviewer's points on the earlier PRs, checked here

As in `pr-briefing-protoback.md`, from #572580, #572904, #572911, #572923
and #573094:

- **`changelog` from `finalAttrs.src.rev`.** Done.
- **Arguments grouped, `# passthru` above `nix-update-script`.** Done.
- **Completions for every shell:** bash, zsh and fish, for both programs.
- **Nothing that doesn't belong in nixpkgs:** no features, no flags.
- **Built on the reviewer's three platforms** by upstream CI, as staged.

## What the recipe does, line by line

- **`src`**: the `v0.4.0` tag of douzebis/prototools.
- **`cargoBuildFlags`/`cargoTestFlags = [ "-p" "git-seriate" "-p"
  "git-seriate-fdp" ]`**: two crates of the workspace, one program each.
  git-seriate is the generic tool; git-seriate-fdp is it with a plugin for
  protobuf descriptors, a separate binary because it links the descriptor
  scanner and renderer. They are released together, at one version, and
  documented together: one package, two programs.
- **`nativeCheckInputs = [ gitMinimal openssh protobuf ]`**: the
  end-to-end tests check every tree they write with `git fsck --strict`,
  sign commits with `ssh-keygen` and an `ssh-agent` of their own, and one
  compares with `protoc`. Without them, those tests skip. None is a runtime
  dependency: git-seriate writes git objects itself, and runs the user's
  `ssh-keygen` or agent only to sign, when the user's git is configured to.
- **`postInstall`**, behind `canExecute`:
  - completions: `GIT_SERIATE_COMPLETE=<shell> git-seriate` and
    `GIT_SERIATE_FDP_COMPLETE=<shell> git-seriate-fdp`, for bash, zsh and
    fish;
  - man pages: `GIT_SERIATE_GEN_MAN=<dir>` and `GIT_SERIATE_FDP_GEN_MAN=<dir>`
    write one per program and per subcommand, 16 in all (`git help
    seriate` finds `git-seriate.1`).
- **`versionCheckHook`**: `git-seriate --version` prints the version
  (`mainProgram`).
- **96 tests** in 16 test binaries run in the check phase, none skipped
  (x86_64-linux, upstream's nixpkgs pin, 2026-11-25).

## Questions a reviewer may ask

- **"Does it need `git` at run time?"** No: it reads and writes the
  repository through gitoxide (`gix`). Users have git anyway, since it
  runs as `git seriate`.
- **"Why one package for two binaries?"** Above: one release, one
  documentation, the second a specialization of the first. Two packages
  would be possible if the reviewer prefers.
- **"Why `openssh` in the checks?"** Commit signing is tested end to end
  with a throwaway key and agent; without `ssh-keygen` those tests skip.
- **"The repository contains binary files?"** The descriptor scanner's test
  corpus (`fdp-scan/tests/corpus/*.bin`), generated and checked by
  upstream's CI, read by git-seriate-fdp's tests.

## What was checked before opening

- Built from the local tree through nixpkgs' `callPackage`, with the
  vendored crates of the local `Cargo.lock`
  (`nix-build -A nixpkgs-staging-rust.git-seriate`): tests, completions,
  man pages and version check.
- To do at the release: in a nixpkgs checkout of `master`, `nix-update`
  for `hash` and `cargoHash`, then build the package alone. Not
  `nixpkgs-review` on the development host, which runs out of memory there.
