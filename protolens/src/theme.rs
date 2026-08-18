// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Theme: maps a `SyntaxRole` to a `ratatui::style::Style` (spec 0116
//! §7, §9). Two built-in palette pairs (dark, light), each in two color
//! depths: RGB, borrowed from VSCode Dark+/Light+, and ANSI-16, a
//! portable fallback — picked by `supports_rgb`. The `System` selector
//! is resolved once at startup.

use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};

use crate::annotation::Tier;
use crate::colorize::{SyntaxRole, ALL_ROLES};
use crate::node_status::Status;

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
    /// Also the annotation's echo of the field *number*, which is the
    /// same fact as the name at the head of the line, and the tag bytes
    /// of the wire row below it (spec 0225 §S12).
    attribute: Color,
    /// Deliberately not VSCode's type color: a type name and its wire
    /// type sit next to a field name and a value on every annotated
    /// line, and the borrowed blue-green read as a cousin of the field
    /// name's blue. The two constants below say what replaced it and
    /// what each is chosen for.
    r#type: Color,
    /// VSCode's link color, now serving only `StringSpecialUrl` — the
    /// domain half of an Any key, which is a URL and not a type. Kept as
    /// its own field rather than folded into `r#type`, which is where it
    /// used to live: the two stopped being the same color when the type
    /// color turned orange, and a URL is not warm.
    link: Color,
    /// Every scalar value a document row can hold — a string, a number,
    /// a bool, an enum value name — wears this one color. What the value
    /// *is* is already said by the type in its annotation and by the
    /// bytes on the wire row under it; a second, quieter spelling of the
    /// same fact in the value's own color only competes with the field
    /// name it sits beside.
    value: Color,
    /// A string's `\n`, `\377` and friends, deliberately still its own
    /// color: an escape is not a value, it is a span *inside* one, and
    /// telling the two apart is what the color is for.
    string_escape: Color,
    /// Also this crate's manage-pane "auto" entry color
    /// (`manage_entry_style`).
    comment: Color,
    /// Not a document color at all: the two pieces of pane chrome that
    /// have to sit apart from every syntax role around them — the
    /// manage-pane origin-path header and the heat cue's tie suffix.
    /// See [`accent_style`].
    accent: Color,
    punctuation_bracket_list: Color,
    punctuation_bracket_extension: Color,
    /// The three severity tiers (spec 0225 §S11). Deliberately not
    /// reusing a color above: a tier must be legible *as* a tier next to
    /// ordinary syntax on the same line, and next to the hex of a wire
    /// row, so sharing a hue with a role would make the two readings
    /// ambiguous exactly where both appear.
    ///
    /// The loudest colors in the palette, and the only ones a wire row
    /// wears at full strength — `leveled` never touches them, so on a
    /// row of blocks brought down to `WIRE_LUMA_*` an anomaly is the one
    /// bright thing. On `Dark` that means fully saturated; on `Light` it
    /// means the opposite, since prominence against white is depth
    /// rather than brightness.
    tier_non_canonical: Color,
    tier_invalid: Color,
    /// The color of a fold whose subtree has not been baked yet
    /// (spec 0249 S12, value measured in spec 0260). Not a tier: it
    /// says "provisional", not "wrong", and it appears only in the fold
    /// margin, never in an annotation.
    status_unbaked: Color,
    /// The one luminance every color above *except the two tiers* is
    /// brought to, by [`doc_leveled`], before it is ever drawn.
    ///
    /// The palette is borrowed from VSCode, where brightness is part of
    /// how each scope is distinguished. On a document that is almost
    /// entirely colored tokens that reads as noise: ten hues at ten
    /// brightnesses give the eye a second axis to track that carries no
    /// information, because the hue already said which scope it is. One
    /// brightness leaves hue as the only variable, and leaves *loudness*
    /// free to mean one thing.
    ///
    /// That one thing is the two *anomaly* tier colors, which are
    /// deliberately not leveled: after this, an anomaly is the only text
    /// on a document row that is brighter (dark) or deeper (light) than
    /// everything around it, without the reader having to know the
    /// palette.
    ///
    /// Heat cues are outside this too — they are their own gradient in
    /// their own reserved column, and the whole point of a gradient is
    /// that its brightness varies.
    ///
    /// The values are legibility choices. Dark's 170 sits under the
    /// palette's brightest three (attribute 209, punctuation-list 216,
    /// type 229) and over its dimmest two (bracket-extension 127,
    /// comment 138), so the effect is mostly a dimming; light's 85 is
    /// the mirror, above its own mean of 68 and under `number`'s 104.
    doc_luma: f32,
}

/// The dark RGB palette, borrowed from VSCode's `dark_plus.json`/
/// `dark_vs.json`. Each trailing comment is that color's closest
/// named-color match from <https://www.color-name.com>, purely for human
/// readability when scanning this file.
const DARK_RGB: RgbPalette = RgbPalette {
    attribute: Color::Rgb(0x9C, 0xDC, 0xFE), // Clear Blue
    // A pale yellow against the field name's pale blue, the pairing of
    // the Ukrainian flag. It replaced a vivid orange, which the wire
    // row is what exposed: leveled down to a band, an orange is a
    // muted red, and a muted red beside the bright red an `Invalid`
    // tier paints is the one confusion the whole tier scheme exists to
    // prevent — a well-formed wire type looked accused.
    //
    // Only its hue and its saturation survive `doc_luma`, so those are
    // what this constant is chosen for: hue 55°, seven degrees off pure
    // yellow, against `tier_non_canonical`'s 36° — and desaturated to
    // 0.42, against that tier's 0.75. A first attempt at 48°/0.55 was
    // reported as too close to the anomaly color, and both axes were
    // moved rather than one, since after leveling saturation is the
    // more legible of the two.
    r#type: Color::Rgb(0xF2, 0xEA, 0x8C),        // Sweet Corn
    link: Color::Rgb(0x4E, 0xC9, 0xB0),          // Subtle Blue Green
    value: Color::Rgb(0xCE, 0x91, 0x78),         // Beauty Copper
    string_escape: Color::Rgb(0xD7, 0xBA, 0x7D), // Mushroom Melt
    comment: Color::Rgb(0x6A, 0x99, 0x55),       // Brussels Sprout
    accent: Color::Rgb(0x56, 0x9C, 0xD6),        // Azul Mystic
    punctuation_bracket_list: Color::Rgb(0xDC, 0xDC, 0xAA), // Pale Hazel
    punctuation_bracket_extension: Color::Rgb(0xD1, 0x69, 0x69), // Alexa
    // VSCode's invalid, scaled up until one channel saturates: same
    // hue, same relative mix, as much light as the hue can carry.
    //
    // Pulled from VSCode's gold (hue 49°) down to amber at 36°, away
    // from `r#type`'s 55°: the two were reported as reading alike, and
    // the tier is the one that has to be unmistakable. It goes toward
    // red rather than green because that is the direction that means
    // *worse* in this palette — `tier_invalid` is at 0° — so the three
    // tiers now run gold-free from 36° to 0° in severity order.
    //
    // 36° is as red as a full-value hue can be while staying brighter
    // than `doc_luma`: at 0.75 saturation it lands on luma 187 against
    // the document's 170. Any redder and an anomaly would be *dimmer*
    // than the ordinary text it has to stand out from.
    tier_non_canonical: Color::Rgb(0xFF, 0xB4, 0x40), // Yellow Orange
    tier_invalid: Color::Rgb(0xFF, 0x55, 0x55),       // Sunset Orange
    // Spec 0260 S1. The scaling above gave this one `#FFAEF8` — full
    // value at saturation 0.318, against 0.667 to 0.749 for every other
    // color the fold margin can wear. It arrived as a tint rather than
    // as a hue and was reported as too close to the default foreground.
    // Saturation is the axis that separates a color from white; there
    // was no room on the other one, the color already being at value
    // 1.0.
    //
    // Hue 285°, saturation 0.70, value 1.0: its neighbors' saturation,
    // at a hue twenty degrees off the pink `#FFAEF8` reads as. Luma 118
    // beside `tier_invalid`'s 121, so it is no dimmer in the margin
    // than the color the palette already trusts there.
    status_unbaked: Color::Rgb(0xD2, 0x4D, 0xFF), // Electric Purple
    doc_luma: 170.0,
};

