# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

# nix/pypi.nix — the per-platform .whl files for PyPI publishing.
#
# Produces $out/ with one .whl per package:
#   prototext_graph-<v>-cp313-cp313-<platform>.whl   (binary, PyO3, dist: prototext-graph)
#   prototext_codec-<v>-cp313-cp313-<platform>.whl   (binary, PyO3, dist: prototext-codec)
#   fdp_scan-<v>-cp313-cp313-<platform>.whl          (binary, PyO3, dist: fdp-scan)
#   prototext_reproto-<v>-py3-none-any.whl           (pure Python, dist: prototext-reproto)
# The binary wheels are maturin's, built in workspaceBuild (spec 0409 S3),
# which also chooses and checks their platform tags (see there).
# protoscan is a Rust binary since spec 0407: no wheel, crates.io only.
#
# The derivation does not publish — publish.sh is assembled by the CI
# assemble job (spec 0098 S6).

{ pkgs
, pythonPkgs
, pyprojectVersion   # nix/rust.nix: a pyproject.toml's version, checked (spec 0405 S5)
, workspaceBuild     # nix/rust.nix: $out/wheels/ holds the extensions' wheels
, reprotoSrc         # reproto's source (from nix/python.nix)
}:

let
  # ---------------------------------------------------------------------------
  # makeWheel — helper that zips up a pre-staged $wheelDir into a .whl file.
  #
  # A wheel is simply a zip archive.  The directory layout must contain:
  #   {dist_name}-{version}.dist-info/  (WHEEL, METADATA, RECORD)
  #   <package source tree>
  #
  # We rely on `python -m wheel pack` which rebuilds a .whl from a directory
  # that was previously unpacked by `python -m wheel unpack`, or we use the
  # lower-level zip approach to avoid requiring wheel itself.
  # ---------------------------------------------------------------------------

  # makeDistInfo name version pyTag abiTag plTag — write the dist-info
  # directory into $PWD, then generate the RECORD file covering all files
  # already present in $PWD (must be called after all package files are staged).
  # Returns a shell fragment (used inside buildPhase).
  #
  # RECORD format: path,sha256:<base64url>,<size>  (one line per file)
  # The RECORD entry itself has an empty hash and size.
  makeDistInfoShell = name: version: pyTag: abiTag: plTag:
    # PEP 427: dist-info directory and wheel filename use the normalized
    # (underscored) name; METADATA Name keeps the canonical dashed form.
    let normName = builtins.replaceStrings ["-"] ["_"] name; in ''
    DI="${normName}-${version}.dist-info"
    mkdir -p "$DI"

    cat > "$DI/WHEEL" <<EOF
Wheel-Version: 1.0
Generator: nix-pypi (prototools)
Root-Is-Purelib: true
Tag: ${pyTag}-${abiTag}-${plTag}
EOF

    cat > "$DI/METADATA" <<EOF
Metadata-Version: 2.1
Name: ${name}
Version: ${version}
EOF

    # Generate RECORD: sha256 base64url hash + size for every file, then
    # a bare entry (no hash/size) for RECORD itself.
    (
      find . -type f ! -name RECORD | sort | while read -r f; do
        f=''${f#./}
        hash=$(openssl dgst -sha256 -binary "$f" | base64 | tr '+/' '-_' | tr -d '=')
        size=$(wc -c < "$f")
        echo "$f,sha256:$hash,$size"
      done
      echo "$DI/RECORD,,"
    ) > "$DI/RECORD"
  '';

  # ---------------------------------------------------------------------------
  # Pure Python wheel helper.
  #
  # Takes:
  #   pkgName  — PyPI distribution name, e.g. "reproto"
  #   version  — version string
  #   src      — path to the package source tree (contains src/<pkg>/)
  #   pkgDir   — bare name of the Python package dir under src/, e.g. "reproto"
  # ---------------------------------------------------------------------------
  makePureWheel = { pkgName, version, src, pkgDir }:
    pkgs.runCommand "${pkgName}-whl" {
      strictDeps = true;
      nativeBuildInputs = [ pkgs.zip pkgs.openssl pythonPkgs.python ];
    } ''
      set -euo pipefail
      WORK=$(mktemp -d)
      cd "$WORK"

      # Copy the Python package tree.
      cp -r "${src}/src/${pkgDir}" .

      ${makeDistInfoShell pkgName version "py3" "none" "any"}

      mkdir -p "$out"
      NORM_NAME=$(echo "${pkgName}" | tr '-' '_')
      WHL="$NORM_NAME-${version}-py3-none-any.whl"
      zip -r "$out/$WHL" "${pkgDir}" "$DI"
    '';


  # Spec 0402 S3: each wheel's version from its own pyproject.toml.
  versionOf = pyprojectVersion;

in pkgs.runCommand "prototools-pypi" {
  strictDeps = true;
  nativeBuildInputs = [ pkgs.zip ];
} ''
  set -euo pipefail
  mkdir -p "$out"

  cp ${workspaceBuild}/wheels/*.whl "$out/"

  cp ${makePureWheel {
    pkgName = "prototext-reproto";
    version = versionOf "reproto";
    src     = reprotoSrc;
    pkgDir  = "reproto";
  }}/*.whl "$out/"
''
