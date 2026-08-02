// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Theme: maps a `SyntaxRole` to a `ratatui::style::Style` (spec 0116
//! §7, §9). Two built-in palette pairs (dark, light), each in two color
//! depths: RGB, borrowed from VSCode Dark+/Light+, and ANSI-16, a
//! portable fallback — picked by `supports_rgb`. The `System` selector
//! is resolved once at startup.

use ratatui::style::{Color, Modifier, Style};

use crate::annotation::Tier;
use crate::colorize::SyntaxRole;

/// The `--theme` CLI flag's three fixed choices (spec 0116 §9).
/// `System` exists only at the CLI-selection layer: it is resolved to
/// `Dark` or `Light` once at startup, before any rendering, so
/// `style_for` only ever sees a resolved variant.
#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub enum ThemeKind {
    Dark,
    Light,
    System,
}

/// The promise `resolve_system` makes, stated once: by the time any
/// style function runs, `--theme system` has already become `Dark` or
/// `Light`. Reaching this is a programming error in the startup path,
/// not a condition a render can encounter.
///
/// A function rather than the seven copies of the same `unreachable!`
/// this file used to carry — the message names `main.rs` as the place
/// the resolution happens, so it is a message worth having exactly one
/// of.
fn system_must_be_resolved() -> ! {
    unreachable!("ThemeKind::System must be resolved before rendering — see main.rs")
}

/// Chooses one of the four members of a palette pair — `Dark`/`Light`
/// times truecolor/ANSI-16 — for the style functions whose entire body
/// is that choice. Each pair argument is `(truecolor, ansi16)`, in the
/// order the destructuring below reads them.
///
/// This also takes `ThemeKind::System` off those callers entirely: they
/// state neither the resolution promise nor the color-depth branch, so
/// there is no fifth copy of either to keep in step, and a new style
/// function of this shape cannot forget one.
///
/// `rgb` is a parameter rather than a `supports_rgb()` call because some
/// callers must be able to ask for either palette from a test — see
/// `style_for_in` for why probing it here would make the ANSI-16 one
/// unreachable.
fn pick<T>(theme: ThemeKind, rgb: bool, dark: (T, T), light: (T, T)) -> T {
    let (truecolor, ansi16) = match theme {
        ThemeKind::Dark => dark,
        ThemeKind::Light => light,
        ThemeKind::System => system_must_be_resolved(),
    };
    if rgb {
        truecolor
    } else {
        ansi16
    }
}

/// Maps `role` to a `Style`. `theme` must already be resolved to
/// `Dark`/`Light` (see `resolve_system`); passing `System` here is a
/// programming error.
///
/// Picks between an RGB palette (borrowed from VSCode's Dark+/Light+
/// themes) and a portable ANSI-16 fallback via `supports_rgb`
/// (spec 0116 §9).
pub fn style_for(role: SyntaxRole, theme: ThemeKind) -> Style {
    style_for_in(role, theme, supports_rgb())
}

/// `style_for` with the color depth passed in rather than probed.
///
/// The split exists for the tests: `supports_rgb` reads the ambient
/// terminal through two process-wide caches, so a test that wanted the
/// ANSI-16 palette could only ask for it by unsetting `COLORTERM` and
/// hoping the terminal `cargo test` runs under is not itself
/// true-color-capable — which on a modern development machine it is, so
/// the test skipped itself and reported a pass. Taking the flag as an
/// argument makes both palettes reachable from a test with no
/// environment mutation and no skipping.
fn style_for_in(role: SyntaxRole, theme: ThemeKind, rgb: bool) -> Style {
    match (theme, rgb) {
        (ThemeKind::Dark, true) => style_for_rgb(role, &DARK_RGB),
        (ThemeKind::Dark, false) => style_for_dark_ansi16(role),
        (ThemeKind::Light, true) => style_for_rgb(role, &LIGHT_RGB),
        (ThemeKind::Light, false) => style_for_light_ansi16(role),
        (ThemeKind::System, _) => system_must_be_resolved(),
    }
}

/// Whether the terminal advertises 24-bit color support, checked in the
/// same order Vim does (patch 9.1.1060, vim/vim#16490): `COLORTERM=
/// truecolor`/`24bit` (the signal `bat`, `delta` and most other Rust
/// terminal tools key off — an uncached env lookup) first; then a live
/// XTGETTCAP query (`xtgettcap_reports_rgb`, cached); then a static
/// terminfo capability probe (`terminfo_reports_rgb`, cached) for
/// terminals that don't answer the live query.
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