/// The light RGB palette, borrowed from VSCode's `light_plus.json`/
/// `light_vs.json`. See `DARK_RGB` for the trailing-comment convention.
const LIGHT_RGB: RgbPalette = RgbPalette {
    attribute: Color::Rgb(0xE5, 0x00, 0x00),     // Electric Red
    r#type: Color::Rgb(0xAF, 0x3A, 0x03),        // Burnt Orange
    link: Color::Rgb(0x26, 0x7F, 0x99),          // Jelly Bean Blue
    value: Color::Rgb(0xA3, 0x15, 0x15),         // San Diego
    string_escape: Color::Rgb(0xEE, 0x00, 0x00), // Strong Red
    comment: Color::Rgb(0x00, 0x80, 0x00),       // Digital Green
    accent: Color::Rgb(0x00, 0x00, 0xFF),        // Blue
    punctuation_bracket_list: Color::Rgb(0x04, 0x51, 0xA5), // French Blue
    punctuation_bracket_extension: Color::Rgb(0x81, 0x1F, 0x3F), // Dried Burgundy
    // Deepened from VSCode's `#BF8803`, which was the one weak anomaly
    // mark on white: luma 138 against the violet's 53 and the red's 63,
    // so it read as the quietest of the three while meaning more than
    // an unbaked fold does. The other two are already deep and stand.
    tier_non_canonical: Color::Rgb(0x9C, 0x6A, 0x00), // Golden Brown
    tier_invalid: Color::Rgb(0xE5, 0x14, 0x00),       // Scarlet
    status_unbaked: Color::Rgb(0xAF, 0x00, 0xDB),     // Violet
    doc_luma: 85.0,
};

