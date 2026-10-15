# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

# nix/demo-shells.nix — one shell per demo directory (spec 0394).
#
#   teleprompt         — bin/teleprompt, wrapped (from nix/grehack2026-demo.nix)
#   grehack2026-shell  — `cd grehack2026` (or eve/, bob/), then `nix-shell`
#   grpconf2026-shell  — `cd grpconf2026`, then `nix-shell`
#
# Every tool comes from Nix, built from committed sources, as the workshop
# image does (G2); nothing from target/release/ or bin/. These are not
# development shells (N3): no cargo, no Python environment, and no
# NIXSHELL_REPO, so the post-edit lint hook does not take them for the
# dev-shell.
{ pkgs
, grehackDemo       # nix/grehack2026-demo.nix: teleprompt, demoTools, demoEnv
, grpconfTalk       # nix/grpconf2026-demo.nix: demoTools, demoEnv, deck
, grehackRuntime    # grehack2026.runtime: the workshop image's tool set
, prototext
, protolens
, reproto
, protoscan
, wktDb
}:

let
  inherit (grehackDemo) teleprompt;

  # demoEnv as shellHook lines: assignments, then one export.
  exportEnv = env: ''
    ${pkgs.lib.toShellVars env}
    export ${builtins.concatStringsSep " " (builtins.attrNames env)}
  '';

  # Spec 0394 S4: not a development shell. nix-shell inherits the caller's
  # environment, so a demo shell entered from the dev-shell would otherwise
  # keep its NIXSHELL_REPO, and the post-edit lint hook would take this
  # shell, which has no cargo or ruff, for the dev-shell.
  notDevShell = "unset NIXSHELL_REPO";
in
{
  inherit teleprompt;

  # grehack2026/, eve/ and bob/ (spec 0394 S1): the workshop image's tools
  # and the talk's, the same set the image carries (spec 0403 S1).
  grehack2026-shell = pkgs.mkShell {
    name = "grehack2026";
    packages = [ grehackRuntime ] ++ grehackDemo.demoTools;
    shellHook = ''
      ${notDevShell}
      ${exportEnv grehackDemo.demoEnv}
    '';
  };

  # grpconf2026/ (spec 0394 S1, S5): its tools, environment and bob/ from
  # the same definition the workshop image uses (spec 0404 S1, S2).
  grpconf2026-shell = pkgs.mkShell {
    name = "grpconf2026";
    packages = [ prototext protolens reproto protoscan wktDb ]
      ++ grpconfTalk.demoTools;
    shellHook = ''
      ${notDevShell}
      ${exportEnv grpconfTalk.demoEnv}

      # Spec 0394 S5 (from the dev-shell's former _hook_demo): populate
      # bob/ and alice/ from the deck's bob/ (spec 0404 S2), writable, so the
      # presenter has a working directory. Guarded by a sentinel recording
      # the store path that last populated bob/, so re-entering is free.
      # bob/ and alice/ are gitignored; beats/ is committed and untouched.
      _grpconf_stage() {
        local bob="$PWD/bob"
        local sentinel="$bob/.demo-source"
        local demo="${grpconfTalk.deck}/bob"
        if [[ "$(cat "$sentinel" 2>/dev/null)" == "$demo" ]]; then
          return
        fi
        echo "grpconf2026: populating bob/ and alice/ from the deck"
        rm -rf "$bob"
        mkdir -p "$bob" "$PWD/alice"
        cp --no-preserve=mode "$demo/app" "$demo/logfile" "$demo/capture" "$bob/"
        chmod +x "$bob/app"
        echo "$demo" > "$sentinel"
      }
      if [[ "$(basename "$PWD")" == grpconf2026 ]]; then
        _grpconf_stage
      else
        echo "grpconf2026 shell: run nix-shell from grpconf2026/ to populate bob/" >&2
      fi
      unset -f _grpconf_stage
    '';
  };
}
