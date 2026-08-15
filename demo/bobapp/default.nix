# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

# demo/bobapp/default.nix — builds the bobapp binary.
#
# bobapp is a standalone Cargo workspace (excluded from the root workspace,
# spec 0241 S1) that depends on prototext-core and workspace-hack as path
# dependencies.  Its Cargo.lock is separate from the root workspace's.
#
# The binary is consumed by default.nix's grpconf-demo target; it is not
# wired into `ci` or `full-tests` because the live API key is a runtime
# concern only, not a build one.
#
# Inputs:
#   pkgs       — the same nixpkgs pin as the root default.nix
#   crane      — the same crane version as the root default.nix
#   bobappDesc — the python.bobappDesc derivation: $out/bobapp.desc
#                (BOBAPP_DESCRIPTOR_SET for build.rs)
#
# Output: $out/bin/bobapp
#
# Crane technique: workspace-not-at-source-root.
#   The source tree is rooted at the repo root so that path dependencies
#   (prototext-core, workspace-hack) are reachable.  postUnpack then enters
#   demo/bobapp/ and sets sourceRoot=".": cargo runs with its Cargo.toml and
#   Cargo.lock at ".", resolving path deps via "../../prototext-core" etc.
#   No --manifest-path is needed; mkDummySrc sees the right Cargo.lock at ".".
#   Reference: crane docs/faq/workspace-not-at-source-root.md

{ pkgs, crane, bobappDesc }:

let
  repoRoot = ../..;

  src = pkgs.lib.fileset.toSource {
    root    = repoRoot;
    fileset = pkgs.lib.fileset.unions [
      # bobapp itself
      (crane.fileset.commonCargoSources (repoRoot + /demo/bobapp))
      # path dependencies
      (crane.fileset.commonCargoSources (repoRoot + /prototext-core))
      (crane.fileset.commonCargoSources (repoRoot + /workspace-hack))
    ];
  };

  # Vendor deps from bobapp's own Cargo.lock.
  cargoVendorDir = crane.vendorCargoDeps {
    inherit src;
    cargoLock = ./Cargo.lock;
    cargoToml  = ./Cargo.toml;
  };

  commonArgs = {
    inherit src cargoVendorDir;
    pname   = "bobapp";
    version = "0.1.0";

    # cargoLock and cargoToml tell crane (and mkDummySrc) which manifest and
    # lock file belong to this build.
    cargoLock = ./Cargo.lock;
    cargoToml  = ./Cargo.toml;

    # Enter the bobapp subdirectory before cargo runs.  With sourceRoot="."
    # cargo sees its Cargo.toml and Cargo.lock at the working-directory root,
    # and path deps like "../../prototext-core" resolve naturally.
    postUnpack = ''
      cd $sourceRoot/demo/bobapp
      sourceRoot="."
    '';

    strictDeps        = true;
    nativeBuildInputs = [ pkgs.cargo pkgs.rustc ];
    # build.rs reads BOBAPP_DESCRIPTOR_SET to embed the descriptor set.
    env.BOBAPP_DESCRIPTOR_SET = "${bobappDesc}/bobapp.desc";
  };

  depsCache = crane.buildDepsOnly commonArgs;

in crane.buildPackage (commonArgs // {
  cargoArtifacts = depsCache;
})
