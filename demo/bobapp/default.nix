# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

# demo/bobapp/default.nix — builds one bobapp binary.
#
# bobapp is a standalone Cargo workspace (excluded from the root workspace,
# spec 0241 S1) that depends on prototext-core and workspace-hack as path
# dependencies.  Its Cargo.lock is separate from the root workspace's.
#
# Everything that distinguishes a variant arrives through `variant` and
# `bobappDesc`; neither is in commonArgs, so variants share one dependency
# cache and one dummy build.
#
# The binaries are consumed by default.nix's grpconf-demo target; they are
# not wired into `ci` or `full-tests` because the live API key is a runtime
# concern only, not a build one.
#
# Inputs:
#   pkgs       — the same nixpkgs pin as the root default.nix
#   crane      — the same crane version as the root default.nix
#   variant    — names the derivation, the descriptor file inside bobappDesc,
#                and the installed binary (e.g. "bobapp")
#   bobappDesc — the python.bobappDesc derivation, holding
#                $out/${variant}.desc (BOBAPP_DESCRIPTOR_SET for build.rs).
#                Contains only Places FDPs — what protoscan finds in the binary.
#   extraDesc  — the python.bobappExtraDesc derivation, holding
#                $out/bobapp-extra.desc (BOBAPP_EXTRA_DESCRIPTOR_SET at runtime).
#                Contains Routes FDPs + log.proto; used for log encoding.
#
# Output: $out/bin/${variant}
#
# Crane technique: workspace-not-at-source-root.
#   The source tree is rooted at the repo root so that path dependencies
#   (prototext-core, workspace-hack) are reachable.  postUnpack then enters
#   demo/bobapp/ and sets sourceRoot=".": cargo runs with its Cargo.toml and
#   Cargo.lock at ".", resolving path deps via "../../prototext-core" etc.
#   No --manifest-path is needed; mkDummySrc sees the right Cargo.lock at ".".
#   Reference: crane docs/faq/workspace-not-at-source-root.md

{ pkgs, crane, variant, bobappDesc, extraDesc }:

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
  };

  depsCache = crane.buildDepsOnly commonArgs;

in crane.buildPackage (commonArgs // {
  pname          = variant;
  cargoArtifacts = depsCache;

  # Neither of these is in commonArgs: the dependency cache is shared between
  # the two variants and must not rehash when a descriptor set is rebuilt.
  env = {
    # build.rs reads BOBAPP_DESCRIPTOR_SET to embed the Places-only descriptor set.
    BOBAPP_DESCRIPTOR_SET       = "${bobappDesc}/${variant}.desc";
    # Runtime pool for log encoding (Routes + log.proto); not embedded.
    BOBAPP_EXTRA_DESCRIPTOR_SET = "${extraDesc}/bobapp-extra.desc";
  };

  # Cargo only knows how to build a binary called `bobapp`.
  # Skip the rename when variant already matches the crate binary name.
  postInstall = pkgs.lib.optionalString (variant != "bobapp") ''
    mv "$out/bin/bobapp" "$out/bin/${variant}"
  '';
})
