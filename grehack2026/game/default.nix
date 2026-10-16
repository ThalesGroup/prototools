# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

# grehack2026/game/default.nix — life-server, life-client and life-tap
# (spec 0375).
#
# A standalone Cargo project with its own Cargo.lock, excluded from the root
# workspace and subtracted from default.nix's workspaceSrc, like demo/bobapp
# (spec 0241 S1): tonic, hyper and tokio must not enter the workspace graph.
#
# build.rs runs protoc on proto/grehack/life/v1/life.proto; the binaries
# embed its FileDescriptorProto for protoscan to find (spec 0375 S3), and
# descriptor.proto's beside it for reproto (spec 0388 S13). The
# unit tests run as part of the build (crane's cargo test).
#
# life-tap runs dumpcap and tshark, so it is wrapped with wireshark-cli on
# its PATH and runs the same in the workshop image and on any Nix machine
# (spec 0375 S10). It still needs the capture privilege: root, or NixOS's
# programs.wireshark.
#
# Inputs:
#   pkgs   — the same nixpkgs pin as the root default.nix
#   crane  — the same crane as the root default.nix
#
# Output: $out/bin/{life-server,life-client,life-tap}

{ pkgs, crane }:

let
  # The source is rooted at the repository, not at life/, so life-server's
  # path dependency on prototext-core (spec 0377 S2) is reachable — the
  # workspace-not-at-source-root technique bobapp uses (spec 0241, its
  # default.nix). postUnpack enters life/ and sets sourceRoot=".", so cargo
  # resolves "../../prototext-core" and "../../prototext-core"'s own
  # "../workspace-hack" naturally.
  repoRoot = ../..;

  src = pkgs.lib.fileset.toSource {
    root    = repoRoot;
    fileset = pkgs.lib.fileset.unions [
      (crane.fileset.commonCargoSources (repoRoot + /grehack2026/game))
      (repoRoot + /grehack2026/game/proto)
      # Path dependencies of life, and their own path dependencies.
      (crane.fileset.commonCargoSources (repoRoot + /prototext-core))
      # prototext-core's own path dependency (spec 0415): its Cargo sources,
      # and the C it compiles, which commonCargoSources does not select.
      (crane.fileset.commonCargoSources (repoRoot + /prototext-upb))
      (repoRoot + /prototext-upb/vendor)
      (repoRoot + /prototext-upb/src/shim.c)
      # Their workspace root: prototext-core inherits its version from it
      # (spec 0405 S5). Cargo reads it for the inherited fields only; its
      # `exclude` keeps this crate a separate project.
      (repoRoot + /Cargo.toml)
      (crane.fileset.commonCargoSources (repoRoot + /workspace-hack))
    ];
  };

  commonArgs = {
    inherit src;
    pname   = "life";
    version = "0.1.0";
    strictDeps = true;
    # prost-build runs protoc; PROTOC names it rather than relying on PATH.
    nativeBuildInputs = [ pkgs.protobuf pkgs.makeWrapper ];
    PROTOC = "${pkgs.protobuf}/bin/protoc";

    cargoLock  = ./Cargo.lock;
    cargoToml  = ./Cargo.toml;
    postUnpack = ''
      cd $sourceRoot/grehack2026/game
      sourceRoot="."
    '';
  };

  depsCache = crane.buildDepsOnly commonArgs;

in crane.buildPackage (commonArgs // {
  cargoArtifacts = depsCache;

  postInstall = ''
    wrapProgram $out/bin/life-tap \
      --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.wireshark-cli ]}
    # life-client runs every command it receives through the covert channel
    # (specs 0384, 0385), and every command Bob types after `s` (spec 0380),
    # as `sh -c <command>`. Wrap `bash` (for `sh`) onto the client's PATH,
    # like wireshark-cli on life-tap's, so a command resolves the same in the
    # workshop image and on any Nix machine, whatever the login shell's PATH.
    # `fortune` rides along as a harmless command to send through the
    # channel; nothing runs it on its own since spec 0385 dropped the number
    # 42 special case. The wrapper makes $out/bin/life-client a script: the
    # binary, with its embedded descriptors, is .life-client-wrapped.
    wrapProgram $out/bin/life-client \
      --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.bash pkgs.fortune ]}
  '';

  meta = {
    description = "Game of life over cleartext gRPC, and a tap on its traffic (GreHack 2026)";
    license     = pkgs.lib.licenses.mit;
    mainProgram = "life-client";
  };
})
