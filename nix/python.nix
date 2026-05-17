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

  # Wrap treeSitterTextproto (a bare .so store path) as a minimal Python
  # package so it can appear in propagatedBuildInputs and propagates
  # correctly to consumers.  The .so is installed directly into site-packages.
  treeSitterTextprotoPkg = pythonPkgs.buildPythonPackage {
    pname   = "textproto";
    version = "0.1.0";
    format  = "other";
    src     = treeSitterTextproto;
    installPhase = ''
      site="$out/lib/${pythonPkgs.python.libPrefix}/site-packages"
      mkdir -p "$site"
      cp ${treeSitterTextproto}/textproto*.so "$site/"
      cp ${treeSitterTextproto}/textproto.pyi "$site/"
    '';
  };

  # Common Python dependencies for reproto (used by both reprotoBare and reproto).
  # reprotoPropagatedDeps: runtime deps only (no test tools, no codec).
  # tree-sitter and tree-sitter-language-pack are runtime deps because
  # split_fdps.py imports them at module load time (top-level imports).
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
    pythonPkgs.tree-sitter
    pythonPkgs.tree-sitter-language-pack
    treeSitterTextprotoPkg
  ];

  # Full Python dependency set for running the reproto test suite and for the
  # dev-shell PYTHONPATH.  Extends reprotoPropagatedDeps with the codec and
  # pytest tools.  Used by reprotoTests, pythonLint, and dev-shell.
  reprotoTestDeps = reprotoPropagatedDeps ++ [
    prototextCodec
    fdpScanLib
    scoringGraphLib
    pythonPkgs.pytest
    pythonPkgs."pytest-xdist"
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
      (pythonPkgs.python.withPackages (_: reprotoTestDeps))
    ];
  } ''
    export PYTHONPATH="${reprotoSrcFull}/src"
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
      (pythonPkgs.python.withPackages (_: reprotoPropagatedDeps ++ [
        pythonPkgs.pytest
      ]))
    ];
  } ''
    set -euo pipefail

    # pyright needs a writable working directory for its cache.
    cd "$TMPDIR"

    # Make the prototext_codec_lib and scoring_graph_lib .pyi stubs visible to
    # pyright.  tree-sitter, tree-sitter-language-pack, and treeSitterTextproto
    # are now in reprotoPropagatedDeps and reach pyright via the Python env.
    export PYTHONPATH="${reprotoSrcFull}/src:${prototextExtensionArtifacts}:${scoringGraphExtensionArtifacts}"

    # Write a hermetic pyrightconfig.json.
    cat > pyrightconfig.json <<EOF
{
  "typeCheckingMode": "basic",
  "extraPaths": [
    "${reprotoSrcFull}/src",
    "${prototextExtensionArtifacts}",
    "${scoringGraphExtensionArtifacts}"
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
  # googleapis DB + tests — separate from ci; triggered by
  # nix-build -A full-tests only.
  #
  # Two derivations:
  #   googleapisDb    — fetches pinned googleapis corpus, compiles protos,
  #                     runs reproto --build-schema-db, and instantiates
  #                     N_INSTANCES randomly-sampled .pb messages.
  #                     Cached by Nix; only rebuilt when inputs change.
  #   googleapisTests — runs pytest with STRESS_DB pointing at googleapisDb.
  #                     Rebuilt whenever the test code or prototext changes.
  # ---------------------------------------------------------------------------

  # Pinned googleapis corpus.
  corpusGoogleapis = pkgs.fetchzip {
    url    = "https://github.com/googleapis/googleapis/archive/83e70370751716489986478edc8713b455b21e86.tar.gz";
    sha256 = "03xhi19zkcmfqzlzpn3inma8aj09a9xq8kvn3frsvsjv55k0y9d1";
    stripRoot = true;
  };

  # Compile every .proto in the googleapis corpus into a single multi-FDP FDS.
  # Pure function of corpus + protoc; cached independently of the DB build so
  # that changes to reproto/instantiation logic don't force a recompile.
  googleapisPbs = pkgs.runCommand "googleapis-pbs" {
    buildInputs = [ pkgs.protobuf ];
  } ''
    set -euo pipefail
    mkdir -p "$out"

    # Collect all .proto paths (excluding preview/) into a response file.
    # protoc supports @<file> to avoid ARG_MAX limits with large corpora.
    # NOTE: must use $TMPDIR, not /tmp — on macOS the Nix sandbox does not
    # grant write access to /tmp (which resolves to /private/tmp), so writing
    # there would fail with "Permission denied" on x86_64-darwin builds.
    find "${corpusGoogleapis}" -name '*.proto' \
         ! -path "${corpusGoogleapis}/preview/*" \
         | sort | sed "s|^${corpusGoogleapis}/||" > "$TMPDIR/proto_list.txt"

    # Single protoc invocation: one multi-FDP FDS covering the whole corpus.
    protoc \
      --proto_path="${corpusGoogleapis}" \
      --descriptor_set_out="$out/googleapis.pb" \
      --include_imports \
      @"$TMPDIR/proto_list.txt"
  '';

  # Build the googleapis schema DB + instantiated messages.
  # Depends on googleapisPbs (single multi-FDP FDS) so proto compilation is
  # not repeated when reproto or instantiation logic changes.
  googleapisDb = pkgs.runCommand "googleapis-db" {
    buildInputs = [
      prototext
      reproto
      (pythonPkgs.python.withPackages (_: reprotoPropagatedDeps))
    ];
  } ''
    set -euo pipefail

    # ── Build the schema DB ───────────────────────────────────────────────────
    mkdir -p "$out"
    reproto \
      --use-variant all \
      --prost-workaround \
      --output-root="$out/reproto-out" \
      --emit-scoring-graphs \
      --build-schema-db="$out/googleapis.desc" \
      "${googleapisPbs}/googleapis.pb"

    # ── Instantiate one .pb per sampled type ──────────────────────────────────
    # Number of types to instantiate (includes potential 0-byte skips).
    N_INSTANCES=400
    TYPES_YAML=${../tests/stress/googleapis-types.yaml}
    # Mandatory set: types listed in googleapis-types.yaml (used by tests).
    MANDATORY=$(grep '^\s*- ' "$TYPES_YAML" | sed 's/^\s*- //')
    # Full type list from the DB (empty protobuf matches everything).
    ALL=$(printf "" | prototext --descriptor "$out/googleapis.desc" list-schemas --top 999999 | grep '^  - ' | sed 's/^  - //')
    # Sample additional types to reach N_INSTANCES, excluding mandatory ones.
    EXTRA=$(comm -23 \
              <(echo "$ALL"   | sort -u) \
              <(echo "$MANDATORY" | sort -u) \
            | shuf -n $(( N_INSTANCES - $(echo "$MANDATORY" | wc -l) )))
    FQDNS=$(printf '%s\n%s' "$MANDATORY" "$EXTRA")
    mkdir -p "$out/instances"
    reproto-instantiate-schema \
      --descriptor "$out/googleapis.desc" \
      -O "$out/instances" \
      $FQDNS
  '';

  googleapisTests = pkgs.runCommand "googleapis-tests" {
    buildInputs = [
      prototext
      reproto
      (pythonPkgs.python.withPackages (_: reprotoTestDeps))
    ];
  } ''
    set -euo pipefail
    export PYTHONPATH="${reprotoSrcFull}/src"
    export STRESS_DB="${googleapisDb}/googleapis.desc"
    pytest -p no:cacheprovider ${../tests/stress}/
    touch $out
  '';

  # ---------------------------------------------------------------------------
  # Custom DB + tests — fixture types and opentelemetry-proto.
  #
  # Two derivations:
  #   customDb    — compiles reproto fixture protos + opentelemetry-proto corpus,
  #                 runs reproto --build-schema-db.
  #   customTests — runs pytest with CUSTOM_DB pointing at customDb.
  # ---------------------------------------------------------------------------

  # Pinned opentelemetry-proto corpus.
  corpusOtel = pkgs.fetchzip {
    url    = "https://github.com/open-telemetry/opentelemetry-proto/archive/1d70aa012dc42a5e74a215ce31c1fd84244ce89e.tar.gz";
    sha256 = "1rp5sv9rbkvdrqcy58pgq35j6pbqh6hgsz34c54mk982pbkgg1bx";
    stripRoot = true;
  };

  customDb = pkgs.runCommand "custom-db" {
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

    # ── Compile opentelemetry-proto corpus (skip failures) ────────────────────
    find "${corpusOtel}" -name '*.proto' | sort | while read -r proto; do
      rel=''${proto#"${corpusOtel}/"}
      flat=''${rel//\//_}
      flat=''${flat%.proto}
      protoc --proto_path="${corpusOtel}" \
             --descriptor_set_out="$PB/$flat.pb" \
             "$rel" 2>/dev/null || rm -f "$PB/$flat.pb"
    done

    # ── Build the schema DB ───────────────────────────────────────────────────
    mkdir -p "$out"
    reproto \
      --use-variant all \
      --prost-workaround \
      -I"$PB" \
      --output-root="$out/reproto-out" \
      --emit-scoring-graphs \
      --build-schema-db="$out/custom.desc" \
      .
  '';

  customTests = pkgs.runCommand "custom-tests" {
    buildInputs = [
      prototext
      reproto
      (pythonPkgs.python.withPackages (_: reprotoTestDeps))
    ];
  } ''
    set -euo pipefail
    export PYTHONPATH="${reprotoSrcFull}/src"
    export CUSTOM_DB="${customDb}/custom.desc"
    pytest -p no:cacheprovider ${../tests/custom}/
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
    googleapisPbs
    googleapisDb
    googleapisTests
    customDb
    customTests;
}
