# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

# nix/shells.nix — user-shell and dev-shell definitions.
#
# user-shell: plain shell with prototext, protolens, reproto, protoscan installed.
#             Activated by `nix-shell` (via shell.nix).
#
# dev-shell:  full development environment with Cargo, Rust tools, Python
#             tools, and a structured shellHook.  Activated by
#             `nix-shell dev-shell.nix`.
#
#             The shellHook is structured as named Bash functions called in
#             order, each printing a one-line recap:
#
#               _hook_env        — exports NIXSHELL_REPO, PROTOTEXT_DESCRIPTOR_SET,
#                                  PROTOTEXT_WKT_SET, PROTOTEXT_GOOGLEAPIS_SET,
#                                  PYO3_PYTHON, PATH, PYTHONPATH
#               _hook_python     — writes python.env, pyrightconfig.json, ruff.toml
#               _hook_protos     — compiles fixture .pb descriptors (guarded)
#               _hook_codegen    — runs patch_reproto.sh (guarded)
#               _hook_rust       — exports RUSTFLAGS, TREE_SITTER_TEXTPROTO_*;
#                                  writes rust-toolchain.toml; rustup toolchain
#                                  install
#               _hook_cargo      — cargo build --release -p prototext / -p
#                                  protolens (guarded); runs after _hook_rust
#                                  since protolens's build.rs requires
#                                  TREE_SITTER_TEXTPROTO_LIB_DIR/QUERIES_DIR
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
, protolens
, reproto
, reprotoSrc        # filtered reproto source (builtins.path)
, reprotoBare       # bootstrap reproto package
, reprotoTestDeps   # full Python dep list for the dev-shell
, treeSitterTextproto
, treeSitterTextprotoRustLib
, protoscan
, wktDb             # well-known-types schema DB; carries the PROTOTEXT_DESCRIPTOR_SET setup-hook
, googleapisDb      # googleapis schema DB; dev-shell only (PROTOTEXT_GOOGLEAPIS_SET)
, grpconfDemo       # grpconf-demo stage: bin/bobapp, logfile, capture, beats/
, buf               # narrow-pinned buf (newer than the main nixpkgs pin's 1.59.0; see default.nix)
}:

let
  # Python packages the dev shell wants and no derivation does.
  #
  # `pyte` is a terminal emulator: a pty harness feeds protolens' escape
  # sequences to it and reads the resulting *screen* back, which is what
  # makes "what did that keystroke actually draw?" a question answerable
  # without recompiling the Rust test binary.
  devOnlyPyDeps = [ pythonPkgs.pyte ];
in

