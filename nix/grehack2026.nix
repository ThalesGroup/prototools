# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

# nix/grehack2026.nix — the GreHack 2026 workshop image (spec 0374).
#
# One image per architecture, built by Nix on that architecture: CI builds
# linux/amd64 on ubuntu-latest and linux/arm64 on ubuntu-24.04-arm, and joins
# them under one reference (.github/workflows/workshop-image.yml).
#
# Attributes (exported from default.nix as `grehack2026.*`):
#   runtime      — the tools: prototext, protolens, reproto, protoscan, the
#                  WKT schema database, and the game of life with its tap
#                  (spec 0375); the demos' tools bring Neovim and buf
#   life         — life-server, life-client, life-tap (spec 0375)
#   image        — a streamLayeredImage script: `./result | docker load`
#   closureCheck — fails when a denied store path enters the image
#   publishTools — skopeo and crane, for the CI workflow
{ pkgs
, prototext
, protolens
, reproto
, protoscan
, life
, wktDb
, googleapisDb
, googleapisPbs
, mkClosureCheck
, grehackDemo     # nix/grehack2026-demo.nix: the talk's tools and material
, grpconfTalk     # nix/grpconf2026-demo.nix: the stretch-goal talk (spec 0404)
, gitRevision ? null
}:

let
  lib = pkgs.lib;

  # The tools the image carries (spec 0374 S2): the regular bundle's
  # members. Neovim and buf come with the demos' tools (spec 0420 S8).
  runtime = pkgs.symlinkJoin {
    name  = "prototools-runtime";
    # wireshark-cli on the participants' PATH too, not only on life-tap's:
    # the tap prints its dumpcap and tshark commands to be run and adapted
    # by hand (spec 0375 S6, S7).
    #
    # fortune on the participants' PATH too: a harmless command to send
    # through the covert channel. life-client carries its own wrapped copy;
    # putting it here lets a participant run `fortune` by hand and compare
    # with what the client sends back.
    paths = [ prototext protolens reproto protoscan wktDb life pkgs.wireshark-cli pkgs.fortune ];
  };

  uid = 1000;
  gid = 1000;
  home = "/home/hacker";

  # /etc/passwd and /etc/group with root, nobody and `hacker` (spec 0374 S4),
  # so the prompt and `whoami` work for the default user. Any other uid
  # (`docker run --user "$(id -u):$(id -g)"`) still works: HOME is set
  # explicitly and writable by everyone.
  nss = pkgs.dockerTools.fakeNss.override {
    extraPasswdLines = [
      "hacker:x:${toString uid}:${toString gid}:GreHack workshop:${home}:${pkgs.bashInteractive}/bin/bash"
    ];
    extraGroupLines = [ "hacker:x:${toString gid}:" ];
  };

  # The login shell's banner and prompt: what the tools are, where the
  # material is, and how to mount a directory from the laptop.
  profile = pkgs.writeTextDir "etc/profile" ''
    export PS1='\[\e[1;32m\]prototools\[\e[0m\]:\w\$ '
    if [ -t 1 ] && [ -z "''${PROTOTOOLS_QUIET:-}" ]; then
      cat <<'BANNER'

      prototools — GreHack 2026 workshop

      Tools:     prototext  protolens  reproto  protoscan
      Material:  /workshop
      Your files: /work, when started with -v "$PWD":/work

      The game of life, in three terminals (SETUP.md, section 4):
        1  this one                             life-client
        2  podman exec -it workshop bash        then: life-server
        3  podman exec -it workshop life-tap    the tap; writes /work/capture
      With Docker: docker exec, and -u 0 for the tap. Each stops with Ctrl-C.
      tmux is here too, for one window of panes.

      The talk: cd grehack2026 && teleprompt grehack2026.sh   (SETUP.md, section 5)

      Stretch goals (SETUP.md, section 6):
        cd grpconf2026 && teleprompt grpconf2026.sh                the gRPConf talk
        protolens --type google.protobuf.FileDescriptorProto \
            anomalies.pb --script anomalies.script                the anomalies

      Schema databases: $PROTOTEXT_DESCRIPTOR_SET (well-known types, the default)
                        $PROTOTEXT_GOOGLEAPIS_SET (googleapis; pass it with --descriptor-set)

    BANNER
    fi
  '';

  # tmux for one window of panes (spec 0375 S8), without systemd's
  # libraries, which it links by default: 0.9 MiB compressed against 2.5.
  tmux = pkgs.tmux.override { withSystemd = false; };

  # Kitty images from inside a tmux pane (spec 0403 S11): chafa wraps the
  # kitty graphics protocol in tmux's passthrough, which tmux drops unless
  # allowed.
  tmuxConf = pkgs.writeTextDir "etc/tmux.conf" ''
    set -g allow-passthrough on
  '';

  # What goes into the image's root (symlinked from the store). The
  # googleapis databases are not here: they are referenced from `config.Env`
  # below, which puts them in the closure — each in its own layer — without
  # scattering their files across `/`.
  contents = [
    runtime
    pkgs.bashInteractive
    pkgs.coreutils
    pkgs.gnugrep
    pkgs.less
    pkgs.ncurses
    tmux
    tmuxConf
    pkgs.cacert
    nss
    profile
  ] ++ grehackDemo.demoTools    # the talk (spec 0403 S1)
    ++ grpconfTalk.demoTools;   # the stretch-goal talk (spec 0404 S3)

  # Both talks' environments in one (spec 0404 S3). They must agree on the
  # default descriptor set, or one talk would run against the wrong one.
  demoEnv =
    assert grehackDemo.demoEnv.PROTOTEXT_DESCRIPTOR_SET
        == grpconfTalk.demoEnv.PROTOTEXT_DESCRIPTOR_SET;
    grehackDemo.demoEnv // grpconfTalk.demoEnv;


  image = pkgs.dockerTools.streamLayeredImage {
    name = "prototools-workshop";
    tag  = "latest";
    inherit contents;

    # Writable directories, created in the customisation layer (spec 0374
    # S4). `/tmp`: protolens's Neovim socket and blob scratch files go to
    # std::env::temp_dir(), and a Nix image has none. `$HOME` and
    # `/workshop` are 1777 too, so every uid can write there.
    extraCommands = ''
      mkdir -p tmp .${home} workshop work
      chmod 1777 tmp .${home} workshop work
      cp ${../tests/fixtures/anomalies.pb}     workshop/anomalies.pb
      cp ${../tests/fixtures/anomalies.script} workshop/anomalies.script
      # Spec 0374 S13: the image's own README; the fixture's, which
      # explains each anomaly, beside it as anomalies.md.
      cp ${../grehack2026/workshop-README.md}  workshop/README.md
      cp ${../tests/fixtures/README.md}        workshop/anomalies.md
      chmod 0666 workshop/*

      # The talk's material, writable: the deck writes capture/, life.desc,
      # life/ and eve/server.log next to itself (spec 0403 S7).
      cp -r ${grehackDemo.deck} workshop/grehack2026
      chmod -R u+w,a+rwX workshop/grehack2026
      find workshop/grehack2026 -type d -exec chmod 1777 {} +

      # The stretch-goal talk, writable likewise: its init recreates alice/
      # next to the deck (spec 0404 S3, S5).
      cp -r ${grpconfTalk.deck} workshop/grpconf2026
      chmod -R u+w,a+rwX workshop/grpconf2026
      find workshop/grpconf2026 -type d -exec chmod 1777 {} +
      chmod 0777 workshop/grpconf2026/bob/app
    '';

    config = {
      User       = "${toString uid}:${toString gid}";
      WorkingDir = "/workshop";
      Cmd        = [ "${pkgs.bashInteractive}/bin/bash" "--login" ];
      Env = [
        "HOME=${home}"
        "PATH=/bin"
        "SSL_CERT_FILE=/etc/ssl/certs/ca-bundle.crt"
        "LANG=C.UTF-8"
        # ncurses reads only its own terminfo directory; kitty's (spec 0403
        # S10) is linked into /share/terminfo with the image's contents.
        "TERMINFO_DIRS=/share/terminfo"
      # The tools read PROTOTEXT_DESCRIPTOR_SET (spec 0090); the googleapis
      # pair and PROTOTEXT_WKT_SET are for people and the decks. All set the
      # same way as in the demo shells (specs 0403 S9, 0404 S1).
      ] ++ lib.mapAttrsToList (n: v: "${n}=${v}") demoEnv;
      Labels = {
        "org.opencontainers.image.title"    = "prototools-workshop";
        "org.opencontainers.image.source"   = "https://github.com/douzebis/prototools";
        "org.opencontainers.image.licenses" = "MIT";
        # Spec 0374 S11: the same text the workflow writes on the index,
        # which is what the registry page shows; here so `docker inspect`
        # shows it too.
        "org.opencontainers.image.description" =
          builtins.readFile ../grehack2026/image-description.txt;
      } // lib.optionalAttrs (gitRevision != null) {
        "org.opencontainers.image.revision" = gitRevision;
      };
    };
  };

  # Spec 0374 S2: the leaks, plus what is a feature on a desktop and dead
  # weight in a container.
  closureCheck = mkClosureCheck {
    name  = "grehack2026-image-closure-check";
    roots = contents ++ [ googleapisDb googleapisPbs ];
    deny  = [ "-deps-deps" "winapi" "cargo-package" "wl-clipboard" "perl" "ruby" ];
  };

  # What CI publishes with (spec 0374 S6), from the repository's nixpkgs pin:
  # skopeo pushes an image archive, crane joins the pushed images in an index.
  publishTools = pkgs.symlinkJoin {
    name  = "grehack2026-publish-tools";
    paths = [ pkgs.skopeo pkgs.crane ];
  };

in { inherit runtime life image closureCheck publishTools; }
