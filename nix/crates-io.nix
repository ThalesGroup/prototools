# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

# nix/crates-io.nix — Package the publishable .crate tarballs.
#
# Produces $out/ with one tarball per publishable workspace crate:
#   tree-sitter-textproto, prototext-upb, prototext-core, prototext-graph, prototext-schema,
#   prototools-complete, prototext, fdp-scan, protoscan, prototext-lens
# (that is also their dependency order: the list scripts/publishable-crates
# gives the release scripts, which this one must match — spec 0412 S3).
#
# cargo package --no-verify assembles source tarballs without compiling.
# External dependencies are vendored via crane.vendorCargoDeps so that
# Cargo can resolve them without network access inside the Nix sandbox.
#
# Workspace-internal path dependencies also need to be resolvable when
# cargo validates the packaged manifest.
# We inject a [patch.crates-io] section into CARGO_HOME/config.toml
# pointing at the local workspace paths so that cargo can satisfy version
# requirements for these not-yet-published crates.
#
# HOME and CARGO_HOME are set to writable temp dirs because Cargo writes
# its registry cache there; the real paths are inaccessible inside the
# Nix sandbox.
#
# This is a packaging check on main. A release is packaged, verified and
# published by .github/workflows/publish.yaml (spec 0412).

{ pkgs
, crane
, workspaceSrc
, commonArgs    # from nix/rust.nix — carries PYO3_PYTHON, etc.
}:

let
  # Vendor all external Cargo deps from the workspace Cargo.lock so that
  # cargo can resolve them without network access.
  vendoredDeps = crane.vendorCargoDeps { src = workspaceSrc; };

in pkgs.stdenv.mkDerivation (commonArgs // {
  pname   = "prototools-crates-io";
  # Spec 0402 S3: prototext's release, the crate the bundle is named for.
  version = (pkgs.lib.importTOML ../Cargo.toml).workspace.package.version;
  src     = workspaceSrc;

  nativeBuildInputs = commonArgs.nativeBuildInputs ++ [
    pkgs.protobuf
    crane.configureCargoVendoredDepsHook
  ];
  cargoVendorDir = vendoredDeps;

  # HOME and CARGO_HOME must be writable before configureCargoVendoredDepsHook
  # runs (it writes $CARGO_HOME/config.toml).
  preConfigure = ''
    export HOME=$(mktemp -d)
    export CARGO_HOME=$(mktemp -d)
  '';

  # After the vendored deps hook sets up the registry replacement, inject
  # [patch.crates-io] entries for workspace-local crates so cargo can satisfy
  # their version requirements during packaging validation without needing them
  # in the vendor directory.
  postConfigure = ''
    cat >> "$CARGO_HOME/config.toml" <<EOF

[patch.crates-io]
tree-sitter-textproto = { path = "$PWD/reproto/tree-sitter-textproto" }
prototext-core   = { path = "$PWD/prototext-core" }
prototext-graph  = { path = "$PWD/prototext-graph" }
prototext-schema = { path = "$PWD/prototext-schema" }
prototools-complete = { path = "$PWD/prototools-complete" }
prototext        = { path = "$PWD/prototext" }
fdp-scan         = { path = "$PWD/fdp-scan" }
workspace-hack   = { path = "$PWD/workspace-hack" }
EOF
  '';

  # In dependency order, so this list doubles as the publish order.
  buildPhase = ''
    cargo package -p tree-sitter-textproto --no-verify
    cargo package -p prototext-upb    --no-verify
    cargo package -p prototext-core   --no-verify
    cargo package -p prototext-graph  --no-verify
    cargo package -p prototext-schema --no-verify
    cargo package -p prototools-complete --no-verify
    cargo package -p prototext        --no-verify
    cargo package -p fdp-scan         --no-verify
    cargo package -p protoscan        --no-verify
    cargo package -p prototext-lens   --no-verify
  '';

  installPhase = ''
    mkdir -p $out
    cp target/package/tree-sitter-textproto-*.crate $out/
    cp target/package/prototext-upb-*.crate    $out/
    cp target/package/prototext-core-*.crate   $out/
    cp target/package/prototext-graph-*.crate  $out/
    cp target/package/prototext-schema-*.crate $out/
    cp target/package/prototools-complete-*.crate $out/
    cp target/package/prototext-*.crate        $out/
    cp target/package/fdp-scan-*.crate         $out/
    cp target/package/protoscan-*.crate        $out/
    cp target/package/prototext-lens-*.crate   $out/
  '';

  # No tests to run — cargo package --no-verify handles validation.
  doCheck = false;
})
