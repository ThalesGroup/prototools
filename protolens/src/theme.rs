// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Theme: maps a `SyntaxRole` (spec 0116 §7) to a `ratatui::style::Style`
//! (spec 0116 §9) — two fixed, built-in palette pairs (dark, light), each
//! in two color depths (RGB, borrowed from VSCode Dark+/Light+; ANSI-16,
//! a portable fallback — picked via `supports_rgb`, which checks
//! `COLORTERM` first, then a live XTGETTCAP query to the terminal, then
//! falls back to a static terminfo capability probe — mirroring Vim's own
//! layered true-color auto-detection), plus a `System` selector resolved
//! once at startup.

use ratatui::style::{Color, Modifier, Style};

use crate::colorize::SyntaxRole;

/// The `--theme` CLI flag's three fixed choices (spec 0116 §9). `System`
/// exists only at the CLI-selection layer — it is resolved to `Dark` or
/// `Light` once at startup, before any rendering happens; `style_for`
/// itself only ever takes the resolved `Dark`/`Light` variant.
#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub enum ThemeKind {
    Dark,
    Light,
    System,
}

/// Maps `role` to a `Style`. `theme` must already be resolved to
/// `Dark`/`Light` (see `resolve_system`); passing `System` here is a
/// programming error.
///
/// Picks between an RGB palette (borrowed from VSCode's Dark+/Light+
/// themes) and a portable ANSI-16 fallback, based on `supports_rgb`
/// (spec 0116 §9).
pub fn style_for(role: SyntaxRole, theme: ThemeKind) -> Style {
    match theme {
        ThemeKind::Dark if supports_rgb() => style_for_dark_rgb(role),
        ThemeKind::Dark => style_for_dark_ansi16(role),
        ThemeKind::Light if supports_rgb() => style_for_light_rgb(role),
        ThemeKind::Light => style_for_light_ansi16(role),
        ThemeKind::System => {
            unreachable!("ThemeKind::System must be resolved before rendering — see main.rs")
        }
    }
}

/// Whether the terminal advertises 24-bit color support, checked in the
/// same order Vim does (patch 9.1.1060, vim/vim#16490): `COLORTERM=
/// truecolor`/`24bit` (the signal `bat`, `delta`, and most other Rust
/// terminal tools key off — a plain env lookup, re-checked on every
/// call, no caching) first; then a live XTGETTCAP query to the terminal
/// (`xtgettcap_reports_rgb`, cached); then a static terminfo capability
/// probe (`terminfo_reports_rgb`, cached) for terminals that don't
/// answer the live query.
fn supports_rgb() -> bool {
    colorterm_reports_truecolor() || xtgettcap_reports_rgb() || terminfo_reports_rgb()
}

fn colorterm_reports_truecolor() -> bool {
    matches!(
        std::env::var("COLORTERM").as_deref(),
        Ok("truecolor") | Ok("24bit")
    )
}

/// XTGETTCAP query string for the "RGB" capability: DCS `+q`, followed
/// by the capability name hex-encoded byte-by-byte (`RGB` = `52 47 42`),
/// terminated by ST. See
/// <https://gnanenthiran.medium.com/decoding-xtgettcap-2a8ba98e26f7> and
/// Vim's own `term.c` (`t_xtgettcap`).
const XTGETTCAP_RGB_QUERY: &str = "\x1bP+q524742\x1b\\";

/// Live XTGETTCAP fallback for when `COLORTERM` isn't set — this is
/// Vim's *primary* true-color signal (patch 9.1.1060, vim/vim#16490):
/// actively query the live terminal (not just its static terminfo
/// entry) for the `RGB` termcap capability. Some terminals answer this
/// correctly even though their terminfo database entry doesn't
/// advertise `RGB`/`Tc`/`max_colors=16777216` — `terminfo_reports_rgb`
/// remains as the fallback for terminals that don't answer this live
/// query at all (e.g. some xterm builds).
///
/// Only attempted when both stdin and stdout are real terminals — under
/// `cargo test` (and other non-interactive contexts) this is false, so
/// no terminal I/O is attempted and the function returns `false`
/// immediately.
///
/// Cached in a `OnceLock`: like `terminfo_reports_rgb`, the answer
/// cannot change during a single process's lifetime, and — unlike a
/// static terminfo lookup — repeating this query would mean repeated,
/// real terminal round-trips.
fn xtgettcap_reports_rgb() -> bool {
    static CACHE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHE.get_or_init(|| {
        use std::io::IsTerminal;
        if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
            return false;
        }
        query_xtgettcap_rgb().unwrap_or(false)
    })
}

