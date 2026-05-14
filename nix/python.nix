# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

# nix/python.nix — Python packages: prototextCodec, fdpScanLib,
#                  scoringGraphLib, reproto pipeline, protoscan,
#                  lint and ruff checks, stress tests.
#
# Source pipeline:
#
#   ./reproto/  ──[builtins.path]──▶  reprotoSrc
#                                          │
#                                 [pkgs.runCommand]
#                                          │
#                                          ▼
#                                   reprotoSrcFull
#                                          │
#                           ┌──────────────┼──────────────┐
#                           ▼              ▼              ▼
#                       reprotoBare    reproto      reprotoTests …

{ pkgs
, pythonPkgs
, pythonBin
, prototext
, prototextCodec
, fdpScanLib
, scoringGraphLib
, prototextExtensionArtifacts   # store path: $out/artifacts/ from prototext_codec ext
, scoringGraphExtensionArtifacts # store path: $out/artifacts/ from scoring_graph ext
, treeSitterTextproto
}:

let

  # ---------------------------------------------------------------------------
  # reproto source pipeline
  # ---------------------------------------------------------------------------

  # reprotoSrc — filtered snapshot of the ./reproto working-tree directory.
  # Uses builtins.path (an eval-time store import, not a build derivation).
  # Unstable files (.pb outputs, __pycache__, result symlinks) are excluded
  # to keep the store hash stable across unrelated working-tree changes.
  reprotoSrc = builtins.path {
    name   = "reproto-src";
    path   = ../reproto;
    filter = path: type:
      let
        base       = baseNameOf (toString path);
        skipPb     = type == "regular" && pkgs.lib.hasSuffix ".pb" base;
        skipCache  = base == "__pycache__";
        skipResult = pkgs.lib.hasPrefix "result" base;
      in
        !skipPb && !skipCache && !skipResult;
  };

  # Common Python dependencies for reproto (used by both reprotoBare and reproto).
  # reprotoPropagatedDeps: runtime deps only (no test tools, no codec).
  reprotoPropagatedDeps = [
    pythonPkgs.click
    pythonPkgs.google-re2
    pythonPkgs.lark
    pythonPkgs.protobuf
    pythonPkgs.pyvis
    pythonPkgs.pyyaml
    pythonPkgs.rapidfuzz
    pythonPkgs.rich
    pythonPkgs.types-protobuf
  ];

  # Full Python dependency set for running the reproto test suite and for the
  # dev-shell PYTHONPATH.  Extends reprotoPropagatedDeps with the codec, pytest,
  # and tree-sitter.  Used by reprotoTests, pythonLint, and dev-shell.
  reprotoTestDeps = reprotoPropagatedDeps ++ [
    prototextCodec
    fdpScanLib
    scoringGraphLib
    pythonPkgs.pytest
    pythonPkgs."pytest-xdist"
    pythonPkgs.tree-sitter
    pythonPkgs.tree-sitter-language-pack
  ];

  # Bootstrap package — installs reproto without running tests.
  # Provides bin/reproto and carries the patch scripts for the codegen stage.
  reprotoBare = pythonPkgs.buildPythonPackage {
    pname   = "reproto-bare";
    version = "0.1.0";
    src     = reprotoSrc;
    pyproject = true;

    nativeBuildInputs = [ pythonPkgs.setuptools pythonPkgs.wheel ];
    propagatedBuildInputs = reprotoPropagatedDeps;

    doCheck = false;

    postInstall = ''
      mkdir -p $out/patch
      cp -r ${reprotoSrc}/patch/* $out/patch/
    '';
  };

  # reprotoSrcFull — enriched source tree: copies reprotoSrc, seeds well-known
  # .proto sources from pkgs.protobuf, then runs patch_reproto.sh to compile
  # fixture .proto files into .pb descriptors.
  # "Full" signals that this is the complete, ready-for-buildPythonPackage tree.
  reprotoSrcFull = pkgs.runCommand "reproto-src-full" {
    buildInputs = [
      reprotoBare
      pkgs.protobuf        # provides protoc and well-known .proto includes
    ];
  } ''
    set -euo pipefail
    cp -r ${reprotoSrc} $out
    chmod -R u+w $out

    # Seed well-known-type .proto sources from pkgs.protobuf.
    mkdir -p $out/src/resources/google/protobuf
    cp ${pkgs.protobuf}/include/google/protobuf/*.proto \
       $out/src/resources/google/protobuf/

    bash ${reprotoBare}/patch/patch_reproto.sh "${reprotoBare}" "$out"
  '';

  # Final reproto package — built from the codegen output, with tests.
  reproto = pythonPkgs.buildPythonPackage {
    pname   = "reproto";
    version = "0.1.0";
    src     = reprotoSrcFull;
    pyproject = true;

    nativeBuildInputs = [
      pythonPkgs.setuptools
      pythonPkgs.wheel
      pythonPkgs.pytest
      pythonPkgs."pytest-xdist"
      pkgs.installShellFiles
    ];
    propagatedBuildInputs = reprotoPropagatedDeps ++ [
      prototextCodec   # reproto.load imports prototext_codec_lib at module load time
      scoringGraphLib  # reproto --build-schema-db imports scoring_graph_lib
    ];

    doCheck = false;

    postInstall = ''
      installShellCompletion --cmd reproto \
        --bash ${reprotoSrc}/src/reproto/completions.sh

      # Generate and install man page.
      $out/bin/reproto-gen-man $out/share/man/man1
    '';
  };

  # Tests run separately so that the installable reproto package has doCheck = false
  # (avoiding pytest during nix-shell) while ci still enforces test passage.
  reprotoTests = pkgs.runCommand "reproto-tests" {
    buildInputs = [
      pkgs.protobuf
      pkgs.buf
      prototext
      treeSitterTextproto
      (pythonPkgs.python.withPackages (_: reprotoTestDeps))
    ];
  } ''
    export PYTHONPATH="${reprotoSrcFull}/src:${treeSitterTextproto}"
    pytest -p no:cacheprovider ${reprotoSrcFull}/src/reproto/tests/ -x
    touch $out
  '';

  # ---------------------------------------------------------------------------
  # protoscan — Python CLI for scanning binaries for embedded FDP blobs
  # ---------------------------------------------------------------------------

  protoscan = pythonPkgs.buildPythonPackage {
    pname   = "protoscan";
    version = "0.1.0";
    src     = ../protoscan;
    pyproject = true;

    nativeBuildInputs = [
      pythonPkgs.setuptools
      pythonPkgs.wheel
      pkgs.installShellFiles
    ];
    propagatedBuildInputs = [
      pythonPkgs.click
      pythonPkgs.protobuf
      fdpScanLib
    ];

    doCheck = false;

    postInstall = ''
      installShellCompletion --cmd protoscan \
        --bash <(_PROTOSCAN_COMPLETE=bash_source $out/bin/protoscan)

      $out/bin/protoscan-gen-man $out/share/man/man1
    '';
  };

  # ---------------------------------------------------------------------------
  # Python lint — pyright type checking for the reproto Python package.
  #
  # Runs against reprotoSrcFull so the generated .pb descriptor files
  # are present.  The extension artifacts are injected so pyright can
  # resolve prototext_codec_lib and scoring_graph_lib imports via the
  # generated .pyi stubs.
  # ---------------------------------------------------------------------------
  pythonLint = pkgs.runCommand "python-lint" {
    buildInputs = [
      pkgs.pyright
      treeSitterTextproto
      (pythonPkgs.python.withPackages (_: reprotoPropagatedDeps ++ [
        pythonPkgs.pytest
        pythonPkgs.tree-sitter
        pythonPkgs.tree-sitter-language-pack
      ]))
    ];
  } ''
    set -euo pipefail

    # pyright needs a writable working directory for its cache.
    cd "$TMPDIR"

    # Make the prototext_codec_lib, scoring_graph_lib .pyi stubs and
    # textproto extension visible to pyright.
    export PYTHONPATH="${reprotoSrcFull}/src:${prototextExtensionArtifacts}:${scoringGraphExtensionArtifacts}:${treeSitterTextproto}"

    # Write a hermetic pyrightconfig.json.
    cat > pyrightconfig.json <<EOF
{
  "typeCheckingMode": "basic",
  "extraPaths": [
    "${reprotoSrcFull}/src",
    "${prototextExtensionArtifacts}",
    "${scoringGraphExtensionArtifacts}",
    "${treeSitterTextproto}"
  ],
  "exclude": [
    "result*",
    "docs/mockup"
  ]
}
EOF

    echo "--- pyright ---"
    pyright ${reprotoSrcFull}/src/

    touch $out
  '';

  # ---------------------------------------------------------------------------
  # Python ruff check — style and correctness linting for the reproto package.
  # ---------------------------------------------------------------------------
  pythonRuff = pkgs.runCommand "python-ruff" {
    buildInputs = [ pythonPkgs.ruff ];
  } ''
    set -euo pipefail
    echo "--- ruff ---"
    ruff check --no-cache --exclude docs/mockup ${reprotoSrcFull}/src/
    touch $out
  '';

  # ---------------------------------------------------------------------------
  # Stress tests (spec 0054) — separate from ci; triggered by
  # nix-build -A full-tests only.
  #
  # Two derivations:
  #   stressDb    — fetches pinned corpora, compiles protos, runs
  #                 reproto --build-schema-db.  Cached by Nix; only rebuilt
  #                 when inputs (corpora hashes, fixture protos, reproto) change.
  #   stressTests — runs pytest with STRESS_DB pointing at stressDb.
  #                 Rebuilt whenever the test code or prototext changes.
  # ---------------------------------------------------------------------------

  # Pinned remote corpora (must match REMOTE_CORPORA in test_stress.py).
  stressCorpusGoogleapis = pkgs.fetchzip {
    url    = "https://github.com/googleapis/googleapis/archive/83e70370751716489986478edc8713b455b21e86.tar.gz";
    sha256 = "03xhi19zkcmfqzlzpn3inma8aj09a9xq8kvn3frsvsjv55k0y9d1";
    stripRoot = true;
  };
  stressCorpusOtel = pkgs.fetchzip {
    url    = "https://github.com/open-telemetry/opentelemetry-proto/archive/1d70aa012dc42a5e74a215ce31c1fd84244ce89e.tar.gz";
    sha256 = "1rp5sv9rbkvdrqcy58pgq35j6pbqh6hgsz34c54mk982pbkgg1bx";
    stripRoot = true;
  };

  # Build the schema DB once; cached until inputs change.
  stressDb = pkgs.runCommand "stress-db" {
    buildInputs = [
      pkgs.protobuf
      reproto
    ];
  } ''
    set -euo pipefail

    FIXTURES="${reprotoSrcFull}/src/reproto/tests/fixtures"
    PB=$TMPDIR/pb
    mkdir -p "$PB"

    # ── Compile fixture protos ────────────────────────────────────────────────
    compile_fixture() {
      local proto=$1 stem
      stem=''${proto%.proto}
      protoc -I"$FIXTURES" --descriptor_set_out="$PB/$stem.pb" "$proto"
    }
    compile_fixture field_comprehensive.proto
    compile_fixture default_values_proto2.proto
    compile_fixture group_proto2.proto
    compile_fixture extensions_proto2.proto
    compile_fixture message_comprehensive.proto
    compile_fixture packed_proto3.proto
    compile_fixture phone_number.proto
    compile_fixture address_book.proto
    compile_fixture editions_rendering.proto

    # ── Compile a corpus dir (skip failures — missing imports) ────────────────
    compile_corpus() {
      local corpus_root=$1
      find "$corpus_root" -name '*.proto' | sort | while read -r proto; do
        local rel flat
        rel=''${proto#"$corpus_root/"}
        flat=''${rel//\//_}
        flat=''${flat%.proto}
        protoc --proto_path="$corpus_root" \
               --descriptor_set_out="$PB/$flat.pb" \
               "$rel" 2>/dev/null || rm -f "$PB/$flat.pb"
      done
    }

    # googleapis — skip the preview/ subtree
    find "${stressCorpusGoogleapis}" -name '*.proto' \
         ! -path "${stressCorpusGoogleapis}/preview/*" \
         | sort | while read -r proto; do
      rel=''${proto#"${stressCorpusGoogleapis}/"}
      flat=''${rel//\//_}; flat=''${flat%.proto}
      protoc --proto_path="${stressCorpusGoogleapis}" \
             --descriptor_set_out="$PB/$flat.pb" \
             "$rel" 2>/dev/null || rm -f "$PB/$flat.pb"
    done

    # opentelemetry-proto
    compile_corpus "${stressCorpusOtel}"

    # ── Build the schema DB ───────────────────────────────────────────────────
    mkdir -p "$out"
    reproto \
      --use-variant all \
      --prost-workaround \
      -I"$PB" \
      --output-root="$out/reproto-out" \
      --emit-scoring-graphs \
      --build-schema-db="$out/stress.desc" \
      .
  '';

  stressTests = pkgs.runCommand "stress-tests" {
    buildInputs = [
      prototext
      reproto
      (pythonPkgs.python.withPackages (_: reprotoTestDeps))
    ];
  } ''
    set -euo pipefail
    export PYTHONPATH="${reprotoSrcFull}/src"
    export STRESS_DB="${stressDb}/stress.desc"
    pytest -p no:cacheprovider ${../tests/stress}/
    touch $out
  '';

in {
  inherit
    reprotoSrc
    reprotoSrcFull
    reprotoBare
    reprotoPropagatedDeps
    reprotoTestDeps
    reproto
    reprotoTests
    protoscan
    pythonLint
    pythonRuff
    stressDb
    stressTests;
}
