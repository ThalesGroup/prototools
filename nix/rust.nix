# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

# nix/rust.nix — Crane derivations: dep cache, fmt, clippy, tests, prototext
#                binary, and the three PyO3 extensions via makePyo3Extension.
#
# Pipeline diagram:
#
#   src (Rust sources, fixtures/)
#     │
#     ├──[buildDepsOnly]──▶  depsCache  ──────────────────┐
#     │                                                    │
#     ├──[cargoFmt]──▶  rustFmt                            │
#     │                                                    │
#     ├──[cargoClippy, cargoArtifacts=depsCache]──▶  rustClippy
#     │                                                    │
#     ├──[cargoTest, cargoArtifacts=depsCache]──▶  rustTests
#     │                                                    │
#     ├──[buildPackage, cargoArtifacts=rustTests]──▶  prototext
#     │                                                    │
#     ├──[makePyo3Extension, cargoArtifacts=depsCache]──▶  prototextCodec
#     ├──[makePyo3Extension, cargoArtifacts=depsCache]──▶  fdpScanLib
#     └──[makePyo3Extension, cargoArtifacts=depsCache]──▶  scoringGraphLib

{ pkgs
, crane
, pythonPkgs
, pythonBin
, pythonExecutable
, pyo3Rustflags
, src
, protoPatchPhase
}:

let

  # ---------------------------------------------------------------------------
  # Shared flag strings — single source of truth for repeated cargo arguments.
  # ---------------------------------------------------------------------------

  # Cargo flags for workspace-wide derivations (fmt, clippy, tests).
  # pyo3 crates are included: PYO3_PYTHON is set in commonArgs so every
  # sandbox can compile prototext_codec without a separate dep cache.
  workspaceArgs = "--no-default-features --workspace";

  # ---------------------------------------------------------------------------
  # Base argument sets — hierarchic composition.
  #
  # commonArgs: base for ALL Crane derivations (Rust + pyo3).
  #   Carries PYO3_PYTHON and RUSTFLAGS globally so that a single depsCache
  #   covers the whole workspace including prototext_codec.
  #
  # protocArgs: extends commonArgs for derivations that invoke protoc.
  #
  # Crane builds in release mode by default via configureCargoCommonVarsHook.
  # All cargo build invocations in the shellHook must pass --release explicitly.
  # ---------------------------------------------------------------------------
  commonArgs = {
    inherit src;
    pname             = "prototools";
    version           = "0.1.4";
    strictDeps        = true;
    nativeBuildInputs = [ pkgs.cargo pkgs.rustc pythonBin ];
    env.PYO3_PYTHON   = pythonExecutable;
    RUSTFLAGS         = pyo3Rustflags;
  };

  protocArgs = commonArgs // {
    nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ pkgs.protobuf ];
    patchPhase        = protoPatchPhase;
  };

  # ---------------------------------------------------------------------------
  # Shared dependency cache — built once, reused by all Crane derivations.
  # Covers the whole workspace including prototext_codec: PYO3_PYTHON and
  # RUSTFLAGS are present in commonArgs so the pyo3 build script succeeds.
  # buildDepsOnly stubs build.rs with a dummy source, so no patchPhase needed.
  # ---------------------------------------------------------------------------
  depsCache = crane.buildDepsOnly (commonArgs // {
    pname          = "prototools-deps";
    cargoExtraArgs = workspaceArgs;
  });

  # ---------------------------------------------------------------------------
  # Lint checks — separate derivations for Nix-level caching and parallelism.
  #
  # cargoFmt needs no compiled artifacts.
  # cargoClippy reuses depsCache so only the thin analysis layer is added.
  # A single workspace-wide clippy replaces the former three derivations
  # (rustClippy, rustClippyPyo3, rustClippyScoringGraph).
  # ---------------------------------------------------------------------------
  rustFmt = crane.cargoFmt (commonArgs // {
    pname = "prototools-fmt";
  });

  rustClippy = crane.cargoClippy (commonArgs // {
    pname                = "prototools-clippy";
    cargoArtifacts       = depsCache;
    cargoExtraArgs       = workspaceArgs;
    cargoClippyExtraArgs = "-- -D warnings";
  });

  # ---------------------------------------------------------------------------
  # Tests — workspace-wide, reusing depsCache.
  # ---------------------------------------------------------------------------
  rustTests = crane.cargoTest (protocArgs // {
    pname          = "prototools-tests";
    cargoArtifacts = depsCache;
    cargoExtraArgs = workspaceArgs;
  });

  # ---------------------------------------------------------------------------
  # Final package — builds only the prototext binary.
  # checkPhase asserts that fmt, clippy, and tests all passed by referencing
  # their store paths (Nix fails the build if any derivation is missing).
  # ---------------------------------------------------------------------------
  prototext = crane.buildPackage (protocArgs // {
    pname          = "prototext";
    cargoArtifacts = rustTests;
    cargoExtraArgs = "--no-default-features -p prototext";
    nativeBuildInputs = protocArgs.nativeBuildInputs ++ [ pkgs.installShellFiles ];
    # doCheck = false: tests are already run by the dedicated rust-tests
    # derivation (which has protoc in nativeBuildInputs).
    doCheck        = false;

    postInstall = ''
      # Install shell completions.
      installShellCompletion --cmd prototext \
        --bash <(PROTOTEXT_COMPLETE=bash $out/bin/prototext | sed \
          -e 's|-o nospace -o bashdefault|-o nospace -o filenames -o bashdefault|g' \
          -e 's|words\[COMP_CWORD\]="$2"|local _cur="''${COMP_LINE:0:''${COMP_POINT}}"; _cur="''${_cur##* }"; words[COMP_CWORD]="''${_cur}"|') \
        --zsh  <(PROTOTEXT_COMPLETE=zsh  $out/bin/prototext) \
        --fish <(PROTOTEXT_COMPLETE=fish $out/bin/prototext)

      # Generate and install man page.
      $out/bin/prototext-gen-man $out/share/man/man1
    '';

    meta = with pkgs.lib; {
      description  = "Command-line tool for Protocol Buffer messages (prototext binary)";
      longDescription = ''
        prototools is a collection of CLI utilities for working with Protocol
        Buffer messages.  The first tool, prototext, converts between binary
        protobuf wire format and protoc-style enhanced textproto, with lossless
        round-trip by default.
      '';
      homepage    = "https://github.com/douzebis/prototools";
      license     = licenses.mit;
      maintainers = with maintainers; [ ];  # add: douzebis once registered
      mainProgram = "prototext";
      platforms   = platforms.unix;
    };
  });

  # ---------------------------------------------------------------------------
  # makePyo3Extension — shared helper for the three PyO3 extensions.
  #
  # Each PyO3 extension follows the same pattern:
  #   1. Crane buildPackage compiles --lib and --bin <crate>_post_build in one
  #      invocation (avoids a second full compilation — see S9 in spec 0066).
  #   2. postBuild runs the already-compiled stub generator directly with
  #      CARGO_MANIFEST_DIR set, writing <libName>.pyi into the crate dir.
  #      (The NotPresent panic in spec 0038 was caused by missing
  #      CARGO_MANIFEST_DIR, not by dynamic linking; pyo3-stub-gen uses
  #      the inventory crate's static constructors, not dynamic .so loading.)
  #   3. installPhase copies the .so and .pyi into $out/artifacts/.
  #   4. A buildPythonPackage wrapper copies the artifacts into the
  #      pyproject source tree and installs the wheel.
  #
  # Parameters:
  #   crateName    — Cargo package name, e.g. "prototext_codec"
  #   crateDir     — source-tree directory name, e.g. "prototext-pyo3"
  #                  (used as CARGO_MANIFEST_DIR)
  #   pyDir        — Nix path to Python package source, e.g. ./prototext-pyo3
  #   libName      — cdylib base name (from Cargo [[lib]] name), e.g.
  #                  "prototext_codec_lib" (produces lib<libName>.{so,dylib})
  #   pyiName      — name used by pyo3-stub-gen for the .pyi file (= pyproject
  #                  [project] name), e.g. "prototext_codec"
  #   postBuildBin — name of the stub-generator binary target, e.g.
  #                  "prototext_post_build" (may differ from crateName)
  #
  # Returns: an attrset { pkg, artifacts } where:
  #   pkg       — the installable buildPythonPackage derivation
  #   artifacts — store path to the $out/artifacts directory of the Crane build
  #               (exposes the .so and .pyi for use by pyright/pythonLint)
  # ---------------------------------------------------------------------------
  makePyo3Extension = { crateName, crateDir, pyDir, libName, pyiName, postBuildBin }:
    let
      # libExt is resolved at Nix eval time to a literal string "so" or
      # "dylib" — it looks like a bash assignment but is a Nix interpolation.
      libExt = if pkgs.stdenv.isDarwin then "dylib" else "so";
      ext = crane.buildPackage (commonArgs // {
        pname          = "${crateName}-extension";
        cargoExtraArgs = "-p ${crateName} --lib";
        doCheck        = false;
        cargoArtifacts = depsCache;
        # Build both the cdylib and the stub-generator binary in one invocation
        # to avoid a redundant recompile of the crate (S9).
        # The custom buildPhaseCargoCommand replaces Crane's default; we must
        # set doNotPostBuildInstallCargoBinaries so Crane's
        # installFromCargoBuildLogHook does not fail looking for the build log.
        buildPhaseCargoCommand = "cargo build --release -p ${crateName} --lib --bin ${postBuildBin}";
        doNotPostBuildInstallCargoBinaries = true;
        # Clear stale fingerprints so the pyo3 build script re-runs here.
        preBuild = "rm -f target/release/.fingerprint/${crateName}-*/invoked.timestamp";
        installPhase = ''
          # Run the stub generator with CARGO_MANIFEST_DIR set so pyo3-stub-gen
          # can locate pyproject.toml and write the .pyi stub next to it.
          # pyo3-stub-gen names the .pyi file after the pyproject [project] name
          # (pyiName), not after the Rust cdylib name (libName).
          # (The NotPresent panic in spec 0038 was caused by CARGO_MANIFEST_DIR
          # being absent, not by a dynamic linking issue.)
          CARGO_MANIFEST_DIR="$PWD/${crateDir}" ./target/release/${postBuildBin}

          mkdir -p $out/artifacts
          # Rename lib<libName>.{so,dylib} → <libName>.so (drops the lib prefix)
          # so the Python import `from .<libName> import ...` resolves correctly.
          cp target/release/lib${libName}.${libExt} $out/artifacts/${libName}.so
          # Rename <pyiName>.pyi → <libName>.pyi to match Python import name.
          cp ${crateDir}/${pyiName}.pyi $out/artifacts/${libName}.pyi
        '';
      });
      pkg = pythonPkgs.buildPythonPackage {
        pname     = crateName;
        version   = "0.1.0";
        format    = "pyproject";
        src       = pyDir;
        buildInputs = [ pythonPkgs.hatchling ext ];
        # Copy .so and .pyi into the package subdirectory (<libName>/) where
        # hatchling expects them (alongside __init__.py).
        patchPhase = ''
          cp ${ext}/artifacts/${libName}.* ${libName}/
        '';
      };
    in { inherit pkg; artifacts = "${ext}/artifacts"; };

  # ---------------------------------------------------------------------------
  # PyO3 extensions — prototext_codec, fdp_scan, scoring_graph
  # ---------------------------------------------------------------------------

  _prototextCodecExt = makePyo3Extension {
    crateName    = "prototext_codec";
    crateDir     = "prototext-pyo3";
    pyDir        = ../prototext-pyo3;
    libName      = "prototext_codec_lib";
    pyiName      = "prototext_codec";
    postBuildBin = "prototext_post_build";
  };

  _fdpScanLibExt = makePyo3Extension {
    crateName    = "fdp_scan_extension";
    crateDir     = "fdp-scan-pyo3";
    pyDir        = ../fdp-scan-pyo3;
    libName      = "fdp_scan_lib";
    pyiName      = "fdp_scan";
    postBuildBin = "fdp_scan_post_build";
  };

  _scoringGraphLibExt = makePyo3Extension {
    crateName    = "scoring_graph_extension";
    crateDir     = "score-graph-pyo3";
    pyDir        = ../score-graph-pyo3;
    libName      = "scoring_graph_lib";
    pyiName      = "scoring_graph";
    postBuildBin = "scoring_graph_post_build";
  };

  prototextCodec    = _prototextCodecExt.pkg;
  fdpScanLib        = _fdpScanLibExt.pkg;
  scoringGraphLib   = _scoringGraphLibExt.pkg;

in {
  inherit
    commonArgs
    depsCache
    rustFmt
    rustClippy
    rustTests
    prototext
    prototextCodec
    fdpScanLib
    scoringGraphLib;
  prototextExtensionArtifacts   = _prototextCodecExt.artifacts;
  scoringGraphExtensionArtifacts = _scoringGraphLibExt.artifacts;
}