/// Performs the actual XTGETTCAP round-trip. Mirrors `terminal-light`'s
/// own `xterm.rs::query` raw-mode-wrapping pattern: temporarily enables
/// raw mode (if not already enabled) so the response isn't held back
/// waiting for a newline, then restores the prior mode.
///
/// Must run before the TUI's own crossterm event loop starts polling
/// the terminal (see `main.rs`'s `theme::prime_supports_rgb` call) —
/// two concurrent readers of the tty would race.
fn query_xtgettcap_rgb() -> Result<bool, xterm_query::XQError> {
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode, is_raw_mode_enabled};
    let switch_to_raw = !is_raw_mode_enabled()?;
    if switch_to_raw {
        enable_raw_mode()?;
    }
    let res = xterm_query::query(XTGETTCAP_RGB_QUERY, 100u16);
    if switch_to_raw {
        disable_raw_mode()?;
    }
    res.map(|response| parse_xtgettcap_response(&response))
}

/// Whether an XTGETTCAP response confirms the queried capability is
/// supported: a successful response contains `1+r` followed by the
/// hex-encoded capability name (`0+r` signals "unsupported", with no
/// capability name echoed back).
fn parse_xtgettcap_response(response: &str) -> bool {
    response.contains("1+r524742")
}

/// Terminfo-based fallback for when neither `COLORTERM` nor a live
/// XTGETTCAP query confirm true-color support — mirrors Vim's own
/// true-color auto-detection (patch 9.1.1060, vim/vim#16490): query the
/// terminal's *static* terminfo entry for the non-standard `RGB`/`Tc`
/// boolean capabilities, or a `max_colors` value of `0x1000000`
/// (16,777,216) — the sentinel some terminfo entries (e.g. `xterm-direct`)
/// use for true-color support.
///
/// Cached in a `OnceLock`: parsing the terminfo database from disk is
/// comparatively expensive, and — unlike `COLORTERM` — the answer cannot
/// change during a single process's lifetime (`TERM` isn't toggled at
/// runtime, and no test in this module mutates it).
fn terminfo_reports_rgb() -> bool {
    static CACHE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHE.get_or_init(|| {
        terminfo::Database::from_env()
            .map(|db| database_reports_rgb(&db))
            .unwrap_or(false)
    })
}

fn database_reports_rgb(db: &terminfo::Database) -> bool {
    if db.raw("RGB").is_some() || db.raw("Tc").is_some() {
        return true;
    }
    matches!(
        db.get::<terminfo::capability::MaxColors>(),
        Some(max) if i32::from(max) == 0x0100_0000
    )
}

/// Forces early, one-time evaluation of the cached `xtgettcap_reports_rgb`
/// result. Must be called before `tui::run` takes over the terminal with
/// raw mode + the alternate screen and starts its own crossterm event-
/// polling loop — otherwise the probe's read and the TUI's own input
/// handling could race over the same tty. Mirrors `resolve_system`'s own
/// early-startup OSC query in `main.rs`.
pub fn prime_supports_rgb() {
    xtgettcap_reports_rgb();
}

/// Named RGB constants for the dark palette (spec 0116 §9's "RGB
/// palette" table), borrowed from VSCode's `dark_plus.json`/
/// `dark_vs.json`. Doc comments cite each color's closest named-color
/// match from <https://www.color-name.com>, purely for human
/// readability when scanning this file — VSCode itself has no
/// equivalent naming, only semantic scope names (mirrored here by
/// which `SyntaxRole` each constant is named after). Centralized here,
/// and referenced (not re-typed) wherever the same VSCode color
/// applies to more than one role/function, so a color only ever needs
/// updating in one place.
mod dark_rgb {
    use ratatui::style::Color;

    /// "Clear Blue".
    pub const ATTRIBUTE: Color = Color::Rgb(0x9C, 0xDC, 0xFE);
    /// "Subtle Blue Green" — also VSCode's link-color (`StringSpecialUrl`)
    /// and `Constant`, and this crate's own focused-pane accent
    /// (`focus_style`).
    pub const TYPE: Color = Color::Rgb(0x4E, 0xC9, 0xB0);
    /// "Beauty Copper".
    pub const STRING_LITERAL: Color = Color::Rgb(0xCE, 0x91, 0x78);
    /// "Mushroom Melt".
    pub const STRING_ESCAPE: Color = Color::Rgb(0xD7, 0xBA, 0x7D);
    /// "Brussels Sprout" — also this crate's manage-pane "auto" entry
    /// color (`manage_entry_style`).
    pub const COMMENT: Color = Color::Rgb(0x6A, 0x99, 0x55);
    /// "Rainee".
    pub const NUMBER: Color = Color::Rgb(0xB5, 0xCE, 0xA8);
    /// "Azul Mystic" — also this crate's manage-pane origin-path
    /// header color (`render_manage_pane`, via `style_for`).
    pub const BOOLEAN: Color = Color::Rgb(0x56, 0x9C, 0xD6);
    /// "Pale Hazel".
    pub const PUNCTUATION_BRACKET_LIST: Color = Color::Rgb(0xDC, 0xDC, 0xAA);
    /// "Alexa".
    pub const PUNCTUATION_BRACKET_EXTENSION: Color = Color::Rgb(0xD1, 0x69, 0x69);
}

/// Named RGB constants for the light palette (spec 0116 §9's "RGB
/// palette" table), borrowed from VSCode's `light_plus.json`/
/// `light_vs.json`. See `dark_rgb` for the naming/reuse convention.
mod light_rgb {
    use ratatui::style::Color;