/// Live XTGETTCAP fallback for when `COLORTERM` isn't set — Vim's
/// *primary* true-color signal (patch 9.1.1060, vim/vim#16490): query
/// the live terminal, not just its static terminfo entry, for the `RGB`
/// termcap capability. Some terminals answer correctly even though
/// their terminfo entry advertises none of `RGB`/`Tc`/
/// `max_colors=16777216`; `terminfo_reports_rgb` covers the terminals
/// that don't answer this live query at all (e.g. some xterm builds).
///
/// Only attempted when both stdin and stdout are real terminals — under
/// `cargo test` and other non-interactive contexts this is false, so no
/// terminal I/O happens and the answer is `false`.
///
/// Cached in a `OnceLock`: the answer cannot change during a single
/// process's lifetime, and — unlike a static terminfo lookup —
/// repeating this query would mean repeated terminal round-trips.
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

/// Performs the XTGETTCAP round-trip. Mirrors `terminal-light`'s own
/// `xterm.rs::query` raw-mode-wrapping pattern: temporarily enables raw
/// mode (if not already enabled) so the response isn't held back
/// waiting for a newline, then restores the prior mode.
///
/// Must run before the TUI's crossterm event loop starts polling the
/// terminal (see `main.rs`'s `theme::prime_supports_rgb` call) — two
/// concurrent readers of the tty would race.
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
/// XTGETTCAP query confirm true-color support — mirrors Vim (patch
/// 9.1.1060, vim/vim#16490): query the terminal's *static* terminfo
/// entry for the non-standard `RGB`/`Tc` boolean capabilities, or a
/// `max_colors` value of `0x1000000` (16,777,216), the sentinel some
/// terminfo entries (e.g. `xterm-direct`) use for true color.
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

/// Forces early, one-time evaluation of the cached
/// `xtgettcap_reports_rgb` result. Must be called before `tui::run`
/// takes over the terminal with raw mode + the alternate screen and
/// starts its own crossterm event-polling loop — otherwise the probe's
/// read and the TUI's input handling race over the same tty.
pub fn prime_supports_rgb() {
    xtgettcap_reports_rgb();
}

/// The nine colors an RGB palette names (spec 0116 §9's "RGB palette"
/// table), as one struct rather than as a module of constants per
/// palette — so the `SyntaxRole` mapping below is written once instead
/// of once per palette. The two used to be verbatim copies differing
/// only in a module path.
///
/// Field names mirror the `SyntaxRole` each color serves; VSCode, which
/// these are borrowed from, has no equivalent naming, only semantic
/// scope names. A color serving more than one role is one field
/// referenced twice, never a value repeated.
struct RgbPalette {
    attribute: Color,
    /// Also VSCode's link color (`StringSpecialUrl`) and `Constant`, and
    /// this crate's own focused-pane accent (`focus_style`).
    r#type: Color,
    string_literal: Color,
    string_escape: Color,
    /// Also this crate's manage-pane "auto" entry color
    /// (`manage_entry_style`).
    comment: Color,
    number: Color,
    /// Also this crate's manage-pane origin-path header color
    /// (`render_manage_pane`, via `style_for`).
    boolean: Color,
    punctuation_bracket_list: Color,
    punctuation_bracket_extension: Color,
    /// The three severity tiers (spec 0225 §S11). Deliberately not
    /// reusing a color above: a tier must be legible *as* a tier next to
    /// ordinary syntax on the same line, and next to the hex of a wire
    /// row, so sharing a hue with a role would make the two readings
    /// ambiguous exactly where both appear.
    tier_landmark: Color,
    tier_non_canonical: Color,
    tier_invalid: Color,
}

/// The dark RGB palette, borrowed from VSCode's `dark_plus.json`/
/// `dark_vs.json`. Each trailing comment is that color's closest
/// named-color match from <https://www.color-name.com>, purely for human
/// readability when scanning this file.
const DARK_RGB: RgbPalette = RgbPalette {
    attribute: Color::Rgb(0x9C, 0xDC, 0xFE),      // Clear Blue
    r#type: Color::Rgb(0x4E, 0xC9, 0xB0),         // Subtle Blue Green
    string_literal: Color::Rgb(0xCE, 0x91, 0x78), // Beauty Copper
    string_escape: Color::Rgb(0xD7, 0xBA, 0x7D),  // Mushroom Melt
    comment: Color::Rgb(0x6A, 0x99, 0x55),        // Brussels Sprout
    number: Color::Rgb(0xB5, 0xCE, 0xA8),         // Rainee
    boolean: Color::Rgb(0x56, 0x9C, 0xD6),        // Azul Mystic
    punctuation_bracket_list: Color::Rgb(0xDC, 0xDC, 0xAA), // Pale Hazel
    punctuation_bracket_extension: Color::Rgb(0xD1, 0x69, 0x69), // Alexa
    tier_landmark: Color::Rgb(0xC5, 0x86, 0xC0),  // Light Violet
    tier_non_canonical: Color::Rgb(0xCC, 0xA7, 0x00), // Buddha Gold
    tier_invalid: Color::Rgb(0xF1, 0x4C, 0x4C),   // Fire Opal
};

