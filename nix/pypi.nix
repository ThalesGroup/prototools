# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

# nix/pypi.nix — the per-platform .whl files for PyPI publishing.
#
# Produces $out/ with one .whl per package, each built by its own build
# backend from its pyproject.toml (spec 0411 S4):
#   prototext_codec-<v>-cp313-cp313-<platform>.whl        (maturin, dist: prototext-codec)
#   prototext_graph-<v>-cp313-cp313-<platform>.whl        (maturin, dist: prototext-graph)
#   fdp_scan-<v>-cp313-cp313-<platform>.whl               (maturin, dist: fdp-scan)
#   tree_sitter_textproto-<v>-cp313-cp313-<platform>.whl  (setuptools, dist: tree-sitter-textproto)
#   prototext_reproto-<v>-py3-none-any.whl                (setuptools, dist: prototext-reproto)
# The extensions' wheels come from workspaceBuild (spec 0409 S3), which
# chooses and checks their platform tags; the other two are the `dist`
# outputs of the packages nix/python.nix builds.
# protoscan is a Rust binary since spec 0407: no wheel, crates.io only.
#
# The derivation does not publish — publish.sh is assembled by the CI
# assemble job (spec 0098 S6).

{ pkgs
, pythonPkgs
, workspaceBuild          # nix/rust.nix: $out/wheels/ holds the extensions' wheels
, reproto                 # nix/python.nix: the package; its `dist` holds its wheel
, treeSitterTextprotoPkg  # nix/python.nix: the grammar; its `dist` holds its wheel
}:

let
  # setuptools tags a Linux wheel linux_<arch>, which PyPI refuses. The
  # grammar's extension links libc alone, so the manylinux tag the
  # extensions carry fits it if it needs no newer glibc symbol than theirs:
  # checked here, then retagged. (On darwin, setuptools already tags the
  # deployment target, macosx_14_0_arm64.)
  manylinux = "manylinux_2_34";
  retagLinux = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
    for whl in tree_sitter_textproto-*-linux_*.whl; do
      mkdir unpacked
      python -m zipfile -e "$whl" unpacked
      newest=$(objdump -T unpacked/tree_sitter_textproto/*.so \
        | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -V | tail -1)
      if [ "$(printf '%s\n' "$newest" 2.34 | sort -V | tail -1)" != 2.34 ]; then
        echo "$whl needs GLIBC_$newest, newer than ${manylinux} allows" >&2
        exit 1
      fi
      rm -r unpacked
      python -m wheel tags --remove \
        --platform-tag ${manylinux}_${pkgs.stdenv.hostPlatform.uname.processor} "$whl"
    done
  '';
in pkgs.runCommand "prototools-pypi" {
  strictDeps = true;
  nativeBuildInputs = [
    (pythonPkgs.python.withPackages (ps: [ ps.wheel ]))
    pkgs.binutils
  ];
} ''
  set -euo pipefail
  mkdir -p "$out"
  cd "$out"
  cp ${workspaceBuild}/wheels/*.whl \
     ${reproto.dist}/*.whl \
     ${treeSitterTextprotoPkg.dist}/*.whl .
  chmod u+w *.whl
  ${retagLinux}
''