{
  # ---------------------------------------------------------------------------
  # User shell — plain shell with prototext and reproto installed.
  # ---------------------------------------------------------------------------
  user-shell = pkgs.mkShell {
    name = "prototools-user";

    # wktDb contributes no binary — nix-shell sources each build input's
    # setup-hook, and its is what exports PROTOTEXT_DESCRIPTOR_SET
    # (spec 0228 S5), so no shellHook line is needed for it.
    buildInputs = [ prototext protolens reproto protoscan wktDb ];

    shellHook = ''
      old_opts=$(set +o)
      set -euo pipefail

      export NIXSHELL_REPO="${repoRoot}"
      export MANPATH="${prototext}/share/man:${protolens}/share/man:${reproto}/share/man:${protoscan}/share/man:''${MANPATH:-}"
      source ${prototext}/share/bash-completion/completions/prototext.bash
      source ${protolens}/share/bash-completion/completions/protolens.bash
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

    nativeBuildInputs = (with pkgs; [
      cargo
      rustc
      rustfmt
      clippy
      reuse
      gh
      protobuf
      # Google's wire-format inspector — a second, independent opinion on
      # what a blob's bytes say, for cross-checking prototext/protolens.
      protoscope
      neovim
      mandoc
      zola
      pythonPkgs.pytest
      pythonPkgs."pytest-xdist"
      pythonPkgs.ruff
      pythonPkgs.twine
      pkgs.pyright
      # Arithmetic in shell one-liners (benchmark and allocation-count math).
      bc

      # ── Performance audit toolchain ──────────────────────────────────────
      # This machine's benchmark noise floor is target-dependent and can be
      # large (`prototext-graph --bench score` measured a +15.9% same-binary
      # delta on one row), which puts many real effects below the resolution
      # of a wall clock. The tools below exist so that a question about
      # instructions, allocations or cache behaviour can be answered with a
      # deterministic counter instead of a timer — the approach that settled
      # spec 0179 when ~20 minutes of Criterion could not.
      #
      # valgrind — callgrind (exact per-function instruction counts and a
      #   call graph), cachegrind (cache/branch simulation), DHAT (heap
      #   access profile) and massif (heap over time), plus the
      #   `callgrind_annotate` / `cg_annotate` / `ms_print` reporters. It
      #   simulates rather than samples, so it needs no kernel permission
      #   and works under the default `perf_event_paranoid = 2`. Roughly
      #   50x slowdown, which the corpus replay can afford.
      valgrind
      # hyperfine — whole-command wall clock with warmup, and min/median
      #   rather than mean (`--export-json` for the record). Replaces
      #   hand-rolled `time` loops for end-to-end binary measurements.
      hyperfine
      # perf — the only tool here that reads real hardware counters at full
      #   speed. Requires `/proc/sys/kernel/perf_event_paranoid <= 1`; the
      #   default on this VM is 2, so it is present but inert until that
      #   sysctl is lowered. Kept in the shell so enabling it is a one-line
      #   root change rather than another shell rebuild.
      perf
      # gprof2dot + graphviz — render a callgrind profile as a call graph.
      #   `chafa` above displays the result inline in the terminal, so a
      #   headless VM is not a limitation.
      gprof2dot
      graphviz
      # Demo / ASCII-art utilities (not part of the released package)
      figlet
      toilet
      imagemagick
      chafa
      tree
      bat
      # readline — libreadline is needed by the teleprompt Python server,
      # which uses ctypes to access rl_add_funmap_entry and readline() directly
      # for proper multi-line buffer support.
      readline
    ]) ++ [ buf grpconfDemo ];

    shellHook = ''
      old_opts=$(set +o)
      set -euo pipefail

      # ── Named hook functions ───────────────────────────────────────────────

      _hook_env() {
        echo "[hook] env: NIXSHELL_REPO, PROTOTEXT_{DESCRIPTOR,WKT,GOOGLEAPIS}_SET, PROTOTEXT_ANOMALIES_BLOB, PYO3_PYTHON, PATH, PYTHONPATH, TELEPROMPT_LIBREADLINE"
        # Detected by ~/.claude/hooks/claude-hook-post-edit-lint to confirm
        # that the active nix-shell belongs to this repo.
        export NIXSHELL_REPO="${repoRoot}"

        # Path to libreadline, used by bin/teleprompt's Python server
        # to access rl_add_funmap_entry and readline() via ctypes.
        export TELEPROMPT_LIBREADLINE="${pkgs.readline}/lib/libreadline.so"

        # The toolset builds its binaries from source here, so wktDb is not
        # a build input and its setup-hook never fires (spec 0228 S6).
        # Export the same value explicitly, so `nix-shell` and
        # `nix-shell dev-shell.nix` agree.
        export PROTOTEXT_DESCRIPTOR_SET="${wktDb}/share/prototools/wkt.desc"

        # The two schema DBs by name, for reaching at one directly rather
        # than switching the default. Both are `<stub>.desc` beside a
        # `<stub>/` holding hopcroft.rkyv, index.rkyv and the decompiled
        # proto/ tree, which is the layout every consumer derives from the
        # descriptor path with its extension stripped — so either one is a
        # complete answer to --descriptor-set on its own.
        #
        # PROTOTEXT_WKT_SET is the same path as the default above, named so
        # that a script asking for the WKTs says so rather than relying on
        # what the default happens to be today.
        #
        # dev-shell only. googleapisDb is a full-tests derivation — a corpus
        # fetch, a whole-corpus protoc run and a reproto pass — and putting
        # it in user-shell would make a first `nix-shell` build all of it.
        export PROTOTEXT_WKT_SET="${wktDb}/share/prototools/wkt.desc"
        export PROTOTEXT_GOOGLEAPIS_SET="${googleapisDb}/googleapis.desc"

        # The spec-0226 fixture: one example of every annotation prototext can
        # emit, as `#@` prototext text under a `.pb` name.  It is a test
        # fixture of prototext-core and protolens first — hence
        # tests/fixtures/ rather than grpconf/, which must contribute nothing
        # to the Rust build — but it is also the one blob that shows the whole
        # vocabulary at once, so a demo beat may want to reach for it.  Named
        # here so that such a beat never has to spell a repo-relative path.
        #
        # A source path, not a store path, unlike the two sets above: the
        # fixture is committed, and it is edited by hand.  Its `.script`
        # sidecar is found *beside the blob* by protolens' script discovery,
        # which the source tree satisfies for free.
        export PROTOTEXT_ANOMALIES_BLOB="${repoRoot}/tests/fixtures/anomalies.pb"

        export PYO3_PYTHON="${pythonExecutable}"
        export PATH="${repoRoot}/bin:${pythonBin}/bin:${repoRoot}/target/release:$PATH"
        # `devOnlyPyDeps` is appended rather than folded into
        # reprotoTestDeps: that list is also the dep set of reprotoTests,
        # googleapisTests and customTests, and nothing that runs under
        # `ci` needs a terminal emulator.
        export PYTHONPATH="$PWD/reproto/src:$PWD/protoscan/src:${treeSitterTextproto}:${pythonPkgs.makePythonPath (reprotoTestDeps ++ devOnlyPyDeps)}:$PYTHONPATH"
      }

      _hook_python() {
        echo "[hook] python: python.env, pyrightconfig.json, ruff.toml"
        # Write python.env so VS Code / Pylance picks up the interpreter and
        # PYTHONPATH.  Named python.env (not .env) for visibility, consistent
        # with ruff.toml and rust-toolchain.toml.
        printf '# Auto-generated by nix/shells.nix — do not edit by hand.\n' > python.env
        echo "PYTHON_INTERPRETER=${pythonExecutable}" >> python.env
        echo "PYTHONPATH=$PYTHONPATH" >> python.env

        # Generate pyrightconfig.json from $PYTHONPATH so pyright CLI and
        # Pylance stay in sync with default.nix automatically.
        python3 -c "
import json, os
paths = [p for p in os.environ['PYTHONPATH'].split(':') if p]
cfg = {
  '_comment': 'Auto-generated by nix/shells.nix — do not edit by hand.',
  'pythonVersion': '3.13',
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
# Auto-generated by nix/shells.nix — do not edit by hand.
exclude = [
  "docs/mockup",
]
RUFFEOF
      }

      _hook_protos() {
        # Compile prototext fixture .pb descriptors into
        # prototext/fixtures/prebuilt/, mirroring what protoPatchPhase does in
        # default.nix.  The list must stay in step with that phase and with
        # prototext/build.rs's fallback, which copies all four unconditionally.
        #
        # Each file is guarded on its own rather than behind a single
        # descriptor.pb sentinel: with one sentinel, a fixture added after a
        # working tree was first populated is never compiled there, and the
        # failure surfaces far away as a build.rs panic ("failed to copy
        # message_set.pb: No such file or directory").
        local prebuilt="$PWD/prototext/fixtures/prebuilt"
        local schemas="$PWD/prototext/fixtures/schemas"
        local name missing=()
        for name in descriptor knife enum_collision message_set; do
          [[ -f "$prebuilt/$name.pb" ]] || missing+=("$name")
        done
        if (( ''${#missing[@]} == 0 )); then
          echo "[hook] protos: already compiled — skipping"
          return
        fi
        echo "[hook] protos: compiling ''${missing[*]}"
        mkdir -p "$prebuilt"
        for name in "''${missing[@]}"; do
          if [[ $name == descriptor ]]; then
            # Not in fixtures/schemas: protoc resolves it from its own include.
            protoc \
              --descriptor_set_out="$prebuilt/descriptor.pb" \
              google/protobuf/descriptor.proto
          else
            protoc \
              --descriptor_set_out="$prebuilt/$name.pb" \
              --proto_path="$schemas" \
              "$name.proto"
          fi
        done
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
        #
        # Watches prototext/src plus its path dependencies (prototext-core,
        # prototext-graph) and their Cargo.toml/Cargo.lock, since editing a
        # dependency crate must also trigger a rebuild. Uses `find -newer`
        # (not `-nt` on the directory itself) so content edits to existing
        # files are detected, not just files being added/removed.
        local bin="$PWD/target/release/prototext"
        local watch=(prototext/src prototext-core/src prototext-graph/src
                     prototext/Cargo.toml prototext-core/Cargo.toml prototext-graph/Cargo.toml
                     Cargo.lock)
        local stale=0
        if [[ ! -f "$bin" ]]; then
          stale=1
        else
          for p in "''${watch[@]}"; do
            if [[ -e "$p" && -n "$(find "$p" -newer "$bin" -print -quit 2>/dev/null)" ]]; then
              stale=1
              break
            fi
          done
        fi
        if [[ "$stale" -eq 1 ]]; then
          echo "[hook] cargo: cargo build --release -p prototext"
          cargo build --release --locked -p prototext
        else
          echo "[hook] cargo: prototext binary up to date — skipping"
        fi

        # Same staleness guard for protolens (depends on prototext-core and
        # prototext-graph, but not on prototext itself).
        local bin2="$PWD/target/release/protolens"
        local watch2=(protolens/src prototext-core/src prototext-graph/src
                      protolens/Cargo.toml prototext-core/Cargo.toml prototext-graph/Cargo.toml
                      Cargo.lock)
        local stale2=0
        if [[ ! -f "$bin2" ]]; then
          stale2=1
        else
          for p in "''${watch2[@]}"; do
            if [[ -e "$p" && -n "$(find "$p" -newer "$bin2" -print -quit 2>/dev/null)" ]]; then
              stale2=1
              break
            fi
          done
        fi
        if [[ "$stale2" -eq 1 ]]; then
          echo "[hook] cargo: cargo build --release -p protolens"
          cargo build --release --locked -p protolens
        else
          echo "[hook] cargo: protolens binary up to date — skipping"
        fi
      }

      _hook_rust() {
        echo "[hook] rust: RUSTFLAGS, TREE_SITTER_TEXTPROTO_*, rust-toolchain.toml, rustup install"
        # RUSTFLAGS is set globally in commonArgs (Nix build) so that all Crane
        # derivations share a single fingerprint.  Export the same value here
        # so that manual `cargo build -p prototext_codec_lib` in the shell aligns.
        # TREE_SITTER_TEXTPROTO_LIB_DIR/QUERIES_DIR mirror commonArgs.env so
        # that manual `cargo build -p protolens` aligns too (spec 0116 §7).
        export RUSTFLAGS="${pyo3Rustflags}"
        export TREE_SITTER_TEXTPROTO_LIB_DIR="${treeSitterTextprotoRustLib}/lib"
        export TREE_SITTER_TEXTPROTO_QUERIES_DIR="${treeSitterTextprotoRustLib}/queries"

        # Generate rust-toolchain.toml so rust-analyzer uses the same rustc
        # version as the nix-shell build.  Only written when the content
        # changes to avoid invalidating Cargo fingerprints on every entry.
        _toolchain_content="# Auto-generated by nix/shells.nix — do not edit by hand.
[toolchain]
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
        if command -v protolens &>/dev/null; then
          PROTOLENS_GEN_MAN=man/man1 protolens
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

      _hook_demo() {
        # Populate grpconf2026/{bob,beats,alice} from the grpconf-demo nix
        # derivation so that the presenter has a writable working directory.
        #
        # The nix store is read-only, so the beats cannot write app.desc or src/.
        # --no-preserve=mode strips the 0444/0555 modes from the copy so that
        # all files are writable.
        #
        # Layout written into grpconf2026/:
        #   bob/app          the bobapp binary (places v1 + routes v2)
        #   bob/logfile      the log with four anomalies
        #   bob/capture      one captured request body
        #   beats/           every grpconf2026/beats/*.script
        #   alice/           empty writable scratch directory for Alice's outputs
        #
        # googleapis is not included: $PROTOTEXT_GOOGLEAPIS_SET already provides it.
        #
        # The copy is guarded by a sentinel file recording the nix store path
        # that last populated the directory.  If it matches, skip everything to
        # avoid the overhead on every shell entry after the first.
        #
        # grpconf2026/{bob,beats,alice} are gitignored, so nothing here touches
        # the repo index.
        local stage="$PWD/grpconf2026"
        local bob="$stage/bob"
        local sentinel="$bob/.demo-source"
        local demo="${grpconfDemo}"
        if [[ "$(cat "$sentinel" 2>/dev/null)" == "$demo" ]]; then
          echo "[hook] demo: grpconf2026/ up to date — skipping"
          return
        fi
        echo "[hook] demo: populating grpconf2026/ from grpconf-demo"
        rm -rf "$bob" "$stage/beats"
        mkdir -p "$bob" "$stage/beats" "$stage/alice"
        cp --no-preserve=mode "$demo/bin/bobapp" "$bob/app"
        cp --no-preserve=mode "$demo/logfile"    "$bob/logfile"
        cp --no-preserve=mode "$demo/capture"   "$bob/capture"
        cp -r --no-preserve=mode "$demo/beats"/. "$stage/beats/"
        # Record which nix derivation populated the directory.
        echo "$demo" > "$sentinel"
        echo "[hook] demo: grpconf2026/ ready ($(du -sh "$stage" | cut -f1) total)"
      }

      _hook_completions() {
        echo "[hook] completions: prototext, protolens, reproto, protoscan"
        # bash completion for prototext
        if command -v prototext &>/dev/null; then
          source <(PROTOTEXT_COMPLETE=bash prototext | sed \
            -e 's|-o nospace -o bashdefault|-o nospace -o filenames -o bashdefault|g' \
            -e 's|words\[COMP_CWORD\]="$2"|local _cur="''${COMP_LINE:0:''${COMP_POINT}}"; _cur="''${_cur##* }"; words[COMP_CWORD]="''${_cur}"|')
        fi

        # bash completion for protolens
        if command -v protolens &>/dev/null; then
          source <(PROTOLENS_COMPLETE=bash protolens | sed \
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
      _hook_rust
      _hook_cargo
      _hook_demo
      _hook_man
      _hook_completions

      unset shellHook

      [[ "$old_opts" == *"set -o errexit"*  ]] && set -e || set +e
      [[ "$old_opts" == *"set -o nounset"*  ]] && set -u || set +u
      [[ "$old_opts" == *"set -o pipefail"* ]] && set -o pipefail || set +o pipefail
    '';
  };
}