/// The light RGB palette, borrowed from VSCode's `light_plus.json`/
/// `light_vs.json`. See `DARK_RGB` for the trailing-comment convention.
const LIGHT_RGB: RgbPalette = RgbPalette {
    attribute: Color::Rgb(0xE5, 0x00, 0x00),      // Electric Red
    r#type: Color::Rgb(0x26, 0x7F, 0x99),         // Jelly Bean Blue
    string_literal: Color::Rgb(0xA3, 0x15, 0x15), // San Diego
    string_escape: Color::Rgb(0xEE, 0x00, 0x00),  // Strong Red
    comment: Color::Rgb(0x00, 0x80, 0x00),        // Digital Green
    number: Color::Rgb(0x09, 0x86, 0x58),         // Funky Green
    boolean: Color::Rgb(0x00, 0x00, 0xFF),        // Blue
    punctuation_bracket_list: Color::Rgb(0x04, 0x51, 0xA5), // French Blue
    punctuation_bracket_extension: Color::Rgb(0x81, 0x1F, 0x3F), // Dried Burgundy
    tier_landmark: Color::Rgb(0xAF, 0x00, 0xDB),  // Violet
    tier_non_canonical: Color::Rgb(0xBF, 0x88, 0x03), // Golden Brown
    tier_invalid: Color::Rgb(0xE5, 0x14, 0x00),   // Scarlet
};

