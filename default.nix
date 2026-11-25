# SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
# SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

# default.nix — thin entry point.
#
# All build logic lives in nix/rust.nix, nix/python.nix, nix/shells.nix.
# This file:
#   1. Pins nixpkgs and crane.
#   2. Defines shared inputs (depsSrc, workspaceSrc, pythonBin).
#   3. Imports the three sub-files and wires their outputs together.
#   4. Assembles the ci and full-tests targets.
#   5. Exposes all public attributes.

{ # The main pin, nixos-26.05, in nix/nixpkgs.json (spec 0478): a file, so
  # that downstream can read it (its rev and sha256) with builtins alone.
  pkgs ? (let pin = builtins.fromJSON (builtins.readFile ./nix/nixpkgs.json); in
    import (fetchTarball {
      url    = "https://github.com/NixOS/nixpkgs/archive/${pin.rev}.tar.gz";
      inherit (pin) sha256;
    }) {})
, # The Python package set. Left out, `pkgs`' Python with spec 0460's
  # Python protobuf (protobufRelease.pythonPkgs, below); passed in, it
  # must have that protobuf (spec 0461 S3).
  pythonPkgs ? null
# buf from the main pin (spec 0402 S2): 26.05's is 1.73.0, past the two
# fixes protolens's Neovim integration needs (spec 0145/0146) — v1.60.0's
# `buf lsp serve` --timeout default of 0, and v1.61.0's fix for the
# SIGSEGV in buflsp.(*file).RefreshIR on a locally-materialized WKT file —
# which a separate nixpkgs-unstable pin supplied until then.
, buf ? pkgs.buf
# The commit an image is built from, for its org.opencontainers.image.revision
# label (spec 0374 S5). CI passes `--argstr gitRevision "$GITHUB_SHA"`; a local
# build leaves it out, and the image has no revision label.
, gitRevision ? null
# The Cargo profile the tools are built in (spec 0485): "release", what
# ships, or "quick" (no LTO, 16 codegen units), for development and
# low-memory builds: --argstr cargoProfile quick. The tests build in quick
# either way; releases, crates.io, PyPI and nixpkgs stay on release.
, cargoProfile ? "release"
# The nixpkgs the staged recipes are also checked against, as the nixpkgs
# PRs build (spec 0423 S4): the nixos-unstable pin of nix/unstable.json,
# moved by scripts/update-unstable-pin. The weekly check passes the
# channel's current tarball instead (S7).
, unstable ? (let pin = builtins.fromJSON (builtins.readFile ./nix/unstable.json); in
    fetchTarball {
      url = "https://github.com/NixOS/nixpkgs/archive/${pin.rev}.tar.gz";
      inherit (pin) sha256;
    })
}@args:

assert builtins.elem cargoProfile [ "release" "quick" ]
  || throw "cargoProfile: \"release\" or \"quick\", not \"${cargoProfile}\" (spec 0485 N2)";

