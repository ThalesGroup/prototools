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
#            ├──▶  protolens           (copied out, completions, man page)
#            └──▶  prototextCodec, fdpScanLib, prototextGraphLib  (maturin's wheels)
#
# Every shipped crate embeds the WKT scoring graph committed under
# prototext/wkt/prebuilt/ (S2); default.nix's wkt-prebuilt-check keeps that
# copy equal to what the code generates (S3). Nothing here waits for the
# tests (S4).

{ pkgs
, protoc            # nix/protobuf.nix: the one protobuf release (spec 0460)
, crane
, pythonPkgs
, pythonBin
, pythonExecutable
, depsSrc
, workspaceSrc
, buf               # narrow-pinned buf (newer than the main nixpkgs pin's 1.59.0; see default.nix)
, cargoProfile      # default.nix: "release" (what ships) or "quick" (spec 0485)
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
  # protocArgs: extends commonArgs with the release's protoc (spec 0462).
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
  # and an older lower bound would let pip pair a new package with old
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

  # maturin's PYO3_ENVIRONMENT_SIGNATURE, which it sets when it runs Cargo:
  # <implementation>-<major>.<minor>-<pointer width>bit, for the Python and
  # the platform the extension runs on. Set for every Cargo run that
  # compiles pyo3 (commonArgs, the dev-shell), or pyo3-build-config's build
  # script, which depends on it, reruns in maturin's step and pyo3's crates
  # recompile (spec 0477 S3). workspaceBuild fails if maturin compiles
  # any crate but the extension's (S5).
  pyo3EnvironmentSignature =
    "${pythonPkgs.python.implementation}-${pythonPkgs.python.pythonVersion}-"
    + "${toString pkgs.stdenv.hostPlatform.parsed.cpu.bits}bit";

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
    env.PYO3_ENVIRONMENT_SIGNATURE = pyo3EnvironmentSignature;
  };

  # protoc for prototext's roundtrip tests, which compare with it. The
  # fixtures prototext's build.rs reads are committed (spec 0405 S1).
  protocArgs = commonArgs // {
    nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ protoc ];
  };

  # ---------------------------------------------------------------------------
  # Shared dependency cache — built once, reused by all Crane derivations.
  # Uses protocArgs (includes protoc) so the sandbox environment matches
  # all consumers. buildDepsOnly stubs build.rs so protoc is never invoked, but
  # having protobuf present prevents fingerprint mismatches that would force
  # external deps to recompile in every downstream derivation.
  # No protoc run: dummy build.rs never calls it.
  # ---------------------------------------------------------------------------
  # The profile of what builds the tools (spec 0485 S1): crane's default,
  # `release`, unless default.nix's cargoProfile says `quick`. Set only
  # then, so that the default's derivations are today's, byte for byte
  # (G2).
  profileArgs = pkgs.lib.optionalAttrs (cargoProfile != "release") {
    CARGO_PROFILE = cargoProfile;
  };

  depsCache = crane.buildDepsOnly (protocArgs // profileArgs // {
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

  # On depsCache, so in its profile: another would rebuild every dependency.
  rustClippy = crane.cargoClippy (protocArgs // profileArgs // {
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
  # dependency cache, since a profile is part of every artifact's identity;
  # under cargoProfile = "quick", depsCache is that cache already, and the
  # tests take it (spec 0485 S2): one dependency build fewer.
  depsCacheTests = if cargoProfile == "quick" then depsCache else crane.buildDepsOnly (protocArgs // {
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
    # git-seriate's end-to-end tests check every tree they make with
    # `git fsck --strict` (spec 0452 G5), and sign with ssh-keygen and an
    # ssh-agent of their own (spec 0463); without them, they skip.
    nativeBuildInputs = protocArgs.nativeBuildInputs ++ [ pkgs.gitMinimal pkgs.openssh ];
    CARGO_PROFILE  = "quick";
    cargoArtifacts = depsCacheTests;
    cargoExtraArgs = workspaceArgs;
    # No terminal variable: a test that depends on the color depth sets
    # it itself (spec 0410 S1), so the suite passes in nixpkgs's sandbox
    # as it does here.
  });

  # The committed prototext-upb/src/descriptor_access.rs is what
  # vendor/descriptor.pb generates (spec 0424 S6): a stale file fails ci,
  # as a stale pyo3 stub fails workspaceBuild. A check of the repository,
  # not of the software: the test is #[ignore]d, so a downstream
  # `cargo test` (nixpkgs' check phase, a crates.io user) skips it and
  # needs no rustfmt. It runs here, with the toolchain's rustfmt, which
  # lays the generated code out as rustFmt checks it.
  descriptorAccessCheck = crane.cargoTest (protocArgs // {
    src                = workspaceSrc;
    pname              = "prototext-upb-descriptor-access";
    CARGO_PROFILE      = "quick";
    cargoArtifacts     = depsCacheTests;
    cargoExtraArgs     = "-p prototext-upb --test descriptor_access";
    cargoTestExtraArgs = "-- --ignored";
    nativeBuildInputs  = protocArgs.nativeBuildInputs ++ [ pkgs.rustfmt ];
  });

  # workspace-hack is what cargo-hakari generates, every member depends on
  # it, and it makes their features agree: otherwise a build of one crate
  # (maturin's of each wheel, a nixpkgs recipe's) compiles its dependencies
  # again with other features than the workspace build did. A dependency
  # or feature changed in any Cargo.toml makes it stale: this fails `ci`
  # then, with the command that refreshes it. Offline, on the vendored
  # dependencies: hakari reads `cargo metadata`, and builds nothing.
  hakariCheck = crane.mkCargoDerivation (commonArgs // {
    src                     = workspaceSrc;
    pname                   = "prototools-hakari-check";
    cargoArtifacts          = null;
    doInstallCargoArtifacts = false;
    nativeBuildInputs       = commonArgs.nativeBuildInputs ++ [ pkgs.cargo-hakari ];
    buildPhaseCargoCommand  = ''
      if ! cargo hakari generate --diff || ! cargo hakari manage-deps --dry-run \
          || ! cargo hakari verify; then
        echo "workspace-hack is stale. Refresh it with:" >&2
        echo "  cargo hakari generate && cargo hakari manage-deps" >&2
        exit 1
      fi
    '';
    installPhaseCommand = "touch $out";
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
    then "target/${cargoProfile}"
    else "target/${pkgs.stdenv.hostPlatform.rust.rustcTarget}/${cargoProfile}";
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
  # every binary and every extension's rlib, the stub generators included,
  # and maturin each wheel's module from them, so everything that ships is
  # copied out of it:
  #
  #   $out/bin/{prototext,protolens,protoscan}
  #   $out/wheels/<dist>-<version>-cp313-cp313-<platform>.whl
  #
  # The wheels are maturin's (spec 0409 S3), built from each extension's
  # pyproject.toml as nixpkgs's maturinBuildHook builds them, on this
  # build's target directory. Each maturin run compiles only its
  # extension's crate, as the module (spec 0477): the extensions are rlibs
  # only, so the build phase compiles their dependencies as maturin does
  # under thin LTO (a cdylib and an rlib made it compile them once more, as
  # bitcode only, about 130 crates); and PYO3_ENVIRONMENT_SIGNATURE is
  # set for both (commonArgs).
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
  # What maturin may compile beside the extension's own crate (spec 0477
  # S5, S8): tree-sitter-highlight (protolens) is a staticlib too, so the
  # workspace build compiles regex's tree as machine code, which maturin's
  # one extension, without it, compiles as LTO bitcode again; and the crates
  # above them. The first wheel compiles them, the others reuse them.
  maturinResidual = [
    "aho-corasick" "bstr" "fdp-scan" "globset" "memchr" "prototext"
    "prototext-core" "prototext-graph" "prototext-schema" "prototext-upb"
    "prototools-complete" "pyo3-stub-gen" "regex" "regex-automata"
    "regex-syntax" "rustpython-ast" "rustpython-parser"
    "rustpython-parser-core" "rustpython-parser-vendored" "serde_json"
    "winnow" "workspace-hack"
  ];

  manylinuxArgs = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux
    "--compatibility manylinux_2_34";

  # maturin's spelling of the profile: `--release` as before for the
  # default (G2).
  maturinProfile =
    if cargoProfile == "release" then "--release" else "--profile ${cargoProfile}";

  workspaceBuild = crane.buildPackage (protocArgs // profileArgs // {
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
      cp ${profileDir}/prototext \
         ${profileDir}/protolens ${profileDir}/protoscan \
         ${profileDir}/protoback ${profileDir}/git-seriate \
         ${profileDir}/git-seriate-fdp $out/bin/
    '' + pkgs.lib.concatMapStrings (e:
      let
        committed = "${e.crateDir}/${e.libName}/${e.libName}.pyi";
        generated = "target/stubs/${e.crateDir}/${e.libName}/${e.libName}/__init__.pyi";
      in whenRunnable ''
      CARGO_MANIFEST_DIR="$PWD/${e.crateDir}" ./${profileDir}/${e.postBuildBin}
      # The committed stub is what the wheel ships: it must be what the
      # generator writes, without the ruff header it cannot be told to
      # leave out (scripts/strip-stub-header). (A cross build cannot run
      # the generator, and relies on the native builds' comparison.)
      sh scripts/strip-stub-header < ${generated} > "$TMPDIR/${e.libName}.pyi"
      if ! diff -u ${committed} "$TMPDIR/${e.libName}.pyi" >&2; then
        echo "${committed} is stale (diff above). Refresh it with:" >&2
        echo "  scripts/refresh-stubs" >&2
        exit 1
      fi
    '' + ''
      set -o pipefail
      env -u _PYTHON_HOST_PLATFORM maturin build ${maturinProfile} --offline \
        -m ${e.crateDir}/Cargo.toml ${maturinArgs} ${manylinuxArgs} --out $out/wheels \
        2>&1 | tee "$TMPDIR/maturin-${e.libName}.log"
      # maturin compiles the extension's crate and maturinResidual alone
      # (spec 0477 S5): any other crate compiled again means the build
      # phase and maturin no longer build it alike (a maturin that changed
      # its PYO3_ENVIRONMENT_SIGNATURE convention, a cdylib declared again,
      # a feature workspace-hack no longer unifies).
      again=$(grep -E '^ *Compiling ' "$TMPDIR/maturin-${e.libName}.log" \
        | grep -vE '^ *Compiling (${pkgs.lib.concatStringsSep "|"
            ([ e.crateName ] ++ maturinResidual)}) ' || true)
      if [ -n "$again" ]; then
        echo "maturin compiled these again for ${e.crateDir} (spec 0477 S5):" >&2
        echo "$again" >&2
        echo "Why: rebuild with CARGO_LOG=cargo::core::compiler::fingerprint=info." \
             "PYO3_ENVIRONMENT_SIGNATURE is '$PYO3_ENVIRONMENT_SIGNATURE' here," \
             "maturin's is ${pyo3EnvironmentSignature} by our reckoning." \
             "A dependency change that legitimately moves the residual:" \
             "update maturinResidual (nix/rust.nix)." >&2
        exit 1
      fi
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
  # protoback — reproto's successor, in Rust, on upb (spec 0424). Not
  # published on crates.io; exported as `protoback` for downstream builds.
  # Its
  # completion and man pages come from the binary itself, as protoscan's
  # (spec 0433 S7).
  # ---------------------------------------------------------------------------
  protoback = pkgs.runCommand "protoback-${workspaceVersion}" {
    nativeBuildInputs = [ pkgs.installShellFiles ];
    meta = metaCommon // {
      description = "Reconstruct .proto sources and schema databases from protobuf descriptors";
      mainProgram = "protoback";
    };
  } (''
    mkdir -p $out/bin
    cp ${workspaceBuild}/bin/protoback $out/bin/
  '' + whenRunnable ''
    installShellCompletion --cmd protoback \
      --bash <(PROTOBACK_COMPLETE=bash $out/bin/protoback) \
      --zsh  <(PROTOBACK_COMPLETE=zsh  $out/bin/protoback) \
      --fish <(PROTOBACK_COMPLETE=fish $out/bin/protoback)
    PROTOBACK_GEN_MAN=$out/share/man/man1 $out/bin/protoback
    $out/bin/protoback --version | grep -qF ${workspaceVersion}
  '');

  # ---------------------------------------------------------------------------
  # git-seriate and git-seriate-fdp — undated snapshots of files stored in
  # git, and one linear history rebuilt from them (specs 0450, 0452, 0453).
  # git runs each as `git seriate` and `git seriate-fdp`; their completions
  # register against the executables, and their man pages are named so that
  # `git help seriate` finds them. Not published on crates.io yet.
  # ---------------------------------------------------------------------------
  mkGitSeriate = { exe, env, description }:
    pkgs.runCommand "${exe}-${workspaceVersion}" {
      nativeBuildInputs = [ pkgs.installShellFiles ];
      meta = metaCommon // {
        inherit description;
        mainProgram = exe;
      };
    } (''
      mkdir -p $out/bin
      cp ${workspaceBuild}/bin/${exe} $out/bin/
    '' + whenRunnable ''
      installShellCompletion --cmd ${exe} \
        --bash <(${env}_COMPLETE=bash $out/bin/${exe}) \
        --zsh  <(${env}_COMPLETE=zsh  $out/bin/${exe}) \
        --fish <(${env}_COMPLETE=fish $out/bin/${exe})
      ${env}_GEN_MAN=$out/share/man/man1 $out/bin/${exe}
      $out/bin/${exe} --version | grep -qF ${workspaceVersion}
    '');

  gitSeriate = mkGitSeriate {
    exe = "git-seriate";
    env = "GIT_SERIATE";
    description = "Store undated snapshots of files in git, and rebuild one linear history from them";
  };

  gitSeriateFdp = mkGitSeriate {
    exe = "git-seriate-fdp";
    env = "GIT_SERIATE_FDP";
    description = "git-seriate for the protobuf descriptors embedded in executables";
  };

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
  '';

  protolensMeta = metaCommon // {
    description = "Terminal viewer to decode, navigate and extract binary protobuf messages";
    mainProgram = "protolens";
  };

  # The binary, its completions and man page, copied out of workspaceBuild.
  # No wrapper: `v` opens the user's editor (spec 0420), so the package
  # bundles no editor and no editor configuration; the demos bring theirs.
  protolens = pkgs.runCommand "protolens-${protolensVersion}" {
    nativeBuildInputs = [ pkgs.installShellFiles ];
    meta              = protolensMeta;
    passthru.tests    = { inherit rustTests; };
  } ''
    mkdir -p $out/bin
    cp ${workspaceBuild}/bin/protolens $out/bin/
    ${protolensPostInstall}
  '';

  # A lean Neovim for the demos and the workshop image (spec 0374 S2): no
  # clipboard (there is no desktop clipboard in a container; wl-clipboard
  # also brings perl), and no Ruby or Python providers, which the demo
  # config — pure Lua and Neovim's built-in LSP client — never uses. 98 MiB instead of 407 MiB, and
  # no compilation: `wrapNeovimUnstable` only wraps the shared neovim-unwrapped.
  # (`pkgs.neovim.override { waylandSupport = false; }` does not evaluate: the
  # argument belongs to wrapNeovimUnstable, not to the `neovim` package.)
  #
  # wrapRc = false: the default wrapper exports VIMINIT pointing at a generated
  # (here empty) init.lua, which would override both ways the demos give it
  # their config: VIMINIT in their environment (spec 0420 S8), and
  # $XDG_CONFIG_HOME/nvim/init.lua in teleprompt's wrapper (spec 0395).
  neovimLean = pkgs.wrapNeovimUnstable pkgs.neovim-unwrapped {
    wrapRc         = false;
    waylandSupport = false;
    withRuby       = false;
    withPython3    = false;
  };
  # buf for the demos and the image (spec 0375 S9): its `buf` binary alone,
  # copied rather than linked so the full package leaves the closure. The
  # demo config runs only `buf lsp serve`; the two protoc-gen-buf-* plugins
  # are 72 MiB unpacked.
  bufLean = pkgs.runCommand "buf-lean-${buf.version}" { } ''
    install -Dm755 ${buf}/bin/buf $out/bin/buf
  '';

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
    pyo3EnvironmentSignature
    depsCache
    rustFmt
    rustClippy
    rustTests
    descriptorAccessCheck hakariCheck
    workspaceBuild
    prototext
    protoscan
    protoback
    gitSeriate
    gitSeriateFdp
    protolens
    neovimLean
    bufLean
    prototextCodec
    fdpScanLib
    prototextGraphLib;
}
