# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

# nix/python.nix — the Python side: the grammar's Python module, the
#                  extensions' Python tests, the full tests (googleapis and
#                  custom databases) and the descriptor sets bobapp embeds.
#                  reproto, built here until 0.4.0, is retired (spec 0487).

{ pkgs
, protoc            # nix/protobuf.nix: the one protobuf release (spec 0460)
, metaCommon        # nix/rust.nix: homepage, license, maintainers, platforms
, pyprojectVersion  # nix/rust.nix: a pyproject.toml's version, checked (spec 0405 S5)
, pythonPkgs
, pythonBin
, prototext
, prototextCodec
, fdpScanLib
, prototextGraphLib
, protoback         # nix/rust.nix: what the full tests' databases are built with
}:

let
  # Sources hold tracked files only, in a git checkout (spec 0479).
  trackedSource = (import ./tracked.nix { inherit (pkgs) lib; }).toSource;

  # The textproto grammar's Python module (spec 0408 S4): a setuptools
  # package with one C extension, built from the committed parser
  # (src/parser.c) and binding.c with a C compiler alone. Its module is
  # tree_sitter_textproto, as tree-sitter names grammar bindings, and its
  # wheel goes to PyPI with the others (spec 0411).
  treeSitterTextprotoPkg = pythonPkgs.buildPythonPackage {
    pname   = "tree-sitter-textproto";
    version = pyprojectVersion "tree-sitter-textproto";
    meta = metaCommon // {
      description = "The textproto tree-sitter grammar, for the Python tree_sitter library";
    };
    src = trackedSource {
      root    = ../tree-sitter-textproto;
      fileset = pkgs.lib.fileset.unions [
        ../tree-sitter-textproto/pyproject.toml
        ../tree-sitter-textproto/setup.py
        ../tree-sitter-textproto/binding.c
        ../tree-sitter-textproto/src
        ../tree-sitter-textproto/tree_sitter_textproto
        ../tree-sitter-textproto/LICENSE
      ];
    };
    pyproject = true;
    build-system = [ pythonPkgs.setuptools ];
    pythonImportsCheck = [ "tree_sitter_textproto" ];
  };

  # The full tests' Python: pytest, and yaml for their type lists; they run
  # prototext and protoback as programs. The Nix pytest runs are serial,
  # one process (spec 0485 S4): their work is a session fixture, which
  # each xdist worker would run again.
  fullTestDeps = [
    pythonPkgs.pytest
    pythonPkgs."pytest-xdist"
    pythonPkgs.pyyaml
  ];

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
  # The Python linters' settings, defined once: the CI checks below use
  # them as they are, and the dev shell's hook writes them for the editor
  # and the command line (nix/shells.nix), adding pyright's extraPaths from
  # $PYTHONPATH. pyright reads comments in its configuration, and warns on
  # an unknown key: the header is a comment.
  # ---------------------------------------------------------------------------
  lintSettings = rec {
    header = "Generated from lintSettings in nix/python.nix: do not edit by hand.";
    pyright = {
      pythonVersion = "3.13";
      typeCheckingMode = "standard";
      exclude = [
        "result*"
        "prototext-pyo3/prototext_codec_lib"
        "fdp-scan-pyo3/fdp_scan_lib"
        "prototext-graph-pyo3/prototext_graph_lib"
        # The grammar's Python module: a C extension known to pyright only
        # by its stub, like the pyo3 packages above (spec 0408 S4).
        "tree-sitter-textproto"
        "docs/mockup"
      ];
    };
    ruffExclude = [ "docs/mockup" ];
    # pyright's settings but extraPaths, which the shell's hook adds.
    pyrightBase = pkgs.writeText "pyright-settings.json" (builtins.toJSON pyright);
    pyrightConfig = extraPaths: pkgs.writeText "pyrightconfig.json"
      ("// ${header}\n" + builtins.toJSON (pyright // { inherit extraPaths; }) + "\n");
    ruffToml = pkgs.writeText "ruff.toml" (
      "# ${header}\nexclude = [\n"
      + pkgs.lib.concatMapStrings (e: "  ${builtins.toJSON e},\n") ruffExclude
      + "]\n");
    ruffExcludeFlags = pkgs.lib.concatMapStringsSep " "
      (e: "--exclude ${pkgs.lib.escapeShellArg e}") ruffExclude;
  };

  # ---------------------------------------------------------------------------
  # Python lint: pyright and ruff on the repository's Python, which reproto
  # was most of until it was retired (spec 0487): the extensions' and the
  # full tests, protoback's plugin adapter, the scripts. attic/ and docs/
  # are left out, and the extensions' own packages are pyright-excluded
  # (lintSettings): their stubs come with the installed extensions.
  # ---------------------------------------------------------------------------
  pythonSrc =
    let fs = pkgs.lib.fileset; in trackedSource {
      root = ../.;
      fileset = fs.difference
        (fs.fileFilter (f: f.hasExt "py") ../.)
        (fs.unions [ ../attic ../docs ]);
    };

  # What that Python imports, and pyright's stubs for protobuf; the dev
  # shell's PYTHONPATH too (nix/shells.nix).
  lintDeps = fullTestDeps ++ [
    prototextCodec
    fdpScanLib
    pythonPkgs.protobuf
    pythonPkgs.types-protobuf
  ];

  pythonLint = pkgs.runCommand "python-lint" {
    strictDeps = true;
    nativeBuildInputs = [
      pkgs.pyright
      (pythonPkgs.python.withPackages (_: lintDeps))
    ];
  } ''
    set -euo pipefail
    # pyright needs a writable working directory for its cache.
    cp -r ${pythonSrc} src && chmod -R u+w src && cd src
    cp ${lintSettings.pyrightConfig [ ]} pyrightconfig.json
    echo "--- pyright ---"
    pyright
    touch $out
  '';

  pythonRuff = pkgs.runCommand "python-ruff" {
    strictDeps = true;
    nativeBuildInputs = [ pythonPkgs.ruff ];
  } ''
    set -euo pipefail
    echo "--- ruff ---"
    ruff check --no-cache ${lintSettings.ruffExcludeFlags} ${pythonSrc}
    touch $out
  '';

  # ---------------------------------------------------------------------------
  # googleapis DB + tests — separate from ci; triggered by
  # nix-build -A full-tests only.
  #
  # Two derivations:
  #   googleapisDb    — fetches pinned googleapis corpus, compiles protos,
  #                     runs protoback --schema-db-out, and instantiates
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
  # that changes to protoback/instantiation logic don't force a recompile.
  googleapisPbs = pkgs.runCommand "googleapis-pbs" {
    strictDeps = true;
    nativeBuildInputs = [ protoc ];
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
    nativeBuildInputs = [ protoc ];
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
  # not repeated when protoback or instantiation logic changes.
  googleapisDb = pkgs.runCommand "googleapis-db" {
    strictDeps = true;
    nativeBuildInputs = [
      prototext
      protoback
      pythonPkgs.python
    ];
  } ''
    set -euo pipefail

    # ── Build the schema DB ───────────────────────────────────────────────────
    # -O must be the schema-db stub's own `proto` child: that is the one
    # location protolens looks in when no -I/--proto-root is given (spec
    # 0155 G2), so `protolens <db>/googleapis.desc` gets working
    # jump-to-definition with no flag. It is also the only place inside
    # the stub directory protoback permits (spec 0155 G1), the rest of it
    # being hopcroft.rkyv/index.rkyv.
    #
    mkdir -p "$out"
    protoback \
      --builtin all \
      --force-proto2-for-editions \
      --proto-out="$out/googleapis/proto" \
      --emit-scoring-yaml \
      --emit-binary \
      --schema-db-out="$out/googleapis.desc" \
      "${googleapisPbs}/googleapis.pb"

    # ── Instantiate one .pb per sampled type ──────────────────────────────────
    # Number of types to instantiate (includes potential 0-byte skips).
    N_INSTANCES=400
    TYPES_YAML=${trackedSource { root = ../tests/stress; fileset = ../tests/stress/googleapis-types.yaml; }}/googleapis-types.yaml
    FIXTURES_DIR=${trackedSource { root = ../tests/fixtures/instances; fileset = ../tests/fixtures/instances; }}

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
    protoback instantiate \
      --descriptor-set "$out/googleapis.desc" \
      -O "$out/instances" \
      $FQDNS
  '';

  googleapisTests = pkgs.runCommand "googleapis-tests" {
    strictDeps = true;
    nativeBuildInputs = [
      prototext
      protoback
      (pythonPkgs.python.withPackages (_: fullTestDeps))
    ];
  } ''
    set -euo pipefail
    export STRESS_DB="${googleapisDb}/googleapis.desc"
    pytest -p no:cacheprovider ${../tests/stress}/
    touch $out
  '';

  # ---------------------------------------------------------------------------
  # Custom DB + tests — fixture types and opentelemetry-proto.
  #
  # Two derivations:
  #   customDb    — compiles the custom fixture protos + opentelemetry-proto corpus,
  #                 runs protoback --schema-db-out.
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
      protoc
      protoback
    ];
  } ''
    set -euo pipefail

    FIXTURES="${trackedSource { root = ../protoback/tests/fixtures; fileset = ../protoback/tests/fixtures; }}"
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
    protoback \
      --builtin all \
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
      protoback
      (pythonPkgs.python.withPackages (_: fullTestDeps))
    ];
  } ''
    set -euo pipefail
    export CUSTOM_DB="${customDb}/custom.desc"
    pytest -p no:cacheprovider ${../tests/custom}/
    touch $out
  '';

in {
  inherit
    treeSitterTextprotoPkg
    fullTestDeps
    lintDeps
    fdpScanTests
    prototextCodecTests
    pythonLint
    pythonRuff
    lintSettings
    googleapisPbs
    googleapisDb
    googleapisTests
    bobappDesc
    bobappExtraDesc
    customDb
    customTests;
}