let
  # Sources hold tracked files only, in a git checkout (spec 0479).
  trackedSource = (import ./nix/tracked.nix { inherit (pkgs) lib; }).toSource;

  # The one protobuf release (spec 0460): protoc, and the version every
  # copy of descriptor.proto is checked against. Exported, for downstream
  # builds (spec 0461 S1).
  protobufRelease = import ./nix/protobuf.nix { inherit pkgs unstablePkgs; };
  protoc = protobufRelease.protoc;

  # Spec 0461 S2, S3: the argument, if passed, with the release's Python
  # protobuf; else the release's own set.
  pythonPkgs =
    let
      given = args.pythonPkgs or null;
      want = "7.${protobufRelease.release}";
    in
    if given == null then protobufRelease.pythonPkgs
    else assert pkgs.lib.assertMsg (given.protobuf.version == want)
      "pythonPkgs has Python protobuf ${given.protobuf.version}; this build needs ${want} (spec 0460): pass (import <this repository> { inherit pkgs; }).protobufRelease.pythonPkgs, or leave pythonPkgs out";
      given;

  # Fetched at evaluation time, not built (spec 0402 S1): a `fetchgit`
  # derivation imported by `callPackage` is import-from-derivation, which
  # nixpkgs forbids and which stalls evaluation on a build. CI evaluates
  # `ci` with IFD disallowed so it cannot come back.
  crane = pkgs.callPackage (builtins.fetchTarball {
    # v0.21.3: from v0.21.0 on, no broken-string-indentation warning on
    # recent Nix. Not 0.22: it needs Nix 2.31.2 or later.
    url    = "https://github.com/ipetkov/crane/archive/2b3a5a88d852575758e1eb6ac9ee677fcd633fc1.tar.gz";
    sha256 = "1a27r58apm5arxyydn78j9yz0aa7d20v4ai688d23lbivfs0jaa5";
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
  depsSrc = trackedSource {
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

  # A Python package's sources, without the __pycache__ a working tree has.
  pythonPackageFilter = dir: pkgs.lib.fileset.fileFilter
    (f: f.hasExt "py" || f.hasExt "pyi")
    dir;

  # workspaceSrc — all workspace crate sources + filtered fixture dirs, minus
  # target/ artefact trees.  Used by rustFmt, rustClippy, rustTests.
  workspaceSrc = trackedSource {
    root   = ./.;
    fileset = pkgs.lib.fileset.difference
      (pkgs.lib.fileset.unions [
        (crane.fileset.commonCargoSources ./.)
        # cargo-hakari's settings, which rust.hakariCheck reads.
        ./.config/hakari.toml
        # The textproto grammar's committed parser and highlights query,
        # which its crate compiles and carries for protolens (specs 0408 S5,
        # 0414 S2).
        ./tree-sitter-textproto/src
        ./tree-sitter-textproto/highlights.scm
        # What the grammar's crate packages besides (spec 0414 S1): its
        # license notices, and the grammar the parser is generated from.
        ./tree-sitter-textproto/LICENSE
        ./tree-sitter-textproto/grammar.js
        # The extensions' Python packages, __init__.py and committed stub:
        # maturin packs them into the wheels, and workspaceBuild compares
        # the stub generator's output with the committed stubs (spec 0409).
        (pythonPackageFilter ./prototext-pyo3/prototext_codec_lib)
        (pythonPackageFilter ./fdp-scan-pyo3/fdp_scan_lib)
        (pythonPackageFilter ./prototext-graph-pyo3/prototext_graph_lib)
        # What drops the stub generator's ruff header before that
        # comparison (spec 0409 S2).
        ./scripts/strip-stub-header
        # protoback's built-in variant (spec 0428 S7) and profiles (spec
        # 0447 S6), which it embeds.
        ./protoback/data/variants/google-protobuf.yaml
        (pkgs.lib.fileset.fileFilter (f: f.hasExt "pb")
          ./protoback/data/variants/google-protobuf)
        (pkgs.lib.fileset.fileFilter (f: f.hasExt "yaml")
          ./protoback/data/profiles)
        # pyvis's page, which protoback embeds to write (as reproto did) its
        # --scoring-html-out byte for byte (spec 0430 S8).
        ./protoback/src/scoring/html
        # The adapter protoback runs Python plugins through (spec 0431 S3).
        ./protoback/src/plugin/adapter.py
        (fixtureFilter ./prototext/fixtures)
        (fixtureFilter ./prototext-graph/tests/fixtures)
        (fixtureFilter ./tests/fixtures)
        # fdp-scan's regression corpus (spec 0448 S8): the layouts and their
        # truth, which `fdp-scan/tests/corpus.rs` reads. Not the generator,
        # which only fdpScanCorpusCheck runs.
        (pkgs.lib.fileset.fileFilter (f: f.hasExt "bin" || f.hasExt "json")
          ./fdp-scan/tests/corpus)
        # seriation's corpora (spec 0451 S5, S6): the generator's parameters
        # and the expected orders, which `seriation/tests/corpora.rs` reads;
        # the corpora themselves it regenerates.
        (pkgs.lib.fileset.fileFilter (f: f.hasExt "args" || f.hasExt "txt")
          ./seriation/tests/corpora)
        # The existing implementation's histories of those corpora, which
        # `git-seriate/tests/settle.rs` compares its own with (spec 0452 S9).
        (pkgs.lib.fileset.fileFilter (f: f.hasExt "tsv")
          ./git-seriate/tests/histories)
        # The FDP renderer's text, which `git-seriate-fdp/tests/rendering.rs`
        # holds its version to (spec 0464 S5).
        (pkgs.lib.fileset.fileFilter (f: f.hasExt "txt")
          ./git-seriate-fdp/tests/rendering)
        # prototext-core/fixtures is taken wholesale rather than through
        # fixtureFilter: `benches/codec.rs` include_bytes! the .txtpb protoc
        # rendering, an extension fixtureFilter deliberately drops.  The
        # directory holds nothing else — just descriptor.pb, that .txtpb, and
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
        # graph, which `prototext/build.rs` always copies (specs 0239 S2,
        # 0405 S2). Taken wholesale: fixtureFilter admits only
        # .pb/.proto/.yaml/.license, not .rkyv.
        ./prototext/wkt
        # upb's vendored single-file build and the shim compiled with it,
        # which are C, not Cargo sources (spec 0415 S1, S2), and the
        # editions fixtures its tests load.
        ./prototext-upb/vendor
        ./prototext-upb/src/shim.c
        (fixtureFilter ./prototext-upb/tests/fixtures)
        # The text forms of two of those sets, as protoc --decode prints
        # them, which the text-format tests parse (spec 0424 S5).
        (pkgs.lib.fileset.fileFilter (f: f.hasExt "txtpb") ./prototext-upb/tests/fixtures)
        # Not README.md, which the crates' manifests name for their
        # crates.io packages: a README edit would rebuild everything.
        # cratesIoSrc adds it (spec 0485 S9).
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
        # grehack2026/game, the GreHack 2026 game of life, likewise (spec
        # 0375 S1): its own Cargo project, built by its own derivation.
        (pkgs.lib.fileset.maybeMissing ./grehack2026/game)
      ]);
  };

  # NOTE: per-crate src isolation is not feasible with a single Cargo workspace
  # because Cargo validates all member source entry points (src/lib.rs etc.)
  # even for unused members.  Per-crate isolation would require splitting the
  # Cargo workspace.  See spec 0078 for details.

  # prototext's four test descriptors, compiled with the pinned protoc. They
  # are committed under prototext/fixtures/prebuilt/ (spec 0405 S1), so no
  # build runs protoc for them; prototextFixturesCheck fails `ci` when the
  # committed copies no longer match what this produces (a nixpkgs bump
  # changes descriptor.pb as it changes the WKT graph).
  prototextFixtures = pkgs.runCommand "prototext-fixtures" {
    strictDeps = true;
    nativeBuildInputs = [ protoc ];
  } ''
    mkdir -p $out
    protoc \
      --descriptor_set_out=$out/descriptor.pb \
      --include_imports \
      ${pkgs.lib.concatStringsSep " \\\n      " wktSources}
    for name in knife enum_collision message_set; do
      protoc \
        --descriptor_set_out=$out/$name.pb \
        --proto_path=${./prototext/fixtures/schemas} \
        $name.proto
    done
  '';

  prototextFixturesCheck = pkgs.runCommand "prototext-fixtures-check" { } ''
    stale=0
    for f in descriptor knife enum_collision message_set; do
      cmp -s ${prototextFixtures}/$f.pb ${./prototext/fixtures/prebuilt}/$f.pb || stale=1
    done
    if [ "$stale" = 1 ]; then
      echo "prototext/fixtures/prebuilt/ is stale. Refresh it with:" >&2
      echo "  cp \$(nix-build -A prototext-fixtures)/*.pb prototext/fixtures/prebuilt/" >&2
      exit 1
    fi
    touch $out
  '';

  # fdp-scan's regression corpus regenerated with the pinned Python
  # protobuf (spec 0448 S8). The layouts are the well-known types' own
  # serialized descriptors, so they move with that version; committed as
  # generated so that the corpus's contributor and we score the same bytes.
  fdpScanCorpus = pkgs.runCommand "fdp-scan-corpus" {
    strictDeps = true;
    nativeBuildInputs = [ (pythonPkgs.python.withPackages (ps: [ ps.protobuf ])) ];
  } ''
    python3 -I ${./fdp-scan/tests/corpus/generate.py} public $out
  '';

  fdpScanCorpusCheck = pkgs.runCommand "fdp-scan-corpus-check" { } ''
    committed=${trackedSource {
      root = ./fdp-scan/tests/corpus;
      fileset = pkgs.lib.fileset.fileFilter (f: f.hasExt "bin" || f.hasExt "json")
        ./fdp-scan/tests/corpus;
    }}
    if ! diff -r ${fdpScanCorpus} "$committed" >/dev/null; then
      echo "fdp-scan/tests/corpus/ is stale. Refresh it with:" >&2
      echo "  cp \$(nix-build -A fdp-scan-corpus)/* fdp-scan/tests/corpus/" >&2
      exit 1
    fi
    touch $out
  '';

  # ---------------------------------------------------------------------------
  # Python interpreter.
  # ---------------------------------------------------------------------------
  pythonBin        = pythonPkgs.python;
  pythonExecutable = "${pythonBin}/bin/python";

  # No RUSTFLAGS for libpython (spec 0402 S6): each pyo3 crate's build.rs
  # links it into its own stub generator and tests only, so the Python
  # extensions themselves never link it. PYO3_PYTHON (nix/rust.nix) is all
  # those build scripts need to find it.

  # ---------------------------------------------------------------------------
  # protoback's well-known types (spec 0409 S4; reproto's until spec 0487).
  #
  # protoback embeds the WKTs' descriptors in its built-in variant
  # (protoback/data/variants/google-protobuf/), for the descriptor sets
  # that leave them implicit. They are committed, so protoback builds from
  # its source as it stands:
  #
  # wktVariantGenerated — compile-wkt.sh run with the pinned protobuf:
  #   its .proto files, compiled by its protoc. Input only to the check.
  # wktVariantCheck — fails `ci`, with the diff and the refresh command,
  #   when the committed .pb differ from what the pinned protobuf compiles.
  # ---------------------------------------------------------------------------

  wktVariantGenerated = pkgs.runCommand "wkt-variant-generated" {
    strictDeps = true;
    nativeBuildInputs = [ protoc ];
  } ''
    bash ${pkgs.lib.fileset.toSource {
      root = ./protoback/data;
      fileset = pkgs.lib.fileset.unions [
        ./protoback/data/compile-wkt.sh ./protoback/data/lib_proto_helpers.sh
      ];
    }}/compile-wkt.sh ${protoc}/include $out
  '';

  wktVariantCheck = pkgs.runCommand "wkt-variant-check" { } ''
    committed=${./protoback/data/variants/google-protobuf/google}
    if ! diff -r $committed ${wktVariantGenerated}/google >&2; then
      echo "protoback/data/variants/google-protobuf/google/ is stale (diff above). Refresh it with:" >&2
      echo "  cp -r \$(nix-build -A wkt-variant-generated)/google/. protoback/data/variants/google-protobuf/google/ && chmod -R u+w protoback/data/variants/google-protobuf/google" >&2
      exit 1
    fi
    touch $out
  '';

  # The vendored upb is of the release (spec 0460 S2): its VERSION's tag,
  # and its descriptor.proto byte for byte that release's.
  upbReleaseCheck = pkgs.runCommand "upb-release-check" { } ''
    version=${./prototext-upb/vendor/VERSION}
    tag=$(sed -n '1s/^protobuf v\([^ ]*\) .*/\1/p' "$version")
    if [ "$tag" != "${protobufRelease.release}" ]; then
      echo "prototext-upb/vendor is protobuf v$tag; the release is ${protobufRelease.release} (nix/protobuf.nix). Refresh it with:" >&2
      echo "  scripts/update-upb <google-protobuf crate version> v${protobufRelease.release}" >&2
      exit 1
    fi
    if ! cmp ${./prototext-upb/vendor/descriptor.proto} ${protoc}/include/google/protobuf/descriptor.proto >&2; then
      echo "prototext-upb/vendor/descriptor.proto is not protobuf ${protobufRelease.release}'s. Refresh it with:" >&2
      echo "  scripts/update-upb <google-protobuf crate version> v${protobufRelease.release}" >&2
      exit 1
    fi
    touch $out
  '';

  # The copies agree with each other (spec 0460 S4): the variant's
  # descriptor.pb is the vendored one without its source info.
  descriptorCopiesCheck = pkgs.runCommand "descriptor-copies-check" {
    nativeBuildInputs = [ (pythonPkgs.python.withPackages (ps: [ ps.protobuf ])) ];
  } ''
    python3 -I - ${./prototext-upb/vendor/descriptor.pb} \
      ${./protoback/data/variants/google-protobuf/google/protobuf/descriptor.pb} <<'EOF'
    import sys
    from google.protobuf import descriptor_pb2
    def file(path):
        s = descriptor_pb2.FileDescriptorSet()
        s.ParseFromString(open(path, "rb").read())
        (f,) = [f for f in s.file if f.name == "google/protobuf/descriptor.proto"]
        f.ClearField("source_code_info")
        return f.SerializeToString(deterministic=True)
    if file(sys.argv[1]) != file(sys.argv[2]):
        sys.exit("the variant's descriptor.pb is not prototext-upb/vendor/descriptor.pb "
                 "without its source info: refresh both from one release (spec 0460)")
    EOF
    touch $out
  '';

  # ---------------------------------------------------------------------------
  # tree-sitter-textproto — the textproto grammar (spec 0408).
  #
  # The generated parser (tree-sitter-textproto/src/) is committed,
  # so each consumer builds it with a C compiler alone: the Python
  # module through its setuptools package (nix/python.nix), protolens through
  # its build.rs (the cc crate). Nothing here feeds a build any more:
  #
  # treeSitterTextprotoGenerated — the pinned `tree-sitter generate` run on
  #   our vendored, locally modified grammar.js (spec 0121). Input only to
  #   the check below.
  # treeSitterTextprotoCheck — fails `ci`, with the diff and the refresh
  #   command, when the committed src/ differs from what the pinned
  #   generator writes. A build never refreshes the committed copy.
  # treeSitterTextprotoHighlightTest — `tree-sitter test` on the committed
  #   parser and highlights.scm (spec 0116).
  # ---------------------------------------------------------------------------

  treeSitterTextprotoGenerated = pkgs.runCommand "tree-sitter-textproto-generated" {
    strictDeps = true;
    nativeBuildInputs = [ pkgs.tree-sitter pkgs.nodejs ];
  } ''
    export HOME="$TMPDIR"
    cp ${./tree-sitter-textproto/grammar.js} grammar.js
    tree-sitter generate
    mkdir -p $out
    cp -r src $out/src
  '';

  treeSitterTextprotoCheck = pkgs.runCommand "tree-sitter-textproto-check" { } ''
    if ! diff -ru ${./tree-sitter-textproto/src} ${treeSitterTextprotoGenerated}/src >&2; then
      echo "tree-sitter-textproto/src/ is stale (diff above). Refresh it with:" >&2
      echo "  cp -r \$(nix-build -A tree-sitter-textproto-generated)/src/. tree-sitter-textproto/src/ && chmod -R u+w tree-sitter-textproto/src" >&2
      exit 1
    fi
    touch $out
  '';

  # `tree-sitter test` reads queries/highlights.scm and test/highlight/
  # relative to its own cwd, so this assembles a minimal grammar directory
  # (the committed parser, highlights.scm and test file, and a
  # tree-sitter.json) and runs `tree-sitter test` in it (no
  # parser-directories discovery config needed for `test`, unlike
  # `tree-sitter highlight`).
  treeSitterTextprotoHighlightTest = pkgs.runCommand "tree-sitter-textproto-highlight-test" {
    nativeBuildInputs = [ pkgs.tree-sitter pkgs.nodejs pkgs.stdenv.cc ];
  } ''
    set -euo pipefail
    export HOME="$TMPDIR"
    mkdir -p work/queries work/test/highlight
    cd work
    cp ${./tree-sitter-textproto/grammar.js} grammar.js
    cp ${./tree-sitter-textproto/highlights.scm} queries/highlights.scm
    cp ${./tree-sitter-textproto/test/highlight/textproto.txt} test/highlight/textproto.txt
    # The committed parser (spec 0408 S3): the one the tools ship.
    cp -r ${./tree-sitter-textproto/src} src
    chmod -R u+w src
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
  # rust    — a single Crane workspace: one release build (workspaceBuild)
  #           that every binary and extension is copied out of. All of it
  #           embeds the WKT graph committed under prototext/wkt/prebuilt/
  #           (spec 0401 S1, S2), so nothing waits for the graph below.
  # python  — the grammar's Python module, the full tests, on the rust
  #           extensions.
  # wktRkyv — the graph regenerated by protoback: a leaf, which only
  #           wktDb's schema DB and wkt-prebuilt-check read.
  #
  # All shared Crane artefacts (depsCache, rustTests, etc.) come from the
  # single rust import — Rust sources are compiled exactly once.
  # ---------------------------------------------------------------------------

  rust = import ./nix/rust.nix {
    inherit pkgs protoc crane pythonPkgs pythonBin pythonExecutable
            depsSrc workspaceSrc buf cargoProfile;
  };

  python = import ./nix/python.nix {
    inherit pkgs pythonPkgs pythonBin protoc;
    inherit (rust) metaCommon pyprojectVersion;
    prototext = rust.prototext;
    inherit (rust) prototextCodec fdpScanLib prototextGraphLib protoback;
  };

  # What `cargo package` needs besides the workspace: the README the
  # crates' manifests name (`readme = "../README.md"`), left out of
  # workspaceSrc so that editing it rebuilds no tool (spec 0485 S9).
  cratesIoSrc = trackedSource {
    root    = ./.;
    fileset = pkgs.lib.fileset.unions [
      (pkgs.lib.fileset.fromSource workspaceSrc)
      ./README.md
    ];
  };

  cratesIo = import ./nix/crates-io.nix {
    inherit pkgs crane protoc;
    workspaceSrc = cratesIoSrc;
    inherit (rust) commonArgs;
  };

  pypi = import ./nix/pypi.nix {
    inherit pkgs pythonPkgs;
    inherit (rust) workspaceBuild;
    inherit (python) treeSitterTextprotoPkg;
  };

  # The WKT scoring graph, generated by protoback from the code as it is
  # (spec 0487 S3; reproto generated it before, byte for byte the same).
  # proto filenames are read from prototext/wkt/SOURCES at eval time.
  #
  # No shipped binary embeds this one: they all embed the copy committed
  # under prototext/wkt/prebuilt/ (spec 0401 S2), and wkt-prebuilt-check
  # below fails `ci` when the two differ (S3). wktDb takes its schema DB
  # (wkt-db.desc, proto/) from here.
  wktRkyv = pkgs.runCommand "wkt-rkyv" {
    strictDeps = true;
    nativeBuildInputs = [
      protoc
      rust.protoback
    ];
  } ''
    set -euo pipefail
    mkdir -p "$out"

    # Compile WKT .proto files (from prototext/wkt/SOURCES) into one FDS.
    protoc \
      --descriptor_set_out="$out/wkt.desc" \
      --include_imports \
      ${pkgs.lib.concatStringsSep " \\\n      " wktSources}

    # Build the Hopcroft scoring graph from the WKT descriptor.
    # protoback -I takes a directory of .pb files; DESCRIPTOR_FILES are positional.
    # --schema-db-out writes wkt-db.desc and wkt-db/{hopcroft,index}.rkyv.
    # We copy hopcroft.rkyv to $out/wkt.rkyv for the build.rs fast-path.
    # --emit-extension-ranges is required, not optional: protoscan scores
    # descriptors under Policy::Scan against this graph, and that policy
    # asserts the graph carries range data (spec 0238 S9, spec 0239 S2).
    #
    # -O writes the decompiled .proto sources into the stub's `proto`
    # child (spec 0228 S2), which is the one path protoback allows inside
    # the reserved stub directory and exactly where protolens's
    # --proto-root falls back to (spec 0155 G2). Emitting it here rather
    # than in a second derivation keeps the sources, the .desc and the
    # .rkyv the Rust fast path is built against from ever drifting apart.
    #
    # The stub is `wkt-db`, not `wkt`: $out/wkt.desc is already the raw
    # protoc output above, and -I hands protoback that whole directory.
    #
    protoback \
      --schema-db-out="$out/wkt-db.desc" \
      --emit-extension-ranges \
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
  # The committed graph is what ships (spec 0401 S2); this keeps it honest
  # (S3). A stale copy fails `ci` like a failing test, and says how to fix
  # it. With nothing waiting for the tests, `nix-build -A wkt-rkyv` works
  # even across a GRAPH_VERSION bump, so the refresh is always this.
  wktPrebuiltCheck = pkgs.runCommand "wkt-prebuilt-check" { } ''
    stale=0
    for f in wkt.rkyv wkt_index.rkyv; do
      cmp -s ${wktRkyv}/$f ${./prototext/wkt/prebuilt}/$f || stale=1
    done
    if [ "$stale" = 1 ]; then
      echo "prototext/wkt/prebuilt/ is stale. Refresh it with:" >&2
      echo "  cp \$(nix-build -A wkt-rkyv)/{wkt,wkt_index}.rkyv prototext/wkt/prebuilt/" >&2
      exit 1
    fi
    touch $out
  '';

  wktDb = pkgs.runCommand "wkt-db" {
    meta = rust.metaCommon // {
      description = "Well-known protobuf types as a prototools schema database";
    };
  } ''
    set -euo pipefail
    install -Dm444 ${wktRkyv}/wkt-db.desc "$out/share/prototools/wkt.desc"
    cp -r ${wktRkyv}/wkt-db "$out/share/prototools/wkt"
    chmod -R u+w "$out/share/prototools/wkt"

    mkdir -p "$out/nix-support"
    echo "export PROTOTEXT_DESCRIPTOR_SET=$out/share/prototools/wkt.desc" \
      > "$out/nix-support/setup-hook"
  '';

  shells = import ./nix/shells.nix {
    inherit pkgs pythonPkgs pythonBin pythonExecutable buf protoc;
    inherit (rust) prototext protolens protoback;
    inherit (python) lintDeps lintSettings;
    inherit (rust) protoscan pyo3EnvironmentSignature;
    inherit wktDb;
    # The demos' inputs — the googleapis database, the grpconf stage, the
    # life binaries — are not passed: each demo has its own shell
    # (spec 0394, demoShells below).
    repoRoot    = toString ./.;
    rustcVersion = pkgs.rustc.unwrapped.version;
  };

  # ---------------------------------------------------------------------------
  # bobappDemo — the demo binary for grpconf2026 (separate Cargo workspace,
  # spec 0241 S1).  Built from demo/bobapp/default.nix; not wired into ci or
  # full-tests.
  #
  # variant = "bobapp" matches the descriptor file bobapp.desc and the crate
  # binary name, so postInstall's rename is skipped (see demo/bobapp/default.nix).
  # nix/grpconf2026-demo.nix stages it as the deck's bob/app (spec 0404).
  # ---------------------------------------------------------------------------
  bobappDemo = import ./demo/bobapp/default.nix {
    inherit pkgs crane;
    variant    = "bobapp";
    bobappDesc = python.bobappDesc;
    extraDesc  = python.bobappExtraDesc;
  };

  # ---------------------------------------------------------------------------
  # grpconf-demo — read-only stage for the gRPConf 2026 live demo.
  #
  # Contains the demo binary and pre-minted fixtures.  beats/ is committed
  # source and is used directly from the working tree; it is not included here.
  #
  #   $out/bin/bobapp          the demo binary (Places embedded; Routes via extra pool)
  #   $out/capture             one SearchText request body (spec 0350 G5)
  #   $out/logfile             the log with four anomalies (Routes entries first)
  #
  # googleapis is not included: $PROTOTEXT_GOOGLEAPIS_SET already provides it.
  #
  # Build once:     nix-build -A grpconf-demo
  # Staged into grpconf2026/bob/ via grpconfTalk.deck (spec 0404 S1, S2).
  # ---------------------------------------------------------------------------
  grpconfDemo = pkgs.runCommand "grpconf-demo" { } ''
    set -euo pipefail
    mkdir -p "$out/bin"

    # The demo binary.
    cp ${bobappDemo}/bin/bobapp "$out/bin/bobapp"

    # Committed fixtures: the pre-minted request capture and log.
    cp ${./grpconf2026/fixtures/bobshark} "$out/capture"
    cp ${./grpconf2026/fixtures/boblog}   "$out/logfile"
  '';

  # ---------------------------------------------------------------------------
  # Convenience bundle: prototext + protolens + protoback + protoscan, and
  # git-seriate and git-seriate-fdp
  #
  # wktDb contributes no binary — it is here for its setup-hook, which
  # exports PROTOTEXT_DESCRIPTOR_SET (spec 0228 S4). It is the only path
  # with one, so the join has no conflict to resolve.
  # ---------------------------------------------------------------------------
  prototools = pkgs.symlinkJoin {
    name   = "prototools";
    paths  = [ rust.prototext rust.protolens rust.protoback rust.protoscan
               rust.gitSeriate rust.gitSeriateFdp wktDb ];
  };

  # Fails when a store path whose name contains one of `deny` enters the
  # closure of `roots` (spec 0374 S2). Names, not paths, so a leak is caught
  # whatever its hash; the log lists every offender.
  mkClosureCheck = { name, roots, deny }:
    let info = pkgs.closureInfo { rootPaths = roots; };
    in pkgs.runCommand name { } ''
      found=0
      for pattern in ${pkgs.lib.escapeShellArgs deny}; do
        if grep -F -- "$pattern" ${info}/store-paths; then found=1; fi
      done
      if [ "$found" = 1 ]; then
        echo "denied store paths in the closure (patterns: ${pkgs.lib.concatStringsSep " " deny})" >&2
        exit 1
      fi
      touch $out
    '';

  # Spec 0374 S2: the regular bundle carries no build inputs. (wl-clipboard
  # and perl are allowed here: they are the desktop clipboard feature.)
  prototoolsClosureCheck = mkClosureCheck {
    name  = "prototools-closure-check";
    roots = [ prototools ];
    deny  = [ "-deps-deps" "winapi" "cargo-package" ];
  };

  # The GreHack 2026 game of life and its tap (spec 0375): a separate Cargo
  # project, like bobapp, and not in `ci`; the workshop image's workflow
  # builds it, unit tests included, on both architectures.
  grehackLife = import ./grehack2026/game/default.nix { inherit pkgs crane protoc; };

  # What the GreHack 2026 talk needs beyond the prototools, defined once for
  # the demo shell and the workshop image (spec 0403).
  grehackDemo = import ./nix/grehack2026-demo.nix {
    inherit pkgs protoc wktDb;
    telepromptSrc = ./bin/teleprompt;
    deckSrc       = ./grehack2026;
    inherit (rust) neovimLean bufLean;
  };

  # The same for the gRPConf 2026 talk, which the image also carries as a
  # stretch goal (spec 0404).
  grpconfTalk = import ./nix/grpconf2026-demo.nix {
    inherit pkgs protoc wktDb grpconfDemo;
    inherit (grehackDemo) teleprompt;
    inherit (rust) neovimLean bufLean;
    inherit (python) googleapisDb googleapisPbs;
    deckSrc = ./grpconf2026;
  };

  # The GreHack 2026 workshop image (spec 0374).
  grehack2026 = import ./nix/grehack2026.nix {
    inherit pkgs wktDb mkClosureCheck gitRevision grehackDemo grpconfTalk;
    life = grehackLife;
    inherit (rust) prototext protolens protoback;
    inherit (python) googleapisDb googleapisPbs;
    inherit (rust) protoscan;
  };

  # One shell per demo directory (spec 0394): grehack2026/shell.nix and
  # grpconf2026/shell.nix select these. Built from Nix only, never from
  # target/release/.
  demoShells = import ./nix/demo-shells.nix {
    inherit pkgs wktDb grehackDemo grpconfTalk;
    grehackRuntime = grehack2026.runtime;
    inherit (rust) prototext protolens protoback;
    inherit (rust) protoscan;
  };

  # ---------------------------------------------------------------------------
  # CI targets
  #
  # ci        — builds all packages and runs quick tests/linters.
  #             Use: nix-build -A ci  (also the default target).
  # full-tests — ci plus stress tests and slow integration tests.
  #             Use: nix-build -A full-tests
  # ---------------------------------------------------------------------------
  # ci's members; scripts/protobuf-release-check reads their build
  # closure (spec 0462 S3).
  ciMembers = [
    rust.rustFmt rust.rustClippy rust.rustTests rust.descriptorAccessCheck rust.hakariCheck
    rust.prototext rust.protolens
    rust.prototextCodec rust.fdpScanLib rust.prototextGraphLib
    rust.protoscan rust.gitSeriate rust.gitSeriateFdp
    python.fdpScanTests python.prototextCodecTests
    python.pythonLint python.pythonRuff
    treeSitterTextprotoHighlightTest treeSitterTextprotoCheck
    wktVariantCheck upbReleaseCheck descriptorCopiesCheck
    wktDb wktPrebuiltCheck prototextFixturesCheck fdpScanCorpusCheck
    prototoolsClosureCheck
    completionTests ciWorkflowCheck
    # nixpkgsStaging builds in its own workflow (nixpkgs-staging.yml): two
    # more full Rust builds would weigh on every CI run. The check stays.
    nixpkgsStagingCheck
  ];

  ci = pkgs.linkFarmFromDrvs "ci" ciMembers;

  # ci-no-clippy — same as ci but without rustClippy.
  # Used on platforms where clippy is known to fail (e.g. macos-15-intel).
  ci-no-clippy = pkgs.linkFarmFromDrvs "ci-no-clippy" [
    rust.rustFmt rust.rustTests rust.descriptorAccessCheck rust.hakariCheck
    rust.prototext rust.protolens
    rust.prototextCodec rust.fdpScanLib rust.prototextGraphLib
    rust.protoscan rust.gitSeriate rust.gitSeriateFdp
    python.fdpScanTests python.prototextCodecTests
    python.pythonLint python.pythonRuff
    treeSitterTextprotoHighlightTest treeSitterTextprotoCheck
    wktDb wktPrebuiltCheck prototextFixturesCheck fdpScanCorpusCheck
    prototoolsClosureCheck
    completionTests ciWorkflowCheck
    # nixpkgsStaging builds in its own workflow (nixpkgs-staging.yml): two
    # more full Rust builds would weigh on every CI run. The check stays.
    nixpkgsStagingCheck
  ];

  # ---------------------------------------------------------------------------
  # nixpkgs staging (spec 0405 S6, S7): the files under nixpkgs/ are what a
  # nixpkgs PR submits. nixpkgsStaging builds each staged recipe from the
  # local tree, as nixpkgs builds it from the tag: same callPackage, with src
  # and the vendored dependencies taken from here, so no hash needs updating
  # between releases.
  # ---------------------------------------------------------------------------
  # stagingSrc: the workspace, plus what the Python recipes build from
  # (the grammar's binding) and the extensions' test suites.
  stagingSrc =
    let
      fs = pkgs.lib.fileset;
      noPyc = dir: fs.difference dir (fs.fileFilter (f: f.hasExt "pyc") dir);
    in trackedSource {
      root    = ./.;
      fileset = fs.unions [
        (fs.fromSource workspaceSrc)
        (noPyc ./tree-sitter-textproto)
        (noPyc ./prototext-pyo3/tests)
        (noPyc ./fdp-scan-pyo3/tests)
      ];
    };

  # Each staged recipe, built as nixpkgs builds it from the tag, but from a
  # local source (workspaceSrc for the Rust applications, which need nothing
  # else), with the vendored
  # dependencies of the local Cargo.lock and the version the tree carries
  # (for the version checks).
  #   prototext (PR 1), protoscan (PR 2, spec 0407 S8), protolens (PR 4,
  #   spec 0410 S4), protoback and git-seriate (spec 0486 S4): Rust
  #   applications;
  #   the four python3Packages libraries, which took reproto's source,
  #   version and vendored crates (PR 3, spec 0411 S6) until reproto was
  #   retired (spec 0487), and now fetch their own.
  stagedRust = {
    prototext = ./nixpkgs/pkgs/by-name/pr/prototext/package.nix;
    protoscan = ./nixpkgs/pkgs/by-name/pr/protoscan/package.nix;
    protolens = ./nixpkgs/pkgs/by-name/pr/protolens/package.nix;
    protoback = ./nixpkgs/pkgs/by-name/pr/protoback/package.nix;
    git-seriate = ./nixpkgs/pkgs/by-name/gi/git-seriate/package.nix;
  };
  # The four libraries, python3Packages entries in nixpkgs.
  stagedPythonLibs = pkgs.lib.genAttrs
    [ "fdp-scan" "prototext-codec" "prototext-graph" "tree-sitter-textproto" ]
    (name: ./nixpkgs/pkgs/development/python-modules + "/${name}/default.nix");

  # The staged recipes built with a given nixpkgs and Python package set:
  # the main pin's for nixpkgs-staging, the unstable pin's for
  # nixpkgs-staging-unstable (spec 0423 S5).
  mkStaging = sp: py:
    let
      stagedBuild = src: version: drv: drv.overrideAttrs (old:
        let vendored = sp.rustPlatform.importCargoLock { lockFile = ./Cargo.lock; }; in
        { inherit src version; }
        // sp.lib.optionalAttrs (old ? cargoDeps) { cargoDeps = vendored; }
        // sp.lib.optionalAttrs (old ? passthru.cargoDeps) {
          passthru = old.passthru // { cargoDeps = vendored; };
        });
      rustBuilds = builtins.mapAttrs
        (name: file: stagedBuild workspaceSrc rust.workspaceVersion (sp.callPackage file { }))
        stagedRust;
      # The Python package set, as nixpkgs passes python3Packages, with
      # the four libraries in it.
      pythonPkgs = py // builtins.mapAttrs
        (_: file: stagedBuild stagingSrc rust.workspaceVersion (py.callPackage file { }))
        stagedPythonLibs;
    in {
      inherit rustBuilds pythonPkgs;
      farm = name: sp.linkFarmFromDrvs name
        (builtins.attrValues rustBuilds
          ++ map (lib: pythonPkgs.${lib}) (builtins.attrNames stagedPythonLibs));
    };

  # As nixpkgs builds the recipes: its default Python, with its own Python
  # protobuf, not spec 0460's (spec 0462 S2).
  staging = mkStaging pkgs pkgs.python3Packages;
  stagedRustBuilds   = staging.rustBuilds;
  stagedPythonPkgs   = staging.pythonPkgs;
  nixpkgsStaging     = staging.farm "nixpkgs-staging";

  # Against the nixos-unstable pin, with its default Python, as nixpkgs
  # builds the PRs (spec 0423 S4, S5).
  unstablePkgs = import unstable { };
  stagingUnstable = mkStaging unstablePkgs unstablePkgs.python3Packages;
  nixpkgsStagingUnstable = stagingUnstable.farm "nixpkgs-staging-unstable";

  # Every staged file, and the version each declares (callPackage'd as
  # nixpkgs calls it).
  stagedFiles = builtins.attrValues stagedRust
    ++ builtins.attrValues stagedPythonLibs;
  stagedVersions =
    builtins.mapAttrs (_: file: (pkgs.callPackage file { }).version) stagedRust
    // builtins.mapAttrs
      (_: file: (pkgs.python3Packages.callPackage file { }).version) stagedPythonLibs;

  # nixfmt, and the version rule: at a release every staged version equals
  # the workspace's; between releases the workspace carries X.Y.Z-dev, and a
  # staged version is at most X.Y.Z: older for a package nixpkgs already
  # ships (prototext), equal for one waiting for its first release there.
  nextRelease = pkgs.lib.removeSuffix "-dev" rust.workspaceVersion;
  stagedVersionOk = version:
    if nextRelease != rust.workspaceVersion
    then builtins.compareVersions version nextRelease <= 0
    else version == rust.workspaceVersion;
  nixpkgsStagingCheck = pkgs.runCommand "nixpkgs-staging-check" {
    strictDeps = true;
    nativeBuildInputs = [ pkgs.nixfmt ];
  } (''
    nixfmt --check ${pkgs.lib.concatMapStringsSep " " (file: "${file}") stagedFiles}
  '' + pkgs.lib.concatStrings (pkgs.lib.mapAttrsToList (name: version:
    pkgs.lib.optionalString (!stagedVersionOk version) ''
      echo "nixpkgs/: staged ${name} is ${version}, which does not fit" \
        "the workspace's ${rust.workspaceVersion} (equal at a release, at most ${nextRelease} during development)" >&2
      exit 1
    '') stagedVersions) + ''
    touch $out
  '');

  # Spec 0406 S6: press Tab in an interactive bash, on a pseudo-terminal
  # (the sandbox provides one), and check the line, for prototext,
  # protolens, protoscan and protoback (spec 0433 S7: -I-relative paths and
  # blob members). bashInteractive: stdenv's bash has no readline.
  completionTests = pkgs.runCommand "completion-tests" {
    strictDeps = true;
    nativeBuildInputs = [ pkgs.bashInteractive pkgs.python3 ];
  } ''
    mkdir bin
    ln -s ${rust.prototext}/bin/prototext ${rust.protolens}/bin/protolens \
      ${rust.protoscan}/bin/protoscan ${rust.protoback}/bin/protoback bin/
    python3 ${./prototools-complete/tests/tab_completion.py} bin | tee $out
  '';

  # nix.yml and scripts/ci run the same Nix steps in the same order (spec
  # 0485 S7): the commands of nix.yml's `run:` steps that name nix-build,
  # nix-instantiate or scripts/, its matrix target written CI_TARGET,
  # against `scripts/ci --list-steps`. A step added to one alone fails ci.
  ciWorkflowCheck = pkgs.runCommand "ci-workflow-check" {
    strictDeps = true;
    nativeBuildInputs = [ (pkgs.python3.withPackages (ps: [ ps.pyyaml ])) ];
  } ''
    bash ${./scripts/ci} --list-steps > script-steps
    python3 -I - ${./.github/workflows/nix.yml} script-steps <<'EOF'
    import re, sys, yaml
    workflow = yaml.safe_load(open(sys.argv[1]))
    ours = [l.strip() for l in open(sys.argv[2]) if l.strip()]
    theirs = []
    for job in workflow["jobs"].values():
        for step in job.get("steps", []):
            run = " ".join(str(step.get("run", "")).split())
            if re.search(r"nix-build|nix-instantiate|scripts/", run):
                theirs.append(run.replace("''${{ matrix.ci-target }}", "CI_TARGET"))
    if theirs != ours:
        print("nix.yml and scripts/ci do not run the same steps (spec 0485 S7):",
              file=sys.stderr)
        for label, steps in (("nix.yml", theirs), ("scripts/ci", ours)):
            print(f"  {label}:", file=sys.stderr)
            for s in steps:
                print(f"    {s}", file=sys.stderr)
        print("Add the step to both: nix.yml's steps, and nix_steps in scripts/ci.",
              file=sys.stderr)
        sys.exit(1)
    EOF
    touch $out
  '';

  # The release shell (spec 0412 S2): what scripts/check-crates and
  # scripts/publish-crates need, in CI's publish workflow and locally.
  releaseShell = pkgs.mkShell {
    nativeBuildInputs = [ pkgs.cargo pkgs.rustc pkgs.curl ];
    NIXSHELL_REPO = toString ./.;
  };

  full-tests = pkgs.linkFarmFromDrvs "full-tests" [
    ci python.googleapisDb python.googleapisTests python.customDb python.customTests
  ];

in
{
  default              = ci;
  # Spec 0461: the protobuf release, for downstream builds.
  inherit protobufRelease;
  prototools           = prototools;
  prototext            = rust.prototext;
  workspace-build      = rust.workspaceBuild;
  protolens            = rust.protolens;
  rust-fmt             = rust.rustFmt;
  rust-clippy          = rust.rustClippy;
  rust-tests           = rust.rustTests;
  descriptor-access-check = rust.descriptorAccessCheck;
  hakari-check         = rust.hakariCheck;
  prototext-codec      = rust.prototextCodec;
  wkt-rkyv             = wktRkyv;
  wkt-prebuilt-check   = wktPrebuiltCheck;
  fdp-scan-corpus      = fdpScanCorpus;
  fdp-scan-corpus-check = fdpScanCorpusCheck;
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
  bobapp-extra-desc    = python.bobappExtraDesc;
  bobapp               = bobappDemo;
  grpconf-demo         = grpconfDemo;
  inherit grehack2026;
  prototools-closure-check = prototoolsClosureCheck;
  user-shell           = shells.user-shell;
  dev-shell            = shells.dev-shell;
  release-shell        = releaseShell;
  grehack2026-shell    = demoShells.grehack2026-shell;
  grpconf2026-shell    = demoShells.grpconf2026-shell;
  teleprompt           = demoShells.teleprompt;
  wkt-db               = wktDb;
  prototext-fixtures   = prototextFixtures;
  nixpkgs-staging      = nixpkgsStaging;
  # One staged Rust application at a time: nix-build -A nixpkgs-staging-rust.protoback
  nixpkgs-staging-rust = stagedRustBuilds;
  # One staged Python library at a time: nix-build -A nixpkgs-staging-python.fdp-scan
  nixpkgs-staging-python = pkgs.lib.getAttrs (builtins.attrNames stagedPythonLibs) stagedPythonPkgs;
  nixpkgs-staging-check = nixpkgsStagingCheck;
  ci-workflow-check    = ciWorkflowCheck;
  nixpkgs-staging-unstable = nixpkgsStagingUnstable;
  prototext-fixtures-check = prototextFixturesCheck;
  protoscan            = rust.protoscan;
  protoback            = rust.protoback;
  git-seriate          = rust.gitSeriate;
  git-seriate-fdp      = rust.gitSeriateFdp;
  fdp-scan-lib         = rust.fdpScanLib;
  prototext-graph-lib  = rust.prototextGraphLib;
  crates-io            = cratesIo;
  pypi                 = pypi;
  tree-sitter-textproto-highlight-test = treeSitterTextprotoHighlightTest;
  tree-sitter-textproto-generated = treeSitterTextprotoGenerated;
  tree-sitter-textproto-check = treeSitterTextprotoCheck;
  wkt-variant-generated = wktVariantGenerated;
  wkt-variant-check     = wktVariantCheck;
  upb-release-check     = upbReleaseCheck;
  descriptor-copies-check = descriptorCopiesCheck;
}