    /// "Electric Red".
    pub const ATTRIBUTE: Color = Color::Rgb(0xE5, 0x00, 0x00);
    /// "Jelly Bean Blue" — also VSCode's link-color (`StringSpecialUrl`)
    /// and `Constant`, and this crate's own focused-pane accent
    /// (`focus_style`).
    pub const TYPE: Color = Color::Rgb(0x26, 0x7F, 0x99);
    /// "San Diego".
    pub const STRING_LITERAL: Color = Color::Rgb(0xA3, 0x15, 0x15);
    /// "Strong Red".
    pub const STRING_ESCAPE: Color = Color::Rgb(0xEE, 0x00, 0x00);
    /// "Digital Green" — also this crate's manage-pane "auto" entry
    /// color (`manage_entry_style`).
    pub const COMMENT: Color = Color::Rgb(0x00, 0x80, 0x00);
    /// "Funky Green".
    pub const NUMBER: Color = Color::Rgb(0x09, 0x86, 0x58);
    /// "Blue" — also this crate's manage-pane origin-path header
    /// color (`render_manage_pane`, via `style_for`).
    pub const BOOLEAN: Color = Color::Rgb(0x00, 0x00, 0xFF);
    /// "French Blue".
    pub const PUNCTUATION_BRACKET_LIST: Color = Color::Rgb(0x04, 0x51, 0xA5);
    /// "Dried Burgundy".
    pub const PUNCTUATION_BRACKET_EXTENSION: Color = Color::Rgb(0x81, 0x1F, 0x3F);
}

/// RGB palette, dark — borrowed from VSCode's `dark_plus.json`/
/// `dark_vs.json` (spec 0116 §9's "RGB palette" table; scope names
/// cited there).
fn style_for_dark_rgb(role: SyntaxRole) -> Style {
    match role {
        SyntaxRole::Attribute => Style::default().fg(dark_rgb::ATTRIBUTE),
        SyntaxRole::Type => Style::default().fg(dark_rgb::TYPE),
        SyntaxRole::StringLiteral => Style::default().fg(dark_rgb::STRING_LITERAL),
        SyntaxRole::StringEscape => Style::default().fg(dark_rgb::STRING_ESCAPE),
        SyntaxRole::StringSpecialUrl => Style::default()
            .fg(dark_rgb::TYPE)
            .add_modifier(Modifier::UNDERLINED),
        SyntaxRole::Comment => Style::default().fg(dark_rgb::COMMENT),
        SyntaxRole::Number => Style::default().fg(dark_rgb::NUMBER),
        SyntaxRole::Boolean => Style::default().fg(dark_rgb::BOOLEAN),
        SyntaxRole::Constant => Style::default().fg(dark_rgb::TYPE),
        SyntaxRole::PunctuationDelimiter => Style::default(),
        SyntaxRole::PunctuationBracket => Style::default(),
        SyntaxRole::PunctuationBracketList => {
            Style::default().fg(dark_rgb::PUNCTUATION_BRACKET_LIST)
        }
        SyntaxRole::PunctuationBracketExtension => {
            Style::default().fg(dark_rgb::PUNCTUATION_BRACKET_EXTENSION)
        }
    }
}

/// RGB palette, light — borrowed from VSCode's `light_plus.json`/
/// `light_vs.json` (spec 0116 §9's "RGB palette" table; scope names
/// cited there).
fn style_for_light_rgb(role: SyntaxRole) -> Style {
    match role {
        SyntaxRole::Attribute => Style::default().fg(light_rgb::ATTRIBUTE),
        SyntaxRole::Type => Style::default().fg(light_rgb::TYPE),
        SyntaxRole::StringLiteral => Style::default().fg(light_rgb::STRING_LITERAL),
        SyntaxRole::StringEscape => Style::default().fg(light_rgb::STRING_ESCAPE),
        SyntaxRole::StringSpecialUrl => Style::default()
            .fg(light_rgb::TYPE)
            .add_modifier(Modifier::UNDERLINED),
        SyntaxRole::Comment => Style::default().fg(light_rgb::COMMENT),
        SyntaxRole::Number => Style::default().fg(light_rgb::NUMBER),
        SyntaxRole::Boolean => Style::default().fg(light_rgb::BOOLEAN),
        SyntaxRole::Constant => Style::default().fg(light_rgb::TYPE),
        SyntaxRole::PunctuationDelimiter => Style::default(),
        SyntaxRole::PunctuationBracket => Style::default(),
        SyntaxRole::PunctuationBracketList => {
            Style::default().fg(light_rgb::PUNCTUATION_BRACKET_LIST)
        }
        SyntaxRole::PunctuationBracketExtension => {
            Style::default().fg(light_rgb::PUNCTUATION_BRACKET_EXTENSION)
        }
    }
}

