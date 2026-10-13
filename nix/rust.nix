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
#            └──▶  prototextCodec, fdpScanLib, prototextGraphLib  (maturin's wheels)
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
  #
  # A dependency on one of this repository's Python packages must require
  # the same version, as `<name>>=<version>`: they are released together,
  # and an older lower bound would let pip pair a new reproto with old
  # extensions (spec 0411 S3).
  pyprojectVersion = dir:
    let
      project  = (pkgs.lib.importTOML (../. + "/${dir}/pyproject.toml")).project;
      v        = project.version;
      internal = [ "fdp-scan" "prototext-codec" "prototext-graph" "tree-sitter-textproto" ];
      nameOf   = d: builtins.head (builtins.match "([A-Za-z0-9_.-]+).*" d);
      stale    = builtins.filter
        (d: builtins.elem (nameOf d) internal && d != "${nameOf d}>=${pythonVersion}")
        (project.dependencies or [ ]);
    in if v != pythonVersion
       then throw "${dir}/pyproject.toml has version ${v}; the workspace is ${workspaceVersion} (Cargo.toml), so ${pythonVersion}"
       else if stale != [ ]
       then throw "${dir}/pyproject.toml: ${builtins.concatStringsSep ", " stale}: this repository's packages are required as <name>>=${pythonVersion}"
       else v;
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
    # No terminal variable: a test that depends on the color depth sets
    # it itself (spec 0410 S1), so the suite passes in nixpkgs's sandbox
    # as it does here.
  });

  # Common postInstall for both prototext variants.
  prototextPostInstall = whenRunnable ''
    # Install shell completions.
    installShellCompletion --cmd prototext \
      --bash <(PROTOTEXT_COMPLETE=bash $out/bin/prototext) \
      --zsh  <(PROTOTEXT_COMPLETE=zsh  $out/bin/prototext) \
      --fish <(PROTOTEXT_COMPLETE=fish $out/bin/prototext)

    # Generate and install man page (spec 0413 S3).
    PROTOTEXT_GEN_MAN=$out/share/man/man1 $out/bin/prototext

    # Spec 0402 S8, 0413 S5: every program says the version Nix calls it
    # (what versionCheckHook checks, for a derivation without its phase).
    for program in $out/bin/*; do
      "$program" --version | grep -qF ${prototextVersion} || {
        echo "$program --version does not print ${prototextVersion}" >&2
        exit 1
      }
    done
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
  # The three PyO3 extensions, whose wheels workspaceBuild builds.
  #
  #   crateName    — Cargo package name, e.g. "prototext_codec_lib"
  #   crateDir     — the crate directory, relative to the workspace root; the
  #                  stub generator runs with it as CARGO_MANIFEST_DIR
  #   libName      — cdylib base name (Cargo [[lib]] name), and the Python
  #                  package; the compiled module inside it has the same
  #                  name, which each pyproject.toml gives maturin and
  #                  pyo3-stub-gen as tool.maturin.module-name
  #                  (<libName>.<libName>; without it, the generator falls
  #                  back to the [project] name, and a stub refers to its
  #                  own classes under that wrong module name)
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
  #   $out/bin/{prototext,protolens,protoscan}
  #   $out/wheels/<dist>-<version>-cp313-cp313-<platform>.whl
  #
  # The wheels are maturin's (spec 0409 S3), built from each extension's
  # pyproject.toml as nixpkgs's maturinBuildHook builds them, on this
  # build's target directory, so maturin recompiles little.
  # - Linux: maturin checks the extension against manylinux_2_34, the
  #   newest glibc it needs (Rust's std links symbols up to GLIBC_2.34),
  #   and fails if it needs more. Tagged 2_28 until 2026-10-10, which let
  #   pip install the wheels where the import then failed.
  # - Darwin: the tag is the deployment target stdenv builds for
  #   (MACOSX_DEPLOYMENT_TARGET), which maturin reads.
  # - `env -u _PYTHON_HOST_PLATFORM`: nixpkgs's Python sets it to a
  #   linux-<arch> placeholder, which maturin would take as the platform tag.
  #
  # The stub generators run first, with CARGO_MANIFEST_DIR set so
  # pyo3-stub-gen finds pyproject.toml (the NotPresent panic of spec 0038
  # was a missing CARGO_MANIFEST_DIR). They write into target/stubs/, never
  # over the committed stub that maturin packages (spec 0409 S2), and a
  # stale committed stub fails the build.
  #
  # doInstallCargoArtifacts = false: nothing builds on top of it, and an
  # installed target.tar.zst (with `.prev` pointing at depsCache and the
  # vendored registry) would enter the closure of whatever copies from it
  # (spec 0374 S2). Crane's postInstall hook strips the vendored sources'
  # store paths rustc embeds as panic locations, for the same reason.
  # ---------------------------------------------------------------------------
  maturinArgs =
    if canRun
    then "-i ${pythonExecutable}"
    # Cross: maturin cannot run the host's interpreter; it knows CPython's
    # configuration per target.
    else "-i python${pythonPkgs.python.pythonVersion} --target ${pkgs.stdenv.hostPlatform.rust.rustcTarget}";
  manylinuxArgs = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux
    "--compatibility manylinux_2_34";

  workspaceBuild = crane.buildPackage (protocArgs // {
    src                                = workspaceSrc;
    pname                              = "prototools-workspace";
    nativeBuildInputs                  = protocArgs.nativeBuildInputs ++ [ pkgs.maturin ];
    cargoArtifacts                     = depsCache;
    doCheck                            = false;
    doInstallCargoArtifacts            = false;
    buildPhaseCargoCommand             = "cargoWithProfile build ${workspaceArgs}";
    doNotPostBuildInstallCargoBinaries = true;
    installPhaseCommand                = ''
      mkdir -p $out/bin $out/wheels
      # No protolens on the `upb` branch: it is out of the workspace until
      # its own move to upb (spec 0415 N1), and comes back here with it.
      cp ${profileDir}/prototext ${profileDir}/protoscan $out/bin/
    '' + pkgs.lib.concatMapStrings (e:
      let
        committed = "${e.crateDir}/${e.libName}/${e.libName}.pyi";
        generated = "target/stubs/${e.crateDir}/${e.libName}/${e.libName}/__init__.pyi";
      in whenRunnable ''
      CARGO_MANIFEST_DIR="$PWD/${e.crateDir}" ./${profileDir}/${e.postBuildBin}
      # The committed stub is what the wheel ships: it must be what the
      # generator writes. (A cross build cannot run the generator, and
      # relies on the native builds' comparison.)
      if ! diff -u ${committed} ${generated} >&2; then
        echo "${committed} is stale (diff above). Refresh it with:" >&2
        echo "  (cd ${e.crateDir} && CARGO_MANIFEST_DIR=\$PWD cargo run --release --bin ${e.postBuildBin}) && cp ${generated} ${committed}" >&2
        exit 1
      fi
    '' + ''
      env -u _PYTHON_HOST_PLATFORM maturin build --release --offline \
        -m ${e.crateDir}/Cargo.toml ${maturinArgs} ${manylinuxArgs} --out $out/wheels
    '') (builtins.attrValues pyo3Extensions);
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
    cp ${workspaceBuild}/bin/prototext $out/bin/
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
    description = "Terminal viewer to decode, navigate and extract binary protobuf messages";
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

  # `v`'s Neovim handoff (spec 0144 G5/G6) works with whatever is on PATH,
  # and says what is missing (spec 0410 S2). Our builds bundle a pinned
  # Neovim and `buf` (for `buf lsp serve`) anyway, so the key works out of
  # the box, in the workshop image above all; nixpkgs's recipe makes both
  # opt-in (spec 0410 S3). A separate derivation from the compile (spec 0374
  # S2), so a different Neovim costs a shell script, not a Rust build.
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
  # Each installs the wheel maturin built in workspaceBuild (spec 0409 S3):
  # the same wheel PyPI gets. nixpkgs builds the same pyproject.toml with
  # maturinBuildHook, one Rust build per package; here the three share
  # workspaceBuild's.
  # ---------------------------------------------------------------------------
  makePyo3Extension = e:
    let
      distName = builtins.replaceStrings [ "-" ] [ "_" ]
        (pkgs.lib.importTOML (../. + "/${e.crateDir}/pyproject.toml")).project.name;
    in pythonPkgs.buildPythonPackage {
      pname   = e.crateName;
      version = pyprojectVersion e.crateDir;
      format  = "wheel";
      src     = "${workspaceBuild}/wheels";
      # wheelUnpackPhase copies $src, a single wheel; this one is a
      # directory of three, each with its platform tag in its name. No
      # postUnpack, as in wheelUnpackPhase: its hooks expect a sourceRoot.
      unpackPhase = ''
        runHook preUnpack
        mkdir dist
        cp $src/${distName}-*.whl dist/
      '';
      pythonImportsCheck = [ e.libName ];
      meta = metaCommon // { inherit (e) description; };
    };

  prototextCodec    = makePyo3Extension pyo3Extensions.prototextCodec;
  fdpScanLib        = makePyo3Extension pyo3Extensions.fdpScan;
  prototextGraphLib = makePyo3Extension pyo3Extensions.prototextGraph;

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
}
