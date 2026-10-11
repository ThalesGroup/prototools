# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

# nix/python.nix — Python packages: prototextCodec, fdpScanLib,
#                  prototextGraphLib, reproto pipeline,
#                  lint and ruff checks, stress tests.
#
# Source pipeline:
#
#   ./reproto/  ──[lib.fileset]──▶  reprotoSrc   (src/, pyproject.toml, patch/)
#                                          │
#                                 [pkgs.runCommand]
#                                          │
#                                          ▼
#                                   reprotoSrcFull
#                                          │
#                           ┌──────────────┼──────────────┐
#                           ▼              ▼              ▼
#                        wktRkyv        reproto      reprotoTests …

{ pkgs
, metaCommon        # nix/rust.nix: homepage, license, maintainers, platforms
, pyprojectVersion  # nix/rust.nix: a pyproject.toml's version, checked (spec 0405 S5)
, pythonPkgs
, pythonBin
, prototext
, prototextCodec
, fdpScanLib
, prototextGraphLib
, prototextExtensionArtifacts      # store path: $out/artifacts/ from prototext_codec ext
, prototextGraphExtensionArtifacts # store path: $out/artifacts/ from prototext_graph ext
, fdpScanExtensionArtifacts       # store path: $out/artifacts/ from fdp_scan ext
}:

let
  # Spec 0402 S3: versions from the manifests (see nix/rust.nix).
  reprotoVersion   = pyprojectVersion "reproto";
  # Spec 0402 S7: what runs a freshly installed script runs only where the
  # host's binaries can run.
  whenRunnable = pkgs.lib.optionalString
    (pkgs.stdenv.buildPlatform.canExecute pkgs.stdenv.hostPlatform);


  # ---------------------------------------------------------------------------
  # reproto source pipeline
  # ---------------------------------------------------------------------------

  # reprotoSrc — what the reproto package is built from: src/,
  # pyproject.toml and patch/ (spec 0401 S7). README.md and
  # tree-sitter-textproto/ are left out, so editing them rebuilds nothing:
  # reprotoSrcFull feeds wktRkyv, whose graph wkt-prebuilt-check compares.
  # The .pb descriptors patch_reproto.sh writes into a working tree (the
  # dev-shell's codegen hook) are left out too, the committed .golden.pb
  # fixtures kept, and so are __pycache__ files.
  reprotoSrc =
    let
      fs   = pkgs.lib.fileset;
      root = ../reproto;
      generated = fs.fileFilter (f:
        (f.hasExt "pb" && !(pkgs.lib.hasSuffix ".golden.pb" f.name))
        || f.hasExt "pyc") (root + "/src");
    in fs.toSource {
      inherit root;
      fileset = fs.difference
        (fs.unions [ (root + "/src") (root + "/pyproject.toml") (root + "/patch") ])
        generated;
    };

  # The textproto grammar's Python module (spec 0408 S4): a setuptools
  # package with one C extension, built from the committed parser
  # (src/parser.c) and binding.c with a C compiler alone.
  treeSitterTextprotoPkg = pythonPkgs.buildPythonPackage {
    pname   = "tree-sitter-textproto";
    version = pyprojectVersion "reproto/tree-sitter-textproto";
    meta = metaCommon // {
      description = "The textproto tree-sitter grammar, for the Python tree_sitter library";
    };
    src = pkgs.lib.fileset.toSource {
      root    = ../reproto/tree-sitter-textproto;
      fileset = pkgs.lib.fileset.unions [
        ../reproto/tree-sitter-textproto/pyproject.toml
        ../reproto/tree-sitter-textproto/setup.py
        ../reproto/tree-sitter-textproto/binding.c
        ../reproto/tree-sitter-textproto/src
        ../reproto/tree-sitter-textproto/textproto
      ];
    };
    pyproject = true;
    build-system = [ pythonPkgs.setuptools ];
    pythonImportsCheck = [ "textproto" ];
  };

  # Common Python dependencies for reproto.
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
    pythonPkgs.tree-sitter
    pythonPkgs.tree-sitter-language-pack
    treeSitterTextprotoPkg
  ];

  # Python dependency set for the wktRkyv derivation: everything reproto
  # needs to run `--schema-db-out`, and nothing more.
  #
  # fdpScanLib is not needed here. reproto imports fdp_scan_lib only for
  # -I on a blob (spec 0243), from inside that branch, never at module
  # load, so wktRkyv runs without it; it is attached to the final reproto
  # package instead. (It used to be banned outright: fdp_scan_lib embedded
  # the freshly generated graph, so it depended on wktRkyv. It now embeds
  # the committed one, spec 0401 S2, and the ban is history.)
  wktRkyvDeps = reprotoPropagatedDeps ++ [
    prototextCodec
    prototextGraphLib
    pythonPkgs.pytest
    pythonPkgs."pytest-xdist"
  ];

  # Full Python dependency set for running the reproto test suite and for the
  # dev-shell PYTHONPATH.  Extends reprotoPropagatedDeps with the codec and
  # pytest tools.  Used by reprotoTests, pythonLint, and dev-shell.
  # types-protobuf is typing stubs, for pyright: lint and dev-shell only,
  # never a runtime dependency (spec 0402 S5).
  reprotoTestDeps = wktRkyvDeps ++ [ fdpScanLib pythonPkgs.types-protobuf ];

  # reprotoSrcFull — enriched source tree: copies reprotoSrc, seeds well-known
  # .proto sources from pkgs.protobuf, then runs patch_reproto.sh to compile
  # fixture .proto files into .pb descriptors. The script needs only protoc,
  # so it runs straight from the source (spec 0401 S6).
  # "Full" signals that this is the complete, ready-for-buildPythonPackage tree.
  reprotoSrcFull = pkgs.runCommand "reproto-src-full" {
    strictDeps = true;
    nativeBuildInputs = [
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

    bash ${reprotoSrc}/patch/patch_reproto.sh "$out"
  '';

  # Final reproto package — built from the codegen output, with tests.
  reproto = pythonPkgs.buildPythonPackage {
    pname   = "reproto";
    version = reprotoVersion;
    meta = metaCommon // {
      description = "Reconstruct .proto sources and schema databases from protobuf descriptors";
      mainProgram = "reproto";
    };
    src     = reprotoSrcFull;
    pyproject = true;

    # Spec 0402 S5: build-system, dependencies, and checks as nixpkgs
    # spells them. The test suite runs in reprotoTests (ci), not here;
    # the install check is the import of the package.
    build-system = [ pythonPkgs.setuptools ];
    nativeBuildInputs = [ pkgs.installShellFiles ];
    dependencies = reprotoPropagatedDeps ++ [
      prototextCodec   # reproto.load imports prototext_codec_lib at module load time
      prototextGraphLib  # reproto --schema-db-out imports scoring_graph_lib
      # reproto -I <blob> imports fdp_scan_lib to expand the blob (spec
      # 0243 S10). It belongs here and NOT in reprotoPropagatedDeps, for
      # the reason spelled out on wktRkyvDeps above: the shared list is
      # what wktRkyv runs reproto with, and wktRkyv's graph is what
      # wkt-prebuilt-check compares. reproto.load defers the import into
      # the blob branch, so wktRkyv never needs it.
      fdpScanLib
    ];

    doCheck = false;
    doInstallCheck = true;
    pythonImportsCheck = [ "reproto" ];
    # Spec 0402 S8: `reproto --version` says the version Nix calls it
    # (meta.mainProgram names the program).
    nativeInstallCheckInputs = [ pkgs.versionCheckHook ];
    passthru.tests = { inherit reprotoTests; };

    postInstall = ''
      installShellCompletion --cmd reproto \
        --bash ${reprotoSrc}/src/reproto/completions.sh

    '' + whenRunnable ''
      # Generate and install man page.
      $out/bin/reproto-gen-man $out/share/man/man1
    '';
  };

  # Tests run separately so that the installable reproto package has doCheck = false
  # (avoiding pytest during nix-shell) while ci still enforces test passage.
  reprotoTests = pkgs.runCommand "reproto-tests" {
    strictDeps = true;
    nativeBuildInputs = [
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

  # Tests for fdp_scan_lib — run against the installed extension (no PYTHONPATH
  # override needed; the installed package is already on the Python path via
  # withPackages).
  fdpScanTests = pkgs.runCommand "fdp-scan-tests" {
    strictDeps = true;
    nativeBuildInputs = [
      (pythonPkgs.python.withPackages (_: [
        fdpScanLib
        pythonPkgs.protobuf
        pythonPkgs.pytest
        pythonPkgs."pytest-xdist"
      ]))
    ];
  } ''
    pytest -p no:cacheprovider ${../fdp-scan-pyo3}/tests/ -x
    touch $out
  '';

  # Tests for prototext_codec_lib — run against the installed extension.
  prototextCodecTests = pkgs.runCommand "prototext-codec-tests" {
    strictDeps = true;
    nativeBuildInputs = [
      (pythonPkgs.python.withPackages (_: [
        prototextCodec
        pythonPkgs.protobuf
        pythonPkgs.pytest
        pythonPkgs."pytest-xdist"
      ]))
    ];
  } ''
    pytest -p no:cacheprovider ${../prototext-pyo3}/tests/ -x
    touch $out
  '';

  # ---------------------------------------------------------------------------
  # Python lint — pyright type checking for the reproto Python package.
  #
  # Runs against reprotoSrcFull so the generated .pb descriptor files
  # are present.  The extension artifacts are injected so pyright can
  # resolve prototext_codec_lib, scoring_graph_lib and fdp_scan_lib
  # imports via the generated .pyi stubs.
  #
  # fdp_scan_lib is here even though it is banned from
  # reprotoPropagatedDeps (see the comment there).  The ban exists so
  # that wktRkyv's inputs stay free of the WKT graph reproto is itself
  # used to build; pythonLint is a leaf nothing builds from, so
  # depending on the extension closes no cycle.  It has to be listed:
  # load.py's import of it is deferred to inside the function (spec 0243
  # S10) for that same eval-recursion reason, but pyright resolves
  # imports statically wherever they sit, so deferring hides nothing
  # from it.
  # ---------------------------------------------------------------------------
  pythonLint = pkgs.runCommand "python-lint" {
    strictDeps = true;
    nativeBuildInputs = [
      pkgs.pyright
      (pythonPkgs.python.withPackages (_: reprotoPropagatedDeps ++ [
        pythonPkgs.pytest
        pythonPkgs.types-protobuf
      ]))
    ];
  } ''
    set -euo pipefail

    # pyright needs a writable working directory for its cache.
    cd "$TMPDIR"

    # Make the prototext_codec_lib, scoring_graph_lib and fdp_scan_lib
    # .pyi stubs visible to pyright.  tree-sitter, tree-sitter-language-pack,
    # and treeSitterTextproto are now in reprotoPropagatedDeps and reach
    # pyright via the Python env.
    export PYTHONPATH="${reprotoSrcFull}/src:${prototextExtensionArtifacts}:${prototextGraphExtensionArtifacts}:${fdpScanExtensionArtifacts}"

    # Write a hermetic pyrightconfig.json.
    cat > pyrightconfig.json <<EOF
{
  "_comment": "Auto-generated by nix/python.nix — do not edit by hand.",
  "pythonVersion": "3.13",
  "typeCheckingMode": "basic",
  "extraPaths": [
    "${reprotoSrcFull}/src",
    "${prototextExtensionArtifacts}",
    "${prototextGraphExtensionArtifacts}",
    "${fdpScanExtensionArtifacts}"
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
    strictDeps = true;
    nativeBuildInputs = [ pythonPkgs.ruff ];
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
  #                     runs reproto --schema-db-out, and instantiates
  #                     N_INSTANCES .pb messages: hand-crafted fixtures first,
  #                     then a seeded pseudo-random draw for the remainder.
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
    strictDeps = true;
    nativeBuildInputs = [ pkgs.protobuf ];
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

  # Descriptor sets for bobapp (spec 0350 S3).
  #
  # Two separate sets are produced:
  #
  #   bobappDesc      — Places/SearchText transitive closure only.
  #                     Embedded in the binary via include_bytes!.
  #                     What protoscan finds when it scans the binary.
  #                     Does NOT include log.proto or Routes types.
  #
  #   bobappExtraDesc — Routes/ComputeRoutes transitive closure + log.proto.
  #                     Loaded at runtime from BOBAPP_EXTRA_DESCRIPTOR_SET.
  #                     Used for all log encoding (Log, Entry, both payload
  #                     types).  Not embedded; never found by protoscan.
  #
  # log.proto imports both services, so it cannot be compiled with
  # --include_imports into the Places-only set without pulling Routes in.
  # It is therefore excluded from bobappDesc entirely: the log envelope is
  # opaque under bobapp.desc, which is the demo's intent.
  #
  # Cheap enough for `ci`: two small protoc runs over a handful of files.
  # Pulls in corpusGoogleapis but not googleapisPbs or googleapisDb.
  bobappDescOf = variant: entryPoints: pkgs.runCommand "${variant}-desc" {
    strictDeps = true;
    nativeBuildInputs = [ pkgs.protobuf ];
  } ''
    set -euo pipefail
    mkdir -p "$out"

    protoc \
      --proto_path="${corpusGoogleapis}" \
      --proto_path="${../demo/bobapp/proto}" \
      --descriptor_set_out="$out/${variant}.desc" \
      --include_imports \
      ${pkgs.lib.escapeShellArgs entryPoints}
  '';

  # Embedded — Places/SearchText only; what protoscan finds in the binary.
  bobappDesc = bobappDescOf "bobapp" [
    "google/maps/places/v1/places_service.proto"
  ];

  # Extra — full runtime pool: Routes + Places + log.proto + error_details.
  # Used for all encoding (ComputeRoutes, SearchText, log).  Not embedded;
  # never found by protoscan.  error_details.proto is needed by anomaly.rs
  # (google.rpc.ErrorInfo for the debug trace).
  bobappExtraDesc = bobappDescOf "bobapp-extra" [
    "google/maps/routing/v2/routes_service.proto"
    "google/maps/places/v1/places_service.proto"
    "google/rpc/error_details.proto"
    "bobapp/v1/log.proto"
  ];

  # Build the googleapis schema DB + instantiated messages.
  # Depends on googleapisPbs (single multi-FDP FDS) so proto compilation is
  # not repeated when reproto or instantiation logic changes.
  googleapisDb = pkgs.runCommand "googleapis-db" {
    strictDeps = true;
    nativeBuildInputs = [
      prototext
      reproto
      (pythonPkgs.python.withPackages (_: reprotoPropagatedDeps))
    ];
  } ''
    set -euo pipefail

    # ── Build the schema DB ───────────────────────────────────────────────────
    # -O must be the schema-db stub's own `proto` child: that is the one
    # location protolens looks in when no -I/--proto-root is given (spec
    # 0155 G2), so `protolens <db>/googleapis.desc` gets working
    # jump-to-definition with no flag. It is also the only place inside
    # the stub directory reproto permits (spec 0155 G1), the rest of it
    # being hopcroft.rkyv/index.rkyv.
    #
    mkdir -p "$out"
    reproto \
      --use-variant all \
      --force-proto2-for-editions \
      --proto-out="$out/googleapis/proto" \
      --emit-scoring-yaml \
      --emit-binary \
      --schema-db-out="$out/googleapis.desc" \
      "${googleapisPbs}/googleapis.pb"

    # ── Instantiate one .pb per sampled type ──────────────────────────────────
    # Number of types to instantiate (includes potential 0-byte skips).
    N_INSTANCES=400
    TYPES_YAML=${builtins.path { name = "googleapis-types-yaml"; path = ../tests/stress/googleapis-types.yaml; }}
    FIXTURES_DIR=${builtins.path { name = "googleapis-fixtures"; path = ../tests/fixtures/instances; }}

    # Phase 1: encode hand-crafted fixtures into .pb files.
    # These are committed to git (as annotated textproto with .pb extension)
    # and have well-known, realistic content.
    # They are always present regardless of the random draw in phase 2.
    mkdir -p "$out/instances"
    find "$FIXTURES_DIR" -name "*.pb" | while read f; do
      rel=$(realpath --relative-to="$FIXTURES_DIR" "$f")
      pb="$out/instances/$rel"
      mkdir -p "$(dirname "$pb")"
      prototext encode < "$f" > "$pb"
    done

    # Phase 2: fill remaining slots with a pseudo-random draw.
    # Uses a seeded Python shuffle for a stable, reproducible permutation
    # that is independent of the OS random source.
    MANDATORY=$(grep '^\s*- ' "$TYPES_YAML" | sed 's/^\s*- //')
    # Full type list from the DB (empty protobuf matches everything).
    # list-schemas now emits YAML with "  - type: <fqdn>" lines.
    ALL=$(printf "" | prototext --descriptor-set "$out/googleapis.desc" list-schemas --top 999999 | grep '^  - type: ' | sed 's/^  - type: //')
    # Already-instantiated types (phase 1 + mandatory) — don't overwrite.
    ALREADY=$(find "$out/instances" -name "*.pb" | sed "s|$out/instances/||;s|\.pb$||;s|/|.|g")
    N_MANDATORY=$(echo "$MANDATORY" | wc -l)
    N_ALREADY=$(echo "$ALREADY" | grep -c . || true)
    N_EXTRA=$(( N_INSTANCES - N_MANDATORY - N_ALREADY ))
    EXTRA=""
    if [ "$N_EXTRA" -gt 0 ]; then
      EXTRA=$(comm -23 \
                <(echo "$ALL"      | sort -u) \
                <(printf '%s\n%s\n%s' "$MANDATORY" "$ALREADY" "" | sort -u) \
              | python3 -c "
import sys, random
lines = [l.rstrip() for l in sys.stdin if l.strip()]
random.seed('prototools-googleapis-instances-v1')
random.shuffle(lines)
print('\n'.join(lines[:$N_EXTRA]))
")
    fi
    # Exclude mandatory types already covered by phase 1 hand-crafted fixtures.
    MANDATORY_NEW=$(comm -23 \
                     <(echo "$MANDATORY" | sort -u) \
                     <(echo "$ALREADY"   | sort -u))
    FQDNS=$(printf '%s\n%s' "$MANDATORY_NEW" "$EXTRA")
    reproto-instantiate-schema \
      --descriptor-set "$out/googleapis.desc" \
      -O "$out/instances" \
      $FQDNS
  '';

  googleapisTests = pkgs.runCommand "googleapis-tests" {
    strictDeps = true;
    nativeBuildInputs = [
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
  #                 runs reproto --schema-db-out.
  #   customTests — runs pytest with CUSTOM_DB pointing at customDb.
  # ---------------------------------------------------------------------------

  # Pinned opentelemetry-proto corpus.
  corpusOtel = pkgs.fetchzip {
    url    = "https://github.com/open-telemetry/opentelemetry-proto/archive/1d70aa012dc42a5e74a215ce31c1fd84244ce89e.tar.gz";
    sha256 = "1rp5sv9rbkvdrqcy58pgq35j6pbqh6hgsz34c54mk982pbkgg1bx";
    stripRoot = true;
  };

  customDb = pkgs.runCommand "custom-db" {
    strictDeps = true;
    nativeBuildInputs = [
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
      --force-proto2-for-editions \
      -I"$PB" \
      --proto-out="$out/custom/proto" \
      --emit-scoring-yaml \
      --schema-db-out="$out/custom.desc" \
      .
  '';

  customTests = pkgs.runCommand "custom-tests" {
    strictDeps = true;
    nativeBuildInputs = [
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
    reprotoPropagatedDeps
    wktRkyvDeps
    reprotoTestDeps
    reproto
    reprotoTests
    fdpScanTests
    prototextCodecTests
    pythonLint
    pythonRuff
    googleapisPbs
    googleapisDb
    googleapisTests
    bobappDesc
    bobappExtraDesc
    customDb
    customTests;
}
