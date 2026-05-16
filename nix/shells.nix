# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

# nix/shells.nix — user-shell and dev-shell definitions.
#
# user-shell: plain shell with prototext, reproto, protoscan installed.
#             Activated by `nix-shell` (via shell.nix).
#
# dev-shell:  full development environment with Cargo, Rust tools, Python
#             tools, and a structured shellHook.  Activated by
#             `nix-shell dev-shell.nix`.
#
#             The shellHook is structured as named Bash functions called in
#             order, each printing a one-line recap:
#
#               _hook_env        — exports NIXSHELL_REPO, PYO3_PYTHON, PATH, PYTHONPATH
#               _hook_python     — writes python.env, pyrightconfig.json, ruff.toml
#               _hook_protos     — compiles fixture .pb descriptors (guarded)
#               _hook_codegen    — runs patch_reproto.sh (guarded)
#               _hook_cargo      — cargo build --release -p prototext (guarded)
#               _hook_rust       — exports RUSTFLAGS; writes rust-toolchain.toml;
#                                  rustup toolchain install
#               _hook_man        — generates man pages into man/man1/
#               _hook_completions — sources bash completions

{ pkgs
, pythonPkgs
, pythonBin
, pythonExecutable
, pyo3Rustflags
, repoRoot          # toString ./.  — used for NIXSHELL_REPO and PATH
, rustcVersion      # pkgs.rustc.unwrapped.version
, prototext
, reproto
, reprotoSrc        # filtered reproto source (builtins.path)
, reprotoBare       # bootstrap reproto package
, reprotoTestDeps   # full Python dep list for the dev-shell
, treeSitterTextproto
, protoscan
}:

{
  # ---------------------------------------------------------------------------
  # User shell — plain shell with prototext and reproto installed.
  # ---------------------------------------------------------------------------
  user-shell = pkgs.mkShell {
    name = "prototools-user";

    buildInputs = [ prototext reproto protoscan ];

    shellHook = ''
      old_opts=$(set +o)
      set -euo pipefail

      export NIXSHELL_REPO="${repoRoot}"
      export MANPATH="${prototext}/share/man:${reproto}/share/man:${protoscan}/share/man:''${MANPATH:-}"
      source ${prototext}/share/bash-completion/completions/prototext.bash
      source ${reproto}/share/bash-completion/completions/reproto.bash
      source ${protoscan}/share/bash-completion/completions/protoscan.bash

      [[ "$old_opts" == *"set -o errexit"*  ]] && set -e || set +e
      [[ "$old_opts" == *"set -o nounset"*  ]] && set -u || set +u
      [[ "$old_opts" == *"set -o pipefail"* ]] && set -o pipefail || set +o pipefail
    '';
  };

  # ---------------------------------------------------------------------------
  # Development shell
  # ---------------------------------------------------------------------------
  dev-shell = pkgs.mkShell {
    name = "prototools-dev";

    # Allow cargo to write build artifacts to target/ (outside /nix/store).
    NIX_ENFORCE_PURITY = 0;

    nativeBuildInputs = with pkgs; [
      cargo
      rustc
      rustfmt
      clippy
      reuse
      gh
      protobuf
      buf
      mandoc
      zola
      pythonPkgs.pytest
      pythonPkgs."pytest-xdist"
      pythonPkgs.ruff
      pkgs.pyright
    ];

    shellHook = ''
      old_opts=$(set +o)
      set -euo pipefail

      # ── Named hook functions ───────────────────────────────────────────────

      _hook_env() {
        echo "[hook] env: NIXSHELL_REPO, PYO3_PYTHON, PATH, PYTHONPATH"
        # Detected by ~/.claude/hooks/claude-hook-post-edit-lint to confirm
        # that the active nix-shell belongs to this repo.
        export NIXSHELL_REPO="${repoRoot}"
        export PYO3_PYTHON="${pythonExecutable}"
        export PATH="${repoRoot}/bin:${pythonBin}/bin:${repoRoot}/target/release:$PATH"
        export PYTHONPATH="$PWD/reproto/src:$PWD/protoscan/src:${treeSitterTextproto}:${pythonPkgs.makePythonPath reprotoTestDeps}:$PYTHONPATH"
      }

      _hook_python() {
        echo "[hook] python: python.env, pyrightconfig.json, ruff.toml"
        # Write python.env so VS Code / Pylance picks up the interpreter and
        # PYTHONPATH.  Named python.env (not .env) for visibility, consistent
        # with ruff.toml and rust-toolchain.toml.
        echo "PYTHON_INTERPRETER=${pythonExecutable}" > python.env
        echo "PYTHONPATH=$PYTHONPATH" >> python.env

        # Generate pyrightconfig.json from $PYTHONPATH so pyright CLI and
        # Pylance stay in sync with default.nix automatically.
        python3 -c "
import json, os
paths = [p for p in os.environ['PYTHONPATH'].split(':') if p]
cfg = {
  'extraPaths': paths,
  'exclude': [
    'result*',
    'prototext-pyo3/prototext_codec_lib',
    'fdp-scan-pyo3/fdp_scan_lib',
    'scoring-graph-pyo3/scoring_graph_lib',
    'docs/mockup',
  ],
}
with open('pyrightconfig.json', 'w') as f:
    json.dump(cfg, f, indent=2)
    f.write('\n')
"

        # Generate ruff.toml so that ruff check (run by the lint hook) excludes
        # the docs/mockup scratch directory.
        cat > ruff.toml <<'RUFFEOF'
exclude = [
  "docs/mockup",
]
RUFFEOF
      }

      _hook_protos() {
        # Compile prototext fixture .pb descriptors into
        # prototext/fixtures/prebuilt/, mirroring what protoPatchPhase does in
        # the Nix build.  Skipped if the descriptor files are already present.
        if [[ ! -f "$PWD/prototext/fixtures/prebuilt/descriptor.pb" ]]; then
          echo "[hook] protos: compiling fixture .pb descriptors"
          mkdir -p "$PWD/prototext/fixtures/prebuilt"
          protoc \
            --descriptor_set_out="$PWD/prototext/fixtures/prebuilt/descriptor.pb" \
            google/protobuf/descriptor.proto
          protoc \
            --descriptor_set_out="$PWD/prototext/fixtures/prebuilt/knife.pb" \
            --proto_path="$PWD/prototext/fixtures/schemas" \
            knife.proto
          protoc \
            --descriptor_set_out="$PWD/prototext/fixtures/prebuilt/enum_collision.pb" \
            --proto_path="$PWD/prototext/fixtures/schemas" \
            enum_collision.proto
        else
          echo "[hook] protos: already compiled — skipping"
        fi
      }

      _hook_codegen() {
        # Seed well-known .proto sources and compile .pb descriptors into the
        # working tree, mirroring what reprotoSrcFull does in the Nix build.
        # Skipped if the descriptor files are already present.
        if [[ ! -f "$PWD/reproto/src/resources/google/protobuf/descriptor.pb" ]]; then
          echo "[hook] codegen: running patch_reproto.sh"
          mkdir -p "$PWD/reproto/src/resources/google/protobuf"
          cp ${pkgs.protobuf}/include/google/protobuf/*.proto \
             "$PWD/reproto/src/resources/google/protobuf/"
          bash "$PWD/reproto/patch/patch_reproto.sh" \
            "${reprotoBare}" "$PWD/reproto"
        else
          echo "[hook] codegen: already done — skipping"
        fi
      }

      _hook_cargo() {
        # Build prototext only when the binary is absent or sources are newer.
        # Cargo's own incremental logic handles finer-grained staleness within
        # the working tree; this guard avoids the ~23s invocation overhead on
        # warm nix-shell entries when nothing has changed.
        if [[ ! -f "$PWD/target/release/prototext" ]] || \
           [[ "$PWD/prototext/src" -nt "$PWD/target/release/prototext" ]]; then
          echo "[hook] cargo: cargo build --release -p prototext"
          cargo build --release --locked -p prototext
        else
          echo "[hook] cargo: prototext binary up to date — skipping"
        fi
      }

      _hook_rust() {
        echo "[hook] rust: RUSTFLAGS, rust-toolchain.toml, rustup install"
        # RUSTFLAGS is set globally in commonArgs (Nix build) so that all Crane
        # derivations share a single fingerprint.  Export the same value here
        # so that manual `cargo build -p prototext_codec` in the shell aligns.
        export RUSTFLAGS="${pyo3Rustflags}"

        # Generate rust-toolchain.toml so rust-analyzer uses the same rustc
        # version as the nix-shell build.  Only written when the content
        # changes to avoid invalidating Cargo fingerprints on every entry.
        _toolchain_content="[toolchain]
channel = \"${rustcVersion}\"
components = [\"rust-src\", \"rustfmt\", \"clippy\"]"
        if [[ "$(cat rust-toolchain.toml 2>/dev/null)" != "$_toolchain_content" ]]; then
          rustup toolchain install ${rustcVersion} \
            --component rust-src --no-self-update 2>/dev/null || true
          printf '%s\n' "$_toolchain_content" > rust-toolchain.toml
        fi
        unset _toolchain_content
      }

      _hook_man() {
        echo "[hook] man: generating man pages into man/man1/"
        mkdir -p man/man1
        if command -v prototext-gen-man &>/dev/null; then
          prototext-gen-man man/man1
        fi
        if python3 -c "import reproto.gen_man" 2>/dev/null; then
          python3 -m reproto.gen_man man/man1
        fi
        if python3 -c "import protoscan.gen_man" 2>/dev/null; then
          python3 -m protoscan.gen_man man/man1
        fi
        export MANPATH="$PWD/man:''${MANPATH:-}"
        makewhatis "$PWD/man" 2>/dev/null || true
      }

      _hook_completions() {
        echo "[hook] completions: prototext, reproto, protoscan"
        # bash completion for prototext
        if command -v prototext &>/dev/null; then
          source <(PROTOTEXT_COMPLETE=bash prototext | sed \
            -e 's|-o nospace -o bashdefault|-o nospace -o filenames -o bashdefault|g' \
            -e 's|words\[COMP_CWORD\]="$2"|local _cur="''${COMP_LINE:0:''${COMP_POINT}}"; _cur="''${_cur##* }"; words[COMP_CWORD]="''${_cur}"|')
        fi

        # bash completion for reproto (pre-built script, avoids slow click invocation)
        eval "$(cat $PWD/reproto/src/reproto/completions.sh)"

        # bash completion for protoscan
        eval "$(_PROTOSCAN_COMPLETE=bash_source protoscan)"
      }

      # ── Run all hook steps in order ────────────────────────────────────────
      _hook_env
      _hook_python
      _hook_protos
      _hook_codegen
      _hook_cargo
      _hook_rust
      _hook_man
      _hook_completions

      [[ "$old_opts" == *"set -o errexit"*  ]] && set -e || set +e
      [[ "$old_opts" == *"set -o nounset"*  ]] && set -u || set +u
      [[ "$old_opts" == *"set -o pipefail"* ]] && set -o pipefail || set +o pipefail
    '';
  };
}
