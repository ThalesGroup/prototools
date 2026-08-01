<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0145 — protolens: bundle a minimal Neovim config with `buf`'s LSP for `.proto` navigation

Status: implemented
Implemented in: 2026-07-18
Refs: docs/specs/0144-protolens-neovim-jump-to-definition.md (G6 committed
      to — but explicitly deferred to a later spec — a minimal Lua config
      wiring Neovim's LSP client to `buf lsp serve`; N6 ("no broader
      Neovim plugin ecosystem beyond the minimal committed Lua config")
      already anticipated this spec's scope. 0144's G5/G3 job-control
      handoff and cursor-positioning are unchanged by this spec — this is
      purely additive, giving the user real navigation *once already
      inside* the handed-off Neovim, whereas 0144 only gets them to a
      single starting location), 2026-07-18 glitch-fix session (user
      question: "once into neovim, how do I navigate the proto sources
      ... could protolens automatically take care of this provisioning?")
App: protolens

## Background

Spec 0144 gives `v` a way to land the cursor on a type's declaration in a
handed-off Neovim, but once there, plain Neovim has no idea `.proto`
files exist: no filetype, no syntax highlighting, and — the actual gap
this spec closes — no way to jump from a field's type reference to *that*
type's declaration, which may live in a different file than the one `v`
opened. `buf` (already a mandatory runtime dependency since spec 0144
G6, bundled onto `PATH` via `wrapProgram`) ships a built-in language
server (`buf lsp serve`, confirmed real subcommand — `buf lsp serve
--help` against this repo's pinned `buf` 1.59.0: stdio by default, `
--pipe <socket>` optional) that can provide exactly this, via Neovim's
native LSP client.

Today nothing wires the two together. This spec bundles a small,
self-contained Neovim config (committed to the repo, packaged into the
`protolens` Nix derivation, loaded automatically by `neovim::open_editor`
via `-u <config>`) that gives `.proto` filetype detection, minimal syntax
highlighting, and an LSP client pointed at `buf lsp serve` with
`gd`/`gr`/`K` navigation — independent of whatever (if anything) the
invoking user has in their own `~/.config/nvim`.

This spec's config is loaded via `-u <config>` — a **full profile
replacement**, not layered onto the user's own `init.lua`/plugins/
colorscheme. This gives a reproducible, guaranteed-working experience
independent of the user's personal setup (and of whether they even use
Neovim day-to-day), at the accepted cost of losing their own
keybindings/theme/plugins while inside the handed-off session (N4). No
layering/opt-out mechanism is provided.

## Goals

- **G1 (bundled Neovim config, single file).** A new committed file,
  `protolens/nvim/init.lua`, providing everything below in one
  self-contained script — no third-party plugins, no plugin manager, no
  `nvim-lspconfig`/`mason` dependency (N2).

- **G2 (`.proto` filetype + minimal syntax highlighting).** Neovim's
  stock runtime has no built-in `.proto` filetype. `vim.filetype.add`
  maps the `.proto` extension to filetype `proto`; a `FileType`
  autocommand sets `commentstring = "// %s"` and a small hand-rolled
  `syntax match`/`syntax keyword` block (line comments `//`, block
  comments `/* */`, string literals, and the core `.proto` keyword set:
  `syntax package import option message enum service rpc returns
  repeated optional required reserved oneof map extend extensions group
  stream`) — no treesitter grammar bundled (N1), no semantic-token-based
  highlighting (N5); this is plain keyword/regex highlighting only.

- **G3 (LSP client wired to `buf lsp serve`).** On the same `FileType`
  autocommand, `vim.lsp.start({ name = "buf", cmd = { "buf", "lsp",
  "serve" }, root_dir = <resolved root> })`. Root resolution: walk up
  from the buffer's directory for `buf.yaml`, `buf.work.yaml`, or `.git`
  (`vim.fs.root`); if none is found, fall back to the
  `PROTOTEXT_PROTO_ROOT` environment variable — spec 0144 G4's own env
  var name (`-I`/`--proto-root`'s `env` binding) — which `open_editor`
  (G6 below) sets explicitly on the spawned `nvim`'s environment from
  `app.proto_root`'s current, already-validated value, so it reflects
  `:proto-root` overrides too, not just the CLI flag/env var protolens
  itself was launched with.
  `buf lsp serve`'s actual capability set — whether it implements
  `textDocument/definition`/`references`/`hover` — is unverified as of
  this draft (no live Neovim in the drafting sandbox). Accepted as a
  known limitation of this spec: confirm against a real `buf lsp serve`
  during implementation; G4's keymaps below may need trimming if some
  are unsupported.

- **G4 (navigation keymaps).** An `LspAttach` autocommand binds, buffer-
  locally: `gd` → `vim.lsp.buf.definition()`, `gr` →
  `vim.lsp.buf.references()`, `K` → `vim.lsp.buf.hover()`. `Ctrl-Z`
  needs no new binding — Neovim's own default Normal-mode `Ctrl-Z` is
  already `:suspend` (built into Vim/Neovim, not user-configurable
  territory), and spec 0144's job-control glitch fix (2026-07-18,
  `SIGTTOU` handling in `open_editor`) already makes the resulting
  suspend/resume round-trip back into `protolens` work correctly — no
  further action needed here. (Neovim 0.11 introduces some LSP default
  keymaps of its own, e.g. `grr`/`gra`/`grn`; exact overlap with `gr`
  above to be confirmed against whichever Neovim version `nix/rust.nix`
  actually pins during implementation, and reconciled — either by
  deferring to the built-in default or by this config's explicit
  buffer-local override taking precedence, whichever is cleaner.)

- **G5 (Nix packaging).** In `nix/rust.nix`'s `protolensPostInstall`
  (already gained `pkgs.makeWrapper` + the Neovim/`buf` `PATH` prepend in
  spec 0144 G6):
  1. Install the committed config into the derivation's output:
     `install -Dm444 ${../protolens/nvim/init.lua}
     "$out/share/protolens/nvim/init.lua"` — a plain Nix path literal,
     copied into the store independently of however `workspaceSrc`
     filters sources for the actual `cargo build` (no risk of the Rust-
     source fileset excluding a non-`.rs` asset).
  2. Extend the existing `wrapProgram` call with `--set
     PROTOLENS_NVIM_CONFIG "$out/share/protolens/nvim/init.lua"`.

- **G6 (Rust wiring — `neovim.rs`'s `open_editor`).** Only the
  `EditorState::NotRunning` branch (the one fresh `nvim --listen` spawn;
  the `Suspended` branch's `--remote-tab`/`--remote-send` calls are
  short-lived RPC clients talking to an *already*-running, already-
  configured instance, so they need neither flag):
  - If `PROTOLENS_NVIM_CONFIG` is set in `protolens`'s own environment
    (set by the Nix wrapper, G5; absent in a non-Nix dev build, e.g.
    `cargo run` — in which case Neovim falls back to its own normal
    config resolution, a deliberate, documented degraded mode, not an
    error), add `.arg("-u").arg(config_path)` to the spawned `Command`.
    Refinement (2026-07-18, post-implementation): when unset, `open_editor`
    now defaults `PROTOLENS_NVIM_CONFIG` itself (via `std::env::set_var`,
    only if unset, never overriding the Nix wrapper's value) to
    `${CARGO_MANIFEST_DIR}/nvim/init.lua` — the repo-relative config,
    baked in at compile time — but only if that path actually exists on
    disk (preserves the degraded fallback if the binary is copied away
    from its source checkout). This gives non-Nix dev builds (`cargo
    run`/`cargo build`) the bundled config too, without requiring the
    developer to set the env var by hand.
    Correction (2026-08-01): the same defaulting, but as a plain local
    rather than `std::env::set_var`. The `unsafe` block's claim that no
    other thread was running was false — the handoff shuts down the input
    reader, never the heat worker — and no process-global mutation was
    needed for it in the first place, since nothing reads
    `PROTOLENS_NVIM_CONFIG` except the `-u` flag five lines below.
    Behavior is unchanged; only the mechanism is.
  - Always set `.env("PROTOTEXT_PROTO_ROOT", proto_root)` on the spawned
    `Command` from `app.proto_root`'s current value (G4's own env-var
    name, reused rather than inventing a second one) — `open_definition`
    (spec 0144) already guarantees `app.proto_root` is `Some` and
    resolves to a real, on-disk directory before `pending_editor_open`
    is ever armed, so this is infallible at the point `open_editor` runs.

- **G7 (tests).** No dedicated unit test for the `-u`/`PROTOTEXT_PROTO_
  ROOT` `Command`-building wiring itself — consistent with spec 0144 G7,
  which likewise never asserted `open_editor`'s exact `Command` args
  (this codebase's tests never spawn real external processes, and
  `Command`-building here has no meaningful pure-function seam to
  extract without inventing test-only indirection this spec doesn't
  otherwise need). Verified instead via a manual QA checklist (see Test
  plan) — matching spec 0144 G7's own precedent of deferring the actual
  headless LSP behavior to manual QA.

## Non-goals

- N1: No treesitter grammar/parser bundled — plain `syntax`
  keyword/regex highlighting only (G2).
- N2: No plugin manager, no `nvim-lspconfig`, no `mason` — a single
  hand-written Lua file, loaded via `-u`.
- N3: No support for editors other than Neovim (unchanged from spec
  0144 N2).
- N4: No layering/merging with the user's own personal Neovim config —
  `-u <config>` is a full profile replacement, accepted as a deliberate
  tradeoff (see Background).
- N5: No semantic-token-based or LSP-driven highlighting — G2's syntax
  highlighting is independent of whether the LSP client is even
  attached.
- N6: No special handling of the "no `buf.yaml`/`buf.work.yaml`/`.git`
  found *and* no usable `PROTOTEXT_PROTO_ROOT`" case — `buf lsp serve`
  is started with whatever `root_dir` resolution produces (possibly
  `nil`/omitted); its own behavior in that case, whatever it is, is
  accepted as-is, not specially detected or messaged by protolens.
- N7: No change to spec 0144's own `v` cursor-positioning/job-control
  mechanism — this spec is purely additive, giving navigation
  capability once already inside Neovim.

## Specification

### `protolens/nvim/init.lua` (new file)

Single-file config; SPDX header added via `reuse annotate` per repo
convention (Lua `--` comment form). Sketch (exact Neovim Lua API calls
to be confirmed/adjusted against whichever Neovim version `nix/rust.nix`
pins during implementation — this repo's own live-verification precedent
from spec 0144's G5 applies equally here):

```lua
vim.filetype.add({ extension = { proto = "proto" } })

vim.api.nvim_create_autocmd("FileType", {
  pattern = "proto",
  callback = function(args)
    vim.bo[args.buf].commentstring = "// %s"
    vim.cmd([[
      syntax match protoComment "//.*$"
      syntax region protoComment start="/\*" end="\*/"
      syntax region protoString start=+"+ end=+"+
      syntax keyword protoKeyword syntax package import option message enum
            \ service rpc returns repeated optional required reserved oneof
            \ map extend extensions group stream
      highlight default link protoComment Comment
      highlight default link protoString String
      highlight default link protoKeyword Keyword
    ]])

    local root = vim.fs.root(args.buf, { "buf.yaml", "buf.work.yaml", ".git" })
        or vim.env.PROTOTEXT_PROTO_ROOT

    vim.lsp.start({
      name = "buf",
      cmd = { "buf", "lsp", "serve" },
      root_dir = root,
    })
  end,
})

vim.api.nvim_create_autocmd("LspAttach", {
  callback = function(args)
    local opts = { buffer = args.buf, silent = true }
    vim.keymap.set("n", "gd", vim.lsp.buf.definition, opts)
    vim.keymap.set("n", "gr", vim.lsp.buf.references, opts)
    vim.keymap.set("n", "K", vim.lsp.buf.hover, opts)
  end,
})
```

### `nix/rust.nix`

```nix
protolensPostInstall = ''
  installShellCompletion --cmd protolens \
    ...  # unchanged (spec 0144)
  install -Dm444 ${../protolens/nvim/init.lua} \
    "$out/share/protolens/nvim/init.lua"
  wrapProgram $out/bin/protolens \
    --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.neovim pkgs.buf ]} \
    --set PROTOLENS_NVIM_CONFIG "$out/share/protolens/nvim/init.lua"
'';
```

### `protolens/src/tui/neovim.rs`

`open_editor`'s `EditorState::NotRunning` branch gains, before `.spawn()`:

```rust
let mut cmd = Command::new("nvim");
if let Some(config) = std::env::var_os("PROTOLENS_NVIM_CONFIG") {
    cmd.arg("-u").arg(config);
}
if let Some(proto_root) = &app.proto_root {
    cmd.env("PROTOTEXT_PROTO_ROOT", proto_root);
}
let child = cmd
    .arg("--listen")
    .arg(&socket_path)
    .arg(goto)
    .arg(&req.path)
    .process_group(0)
    .spawn();
```

(The `EditorState::Suspended` branch's two `--remote-*` helper calls are
unchanged — they never spawn a fresh Neovim, so neither `-u` nor the env
var applies.)

## Test plan

See G7. Manual QA checklist (no automated coverage of the actual `buf
lsp serve` behavior — deferred, matching spec 0144 G7's own precedent):

1. `nix-build -A protolens`; run the built binary against a real
   descriptor set with `--proto-root` pointing at a checked-out `.proto`
   corpus containing a `buf.yaml` (or `.git`).
2. Press `v` on a type to hand off into Neovim; confirm `.proto`
   filetype and syntax highlighting are active in the opened buffer.
3. Confirm `buf`'s LSP client attaches (e.g. `:LspInfo` or equivalent).
4. Position the cursor on a field's type reference (not necessarily the
   type `v` itself jumped to) and press `gd`; confirm it jumps to that
   type's declaration, including across files — the actual new
   capability this spec adds beyond spec 0144's single-shot jump.
5. Try `gr` (references) and `K` (hover); confirm reasonable behavior,
   or note as unsupported by `buf lsp serve` if it isn't (see G3's open
   question).
6. Repeat steps 2-4 against a `.proto` file with *no* `buf.yaml`/`.git`
   ancestor, relying on the `PROTOTEXT_PROTO_ROOT` fallback; confirm the
   LSP still attaches sensibly (or document the failure mode, per N6).
7. Confirm a non-Nix dev build (`cargo run`, no `PROTOLENS_NVIM_CONFIG`
   set) still launches Neovim successfully, falling back to the user's
   own config (degraded but functional, not an error).
