<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# GreHack 2026 — prototools workshop setup

The workshop runs in a container image holding prototools and everything
they need. You need a container runtime on your laptop, nothing else. The
image exists for x86-64 and arm64, and your runtime picks the right one.

## 1. Install a container runtime (before the event)

Any OCI runtime works. We recommend:

| Laptop | Recommended | Install guide |
|---|---|---|
| Linux (x86-64 or arm64) | Podman, rootless (the default) | <https://podman.io/docs/installation> |
| Linux, alternative | Docker Engine | <https://docs.docker.com/engine/install/> |
| macOS (Apple silicon or Intel) | Colima with the `docker` CLI: `brew install colima docker` | <https://github.com/abiosoft/colima#installation> |
| macOS, alternative | Podman | <https://podman.io/docs/installation> |

All of these are open source. Docker Desktop works too if you already have
it.

**Optional, recommended: kitty** (<https://sw.kovidgoyal.net/kitty/>), a
terminal that shows pictures as images. The talk's deck (section 5) has a
few; any other terminal shows them as character blocks. WezTerm, Ghostty
and iTerm2 (macOS) show them as images too.

Rootless Podman on Linux runs containers as you: root inside the container
is your own user outside it, so nothing the container writes to your
directory ends up owned by root.

Then start the runtime once and check it works:

```sh
podman run --rm quay.io/podman/hello     # Podman (on macOS, first:
                                         #   podman machine init && podman machine start)
colima start                             # Colima, then:
docker run --rm hello-world
```

**Please do this at home.** On macOS, Colima and Podman download a Linux VM
image on their first start (a few hundred MB). The USB keys at the event do
not carry it.

## 2. Get the workshop image

At home, if you can (about 350 MB):

```sh
podman pull ghcr.io/douzebis/prototools-workshop:grehack2026    # or docker pull
```

Otherwise, at the event, from a USB key, offline:

```sh
/path/to/usb-key/load.sh     # uses podman, or docker when there is no podman
```

`load.sh` picks the image for your laptop, checks its SHA-256 sum and loads
it.

## 3. Run it

From the directory you want to share with the container. With **Podman**:

```sh
podman run -it --rm --name workshop --user 0 --cap-add NET_RAW \
    -e TERM -e COLORTERM -v "$PWD":/work \
    ghcr.io/douzebis/prototools-workshop:grehack2026
```

With **Docker**:

```sh
docker run -it --rm --name workshop --cap-add NET_RAW \
    -e TERM -e COLORTERM -v "$PWD":/work \
    ghcr.io/douzebis/prototools-workshop:grehack2026
```

and with Docker **on Linux**, add `--user "$(id -u):$(id -g)"`, so that the
files you write to `/work` are yours.

`--name workshop` lets more terminals join the same container (section 4);
`--cap-add NET_RAW` lets the tap capture traffic.

You land in `/workshop`, which holds the workshop material. Your directory
is at `/work`. Anything written elsewhere disappears when you leave the
container.

## 4. The game of life, in three terminals

Client, server and tap each run in their own terminal, all in the same
container. The first terminal is the one `run` gave you; open two more on
your laptop:

| Terminal | Podman | Docker |
|---|---|---|
| 1 | `life-client` | `life-client` |
| 2 | `podman exec -it workshop bash`, then `life-server` | `docker exec -it workshop bash`, then `life-server` |
| 3 | `podman exec -it workshop life-tap` | `docker exec -it -u 0 workshop life-tap` |

Each stops with Ctrl-C. The server also writes a traffic log, `server.log`, in
the directory it runs in (`--no-log` turns it off). The tap prints the `dumpcap` and `tshark` commands
it runs, a line per message, and writes each message to `/work/capture` as
a `.pb` file.

**One window instead:** `tmux` is in the image. With Podman, run all three
in panes. With Docker, panes cannot become root, so start the tap from the
laptop and follow it in a pane:

```sh
docker exec -d -u 0 workshop life-tap          # on the laptop
tail -f /work/capture/tap.log                  # in a tmux pane
docker exec -u 0 workshop life-tap --stop      # on the laptop, to stop it
```

## 5. Replay the talk

The talk's deck is in the image, in `/workshop/grehack2026`, with the
pictures. It plays in three windows, as on stage: Alice's, where the deck
runs; Eve's, for the server; and Bob's, for the client.

The deck starts the tap itself, which captures as root: start the container
as root, with either runtime.

```sh
podman run -it --rm --name workshop --user 0 --cap-add NET_RAW \
    -e TERM -e COLORTERM -v "$PWD":/work \
    ghcr.io/douzebis/prototools-workshop:grehack2026
```

(or the same with `docker`). Then:

| Window | Command |
|---|---|
| Alice (the one `run` gave you) | `cd grehack2026 && teleprompt grehack2026.sh` |
| Eve | `podman exec -it -w /workshop/grehack2026/eve workshop bash` |
| Bob | `podman exec -it -w /workshop/grehack2026/bob workshop bash` |

with `docker exec` in place of `podman exec` under Docker. Make Bob's window
at least 96 columns wide, and Alice's at least 120.

In the deck, Enter runs the current step, Down moves to the next one, F1
lists the keys, and Ctrl-D quits (and stops the tap). The 👉 cues say what to
type in Eve's and Bob's windows.

The pictures show as images in kitty (section 1) in a plain `exec` window;
inside a tmux pane they depend on tmux passing them through, which the
image allows.

## 6. Stretch goals (optional)

Two more things to explore if you finish early. Neither needs root or the
network: they run in any container started as in section 3.

### 6.1 The gRPConf 2026 talk

The gRPConf 2026 talk on prototools, with a real-world corpus: Bob's
app talks to Google Maps, and Alice has only the app binary, one captured
request and a log. It shows where `protoc --decode` falls short, schema
recovery from the binary with `protoscan` and `protoback`, type inference
against the whole googleapis corpus (8,000 files), and a byte-exact
re-encoding. Its annexes go further: performance, a `protoback` deep-dive, and
the anomaly taxonomy.

```sh
cd /workshop/grpconf2026 && teleprompt grpconf2026.sh
```

It runs in one window, at least 120 columns wide. Enter runs the current
step, Down moves to the next one, F1 lists the keys, and Ctrl-D quits.

### 6.2 The anomalies study

`/workshop/anomalies.pb` holds one example of every anomaly prototools
reports: every way an encoding can be non-canonical or invalid while a
lenient decoder lets it through. A guided protolens script walks through
them, and `/workshop/anomalies.md` explains each one:

```sh
cd /workshop
protolens --type google.protobuf.FileDescriptorProto anomalies.pb \
    --script anomalies.script
```

The gRPConf talk's annex C walks the same taxonomy, with a blob of its own.

## If it does not start

- **"exec format error" on a Mac:** the runtime cannot run the arm64 image.
  Use the x86-64 image under Rosetta instead: start Colima with
  `colima start --vm-type vz --vz-rosetta`, then add `--platform linux/amd64`
  to `docker pull` and `docker run` (from the USB key:
  `docker load -i prototools-workshop-amd64.tar`, then the same `docker run`).
  It is slower, but works.
- **The tap says it cannot capture:** the container was started without
  `--cap-add NET_RAW`; start it again with it. In the talk (section 5), the
  tap also needs the container to run as root: `--user 0`.
- **"permission denied" on the Docker socket (Linux):** see
  <https://docs.docker.com/engine/install/linux-postinstall/>, or use
  `sudo docker`.