/// Which color of `p` each role takes, and the one modifier the RGB
/// palettes carry (spec 0116 §9's "RGB palette" table; scope names cited
/// there).
///
/// One function for both palettes: the dark and light tables never
/// differed in which role got which named color, only in what the names
/// resolve to.
fn style_for_rgb(role: SyntaxRole, p: &RgbPalette) -> Style {
    match role {
        SyntaxRole::Attribute => Style::default().fg(p.attribute),
        SyntaxRole::Type => Style::default().fg(p.r#type),
        SyntaxRole::StringLiteral => Style::default().fg(p.string_literal),
        SyntaxRole::StringEscape => Style::default().fg(p.string_escape),
        SyntaxRole::StringSpecialUrl => Style::default()
            .fg(p.r#type)
            .add_modifier(Modifier::UNDERLINED),
        SyntaxRole::Comment => Style::default().fg(p.comment),
        SyntaxRole::Number => Style::default().fg(p.number),
        SyntaxRole::Boolean => Style::default().fg(p.boolean),
        SyntaxRole::Constant => Style::default().fg(p.r#type),
        SyntaxRole::PunctuationDelimiter => Style::default(),
        SyntaxRole::PunctuationBracket => Style::default(),
        SyntaxRole::PunctuationBracketList => Style::default().fg(p.punctuation_bracket_list),
        SyntaxRole::PunctuationBracketExtension => {
            Style::default().fg(p.punctuation_bracket_extension)
        }
        SyntaxRole::AnnotationLandmark => Style::default().fg(tier_color_rgb(Tier::Landmark, p)),
        SyntaxRole::AnnotationNonCanonical => {
            Style::default().fg(tier_color_rgb(Tier::NonCanonical, p))
        }
        SyntaxRole::AnnotationInvalid => Style::default().fg(tier_color_rgb(Tier::Invalid, p)),
    }
}

/// The severity color of `tier` (spec 0225 §S11).
///
/// Three one-line tables — this and the two ANSI-16 ones below — rather
/// than inlining the colors into the `SyntaxRole` arms above and into
/// `tier_style_in`. The annotation reaches a tier through a capture and
/// a role; the wire row names the tier outright; both must land on the
/// same color, and a second copy is how they would stop.
fn tier_color_rgb(tier: Tier, p: &RgbPalette) -> Color {
    match tier {
        Tier::Landmark => p.tier_landmark,
        Tier::NonCanonical => p.tier_non_canonical,
        Tier::Invalid => p.tier_invalid,
    }
}

fn tier_color_dark_ansi16(tier: Tier) -> Color {
    match tier {
        Tier::Landmark => Color::LightMagenta,
        Tier::NonCanonical => Color::Yellow,
        Tier::Invalid => Color::LightRed,
    }
}

fn tier_color_light_ansi16(tier: Tier) -> Color {
    match tier {
        Tier::Landmark => Color::Magenta,
        Tier::NonCanonical => Color::Yellow,
        Tier::Invalid => Color::Red,
    }
}

/// The style a severity tier wears, named directly rather than reached
/// through a syntax capture — what a wire row's bytes use (spec 0225
/// §S11 "one classifier, two rows").
///
/// Hue only. The wire row adds `REVERSED` itself, and only for the two
/// anomaly tiers: reverse is a *locator* for a pair of hex digits lost
/// among forty, not a severity, and a token in a sentence does not need
/// one.
pub fn tier_style(tier: Tier, theme: ThemeKind) -> Style {
    tier_style_in(tier, theme, supports_rgb())
}

/// `tier_style` with the color depth passed in — see `style_for_in` for
/// why the split exists.
fn tier_style_in(tier: Tier, theme: ThemeKind, rgb: bool) -> Style {
    Style::default().fg(pick(
        theme,
        rgb,
        (
            tier_color_rgb(tier, &DARK_RGB),
            tier_color_dark_ansi16(tier),
        ),
        (
            tier_color_rgb(tier, &LIGHT_RGB),
            tier_color_light_ansi16(tier),
        ),
    ))
}

/// The four parts of a wire row that carry a hue (spec 0225 S11).
///
/// A role of its own rather than the document row's `SyntaxRole`,
/// alongside `HeatHue` below — the other role in this file with no
/// grammar capture behind it. Painting a tag with `Attribute`'s style
/// would make the wire row an undeclared second consumer of a document
/// color: the field-name color could no longer be retuned without
/// moving the hex, and a span reporting `Attribute` when it is a tag
/// misdescribes itself. What is borrowed is the *color*, leveled, and
/// `wire_style` is the only place the borrowing happens.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WireRole {
    /// The tag's field-number bits — borrows the field name's color.
    Tag,
    /// The tag's wire-type nibble — borrows the color the annotation
    /// gives the type it names, which is a different fact from the
    /// field number and reads better as a different hue.
    Type,
    /// The length prefix — borrows the comment color.
    Length,
    /// The payload bytes — borrows the row's value's color.
    Payload,
}

/// The one brightness every borrowed hue is brought to.
///
/// Not a fraction of the document color: blending each hue toward the
/// background by a fixed amount keeps the palette's own brightness
/// spread, and a row mixing a bright tag with a dim payload is
/// uncomfortable to read at hex density. Leveling makes the wire row
/// read as one object, distinguished from the document row by
/// brightness and from itself by hue.
///
/// A legibility choice, not a measurement, with one hard bound each
/// way. On dark it must sit below the dimmest borrowed document color
/// or the "dim" becomes a brighten; on light, above the brightest.
/// Between those, higher is more comfortable and less subordinate.
const WIRE_LUMA_DARK: f32 = 130.0;
const WIRE_LUMA_LIGHT: f32 = 150.0;

/// The style a wire byte wears (spec 0225 S11): `borrowed`'s color,
/// brought to the wire row's brightness.
///
/// `borrowed` is the `SyntaxRole` the document row above carries at the
/// corresponding column, or `None` when it has none — `Length` supplies
/// its own, since a length prefix's color must not depend on whether
/// that particular row happens to carry an annotation.
pub fn wire_style(role: WireRole, borrowed: Option<SyntaxRole>, theme: ThemeKind) -> Style {
    let source = match role {
        WireRole::Length => Some(SyntaxRole::Comment),
        WireRole::Tag | WireRole::Type | WireRole::Payload => borrowed,
    };
    match source {
        Some(role) => dimmed(style_for(role, theme), theme),
        None => Style::default().add_modifier(Modifier::DIM),
    }
}

/// The same hue at the wire row's brightness (spec 0225 S11).
///
/// Which branch applies is read off the color itself rather than from a
/// second `supports_rgb()` call: `style_for` has already resolved which
/// palette is in play, so the color's shape is the exact discriminator
/// and cannot disagree with it.
///
/// Hue only, like `tier_style` — an inherited `UNDERLINED` or `ITALIC`
/// would be a locator on a row that has its own.
pub fn dimmed(style: Style, theme: ThemeKind) -> Style {
    match style.fg {
        Some(Color::Rgb(r, g, b)) => Style::default().fg(leveled(r, g, b, theme)),
        Some(color) => Style::default().fg(color).add_modifier(Modifier::DIM),
        None => Style::default().add_modifier(Modifier::DIM),
    }
}

/// Rec. 709 relative luminance, the standard weighting for how bright a
/// color looks rather than how large its channels are.
fn luma(r: f32, g: f32, b: f32) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// Blends a truecolor toward the palette's background — black on dark,
/// white on light — by exactly as much as it takes to land on
/// [`WIRE_LUMA_DARK`]/[`WIRE_LUMA_LIGHT`].
///
/// Luminance is affine in the blend factor, so the amount needed is a
/// division rather than a search, and the result's luminance is the
/// target exactly. A color already past the target is left alone: the
/// row is meant to recede, never to be pushed forward.
fn leveled(r: u8, g: u8, b: u8, theme: ThemeKind) -> Color {
    let (target, background) = match theme {
        ThemeKind::Dark => (WIRE_LUMA_DARK, 0.0),
        ThemeKind::Light => (WIRE_LUMA_LIGHT, 255.0),
        ThemeKind::System => system_must_be_resolved(),
    };
    let here = luma(f32::from(r), f32::from(g), f32::from(b));
    let span = background - here;
    if span.abs() < 1.0 {
        // The color is already the background's brightness; there is
        // nothing to blend toward and nothing worth showing either.
        return Color::Rgb(r, g, b);
    }
    let t = ((target - here) / span).clamp(0.0, 1.0);
    let mix = |c: u8| {
        let c = f32::from(c);
        (c + (background - c) * t) as u8
    };
    Color::Rgb(mix(r), mix(g), mix(b))
}

/// ANSI-16 fallback palette, dark (spec 0116 §9's "ANSI-16 palette"
/// table).
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
        SyntaxRole::AnnotationLandmark => {
            Style::default().fg(tier_color_dark_ansi16(Tier::Landmark))
        }
        SyntaxRole::AnnotationNonCanonical => {
            Style::default().fg(tier_color_dark_ansi16(Tier::NonCanonical))
        }
        SyntaxRole::AnnotationInvalid => Style::default().fg(tier_color_dark_ansi16(Tier::Invalid)),
    }
}

