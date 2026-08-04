# SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
# SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

# default.nix — thin entry point.
#
# All build logic lives in nix/rust.nix, nix/python.nix, nix/shells.nix.
# This file:
#   1. Pins nixpkgs and crane.
#   2. Defines shared inputs (depsSrc, workspaceSrc, pythonBin, pyo3Rustflags, protoPatchPhase).
#   3. Imports the three sub-files and wires their outputs together.
#   4. Assembles the ci and full-tests targets.
#   5. Exposes all public attributes.

{ pkgs ? (import (fetchTarball {
    # nixos-25.11 @ 2026-03-30 (git rev 1073dad219cb244572b74da2b20c7fe39cb3fa9e)
    url    = "https://github.com/NixOS/nixpkgs/archive/1073dad219cb244572b74da2b20c7fe39cb3fa9e.tar.gz";
    sha256 = "0xgsq0cfjnl2axbzzw579jrjq9g8mhbgjgfippl3qx03im636p5l";
  }) {})
, pythonPkgs ? pkgs.python313Packages
# buf — narrow override, pinned separately from the main nixpkgs revision
# above: the main pin's buf is 1.59.0, which predates upstream fixes
# critical to protolens's Neovim integration (spec 0145/0146) —
# v1.60.0 changed `buf lsp serve`'s default --timeout from 2m0s to 0 (no
# timeout), and v1.61.0 fixed a regression in LSP well-known-types
# handling that reliably crashed `buf lsp serve` (SIGSEGV in
# buflsp.(*file).RefreshIR) when navigating to a locally-materialized WKT
# file such as google/protobuf/any.proto (as reproto emits under -O, spec
# 0146) — live-reproduced and root-caused 2026-07-18. Rest of the
# toolchain (rustc, protobuf, etc.) stays on the main pin.
, buf ? (import (fetchTarball {
    # nixpkgs-unstable @ 2026-07-18 (git rev 31cd72fdba8fa052e437ce7e6879c4fe62def10f)
    url    = "https://github.com/NixOS/nixpkgs/archive/31cd72fdba8fa052e437ce7e6879c4fe62def10f.tar.gz";
    sha256 = "107f6kp5kjxsh9aggnqfanlfn5mw24gq19alkdvld75vimv5r3jl";
  }) {}).buf
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
  # Source sets — one per granularity level, all using lib.fileset so that
  # target/ build artefacts are naturally excluded (fileset operates on files,
  # not directory nodes, so target/ is never in scope for mkCrateSrc).
  #
  # workspaceSrc still needs explicit target/ subtraction because
  # crane.fileset.commonCargoSources ./.  admits .rs/.toml files found inside
  # target/ (verified: 59 such files on a local build).
  # ---------------------------------------------------------------------------

  # depsSrc — manifest files only.
  # NOTE: not currently used for depsCache — Crane's cargoArtifacts fingerprint
  # matching requires depsCache to use the same src as consuming derivations
  # (workspaceSrc).  Kept for reference / future experimentation.
  depsSrc = pkgs.lib.fileset.toSource {
    root   = ./.;
    fileset = crane.fileset.cargoTomlAndLock ./.;
  };

  # fixtureFilter — admits only files the Rust tests actually need from fixture
  # directories: .pb, .proto, .yaml, .license, Cargo.lock.  Excludes .md, .py,
  # .pyc, .gitignore, __pycache__ and other non-Rust artefacts that would
  # otherwise pollute the hash.
  fixtureFilter = dir: pkgs.lib.fileset.fileFilter
    (f: f.hasExt "pb" || f.hasExt "proto" || f.hasExt "yaml" || f.hasExt "license")
    dir;

  # workspaceSrc — all workspace crate sources + filtered fixture dirs, minus
  # target/ artefact trees.  Used by rustFmt, rustClippy, rustTests.
  workspaceSrc = pkgs.lib.fileset.toSource {
    root   = ./.;
    fileset = pkgs.lib.fileset.difference
      (pkgs.lib.fileset.unions [
        (crane.fileset.commonCargoSources ./.)
        (fixtureFilter ./prototext/fixtures)
        (fixtureFilter ./reproto/src/reproto/tests/fixtures)
        (fixtureFilter ./prototext-graph/tests/fixtures)
        (fixtureFilter ./tests/fixtures)
        # prototext-core/fixtures is taken wholesale rather than through
        # fixtureFilter: `benches/codec.rs` include_bytes! the .txt protoc
        # rendering, an extension fixtureFilter deliberately drops.  The
        # directory holds nothing else — just descriptor.pb, that .txt, and
        # their two .license sidecars.  Omitting it broke `nix-build`
        # outright, since the spec-0163 test in
        # `prototext-core/src/serialize/render_text/mod.rs` include_bytes!
        # descriptor.pb and so cannot even compile without it.
        ./prototext-core/fixtures
        # grpconf/anomalies.pb is `#@` prototext text under a `.pb` name
        # (spec 0226), and `prototext-core/tests/anomaly_fixture.rs`
        # include_str! it, so the test cannot compile without it.
        (fixtureFilter ./grpconf)
        # prototext/wkt/prebuilt/*.rkyv — the git-committed WKT scoring
        # graph. `prototext/build.rs` copies it under `--features
        # prebuilt-wkt`, which nix/rust.nix's bootstrapArgs now passes to
        # every workspace-wide build (spec 0239 S2). Taken wholesale:
        # fixtureFilter admits only .pb/.proto/.yaml/.license, not .rkyv.
        ./prototext/wkt
        ./README.md
      ])
      (pkgs.lib.fileset.unions [
        (pkgs.lib.fileset.maybeMissing ./target)
        (pkgs.lib.fileset.maybeMissing ./prototext-graph/target)
      ]);
  };

  # NOTE: per-crate src isolation is not feasible with a single Cargo workspace
  # because Cargo validates all member source entry points (src/lib.rs etc.)
  # even for unused members.  Per-crate isolation would require splitting the
  # Cargo workspace.  See spec 0078 for details.

  # patchPhase shared by all Crane derivations that compile prototext.
  # Compiles the three .proto schemas into fixtures/prebuilt/ using protoc so
  # that build.rs can copy them into $OUT_DIR without needing protox.
  protoPatchPhase = ''
    runHook prePatch

    mkdir -p prototext/fixtures/prebuilt

    protoc \
      --descriptor_set_out=prototext/fixtures/prebuilt/descriptor.pb \
      --include_imports \
      ${pkgs.lib.concatStringsSep " \\\n      " wktSources}

    protoc \
      --descriptor_set_out=prototext/fixtures/prebuilt/knife.pb \
      --proto_path=prototext/fixtures/schemas \
      knife.proto

    protoc \
      --descriptor_set_out=prototext/fixtures/prebuilt/enum_collision.pb \
      --proto_path=prototext/fixtures/schemas \
      enum_collision.proto

    protoc \
      --descriptor_set_out=prototext/fixtures/prebuilt/message_set.pb \
      --proto_path=prototext/fixtures/schemas \
      message_set.proto

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
  # shellHook so that manual `cargo build -p prototext_codec_lib` aligns.
  pyo3Rustflags = "-L ${pythonBin}/lib -lpython${pythonPkgs.python.pythonVersion}";

  # ---------------------------------------------------------------------------
  # tree-sitter-textproto — plain C Python extension for the textproto grammar,
  # plus a static Rust-linkable lib and a highlight-query regression check.
  #
  # treeSitterTextprotoGenerated — codegen only (shared): runs `tree-sitter
  #   generate` once against our own committed, locally-modified grammar.js
  #   (docs/specs/0121-tree-sitter-textproto-field-no-vendoring.md) and our
  #   own committed highlights.scm. Consumed by both treeSitterTextproto
  #   (Python extension) and treeSitterTextprotoRustLib (Rust static lib) so
  #   codegen never runs twice.
  # treeSitterTextproto — Python C extension (unchanged behavior), now
  #   consuming the shared generated parser.c instead of re-running
  #   `tree-sitter generate` itself.
  # treeSitterTextprotoRustLib — static lib (.a) + queries/highlights.scm,
  #   consumed by protolens's build.rs (via nix/rust.nix's commonArgs.env).
  # treeSitterTextprotoHighlightTest — `tree-sitter generate && tree-sitter
  #   test` check against our committed grammar.js/highlights.scm/test file,
  #   wired into ci/ci-no-clippy.
  # ---------------------------------------------------------------------------

  treeSitterTextprotoGenerated = pkgs.stdenv.mkDerivation {
    name              = "tree-sitter-textproto-generated";
    src               = ./reproto/tree-sitter-textproto;
    nativeBuildInputs = [ pkgs.tree-sitter pkgs.nodejs ];
    buildPhase = ''
      tree-sitter generate
    '';
    installPhase = ''
      mkdir -p $out/src $out/queries
      cp src/parser.c $out/src/
      cp -r src/tree_sitter $out/src/tree_sitter
      cp highlights.scm $out/queries/highlights.scm
    '';
  };

  treeSitterTextproto = pkgs.stdenv.mkDerivation {
    name        = "tree-sitter-textproto";
    src         = ./reproto/tree-sitter-textproto;
    buildInputs = [ pythonBin ];
    buildPhase  = ''
      $CC -shared -fPIC \
        -o textproto$(python3-config --extension-suffix) \
        binding.c ${treeSitterTextprotoGenerated}/src/parser.c \
        -I ${treeSitterTextprotoGenerated}/src \
        $(python3-config --includes --ldflags) \
        ${pkgs.lib.optionalString pkgs.stdenv.isDarwin "-undefined dynamic_lookup"}
    '';
    installPhase = ''
      mkdir -p $out
      cp textproto*.so $out/
      cp ${./reproto/tree-sitter-textproto/textproto.pyi} $out/textproto.pyi
    '';
  };

  treeSitterTextprotoRustLib = pkgs.stdenv.mkDerivation {
    name       = "tree-sitter-textproto-rust-lib";
    dontUnpack = true;
    buildPhase = ''
      $CC -c -fPIC -I ${treeSitterTextprotoGenerated}/src \
        -o parser.o ${treeSitterTextprotoGenerated}/src/parser.c
      $AR rcs libtree-sitter-textproto.a parser.o
    '';
    installPhase = ''
      mkdir -p $out/lib $out/queries
      cp libtree-sitter-textproto.a $out/lib/
      cp ${treeSitterTextprotoGenerated}/queries/highlights.scm $out/queries/
    '';
  };

  # Standalone from treeSitterTextprotoGenerated — `tree-sitter test` reads
  # queries/highlights.scm and test/highlight/ relative to its own cwd, so
  # this assembles a minimal grammar directory (grammar.js + our committed
  # highlights.scm + test file + a tree-sitter.json) and runs `tree-sitter
  # generate && tree-sitter test` against it directly (no parser-directories
  # discovery config needed for `test`, unlike `tree-sitter highlight`).
  treeSitterTextprotoHighlightTest = pkgs.runCommand "tree-sitter-textproto-highlight-test" {
    nativeBuildInputs = [ pkgs.tree-sitter pkgs.nodejs pkgs.stdenv.cc ];
  } ''
    set -euo pipefail
    export HOME="$TMPDIR"
    mkdir -p work/queries work/test/highlight
    cd work
    cp ${./reproto/tree-sitter-textproto/grammar.js} grammar.js
    cp ${./reproto/tree-sitter-textproto/highlights.scm} queries/highlights.scm
    cp ${./reproto/tree-sitter-textproto/test/highlight/textproto.txt} test/highlight/textproto.txt
    cat > tree-sitter.json <<'JSON'
    {
      "grammars": [
        {
          "name": "textproto",
          "camelcase": "Textproto",
          "scope": "source.textproto",
          "file-types": ["textproto", "txt"],
          "highlights": "queries/highlights.scm"
        }
      ],
      "metadata": { "version": "0.0.0", "license": "ISC" }
    }
    JSON
    tree-sitter generate
    tree-sitter test
    touch $out
  '';

  # ---------------------------------------------------------------------------
  # WKT proto list — read from the committed SOURCES file at eval time so
  # default.nix never needs updating when the list changes.
  # ---------------------------------------------------------------------------
  wktSources =
    let
      raw  = builtins.readFile ./prototext/wkt/SOURCES;
      lines = pkgs.lib.splitString "\n" raw;
    in
      builtins.filter (l: l != "") lines;

  # ---------------------------------------------------------------------------
  # Sub-file imports
  #
  # wkt-db cycle break (single Rust import, no double-compile):
  #
  #   rust      — single Crane workspace; produces prototextBare + prototext.
  #               prototextBare is built unconditionally (no wktRkyv needed).
  #               prototext (full) receives wktRkyv; falls back to prototextBare
  #               when wktRkyv is null (never the case here).
  #   python    — reprotoBare depends only on the Python codec, not on any Rust
  #               binary.  reprotoTests/googleapisTests/customTests use rust.prototext (lazy).
  #   wktRkyv   — uses python.reprotoBare to run reproto --schema-db-out.
  #               Does NOT depend on rust.prototext, breaking the cycle.
  #
  # All shared Crane artefacts (depsCache, rustTests, etc.) come from the
  # single rust import — Rust sources are compiled exactly once.
  # ---------------------------------------------------------------------------

  rust = import ./nix/rust.nix {
    inherit pkgs crane pythonPkgs pythonBin pythonExecutable pyo3Rustflags
            depsSrc workspaceSrc protoPatchPhase wktRkyv treeSitterTextprotoRustLib buf;
  };

  python = import ./nix/python.nix {
    inherit pkgs pythonPkgs pythonBin treeSitterTextproto;
    # rust.prototext (full, lazy): only forced when reprotoTests/googleapisTests/customTests
    # are built, by which time wktRkyv is already available.
    prototext = rust.prototext;
    inherit (rust) prototextCodec fdpScanLib prototextGraphLib
                   prototextExtensionArtifacts prototextGraphExtensionArtifacts;
  };

  cratesIo = import ./nix/crates-io.nix {
    inherit pkgs crane workspaceSrc protoPatchPhase;
    inherit (rust) commonArgs;
  };

  pypi = import ./nix/pypi.nix {
    inherit pkgs pythonPkgs workspaceSrc;
    reprotoSrcFull = python.reprotoSrcFull;
    inherit (rust) prototextExtensionArtifacts
                   fdpScanExtensionArtifacts
                   prototextGraphExtensionArtifacts;
  };

  # Pre-build the WKT scoring graph using python.reprotoBare.
  # proto filenames are read from prototext/wkt/SOURCES at eval time.
  # python.reprotoBare does not depend on the Rust prototext binary, so there
  # is no cycle: wktRkyv → python.reprotoBare → (pure Python) ✓
  #
  # wktRkyvDeps, not reprotoTestDeps: the latter carries fdpScanLib, which
  # embeds this very graph (spec 0239 S1) and so depends on wktRkyv.
  wktRkyv = pkgs.runCommand "wkt-rkyv" {
    buildInputs = [
      pkgs.protobuf
      (pythonPkgs.python.withPackages (_: python.wktRkyvDeps))
    ];
  } ''
    set -euo pipefail
    mkdir -p "$out"
    export PYTHONPATH="${python.reprotoSrcFull}/src"

    # Compile WKT .proto files (from prototext/wkt/SOURCES) into one FDS.
    protoc \
      --descriptor_set_out="$out/wkt.desc" \
      --include_imports \
      ${pkgs.lib.concatStringsSep " \\\n      " wktSources}

    # Build the Hopcroft scoring graph from the WKT descriptor.
    # reproto -I takes a directory of .pb files; DESCRIPTOR_FILES are positional.
    # --schema-db-out writes schemas.desc and schemas/hopcroft.rkyv.
    # We copy hopcroft.rkyv to $out/wkt.rkyv for the build.rs fast-path.
    # --emit-extension-ranges is required, not optional: protoscan scores
    # descriptors under Policy::Scan against this graph, and that policy
    # asserts the graph carries range data (spec 0238 S9, spec 0239 S2).
    python -m reproto.cli \
      --schema-db-out="$out/schemas.desc" \
      --emit-extension-ranges \
      -I "$out" \
      wkt.desc
    cp "$out/schemas/hopcroft.rkyv" "$out/wkt.rkyv"
    cp "$out/schemas/index.rkyv"    "$out/wkt_index.rkyv"
  '';

  shells = import ./nix/shells.nix {
    inherit pkgs pythonPkgs pythonBin pythonExecutable pyo3Rustflags treeSitterTextproto
            treeSitterTextprotoRustLib buf;
    inherit (rust) prototext protolens;
    inherit (python) reprotoSrc reprotoBare reprotoTestDeps reproto protoscan;
    repoRoot    = toString ./.;
    rustcVersion = pkgs.rustc.unwrapped.version;
  };

  # ---------------------------------------------------------------------------
  # Convenience bundle: prototext + protolens + reproto + protoscan
  # ---------------------------------------------------------------------------
  prototools = pkgs.symlinkJoin {
    name   = "prototools";
    paths  = [ rust.prototext rust.protolens python.reproto python.protoscan ];
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
    rust.prototextBare rust.prototext rust.protolens
    rust.prototextCodec rust.fdpScanLib rust.prototextGraphLib
    python.reproto python.protoscan
    python.reprotoTests python.protoscanTests python.fdpScanTests python.prototextCodecTests
    python.pythonLint python.pythonRuff
    treeSitterTextprotoHighlightTest
  ];

  # ci-no-clippy — same as ci but without rustClippy.
  # Used on platforms where clippy is known to fail (e.g. macos-15-intel).
  ci-no-clippy = pkgs.linkFarmFromDrvs "ci-no-clippy" [
    rust.rustFmt rust.rustTests
    rust.prototextBare rust.prototext rust.protolens
    rust.prototextCodec rust.fdpScanLib rust.prototextGraphLib
    python.reproto python.protoscan
    python.reprotoTests python.protoscanTests python.fdpScanTests python.prototextCodecTests
    python.pythonLint python.pythonRuff
    treeSitterTextprotoHighlightTest
  ];

  full-tests = pkgs.linkFarmFromDrvs "full-tests" [
    ci python.googleapisDb python.googleapisTests python.customDb python.customTests
  ];

in
{
  default              = ci;
  prototools           = prototools;
  prototext            = rust.prototext;
  prototext-bare       = rust.prototextBare;
  protolens            = rust.protolens;
  rust-fmt             = rust.rustFmt;
  rust-clippy          = rust.rustClippy;
  rust-tests           = rust.rustTests;
  prototext-codec      = rust.prototextCodec;
  reproto              = python.reproto;
  reproto-bare         = python.reprotoBare;
  reproto-tests        = python.reprotoTests;
  protoscan-tests      = python.protoscanTests;
  fdp-scan-tests       = python.fdpScanTests;
  prototext-codec-tests = python.prototextCodecTests;
  python-lint          = python.pythonLint;
  python-ruff          = python.pythonRuff;
  ci                   = ci;
  ci-no-clippy         = ci-no-clippy;
  full-tests           = full-tests;
  googleapis-pbs       = python.googleapisPbs;
  googleapis-db        = python.googleapisDb;
  googleapis-tests     = python.googleapisTests;
  custom-db            = python.customDb;
  custom-tests         = python.customTests;
  user-shell           = shells.user-shell;
  dev-shell            = shells.dev-shell;
  protoscan            = python.protoscan;
  fdp-scan-lib         = rust.fdpScanLib;
  prototext-graph-lib  = rust.prototextGraphLib;
  crates-io            = cratesIo;
  pypi                 = pypi;
  tree-sitter-textproto-highlight-test = treeSitterTextprotoHighlightTest;
}
