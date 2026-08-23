<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0344 — teleprompt built-in header

Status: implemented
Implemented in: 2026-08-22
App: teleprompt (bin/teleprompt)
Refs: docs/specs/ — none (teleprompt is not a protolens/prototext component)

## Background

`demo/header` renders a section title as SVG chevrons + text, piped through
ImageMagick → chafa.  Scripts driven by `teleprompt` call it as an external
subprocess.

Two problems:

1. **External dependency.** The script must be on `$PATH` or reached via a
   relative path.  A `teleprompt` script run from a different working
   directory, or by a presenter who has not sourced the dev-shell, silently
   gets no header.

2. **Wrong font ratio.** chafa's default `--font-ratio 1/2` assumes cells
   are exactly twice as tall as they are wide.  On the development machine
   (Alacritty, DejaVu Sans Mono, measured via `XTWINOPS`):
   - window: 1918 × 1131 px; grid: 137 × 39 cells
   - cell: 14 × 29 px → ratio **14/29 ≈ 0.483** (not 0.500)
   The error is small but measurable: chafa's half-block pixels are
   slightly taller than they should be, making the chevrons look squashed.

## Goals

- **G1.** Embed the SVG+chafa rendering path of `demo/header` as a Bash
  function inside `teleprompt`, available to every script it drives.
- **G2.** Measure the terminal's actual cell geometry once at startup and
  derive the correct `--font-ratio` automatically, with a safe fallback.
- **G3.** Warn at startup if `magick` or `chafa` are not on `$PATH`.

## Non-goals

- **N1.** The `--figlet` fallback path of `demo/header` is not embedded.
  It depends on `figlet` and pre-baked `.txt` chevron files and is only
  used for development convenience.  `demo/header` remains the standalone
  script for that path and for use outside `teleprompt`.
- **N2.** Enabling sixel/kitty pixel-protocol output is out of scope.
  The probe in S1 only reads cell geometry; it does not attempt graphics
  capability negotiation.
- **N3.** `demo/header` is not changed or removed.

## Specification

### S1 — Startup: terminal geometry probe

At startup, before the splash screen, `teleprompt` queries the terminal for
its pixel and cell dimensions using `XTWINOPS` escape sequences, unless
`TERM=dumb` or `NO_COLOR` is set (in which case the terminal is known to be
non-interactive and the probe is skipped).

Probe sequence:

1. Save terminal state: `old_stty=$(stty -g </dev/tty)`.
2. `stty raw -echo </dev/tty` — suppress echo of the escape response.
3. Write `\e[14t\e[18t` to `/dev/tty`.
   - `CSI 14 t` → terminal replies `CSI 4 ; <px_h> ; <px_w> t`
   - `CSI 18 t` → terminal replies `CSI 8 ; <rows> ; <cols> t`
4. Read the response with `read -t 0.3 -r response </dev/tty` (0.3 s
   timeout — long enough for a local terminal, short enough not to stall
   startup noticeably if the terminal does not reply).
5. Restore terminal state: `stty "$old_stty" </dev/tty`.
6. Parse `px_h`, `px_w`, `rows`, `cols` from `$response` using parameter
   expansion or `grep -oP`.
7. If all four are non-zero positive integers, compute integer cell
   dimensions and set:
   ```
   TELEPROMPT_FONT_RATIO="$(( px_w / cols ))/$(( px_h / rows ))"
   ```
8. Otherwise set `TELEPROMPT_FONT_RATIO="1/2"`.

`TELEPROMPT_FONT_RATIO` is a script-global variable consumed by `header`
(S2).

After the probe, print one dim status line (before the splash):
```
[teleprompt] font-ratio: 14/29  (measured)
```
or
```
[teleprompt] font-ratio: 1/2  (default)
```

### S2 — Startup: dependency check

After the probe, check that `magick` and `chafa` are on `$PATH`.  If either
is missing, print a yellow warning:

```
[teleprompt] warning: 'magick' not found — header() will not render
[teleprompt] warning: 'chafa' not found — header() will not render
```

Do not abort — the presenter may not use `header` at all, or may be
rehearsing on a machine without the graphics stack.  The dev-shell provides
both commands via `imagemagick` and `chafa` packages; this warning fires
only outside the dev-shell.

### S3 — Built-in `header` function

A function named `header` is defined in `teleprompt`'s environment.
Signature (positional arguments):

```bash
header <title> [<color>] [<height>]
```

Defaults: color `'#4d8fff'`, height `5` rows.

The function body implements the SVG+chafa rendering path:

- Section-number stripping (`${TITLE##+([0-9]).+( )}`), guarded by
  `shopt -s extglob` scoped inside the function.
- ImageMagick text-width measurement (used to size the viewBox).
- Inline SVG assembly (two chevrons + bold text).
- `magick | chafa` pipeline rendering the SVG to the terminal.

`header` is exported with `export -f header` so that scripts executed via
`eval` inside teleprompt's loop can call it as a bare command.

### S4 — Callable from scripts

Because `header` lives in `teleprompt`'s shell environment, scripts call it
as a bare command:

```bash
\
header "5. The schema is in the executables"
```

The leading `\` makes `teleprompt` treat the line as an executable step.
`demo/header` on `$PATH` remains available as a fallback for scripts run
outside `teleprompt`.

## Alternatives considered

**Reading cell size from `$COLUMNS`/`$LINES` only.** These give the cell
count but not the pixel size, so the ratio cannot be derived.  Ruled out.

**Probing graphics protocol (sixel/kitty) and using pixel output.**
Completely sidesteps the font-ratio problem.  Deferred as N2 — it requires
handling terminal capability strings and is a separate piece of work.

**Hardcoding `14/29`.** Correct for the development machine, wrong anywhere
else.  Ruled out in favor of the runtime probe with fallback.

## Test plan

1. `probe_measured` — run `teleprompt` in Alacritty; startup line says
   `(measured)` and ratio matches `cell_w/cell_h` computed from
   `\e[14t`/`\e[18t` responses.
2. `probe_fallback` — run `teleprompt` with `TERM=dumb`; startup line says
   `1/2 (default)`.
3. `header_renders` — a script containing `header "Test"` renders chevrons
   + text without error inside the dev-shell.
4. `header_missing_tool` — run `teleprompt` outside the dev-shell with
   `magick` absent from `$PATH`; startup prints the yellow warning and does
   not abort.
5. `header_color` — `header --color bright_red "Test"` resolves the name to
   `#ff5555` and renders in red.
6. `header_allcaps` — `header --all-caps "hello"` renders `HELLO`.
7. `header_section_strip` — `header "3. My Title"` renders `My Title`
   (number and dot stripped).

## Measured outcome

Implemented in `bin/teleprompt`.  The probe correctly reads `14/29` on
Alacritty with DejaVu Sans Mono on the development machine (1918×1131 px
window, 137×39 cell grid).  The `1/2` fallback fires when `TERM=dumb` or
`NO_COLOR` is set, and when `stty -g` fails (no tty available).

`TELEPROMPT_FONT_RATIO` is computed and reported at startup but is not
currently forwarded to chafa (chafa uses its compiled-in default).  This
is a known limitation — forwarding it via `--font-ratio` is left for a
follow-up once the chafa version in the dev-shell is confirmed to support
the flag.

`SCRIPT_DIR` (previously used to locate the external `demo/header`) was
removed as it became unused once the built-in `header` function replaced the
subprocess call.