/// ANSI-16 fallback palette, light (spec 0116 §9's "ANSI-16 palette"
/// table).
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
        SyntaxRole::AnnotationLandmark => {
            Style::default().fg(tier_color_light_ansi16(Tier::Landmark))
        }
        SyntaxRole::AnnotationNonCanonical => {
            Style::default().fg(tier_color_light_ansi16(Tier::NonCanonical))
        }
        SyntaxRole::AnnotationInvalid => {
            Style::default().fg(tier_color_light_ansi16(Tier::Invalid))
        }
    }
}

/// Manage-pane auto-override type-label color (spec 0130): a standalone
/// style function independent of `SyntaxRole`/`RECOGNIZED_NAMES` (those
/// are strictly one variant per `queries/highlights.scm` capture name;
/// this has no corresponding syntax capture). Only `auto` entries get a dedicated color, mirroring
/// `Comment`'s values (minus `ITALIC`) — manual entries render in the
/// terminal's plain default style, so only auto-derived entries stand
/// out. (The manage-pane origin-path header row has its own, separate
/// styling — `theme::style_for(SyntaxRole::Boolean, theme)`, applied
/// directly by `render_manage_pane`, not through this function.)
pub fn manage_entry_style(auto: bool, theme: ThemeKind) -> Style {
    if !auto {
        return Style::default();
    }
    Style::default().fg(pick(
        theme,
        supports_rgb(),
        (DARK_RGB.comment, Color::DarkGray),
        (LIGHT_RGB.comment, Color::DarkGray),
    ))
}

/// Purpose-designed backgrounds for spec 0194's caret cues — not
/// borrowed from `dark_rgb`/`light_rgb`, which are foreground palettes.
///
/// The two shades of each pair must differ from each other and not only
/// from the terminal's own background: the caret sits *on* the caret's
/// row by definition, so the weaker cue is always underneath the
/// stronger one (spec 0194 S4).
mod caret_rgb {
    use ratatui::style::Color;

    /// Dark theme, the caret's row — VSCode dark's own list-hover
    /// background, a barely-there lift off `#1E1E1E`.
    pub const DARK_ROW: Color = Color::Rgb(0x2A, 0x2D, 0x2E);
    /// Dark theme, the caret's own cell while its brace partner is
    /// showing — several steps lighter than `DARK_ROW`, so the caret
    /// still reads as a cell rather than as part of its row.
    pub const DARK_PAIRED: Color = Color::Rgb(0x51, 0x5C, 0x6A);
    /// Light theme, the caret's row.
    pub const LIGHT_ROW: Color = Color::Rgb(0xEC, 0xEC, 0xEC);
    /// Light theme, the caret's own cell while its brace partner is
    /// showing.
    pub const LIGHT_PAIRED: Color = Color::Rgb(0xC8, 0xD3, 0xE0);
}

/// Spec 0194 S2: the caret itself — one character drawn inside out,
/// keeping whatever syntax color it already had.
///
/// Theme-independent, and deliberately a bare modifier rather than a
/// color pair: reversing is what a terminal block cursor does, so it
/// lands correctly on any palette the user has configured, including
/// ones this crate knows nothing about. Spec 0194 S4 hands the same
/// style to the *matching* brace when there is one — it is the strong
/// cue, not the caret's own.
pub fn caret_style() -> Style {
    Style::default().add_modifier(Modifier::REVERSED)
}

