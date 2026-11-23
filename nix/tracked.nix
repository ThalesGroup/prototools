# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

# Spec 0479: a local build is the build of its commit.
#
# A lib.fileset source takes what is on disk, tracked by git or not: files
# git ignores (the dev-shell's ruff.toml and rust-toolchain.toml, fixtures
# tests write, a stray Cargo.lock) would enter it, and a build from a
# checkout would not be the build of its commit. `toSource` here is
# lib.fileset.toSource with the fileset intersected with the files git
# tracks (staged ones count), when the repository is a git checkout; a
# source without .git (fetched, a tarball) holds tracked files only, and is
# taken as it is.
#
# gitTracked copies the tracked tree into the store before filtering, once
# per evaluation (about 22 MB); it works on shallow clones.
{ lib }:
let
  root = ../.;
  isCheckout = builtins.pathExists (root + "/.git");
  tracked = fileset:
    if isCheckout
    then lib.fileset.intersection fileset (lib.fileset.gitTracked root)
    else fileset;
in {
  inherit tracked;
  toSource = { root, fileset }: lib.fileset.toSource {
    inherit root;
    fileset = tracked fileset;
  };
}