/// Which color of `p` each role takes, and the one modifier the RGB
/// palettes carry (spec 0116 §9's "RGB palette" table; scope names cited
/// there).
///
/// One function for both palettes: the dark and light tables never
/// differed in which role got which named color, only in what the names
/// resolve to.
fn style_for_rgb(role: SyntaxRole, p: &RgbPalette) -> Style {
    // Every role but the two anomaly tiers goes through `doc_leveled`.
    // See `RgbPalette::doc_luma`.
    let hue = |color| Style::default().fg(doc_leveled(color, p.doc_luma));
    match role {
        SyntaxRole::Attribute => hue(p.attribute),
        SyntaxRole::Type => hue(p.r#type),
        // The four value roles — a string, a number, a bool, an enum
        // value name — take one color. See `RgbPalette::value`.
        SyntaxRole::StringLiteral => hue(p.value),
        SyntaxRole::Number => hue(p.value),
        SyntaxRole::Boolean => hue(p.value),
        SyntaxRole::Constant => hue(p.value),
        SyntaxRole::StringEscape => hue(p.string_escape),
        SyntaxRole::StringSpecialUrl => hue(p.link).add_modifier(Modifier::UNDERLINED),
        SyntaxRole::Comment => hue(p.comment),
        SyntaxRole::PunctuationDelimiter => Style::default(),
        SyntaxRole::PunctuationBracket => Style::default(),
        SyntaxRole::PunctuationBracketList => hue(p.punctuation_bracket_list),
        SyntaxRole::PunctuationBracketExtension => hue(p.punctuation_bracket_extension),
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
/// `tier_color`. The annotation reaches a tier through a capture and a
/// role; the wire row names the tier outright; both must land on the
/// same color, and a second copy is how they would stop.
fn tier_color_rgb(tier: Tier, p: &RgbPalette) -> Color {
    match tier {
        Tier::NonCanonical => p.tier_non_canonical,
        Tier::Invalid => p.tier_invalid,
    }
}

fn tier_color_dark_ansi16(tier: Tier) -> Color {
    match tier {
        Tier::NonCanonical => Color::Yellow,
        Tier::Invalid => Color::LightRed,
    }
}

fn tier_color_light_ansi16(tier: Tier) -> Color {
    match tier {
        Tier::NonCanonical => Color::Yellow,
        Tier::Invalid => Color::Red,
    }
}

/// The style a severity tier wears on a wire row: the tier's color as
/// a *band* (spec 0225 §S11 "one classifier, two rows").
///
/// Never leveled — these are the loudest colors in the palette and the
/// wire row wears them at full strength.
///
/// A wire row spends both channels and neither is free for a tier to
/// borrow: the foreground is the hex itself, one muted color the whole
/// row long so that the digits read as digits, and the background says
/// what each byte is *for*. An anomaly is the one thing that outranks
/// what a byte is for, so it takes the background outright — the tier
/// color unleveled, against the leveled bands of everything around it.
///
/// The alternative, tried first, was the tier in the foreground over
/// the region's own band. It reads as a slightly different shade of
/// text on a screen already full of colored text; a band swap at full
/// strength does not.
///
/// The hex on top of it is [`band_text`]: the contrast comes out of the
/// same choice that made these colors loud.
pub fn tier_band(tier: Tier, theme: ThemeKind) -> Style {
    tier_band_in(tier, theme, supports_rgb())
}

fn tier_band_in(tier: Tier, theme: ThemeKind, rgb: bool) -> Style {
    Style::default()
        .bg(tier_color(tier, theme, rgb))
        .fg(band_text(theme, rgb))
}

/// The band a wire row draws on the bytes of a field no schema declares
/// (spec 0279's 2026-08-12 amendment).
///
/// Not a tier, and deliberately not shaped like one: nothing is wrong
/// with these bytes, and `annotation::tier_of` emits no keyword for
/// them. It borrows the fold margin's own [`Status::Unknown`] blue, so
/// that the toggle summarizing a subtree and the bytes inside it are one
/// fact read twice — the same argument `tier_band` makes for the tiers.
pub fn unknown_band(theme: ThemeKind) -> Style {
    unknown_band_in(theme, supports_rgb())
}

fn unknown_band_in(theme: ThemeKind, rgb: bool) -> Style {
    Style::default()
        .bg(unknown_color(theme, rgb))
        .fg(band_text(theme, rgb))
}

/// VSCode's `constant.other` blue on each side: light against black,
/// deep against white, mirroring how the tiers are chosen. Unleveled
/// like them — in the fold margin prominence is the whole job, and on a
/// wire row a band has to outrank the hue the region borrowed.
fn unknown_color(theme: ThemeKind, rgb: bool) -> Color {
    pick(
        theme,
        rgb,
        (Color::Rgb(0x4F, 0xC1, 0xFF), Color::LightBlue),
        (Color::Rgb(0x00, 0x70, 0xC1), Color::Blue),
    )
}

/// The hex drawn on top of a band: the page's own background, which is
/// the far end of the palette from where these colors were chosen to
/// sit. In sixteen colors it is black on both themes, those being all
/// mid-brightness whatever the terminal renders them as.
fn band_text(theme: ThemeKind, rgb: bool) -> Color {
    pick(
        theme,
        rgb,
        (Color::Black, Color::Black),
        (Color::White, Color::Black),
    )
}

/// Spec 0247 S10: the color a fold toggle wears for the worst thing in
/// its subtree, or `None` for [`Status::Ok`] — which keeps the margin
/// unstyled rather than painting it a "fine" color, so that a colored
/// toggle is the only thing on the row that changed.
///
/// `Status::Invalid` borrows its tier color outright rather than
/// approximating it: the toggle and the annotation it is summarizing are
/// two readings of one fact (the same argument `tier_band` makes for the
/// wire row), and a second copy is how they would drift.
/// `Status::Unknown` and `Status::Unbaked` have no tier to borrow from
/// — neither is an anomaly, one being a schema with nothing to say and
/// the other a subtree nobody has looked at — so each gets a color of
/// its own.
///
/// `Status::NonCanonical` is the exception, and it is one on purpose
/// (2026-08-10). Its tier is an amber at 36°, chosen against the *other
/// document colors* (see `DARK_RGB::tier_non_canonical`); in the margin
/// the only colors it is ever seen against are the four here, and 36°
/// against `Invalid`'s 0° was reported as too close to read apart. The
/// margin is one glyph wide, which is the hardest place in the app to
/// judge a hue, so it gets its own scale — white, violet, blue, yellow,
/// red — held apart by `no_two_fold_margin_colors_share_a_neighborhood`.
/// Sharing the amber would keep one invariant at the cost of the one the
/// column exists for.
pub fn status_color(status: Status, theme: ThemeKind) -> Option<Color> {
    status_color_in(status, theme, supports_rgb())
}

fn status_color_in(status: Status, theme: ThemeKind, rgb: bool) -> Option<Color> {
    Some(match status {
        Status::Ok => return None,
        // Spec 0249 S12's violet, measured in spec 0260. Unleveled
        // like the tiers: the margin is its own column, and a fold
        // nobody has read has to be visible in it.
        Status::Unbaked => pick(
            theme,
            rgb,
            (DARK_RGB.status_unbaked, Color::LightMagenta),
            (LIGHT_RGB.status_unbaked, Color::Magenta),
        ),
        Status::Unknown => unknown_color(theme, rgb),
        // The tier's amber moved to yellow, for the margin only. Hue
        // 54°, six degrees short of pure: far enough from `Invalid` at
        // 0° that the two cannot be confused at one glyph, and short of
        // 60° so it is still a warm color rather than a green-yellow.
        //
        // Dark was #FFF14D — the saturation its neighbors carry (0.70)
        // — and was reported as too close to white. Saturation is the
        // wrong axis *for yellow* and only for yellow: it is the one hue
        // whose fully saturated form is already near white in luminance,
        // so #FFF14D sat 23 points below pure white while violet, red
        // and blue sat 82 to 137 below at the same saturation.
        //
        // Corrected 2026-08-10 by taking blue to zero and pulling red
        // and green down with it. #E8D200 (luma 200) was the first
        // attempt and was still read as white, so this is the second:
        // luma 166, which puts it *below* `Status::Unknown`'s blue at
        // 173 — inside the range the other three occupy rather than
        // above all of them. What is spent doing that is hue margin:
        // 53.8° against the 50° floor the pairwise test enforces, which
        // is about as dark as a yellow can go before it has to start
        // turning green to stay clear of the red.
        //
        // The luma ceiling in `every_status_color_is_a_hue_and_not_a_tint`
        // is set so neither rejected value can pass again.
        //
        // Light already answered the same question from the other side,
        // and is unchanged: it matches the *lightness* the light tier
        // was deepened to, since on white a full-value yellow is the one
        // mark that disappears. #827800 is `tier_non_canonical`'s luma
        // to within a point, at 55° instead of 41°.
        Status::NonCanonical => pick(
            theme,
            rgb,
            (Color::Rgb(0xC2, 0xAE, 0x00), Color::Yellow),
            (Color::Rgb(0x82, 0x78, 0x00), Color::Yellow),
        ),
        Status::Invalid => tier_color(Tier::Invalid, theme, rgb),
    })
}

/// Spec 0318 S5/S7: the color of the bar an override preview draws in
/// the fold column, saying how much of the node the preview is.
///
/// Green, yellow, orange — a fidelity ramp, and the reader's own
/// vocabulary for one. It is not the anomaly ramp and must not be read
/// as one; what keeps the two apart is the column. Nothing but this bar
/// is ever drawn in an overlay row's fold column, a preview's bars are a
/// contiguous run where a status color is a lone triangle, and the glyph
/// differs (`│` against `⏵`/`⏷`). The overlap that remains is yellow,
/// which `Status::NonCanonical` also wears; the bar is deliberately a
/// warmer, more saturated one so that a committed row's triangle and a
/// preview's bar do not read as the same mark.
///
/// Unleveled, like the tiers and `status_unbaked`, and for the same
/// reason: the margin is its own column, and something the reader is
/// meant to notice without looking for it cannot be brought down to the
/// document's luminance.
pub fn preview_tier_color(tier: PreviewTierHue, theme: ThemeKind) -> Color {
    let rgb = supports_rgb();
    match tier {
        // Hue 120°. The one color in the app that means "nothing to
        // report" out loud rather than by absence, which is what a
        // preview showing the whole node needs to say — silence here
        // would be indistinguishable from no preview at all.
        PreviewTierHue::Whole => pick(
            theme,
            rgb,
            (Color::Rgb(0x3F, 0xC3, 0x3F), Color::Green),
            (Color::Rgb(0x1B, 0x7F, 0x1B), Color::Green),
        ),
        // Hue 45°, warmer and more saturated than `NonCanonical`'s 54°.
        PreviewTierHue::Clean => pick(
            theme,
            rgb,
            (Color::Rgb(0xFF, 0xBF, 0x00), Color::Yellow),
            (Color::Rgb(0xA8, 0x7E, 0x00), Color::Yellow),
        ),
        // Hue 25°. Orange, because this is the tier where the rendering
        // below it may carry a truncation the data does not have.
        PreviewTierHue::Ragged => pick(
            theme,
            rgb,
            (Color::Rgb(0xFF, 0x7A, 0x1A), Color::LightRed),
            (Color::Rgb(0xB5, 0x45, 0x00), Color::Red),
        ),
    }
}

/// The three preview tiers, as this module sees them. A hue role, like
/// [`Tier`] and `HeatHue`: `tui::preview_truncate::PreviewTier` is the
/// decision, this is the color it asks for, and keeping them separate is
/// what stops the theme depending on the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewTierHue {
    Whole,
    Clean,
    Ragged,
}

/// The color of `tier`, in whichever of the four palettes applies.
fn tier_color(tier: Tier, theme: ThemeKind, rgb: bool) -> Color {
    pick(
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
    )
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
    /// The tag's wire-type bits — borrows the color the annotation
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
/// Between those, the closer to the bound the more the wire row asserts
/// itself, and it should not: it is a subordinate reading of the row
/// above. The dimmest borrowed dark color is `comment` at luma 138; the
/// brightest borrowed light one is `number` at 104.
///
/// Both started one step in from their bound — 130 and 150 — and ended
/// far past it, because a *filled band* is not a dimmer version of
/// colored text: it emits light across the whole cell where glyphs emit
/// it across a fraction of one, so a wire row at the document row's own
/// brightness still reads as the louder of the two.
///
/// They can go this far only because [`banded`] leaves the hex alone.
/// While the row was reversed the band carried the glyphs' contrast
/// too, which pinned it at 80; with the hex drawn in its own fixed
/// color the band is free to be nothing but a tint.
///
/// It first settled at 58 (and 197, the same distance from white),
/// which proved a step too quiet: at that depth the four hues are
/// present but hard to tell apart, and naming a byte's purpose from its
/// band is the whole reason the band is colored at all. 78 keeps every
/// property the pair was chosen for — well under `comment`'s 138, well
/// under [`WIRE_TEXT_DARK`]'s 176 so the hex stays legible on top — and
/// buys back the saturation that made the hues nameable. The light
/// target moves by the same 20 to keep the two themes' rows equally
/// far from their page.
const WIRE_LUMA_DARK: f32 = 78.0;
const WIRE_LUMA_LIGHT: f32 = 177.0;

/// The one color a wire row's hex is drawn in, whatever band it sits
/// on.
///
/// Uniform along the row on purpose: with the background carrying both
/// a byte's purpose and its defects, a foreground that also varied
/// would be a third reading of the same forty glyphs. Muted rather than
/// the terminal's own text color, so that a wire row stays visibly the
/// subordinate of the document row above it even where its band is
/// quietest.
///
/// The pair are equally far from their page — 176 above black, 176
/// below white — so neither theme's rows are the louder.
const WIRE_TEXT_DARK: Color = Color::Rgb(0xB0, 0xB0, 0xB0);
const WIRE_TEXT_LIGHT: Color = Color::Rgb(0x4F, 0x4F, 0x4F);

/// What marks something protolens invented rather than read from the
/// file (spec 0307 S2): `Blob`'s wrapper tag and length in the wire
/// row, and the `1` those bytes put on the document row.
///
/// Not a color, on either row. The wire row's two color channels are
/// both spoken for — a background hue for what a byte is *for*, taken
/// over by a tier's band when it is malformed — and the third,
/// [`WIRE_TEXT_DARK`], is the one thing keeping dense hex legible over
/// whichever band it lands on; a green tried there was hard to read
/// against the blue an undeclared field wears. The document row's
/// colors are the grammar's and are not protolens' to spend.
///
/// Italic is the right shape for the fact besides. Provenance is not a
/// property of the byte, it is a note that the byte is not the reader's
/// — the typographic convention for an interjection in someone else's
/// text.
pub const SYNTHETIC: Modifier = Modifier::ITALIC;

/// The style a wire byte wears (spec 0225 S11): `borrowed`'s color,
/// brought to the wire row's brightness and worn as a *background*.
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
        Some(role) => wire_styles(theme)[role.index()],
        None => Style::default().add_modifier(Modifier::DIM),
    }
}

/// Every wire-row style there is, resolved on first use.
///
/// The inputs are a finite set — one `SyntaxRole` per capture, times the
/// two resolved themes — so a wire byte's style is a lookup rather than
/// a computation. It was neither: `wire_style` ran `supports_rgb` (whose
/// first branch is an *uncached* environment read, allocating a `String`)
/// and then `leveled`'s blend, four times for every drawn row of every
/// frame.
///
/// Resolved lazily rather than written out as constants because
/// `leveled` is float arithmetic that no `const fn` can run today, and
/// hand-computing the blended triples into literals would put a second
/// copy of the answer where retuning `WIRE_LUMA_DARK` could not reach
/// it.
fn wire_styles(theme: ThemeKind) -> &'static [Style; ALL_ROLES.len()] {
    static TABLE: OnceLock<[[Style; ALL_ROLES.len()]; 2]> = OnceLock::new();
    let build = |theme| {
        let mut styles = [Style::default(); ALL_ROLES.len()];
        for (slot, role) in styles.iter_mut().zip(ALL_ROLES) {
            *slot = banded(style_for(role, theme), theme);
        }
        styles
    };
    let table = TABLE.get_or_init(|| [build(ThemeKind::Dark), build(ThemeKind::Light)]);
    match theme {
        ThemeKind::Dark => &table[0],
        ThemeKind::Light => &table[1],
        ThemeKind::System => system_must_be_resolved(),
    }
}

