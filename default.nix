# SPDX-FileCopyrightText: 2025 - 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
# SPDX-FileCopyrightText: 2025 - 2026 THALES CLOUD SECURISE SAS
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
    filter = path: type:
      (crane.filterCargoSources path type) ||
      (pkgs.lib.hasInfix "/fixtures/" path);
  };

  # Common arguments shared by all Crane derivations.
  commonArgs = {
    inherit src;
    pname   = "prototools";
    version = "0.1.4";
    strictDeps = true;
    nativeBuildInputs = [ pkgs.cargo pkgs.rustc ];
  };

  # ---------------------------------------------------------------------------
  # Shared dependency cache — built once, reused by tests, clippy, and the
  # final package.  Only rebuilt when Cargo.lock or dependency sources change.
  # ---------------------------------------------------------------------------
  depsCache = crane.buildDepsOnly (commonArgs // {
    pname          = "prototools-deps";
    # Exclude the pyo3 crate: it requires a Python interpreter (PYO3_PYTHON)
    # that is not available in commonArgs.  Its deps are handled by pyo3DepsCache.
    cargoExtraArgs = "--workspace --exclude prototext_codec";
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

  rustClippy = crane.cargoClippy (commonArgs // {
    pname                = "prototools-clippy";
    cargoArtifacts       = depsCache;
    cargoExtraArgs       = "--workspace --exclude prototext_codec";
    cargoClippyExtraArgs = "--all-targets -- --deny warnings";
  });

  rustClippyPyo3 = crane.cargoClippy (pyo3CommonArgs // {
    pname                = "prototools-clippy-pyo3";
    cargoArtifacts       = pyo3DepsCache;
    cargoExtraArgs       = "-p prototext_codec";
    cargoClippyExtraArgs = "--all-targets -- --deny warnings";
  });

  # ---------------------------------------------------------------------------
  # Tests — workspace-wide, reusing depsCache.
  # ---------------------------------------------------------------------------
  rustTests = crane.cargoTest (commonArgs // {
    pname             = "prototools-tests";
    cargoArtifacts    = depsCache;
    cargoExtraArgs    = "--workspace --exclude prototext_codec";
    nativeBuildInputs = (commonArgs.nativeBuildInputs or []) ++ [ pkgs.protobuf ];
  });

  # ---------------------------------------------------------------------------
  # Final package — builds only the prototext binary.
  # checkPhase asserts that fmt, clippy, and tests all passed by referencing
  # their store paths (Nix fails the build if any derivation is missing).
  # ---------------------------------------------------------------------------
  prototools = crane.buildPackage (commonArgs // {
    pname          = "prototools";
    version        = "0.1.0";
    cargoArtifacts = depsCache;
    cargoExtraArgs = "-p prototext";

    nativeBuildInputs = (commonArgs.nativeBuildInputs or []) ++ [ pkgs.installShellFiles ];

    checkPhase = ''
      echo "fmt:    ${rustFmt}"
      echo "clippy: ${rustClippy}"
      echo "tests:  ${rustTests}"
    '';

    postInstall = ''
      # Install shell completions.
      installShellCompletion --cmd prototext \
        --bash <(PROTOTEXT_COMPLETE=bash $out/bin/prototext | sed \
          -e '/^\s*) )$/a\    compopt -o filenames 2>/dev/null' \
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
  pythonBin        = pythonPkgs.python.withPackages (_: []);
  pythonExecutable = "${pythonBin}/bin/python";

  # commonArgs extended with the three values required for a PyO3 Crane build.
  pyo3CommonArgs = commonArgs // {
    env.PYO3_PYTHON    = pythonExecutable;
    RUSTFLAGS          = "-L ${pythonBin}/lib -lpython${pythonPkgs.python.pythonVersion}";
    nativeBuildInputs  = commonArgs.nativeBuildInputs ++ [ pythonBin ];
  };

  # Dependency cache for the pyo3 crate — built once, reused by the extension.
  # RUSTFLAGS must match exactly between here and prototextExtension so that
  # Cargo fingerprints align and .rlib artifacts are reused (not recompiled).
  pyo3DepsCache = crane.buildDepsOnly (pyo3CommonArgs // {
    pname          = "prototext-codec-deps";
    cargoExtraArgs = "-p prototext_codec";
    doCheck        = false;
    # Skip the default `cargo check` pass: the build pass below produces .rlib
    # files that are a strict superset of what check produces.
    buildPhaseCargoCommand = "cargoWithProfile build -p prototext_codec";
  });

  # Compile the cdylib and run post_build to generate the .pyi stub.
  prototextExtension = crane.buildPackage (pyo3CommonArgs // {
    pname          = "prototext-codec-ext";
    cargoArtifacts = pyo3DepsCache;
    cargoExtraArgs = "-p prototext_codec --lib";
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
      cp prototext-pyo3/prototext_codec_lib.pyi      $out/artifacts/prototext_codec_lib.pyi
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
      mandoc
      zola
    ];

    shellHook = ''
      old_opts=$(set +o)
      set -euo pipefail

      # Detected by ~/.claude/hooks/claude-hook-post-edit-lint to confirm
      # that the active nix-shell belongs to this repo.
      export NIXSHELL_REPO="${toString ./.}"

      # Add the cargo release build to PATH so prototext is available after cargo build --release.
      export PATH="${toString ./.}/target/release:$PATH"

      # Build prototext if not already built.
      cargo build --release --locked -p prototext

      # Generate man page into man/man1/ and expose it via MANPATH.
      if command -v prototext-gen-man &>/dev/null; then
        mkdir -p man/man1
        prototext-gen-man man/man1
        export MANPATH="$PWD/man:''${MANPATH:-}"
        makewhatis "$PWD/man" 2>/dev/null || true
      fi

      # bash completion for prototext (workaround for clap_complete path-completion bugs)
      if command -v prototext &>/dev/null; then
        source <(PROTOTEXT_COMPLETE=bash prototext | sed \
          -e '/^\s*) )$/a\    compopt -o filenames 2>/dev/null' \
          -e 's|words\[COMP_CWORD\]="$2"|local _cur="''${COMP_LINE:0:''${COMP_POINT}}"; _cur="''${_cur##* }"; words[COMP_CWORD]="''${_cur}"|')
      fi

      eval "$old_opts"
    '';
  };

in
{
  default              = prototools;
  prototools           = prototools;
  rust-fmt             = rustFmt;
  rust-clippy          = rustClippy;
  rust-clippy-pyo3     = rustClippyPyo3;
  rust-tests           = rustTests;
  prototext-codec      = prototextCodec;
  dev-shell            = dev-shell;
}
