# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

# nix/rust.nix — Crane derivations: dep cache, fmt, clippy, tests, the one
#                release build of the workspace, and what ships from it.
#
# Pipeline diagram (spec 0401):
#
#   src (Rust sources, fixtures/)
#     │
#     ├──[buildDepsOnly]──▶  depsCache  ──────────────────────────────────┐
#     │                                                                    │
#     ├──[cargoFmt]──▶  rustFmt                                            │
#     │                                                                    │
#     ├──[cargoClippy, cargoArtifacts=depsCache]──▶  rustClippy            │
#     │                                                                    │
#     ├──[cargoTest, profile quick]──▶  rustTests  (a ci leaf)              │
#     │                                                                    │
#     └──[buildPackage, cargoArtifacts=depsCache]──▶  workspaceBuild
#            │  one `cargo build --release --workspace`
#            ├──▶  prototext            (copied out, completions, man page)
#            ├──▶  protolensUnwrapped  ──▶  protolens, protolensLean
#            └──▶  prototextCodec, fdpScanLib, prototextGraphLib  (.so + .pyi)
#
# Every shipped crate embeds the WKT scoring graph committed under
# prototext/wkt/prebuilt/ (S2); default.nix's wkt-prebuilt-check keeps that
# copy equal to what the code generates (S3). Nothing here waits for the
# tests (S4).