/// ANSI-16 fallback palette, dark (spec 0116 §9's "ANSI-16 palette"
/// table) — unchanged from this spec's original implementation.
fn style_for_dark_ansi16(role: SyntaxRole) -> Style {
    match role {
        SyntaxRole::Attribute => Style::default(),
        SyntaxRole::Type => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        SyntaxRole::StringLiteral => Style::default().fg(Color::Green),
        SyntaxRole::StringEscape => Style::default()
            .fg(Color::LightGreen)
            .add_modifier(Modifier::BOLD),
        SyntaxRole::StringSpecialUrl => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::UNDERLINED),
        SyntaxRole::Comment => Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
        SyntaxRole::Number => Style::default().fg(Color::Blue),
        SyntaxRole::Boolean => Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
        SyntaxRole::Constant => Style::default().fg(Color::Magenta),
        SyntaxRole::PunctuationDelimiter => Style::default().fg(Color::DarkGray),
        SyntaxRole::PunctuationBracket => Style::default().fg(Color::Gray),
        SyntaxRole::PunctuationBracketList => Style::default().fg(Color::Yellow),
        SyntaxRole::PunctuationBracketExtension => Style::default().fg(Color::LightRed),
    }
}

/// ANSI-16 fallback palette, light (spec 0116 §9's "ANSI-16 palette"
/// table) — unchanged from this spec's original implementation.
fn style_for_light_ansi16(role: SyntaxRole) -> Style {
    match role {
        SyntaxRole::Attribute => Style::default(),
        SyntaxRole::Type => Style::default()
            .fg(Color::Blue)
            .add_modifier(Modifier::BOLD),
        SyntaxRole::StringLiteral => Style::default().fg(Color::Green),
        SyntaxRole::StringEscape => Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
        SyntaxRole::StringSpecialUrl => Style::default()
            .fg(Color::Blue)
            .add_modifier(Modifier::UNDERLINED),
        SyntaxRole::Comment => Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
        SyntaxRole::Number => Style::default().fg(Color::Cyan),
        SyntaxRole::Boolean => Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
        SyntaxRole::Constant => Style::default().fg(Color::Magenta),
        SyntaxRole::PunctuationDelimiter => Style::default().fg(Color::DarkGray),
        SyntaxRole::PunctuationBracket => Style::default().fg(Color::Black),
        SyntaxRole::PunctuationBracketList => Style::default().fg(Color::Yellow),
        SyntaxRole::PunctuationBracketExtension => Style::default().fg(Color::Red),
    }
}

/// Manage-pane auto-override type-label color (spec 0130, restyled
/// 2026-07-18): a small, standalone style function independent of
/// `SyntaxRole`/`RECOGNIZED_NAMES` (those are strictly one variant per
/// `queries/highlights.scm` capture name; this has no corresponding
/// syntax capture). Only `auto` entries get a dedicated color, mirroring
/// `Comment`'s values (minus `ITALIC`) — manual entries render in the
/// terminal's plain default style, so only auto-derived entries stand
/// out. (The manage-pane origin-path header row has its own, separate
/// styling — `theme::style_for(SyntaxRole::Boolean, theme)`, applied
/// directly by `render_manage_pane`, not through this function.)
pub fn manage_entry_style(auto: bool, theme: ThemeKind) -> Style {
    if !auto {
        return Style::default();
    }
    match theme {
        ThemeKind::Dark if supports_rgb() => Style::default().fg(dark_rgb::COMMENT),
        ThemeKind::Dark => Style::default().fg(Color::DarkGray),
        ThemeKind::Light if supports_rgb() => Style::default().fg(light_rgb::COMMENT),
        ThemeKind::Light => Style::default().fg(Color::DarkGray),
        ThemeKind::System => {
            unreachable!("ThemeKind::System must be resolved before rendering — see main.rs")
        }
    }
}

/// Purpose-designed red for the brace pair of the node under the cursor
/// (spec 0193 S2) — not borrowed from `dark_rgb`/`light_rgb`, whose
/// VSCode source expresses its own bracket-match cue as a *border*
/// around the cell, which a terminal cell cannot draw. Follows
/// `heat_rgb`'s precedent of a small purpose-built palette instead.
mod brace_rgb {
    use ratatui::style::Color;

    /// "Red Orange" — bright against a dark background, and still
    /// legible once the cursor row's `REVERSED` turns it into one.
    pub const DARK: Color = Color::Rgb(0xFF, 0x45, 0x3A);
    /// "Boston University Red" — dark enough to read on a light
    /// background, where `DARK`'s orange cast washes out.
    pub const LIGHT: Color = Color::Rgb(0xCC, 0x00, 0x00);
}

