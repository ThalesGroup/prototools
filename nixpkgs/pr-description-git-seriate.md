git-seriate: init at 0.4.0

Add `git-seriate`, which stores snapshots of a set of files taken at
unknown or uncertain times (extracted from binaries, collected from
systems) as git branches, and orders them into one linear history whose
commits change as little as possible from one to the next. The package
also installs `git-seriate-fdp`, git-seriate specialized for protobuf
schemas. Its `extract` command scans executables and other files for the
`FileDescriptorProto` records they embed, and adds each file's as a
snapshot, so that a collection of binaries becomes a history of their
schemas; its `advise` command reports what that history can't merge into
one schema database. Both programs run as git subcommands
(`git seriate`, `git seriate-fdp`).
Changelog: https://github.com/douzebis/prototools/blob/v0.4.0/CHANGELOG.md

The recipe has the shape of the other prototools packages (`prototext`,
`protoscan`, `protolens`), and upstream builds and tests this exact recipe
(staged in the repository's `nixpkgs/` directory) in its CI on
x86_64-linux, aarch64-linux and aarch64-darwin.

## Things done

- Built on platform:
  - [x] x86_64-linux
  - [ ] aarch64-linux
  - [ ] aarch64-darwin
  - (aarch64-linux and aarch64-darwin: built and tested by upstream CI with
    this recipe.)
- Tested, as applicable:
  - [ ] [NixOS tests] in [nixos/tests].
  - [ ] [Package tests] at `passthru.tests`.
  - [ ] Tests in [lib/tests] or [pkgs/test] for functions and "core" functionality.
- [ ] Ran `nixpkgs-review` on this PR. See [nixpkgs-review usage].
- [x] Tested basic functionality of all binary files, usually in `./result/bin/`.
- Nixpkgs Release Notes
  - [ ] Package update: when the change is major or breaking.
- NixOS Release Notes
  - [ ] Module addition: when adding a new NixOS module.
  - [ ] Module update: when the change is significant.
- [x] Fits [CONTRIBUTING.md], [pkgs/README.md], [maintainers/README.md] and other READMEs.
- [x] Follows the [automation/AI policy].

## AI disclosure

This package was prepared with Claude Code (Claude Opus 5.5): the recipe,
the commit message and this description were drafted with it and reviewed
by me, the maintainer, who is responsible for them. The commit carries an
`Assisted-by:` trailer.

[NixOS tests]: https://nixos.org/manual/nixos/unstable/index.html#sec-nixos-tests
[Package tests]: https://github.com/NixOS/nixpkgs/blob/master/pkgs/README.md#package-tests
[nixpkgs-review usage]: https://github.com/Mic92/nixpkgs-review#usage

[CONTRIBUTING.md]: https://github.com/NixOS/nixpkgs/blob/master/CONTRIBUTING.md
[automation/AI policy]: https://github.com/NixOS/nixpkgs/blob/master/CONTRIBUTING.md#automationai-policy
[lib/tests]: https://github.com/NixOS/nixpkgs/blob/master/lib/tests
[maintainers/README.md]: https://github.com/NixOS/nixpkgs/blob/master/maintainers/README.md
[nixos/tests]: https://github.com/NixOS/nixpkgs/blob/master/nixos/tests
[pkgs/README.md]: https://github.com/NixOS/nixpkgs/blob/master/pkgs/README.md
[pkgs/test]: https://github.com/NixOS/nixpkgs/blob/master/pkgs/test

🤖 Generated with [Claude Code](https://claude.com/claude-code)
