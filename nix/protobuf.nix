# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

# nix/protobuf.nix — the one protobuf release (spec 0460 S1).
#
# Every copy of descriptor.proto and the other well-known types comes from
# it: the vendored upb (prototext-upb/vendor, by scripts/update-upb), the
# variant's well-known types reproto and protoback fill in, prototext's
# prebuilt fixtures and WKT graph; and so do the protoc the builds and
# tests run and Python protobuf. default.nix's checks fail `ci` when a copy
# is from another release (S2 to S4).
#
# The main pin (nixos-26.05) stops at protobuf 34. Until it has this
# release:
#   - protoc is the nixos-unstable pin's protobuf_<major>: a tool, which
#     nothing links against, and which is in the binary cache. That pin
#     moves (scripts/update-unstable-pin): the assertion below fails when
#     it brings another release, rather than letting the copies drift.
#   - Python protobuf is the main pin's recipe, its version and source
#     moved to the matching 7.x, built against that protoc: one Python
#     interpreter, whose `withPackages` and package set agree.

{ pkgs, unstablePkgs }:

let
  release = "36.2";

  protoc = unstablePkgs.protobuf_36;

  # Python protobuf's version for protobuf `release`: 7.<release>.
  pythonVersion = "7.${release}";

  # nixpkgs' default Python, which moves with the pin (spec 0462 S2).
  python = pkgs.python3.override {
    self = python;
    packageOverrides = pyfinal: pyprev: {
      protobuf = (pyprev.protobuf.override { protobuf = protoc; }).overridePythonAttrs (_: {
        version = pythonVersion;
        src = pyfinal.fetchPypi {
          pname = "protobuf";
          version = pythonVersion;
          hash = "sha256-SX0EY/8zFmgdpsC56NBstGXWGrzgC2E6tCImF1ZE0bs=";
        };
      });
    };
  };
in
assert pkgs.lib.assertMsg (protoc.version == release)
  "nix/protobuf.nix: protoc is ${protoc.version}, the release is ${release}: move the release (spec 0460), or the unstable pin back";
{
  inherit release protoc python;
  pythonPkgs = python.pkgs;
}