/// Spec 0193 S2: the brace pair of the message/group under the cursor —
/// its opening `{` and its matching `}`, including the synthetic one a
/// folded node renders as `{ ... }`.
///
/// Like `manage_entry_style` above, this is a standalone style function
/// rather than a `SyntaxRole`: `SyntaxRole`/`RECOGNIZED_NAMES` are
/// strictly one variant per `queries/highlights.scm` capture name, and
/// where the cursor happens to be is not something tree-sitter can
/// capture. The *hue* is deliberately theme-independent — red in both,
/// like `focus_style`'s accent — since the point is salience rather
/// than palette harmony; only the shade adapts, for contrast against
/// the respective background.
pub fn brace_match_style(theme: ThemeKind) -> Style {
    let color = match theme {
        ThemeKind::Dark if supports_rgb() => brace_rgb::DARK,
        ThemeKind::Dark => Color::LightRed,
        ThemeKind::Light if supports_rgb() => brace_rgb::LIGHT,
        ThemeKind::Light => Color::Red,
        ThemeKind::System => {
            unreachable!("ThemeKind::System must be resolved before rendering — see main.rs")
        }
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

/// Focused-pane local-statusline style, shared by every focus-tracked
/// pane (main/override/manage — see `tui/mod.rs`'s `pane_focus_style`).
/// Vim-style inverted video (`REVERSED`): `Color::White` is ANSI code 15
/// ("bright white"), the brightest color available, so the focused
/// pane's statusline reads as a distinctly brighter bar than
/// `unfocused_pane_style`'s dimmer `Gray` (ANSI 7) once both are
/// reversed — deliberately theme-independent (same accent in both
/// `Dark`/`Light`), unlike `style_for`'s own RGB-vs-theme dispatch
/// (2026-07-17, reversed-video styling added 2026-07-19).
pub fn focus_style(theme: ThemeKind) -> Style {
    match theme {
        ThemeKind::System => {
            unreachable!("ThemeKind::System must be resolved before rendering — see main.rs")
        }
        ThemeKind::Dark | ThemeKind::Light => Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::REVERSED)
            .add_modifier(Modifier::BOLD),
    }
}

/// Unfocused-pane local-statusline style, paired with `focus_style`
/// above — plain gray, also reversed, so both statuslines read as solid
/// bars (vim-style) while the focused pane's brighter white still
/// stands out by contrast.
pub fn unfocused_pane_style() -> Style {
    Style::default()
        .fg(Color::Gray)
        .add_modifier(Modifier::REVERSED)
}

/// True-color 12-stop "heat" gradients (spec 0138 G6), dimmest (index 0,
/// level 1) to brightest (index 11, level 12) — purpose-designed for the
/// main-pane inference-mismatch cue, not reused from `dark_rgb`/
/// `light_rgb`'s `SyntaxRole`-driven palettes (none of which form a
/// natural 12-step heat ramp).
mod heat_rgb {
    use ratatui::style::Color;

    /// Dark theme, "ember → flame".
    pub const DARK: [Color; 12] = [
        Color::Rgb(0x3D, 0x20, 0x20),
        Color::Rgb(0x4A, 0x24, 0x20),
        Color::Rgb(0x57, 0x28, 0x22),
        Color::Rgb(0x6B, 0x2E, 0x22),
        Color::Rgb(0x7F, 0x34, 0x20),
        Color::Rgb(0x96, 0x39, 0x1C),
        Color::Rgb(0xAD, 0x40, 0x18),
        Color::Rgb(0xC4, 0x49, 0x13),
        Color::Rgb(0xDB, 0x54, 0x0D),
        Color::Rgb(0xF0, 0x60, 0x08),
        Color::Rgb(0xFF, 0x7A, 0x04),
        Color::Rgb(0xFF, 0xAC, 0x06),
    ];

    /// Light theme, "pale → deep red" (ColorBrewer OrRd-inspired).
    pub const LIGHT: [Color; 12] = [
        Color::Rgb(0xFD, 0xED, 0xE4),
        Color::Rgb(0xFC, 0xE0, 0xD0),
        Color::Rgb(0xFB, 0xD0, 0xB8),
        Color::Rgb(0xF8, 0xB8, 0x9C),
        Color::Rgb(0xF4, 0x9E, 0x7E),
        Color::Rgb(0xED, 0x82, 0x61),
        Color::Rgb(0xE3, 0x67, 0x49),
        Color::Rgb(0xD3, 0x4E, 0x36),
        Color::Rgb(0xBE, 0x38, 0x26),
        Color::Rgb(0xA2, 0x23, 0x1A),
        Color::Rgb(0x86, 0x12, 0x10),
        Color::Rgb(0x6E, 0x10, 0x04),
    ];

    /// Dark theme, "ice → azure" (spec 0138 G11, the `Tie` cue's hue) —
    /// derived by swapping the R/B channels of `DARK` above, so it
    /// carries the exact same luminance progression as the red
    /// "Mismatch" gradient, in a blue hue rather than red.
    pub const DARK_BLUE: [Color; 12] = [
        Color::Rgb(0x20, 0x20, 0x3D),
        Color::Rgb(0x20, 0x24, 0x4A),
        Color::Rgb(0x22, 0x28, 0x57),
        Color::Rgb(0x22, 0x2E, 0x6B),
        Color::Rgb(0x20, 0x34, 0x7F),
        Color::Rgb(0x1C, 0x39, 0x96),
        Color::Rgb(0x18, 0x40, 0xAD),
        Color::Rgb(0x13, 0x49, 0xC4),
        Color::Rgb(0x0D, 0x54, 0xDB),
        Color::Rgb(0x08, 0x60, 0xF0),
        Color::Rgb(0x04, 0x7A, 0xFF),
        Color::Rgb(0x06, 0xAC, 0xFF),
    ];

    /// Light theme, "pale → deep blue" — same R/B-channel-swap
    /// derivation from `LIGHT` as `DARK_BLUE` is from `DARK`.
    pub const LIGHT_BLUE: [Color; 12] = [
        Color::Rgb(0xE4, 0xED, 0xFD),
        Color::Rgb(0xD0, 0xE0, 0xFC),
        Color::Rgb(0xB8, 0xD0, 0xFB),
        Color::Rgb(0x9C, 0xB8, 0xF8),
        Color::Rgb(0x7E, 0x9E, 0xF4),
        Color::Rgb(0x61, 0x82, 0xED),
        Color::Rgb(0x49, 0x67, 0xE3),
        Color::Rgb(0x36, 0x4E, 0xD3),
        Color::Rgb(0x26, 0x38, 0xBE),
        Color::Rgb(0x1A, 0x23, 0xA2),
        Color::Rgb(0x10, 0x12, 0x86),
        Color::Rgb(0x04, 0x10, 0x6E),
    ];
}

/// Hue selector for the main-pane heat cue (spec 0138 G9-G12): `Red` is
/// the original `Mismatch` cue ("current type scores below best");
/// `Blue` is the newer `Tie` cue ("current type ties for best") — both
/// share the same brightness-level model (`heat_style`'s `level`
/// parameter), differing only in which 12-stop gradient/ANSI-16 pair is
/// used.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HeatHue {
    Red,
    Blue,
}

