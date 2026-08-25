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
#     ├──[buildDepsOnly]──▶  depsCache  ──────────────────────────────────┐
#     │                                                                    │
#     ├──[cargoFmt]──▶  rustFmt                                            │
#     │                                                                    │
#     ├──[cargoClippy, cargoArtifacts=depsCache]──▶  rustClippy            │
#     │                                                                    │
#     ├──[cargoTest, cargoArtifacts=depsCache]──▶  rustTests               │
#     │                                                                    │
#     ├──[buildPackage, cargoArtifacts=depsCache]──▶  prototextBare ──▶  prototext
#     │                                                                    │
#     ├──[makePyo3Extension, cargoArtifacts=rustTests]──▶  prototextCodec    │
#     ├──[makePyo3Extension, cargoArtifacts=rustTests]──▶  fdpScanLib         │
#     └──[makePyo3Extension, cargoArtifacts=rustTests]──▶  prototextGraphLib

{ pkgs
, crane
, pythonPkgs
, pythonBin
, pythonExecutable
, pyo3Rustflags
, depsSrc
, workspaceSrc
, protoPatchPhase
, wktRkyv ? null   # store path to wkt.rkyv; null for bare/bootstrap builds
, treeSitterTextprotoRustLib   # static lib + queries/highlights.scm for protolens's build.rs
, buf               # narrow-pinned buf (newer than the main nixpkgs pin's 1.59.0; see default.nix)
}:

let

  # ---------------------------------------------------------------------------
  # Shared flag strings — single source of truth for repeated cargo arguments.
  # ---------------------------------------------------------------------------

  # Cargo flags for workspace-wide derivations (fmt, clippy, tests).
  # pyo3 crates are included: PYO3_PYTHON is set in commonArgs so every
  # sandbox can compile prototext_codec_lib without a separate dep cache.
  workspaceArgs = "--no-default-features --workspace";

  # Same, plus prototext's `prebuilt-wkt`, for every workspace-wide build that
  # runs at or before the bootstrap.
  #
  # fdp_scan_lib embeds prototext's WKT scoring graph (spec 0239 S1), so it
  # depends on prototext with default features, and Cargo unifies `wkt-db`
  # on for the whole `--workspace` build. prototext's build.rs then wants a
  # graph, and without WKT_RKYV it tries to generate one by running reproto.
  #
  # rustTests cannot have that graph, because it is upstream of it:
  #
  #   wktRkyv → reprotoSrcFull → reprotoBare → prototextGraphLib → rustTests
  #
  # (reproto --schema-db-out imports prototext_graph_lib to write hopcroft
  # .rkyv; see nix/python.nix.) Handing rustTests a WKT_RKYV would close that
  # loop. prototextBare, rustClippy and protolens are not in the loop, but
  # pointing them at wktRkyv would serialize behind the bootstrap what runs
  # in parallel with it today.
  #
  # `prebuilt-wkt` sidesteps both with the copy committed to git under
  # prototext/wkt/prebuilt/, which is what nixpkgs already builds against.
  # None of these four ships a graph — they consume it as a compile-time
  # blob and throw it away. The one stage that ships one is _fdpScanLibExt,
  # and that one gets the freshly generated graph.
  bootstrapArgs = "${workspaceArgs} --features prebuilt-wkt";

  # ---------------------------------------------------------------------------
  # Base argument sets — hierarchic composition.
  #
  # commonArgs: base for ALL Crane derivations (Rust + pyo3).
  #   Carries PYO3_PYTHON and RUSTFLAGS globally so that a single depsCache
  #   covers the whole workspace including prototext_codec_lib.
  #
  # protocArgs: extends commonArgs with pkgs.protobuf + protoPatchPhase.
  #   Used by ALL derivations including depsCache, so the sandbox environment
  #   is identical everywhere and Cargo fingerprints are stable across the chain.
  #   (buildDepsOnly stubs build.rs so protoc is never actually invoked there.)
  #
  # Crane builds in release mode by default via configureCargoCommonVarsHook.
  # All cargo build invocations in the shellHook must pass --release explicitly.
  # ---------------------------------------------------------------------------
  # commonArgs omits src — each derivation sets its own focused src.
  commonArgs = {
    pname             = "prototools";
    version           = "0.1.4";
    strictDeps        = true;
    nativeBuildInputs = [ pkgs.cargo pkgs.rustc pythonBin ];
    env.PYO3_PYTHON   = pythonExecutable;
    env.TREE_SITTER_TEXTPROTO_LIB_DIR     = "${treeSitterTextprotoRustLib}/lib";
    env.TREE_SITTER_TEXTPROTO_QUERIES_DIR = "${treeSitterTextprotoRustLib}/queries";
    RUSTFLAGS         = pyo3Rustflags;
  };

  protocArgs = commonArgs // {
    nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ pkgs.protobuf ];
    patchPhase        = protoPatchPhase;
  };

  # ---------------------------------------------------------------------------
  # Shared dependency cache — built once, reused by all Crane derivations.
  # Uses protocArgs (includes pkgs.protobuf) so the sandbox environment matches
  # all consumers. buildDepsOnly stubs build.rs so protoc is never invoked, but
  # having protobuf present prevents fingerprint mismatches that would force
  # external deps to recompile in every downstream derivation.
  # No patchPhase needed: dummy build.rs never calls protoc.
  # ---------------------------------------------------------------------------
  depsCache = crane.buildDepsOnly (protocArgs // {
    src            = workspaceSrc;
    pname          = "prototools-deps";
    cargoExtraArgs = workspaceArgs;
    # patchPhase must not run: buildDepsOnly uses dummy sources so proto
    # fixtures are absent and protoc would fail. Override it away.
    patchPhase     = "runHook prePatch; runHook postPatch";
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
    src   = workspaceSrc;
    pname = "prototools-fmt";
  });

  rustClippy = crane.cargoClippy (protocArgs // {
    src                  = workspaceSrc;
    pname                = "prototools-clippy";
    cargoArtifacts       = depsCache;
    cargoExtraArgs       = bootstrapArgs;
    cargoClippyExtraArgs = "-- -D warnings";
  });

  # ---------------------------------------------------------------------------
  # Tests — workspace-wide, reusing depsCache.
  # ---------------------------------------------------------------------------
  rustTests = crane.cargoTest (protocArgs // {
    src            = workspaceSrc;
    pname          = "prototools-tests";
    cargoArtifacts = depsCache;
    cargoExtraArgs = bootstrapArgs;
    # Tell supports_rgb() that RGB is available so color-sensitive tests
    # exercise the RGB code path in the sandbox (no real terminal there).
    COLORTERM      = "truecolor";
  });

  # Common postInstall for both prototext variants.
  prototextPostInstall = ''
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

  prototextMeta = with pkgs.lib; {
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

  # ---------------------------------------------------------------------------
  # prototextBare — prototext without the embedded WKT scoring graph.
  # Used by reproto as a build-time dependency, breaking the circular dep:
  #   prototext-full → wktRkyv → reproto → prototextBare
  #
  # Single cargo build --workspace invocation — identical flags to depsCache,
  # so Cargo reuses all external-dep artifacts without recompiling anything.
  # (A scoped -p prototext invocation would compute different profile unit_for
  # hashes for external deps, invalidating depsCache fingerprints and forcing
  # a full external-dep recompile — see constant-rebuilds in Crane FAQ.)
  #
  # We bypass installFromCargoBuildLogHook (doNotPostBuildInstallCargoBinaries)
  # and install prototext binaries directly by name in installPhaseCommand,
  # the same pattern used by the PyO3 extensions.
  # ---------------------------------------------------------------------------
  prototextBare = crane.buildPackage (protocArgs // {
    src                              = workspaceSrc;
    pname                            = "prototext-bare";
    cargoArtifacts                   = depsCache;
    nativeBuildInputs                = protocArgs.nativeBuildInputs ++ [ pkgs.installShellFiles ];
    doCheck                          = false;
    doInstallCargoArtifacts          = true;
    postInstall                      = prototextPostInstall;
    meta                             = prototextMeta;
    buildPhaseCargoCommand           = "cargoWithProfile build ${bootstrapArgs}";
    doNotPostBuildInstallCargoBinaries = true;
    installPhaseCommand              = ''
      mkdir -p $out/bin
      cp target/release/prototext $out/bin/
      cp target/release/prototext-gen-man $out/bin/
    '';
  });

  # ---------------------------------------------------------------------------
  # prototext (full) — with embedded WKT scoring graph (feature wkt-db).
  # When wktRkyv is non-null (Nix build), WKT_RKYV and WKT_INDEX point at
  # the pre-built files so build.rs skips the reproto invocation.
  # When wktRkyv is null (bare/bootstrap), falls back to prototextBare.
  #
  # Same single-invocation pattern as prototextBare: cargo build --workspace
  # with wkt-db feature enabled reuses all prototextBare artifacts.
  # ---------------------------------------------------------------------------
  prototext =
    if wktRkyv != null then
      crane.buildPackage (protocArgs // {
        src                              = workspaceSrc;
        pname                            = "prototext";
        cargoArtifacts                   = prototextBare;
        nativeBuildInputs                = protocArgs.nativeBuildInputs ++ [ pkgs.installShellFiles ];
        doCheck                          = false;
        WKT_RKYV                         = "${wktRkyv}/wkt.rkyv";
        WKT_INDEX                        = "${wktRkyv}/wkt_index.rkyv";
        postInstall                      = prototextPostInstall;
        meta                             = prototextMeta;
        buildPhaseCargoCommand           = "cargoWithProfile build ${workspaceArgs} --features wkt-db";
        doNotPostBuildInstallCargoBinaries = true;
        installPhaseCommand              = ''
          mkdir -p $out/bin
          cp target/release/prototext $out/bin/
          cp target/release/prototext-gen-man $out/bin/
        '';
      })
    else
      prototextBare;

  # ---------------------------------------------------------------------------
  # protolens — interactive TUI to decode/navigate/extract a binary protobuf.
  # No wkt-db/prebuilt-wkt split needed (unlike prototext): protolens always
  # requires an explicit --descriptor-set (spec 0111 v1, no embedded-WKT
  # fallback), so there is only one build variant. protolens depends only on
  # prototext-core (no build.rs of its own) and prototext-graph, but the
  # single-Cargo-workspace `cargoWithProfile build ${workspaceArgs}` builds
  # the whole workspace regardless (see prototextBare's own comment above on
  # why a scoped -p invocation would break the depsCache fingerprint), so
  # protocArgs/protoPatchPhase (needed by prototext's own build.rs) are still
  # required here too.
  # ---------------------------------------------------------------------------
  protolensPostInstall = ''
    installShellCompletion --cmd protolens \
      --bash <(PROTOLENS_COMPLETE=bash $out/bin/protolens | sed \
        -e 's|-o nospace -o bashdefault|-o nospace -o filenames -o bashdefault|g' \
        -e 's|words\[COMP_CWORD\]="$2"|local _cur="''${COMP_LINE:0:''${COMP_POINT}}"; _cur="''${_cur##* }"; words[COMP_CWORD]="''${_cur}"|') \
      --zsh  <(PROTOLENS_COMPLETE=zsh  $out/bin/protolens) \
      --fish <(PROTOLENS_COMPLETE=fish $out/bin/protolens)

    # Generate and install man page (spec 0228 S11). Before wrapProgram,
    # so the generator runs the real binary rather than the wrapper.
    PROTOLENS_GEN_MAN=$out/share/man/man1 $out/bin/protolens

    # `v`'s Neovim handoff (spec 0144 G5/G6) is a mandatory runtime
    # dependency, not merely a dev-shell convenience — bundle a pinned
    # Neovim and `buf` (for `buf lsp serve`) onto PATH so they resolve
    # regardless of the user's own $PATH.
    # spec 0145 G5: bundle a minimal Neovim config wiring `.proto`
    # filetype/syntax and `buf lsp serve` navigation, loaded via `-u`.
    install -Dm444 ${../protolens/nvim/init.lua} \
      "$out/share/protolens/nvim/init.lua"
    wrapProgram $out/bin/protolens \
      --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.neovim buf ]} \
      --set PROTOLENS_NVIM_CONFIG "$out/share/protolens/nvim/init.lua"
  '';

  protolensMeta = with pkgs.lib; {
    description = "Interactive TUI to decode, navigate, and extract raw bytes from a binary protobuf";
    homepage    = "https://github.com/douzebis/prototools";
    license     = licenses.mit;
    maintainers = with maintainers; [ ];  # add: douzebis once registered
    mainProgram = "protolens";
    platforms   = platforms.unix;
  };

  protolens = crane.buildPackage (protocArgs // {
    src                                = workspaceSrc;
    pname                              = "protolens";
    cargoArtifacts                     = depsCache;
    nativeBuildInputs                  = protocArgs.nativeBuildInputs ++ [ pkgs.installShellFiles pkgs.makeWrapper ];
    doCheck                            = false;
    doInstallCargoArtifacts            = true;
    postInstall                        = protolensPostInstall;
    meta                               = protolensMeta;
    buildPhaseCargoCommand             = "cargoWithProfile build ${bootstrapArgs}";
    doNotPostBuildInstallCargoBinaries = true;
    installPhaseCommand                = ''
      mkdir -p $out/bin
      cp target/release/protolens $out/bin/
    '';
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
  #   crateName    — Cargo package name, e.g. "prototext_codec_lib"
  #   crateDir     — Nix path to the crate directory, e.g. ./prototext-pyo3
  #                  (base name used as CARGO_MANIFEST_DIR)
  #   pyDir        — Nix path to Python package source, e.g. ./prototext-pyo3
  #   libName      — cdylib base name (from Cargo [[lib]] name), e.g.
  #                  "prototext_codec_lib" (produces lib<libName>.{so,dylib})
  #   pyiName      — name used by pyo3-stub-gen for the .pyi file (= pyproject
  #                  [project] name), e.g. "prototext_codec" (from pyproject.toml)
  #   postBuildBin — name of the stub-generator binary target, e.g.
  #                  "prototext_post_build" (may differ from crateName)
  #   extraAttrs   — merged into the Crane derivation; used to hand
  #                  fdp_scan_lib the WKT graph it embeds (see _fdpScanLibExt)
  #
  # Returns: an attrset { pkg, artifacts } where:
  #   pkg       — the installable buildPythonPackage derivation
  #   artifacts — store path to the $out/artifacts directory of the Crane build
  #               (exposes the .so and .pyi for use by pyright/pythonLint)
  # ---------------------------------------------------------------------------
  makePyo3Extension = { crateName, crateDir, pyDir, libName, pyiName, postBuildBin
                      , extraAttrs ? {} }:
    let
      # libExt is resolved at Nix eval time to a literal string "so" or
      # "dylib" — it looks like a bash assignment but is a Nix interpolation.
      libExt = if pkgs.stdenv.isDarwin then "dylib" else "so";
      # crateDirName is the bare directory name used in shell commands.
      crateDirName = baseNameOf (toString crateDir);
      ext = crane.buildPackage (protocArgs // {
        src            = workspaceSrc;
        pname          = "${crateName}-extension";
        cargoExtraArgs = "-p ${crateName} --lib";
        doCheck        = false;
        cargoArtifacts = rustTests;
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
          CARGO_MANIFEST_DIR="$PWD/${crateDirName}" ./target/release/${postBuildBin}

          mkdir -p $out/artifacts
          # Rename lib<libName>.{so,dylib} → <libName>.so (drops the lib prefix)
          # so the Python import `from .<libName> import ...` resolves correctly.
          cp target/release/lib${libName}.${libExt} $out/artifacts/${libName}.so
          # Rename <pyiName>.pyi → <libName>.pyi to match Python import name.
          cp ${crateDirName}/${pyiName}.pyi $out/artifacts/${libName}.pyi
        '';
      } // extraAttrs);
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
  # PyO3 extensions — prototext_codec_lib, fdp_scan_lib, prototext_graph_lib
  # ---------------------------------------------------------------------------

  _prototextCodecExt = makePyo3Extension {
    crateName    = "prototext_codec_lib";
    crateDir     = ../prototext-pyo3;
    pyDir        = ../prototext-pyo3;
    libName      = "prototext_codec_lib";
    pyiName      = "prototext_codec";
    postBuildBin = "prototext_post_build";
  };

  _fdpScanLibExt = makePyo3Extension {
    crateName    = "fdp_scan_lib";
    crateDir     = ../fdp-scan-pyo3;
    pyDir        = ../fdp-scan-pyo3;
    libName      = "fdp_scan_lib";
    pyiName      = "fdp_scan";
    postBuildBin = "fdp_scan_post_build";
    # This is the one stage that *ships* a WKT graph: fdp_scan_lib embeds
    # prototext's for protoscan's Policy::Scan walk (spec 0239 S1). It gets
    # the freshly generated one, not the git-committed prebuilt the
    # bootstrap builds against, so that a wkt/SOURCES edit cannot leave
    # protoscan scoring against a stale schema.
    #
    # Deliberate cost: this stage builds with prototext's default features
    # (wkt-db, no prebuilt-wkt), which does not match bootstrapArgs, so
    # prototext recompiles here — measured ~29 s wall, in a leaf derivation
    # Nix runs in parallel with the other two extensions.
    extraAttrs = {
      WKT_RKYV  = "${wktRkyv}/wkt.rkyv";
      WKT_INDEX = "${wktRkyv}/wkt_index.rkyv";
    };
  };

  _prototextGraphLibExt = makePyo3Extension {
    crateName    = "prototext_graph_lib";
    crateDir     = ../prototext-graph-pyo3;
    pyDir        = ../prototext-graph-pyo3;
    libName      = "prototext_graph_lib";
    pyiName      = "prototext_graph";
    postBuildBin = "prototext_graph_post_build";
  };

  prototextCodec     = _prototextCodecExt.pkg;
  fdpScanLib         = _fdpScanLibExt.pkg;
  prototextGraphLib  = _prototextGraphLibExt.pkg;

in {
  inherit
    commonArgs
    depsCache
    rustFmt
    rustClippy
    rustTests
    prototextBare
    prototext
    protolens
    prototextCodec
    fdpScanLib
    prototextGraphLib;
  prototextExtensionArtifacts      = _prototextCodecExt.artifacts;
  fdpScanExtensionArtifacts        = _fdpScanLibExt.artifacts;
  prototextGraphExtensionArtifacts = _prototextGraphLibExt.artifacts;
}
