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
  # directories: .pb, .proto, .yaml, .script, .license, Cargo.lock.  Excludes
  # .md, .py, .pyc, .gitignore, __pycache__ and other non-Rust artefacts that
  # would otherwise pollute the hash.  .script is spec 0271's guided walk —
  # `protolens/tests/batch_script.rs` runs tests/fixtures/anomalies.script over
  # tests/fixtures/anomalies.pb, so the test cannot pass without it.
  fixtureFilter = dir: pkgs.lib.fileset.fileFilter
    (f: f.hasExt "pb" || f.hasExt "proto" || f.hasExt "yaml"
        || f.hasExt "script" || f.hasExt "license")
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
        # grpconf2026/ is deliberately absent, and must stay that way.  It is
        # the live demo: it *uses* the tools and has no business invalidating
        # their build.  It was in here for `anomalies.pb` and
        # `anomalies.script`, which are not demo artefacts at all but shared
        # test fixtures of prototext-core and protolens; they now live under
        # ./tests/fixtures, admitted above like every other fixture.
        #
        # What that cost while it lasted: grpconf2026/bob/ is gitignored
        # scratch, populated from the grpconf-demo derivation and then written
        # to by the demo itself (beat 6 lands the schema DBs, beat 11 the
        # export).  It carries the unpacked googleapis proto/ tree, which
        # fixtureFilter admits — 134 MB across 23 317 files of workspaceSrc's
        # 140 MB.  Every stage repopulation and every rehearsal rebuilt the
        # entire Rust world.
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
        # demo/bobapp is an excluded Cargo project (spec 0241 S1/S2), but
        # `[workspace] exclude` does not reach crane: commonCargoSources
        # admits any .rs/.toml it finds.  Without this subtraction every edit
        # to bobapp would change workspaceSrc's hash and rebuild the whole
        # Rust world for a demo that ci does not even compile from here.
        (pkgs.lib.fileset.maybeMissing ./demo/bobapp)
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
                   prototextExtensionArtifacts prototextGraphExtensionArtifacts
                   fdpScanExtensionArtifacts;
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
    # --schema-db-out writes wkt-db.desc and wkt-db/{hopcroft,index}.rkyv.
    # We copy hopcroft.rkyv to $out/wkt.rkyv for the build.rs fast-path.
    # --emit-extension-ranges is required, not optional: protoscan scores
    # descriptors under Policy::Scan against this graph, and that policy
    # asserts the graph carries range data (spec 0238 S9, spec 0239 S2).
    #
    # -O writes the decompiled .proto sources into the stub's `proto`
    # child (spec 0228 S2), which is the one path reproto allows inside
    # the reserved stub directory and exactly where protolens's
    # --proto-root falls back to (spec 0155 G2). Emitting it here rather
    # than in a second derivation keeps the sources, the .desc and the
    # .rkyv the Rust fast path is built against from ever drifting apart.
    #
    # The stub is `wkt-db`, not `wkt`: $out/wkt.desc is already the raw
    # protoc output above, and -I hands reproto that whole directory.
    #
    # --emit-descriptor: reproto suppresses google/protobuf/descriptor.proto
    # from -O by default (spec 0150 N1), but it is compiled into wkt.desc
    # like every other WKT and it is the one whose types you see first when
    # protolens opens a descriptor set — without it, `v` on any node of a
    # `--type google.protobuf.FileDescriptorProto` session reports "proto
    # source not found". googleapisDb and customDb pass it for the same
    # reason.
    python -m reproto.cli \
      --schema-db-out="$out/wkt-db.desc" \
      --emit-extension-ranges \
      --emit-descriptor \
      -I "$out" \
      -O "$out/wkt-db/proto" \
      wkt.desc
    cp "$out/wkt-db/hopcroft.rkyv" "$out/wkt.rkyv"
    cp "$out/wkt-db/index.rkyv"    "$out/wkt_index.rkyv"
  '';

  # ---------------------------------------------------------------------------
  # wktDb — the well-known types as the toolset's default descriptor set
  # (spec 0228). Carries wktRkyv's schema-DB output under its user-facing
  # names, plus the setup-hook that exports PROTOTEXT_DESCRIPTOR_SET.
  #
  # The layout is load-bearing, not decorative: every consumer derives its
  # sidecars from the descriptor path with the extension stripped, so this
  # one variable delivers scoring (hopcroft.rkyv), lazy type lookup
  # (index.rkyv) and protolens's jump-to-definition (proto/) at once.
  #
  # A derivation of its own, rather than the hook on wktRkyv: wktRkyv is a
  # build input of prototext (full), so its setup-hook would fire inside
  # that build — and inside the Python test derivations below it — which is
  # exactly the leak spec 0228 S8 exists to prevent.
  # ---------------------------------------------------------------------------
  wktDb = pkgs.runCommand "wkt-db" { } ''
    set -euo pipefail
    install -Dm444 ${wktRkyv}/wkt-db.desc "$out/share/prototools/wkt.desc"
    cp -r ${wktRkyv}/wkt-db "$out/share/prototools/wkt"
    chmod -R u+w "$out/share/prototools/wkt"

    mkdir -p "$out/nix-support"
    echo "export PROTOTEXT_DESCRIPTOR_SET=$out/share/prototools/wkt.desc" \
      > "$out/nix-support/setup-hook"
  '';

  shells = import ./nix/shells.nix {
    inherit pkgs pythonPkgs pythonBin pythonExecutable pyo3Rustflags treeSitterTextproto
            treeSitterTextprotoRustLib buf;
    inherit (rust) prototext protolens;
    inherit (python) reprotoSrc reprotoBare reprotoTestDeps reproto protoscan;
    inherit wktDb;
    # dev-shell only, for PROTOTEXT_GOOGLEAPIS_SET. Entering the dev-shell
    # therefore forces googleapisDb — a fetch of the pinned corpus, a whole-
    # corpus protoc run and a reproto --schema-db-out. It is `full-tests`
    # material, not `ci` material, and it is deliberately kept out of
    # user-shell for that reason.
    inherit (python) googleapisDb bobapp2Desc;
    inherit grpconfDemo;
    repoRoot    = toString ./.;
    rustcVersion = pkgs.rustc.unwrapped.version;
  };

  # ---------------------------------------------------------------------------
  # bobapp — the demo binary (separate Cargo workspace, spec 0241 S1).
  # Built from demo/bobapp/default.nix; not wired into ci or full-tests.
  #
  # Embeds descriptors for google.maps.places.v1, google.maps.routing.v2,
  # and the bobapp/v1/log envelope — nothing else.  See nix/python.nix and
  # grpconf2026/synopsis.md for context.
  #
  # bobapp1 / bobapp2 (the old two-binary split) are kept so that existing
  # nix-build -A targets and ci references do not break; they will be removed
  # once the narrative is updated.
  # ---------------------------------------------------------------------------
  bobapp = import ./demo/bobapp/default.nix {
    inherit pkgs crane;
    variant    = "bobapp";
    bobappDesc = python.bobappDesc;
    traceDesc  = python.bobapp2Desc;
  };

  bobapp1 = import ./demo/bobapp/default.nix {
    inherit pkgs crane;
    variant    = "bobapp1";
    bobappDesc = python.bobapp1Desc;
    traceDesc  = python.bobapp2Desc;
  };

  bobapp2 = import ./demo/bobapp/default.nix {
    inherit pkgs crane;
    variant    = "bobapp2";
    bobappDesc = python.bobapp2Desc;
    traceDesc  = python.bobapp2Desc;
  };

  # ---------------------------------------------------------------------------
  # grpconf-demo — read-only stage for the gRPConf 2026 live demo.
  #
  # Contains everything the presenter needs except the files the beats build
  # live on stage (app.desc, src/).  Those go into a writable working
  # directory (grpconf2026/bob/); see _hook_demo in nix/shells.nix.
  #
  #   $out/bin/bobapp          the demo binary (places v1 + routes v2)
  #   $out/shark               one captured request body (84 bytes)
  #   $out/log                 the log with four anomalies (20 243 bytes)
  #   $out/googleapis.desc     full corpus: 7 771 files, 58 777 types
  #   $out/googleapis/         sidecars: hopcroft.rkyv, index.rkyv, proto/
  #   $out/beats/              every grpconf2026/beats/*.script
  #
  # Build once:     nix-build -A grpconf-demo
  # Populate stage: dev-shell's _hook_demo copies this into grpconf2026/bob/.
  # ---------------------------------------------------------------------------
  grpconfDemo = pkgs.runCommand "grpconf-demo" { } ''
    set -euo pipefail
    mkdir -p "$out/bin" "$out/beats"

    # The demo binary.
    cp ${bobapp}/bin/bobapp "$out/bin/bobapp"

    # Committed fixtures: the pre-minted request capture and log.
    cp ${./grpconf2026/fixtures/bobshark} "$out/shark"
    cp ${./grpconf2026/fixtures/boblog}   "$out/log"

    # The googleapis schema DB.  The descriptor and its sidecars must sit
    # beside each other under the same stem so that protolens finds
    # hopcroft.rkyv and index.rkyv without a warning.
    cp ${python.googleapisDb}/googleapis.desc "$out/googleapis.desc"
    cp -r ${python.googleapisDb}/googleapis   "$out/googleapis"

    # Beat scripts.  Read-only in the stage is fine — they are never written.
    # Copied as a directory so that renaming or adding a beat needs no edit
    # here — the set has already turned over once (log-partial/log-full
    # became log-v1/log-v2) and left this derivation pointing at nothing.
    cp ${./grpconf2026/beats}/*.script "$out/beats/"
  '';

  # ---------------------------------------------------------------------------
  # Convenience bundle: prototext + protolens + reproto + protoscan
  #
  # wktDb contributes no binary — it is here for its setup-hook, which
  # exports PROTOTEXT_DESCRIPTOR_SET (spec 0228 S4). It is the only path
  # with one, so the join has no conflict to resolve.
  # ---------------------------------------------------------------------------
  prototools = pkgs.symlinkJoin {
    name   = "prototools";
    paths  = [ rust.prototext rust.protolens python.reproto python.protoscan wktDb ];
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
    wktDb
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
    wktDb
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
  bobapp-desc          = python.bobappDesc;
  bobapp1-desc         = python.bobapp1Desc;
  bobapp2-desc         = python.bobapp2Desc;
  bobapp               = bobapp;
  bobapp1              = bobapp1;
  bobapp2              = bobapp2;
  grpconf-demo         = grpconfDemo;
  user-shell           = shells.user-shell;
  dev-shell            = shells.dev-shell;
  wkt-db               = wktDb;
  protoscan            = python.protoscan;
  fdp-scan-lib         = rust.fdpScanLib;
  prototext-graph-lib  = rust.prototextGraphLib;
  crates-io            = cratesIo;
  pypi                 = pypi;
  tree-sitter-textproto-highlight-test = treeSitterTextprotoHighlightTest;
}