/// Main-pane heat cue color for the leading glyph column (spec 0138,
/// item 12 of 2026-07-17 feedback; `hue` added for the `Tie` cue, G9-
/// G12). `level` is 1..=12 (see `tui::heat_cue::heat_level`), already
/// gated present by the caller (G4 for `Red`/`Mismatch`, G9 for `Blue`/
/// `Tie`). Returns `None` when the cue must not be shown at all on this
/// terminal — only possible on the ANSI-16 fallback, for `level <= 3`
/// (`best_score <= 3`, G7/G12's low-confidence narrowing of the gate);
/// the truecolor gradient always shows *some* color once the gate has
/// passed, however dim.
pub fn heat_style(level: u8, hue: HeatHue, theme: ThemeKind) -> Option<Style> {
    match theme {
        ThemeKind::Dark if supports_rgb() => {
            Some(Style::default().fg(heat_rgb_color(level, false, hue)))
        }
        ThemeKind::Light if supports_rgb() => {
            Some(Style::default().fg(heat_rgb_color(level, true, hue)))
        }
        ThemeKind::Dark | ThemeKind::Light if level <= 3 => None,
        ThemeKind::Dark | ThemeKind::Light if level <= 7 => Some(Style::default().fg(match hue {
            HeatHue::Red => Color::Red,
            HeatHue::Blue => Color::Blue,
        })),
        ThemeKind::Dark | ThemeKind::Light => Some(Style::default().fg(match hue {
            HeatHue::Red => Color::LightRed,
            HeatHue::Blue => Color::LightBlue,
        })),
        ThemeKind::System => {
            unreachable!("ThemeKind::System must be resolved before rendering — see main.rs")
        }
    }
}

fn heat_rgb_color(level: u8, light: bool, hue: HeatHue) -> Color {
    let idx = level.clamp(1, 12) as usize - 1;
    match (light, hue) {
        (false, HeatHue::Red) => heat_rgb::DARK[idx],
        (true, HeatHue::Red) => heat_rgb::LIGHT[idx],
        (false, HeatHue::Blue) => heat_rgb::DARK_BLUE[idx],
        (true, HeatHue::Blue) => heat_rgb::LIGHT_BLUE[idx],
    }
}

/// The `Mismatch` heat cue's ` [current/best]` suffix color — always the
/// brightest available red (truecolor level 12, or `Color::LightRed` on
/// the ANSI-16 fallback) whenever the cue is present at all, regardless
/// of `level` (spec 0138 N1) — unlike `heat_style`, which grades the
/// leading glyph by `level`. The `Tie` cue's own ` [tie_count@score]`
/// suffix uses no dedicated function of its own — it's styled with
/// `style_for(SyntaxRole::Boolean, theme)` directly, i.e. literally the
/// same styling already used for a `true`/`false` value (spec 0138 G9's
/// own wording), not a new color.
pub fn heat_suffix_style(theme: ThemeKind) -> Style {
    match theme {
        ThemeKind::Dark if supports_rgb() => Style::default().fg(heat_rgb::DARK[11]),
        ThemeKind::Light if supports_rgb() => Style::default().fg(heat_rgb::LIGHT[11]),
        ThemeKind::Dark | ThemeKind::Light => Style::default().fg(Color::LightRed),
        ThemeKind::System => {
            unreachable!("ThemeKind::System must be resolved before rendering — see main.rs")
        }
    }
}