{ pkgs
, crane
, pythonPkgs
, pythonBin
, pythonExecutable
, depsSrc
, workspaceSrc
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

  # Every build embeds the WKT scoring graph committed under
  # prototext/wkt/prebuilt/, which prototext's build.rs copies with no feature
  # flag (specs 0401 S2, 0405 S2). Before, it took `--features prebuilt-wkt`,
  # and without it build.rs ran reproto, which needs the extensions built
  # here: a cycle. default.nix's wkt-prebuilt-check fails `ci` when the copy
  # no longer matches what the code generates.

  # ---------------------------------------------------------------------------
  # Base argument sets — hierarchic composition.
  #
  # commonArgs: base for ALL Crane derivations (Rust + pyo3).
  #   Carries PYO3_PYTHON, which the pyo3 crates' build scripts read to find
  #   the interpreter and the libpython they link per target (spec 0402 S6),
  #   so that a single depsCache covers the whole workspace.
  #
  # protocArgs: extends commonArgs with pkgs.protobuf.
  #   Used by ALL derivations including depsCache, so the sandbox environment
  #   is identical everywhere and Cargo fingerprints are stable across the chain.
  #   (buildDepsOnly stubs build.rs so protoc is never actually invoked there.)
  #
  # Crane builds in release mode by default via configureCargoCommonVarsHook.
  # All cargo build invocations in the shellHook must pass --release explicitly.
  # ---------------------------------------------------------------------------
  # Spec 0402 S3: versions come from the manifests, read at evaluation
  # time (lib.importTOML reads a source file; it is not
  # import-from-derivation). No version literal lives in nix/.
  # One version for the whole workspace (spec 0405 S5): the crates inherit
  # it, and each pyproject.toml must repeat it, or evaluation fails here.
  workspaceVersion = (pkgs.lib.importTOML ../Cargo.toml).workspace.package.version;
  #
  # Between releases the workspace carries X.Y.Z-dev (nixpkgs/README.md),
  # which Python spells X.Y.Z.dev0 (PEP 440): a "-" in a wheel's version
  # would break its file name.
  pythonVersion = builtins.replaceStrings [ "-dev" ] [ ".dev0" ] workspaceVersion;
  pyprojectVersion = dir:
    let v = (pkgs.lib.importTOML (../. + "/${dir}/pyproject.toml")).project.version;
    in if v == pythonVersion then v
       else throw "${dir}/pyproject.toml has version ${v}; the workspace is ${workspaceVersion} (Cargo.toml), so ${pythonVersion}";
  prototextVersion = workspaceVersion;
  protolensVersion = workspaceVersion;

  # commonArgs omits src — each derivation sets its own focused src.
  # The crane derivations build the whole workspace, prototext's release.
  commonArgs = {
    pname             = "prototools";
    version           = prototextVersion;
    strictDeps        = true;
    # Python twice, as strictDeps distinguishes (spec 0402 S6): the
    # interpreter pyo3's build scripts run, and the libpython the stub
    # generators and tests link.
    nativeBuildInputs = [ pkgs.cargo pkgs.rustc pythonBin ];
    buildInputs       = [ pythonBin ];
    env.PYO3_PYTHON   = pythonExecutable;
  };

  # protoc for prototext's roundtrip tests, which compare with it. The
  # fixtures prototext's build.rs reads are committed (spec 0405 S1).
  protocArgs = commonArgs // {
    nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ pkgs.protobuf ];
  };

  # ---------------------------------------------------------------------------
  # Shared dependency cache — built once, reused by all Crane derivations.
  # Uses protocArgs (includes pkgs.protobuf) so the sandbox environment matches
  # all consumers. buildDepsOnly stubs build.rs so protoc is never invoked, but
  # having protobuf present prevents fingerprint mismatches that would force
  # external deps to recompile in every downstream derivation.
  # No protoc run: dummy build.rs never calls it.
  # ---------------------------------------------------------------------------
  depsCache = crane.buildDepsOnly (protocArgs // {
    src            = workspaceSrc;
    pname          = "prototools-deps";
    cargoExtraArgs = workspaceArgs;
    # The fixtures' protoc runs must not: buildDepsOnly uses dummy sources,
    # so the .proto files are absent and protoc would fail.
    postPatch      = "";
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
    cargoExtraArgs       = workspaceArgs;
    cargoClippyExtraArgs = "-- -D warnings";
  });

  # ---------------------------------------------------------------------------
  # Tests — workspace-wide, reusing depsCache. A leaf of `ci`: nothing builds
  # from them (spec 0401 S4), so a package never waits for the suite, and a
  # failing test fails `ci` without blocking the packages.
  # ---------------------------------------------------------------------------
  #
  # The `quick` profile (Cargo.toml): release codegen without LTO and with 16
  # codegen units (spec 0401 S8). The tests compile in a third of the time,
  # and leave the cores to workspaceBuild and clippy, which run alongside;
  # with `release` the three LTO builds starved each other. It needs its own
  # dependency cache, since a profile is part of every artifact's identity.
  depsCacheTests = crane.buildDepsOnly (protocArgs // {
    src             = workspaceSrc;
    pname           = "prototools-deps-tests";
    CARGO_PROFILE   = "quick";
    cargoExtraArgs  = workspaceArgs;
    # Only what cargoTest compiles: no check or build pass.
    cargoCheckCommand = "true";
    cargoBuildCommand = "true";
    postPatch       = "";
  });

  rustTests = crane.cargoTest (protocArgs // {
    src            = workspaceSrc;
    pname          = "prototools-tests";
    CARGO_PROFILE  = "quick";
    cargoArtifacts = depsCacheTests;
    cargoExtraArgs = workspaceArgs;
    # Tell supports_rgb() that RGB is available so color-sensitive tests
    # exercise the RGB code path in the sandbox (no real terminal there).
    # Extends commonArgs' env (`//` is shallow: a bare `env.COLORTERM`
    # here would replace the whole set, PYO3_PYTHON included).
    env            = protocArgs.env // { COLORTERM = "truecolor"; };
  });

  # Common postInstall for both prototext variants.
  prototextPostInstall = whenRunnable ''
    # Install shell completions.
    installShellCompletion --cmd prototext \
      --bash <(PROTOTEXT_COMPLETE=bash $out/bin/prototext) \
      --zsh  <(PROTOTEXT_COMPLETE=zsh  $out/bin/prototext) \
      --fish <(PROTOTEXT_COMPLETE=fish $out/bin/prototext)

    # Generate and install man page.
    $out/bin/prototext-gen-man $out/share/man/man1

    # Spec 0402 S8: the binary says the version Nix calls it (what
    # versionCheckHook checks, for a derivation without its phase).
    $out/bin/prototext --version | grep -qF ${prototextVersion}
  '';

  # Spec 0402 S4: what every installable output's meta shares.
  metaCommon = with pkgs.lib; {
    homepage    = "https://github.com/douzebis/prototools";
    license     = licenses.mit;
    maintainers = with maintainers; [ douzebis ];
    platforms   = platforms.unix;
  };

  prototextMeta = with pkgs.lib; metaCommon // {
    description  = "Lossless converter between binary protobuf and enhanced textproto";
    longDescription = ''
      prototools is a collection of CLI utilities for working with Protocol
      Buffer messages.  The first tool, prototext, converts between binary
      protobuf wire format and protoc-style enhanced textproto, with lossless
      round-trip by default.
    '';
    mainProgram = "prototext";
  };

  # ---------------------------------------------------------------------------
  # The three PyO3 extensions, as workspaceBuild installs them.
  #
  #   crateName    — Cargo package name, e.g. "prototext_codec_lib"
  #   crateDir     — the crate directory, relative to the workspace root; the
  #                  stub generator runs with it as CARGO_MANIFEST_DIR
  #   libName      — cdylib base name (Cargo [[lib]] name); lib<libName>.so;
  #                  also the Python module, and so the stub's name: each
  #                  pyproject.toml names it in tool.maturin.module-name,
  #                  which pyo3-stub-gen reads (without it, the generator
  #                  falls back to the [project] name, and a stub refers to
  #                  its own classes under that wrong module name)
  #   postBuildBin — the stub-generator binary target
  # ---------------------------------------------------------------------------
  pyo3Extensions = {
    prototextCodec = {
      description = "Python bindings to prototext's lossless protobuf codec";
      crateName = "prototext_codec_lib"; crateDir = "prototext-pyo3";
      libName = "prototext_codec_lib";
      postBuildBin = "prototext_post_build";
    };
    fdpScan = {
      description = "Python bindings to the scan for protobuf descriptors embedded in binaries";
      crateName = "fdp_scan_lib";        crateDir = "fdp-scan-pyo3";
      libName = "fdp_scan_lib";
      postBuildBin = "fdp_scan_post_build";
    };
    prototextGraph = {
      description = "Python bindings to the prototools scoring graph builder";
      crateName = "prototext_graph_lib"; crateDir = "prototext-graph-pyo3";
      libName = "prototext_graph_lib";
      postBuildBin = "prototext_graph_post_build";
    };
  };

  # lib<libName>.so on Linux, .dylib on Darwin; installed as <libName>.so,
  # the name Python imports.
  libExt = if pkgs.stdenv.isDarwin then "dylib" else "so";

  # Spec 0402 S7: cross-safe. Cargo writes a cross build under the target
  # triple's own directory, and a binary built for another platform cannot
  # be run to generate completions, man pages or stubs.
  profileDir =
    if pkgs.stdenv.buildPlatform == pkgs.stdenv.hostPlatform
    then "target/release"
    else "target/${pkgs.stdenv.hostPlatform.rust.rustcTarget}/release";
  canRun = pkgs.stdenv.buildPlatform.canExecute pkgs.stdenv.hostPlatform;
  whenRunnable = pkgs.lib.optionalString canRun;

  # ---------------------------------------------------------------------------
  # workspaceBuild — the one release build of the workspace (spec 0401 S1).
  #
  # A single `cargo build --workspace` with depsCache's flags, so Cargo
  # reuses every external-dependency artifact. (A
  # scoped -p invocation computes different unit hashes for the external
  # deps and recompiles them — see constant-rebuilds in the Crane FAQ; the
  # former per-extension builds paid that, about a minute each.) It builds
  # every binary and every cdylib, the stub generators included, so
  # everything that ships is copied out of it:
  #
  #   $out/bin/{prototext,prototext-gen-man,protolens}
  #   $out/ext/<libName>/<libName>.{so,pyi}
  #
  # The stub generators run here, with CARGO_MANIFEST_DIR set so
  # pyo3-stub-gen finds pyproject.toml and writes the .pyi beside it (the
  # NotPresent panic of spec 0038 was a missing CARGO_MANIFEST_DIR).
  #
  # doInstallCargoArtifacts = false: nothing builds on top of it, and an
  # installed target.tar.zst (with `.prev` pointing at depsCache and the
  # vendored registry) would enter the closure of whatever copies from it
  # (spec 0374 S2). Crane's postInstall hook strips the vendored sources'
  # store paths rustc embeds as panic locations, for the same reason.
  # ---------------------------------------------------------------------------
  workspaceBuild = crane.buildPackage (protocArgs // {
    src                                = workspaceSrc;
    pname                              = "prototools-workspace";
    cargoArtifacts                     = depsCache;
    doCheck                            = false;
    doInstallCargoArtifacts            = false;
    buildPhaseCargoCommand             = "cargoWithProfile build ${workspaceArgs}";
    doNotPostBuildInstallCargoBinaries = true;
    installPhaseCommand                = ''
      mkdir -p $out/bin
      cp ${profileDir}/prototext ${profileDir}/prototext-gen-man \
         ${profileDir}/protolens ${profileDir}/protoscan $out/bin/
    '' + pkgs.lib.concatMapStrings (e: ''
      mkdir -p $out/ext/${e.libName}
      cp ${profileDir}/lib${e.libName}.${libExt} $out/ext/${e.libName}/${e.libName}.so
    '' + (if canRun then ''
      CARGO_MANIFEST_DIR="$PWD/${e.crateDir}" ./${profileDir}/${e.postBuildBin}
      # The committed stub is what a cross build ships: it must be what the
      # generator writes.
      if ! diff -u ${e.crateDir}/${e.libName}/${e.libName}.pyi ${e.crateDir}/${e.libName}.pyi >&2; then
        echo "${e.crateDir}/${e.libName}/${e.libName}.pyi is stale (diff above). Refresh it with:" >&2
        echo "  (cd ${e.crateDir} && CARGO_MANIFEST_DIR=\$PWD cargo run --release --bin ${e.postBuildBin}) && cp ${e.crateDir}/${e.libName}.pyi ${e.crateDir}/${e.libName}/" >&2
        exit 1
      fi
      cp ${e.crateDir}/${e.libName}.pyi $out/ext/${e.libName}/${e.libName}.pyi
    '' else ''
      # Cross: the stub generator cannot run here; the committed stub, which
      # every native build compares with the generator's, stands in.
      cp ${e.crateDir}/${e.libName}/${e.libName}.pyi $out/ext/${e.libName}/${e.libName}.pyi
    '')) (builtins.attrValues pyo3Extensions);
  });

  # ---------------------------------------------------------------------------
  # prototext — the binary, copied out of workspaceBuild, with its shell
  # completions and man page. It embeds the committed WKT graph (S2).
  # ---------------------------------------------------------------------------
  prototext = pkgs.runCommand "prototext-${prototextVersion}" {
    nativeBuildInputs = [ pkgs.installShellFiles ];
    meta              = prototextMeta;
    # Spec 0402 S8: where nixpkgs looks for a package's tests.
    passthru.tests    = { inherit rustTests; };
  } ''
    mkdir -p $out/bin
    cp ${workspaceBuild}/bin/prototext ${workspaceBuild}/bin/prototext-gen-man $out/bin/
    ${prototextPostInstall}
  '';

  # ---------------------------------------------------------------------------
  # protoscan — find the protobuf descriptors embedded in a binary (spec 0407).
  # A Rust binary since 0407, copied out of workspaceBuild like prototext; its
  # man page comes from the binary itself (PROTOSCAN_GEN_MAN), as protolens's.
  # ---------------------------------------------------------------------------
  protoscan = pkgs.runCommand "protoscan-${workspaceVersion}" {
    nativeBuildInputs = [ pkgs.installShellFiles ];
    meta = metaCommon // {
      description = "Find the protobuf descriptors embedded in binaries";
      mainProgram = "protoscan";
    };
    passthru.tests = { inherit rustTests; };
  } (''
    mkdir -p $out/bin
    cp ${workspaceBuild}/bin/protoscan $out/bin/
  '' + whenRunnable ''
    installShellCompletion --cmd protoscan \
      --bash <(PROTOSCAN_COMPLETE=bash $out/bin/protoscan) \
      --zsh  <(PROTOSCAN_COMPLETE=zsh  $out/bin/protoscan) \
      --fish <(PROTOSCAN_COMPLETE=fish $out/bin/protoscan)
    PROTOSCAN_GEN_MAN=$out/share/man/man1 $out/bin/protoscan
    $out/bin/protoscan --version | grep -qF ${workspaceVersion}
  '');

  # ---------------------------------------------------------------------------
  # protolens — interactive TUI to decode/navigate/extract a binary protobuf.
  # It always takes an explicit --descriptor-set (spec 0111 v1, no embedded
  # WKT fallback). Its binary comes out of workspaceBuild with the others.
  # ---------------------------------------------------------------------------
  protolensPostInstall = whenRunnable ''
    installShellCompletion --cmd protolens \
      --bash <(PROTOLENS_COMPLETE=bash $out/bin/protolens) \
      --zsh  <(PROTOLENS_COMPLETE=zsh  $out/bin/protolens) \
      --fish <(PROTOLENS_COMPLETE=fish $out/bin/protolens)

    # Generate and install man page (spec 0228 S11), from the real binary.
    PROTOLENS_GEN_MAN=$out/share/man/man1 $out/bin/protolens

    # Spec 0402 S8, as for prototext.
    $out/bin/protolens --version | grep -qF ${protolensVersion}
  '' + ''
    # spec 0145 G5: a minimal Neovim config wiring `.proto` filetype/syntax
    # and `buf lsp serve` navigation, loaded via `-u` by the wrapper below.
    install -Dm444 ${../protolens/nvim/init.lua} \
      "$out/share/protolens/nvim/init.lua"
  '';

  protolensMeta = metaCommon // {
    description = "Interactive TUI to decode, navigate, and extract raw bytes from a binary protobuf";
    mainProgram = "protolens";
  };

  # The binary, its completions, man page and Neovim config — copied out of
  # workspaceBuild once, and wrapped below as many ways as needed (spec 0374
  # S2/G7; spec 0401 S1).
  protolensUnwrapped = pkgs.runCommand "protolens-unwrapped-${protolensVersion}" {
    nativeBuildInputs = [ pkgs.installShellFiles ];
    meta              = protolensMeta;
    passthru.tests    = { inherit rustTests; };
  } ''
    mkdir -p $out/bin
    cp ${workspaceBuild}/bin/protolens $out/bin/
    ${protolensPostInstall}
  '';

  # `v`'s Neovim handoff (spec 0144 G5/G6) is a mandatory runtime dependency,
  # not merely a dev-shell convenience — bundle a pinned Neovim and `buf`
  # (for `buf lsp serve`) onto PATH so they resolve regardless of the user's
  # own $PATH. A separate derivation from the compile (spec 0374 S2), so a
  # different Neovim costs a shell script, not a Rust build.
  bufFull = buf;
  wrapProtolens = { neovim, buf ? bufFull, name ? "protolens" }:
    pkgs.runCommand "${name}-${protolensVersion}" {
      nativeBuildInputs = [ pkgs.makeWrapper ];
      meta              = protolensMeta;
    } ''
      mkdir -p $out/bin
      ln -s ${protolensUnwrapped}/share $out/share
      makeWrapper ${protolensUnwrapped}/bin/protolens $out/bin/protolens \
        --prefix PATH : ${pkgs.lib.makeBinPath [ neovim buf ]} \
        --set PROTOLENS_NVIM_CONFIG ${protolensUnwrapped}/share/protolens/nvim/init.lua
    '';

  # The default Neovim: its wrapper keeps the Wayland clipboard provider
  # (wl-clipboard), which desktop users want.
  protolens = wrapProtolens { neovim = pkgs.neovim; };

  # A lean Neovim for the workshop image (spec 0374 S2): no clipboard (there
  # is no desktop clipboard in a container; wl-clipboard also brings perl),
  # and no Ruby or Python providers, which protolens's init.lua — pure Lua and
  # Neovim's built-in LSP client — never uses. 98 MiB instead of 407 MiB, and
  # no compilation: `wrapNeovimUnstable` only wraps the shared neovim-unwrapped.
  # (`pkgs.neovim.override { waylandSupport = false; }` does not evaluate: the
  # argument belongs to wrapNeovimUnstable, not to the `neovim` package.)
  #
  # wrapRc = false: the default wrapper exports VIMINIT pointing at a generated
  # (here empty) init.lua, and with VIMINIT set Neovim never reads
  # $XDG_CONFIG_HOME/nvim/init.lua — which is how teleprompt's config (desert
  # theme, proto/textproto syntax, spec 0395) gets loaded. protolens passes its
  # own config explicitly and is unaffected.
  neovimLean = pkgs.wrapNeovimUnstable pkgs.neovim-unwrapped {
    wrapRc         = false;
    waylandSupport = false;
    withRuby       = false;
    withPython3    = false;
  };
  # buf for the image (spec 0375 S9): its `buf` binary alone, copied rather
  # than linked so the full package leaves the closure. protolens runs only
  # `buf lsp serve`; the two protoc-gen-buf-* plugins are 72 MiB unpacked.
  bufLean = pkgs.runCommand "buf-lean-${buf.version}" { } ''
    install -Dm755 ${buf}/bin/buf $out/bin/buf
  '';

  protolensLean = wrapProtolens {
    neovim = neovimLean;
    buf    = bufLean;
    name   = "protolens-lean";
  };

  # ---------------------------------------------------------------------------
  # PyO3 extensions — prototext_codec_lib, fdp_scan_lib, prototext_graph_lib.
  #
  # Each is a buildPythonPackage that copies its .so and .pyi, built by
  # workspaceBuild (S1), into the pyproject source tree beside __init__.py,
  # where hatchling picks them up. `artifacts` exposes the same directory to
  # pyright (pythonLint) and the PyPI wheels.
  # ---------------------------------------------------------------------------
  makePyo3Extension = e:
    let
      artifacts = "${workspaceBuild}/ext/${e.libName}";
      pkg = pythonPkgs.buildPythonPackage {
        pname     = e.crateName;
        version   = pyprojectVersion e.crateDir;
        # Spec 0402 S5: pyproject = true with build-system, not the
        # deprecated `format`; the install check imports the extension.
        pyproject = true;
        build-system = [ pythonPkgs.hatchling ];
        src       = ../. + "/${e.crateDir}";
        pythonImportsCheck = [ e.libName ];
        postPatch = ''
          cp ${artifacts}/${e.libName}.* ${e.libName}/
        '';
        meta = metaCommon // { inherit (e) description; };
      };
    in { inherit pkg artifacts; };

  _prototextCodecExt    = makePyo3Extension pyo3Extensions.prototextCodec;
  _fdpScanLibExt        = makePyo3Extension pyo3Extensions.fdpScan;
  _prototextGraphLibExt = makePyo3Extension pyo3Extensions.prototextGraph;

  prototextCodec     = _prototextCodecExt.pkg;
  fdpScanLib         = _fdpScanLibExt.pkg;
  prototextGraphLib  = _prototextGraphLibExt.pkg;

in {
  inherit
    metaCommon
    pyprojectVersion
    workspaceVersion
    commonArgs
    depsCache
    rustFmt
    rustClippy
    rustTests
    workspaceBuild
    prototext
    protoscan
    protolens
    protolensUnwrapped
    protolensLean
    neovimLean
    bufLean
    prototextCodec
    fdpScanLib
    prototextGraphLib;
  prototextExtensionArtifacts      = _prototextCodecExt.artifacts;
  fdpScanExtensionArtifacts        = _fdpScanLibExt.artifacts;
  prototextGraphExtensionArtifacts = _prototextGraphLibExt.artifacts;
}