/// The hue leveled and moved to the background, under
/// [`WIRE_TEXT_DARK`]/[`WIRE_TEXT_LIGHT`] hex (spec 0225 S11).
///
/// Both channels are set explicitly, and `REVERSED` is not used. Under
/// reverse video the terminal draws the hex in its *background* color,
/// which ties the text's legibility to how dark the band is — the band
/// could not be lowered past 80 without taking the digits down with it
/// — and it leaves the row nothing to change when a byte is anomalous
/// except the band it is already using to say what the byte is for.
/// Naming the two colors separately unties both: the band goes as quiet
/// as `WIRE_LUMA_DARK` likes, and `tier_band` can take it over outright.
///
/// Which branch applies is read off the color itself rather than from a
/// second `supports_rgb()` call: `style_for` has already resolved which
/// palette is in play, so the color's shape is the exact discriminator
/// and cannot disagree with it. In 16 colors there is no leveling to be
/// had, and the ANSI color is worn as it is — and its hex keeps the
/// terminal's own foreground, since an RGB text color under an ANSI
/// band is exactly the mixture the fallback exists to avoid.
///
/// Hue only, like `tier_style` — an inherited `UNDERLINED` or `ITALIC`
/// would be a locator on a row that has its own. A style carrying no
/// color has no band either: a band standing for "nothing to say" would
/// be the loudest thing on the screen, so those stay `DIM` text on the
/// ordinary background.
pub fn banded(style: Style, theme: ThemeKind) -> Style {
    let text = match theme {
        ThemeKind::Dark => WIRE_TEXT_DARK,
        ThemeKind::Light => WIRE_TEXT_LIGHT,
        ThemeKind::System => system_must_be_resolved(),
    };
    match style.fg {
        Some(Color::Rgb(r, g, b)) => Style::default().bg(leveled(r, g, b, theme)).fg(text),
        Some(color) => Style::default().bg(color),
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
    blended(r, g, b, target, background)
}

/// A document color brought to its palette's [`RgbPalette::doc_luma`].
///
/// Unlike [`leveled`], which only ever moves a hue *toward* the page,
/// this goes both ways — toward white to lift a color that is under the
/// target on a dark theme, toward black to deepen one that is over it on
/// a light theme. It has to: the point is that every hue ends at the
/// same brightness, and the palette's spread straddles the target in
/// both themes.
///
/// Blending toward the far end of the gray axis desaturates as it goes,
/// which is the only visible cost. It is small at the distances
/// involved — the widest move in either palette is about a fifth of the
/// way — and it lands on the side that matters: the colors being lifted
/// are the dim ones, which had the least to lose.
///
/// A non-RGB color is returned untouched. In sixteen colors there is
/// nothing between the named entries to blend to, and the fallback
/// palette was chosen for legibility as a set rather than for its
/// brightnesses matching.
fn doc_leveled(color: Color, target: f32) -> Color {
    let Color::Rgb(r, g, b) = color else {
        return color;
    };
    let here = luma(f32::from(r), f32::from(g), f32::from(b));
    let background = if here < target { 255.0 } else { 0.0 };
    blended(r, g, b, target, background)
}

/// Blends a truecolor toward `background` by exactly as much as it takes
/// to land its luminance on `target`.
///
/// Luminance is affine in the blend factor, so the amount needed is a
/// division rather than a search, and the result's luminance is the
/// target exactly.
fn blended(r: u8, g: u8, b: u8, target: f32, background: f32) -> Color {
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
///
/// `Type` stays cyan here while the RGB palettes turn it orange: the
/// sixteen colors hold no orange, and both candidates for the warm slot
/// are already spoken for by a severity tier — red by `Invalid`, yellow
/// by `NonCanonical` — so borrowing one would make a declared type look
/// like an anomaly, which is the one confusion the orange was chosen to
/// prevent. What the fallback can deliver is separation between roles,
/// and cyan already delivers it.
fn style_for_dark_ansi16(role: SyntaxRole) -> Style {
    match role {
        SyntaxRole::Attribute => Style::default(),
        SyntaxRole::Type => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        SyntaxRole::StringLiteral => Style::default().fg(Color::Green),
        SyntaxRole::Number => Style::default().fg(Color::Green),
        SyntaxRole::Boolean => Style::default().fg(Color::Green),
        SyntaxRole::Constant => Style::default().fg(Color::Green),
        SyntaxRole::StringEscape => Style::default()
            .fg(Color::LightGreen)
            .add_modifier(Modifier::BOLD),
        SyntaxRole::StringSpecialUrl => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::UNDERLINED),
        SyntaxRole::Comment => Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
        SyntaxRole::PunctuationDelimiter => Style::default().fg(Color::DarkGray),
        SyntaxRole::PunctuationBracket => Style::default().fg(Color::Gray),
        SyntaxRole::PunctuationBracketList => Style::default().fg(Color::Yellow),
        SyntaxRole::PunctuationBracketExtension => Style::default().fg(Color::LightRed),
        SyntaxRole::AnnotationNonCanonical => {
            Style::default().fg(tier_color_dark_ansi16(Tier::NonCanonical))
        }
        SyntaxRole::AnnotationInvalid => Style::default().fg(tier_color_dark_ansi16(Tier::Invalid)),
    }
}

/// ANSI-16 fallback palette, light (spec 0116 §9's "ANSI-16 palette"
/// table). See `style_for_dark_ansi16` for why `Type` is not orange
/// here.
fn style_for_light_ansi16(role: SyntaxRole) -> Style {
    match role {
        SyntaxRole::Attribute => Style::default(),
        SyntaxRole::Type => Style::default()
            .fg(Color::Blue)
            .add_modifier(Modifier::BOLD),
        SyntaxRole::StringLiteral => Style::default().fg(Color::Green),
        SyntaxRole::Number => Style::default().fg(Color::Green),
        SyntaxRole::Boolean => Style::default().fg(Color::Green),
        SyntaxRole::Constant => Style::default().fg(Color::Green),
        SyntaxRole::StringEscape => Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
        SyntaxRole::StringSpecialUrl => Style::default()
            .fg(Color::Blue)
            .add_modifier(Modifier::UNDERLINED),
        SyntaxRole::Comment => Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
        SyntaxRole::PunctuationDelimiter => Style::default().fg(Color::DarkGray),
        SyntaxRole::PunctuationBracket => Style::default().fg(Color::Black),
        SyntaxRole::PunctuationBracketList => Style::default().fg(Color::Yellow),
        SyntaxRole::PunctuationBracketExtension => Style::default().fg(Color::Red),
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
/// styling — [`accent_style`], applied directly by `render_manage_pane`,
/// not through this function.)
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

/// The color of the two pieces of pane chrome that are not document
/// text and must not be mistaken for it: the manage pane's origin-path
/// header, and the heat cue's tie suffix `[3@85]`.
///
/// Both used to borrow `style_for(SyntaxRole::Boolean, …)`, which stopped
/// working when every value role collapsed onto one color — a tie count
/// is not a value, and the origin path is not a document at all. A
/// standalone function, like `manage_entry_style`: neither has a
/// `highlights.scm` capture behind it, so neither belongs in
/// `SyntaxRole`.
pub fn accent_style(theme: ThemeKind) -> Style {
    let modifier = if supports_rgb() {
        Modifier::empty()
    } else {
        Modifier::BOLD
    };
    Style::default()
        .fg(pick(
            theme,
            supports_rgb(),
            (DARK_RGB.accent, Color::Magenta),
            (LIGHT_RGB.accent, Color::Magenta),
        ))
        .add_modifier(modifier)
}

/// Purpose-designed backgrounds for the caret's row and for a matched
/// brace — not borrowed from `dark_rgb`/`light_rgb`, which are
/// foreground palettes.
///
/// The two shades of each pair must differ from each other and not only
/// from the terminal's own background: a folded node draws both members
/// of its pair on the caret's own row, so the match tint is laid over
/// the row cue there (spec 0233 S3).
mod caret_rgb {
    use ratatui::style::Color;

    /// Dark theme, the caret's row — VSCode dark's own list-hover
    /// background, a barely-there lift off `#1E1E1E`.
    pub const DARK_ROW: Color = Color::Rgb(0x2A, 0x2D, 0x2E);
    /// Dark theme, a matched brace — several steps lighter than
    /// `DARK_ROW`, so it still reads as a cell rather than as part of
    /// the row it may be sitting on.
    pub const DARK_MATCH: Color = Color::Rgb(0x51, 0x5C, 0x6A);
    /// Light theme, the caret's row.
    pub const LIGHT_ROW: Color = Color::Rgb(0xEC, 0xEC, 0xEC);
    /// Light theme, a matched brace.
    pub const LIGHT_MATCH: Color = Color::Rgb(0xC8, 0xD3, 0xE0);

    /// Dark theme, the search match the cursor will land on — a warm
    /// amber, deliberately a different hue from `DARK_MATCH`'s cool
    /// blue-gray, since a row can carry both (spec 0235 S14).
    pub const DARK_SEARCH_CURRENT: Color = Color::Rgb(0x8A, 0x63, 0x00);
    /// Dark theme, the other matches on screen — the same hue at about
    /// a third the lift, so the pair reads as one cue at two strengths.
    pub const DARK_SEARCH_OTHER: Color = Color::Rgb(0x4A, 0x38, 0x0A);
    /// Light theme, the search match the cursor will land on.
    pub const LIGHT_SEARCH_CURRENT: Color = Color::Rgb(0xFF, 0xD3, 0x6B);
    /// Light theme, the other matches on screen.
    pub const LIGHT_SEARCH_OTHER: Color = Color::Rgb(0xFF, 0xEE, 0xC2);

    /// Dark theme, the typed pattern while the sweep is still running.
    ///
    /// Hue 60°, pure yellow, at the loud colors' saturation of 0.70.
    /// It used to borrow `tier_non_canonical`, which is 36° — an amber
    /// whose own comment calls it "Yellow Orange" — and it was read as
    /// orange, which is what it is. Nothing on the command row is
    /// leveled or sits beside an anomaly, so this hue has only the
    /// other two prompt colors to stay clear of, and red and violet are
    /// both a long way from 60°.
    pub const DARK_SEARCH_RUNNING: Color = Color::Rgb(0xFF, 0xFF, 0x4D);
    /// Light theme, the typed pattern while the sweep is still running
    /// — the same hue taken down to luma 108, which is where
    /// `tier_non_canonical` sits on white and about as deep as a yellow
    /// can go before it stops being one.
    pub const LIGHT_SEARCH_RUNNING: Color = Color::Rgb(0x7A, 0x7A, 0x00);

    /// Dark theme, the selection — VSCode dark's own
    /// `editor.selectionBackground`. Well clear of `DARK_ROW`, since a
    /// selection on the caret's own row is the common case, and of
    /// `DARK_MATCH`, which may sit inside it.
    pub const DARK_SELECTION: Color = Color::Rgb(0x26, 0x4F, 0x78);
    /// Light theme, the selection.
    pub const LIGHT_SELECTION: Color = Color::Rgb(0xAD, 0xD6, 0xFF);

    /// Spec 0271 S15, dark theme: the script pane's background — the
    /// rule's own hue and saturation taken down to lightness 0.22, so
    /// the pane and the rule that borders it are recognizably one
    /// region and the pane still sits below the document in weight.
    ///
    /// It began at `#142A1C`, half this lightness and below `DARK_ROW`'s,
    /// on the argument that the blob must dominate. On a projector it
    /// simply did not read as green at all, and a commentary pane nobody
    /// can see the edges of is not a pane. The document is a whole
    /// screen and the pane is at most twelve rows (`PANE_MAX`), so what
    /// dominates is decided by area, not by tint.
    pub const DARK_SCRIPT_BG: Color = Color::Rgb(0x25, 0x4B, 0x30);
    /// Dark theme: the separator rule and its legend — the same hue well
    /// clear of the pane it borders, since it carries text.
    pub const DARK_SCRIPT_RULE: Color = Color::Rgb(0x5E, 0xB0, 0x76);
    /// Light theme: the script pane's background — `LIGHT_SCRIPT_RULE`'s
    /// hue and saturation at lightness 0.88, for `DARK_SCRIPT_BG`'s
    /// reason.
    pub const LIGHT_SCRIPT_BG: Color = Color::Rgb(0xD3, 0xEE, 0xDB);
    /// Light theme: the separator rule and its legend.
    pub const LIGHT_SCRIPT_RULE: Color = Color::Rgb(0x2F, 0x7D, 0x46);
}

/// Spec 0271 S15: the script pane's background.
///
/// A background only — the commentary keeps the terminal's own
/// foreground, since it is prose and has no syntax to color. On ANSI-16
/// there is no green dim enough to sit behind text without fighting it,
/// so the pane falls back to no background at all and the separator
/// alone carries the cue.
pub fn script_pane_style(theme: ThemeKind) -> Style {
    match pick(
        theme,
        supports_rgb(),
        (Some(caret_rgb::DARK_SCRIPT_BG), None),
        (Some(caret_rgb::LIGHT_SCRIPT_BG), None),
    ) {
        Some(bg) => Style::default().bg(bg),
        None => Style::default(),
    }
}

/// Spec 0271 S15: the separator rule under the script pane, and the
/// micro-help legend written on it.
pub fn script_rule_style(theme: ThemeKind) -> Style {
    Style::default().fg(pick(
        theme,
        supports_rgb(),
        (caret_rgb::DARK_SCRIPT_RULE, Color::Green),
        (caret_rgb::LIGHT_SCRIPT_RULE, Color::Green),
    ))
}

/// Spec 0194 S2: the caret itself — one character drawn inside out,
/// keeping whatever syntax color it already had.
///
/// Theme-independent, and deliberately a bare modifier rather than a
/// color pair: reversing is what a terminal block cursor does, so it
/// lands correctly on any palette the user has configured, including
/// ones this crate knows nothing about. Spec 0233 S2: nothing else in
/// the pane is drawn this way, and the caret is drawn this way
/// everywhere.
pub fn caret_style() -> Style {
    Style::default().add_modifier(Modifier::REVERSED)
}

/// The selected span (spec 0242 S11, amended 2026-08-05).
///
/// A background tint on spec 0233 S3's rule, not the inversion this
/// used to be. Inversion is the caret's idiom and only the caret's: the
/// caret is the selection's own moving end, so the two cues always meet
/// on one cell, and two reversals cancel — the character at the end of
/// the selection came out looking plain, which is the most confusing
/// thing it could look like.
///
/// The ANSI-16 fallback is magenta rather than the obvious blue. On 16
/// colors the four background cues have to stay tellable apart with no
/// shading to help them: the cursor's row is already `DarkGray`/`Gray`,
/// a matched brace `Blue`/`Cyan` and a search hit `Yellow`, and any of
/// the three can land inside a selection. Magenta is what is left that
/// still reads as a solid block behind text.
pub fn selection_style(theme: ThemeKind) -> Style {
    Style::default().bg(pick(
        theme,
        supports_rgb(),
        (caret_rgb::DARK_SELECTION, Color::Magenta),
        (caret_rgb::LIGHT_SELECTION, Color::LightMagenta),
    ))
}

/// Spec 0233 S3: the brace matching the one the caret is standing on.
///
/// A background tint rather than a second inversion — inversion is the
/// caret's idiom (`caret_style`) and sharing it is what made the two
/// cells hard to tell apart. A background also composes with the
/// character's syntax foreground instead of displacing it.
pub fn brace_match_style(theme: ThemeKind) -> Style {
    Style::default().bg(pick(
        theme,
        supports_rgb(),
        (caret_rgb::DARK_MATCH, Color::Blue),
        (caret_rgb::LIGHT_MATCH, Color::Cyan),
    ))
}

/// Spec 0235 S14: the search match the cursor lands on at `Enter`.
///
/// A background, not an inversion, on spec 0233 S3's rule — and a warm
/// hue, so that it is still tellable from `brace_match_style` and
/// `cursor_row_style` when all three land on one row.
pub fn search_current_style(theme: ThemeKind) -> Style {
    Style::default().bg(pick(
        theme,
        supports_rgb(),
        (caret_rgb::DARK_SEARCH_CURRENT, Color::LightYellow),
        (caret_rgb::LIGHT_SEARCH_CURRENT, Color::LightYellow),
    ))
}

/// Spec 0235 S14: the other occurrences of the pattern on screen —
/// the same hue as `search_current_style`, muted, since they are
/// context rather than the answer.
///
/// The ANSI-16 fallback keeps the hue and gives up the muting: the
/// obvious dim choice there is `DarkGray`/`Gray`, which is what
/// `cursor_row_style` already falls back to, and a match that vanishes
/// on the cursor's own row is worse than one that is merely a shade
/// less bright than the current one.
pub fn search_match_style(theme: ThemeKind) -> Style {
    Style::default().bg(pick(
        theme,
        supports_rgb(),
        (caret_rgb::DARK_SEARCH_OTHER, Color::Yellow),
        (caret_rgb::LIGHT_SEARCH_OTHER, Color::Yellow),
    ))
}

/// Spec 0235 S10: the typed pattern once the live sweep has finished
/// with no match — the whole document seen, the answer is no.
///
/// A foreground rather than a background, because it colors the prompt
/// row's own text and not the document.
pub fn search_unmatched_style(theme: ThemeKind) -> Style {
    Style::default().fg(pick(
        theme,
        supports_rgb(),
        (DARK_RGB.tier_invalid, Color::Red),
        (LIGHT_RGB.tier_invalid, Color::Red),
    ))
}

/// The typed pattern when the sweep has finished with no match but the
/// bake still owes the document text — "not in what has been read",
/// which is not the claim `search_unmatched_style` makes.
///
/// Spec 0249 S12's `Status::Unbaked` violet rather than a fourth hue,
/// and here the reuse is not economy: the fold margin is already drawing
/// that color against the very subtrees this answer is missing, and the
/// activity dot is already drawing it for the bake as a whole. The
/// prompt is reporting the same fact they are, so it says it in the
/// same word.
pub fn search_unbaked_style(theme: ThemeKind) -> Style {
    Style::default().fg(pick(
        theme,
        supports_rgb(),
        (DARK_RGB.status_unbaked, Color::LightMagenta),
        (LIGHT_RGB.status_unbaked, Color::Magenta),
    ))
}

/// Spec 0237 S11/S12: the typed pattern while the sweep is still
/// running and has not found anything *yet*. Distinct from
/// `search_unmatched_style` so that a slow sweep does not read as a
/// failed one.
///
/// Its own yellow rather than the `Tier::NonCanonical` amber it started
/// out borrowing. That borrowing was argued as economy — "no verdict
/// yet" sitting in the same register as "suspicious but not wrong" — but
/// the tier is an *orange*, chosen at 36° precisely so that it could not
/// be mistaken for the yellow next to it in the document, and on the
/// command row it simply read as orange. The three prompt states are
/// their own scale, and they are the whole of what this color has to be
/// legible against.
pub fn search_running_style(theme: ThemeKind) -> Style {
    Style::default().fg(pick(
        theme,
        supports_rgb(),
        (caret_rgb::DARK_SEARCH_RUNNING, Color::Yellow),
        (caret_rgb::LIGHT_SEARCH_RUNNING, Color::Yellow),
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

/// Spec 0286 S6: the accent the viewport label wears while the end of
/// the content is being pushed against.
///
/// A **named** ANSI color, with no `supports_rgb` dispatch: it is
/// painted onto a statusline that is already `REVERSED`, so it becomes
/// that span's visible background and has to read as a solid block
/// against both `focus_style`'s white and `unfocused_pane_style`'s gray
/// on a 16-color terminal as well as a true-color one. Yellow is the
/// conventional "held, not stuck", and is the one bright ANSI color
/// neither bar already uses.
///
/// Theme-independent for the same reason `focus_style` is: it names a
/// state of the application, not a role in the document.
pub fn edge_resistance_color() -> Color {
    Color::Yellow
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

    /// Holds `ENV_MUTEX` and puts `name` back the way it was found.
    ///
    /// The mutex alone is not enough. It only serializes the tests in
    /// this module, and the variables are read from outside it:
    /// `supports_rgb` looks `COLORTERM` up uncached, unlocked, on every
    /// call, from every test in the binary. So a test that ended by
    /// *removing* `COLORTERM` — as these did — did not restore a
    /// pristine environment, it silently pushed the whole rest of the
    /// process onto the ANSI-16 palette for the remainder of the run,
    /// and the RGB palette stopped being exercised through `style_for`
    /// at all. Restoring on `Drop` rather than at the end of the body
    /// also survives a failing assertion.
    struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        name: &'static str,
        prior: Option<String>,
    }

    impl EnvGuard {
        fn new(name: &'static str) -> Self {
            let lock = ENV_MUTEX
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Self {
                _lock: lock,
                name,
                prior: std::env::var(name).ok(),
            }
        }

        fn set(&self, value: &str) {
            // SAFETY: no other thread touches this variable — the guard
            // holds `ENV_MUTEX` for as long as it lives.
            unsafe { std::env::set_var(self.name, value) }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: as above — the lock is released only after this.
            unsafe {
                match &self.prior {
                    Some(value) => std::env::set_var(self.name, value),
                    None => std::env::remove_var(self.name),
                }
            }
        }
    }

    #[test]
    fn resolve_system_reads_colorfgbg_dark() {
        let guard = EnvGuard::new("COLORFGBG");
        guard.set("15;0");
        assert_eq!(resolve_system(), ThemeKind::Dark);
    }

    #[test]
    fn resolve_system_reads_colorfgbg_light() {
        let guard = EnvGuard::new("COLORFGBG");
        guard.set("0;15");
        assert_eq!(resolve_system(), ThemeKind::Light);
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
        let guard = EnvGuard::new("COLORFGBG");
        for fg_bg in ["15;0", "0;15"] {
            guard.set(fg_bg);
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
            brace_match_style(theme);
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
    }

    const ALL_ROLES: [SyntaxRole; 15] = [
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
        SyntaxRole::AnnotationNonCanonical,
        SyntaxRole::AnnotationInvalid,
    ];

    // `PunctuationDelimiter`/`PunctuationBracket` are deliberately
    // unstyled (terminal default) in both the RGB and ANSI-16 palettes
    // — excluded from the "must be Rgb" assertion below.
    const COLORED_ROLES: [SyntaxRole; 13] = [
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
        SyntaxRole::AnnotationNonCanonical,
        SyntaxRole::AnnotationInvalid,
    ];

    /// Spec 0225 §S11: the annotation reaches a tier's color through a
    /// capture and a `SyntaxRole`, the wire row asks for it by name.
    /// They must be the same color, in all four palettes, or the two
    /// rows of one document contradict each other about severity.
    ///
    /// Since spec 0267 there is no exception: both surviving tiers are
    /// anomalies, so neither is leveled and both are equal outright.
    #[test]
    fn a_tier_looks_the_same_named_as_it_does_captured() {
        let pairs = [
            (Tier::NonCanonical, SyntaxRole::AnnotationNonCanonical),
            (Tier::Invalid, SyntaxRole::AnnotationInvalid),
        ];
        for theme in [ThemeKind::Dark, ThemeKind::Light] {
            for rgb in [false, true] {
                for (tier, role) in pairs {
                    assert_eq!(
                        Some(tier_color(tier, theme, rgb)),
                        style_for_in(role, theme, rgb).fg,
                        "{tier:?} disagrees with {role:?} ({theme:?}, rgb={rgb})",
                    );
                }
            }
        }
    }

    /// The tiers must be told apart at a glance, so no two of them
    /// may share a color. Not asserted against the *roles*: ANSI-16 has
    /// sixteen colors and `PunctuationBracketList` already holds yellow,
    /// so a collision there is forced, and harmless — a tier and a
    /// bracket are never candidates for the same span.
    #[test]
    fn no_two_tiers_share_a_color() {
        for theme in [ThemeKind::Dark, ThemeKind::Light] {
            for rgb in [false, true] {
                let tiers = [Tier::NonCanonical, Tier::Invalid];
                let colors: Vec<_> = tiers.iter().map(|&t| tier_color(t, theme, rgb)).collect();
                for (i, c) in colors.iter().enumerate() {
                    assert!(
                        !colors[i + 1..].contains(c),
                        "two tiers share {c:?} ({theme:?}, rgb={rgb})",
                    );
                }
            }
        }
    }

    /// Spec 0260 S1: every color the fold margin can wear is saturated
    /// enough to read as a hue rather than as a tint of the foreground,
    /// **and** dark enough not to read as white.
    ///
    /// The margin wears these *unleveled* — prominence is the column's
    /// whole job — so nothing but the color itself separates one from
    /// white on a dark theme, or from black on a light one.
    /// `Tier::Landmark` used to sit at 0.318 saturation against the
    /// others' 0.667 and up, and was reported as too close to the
    /// default foreground.
    ///
    /// The luma ceiling was added 2026-08-10, after the saturation floor
    /// alone passed a yellow that was still reported as white. Both are
    /// needed and neither implies the other: yellow is the one hue whose
    /// fully saturated form is already near white in luminance, so
    /// raising its saturation — which is what fixed the violet — moves
    /// it no further away.
    ///
    /// 185 is where the ceiling ended up, and it is deliberately snug:
    /// two successive yellows were rejected by eye, at luma 232 and then
    /// at 200, so a ceiling that either could have passed would not have
    /// been carrying the constraint that was actually learned. It leaves
    /// `Status::Unknown`'s blue — the brightest of the four that survive,
    /// at 173 — twelve points of room, and everything else far more.
    /// A future color that has to sit above 185 needs a reason on the
    /// record, not a raised number.
    ///
    /// Floor and ceiling are stated rather than the values: this exists
    /// to hand the constraint to the next person choosing one of these
    /// colors, not to pin the four they inherited.
    #[test]
    fn every_status_color_is_a_hue_and_not_a_tint() {
        let statuses = [
            Status::Unbaked,
            Status::Unknown,
            Status::NonCanonical,
            Status::Invalid,
        ];
        for theme in [ThemeKind::Dark, ThemeKind::Light] {
            for status in statuses {
                let Some(Color::Rgb(r, g, b)) = status_color_in(status, theme, true) else {
                    panic!("{status:?} has no truecolor entry on {theme:?}");
                };
                let (hi, lo) = (r.max(g).max(b), r.min(g).min(b));
                let saturation = f32::from(hi - lo) / f32::from(hi);
                assert!(
                    saturation >= 0.6,
                    "{status:?} on {theme:?} is a tint, not a hue: \
                     #{r:02X}{g:02X}{b:02X}, saturation {saturation:.3}",
                );
                let luma = 0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b);
                assert!(
                    luma <= 185.0,
                    "{status:?} on {theme:?} is bright enough to read as white: \
                     #{r:02X}{g:02X}{b:02X}, luma {luma:.0} of 255",
                );
            }
        }
    }

    /// The fold margin is a five-color scale — unstyled, violet, blue,
    /// yellow, red — and the four that carry a hue have to be told apart
    /// in a single glyph, with no second cue and nothing beside them.
    ///
    /// `NonCanonical` sat 36° from `Invalid` until 2026-08-10 and was
    /// reported as reading as a red. The floor is what the fix bought,
    /// not a claim that one degree less would fail: 55° on `Dark`, and
    /// 50° is stated because `Light` cannot do better — its `Invalid` is
    /// at 5° rather than 0°, and a yellow cannot answer by climbing past
    /// 60° without becoming a green. The two are further apart there
    /// than the hues alone say, the light yellow being half the
    /// scarlet's lightness.
    ///
    /// A color moved into this column has to clear the floor against all
    /// three others, which is the part that is easy to forget.
    #[test]
    fn no_two_fold_margin_colors_share_a_neighborhood() {
        /// Hue in degrees, or `None` for a gray, which has none.
        fn hue(color: Color) -> Option<f32> {
            let Color::Rgb(r, g, b) = color else {
                panic!("{color:?} is not a truecolor entry");
            };
            let (r, g, b) = (f32::from(r), f32::from(g), f32::from(b));
            let (hi, lo) = (r.max(g).max(b), r.min(g).min(b));
            let span = hi - lo;
            if span == 0.0 {
                return None;
            }
            let h = if hi == r {
                (g - b) / span
            } else if hi == g {
                2.0 + (b - r) / span
            } else {
                4.0 + (r - g) / span
            };
            Some((h * 60.0).rem_euclid(360.0))
        }

        let statuses = [
            Status::Unbaked,
            Status::Unknown,
            Status::NonCanonical,
            Status::Invalid,
        ];
        for theme in [ThemeKind::Dark, ThemeKind::Light] {
            let hues: Vec<(Status, f32)> = statuses
                .iter()
                .map(|&s| {
                    let color = status_color_in(s, theme, true).expect("a margin color");
                    (s, hue(color).expect("a margin color is never a gray"))
                })
                .collect();
            for (i, &(a, ha)) in hues.iter().enumerate() {
                for &(b, hb) in &hues[i + 1..] {
                    let apart = (ha - hb).abs().min(360.0 - (ha - hb).abs());
                    assert!(
                        apart >= 50.0,
                        "{a:?} ({ha:.0}°) and {b:?} ({hb:.0}°) are {apart:.0}° apart \
                         on {theme:?} — too close to tell apart in one glyph",
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
        let guard = EnvGuard::new("COLORTERM");
        guard.set("truecolor");
        assert!(supports_rgb());
        guard.set("24bit");
        assert!(supports_rgb());
        guard.set("256color");
        assert!(!colorterm_reports_truecolor());
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
