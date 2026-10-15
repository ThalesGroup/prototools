# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

# nix/grpconf2026-demo.nix — what the gRPConf 2026 talk needs beyond the
# prototools, defined once for its native demo shell and the workshop image
# (spec 0404 S1), as nix/grehack2026-demo.nix does for GreHack.
#
#   demoTools — the teleprompt and the commands the deck calls directly
#   demoEnv   — environment both set the same way
#   deck      — the deck's committed material, with bob/ staged in
{ pkgs
, deckSrc         # ./grpconf2026, as a path: filtered below
, teleprompt      # the one teleprompt both talks share (grehack2026-demo.nix)
, neovimLean
, bufLean
, wktDb
, googleapisDb
, googleapisPbs
, grpconfDemo     # bobapp, the capture and the log (default.nix)
}:

let
  lib = pkgs.lib;

  # The committed material (spec 0404 S1). speech.md, the speaker notes,
  # artifacts.md and the fixture minting are the presenter's, not the
  # audience's (N3).
  material = lib.fileset.toSource {
    root    = deckSrc;
    fileset = lib.fileset.unions [
      (deckSrc + "/grpconf2026.sh")
      (deckSrc + "/grpconf2026.init")
      (deckSrc + "/anomalies.pb")
      (deckSrc + "/anomalies.script")
      (deckSrc + "/beats")
    ];
  };

  wktSet = "${wktDb}/share/prototools/wkt.desc";
in
{
  # The deck calls nothing beyond bash, coreutils and these (spec 0404 S4):
  # the teleprompt, nvim and buf for `view`, and protoc.
  demoTools = [ teleprompt neovimLean bufLean pkgs.protobuf ];

  # Spec 0404 S1. PROTOTEXT_WKT_SET is what annex C names; the dev-shell
  # exports it too (nix/shells.nix).
  demoEnv = {
    PROTOTEXT_DESCRIPTOR_SET = wktSet;
    PROTOTEXT_WKT_SET        = wktSet;
    PROTOTEXT_GOOGLEAPIS_SET = "${googleapisDb}/googleapis.desc";
    PROTOTEXT_GOOGLEAPIS_PBS = "${googleapisPbs}/googleapis.pb";
    # As the GreHack demo's (spec 0420 S8): the same Neovim config.
    EDITOR  = "nvim";
    VIMINIT = "source ${./demo-nvim/init.lua}";
  };

  # The material plus Bob's files, as the deck expects them: bob/app,
  # bob/capture, bob/logfile. The image copies this whole; the native shell
  # stages bob/ from it (spec 0404 S2).
  deck = pkgs.runCommand "grpconf2026-deck" { } ''
    cp -r ${material} $out
    chmod u+w $out
    mkdir $out/bob
    cp ${grpconfDemo}/bin/bobapp $out/bob/app
    cp ${grpconfDemo}/capture    $out/bob/capture
    cp ${grpconfDemo}/logfile    $out/bob/logfile
  '';
}