/// Resolves `ThemeKind::System` to `Dark` or `Light`, once, at startup
/// (spec 0116 §9's "Selection mechanism"):
///
/// 1. `COLORFGBG` env var, if set (some terminals export `fg;bg` ANSI
///    color indices; no terminal I/O needed) — `terminal_light::env::
///    bg_color()`.
/// 2. Otherwise, an OSC 11 query (bounded timeout), via
///    `terminal_light::luma()` — handles tmux/screen passthrough.
/// 3. If neither yields an answer, falls back to `Dark`.
pub fn resolve_system() -> ThemeKind {
    if let Ok(ansi) = terminal_light::env::bg_color() {
        return theme_for_luma(terminal_light::Color::from(ansi).luma());
    }
    match terminal_light::luma() {
        Ok(luma) => theme_for_luma(luma),
        Err(_) => ThemeKind::Dark,
    }
}

/// Threshold matching `terminal-light`'s own doc example (`luma() >
/// 0.6`) for a single dark/light pivot.
fn theme_for_luma(luma: f32) -> ThemeKind {
    if luma > 0.6 {
        ThemeKind::Light
    } else {
        ThemeKind::Dark
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guards every test below that mutates `COLORFGBG`/`COLORTERM`.
    /// `cargo test` runs tests in parallel threads within one process by
    /// default, and env vars are process-global — without this, two such
    /// tests running concurrently can observe each other's set/remove
    /// calls mid-assertion (this caused a real, intermittent failure).
    /// `.unwrap_or_else(...)` shields against lock poisoning from an
    /// earlier panicking test so later tests still run.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn resolve_system_reads_colorfgbg_dark() {
        let _guard = lock_env();
        // SAFETY: single-threaded (guarded by ENV_MUTEX above).
        unsafe {
            std::env::set_var("COLORFGBG", "15;0");
        }
        assert_eq!(resolve_system(), ThemeKind::Dark);
        unsafe {
            std::env::remove_var("COLORFGBG");
        }
    }

    #[test]
    fn resolve_system_reads_colorfgbg_light() {
        let _guard = lock_env();
        // SAFETY: single-threaded (guarded by ENV_MUTEX above).
        unsafe {
            std::env::set_var("COLORFGBG", "0;15");
        }
        assert_eq!(resolve_system(), ThemeKind::Light);
        unsafe {
            std::env::remove_var("COLORFGBG");
        }
    }

    const ALL_ROLES: [SyntaxRole; 13] = [
        SyntaxRole::Attribute,
        SyntaxRole::Type,
        SyntaxRole::StringLiteral,
        SyntaxRole::StringEscape,
        SyntaxRole::StringSpecialUrl,
        SyntaxRole::Comment,
        SyntaxRole::Number,
        SyntaxRole::Boolean,
        SyntaxRole::Constant,
        SyntaxRole::PunctuationDelimiter,
        SyntaxRole::PunctuationBracket,
        SyntaxRole::PunctuationBracketList,
        SyntaxRole::PunctuationBracketExtension,
    ];

    // `PunctuationDelimiter`/`PunctuationBracket` are deliberately
    // unstyled (terminal default) in both the RGB and ANSI-16 palettes
    // — excluded from the "must be Rgb" assertion below.
    const COLORED_ROLES: [SyntaxRole; 11] = [
        SyntaxRole::Attribute,
        SyntaxRole::Type,
        SyntaxRole::StringLiteral,
        SyntaxRole::StringEscape,
        SyntaxRole::StringSpecialUrl,
        SyntaxRole::Comment,
        SyntaxRole::Number,
        SyntaxRole::Boolean,
        SyntaxRole::Constant,
        SyntaxRole::PunctuationBracketList,
        SyntaxRole::PunctuationBracketExtension,
    ];

    #[test]
    fn style_for_uses_ansi16_when_colorterm_absent() {
        let _guard = lock_env();
        // SAFETY: single-threaded (guarded by ENV_MUTEX above).
        unsafe {
            std::env::remove_var("COLORTERM");
        }
        // `terminfo_reports_rgb` is cached process-wide and reflects the
        // *actual* environment `cargo test` runs in — if it's genuinely
        // true-color-capable via terminfo (e.g. a dev's real terminal,
        // as opposed to a headless CI sandbox), `supports_rgb` legitimately
        // returns true even with `COLORTERM` unset, so there's nothing to
        // assert here.
        if terminfo_reports_rgb() {
            return;
        }
        for role in ALL_ROLES {
            for theme in [ThemeKind::Dark, ThemeKind::Light] {
                let style = style_for(role, theme);
                assert!(!matches!(
                    style.fg,
                    Some(Color::Rgb(..)) | Some(Color::Indexed(_))
                ));
            }
        }
    }

    #[test]
    fn style_for_uses_rgb_when_colorterm_truecolor() {
        let _guard = lock_env();
        // SAFETY: single-threaded (guarded by ENV_MUTEX above).
        unsafe {
            std::env::set_var("COLORTERM", "truecolor");
        }
        for role in COLORED_ROLES {
            for theme in [ThemeKind::Dark, ThemeKind::Light] {
                let style = style_for(role, theme);
                assert!(matches!(style.fg, Some(Color::Rgb(..))));
            }
        }
        unsafe {
            std::env::remove_var("COLORTERM");
        }
    }

    #[test]
    fn heat_style_uses_rgb_gradient_when_colorterm_truecolor() {
        let _guard = lock_env();
        // SAFETY: single-threaded (guarded by ENV_MUTEX above).
        unsafe {
            std::env::set_var("COLORTERM", "truecolor");
        }
        for theme in [ThemeKind::Dark, ThemeKind::Light] {
            for hue in [HeatHue::Red, HeatHue::Blue] {
                let level1 = heat_style(1, hue, theme).unwrap();
                let level12 = heat_style(12, hue, theme).unwrap();
                assert!(matches!(level1.fg, Some(Color::Rgb(..))));
                assert!(matches!(level12.fg, Some(Color::Rgb(..))));
                assert_ne!(level1.fg, level12.fg, "brightness must vary by level");
                // Out-of-range levels clamp rather than panic.
                assert_eq!(heat_style(0, hue, theme), heat_style(1, hue, theme));
                assert_eq!(heat_style(200, hue, theme), heat_style(12, hue, theme));
            }
            // Same level, different hue: distinct colors (G11).
            assert_ne!(
                heat_style(6, HeatHue::Red, theme),
                heat_style(6, HeatHue::Blue, theme)
            );
        }
        unsafe {
            std::env::remove_var("COLORTERM");
        }
    }

    #[test]
    fn heat_style_ansi16_fallback_thresholds() {
        let _guard = lock_env();
        // SAFETY: single-threaded (guarded by ENV_MUTEX above).
        unsafe {
            std::env::remove_var("COLORTERM");
        }
        if terminfo_reports_rgb() {
            return;
        }
        for theme in [ThemeKind::Dark, ThemeKind::Light] {
            // Level 3 == `best_score <= 3` (G7/G12's low-confidence absence).
            assert_eq!(heat_style(3, HeatHue::Red, theme), None);
            assert_eq!(heat_style(3, HeatHue::Blue, theme), None);
            // Level 7 == `best_score <= 21`: dark red / dark blue.
            assert_eq!(
                heat_style(7, HeatHue::Red, theme),
                Some(Style::default().fg(Color::Red))
            );
            assert_eq!(
                heat_style(7, HeatHue::Blue, theme),
                Some(Style::default().fg(Color::Blue))
            );
            // Level 8 == `best_score > 21`: bright red / bright blue.
            assert_eq!(
                heat_style(8, HeatHue::Red, theme),
                Some(Style::default().fg(Color::LightRed))
            );
            assert_eq!(
                heat_style(8, HeatHue::Blue, theme),
                Some(Style::default().fg(Color::LightBlue))
            );
        }
    }

    #[test]
    fn heat_suffix_style_is_always_the_brightest_red() {
        let _guard = lock_env();
        // SAFETY: single-threaded (guarded by ENV_MUTEX above).
        unsafe {
            std::env::set_var("COLORTERM", "truecolor");
        }
        for theme in [ThemeKind::Dark, ThemeKind::Light] {
            assert!(matches!(heat_suffix_style(theme).fg, Some(Color::Rgb(..))));
        }
        unsafe {
            std::env::remove_var("COLORTERM");
        }
        if !terminfo_reports_rgb() {
            for theme in [ThemeKind::Dark, ThemeKind::Light] {
                assert_eq!(
                    heat_suffix_style(theme),
                    Style::default().fg(Color::LightRed)
                );
            }
        }
    }

    #[test]
    fn database_reports_rgb_true_capability() {
        let mut builder = terminfo::Database::new();
        builder.name("test");
        builder.raw("RGB", ());
        assert!(database_reports_rgb(&builder.build().unwrap()));
    }

    #[test]
    fn database_reports_rgb_tc_capability() {
        let mut builder = terminfo::Database::new();
        builder.name("test");
        builder.raw("Tc", ());
        assert!(database_reports_rgb(&builder.build().unwrap()));
    }

    #[test]
    fn database_reports_rgb_max_colors_sentinel() {
        let mut builder = terminfo::Database::new();
        builder.name("test");
        builder.set(terminfo::capability::MaxColors(0x0100_0000));
        assert!(database_reports_rgb(&builder.build().unwrap()));
    }

    #[test]
    fn database_reports_rgb_false_for_plain_256color() {
        let mut builder = terminfo::Database::new();
        builder.name("test");
        builder.set(terminfo::capability::MaxColors(256));
        assert!(!database_reports_rgb(&builder.build().unwrap()));
    }

    #[test]
    fn parse_xtgettcap_response_true_on_success() {
        assert!(parse_xtgettcap_response("\x1bP1+r524742\x1b\\"));
    }

    #[test]
    fn parse_xtgettcap_response_false_on_failure() {
        assert!(!parse_xtgettcap_response("\x1bP0+r\x1b\\"));
    }

    #[test]
    fn parse_xtgettcap_response_false_on_garbage() {
        assert!(!parse_xtgettcap_response("not a response"));
    }
}
