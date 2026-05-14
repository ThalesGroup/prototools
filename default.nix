# SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
# SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

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

  # ---------------------------------------------------------------------------
  # Shared flag strings — single source of truth for repeated cargo arguments.
  # ---------------------------------------------------------------------------

  # Cargo flags for workspace-wide derivations (fmt, clippy, tests).
  # pyo3 crates are included: PYO3_PYTHON is set in commonArgs so every
  # sandbox can compile prototext_codec without a separate dep cache.
  workspaceArgs = "--no-default-features --workspace";

  # Cargo package selector used by pyo3-specific derivations (clippy-pyo3,
  # prototextExtension).
  pyo3Args = "-p prototext_codec";

  # Cargo package selector for fdp_scan pyo3 derivations.
  fdpScanArgs = "-p fdp_scan_extension";

  # Cargo package selector for scoring_graph pyo3 derivations.
  scoringGraphArgs = "-p scoring_graph_extension";

  # RUSTFLAGS for linking against CPython.  Set globally in commonArgs so that
  # all Crane derivations carry the same value — keeping Cargo fingerprints
  # consistent across the single shared depsCache.  Also exported in the
  # shellHook so that manual `cargo build -p prototext_codec` aligns.
  pyo3Rustflags = "-L ${pythonBin}/lib -lpython${pythonPkgs.python.pythonVersion}";

  # ---------------------------------------------------------------------------
  # Base argument sets — hierarchic composition.
  #
  # commonArgs: base for ALL Crane derivations (Rust + pyo3).
  #   Carries PYO3_PYTHON and RUSTFLAGS globally so that a single depsCache
  #   covers the whole workspace including prototext_codec.
  #
  # protocArgs: extends commonArgs for derivations that invoke protoc.
  # ---------------------------------------------------------------------------
  commonArgs = {
    inherit src;
    pname             = "prototools";
    version           = "0.1.4";
    strictDeps        = true;
    nativeBuildInputs = [ pkgs.cargo pkgs.rustc pythonBin ];
    env.PYO3_PYTHON   = pythonExecutable;
    RUSTFLAGS         = pyo3Rustflags;
    CARGO_PROFILE     = "release";
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
  # ---------------------------------------------------------------------------
  rustFmt = crane.cargoFmt (commonArgs // {
    pname = "prototools-fmt";
  });

  rustClippy = crane.cargoClippy (protocArgs // {
    pname                = "prototools-clippy";
    cargoArtifacts       = depsCache;
    cargoExtraArgs       = workspaceArgs;
    cargoClippyExtraArgs = "--all-targets -- --deny warnings";
  });

  rustClippyPyo3 = crane.cargoClippy (commonArgs // {
    pname                = "prototools-clippy-pyo3";
    cargoArtifacts       = depsCache;
    cargoExtraArgs       = pyo3Args;
    cargoClippyExtraArgs = "--all-targets -- --deny warnings";
  });

  rustClippyScoringGraph = crane.cargoClippy (commonArgs // {
    pname                = "prototools-clippy-scoring-graph";
    cargoArtifacts       = depsCache;
    cargoExtraArgs       = scoringGraphArgs;
    cargoClippyExtraArgs = "--all-targets -- --deny warnings";
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
  # PyO3 extension — prototext_codec Python package
  # ---------------------------------------------------------------------------

  # Compile the cdylib and the stub-generator binary in a single cargo
  # invocation, then run the already-compiled binary in postBuild to generate
  # the .pyi stub.  This avoids the previous double-compile that occurred when
  # postBuild called `cargo run --bin prototext_post_build` separately.
  #
  # Chains off the shared depsCache (not a separate pyo3DepsCache): commonArgs
  # already carries PYO3_PYTHON and RUSTFLAGS, so the pyo3 build script
  # succeeds and Cargo fingerprints stay aligned with the rest of the workspace.
  #
  # Note: the clippy binary cannot be patched on macos-15-intel due to an
  # install_name_tool path-length limit, so prototextExtension chains off
  # depsCache rather than rustClippyPyo3.  CI enforces pyo3 clippy separately.
  prototextExtension = crane.buildPackage (commonArgs // {
    pname          = "prototext-codec-ext";
    cargoArtifacts = depsCache;
    cargoExtraArgs = "${pyo3Args} --lib";
    doCheck        = false;
    # Clear stale Cargo fingerprints so the pyo3 build script re-runs here.
    preBuild = ''
      rm -rf target/release/build/prototext_codec-*
      rm -rf target/release/.fingerprint/prototext_codec-*
    '';
    postBuild = ''
      echo "Generating prototext_codec_lib stubs..."
      cargo run --release -p prototext_codec --bin prototext_post_build
    '';
    installPhase = ''
      mkdir -p $out/artifacts/
      # The shared library extension differs by platform (.so on Linux, .dylib on macOS).
      ext=${if pkgs.stdenv.isDarwin then "dylib" else "so"}
      cp target/release/libprototext_codec_lib.$ext $out/artifacts/prototext_codec_lib.so
      cp prototext-pyo3/prototext_codec.pyi           $out/artifacts/prototext_codec_lib.pyi
    '';
  });

  # Python package wrapping the .so and .pyi into a wheel.
  prototextCodec = pythonPkgs.buildPythonPackage {
    pname   = "prototext_codec";
    version = "0.1.0";
    format  = "pyproject";
    src     = ./prototext-pyo3;
    buildInputs = [ pythonPkgs.hatchling prototextExtension ];
    patchPhase = ''
      cp ${prototextExtension}/artifacts/prototext_codec_lib.pyi \
         prototext_codec_lib/prototext_codec_lib.pyi
      cp ${prototextExtension}/artifacts/prototext_codec_lib.so  \
         prototext_codec_lib/prototext_codec_lib.so
    '';
  };

  # ---------------------------------------------------------------------------
  # PyO3 extension — fdp_scan Python package
  # ---------------------------------------------------------------------------

  fdpscanExtension = crane.buildPackage (commonArgs // {
    pname          = "fdp-scan-ext";
    cargoArtifacts = depsCache;
    cargoExtraArgs = "${fdpScanArgs} --lib";
    doCheck        = false;
    preBuild = ''
      rm -rf target/release/build/fdp_scan_extension-*
      rm -rf target/release/.fingerprint/fdp_scan_extension-*
    '';
    postBuild = ''
      echo "Generating fdp_scan_lib stubs..."
      cargo run --release -p fdp_scan_extension --bin fdp_scan_post_build
    '';
    installPhase = ''
      mkdir -p $out/artifacts/
      ext=${if pkgs.stdenv.isDarwin then "dylib" else "so"}
      cp target/release/libfdp_scan_lib.$ext $out/artifacts/fdp_scan_lib.so
      cp fdp-scan-pyo3/fdp_scan.pyi          $out/artifacts/fdp_scan_lib.pyi
    '';
  });

  # Python package wrapping the .so and .pyi into a wheel.
  # Importable as `fdp_scan_lib` (the Rust cdylib name).
  fdpScanLib = pythonPkgs.buildPythonPackage {
    pname   = "fdp_scan";
    version = "0.1.0";
    format  = "pyproject";
    src     = ./fdp-scan-pyo3;
    buildInputs = [ pythonPkgs.hatchling fdpscanExtension ];
    patchPhase = ''
      cp ${fdpscanExtension}/artifacts/fdp_scan_lib.pyi fdp_scan_lib/fdp_scan_lib.pyi
      cp ${fdpscanExtension}/artifacts/fdp_scan_lib.so  fdp_scan_lib/fdp_scan_lib.so
    '';
  };

  # ---------------------------------------------------------------------------
  # PyO3 extension — scoring_graph Python package
  # ---------------------------------------------------------------------------

  scoringGraphExtension = crane.buildPackage (commonArgs // {
    pname          = "scoring-graph-ext";
    cargoArtifacts = depsCache;
    cargoExtraArgs = "${scoringGraphArgs} --lib";
    doCheck        = false;
    preBuild = ''
      rm -rf target/release/build/scoring_graph_extension-*
      rm -rf target/release/.fingerprint/scoring_graph_extension-*
    '';
    postBuild = ''
      echo "Generating scoring_graph_lib stubs..."
      cargo run --release -p scoring_graph_extension --bin scoring_graph_post_build
    '';
    installPhase = ''
      mkdir -p $out/artifacts/
      ext=${if pkgs.stdenv.isDarwin then "dylib" else "so"}
      cp target/release/libscoring_graph_lib.$ext $out/artifacts/scoring_graph_lib.so
      cp score-graph-pyo3/scoring_graph.pyi         $out/artifacts/scoring_graph_lib.pyi
    '';
  });

  # Python package wrapping the .so and .pyi into a wheel.
  scoringGraphLib = pythonPkgs.buildPythonPackage {
    pname   = "scoring_graph";
    version = "0.1.0";
    format  = "pyproject";
    src     = ./score-graph-pyo3;
    buildInputs = [ pythonPkgs.hatchling scoringGraphExtension ];
    patchPhase = ''
      cp ${scoringGraphExtension}/artifacts/scoring_graph_lib.pyi \
         scoring_graph_lib/scoring_graph_lib.pyi
      cp ${scoringGraphExtension}/artifacts/scoring_graph_lib.so  \
         scoring_graph_lib/scoring_graph_lib.so
    '';
  };

  # ---------------------------------------------------------------------------
  # protoscan — Python CLI for scanning binaries for embedded FDP blobs
  # ---------------------------------------------------------------------------

  protoscan = pythonPkgs.buildPythonPackage {
    pname   = "protoscan";
    version = "0.1.0";
    src     = ./protoscan;
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
  # reproto — Python package for reconstructing .proto sources from .pb files
  # ---------------------------------------------------------------------------

  # Filtered source for the reproto/ subtree — excludes .pb files (generated),
  # __pycache__ dirs, and result symlinks so the hash is stable.
  reprotoFilteredSrc = builtins.path {
    name   = "reproto-src";
    path   = ./reproto;
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
    src     = reprotoFilteredSrc;
    pyproject = true;

    nativeBuildInputs = [ pythonPkgs.setuptools pythonPkgs.wheel ];
    propagatedBuildInputs = reprotoPropagatedDeps;

    doCheck = false;

    postInstall = ''
      mkdir -p $out/patch
      cp -r ${reprotoFilteredSrc}/patch/* $out/patch/
    '';
  };

  # Codegen stage — seeds well-known .proto files from pkgs.protobuf, then
  # runs patch_reproto.sh to compile them into .pb descriptors.
  reprotoSrcWithCodegen = pkgs.runCommand "reproto-src-with-codegen" {
    buildInputs = [
      reprotoBare
      pkgs.protobuf        # provides protoc and well-known .proto includes
    ];
  } ''
    set -euo pipefail
    cp -r ${reprotoFilteredSrc} $out
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
    src     = reprotoSrcWithCodegen;
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
        --bash ${reprotoFilteredSrc}/src/reproto/completions.sh

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
    export PYTHONPATH="${reprotoSrcWithCodegen}/src:${treeSitterTextproto}"
    pytest -p no:cacheprovider ${reprotoSrcWithCodegen}/src/reproto/tests/ -x
    touch $out
  '';

  # ---------------------------------------------------------------------------
  # Python lint — pyright type checking for the reproto Python package.
  #
  # Runs against reprotoSrcWithCodegen so the generated .pb descriptor files
  # are present.  The prototextExtension artifacts are injected so pyright can
  # resolve prototext_codec_lib imports via the generated .pyi stub.
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
    export PYTHONPATH="${reprotoSrcWithCodegen}/src:${prototextExtension}/artifacts:${scoringGraphExtension}/artifacts:${treeSitterTextproto}"

    # Write a hermetic pyrightconfig.json.
    cat > pyrightconfig.json <<EOF
{
  "typeCheckingMode": "basic",
  "extraPaths": [
    "${reprotoSrcWithCodegen}/src",
    "${prototextExtension}/artifacts",
    "${scoringGraphExtension}/artifacts",
    "${treeSitterTextproto}"
  ],
  "exclude": [
    "result*",
    "docs/mockup"
  ]
}
EOF

    echo "--- pyright ---"
    pyright ${reprotoSrcWithCodegen}/src/

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
    ruff check --no-cache --exclude docs/mockup ${reprotoSrcWithCodegen}/src/
    touch $out
  '';

  # ---------------------------------------------------------------------------
  # User shell — plain shell with prototext and reproto installed.
  # Activated by plain `nix-shell` (via shell.nix).
  # ---------------------------------------------------------------------------
  user-shell = pkgs.mkShell {
    name = "prototools-user";

    buildInputs = [ prototext reproto protoscan ];

    shellHook = ''
      old_opts=$(set +o)
      set -euo pipefail

      export NIXSHELL_REPO="${toString ./.}"
      export MANPATH="${prototext}/share/man:${reproto}/share/man:${protoscan}/share/man:''${MANPATH:-}"
      source ${prototext}/share/bash-completion/completions/prototext.bash
      source ${reproto}/share/bash-completion/completions/reproto.bash
      source ${protoscan}/share/bash-completion/completions/protoscan.bash

      [[ "$old_opts" == *"set -o errexit"*  ]] && set -e || set +e
      [[ "$old_opts" == *"set -o nounset"*  ]] && set -u || set +u
      [[ "$old_opts" == *"set -o pipefail"* ]] && set -o pipefail || set +o pipefail
    '';
  };

  # ---------------------------------------------------------------------------
  # Development shell
  # ---------------------------------------------------------------------------
  dev-shell = pkgs.mkShell {
    name = "prototools-dev";

    # Allow cargo to write build artifacts to target/ (outside /nix/store).
    NIX_ENFORCE_PURITY = 0;

    nativeBuildInputs = with pkgs; [
      cargo
      rustc
      rustfmt
      clippy
      reuse
      gh
      protobuf
      buf
      mandoc
      zola
      pythonPkgs.pytest
      pythonPkgs."pytest-xdist"
      pythonPkgs.ruff
      pkgs.pyright
    ];

    shellHook = ''
      old_opts=$(set +o)
      set -euo pipefail

      # Detected by ~/.claude/hooks/claude-hook-post-edit-lint to confirm
      # that the active nix-shell belongs to this repo.
      export NIXSHELL_REPO="${toString ./.}"

      export PYO3_PYTHON="${pythonExecutable}"

      export PATH="${toString ./.}/bin:${pythonBin}/bin:${toString ./.}/target/release:$PATH"

      export PYTHONPATH="$PWD/reproto/src:$PWD/protoscan/src:${treeSitterTextproto}:${pythonPkgs.makePythonPath reprotoTestDeps}:$PYTHONPATH"

      # Write .env so VS Code / Pylance picks up the interpreter and PYTHONPATH.
      echo "PYTHON_INTERPRETER=${pythonExecutable}" > .env
      echo "PYTHONPATH=$PYTHONPATH" >> .env

      # Generate pyrightconfig.json from $PYTHONPATH so pyright CLI and Pylance
      # stay in sync with default.nix automatically.
      python3 -c "
import json, os
paths = [p for p in os.environ['PYTHONPATH'].split(':') if p]
cfg = {'extraPaths': paths, 'exclude': ['result*', 'prototext-pyo3/prototext_codec_lib', 'fdp-scan-pyo3/fdp_scan_lib', 'score-graph-pyo3/scoring_graph_lib', 'docs/mockup']}
with open('pyrightconfig.json', 'w') as f:
    json.dump(cfg, f, indent=2)
    f.write('\n')
"

      # Generate ruff.toml so that ruff check (run by the lint hook) excludes
      # the docs/mockup scratch directory.
      cat > ruff.toml <<'RUFFEOF'
exclude = [
  "docs/mockup",
]
RUFFEOF

      # Compile prototext fixture .pb descriptors into prototext/fixtures/prebuilt/,
      # mirroring what protoPatchPhase does in the Nix build.
      # Skipped if the descriptor files are already present.
      if [[ ! -f "$PWD/prototext/fixtures/prebuilt/descriptor.pb" ]]; then
        mkdir -p "$PWD/prototext/fixtures/prebuilt"
        protoc \
          --descriptor_set_out="$PWD/prototext/fixtures/prebuilt/descriptor.pb" \
          google/protobuf/descriptor.proto
        protoc \
          --descriptor_set_out="$PWD/prototext/fixtures/prebuilt/knife.pb" \
          --proto_path="$PWD/prototext/fixtures/schemas" \
          knife.proto
        protoc \
          --descriptor_set_out="$PWD/prototext/fixtures/prebuilt/enum_collision.pb" \
          --proto_path="$PWD/prototext/fixtures/schemas" \
          enum_collision.proto
      fi

      # Seed well-known .proto sources and compile .pb descriptors into the
      # working tree, mirroring what reprotoSrcWithCodegen does in the Nix build.
      # Skipped if the descriptor files are already present.
      if [[ ! -f "$PWD/reproto/src/resources/google/protobuf/descriptor.pb" ]]; then
        mkdir -p "$PWD/reproto/src/resources/google/protobuf"
        cp ${pkgs.protobuf}/include/google/protobuf/*.proto \
           "$PWD/reproto/src/resources/google/protobuf/"
        bash "$PWD/reproto/patch/patch_reproto.sh" \
          "${reprotoBare}" "$PWD/reproto"
      fi

      # Build prototext only when the binary is absent or sources are newer.
      # Cargo's own incremental logic handles finer-grained staleness within
      # the working tree; this guard avoids the ~23s invocation overhead on
      # warm nix-shell entries when nothing has changed.
      if [[ ! -f "$PWD/target/release/prototext" ]] || \
         [[ "$PWD/prototext/src" -nt "$PWD/target/release/prototext" ]]; then
        cargo build --release --locked -p prototext
      fi

      # RUSTFLAGS is set globally in commonArgs (Nix build) so that all Crane
      # derivations share a single fingerprint.  Export the same value here so
      # that manual `cargo build -p prototext_codec` in the shell aligns.
      export RUSTFLAGS="${pyo3Rustflags}"

      # Generate man pages into man/man1/ and expose them via MANPATH.
      mkdir -p man/man1
      if command -v prototext-gen-man &>/dev/null; then
        prototext-gen-man man/man1
      fi
      if python3 -c "import reproto.gen_man" 2>/dev/null; then
        python3 -m reproto.gen_man man/man1
      fi
      if python3 -c "import protoscan.gen_man" 2>/dev/null; then
        python3 -m protoscan.gen_man man/man1
      fi
      export MANPATH="$PWD/man:''${MANPATH:-}"
      makewhatis "$PWD/man" 2>/dev/null || true

      # Generate rust-toolchain.toml so rust-analyzer uses the same rustc version
      # as the nix-shell build.  Only written when the content changes to avoid
      # invalidating Cargo fingerprints on every nix-shell entry.
      _toolchain_content="[toolchain]
channel = \"${pkgs.rustc.unwrapped.version}\"
components = [\"rust-src\", \"rustfmt\", \"clippy\"]"
      if [[ "$(cat rust-toolchain.toml 2>/dev/null)" != "$_toolchain_content" ]]; then
        rustup toolchain install ${pkgs.rustc.unwrapped.version} \
          --component rust-src --no-self-update 2>/dev/null || true
        printf '%s\n' "$_toolchain_content" > rust-toolchain.toml
      fi
      unset _toolchain_content

      # bash completion for prototext
      if command -v prototext &>/dev/null; then
        source <(PROTOTEXT_COMPLETE=bash prototext | sed \
          -e 's|-o nospace -o bashdefault|-o nospace -o filenames -o bashdefault|g' \
          -e 's|words\[COMP_CWORD\]="$2"|local _cur="''${COMP_LINE:0:''${COMP_POINT}}"; _cur="''${_cur##* }"; words[COMP_CWORD]="''${_cur}"|')
      fi

      # bash completion for reproto (pre-built script, avoids slow click invocation)
      eval "$(cat $PWD/reproto/src/reproto/completions.sh)"

      # bash completion for protoscan
      eval "$(_PROTOSCAN_COMPLETE=bash_source protoscan)"

      [[ "$old_opts" == *"set -o errexit"*  ]] && set -e || set +e
      [[ "$old_opts" == *"set -o nounset"*  ]] && set -u || set +u
      [[ "$old_opts" == *"set -o pipefail"* ]] && set -o pipefail || set +o pipefail
    '';
  };

  # ---------------------------------------------------------------------------
  # Stress tests (spec 0054) — separate from ci; triggered by
  # nix-build -A stress-tests only.
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

    FIXTURES="${reprotoSrcWithCodegen}/src/reproto/tests/fixtures"
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
    export PYTHONPATH="${reprotoSrcWithCodegen}/src"
    export STRESS_DB="${stressDb}/stress.desc"
    pytest -p no:cacheprovider ${./tests/stress}/
    touch $out
  '';

  # Bundle: prototext + reproto + protoscan (the full prototools suite).
  prototools = pkgs.symlinkJoin {
    name   = "prototools";
    paths  = [ prototext reproto protoscan ];
  };

  # Single target that forces the entire CI closure in dependency order.
  # nix-build -A ci builds fmt → clippy → clippy-pyo3 → tests → prototext → prototext-codec → reproto → protoscan.
  ci = pkgs.linkFarmFromDrvs "ci" [
    rustFmt rustClippy rustClippyPyo3 rustClippyScoringGraph rustTests prototext prototextCodec scoringGraphLib reproto reprotoTests pythonLint pythonRuff protoscan
  ];

in
{
  default              = ci;
  prototools           = prototools;
  prototext            = prototext;
  rust-fmt             = rustFmt;
  rust-clippy          = rustClippy;
  rust-clippy-pyo3     = rustClippyPyo3;
  rust-tests           = rustTests;
  prototext-codec      = prototextCodec;
  reproto              = reproto;
  reproto-bare         = reprotoBare;
  reproto-tests        = reprotoTests;
  python-lint          = pythonLint;
  python-ruff          = pythonRuff;
  ci                   = ci;
  stress-db            = stressDb;
  stress-tests         = stressTests;
  user-shell           = user-shell;
  dev-shell            = dev-shell;
  protoscan            = protoscan;
  fdp-scan-lib         = fdpScanLib;
  scoring-graph-lib    = scoringGraphLib;
}
