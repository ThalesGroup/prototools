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
    # Exclude reproto/ and bin/ — Python-only subtrees that must not perturb
    # the Rust derivation hashes.
    filter = path: type:
      let rel = pkgs.lib.removePrefix (toString ./. + "/") (toString path);
      in
      !(pkgs.lib.hasPrefix "reproto/" rel) &&
      !(pkgs.lib.hasPrefix "bin/" rel) &&
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
  prototools = crane.buildPackage (protocArgs // {
    pname          = "prototools";
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
      description  = "Command-line utilities for Protocol Buffer messages (prototext binary)";
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
      prototextCodec  # reproto.load imports prototext_codec_lib at module load time
    ];

    doCheck = false;

    postInstall = ''
      installShellCompletion --cmd reproto \
        --bash ${reprotoFilteredSrc}/src/reproto/completions.sh
    '';
  };

  # Tests run separately so that the installable reproto package has doCheck = false
  # (avoiding pytest during nix-shell) while ci still enforces test passage.
  reprotoTests = pkgs.runCommand "reproto-tests" {
    buildInputs = [
      pkgs.protobuf
      pkgs.buf
      prototools
      (pythonPkgs.python.withPackages (_: reprotoTestDeps))
    ];
  } ''
    export PYTHONPATH="${reprotoSrcWithCodegen}/src"
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

    # Make the prototext_codec_lib .pyi stub visible to pyright.
    export PYTHONPATH="${reprotoSrcWithCodegen}/src:${prototextExtension}/artifacts"

    # Write a hermetic pyrightconfig.json.
    cat > pyrightconfig.json <<EOF
{
  "typeCheckingMode": "basic",
  "extraPaths": [
    "${reprotoSrcWithCodegen}/src",
    "${prototextExtension}/artifacts"
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

      export PYTHONPATH="$PWD/reproto/src:${pythonPkgs.makePythonPath reprotoTestDeps}:$PYTHONPATH"

      # Write .env so VS Code / Pylance picks up the interpreter and PYTHONPATH.
      echo "PYTHON_INTERPRETER=${pythonExecutable}" > .env
      echo "PYTHONPATH=$PYTHONPATH" >> .env

      # Generate pyrightconfig.json from $PYTHONPATH so pyright CLI and Pylance
      # stay in sync with default.nix automatically.
      python3 -c "
import json, os
paths = [p for p in os.environ['PYTHONPATH'].split(':') if p]
cfg = {'extraPaths': paths, 'exclude': ['result*', 'prototext-pyo3/prototext_codec_lib', 'docs/mockup']}
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

      # Generate man page into man/man1/ and expose it via MANPATH.
      if command -v prototext-gen-man &>/dev/null; then
        mkdir -p man/man1
        prototext-gen-man man/man1
        export MANPATH="$PWD/man:''${MANPATH:-}"
        makewhatis "$PWD/man" 2>/dev/null || true
      fi

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

      [[ "$old_opts" == *"set -o errexit"*  ]] && set -e || set +e
      [[ "$old_opts" == *"set -o nounset"*  ]] && set -u || set +u
      [[ "$old_opts" == *"set -o pipefail"* ]] && set -o pipefail || set +o pipefail
    '';
  };

  # Single target that forces the entire CI closure in dependency order.
  # nix-build -A ci builds fmt → clippy → clippy-pyo3 → tests → prototools → prototext-codec → reproto.
  ci = pkgs.linkFarmFromDrvs "ci" [
    rustFmt rustClippy rustClippyPyo3 rustTests prototools prototextCodec reproto reprotoTests pythonLint pythonRuff
  ];

in
{
  default              = ci;
  prototools           = prototools;
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
  dev-shell            = dev-shell;
}