/// Spec 0194 S4: the caret's own cell while its matching brace is on
/// screen and carrying `caret_style` instead. A background tint, not a
/// reversal — the strong cue belongs to the member the user is looking
/// *for*, not to the one they just moved to.
pub fn caret_paired_style(theme: ThemeKind) -> Style {
    Style::default().bg(pick(
        theme,
        supports_rgb(),
        (caret_rgb::DARK_PAIRED, Color::Blue),
        (caret_rgb::LIGHT_PAIRED, Color::Cyan),
    ))
}

/// Spec 0194 S2/G7: vim's `cursorline` — a dim background across the
/// caret's whole drawn row, chrome included, so one inverted cell on a
/// full screen is never something the user has to hunt for.
///
/// Visibly weaker than the drag selection's full reversal, which is what
/// keeps spec 0194 G4's two cues apart.
pub fn cursor_row_style(theme: ThemeKind) -> Style {
    Style::default().bg(pick(
        theme,
        supports_rgb(),
        (caret_rgb::DARK_ROW, Color::DarkGray),
        (caret_rgb::LIGHT_ROW, Color::Gray),
    ))
}

/// Focused-pane local-statusline style, shared by every focus-tracked
/// pane (main/override/manage — see `tui/mod.rs`'s `pane_focus_style`).
/// Vim-style inverted video (`REVERSED`): `Color::White` is ANSI code 15
/// ("bright white"), the brightest color available, so the focused
/// pane's statusline reads as a distinctly brighter bar than
/// `unfocused_pane_style`'s dimmer `Gray` (ANSI 7) once both are
/// reversed — deliberately theme-independent (same accent in both
/// `Dark`/`Light`), unlike `style_for`'s own RGB-vs-theme dispatch.
pub fn focus_style(theme: ThemeKind) -> Style {
    match theme {
        ThemeKind::System => system_must_be_resolved(),
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

/// Hue selector for the main-pane heat cue (spec 0138 G9-G12): `Red`
/// for the `Mismatch` cue ("current type scores below best"), `Blue`
/// for the `Tie` cue ("current type ties for best"). Both share
/// `heat_style`'s brightness-level model, differing only in which
/// 12-stop gradient/ANSI-16 pair is used.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HeatHue {
    Red,
    Blue,
}

/// Main-pane heat cue color for the leading glyph column (spec 0138;
/// `hue` selects the `Mismatch` or `Tie` cue, G9-G12). `level` is
/// 1..=12 (see `tui::heat_cue::heat_level`), already gated present by
/// the caller (G4 for `Red`/`Mismatch`, G9 for `Blue`/`Tie`). Returns
/// `None` when the cue must not be shown at all on this terminal — only
/// possible on the ANSI-16 fallback, for `level <= 3` (`best_score <=
/// 3`, G7/G12's low-confidence narrowing of the gate); the truecolor
/// gradient always shows *some* color once the gate has passed, however
/// dim.
pub fn heat_style(level: u8, hue: HeatHue, theme: ThemeKind) -> Option<Style> {
    heat_style_in(level, hue, theme, supports_rgb())
}

/// `heat_style` with the color depth passed in rather than probed — see
/// `style_for_in` for why the flag is an argument.
fn heat_style_in(level: u8, hue: HeatHue, theme: ThemeKind, rgb: bool) -> Option<Style> {
    match (theme, rgb) {
        (ThemeKind::Dark, true) => Some(Style::default().fg(heat_rgb_color(level, false, hue))),
        (ThemeKind::Light, true) => Some(Style::default().fg(heat_rgb_color(level, true, hue))),
        (ThemeKind::Dark | ThemeKind::Light, false) if level <= 3 => None,
        (ThemeKind::Dark | ThemeKind::Light, false) if level <= 7 => {
            Some(Style::default().fg(match hue {
                HeatHue::Red => Color::Red,
                HeatHue::Blue => Color::Blue,
            }))
        }
        (ThemeKind::Dark | ThemeKind::Light, false) => Some(Style::default().fg(match hue {
            HeatHue::Red => Color::LightRed,
            HeatHue::Blue => Color::LightBlue,
        })),
        (ThemeKind::System, _) => system_must_be_resolved(),
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
/// leading glyph by `level`. The `Tie` cue's ` [tie_count@score]` suffix
/// has no dedicated function: it is styled with
/// `style_for(SyntaxRole::Boolean, theme)` directly, the same styling as
/// a `true`/`false` value (spec 0138 G9), not a new color.
pub fn heat_suffix_style(theme: ThemeKind) -> Style {
    heat_suffix_style_in(theme, supports_rgb())
}

/// `heat_suffix_style` with the color depth passed in rather than
/// probed — see `style_for_in` for why the flag is an argument.
fn heat_suffix_style_in(theme: ThemeKind, rgb: bool) -> Style {
    Style::default().fg(pick(
        theme,
        rgb,
        (heat_rgb::DARK[11], Color::LightRed),
        (heat_rgb::LIGHT[11], Color::LightRed),
    ))
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

    /// `ThemeKind::System` is a *request*, not a palette, and nothing in
    /// the type system says so: seven styling functions below panic
    /// outright if they are ever handed it, and the only thing standing
    /// between them and a `--theme system` command line is one
    /// hand-written match arm in `main.rs`. So the resolver's own output
    /// is pushed through all seven here.
    ///
    /// The panic that would otherwise be waiting is the worst kind this
    /// program has — mid-frame, with the terminal in raw mode — and it
    /// needs no code change to arrive: a new detection path that gives
    /// up by returning its input, or a `_ =>` passthrough added to
    /// `main.rs`'s match, is enough.
    ///
    /// `COLORFGBG` is set so the resolution stays in memory. Left unset,
    /// `resolve_system` falls through to an OSC 11 query, which under
    /// `cargo test` has no terminal to answer it and pays the timeout.
    #[test]
    fn every_styling_function_accepts_what_resolve_system_returns() {
        let _guard = lock_env();
        for fg_bg in ["15;0", "0;15"] {
            // SAFETY: single-threaded (guarded by ENV_MUTEX above).
            unsafe {
                std::env::set_var("COLORFGBG", fg_bg);
            }
            let theme = resolve_system();
            assert_ne!(
                theme,
                ThemeKind::System,
                "resolution returned the request itself for COLORFGBG={fg_bg}",
            );
            // These four probe the terminal for their color depth, which
            // is left alone: what is under test is that the *theme* is
            // renderable, not which palette comes back.
            manage_entry_style(true, theme);
            caret_paired_style(theme);
            cursor_row_style(theme);
            focus_style(theme);
            for rgb in [false, true] {
                for role in ALL_ROLES {
                    style_for_in(role, theme, rgb);
                }
                for level in 0..=12 {
                    for hue in [HeatHue::Red, HeatHue::Blue] {
                        heat_style_in(level, hue, theme, rgb);
                    }
                }
                heat_suffix_style_in(theme, rgb);
            }
        }
        unsafe {
            std::env::remove_var("COLORFGBG");
        }
    }

    const ALL_ROLES: [SyntaxRole; 16] = [
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
        SyntaxRole::AnnotationLandmark,
        SyntaxRole::AnnotationNonCanonical,
        SyntaxRole::AnnotationInvalid,
    ];

    // `PunctuationDelimiter`/`PunctuationBracket` are deliberately
    // unstyled (terminal default) in both the RGB and ANSI-16 palettes
    // — excluded from the "must be Rgb" assertion below.
    const COLORED_ROLES: [SyntaxRole; 14] = [
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
        SyntaxRole::AnnotationLandmark,
        SyntaxRole::AnnotationNonCanonical,
        SyntaxRole::AnnotationInvalid,
    ];

    /// Spec 0225 §S11: the annotation reaches a tier's color through a
    /// capture and a `SyntaxRole`, the wire row asks for it by name.
    /// They must be the same color, in all four palettes, or the two
    /// rows of one document contradict each other about severity.
    #[test]
    fn a_tier_looks_the_same_named_as_it_does_captured() {
        let pairs = [
            (Tier::Landmark, SyntaxRole::AnnotationLandmark),
            (Tier::NonCanonical, SyntaxRole::AnnotationNonCanonical),
            (Tier::Invalid, SyntaxRole::AnnotationInvalid),
        ];
        for theme in [ThemeKind::Dark, ThemeKind::Light] {
            for rgb in [false, true] {
                for (tier, role) in pairs {
                    assert_eq!(
                        tier_style_in(tier, theme, rgb),
                        style_for_in(role, theme, rgb),
                        "{tier:?} disagrees with {role:?} ({theme:?}, rgb={rgb})",
                    );
                }
            }
        }
    }

    /// The three tiers must be told apart at a glance, so no two of them
    /// may share a color. Not asserted against the *roles*: ANSI-16 has
    /// sixteen colors and `PunctuationBracketList` already holds yellow,
    /// so a collision there is forced, and harmless — a tier and a
    /// bracket are never candidates for the same span.
    #[test]
    fn no_two_tiers_share_a_color() {
        for theme in [ThemeKind::Dark, ThemeKind::Light] {
            for rgb in [false, true] {
                let tiers = [Tier::Landmark, Tier::NonCanonical, Tier::Invalid];
                let colors: Vec<_> = tiers
                    .iter()
                    .map(|&t| tier_style_in(t, theme, rgb).fg.expect("a tier has a color"))
                    .collect();
                for (i, c) in colors.iter().enumerate() {
                    assert!(
                        !colors[i + 1..].contains(c),
                        "two tiers share {c:?} ({theme:?}, rgb={rgb})",
                    );
                }
            }
        }
    }

    /// `COLORTERM` is the first and cheapest of the three true-color
    /// signals, and the only one a test can move: the other two are
    /// process-wide `OnceLock` caches over the ambient terminal. So this
    /// covers the plumbing from the environment into `supports_rgb`, and
    /// the palette tests below cover the palettes — separately, through
    /// the `*_in` functions, which is what lets them cover *both*
    /// palettes on a machine whose terminal is true-color-capable.
    #[test]
    fn supports_rgb_follows_colorterm() {
        let _guard = lock_env();
        // SAFETY: single-threaded (guarded by ENV_MUTEX above).
        unsafe {
            std::env::set_var("COLORTERM", "truecolor");
        }
        assert!(supports_rgb());
        unsafe {
            std::env::set_var("COLORTERM", "24bit");
        }
        assert!(supports_rgb());
        unsafe {
            std::env::set_var("COLORTERM", "256color");
        }
        assert!(!colorterm_reports_truecolor());
        unsafe {
            std::env::remove_var("COLORTERM");
        }
    }

    #[test]
    fn the_ansi16_palette_uses_only_named_colors() {
        for role in ALL_ROLES {
            for theme in [ThemeKind::Dark, ThemeKind::Light] {
                let style = style_for_in(role, theme, false);
                assert!(
                    !matches!(style.fg, Some(Color::Rgb(..)) | Some(Color::Indexed(_))),
                    "{role:?} must be a named ANSI-16 color, got {:?}",
                    style.fg
                );
            }
        }
    }

    #[test]
    fn the_rgb_palette_colors_every_role_that_is_meant_to_be_colored() {
        for role in COLORED_ROLES {
            for theme in [ThemeKind::Dark, ThemeKind::Light] {
                let style = style_for_in(role, theme, true);
                assert!(matches!(style.fg, Some(Color::Rgb(..))), "{role:?}");
            }
        }
    }

    #[test]
    fn heat_style_grades_the_rgb_gradient_by_level() {
        for theme in [ThemeKind::Dark, ThemeKind::Light] {
            for hue in [HeatHue::Red, HeatHue::Blue] {
                let level1 = heat_style_in(1, hue, theme, true).unwrap();
                let level12 = heat_style_in(12, hue, theme, true).unwrap();
                assert!(matches!(level1.fg, Some(Color::Rgb(..))));
                assert!(matches!(level12.fg, Some(Color::Rgb(..))));
                assert_ne!(level1.fg, level12.fg, "brightness must vary by level");
                // Out-of-range levels clamp rather than panic.
                assert_eq!(
                    heat_style_in(0, hue, theme, true),
                    heat_style_in(1, hue, theme, true)
                );
                assert_eq!(
                    heat_style_in(200, hue, theme, true),
                    heat_style_in(12, hue, theme, true)
                );
            }
            // Same level, different hue: distinct colors (G11).
            assert_ne!(
                heat_style_in(6, HeatHue::Red, theme, true),
                heat_style_in(6, HeatHue::Blue, theme, true)
            );
        }
    }

    #[test]
    fn heat_style_ansi16_fallback_thresholds() {
        for theme in [ThemeKind::Dark, ThemeKind::Light] {
            // Level 3 == `best_score <= 3` (G7/G12's low-confidence absence).
            assert_eq!(heat_style_in(3, HeatHue::Red, theme, false), None);
            assert_eq!(heat_style_in(3, HeatHue::Blue, theme, false), None);
            // Level 7 == `best_score <= 21`: dark red / dark blue.
            assert_eq!(
                heat_style_in(7, HeatHue::Red, theme, false),
                Some(Style::default().fg(Color::Red))
            );
            assert_eq!(
                heat_style_in(7, HeatHue::Blue, theme, false),
                Some(Style::default().fg(Color::Blue))
            );
            // Level 8 == `best_score > 21`: bright red / bright blue.
            assert_eq!(
                heat_style_in(8, HeatHue::Red, theme, false),
                Some(Style::default().fg(Color::LightRed))
            );
            assert_eq!(
                heat_style_in(8, HeatHue::Blue, theme, false),
                Some(Style::default().fg(Color::LightBlue))
            );
        }
    }

    #[test]
    fn heat_suffix_style_is_always_the_brightest_red() {
        for theme in [ThemeKind::Dark, ThemeKind::Light] {
            // Truecolor: the top stop of the gradient, whatever the level.
            assert_eq!(
                heat_suffix_style_in(theme, true).fg,
                heat_style_in(12, HeatHue::Red, theme, true).unwrap().fg
            );
            // ANSI-16: the brightest red the palette has.
            assert_eq!(
                heat_suffix_style_in(theme, false),
                Style::default().fg(Color::LightRed)
            );
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
