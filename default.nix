# SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
# SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

# default.nix — thin entry point.
#
# All build logic lives in nix/rust.nix, nix/python.nix, nix/shells.nix.
# This file:
#   1. Pins nixpkgs and crane.
#   2. Defines shared inputs (src, pythonBin, pyo3Rustflags, protoPatchPhase).
#   3. Imports the three sub-files and wires their outputs together.
#   4. Assembles the ci and full-tests targets.
#   5. Exposes all public attributes.

{ pkgs ? (import (fetchTarball {
    # nixos-25.11 @ 2026-03-30 (git rev 1073dad219cb244572b74da2b20c7fe39cb3fa9e)
    url    = "https://github.com/NixOS/nixpkgs/archive/1073dad219cb244572b74da2b20c7fe39cb3fa9e.tar.gz";
    sha256 = "0xgsq0cfjnl2axbzzw579jrjq9g8mhbgjgfippl3qx03im636p5l";
  }) {})
, pythonPkgs ? pkgs.python312Packages
}:

let
  crane = pkgs.callPackage (pkgs.fetchgit {
    url    = "https://github.com/ipetkov/crane.git";
    rev    = "80ceeec0dc94ef967c371dcdc56adb280328f591";
    sha256 = "sha256-e1idZdpnnHWuosI3KsBgAgrhMR05T2oqskXCmNzGPq0=";
  }) { inherit pkgs; };

  # ---------------------------------------------------------------------------
  # Shared inputs — defined here because they are used by multiple sub-files.
  # ---------------------------------------------------------------------------

  # ---------------------------------------------------------------------------
  # Source filtered to only what Cargo needs, keeping the hash stable when
  # unrelated files (docs, fixtures, etc.) change.
  # ---------------------------------------------------------------------------
  src = pkgs.lib.cleanSourceWith {
    src    = pkgs.lib.cleanSource ./.;
    # Keep Cargo sources plus fixtures/ (integration tests + proto schemas).
    # Exclude Python-only subtrees that must not perturb the Rust derivation hashes.
    filter = path: type:
      let rel = pkgs.lib.removePrefix (toString ./. + "/") (toString path);
      in
      !(pkgs.lib.hasPrefix "reproto/" rel) &&
      !(pkgs.lib.hasPrefix "bin/" rel) &&
      !(pkgs.lib.hasPrefix "protoscan/" rel) &&
      ((crane.filterCargoSources path type) ||
       (pkgs.lib.hasInfix "/fixtures/" path));
  };

  # patchPhase shared by all Crane derivations that compile prototext.
  # Compiles the three .proto schemas into fixtures/prebuilt/ using protoc so
  # that build.rs can copy them into $OUT_DIR without needing protox.
  protoPatchPhase = ''
    runHook prePatch

    mkdir -p prototext/fixtures/prebuilt

    protoc \
      --descriptor_set_out=prototext/fixtures/prebuilt/descriptor.pb \
      google/protobuf/descriptor.proto

    protoc \
      --descriptor_set_out=prototext/fixtures/prebuilt/knife.pb \
      --proto_path=prototext/fixtures/schemas \
      knife.proto

    protoc \
      --descriptor_set_out=prototext/fixtures/prebuilt/enum_collision.pb \
      --proto_path=prototext/fixtures/schemas \
      enum_collision.proto

    runHook postPatch
  '';

  # ---------------------------------------------------------------------------
  # Python interpreter — defined early because pyo3Rustflags references it.
  # ---------------------------------------------------------------------------
  pythonBin        = pythonPkgs.python;
  pythonExecutable = "${pythonBin}/bin/python";

  # RUSTFLAGS for linking against CPython.  Set globally in commonArgs so that
  # all Crane derivations carry the same value — keeping Cargo fingerprints
  # consistent across the single shared depsCache.  Also exported in the
  # shellHook so that manual `cargo build -p prototext_codec` aligns.
  pyo3Rustflags = "-L ${pythonBin}/lib -lpython${pythonPkgs.python.pythonVersion}";

  # ---------------------------------------------------------------------------
  # tree-sitter-textproto — plain C Python extension for the textproto grammar
  # ---------------------------------------------------------------------------

  treeSitterTextprotoSrc = pkgs.fetchzip {
    url    = "https://github.com/PorterAtGoogle/tree-sitter-textproto/"
           + "archive/568471b80fd8793d37ed01865d8c2208a9fefd1b.tar.gz";
    sha256 = "056h95fn779p73ik1gyr4n0y0r5w9pk09z25ly6mm42v5jlzq22l";
    stripRoot = true;
  };

  treeSitterTextproto = pkgs.stdenv.mkDerivation {
    name              = "tree-sitter-textproto";
    src               = ./reproto/tree-sitter-textproto;
    buildInputs       = [ pythonBin ];
    nativeBuildInputs = [ pkgs.tree-sitter pkgs.nodejs ];
    buildPhase  = ''
      cp ${treeSitterTextprotoSrc}/grammar.js .
      tree-sitter generate
      gcc -shared -fPIC \
        -o textproto$(python3-config --extension-suffix) \
        binding.c src/parser.c \
        -I src \
        $(python3-config --includes --ldflags)
    '';
    installPhase = ''
      mkdir -p $out
      cp textproto*.so $out/
      cp ${./reproto/tree-sitter-textproto/textproto.pyi} $out/textproto.pyi
    '';
  };

  # ---------------------------------------------------------------------------
  # Sub-file imports
  # ---------------------------------------------------------------------------

  rust = import ./nix/rust.nix {
    inherit pkgs crane pythonPkgs pythonBin pythonExecutable pyo3Rustflags
            src protoPatchPhase;
  };

  python = import ./nix/python.nix {
    inherit pkgs pythonPkgs pythonBin treeSitterTextproto;
    inherit (rust) prototext prototextCodec fdpScanLib scoringGraphLib
                   prototextExtensionArtifacts scoringGraphExtensionArtifacts;
  };

  shells = import ./nix/shells.nix {
    inherit pkgs pythonPkgs pythonBin pythonExecutable pyo3Rustflags;
    inherit (rust) prototext;
    inherit (python) reprotoSrc reprotoBare reprotoTestDeps reproto protoscan;
    repoRoot    = toString ./.;
    rustcVersion = pkgs.rustc.unwrapped.version;
  };

  # ---------------------------------------------------------------------------
  # Convenience bundle: prototext + reproto + protoscan
  # ---------------------------------------------------------------------------
  prototools = pkgs.symlinkJoin {
    name   = "prototools";
    paths  = [ rust.prototext python.reproto python.protoscan ];
  };

  # ---------------------------------------------------------------------------
  # CI targets
  #
  # ci        — builds all packages and runs quick tests/linters.
  #             Use: nix-build -A ci  (also the default target).
  # full-tests — ci plus stress tests and slow integration tests.
  #             Use: nix-build -A full-tests
  # ---------------------------------------------------------------------------
  ci = pkgs.linkFarmFromDrvs "ci" [
    rust.rustFmt rust.rustClippy rust.rustTests
    rust.prototext rust.prototextCodec rust.fdpScanLib rust.scoringGraphLib
    python.reproto python.protoscan
    python.reprotoTests python.pythonLint python.pythonRuff
  ];

  full-tests = pkgs.linkFarmFromDrvs "full-tests" [
    ci python.stressDb python.stressTests
  ];

in
{
  default              = ci;
  prototools           = prototools;
  prototext            = rust.prototext;
  rust-fmt             = rust.rustFmt;
  rust-clippy          = rust.rustClippy;
  rust-tests           = rust.rustTests;
  prototext-codec      = rust.prototextCodec;
  reproto              = python.reproto;
  reproto-bare         = python.reprotoBare;
  reproto-tests        = python.reprotoTests;
  python-lint          = python.pythonLint;
  python-ruff          = python.pythonRuff;
  ci                   = ci;
  full-tests           = full-tests;
  stress-db            = python.stressDb;
  stress-tests         = python.stressTests;
  user-shell           = shells.user-shell;
  dev-shell            = shells.dev-shell;
  protoscan            = python.protoscan;
  fdp-scan-lib         = rust.fdpScanLib;
  scoring-graph-lib    = rust.scoringGraphLib;
}
