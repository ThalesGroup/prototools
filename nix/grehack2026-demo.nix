# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

# nix/grehack2026-demo.nix — what the GreHack 2026 talk needs beyond the
# prototools, defined once for the native demo shell and the workshop image
# (spec 0403 G2). Neither consumer adds to it.
#
#   teleprompt — bin/teleprompt, wrapped with what it calls
#   demoTools  — the teleprompt and the commands the deck calls directly
#   demoEnv    — environment both set the same way
#   deck       — the deck's committed material, as the image copies it
{ pkgs
, telepromptSrc   # ./bin/teleprompt, as a path: imported into the store
, deckSrc         # ./grehack2026, as a path: filtered below
, neovimLean      # the image's Neovim (spec 0374 S2)
, bufLean         # the image's buf (spec 0375 S9)
, wktDb
}:

let
  lib = pkgs.lib;

  # Neovim config for `view` / `view_textproto` / `view_proto`: desert theme,
  # proto and textproto highlighting (spec 0395 fix). The dev-shell writes the
  # same highlighting through _hook_nvim; the demo shells no longer run that
  # hook (spec 0394), so teleprompt carries its own config under XDG_CONFIG_HOME.
  telepromptNvimConfig = pkgs.runCommand "teleprompt-nvim-config" { } ''
    mkdir -p $out/nvim
    cp ${./demo-nvim/init.lua} $out/nvim/init.lua
  '';

  # The banners' font (spec 0403 S3): librsvg, inside chafa, finds fonts
  # through fontconfig, and the image has no /etc/fonts — without this the
  # titles render as nothing. Two faces of DejaVu Sans, 1.5 MiB, rather than
  # the whole family; teleprompt's width table is for the face `header` uses.
  bannerFonts = pkgs.runCommand "dejavu-sans-banner" { } ''
    mkdir -p $out/share/fonts/truetype
    cp ${pkgs.dejavu_fonts}/share/fonts/truetype/DejaVuSans{,-Bold}.ttf \
       $out/share/fonts/truetype/
  '';
  bannerFontsConf = pkgs.makeFontsConf { fontDirectories = [ bannerFonts ]; };

  # bin/teleprompt calls python3 (its readline coprocess, which finds
  # libreadline through TELEPROMPT_LIBREADLINE), chafa (`header`, `picture`),
  # nvim (`view`, `view_textproto`, `view_proto`), and tput/stty. buf is on
  # PATH for the proto LSP the nvim config starts. The lean Neovim and buf
  # are the image's own, so the talk costs the image nothing for them; no
  # ImageMagick, whose closure brings perl (spec 0403 S2, S3).
  teleprompt = pkgs.runCommand "teleprompt"
    { nativeBuildInputs = [ pkgs.makeWrapper ]; }
    ''
      mkdir -p $out/bin
      cp ${telepromptSrc} $out/bin/teleprompt
      chmod +x $out/bin/teleprompt
      patchShebangs $out/bin/teleprompt
      wrapProgram $out/bin/teleprompt \
        --prefix PATH : ${lib.makeBinPath [
          pkgs.bash pkgs.python3 pkgs.chafa neovimLean bufLean pkgs.ncurses pkgs.coreutils
        ]} \
        --set TELEPROMPT_LIBREADLINE ${pkgs.readline}/lib/libreadline.so \
        --set FONTCONFIG_FILE ${bannerFontsConf} \
        --set XDG_CONFIG_HOME ${telepromptNvimConfig}
    '';

  # The deck's committed material (spec 0403 S7). eve/ and bob/ come in with
  # their shell.nix only, which the image ignores; everything the deck
  # generates is gitignored and left out.
  deck = lib.fileset.toSource {
    root    = deckSrc;
    fileset = lib.fileset.unions [
      (deckSrc + "/grehack2026.sh")
      (deckSrc + "/grehack2026.init")
      (deckSrc + "/anomalies.pb")
      (deckSrc + "/beats")
      (lib.fileset.fileFilter (f: f.hasExt "jpeg" || f.name == "s3ns.svg")
        (deckSrc + "/images"))
      (deckSrc + "/eve/.gitkeep")
      (deckSrc + "/bob/.gitkeep")
    ];
  };
in
{
  inherit teleprompt deck;

  # What the deck calls besides the prototools (spec 0403 S1): the
  # teleprompt; chafa for `picture` run by hand; nvim and buf, so protolens's
  # `v` and a bare `nvim` behave the same as inside the teleprompt; protoc
  # (`protoc --decode`); hexdump; and kitty's terminfo, so a kitty window
  # (TERM=xterm-kitty) works in the container (S10).
  demoTools = [
    teleprompt
    pkgs.chafa
    neovimLean
    bufLean
    pkgs.protobuf
    pkgs.util-linuxMinimal
    pkgs.kitty.terminfo
  ];

  # Spec 0403 S9: what both set.
  demoEnv = {
    PROTOTEXT_DESCRIPTOR_SET = "${wktDb}/share/prototools/wkt.desc";
    # protolens's `v` opens $VISUAL, else $EDITOR (spec 0420): the demos'
    # Neovim, with the demos' config through VIMINIT, which only Vim and
    # Neovim read, so the presenter's other programs keep their own
    # configuration (S8). VISUAL too: a presenter's own VISUAL would win
    # over EDITOR. The config is Lua, which Vim cannot source, hence the
    # guard: a Vim started in the demo shell ignores it.
    VISUAL  = "nvim";
    EDITOR  = "nvim";
    VIMINIT = "if has('nvim') | source ${./demo-nvim/init.lua} | endif";
  };
}
