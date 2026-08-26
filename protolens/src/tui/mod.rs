// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The interactive TUI: `App`, its state, and the main loop.
//!
//! A main pane showing the decoded document, plus one side pane that is
//! either the override *selection* pane (pick a type for a range) or the
//! override *management* pane (review and toggle the collection), and a
//! global command/message row (spec 0147). Navigation is a caret over
//! `LinePos { node, line_in_node }` (spec 0194) with folding, a
//! jumplist, search, and mouse support.
//!
//! The node arena is built once from the bytes and never mutated (spec
//! 0216); applying an override rewrites per-slot overlay state and
//! re-renders, but no index ever moves.

use std::collections::HashSet;
use std::collections::VecDeque;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use crossterm::cursor::Show;
use crossterm::event::{
    self as term_event, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use regex::{Regex, RegexBuilder};
use regex_cursor::engines::meta::{
    Builder as CursorBuilder, Config as CursorConfig, Regex as CursorRegex,
};
use regex_cursor::regex_automata::util::syntax::Config as CursorSyntax;
use regex_cursor::Input as CursorInput;
use regex_syntax::hir::{Class, Hir, HirKind, Look};

use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use prototext_core::serialize::render_text::{
    decode_and_render, decode_and_render_indexed, DecodeRenderOpts, FqdnId, FqdnTable, NodeKind,
    NodeSpan, NO_PACKED_RECORD,
};
use prototext_core::Arena;
// Every `NO_FQDN` outside `decode.rs` is in a test module building a
// literal `NodeSpan`.
#[cfg(test)]
use prototext_core::serialize::render_text::NO_FQDN;

#[cfg(test)]
use crate::blob::wrapped;
use crate::blob::Blob;
use crate::colorize::{self, LineStyles, SyntaxRole};
use crate::decode::{self, widen, Decoded, DescriptorContext, TreeNode};
use crate::fold_set::FoldSet;
use crate::node_status::Status;
pub(crate) use lines::LinePos;

use crate::export_descriptor;
use crate::extract::{self, ExtractFormat};
use crate::override_pane::{self, OverrideKind, OverrideOrigin, SortMode};
use crate::provenance::{ProvenanceTable, NOT_RENDERED};
use crate::render_cache::RenderCache;
use crate::theme::{self, ThemeKind};
use bake::BakeStep;
use help_text::HELP_TEXT;
use menu::{key_label, Menu};
use pane_scroll::{
    AnchorLine, EdgeResistance, PaneScroll, RowHeights, WireAnchor, WireRowCache, WireSpan,
    FLAT_ROWS,
};
use popup::{BoxLine, Breakdown, Hover, Popup, PopupBody, WireBox};
use prefetch::{PrefetchStep, PrefetchTrace, PrefetchWalk};
use search::{SearchScope, SweepStep};
pub use terminal::run;

/// Fixed horizontal-pan step, in columns (spec 0113 D24) — a generous but
/// simple constant rather than a fraction of the pane's width, so panning
/// speed doesn't change as the pane is resized. Also used for Ctrl-Up/
/// Ctrl-Down vertical panning (`pan_by_step_clamped`).
const PAN_STEP: usize = 8;

/// Vertical-pan step for the plain mouse wheel — one row per notch,
/// the granularity a wheel notch is expected to have, unlike
/// `PAN_STEP`'s larger Ctrl-Up/Ctrl-Down/Shift+wheel jump.
const WHEEL_PAN_STEP: usize = 1;

/// Minimum terminal width (columns) below which `t` refuses to open the
/// override pane (spec 0114 §2). 60 rather than a rounder 100 because
/// the borderless-pane layout (spec 0147) needs less horizontal room
/// than a bordered one.
const MIN_OVERRIDE_WIDTH: u16 = 60;

/// Maximum gap between two same-line `Down(MouseButton::Left)` events
/// for the second to count as a double-click. Crossterm reports `Down`
/// identically for single and double clicks, so the app disambiguates
/// them itself by comparing consecutive timestamps/positions
/// (`App::last_click`).
const DOUBLE_CLICK_THRESHOLD: Duration = Duration::from_millis(500);

/// Whether a click identified by `key` (a main-pane line index, or a
/// manage-pane entry index), arriving now, is the second half of a
/// double-click against `last`'s previously recorded click — same `key`,
/// within `DOUBLE_CLICK_THRESHOLD` — updating `last` to this click
/// either way. Shared by the main pane and by the manage pane's
/// radio-marker, which offers double-click as an alternative to
/// Shift-click: most terminal emulators intercept Shift-click for
/// native text selection before it ever reaches the app.
fn is_double_click<T: PartialEq>(last: &mut Option<(Instant, T)>, key: T) -> bool {
    let now = Instant::now();
    let is_double = matches!(
        last,
        Some((t, prev)) if *prev == key && now.duration_since(*t) < DOUBLE_CLICK_THRESHOLD
    );
    *last = Some((now, key));
    is_double
}

/// How long a passive status message stays visible in the global
/// command/message row before `track_message_timeout` auto-dismisses it
/// — doesn't apply while that row is actively serving as a text-entry
/// prompt (see that function's doc comment).
const MESSAGE_TIMEOUT: Duration = Duration::from_secs(3);

/// Shown in the global command/message row when the user tries to take
/// focus back to the main pane while the override selection pane is open
/// — by `Tab` or by clicking the main pane. Spec 0185 S5 makes both
/// inert, and silently ignoring them reads as a broken key rather than a
/// deliberate lock, so say why. Auto-dismisses like any other message.
const OVERRIDE_FOCUS_LOCK_MESSAGE: &str =
    "main pane locked while the type pane is open - Esc closes it, Alt-arrows pan it";

/// Auto-dismiss delay for the startup splash screen, in addition to its
/// keypress/mouse dismissal — mirrors `MESSAGE_TIMEOUT`'s
/// deadline-based approach.
const SPLASH_TIMEOUT: Duration = Duration::from_secs(3);

/// Byte budget for `App::render_cache` (spec 0116 §8).
///
/// Derived (spec 0251 S8), where before it was asserted. Since S5 the
/// cache holds preview renders and nothing else, so its unit is one
/// preview and the question is how many a user arrows through.
///
/// A preview's input is bounded by `override_preview_byte_budget`, and
/// its *output* was measured against that bound at the default 4096
/// bytes: **104 512 B** — 2051 lines and 2049 spans — for the worst
/// shape the budget admits, a two-byte field per line
/// (`measure_a_preview_renders_size`). Call it ~25x the input budget.
/// A screenful of the ranked candidate list is ~50 entries, so holding
/// one pass through it costs ~5 MB; 8 MB leaves margin and is nothing
/// against a document that sits at ~1 GB.
///
/// Note the interaction with `--override-preview-byte-budget`: raise it
/// far enough and a single preview exceeds this budget, at which point
/// `RenderCache::insert` rejects every entry and the cache quietly
/// stops working. At the default there are ~80 worst-case entries of
/// headroom.
const RENDER_CACHE_MAX_BYTES: usize = 8 << 20;

/// Single source-of-truth command-name registry (spec 0113 D26) — backs
/// both `resolve_command`'s exact-match-wins prefix dispatch and the
/// command line's Tab-completion (`App::start_tab_completion`).
///
/// It is the source of truth for the *name*, and for nothing else. A
/// command added here alone is dispatchable, completable and useless: it
/// still needs an arm in `run_command`, which otherwise reports it as
/// unimplemented, and an entry in `HELP_TEXT`, without which nobody
/// finds it. The help half is the one that went unnoticed — `proto-root`
/// shipped undocumented — and
/// `tests/help_text.rs::every_command_is_named_in_the_help` is what
/// notices it now.
const COMMANDS: &[&str] = &[
    "export",
    "quit",
    "help",
    "override",
    "save-overrides",
    "restore-overrides",
    "proto-root",
];

/// Every option each command accepts, for the command line's
/// flag completion (spec 0236 S22): a token beginning with `-`
/// completes against this rather than against the command's own value
/// candidates, the way a shell's completion does.
///
/// Same standing as `COMMANDS`: the source of truth for the *spelling*
/// of a flag, and for nothing else. A flag listed here still needs the
/// arm in its command's own argument loop that acts on it.
fn command_flags(cmd: &str) -> &'static [&'static str] {
    match cmd {
        "export" => &[
            "--binary",
            "--descriptor-binary",
            "--descriptor-prototext",
            "--prototext",
        ],
        "override" => &["--as", "--as-new", "--field-name"],
        _ => &[],
    }
}

/// Filter `candidates` to those starting with `prefix` (spec 0113 D26) — a
/// small generic primitive, not ad hoc to any one caller.
fn complete_prefix<'a>(prefix: &str, candidates: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    candidates.filter(|c| c.starts_with(prefix)).collect()
}

/// Longest common leading substring of `items` (byte-safe: only cuts at
/// `char` boundaries). Empty if `items` is empty.
fn longest_common_prefix(items: &[&str]) -> String {
    let Some((&first, rest)) = items.split_first() else {
        return String::new();
    };
    let mut end = first.len();
    for item in rest {
        let mut new_end = 0;
        for (a, b) in first.chars().zip(item.chars()) {
            if a != b {
                break;
            }
            new_end += a.len_utf8();
        }
        end = end.min(new_end);
    }
    first[..end].to_string()
}

/// Spec 0273 S1: what a `/`/`?`/`n` pattern is — a positional path, or
/// a regular expression, decided by the pattern's shape and never by
/// both at once.
///
/// Spec 0195 S2's `smartcase` rule survives in both matching variants:
/// an all-lowercase pattern matches case-insensitively, a pattern
/// carrying any uppercase *literal* matches exactly. The rule is decided
/// once, at construction, rather than per candidate.
#[derive(Debug)]
pub(super) enum SearchPattern {
    /// Spec 0273 S2/S3: a pattern of `/` and ASCII digits searches
    /// positional paths, and nothing else. Segments run **root-first**,
    /// which is the reverse of [`PathScratch::segments`].
    Path(Vec<usize>),
    /// Spec 0273 S7: a regex whose parse is a plain literal runs the
    /// pre-folded needle and the `memchr2` prefilter spec 0235 measured
    /// at six times the every-position walk, rather than the general
    /// engine.
    Literal {
        /// Already lowercased when the search is case-insensitive, so
        /// the per-candidate comparison only has to fold the haystack.
        needle: String,
        case_sensitive: bool,
    },
    /// Spec 0273 S6. Boxed because a `Regex` carries its own cache pool
    /// and would otherwise set this enum's size for every variant.
    Regex(Box<Regex>),
    /// Spec 0274 S3: a pattern that can match a `\n`, and so has to read
    /// the document as the rendered rows joined by one.
    ///
    /// A different engine rather than a flag on `Regex`: this one
    /// searches through a chunked [`regex_cursor::Cursor`], because spec
    /// 0222 leaves the document with no contiguous haystack to hand a
    /// `&str` matcher. A pattern compiles into exactly one variant, so
    /// the two engines never both hold it.
    ///
    /// `Arc` rather than `Box` because spec 0274 S9 runs this engine on
    /// a worker thread; a compiled `Regex` is immutable and its cache
    /// pool is internally synchronized, so sharing it costs a refcount.
    Multi(Arc<CursorRegex>),
}

/// Spec 0273 S6: a pattern that needs more than this to compile is
/// refused. The `regex` crate's own default is ten times larger; the
/// tighter bound is spec 0272's, which recompiles on every keystroke and
/// cannot afford a pattern that takes visible time to build.
const PATTERN_SIZE_LIMIT: usize = 1 << 20;

impl SearchPattern {
    /// Spec 0273 S8: `Err` is a pattern the reader is still typing as
    /// much as it is a mistake, so the caller decides whether to say
    /// anything. The string is the compile diagnostic, for `Enter`.
    pub(super) fn new(pattern: &str) -> Result<Self, String> {
        if let Some(segments) = path_segments(pattern) {
            return Ok(Self::Path(segments));
        }
        // Parsed with the same two flags the builder below is given, so
        // that `^` reads as `Look::StartLM` here and `\A` — which S12
        // refuses — stays distinguishable from it. Case folding is left
        // *off*: S9 reads this HIR's literals to decide it.
        let hir = regex_syntax::ParserBuilder::new()
            .multi_line(true)
            .dot_matches_new_line(false)
            .build()
            .parse(pattern)
            .map_err(|e| e.to_string())?;
        if hir_has_haystack_anchor(&hir) {
            return Err("\\A and \\z anchor to the search window, not the \
                        document; use ^ and $"
                .to_string());
        }
        let case_sensitive =
            hir_has_uppercase_literal(&hir) || pattern_settles_its_own_case(pattern, &hir);
        // Spec 0274 S2: a pattern that can match a `\n` reads the
        // document as one string, so it takes the cursor engine — and
        // takes it whether or not it *has* to, because a reader cannot
        // be asked to know that `a(\n|\r\n)b` crosses a row while
        // `a\s*b` does not.
        if hir_admits_newline(&hir) {
            return CursorBuilder::new()
                .syntax(
                    CursorSyntax::new()
                        .multi_line(true)
                        .dot_matches_new_line(false)
                        .case_insensitive(!case_sensitive),
                )
                // Spec 0274 S3: the same bound the `regex` builder is
                // given below, under the name regex-automata uses for
                // it.
                .configure(CursorConfig::new().nfa_size_limit(Some(PATTERN_SIZE_LIMIT)))
                .build(pattern)
                .map(|re| Self::Multi(Arc::new(re)))
                .map_err(|e| e.to_string());
        }
        if let Some(text) = hir_literal(&hir) {
            let needle = if case_sensitive {
                text
            } else {
                text.to_lowercase()
            };
            return Ok(Self::Literal {
                needle,
                case_sensitive,
            });
        }
        RegexBuilder::new(pattern)
            .multi_line(true)
            .dot_matches_new_line(false)
            // Spec 0273 S9: a *default*, not a restriction — an inline
            // `(?-i)` overrides it, which is vim's `\C`.
            .case_insensitive(!case_sensitive)
            .size_limit(PATTERN_SIZE_LIMIT)
            .build()
            .map(|re| Self::Regex(Box::new(re)))
            .map_err(|e| e.to_string())
    }

    /// Spec 0273 S5: which of the two haystacks this pattern is for. A
    /// path pattern is never matched against row text, and a regex is
    /// never matched against a path.
    pub(super) fn is_path(&self) -> bool {
        matches!(self, Self::Path(_))
    }

    /// Spec 0273 S3: whether the pattern is a **segment-wise** prefix of
    /// the path whose segments are `leaf_first` — so `/1` matches `/1`
    /// and `/1/23`, and matches neither `/12` nor `/2/1`.
    ///
    /// A path is read from the root down, so the part of it a reader
    /// knows is its head; an unanchored match on a path is almost always
    /// an accident, since `/2` occurs somewhere inside most of a large
    /// document's paths.
    ///
    /// `leaf_first` is [`PathScratch::segments`] as
    /// `write_path_segments` leaves it, and comparing against it rather
    /// than against the rendered text is what makes a path candidate
    /// cost no string at all.
    pub(super) fn matches_path(&self, leaf_first: &[usize]) -> bool {
        let Self::Path(wanted) = self else {
            return false;
        };
        wanted.len() <= leaf_first.len()
            && wanted
                .iter()
                .zip(leaf_first.iter().rev())
                .all(|(a, b)| a == b)
    }

    /// The first match's byte range at or after `from`. Spec 0246 S6:
    /// the walk stops at every match rather than every row, and both of
    /// its picks — the first eligible start going forward, the last one
    /// going backward — are loops over this, so they inherit
    /// [`Self::find_range`]'s prefilter and both of its ASCII guards.
    ///
    /// `from` is clamped and rounded *up* to a character boundary,
    /// which costs nothing: no match can start inside a character, so
    /// rounding up cannot skip one. That totality is what lets a caller
    /// pass "one byte past the caret" without first asking how wide the
    /// character under it was.
    ///
    /// The regex arm goes through `find_at` rather than searching a
    /// slice, because `^` and `\b` are decided by what precedes `from`
    /// and a slice would hide it.
    pub(super) fn find_range_from(&self, haystack: &str, from: usize) -> Option<Range<usize>> {
        let mut from = from.min(haystack.len());
        while !haystack.is_char_boundary(from) {
            from += 1;
        }
        match self {
            Self::Path(_) => None,
            Self::Regex(re) => re.find_at(haystack, from).map(|m| m.range()),
            // Spec 0274 S3: `set_start` is this engine's `find_at`. It
            // narrows where the search may begin without narrowing what
            // the look-around can see, which is the whole reason 0273
            // refused to slice.
            Self::Multi(re) => {
                let mut input = CursorInput::new(haystack);
                input.set_start(from);
                re.find(input).map(|m| m.range())
            }
            Self::Literal { .. } => self
                .find_range(&haystack[from..])
                .map(|r| from + r.start..from + r.end),
        }
    }

    /// The first match's byte range. Spec 0235 S14 tints a match over
    /// its whole extent, and folding makes that extent the haystack's
    /// own rather than the needle's — `İ` is two bytes on screen and
    /// three folded characters in the needle. Spec 0194 S8 needs the
    /// range's start for the same reason: a search hit puts the caret on
    /// the match, not merely on its row.
    ///
    /// Spec 0235 S1: the case-insensitive arm runs `starts_with_folded`
    /// only at positions a `memchr2` over the needle's first character's
    /// two cases proposes, instead of at every one. Spec 0235's
    /// Background 4 measured the same walk with a `memchr` in front of
    /// it — that is what the case-*sensitive* row of its table is — at
    /// six times the speed.
    ///
    /// Both guards are load-bearing, and the second is the subtle one.
    /// A prefilter is only the same predicate when folding is one byte
    /// to one byte, which is true of ASCII and not of Unicode in either
    /// direction: `U+212A` KELVIN SIGN folds to `k` and `U+0130` folds
    /// to `i` plus a combining dot, so a haystack holding either would
    /// lose a match that the every-position walk finds. Rendered
    /// textproto is ASCII on all but a handful of string values, so the
    /// fallback is rare and exact.
    pub(super) fn find_range(&self, haystack: &str) -> Option<Range<usize>> {
        let (needle, case_sensitive) = match self {
            // Spec 0273 S5: a path pattern has no text haystack.
            Self::Path(_) => return None,
            Self::Regex(re) => return re.find(haystack).map(|m| m.range()),
            Self::Multi(re) => return re.find(CursorInput::new(haystack)).map(|m| m.range()),
            Self::Literal {
                needle,
                case_sensitive,
            } => (needle, *case_sensitive),
        };
        if case_sensitive {
            let at = haystack.find(needle)?;
            return Some(at..at + needle.len());
        }
        if let Some(first) = needle.as_bytes().first().copied() {
            if first.is_ascii() && haystack.is_ascii() {
                let upper = first.to_ascii_uppercase();
                return memchr::memchr2_iter(first, upper, haystack.as_bytes())
                    .find_map(|i| folded_prefix_len(&haystack[i..], needle).map(|n| i..i + n));
            }
        }
        haystack
            .char_indices()
            .find_map(|(i, _)| folded_prefix_len(&haystack[i..], needle).map(|n| i..i + n))
    }
}

/// Spec 0273 S2: the pattern's segments when it is a **path pattern** —
/// `/` and ASCII digits, starting with `/`, with no empty segment other
/// than a single trailing `/`.
///
/// So `/`, `/1`, `/1/2` and `/1/2/` are paths, while `/1/a`, `1/2`,
/// `//2` and `/1 ` are not. The shape test is the whole of the dispatch
/// rule (spec 0273 N4): there is no `path:` prefix to learn, and it is
/// decidable by eye.
///
/// A bare `/` yields no segments and is therefore a prefix of every
/// path, so it walks every node. Useless and consistent; consistency
/// wins, and it costs the shape test no special case.
fn path_segments(pattern: &str) -> Option<Vec<usize>> {
    let body = pattern.strip_prefix('/')?;
    let body = body.strip_suffix('/').unwrap_or(body);
    if body.is_empty() {
        return Some(Vec::new());
    }
    body.split('/')
        .map(|seg| {
            (!seg.is_empty() && seg.bytes().all(|b| b.is_ascii_digit()))
                .then(|| seg.parse().ok())
                .flatten()
        })
        .collect()
}

/// Spec 0273 S9: whether any *literal* character of the parsed pattern
/// is uppercase — which is what smartcase is about.
///
/// The raw pattern text would be the wrong thing to read: it would make
/// `\D`, `\W` and `\S` case-sensitive patterns, which is not what the
/// reader typing them meant.
fn hir_has_uppercase_literal(hir: &Hir) -> bool {
    fold_hir(hir, &mut |kind| match kind {
        HirKind::Literal(lit) => {
            std::str::from_utf8(&lit.0).is_ok_and(|s| s.chars().any(char::is_uppercase))
        }
        _ => false,
    })
}

/// Spec 0274 S2: whether *some* string the pattern matches contains a
/// newline — which is when reading the document one row at a time could
/// give a different answer from reading it whole.
///
/// The question is about *some* string in the language, not about every
/// one — `\s*id` admits a newline without ever requiring one, and
/// `[0-9]+` neither admits nor requires. Routing on what a pattern
/// *requires* would leave `a(\n|\r\n)b` crossing a row while `a\s*b` did
/// not, which is a distinction no reader can be asked to hold.
///
/// The recursion is exact rather than merely sound, and it cannot be
/// [`fold_hir`] even though it is an *any*-node question: a repetition
/// of `\n` admits one only when it may repeat at all, and `fold_hir`
/// descends into `{0}`'s sub-expression without asking.
///
/// Case folding cannot change the answer, since `\n` has no case
/// variants — so reading the un-folded HIR is enough.
fn hir_admits_newline(hir: &Hir) -> bool {
    match hir.kind() {
        HirKind::Literal(lit) => lit.0.contains(&b'\n'),
        HirKind::Class(Class::Unicode(cls)) => cls
            .ranges()
            .iter()
            .any(|r| r.start() <= '\n' && '\n' <= r.end()),
        HirKind::Class(Class::Bytes(cls)) => cls
            .ranges()
            .iter()
            .any(|r| r.start() <= b'\n' && b'\n' <= r.end()),
        HirKind::Repetition(rep) => rep.max != Some(0) && hir_admits_newline(&rep.sub),
        HirKind::Capture(cap) => hir_admits_newline(&cap.sub),
        HirKind::Concat(parts) | HirKind::Alternation(parts) => {
            parts.iter().any(hir_admits_newline)
        }
        HirKind::Empty | HirKind::Look(_) => false,
    }
}

/// Spec 0273 S9's escape hatch: whether the pattern has already decided
/// its own case, and so must not be handed smartcase's default.
///
/// Asking the parser to fold case and getting the same tree back means
/// there was nothing for the fold to do — either the pattern carries no
/// case-foldable literal (`\d+`, `\{`), or it says `(?-i)` and took the
/// decision itself. Reading the raw text for `(?-i)` would instead find
/// it inside `\Q(?-i)\E` and inside a character class.
fn pattern_settles_its_own_case(pattern: &str, hir: &Hir) -> bool {
    regex_syntax::ParserBuilder::new()
        .multi_line(true)
        .dot_matches_new_line(false)
        .case_insensitive(true)
        .build()
        .parse(pattern)
        .is_ok_and(|folded| folded == *hir)
}

/// Spec 0273 S12: whether the pattern anchors to the haystack rather
/// than to a line.
///
/// On a single-row haystack `\A` and `\z` are merely synonyms for `^`
/// and `$`, so refusing them costs the reader nothing today. Under a
/// successor whose haystack is a window they would come to mean its
/// arbitrary edges, and rejecting them now means that successor changes
/// no pattern's meaning.
///
/// The HIR was parsed with `multi_line(true)`, which is what keeps
/// `Look::Start` (`\A`) distinct from `Look::StartLF` (`^`).
fn hir_has_haystack_anchor(hir: &Hir) -> bool {
    fold_hir(hir, &mut |kind| {
        matches!(kind, HirKind::Look(Look::Start | Look::End))
    })
}

/// The literal text a pattern is, when it is one — spec 0273 S7's tier
/// test.
///
/// Taken from the HIR rather than from the raw pattern text, so that
/// `a\.b` searches for `a.b`: escapes work, which they do not today.
fn hir_literal(hir: &Hir) -> Option<String> {
    if !hir.properties().is_literal() {
        return None;
    }
    let mut out = Vec::new();
    collect_literal(hir, &mut out)?;
    String::from_utf8(out).ok()
}

fn collect_literal(hir: &Hir, out: &mut Vec<u8>) -> Option<()> {
    match hir.kind() {
        HirKind::Empty => Some(()),
        HirKind::Literal(lit) => {
            out.extend_from_slice(&lit.0);
            Some(())
        }
        HirKind::Concat(parts) => parts.iter().try_for_each(|p| collect_literal(p, out)),
        // `is_literal()` admits nothing else, so this is unreachable
        // rather than a fallback — but a future HIR shape reaching it
        // must fall out of the tier, not into a wrong needle.
        _ => None,
    }
}

/// Whether `pred` holds anywhere in the pattern's tree.
///
/// Two predicates walk this HIR — smartcase and the anchor test — and
/// each of them is a question about *some* node, so the recursion is
/// written once. [`hir_admits_newline`] is not one of them: it too asks
/// about some node, but it has to refuse to descend into a repetition
/// that cannot repeat.
fn fold_hir(hir: &Hir, pred: &mut impl FnMut(&HirKind) -> bool) -> bool {
    if pred(hir.kind()) {
        return true;
    }
    match hir.kind() {
        HirKind::Repetition(rep) => fold_hir(&rep.sub, pred),
        HirKind::Capture(cap) => fold_hir(&cap.sub, pred),
        HirKind::Concat(parts) | HirKind::Alternation(parts) => {
            parts.iter().any(|p| fold_hir(p, pred))
        }
        _ => false,
    }
}

/// How many *haystack* bytes `needle` covers when `haystack` is
/// lowercased one `char` at a time — or `None` when `haystack` does not
/// begin with it. The caller has already lowercased `needle`.
///
/// Folding as we compare is what keeps `is_match` allocation-free (spec
/// 0195 G4). It goes through `char::to_lowercase`, which yields an
/// iterator rather than a `char`, because a few characters lowercase to
/// more than one (`İ` to `i` plus a combining dot) and a one-to-one fold
/// would silently misalign the comparison from there on.
///
/// The answer is a haystack length rather than the needle's own because
/// the two differ for exactly those characters — which is also why a
/// needle running out mid-expansion still consumes the whole character
/// that produced it: half a `char` is not a length a caller can slice.
fn folded_prefix_len(haystack: &str, needle: &str) -> Option<usize> {
    let mut wanted = needle.chars().peekable();
    let mut consumed = 0;
    for ch in haystack.chars() {
        if wanted.peek().is_none() {
            break;
        }
        for f in ch.to_lowercase() {
            match wanted.peek() {
                None => break,
                Some(&w) if w == f => {
                    wanted.next();
                }
                Some(_) => return None,
            }
        }
        consumed += ch.len_utf8();
    }
    wanted.peek().is_none().then_some(consumed)
}

/// Shared clamp arithmetic behind the override- and manage-pane highlight
/// movement (`move_override_highlight`, `move_manage_highlight`): moves
/// `current` by `delta`, staying within `0..=max`.
fn clamp_highlight(current: usize, delta: isize, max: usize) -> usize {
    (current as isize + delta).clamp(0, max as isize) as usize
}

/// Shared scroll-to-keep-target-visible arithmetic behind the override
/// and manage panes' own render passes: nudges `scroll` by the minimum
/// amount needed to keep `target` within the `height`-row visible window.
/// No-op when `height` is `0`.
///
/// Spec 0244 S8: stated on the signed top, mirroring
/// `clamp_scroll_to_cursor`. A pane may be over-panned, in which case
/// blank rows above the content push the last of its rows past the bottom
/// edge — comparing `target` against `scroll.index` alone would call
/// those rows visible.
///
/// Deliberately a *minimum* nudge (spec 0244 N2): a target that is
/// already on screen moves the viewport not at all, blank rows and all,
/// so an over-pan survives highlight movement within it.
fn clamp_scroll_to_visible(scroll: &mut PaneScroll, target: usize, height: usize) {
    if height == 0 {
        return;
    }
    let top = scroll.top(&FLAT_ROWS);
    let target = target as isize;
    if target < top {
        scroll.set_top(target, &FLAT_ROWS);
    } else if target >= top + height as isize {
        scroll.set_top(target + 1 - height as isize, &FLAT_ROWS);
    }
}

/// Spec 0244 S5: how far a vertical pan may go, in terminal rows measured
/// from the top of content row 0.
///
/// Both ends leave exactly one terminal row of content on screen: the
/// lower bound puts the content's first row on the pane's last row, the
/// upper bound puts its last row on the pane's first. Bounded by the
/// content and nothing else — deliberately *not* by keeping the cursor or
/// highlight row in view, which is `clamp_scroll_to_cursor`/
/// `clamp_scroll_to_visible`'s job alone.
///
/// In terminal rows rather than whole content rows, so that in wire mode
/// (spec 0225 S8) a bound may fall between a document line and its wire
/// row. That is deliberate: `w` holds the cursor's *terminal* row still
/// across the toggle, so a bound rounded to whole lines would be a
/// position `w` can reach and a pan cannot.
///
/// Both ends are clamped against `0`, so a pane with no room and content
/// with no rows both give a bound of `0..=0` rather than an inverted
/// range.
fn pan_top_bounds(content_rows: usize, pane_height: usize) -> (isize, isize) {
    let min_top = (1 - pane_height as isize).min(0);
    let max_top = content_rows.saturating_sub(1) as isize;
    (min_top, max_top)
}

/// Spec 0286 S1: how far a vertical pan may go before it meets the wall,
/// in the same terminal rows as [`pan_top_bounds`].
///
/// The range within which every row the pane draws is content: the lower
/// bound puts the content's first row on the pane's first, the upper
/// bound its last row on the pane's last. `pan_top_bounds` is where a
/// pan may end up *after* pushing through this.
///
/// A document shorter than the pane collapses this to `0..=0`, which is
/// right — such a document has nothing to scroll, so every pan of it is
/// an over-pan and every one of them meets the wall.
///
/// Clamped against `0` at both ends like its sibling, so `min <= max`
/// always holds and no caller's `.clamp()` can panic.
fn natural_top_bounds(content_rows: usize, pane_height: usize) -> (isize, isize) {
    (0, content_rows.saturating_sub(pane_height) as isize)
}

/// The vertical pan the two side panes share: `override_pan_vertical`
/// and `manage_pan_vertical` differ only in which viewport, which wall
/// and which row count they name.
///
/// Simpler than the main pane's `pan_vertical` in one respect that is
/// the whole of the difference: a side pane's rows are one terminal row
/// each, so `step` is already a count of terminal rows and `(step, up)`
/// is already spec 0286's gesture. The main pane has to translate both
/// through `RowHeights`, which is why it does not come through here.
///
/// A free function rather than a method because it takes two of `App`'s
/// fields mutably at once, and returns rather than assigns
/// `event_changed_nothing` for the same reason.
///
/// Returns whether the pan left the screen as it was: it moved nothing
/// (spec 0245 S2) *and* did not change the wall's cue (spec 0286 S7).
fn side_pan_vertical(
    scroll: &mut PaneScroll,
    wall: &mut EdgeResistance,
    total_rows: usize,
    pane_height: usize,
    step: usize,
    up: bool,
) -> bool {
    let top = scroll.top(&FLAT_ROWS);
    let moved = if up {
        top - step as isize
    } else {
        top + step as isize
    };
    let was_pushing = wall.pushing();
    let landed = wall.land(
        (step, up),
        top,
        moved,
        natural_top_bounds(total_rows, pane_height),
        pan_top_bounds(total_rows, pane_height),
        Instant::now(),
    );
    scroll.set_top(landed, &FLAT_ROWS);
    landed == top && was_pushing == wall.pushing()
}

/// Shared pan-by-`step` arithmetic behind the command bar's Shift+wheel/
/// native ScrollLeft/ScrollRight handling in `handle_mouse` — moves
/// `*offset` by `step`, saturating at `0`. `step` is `WHEEL_PAN_STEP`
/// — the wheel always pans by `1`, unlike Ctrl-Left/Ctrl-Right's
/// `PAN_STEP` — for every caller today, since the command bar has no
/// Ctrl-Left/Ctrl-Right binding of its own (it already auto-pans to
/// keep the text cursor in view).
fn pan_by_step(offset: &mut usize, step: usize, left: bool) {
    *offset = if left {
        offset.saturating_sub(step)
    } else {
        offset.saturating_add(step)
    };
}

/// Shared pan-by-`step` arithmetic for the override and manage panes'
/// horizontal Ctrl-Left/Ctrl-Right. `step` is `PAN_STEP` for the keys,
/// `WHEEL_PAN_STEP` for the wheel.
///
/// Like `pan_by_step`, but bounded on the far side by `max_offset` —
/// each pane's own `..._max_visible_line_len().saturating_sub(width)`,
/// so it stops once the rightmost character of the widest
/// currently-visible row would be shown and never further, as the main
/// pane's `pan_right` does.
///
/// The vertical pans went through here too until spec 0244 S7, which
/// gave all three panes one signed bound (`pan_top_bounds`) that this
/// unsigned offset cannot express.
fn pan_by_step_clamped(offset: &mut usize, max_offset: usize, step: usize, backward: bool) {
    *offset = if backward {
        offset.saturating_sub(step)
    } else {
        (*offset + step).min(max_offset)
    };
}

/// Local-statusline style for a focus-tracked pane (main/override/
/// manage, spec 0147 G2): `theme::focus_style`'s bold accent color when
/// focused, `theme::unfocused_pane_style`'s plain accent otherwise — the
/// visual language protolens uses throughout for "this pane currently
/// holds keyboard focus," applied to the full width of each pane's own
/// `Length(1)` statusline row (`render`'s main-pane statusline,
/// `render_override_pane`, `render_manage_pane`) — mirroring vim's own
/// active/inactive statusline highlight groups rather than tinting a
/// border.
fn pane_focus_style(focused: bool, theme: ThemeKind) -> Style {
    if focused {
        theme::focus_style(theme)
    } else {
        theme::unfocused_pane_style()
    }
}

/// Spec 0147 G2: compose a pane's local statusline as `left` flush-left
/// and `right` flush-right (if any) within `width` columns — mirroring
/// vim's own statusline layout. `right` is always shown in full, never
/// truncated.
///
/// Spec 0193 S3: when `left` would overlap `right`, what survives is
/// `left`'s **tail**, marked with a single leading `<` (vim's `%<`
/// again, but at the other end). `left` reads
/// `<blob path> <node path>: <type>`, longest-lived field first, so its
/// head is the part a user has already read and its tail is the part
/// that changes as the cursor moves.
fn statusline_text(left: &str, right: Option<&str>, width: usize) -> String {
    let Some(right) = right else {
        return left.chars().take(width).collect();
    };
    let right: String = right.chars().take(width).collect();
    let budget = width.saturating_sub(right.chars().count());
    if budget == 0 {
        return right;
    }
    let left_chars: Vec<char> = left.chars().collect();
    if left_chars.len() <= budget {
        let pad = budget - left_chars.len();
        let mut line: String = left_chars.into_iter().collect();
        line.push_str(&" ".repeat(pad));
        line.push_str(&right);
        line
    } else {
        let dropped = left_chars.len() - (budget - 1);
        let mut line = String::from("<");
        line.extend(left_chars.into_iter().skip(dropped));
        line.push_str(&right);
        line
    }
}

/// Spec 0286 S6: a composed statusline as a drawable line, with its
/// viewport label picked out in the edge-resistance accent while the end
/// of the content is being pushed against.
///
/// The label is matched as a **suffix** rather than searched for. It is
/// the last thing in the ruler, which is the last thing in the right
/// half, in every branch `statusline_text` can take — including both of
/// its truncating ones. A label that is not there was truncated away
/// with the rest of the ruler, and there is nothing left to color.
///
/// The bar is `REVERSED` (`theme::focus_style`), so the accent given as
/// a foreground is what paints the span's visible *background*: the
/// label becomes a colored block rather than colored glyphs.
fn statusline_line(
    text: String,
    label: Option<&str>,
    style: Style,
    pushing: bool,
) -> Line<'static> {
    let accented = match (pushing, label) {
        (true, Some(label)) => text
            .strip_suffix(label)
            .map(|head| (head.to_string(), label)),
        _ => None,
    };
    match accented {
        Some((head, label)) => Line::from(vec![
            Span::styled(head, style),
            Span::styled(label.to_string(), style.fg(theme::edge_resistance_color())),
        ]),
        None => Line::styled(text, style),
    }
}

/// Spec 0193 S4: vim's viewport indicator, to be shown *alongside* the
/// cursor's own `L<n>/<m>` rather than instead of it (A6) — it answers
/// "where is the window", which panning changes and the cursor ruler
/// cannot report.
///
/// `All` when everything fits, `Top`/`Bot` at the ends, and otherwise
/// the window's position as a percentage of the scrollable range — not
/// of the document, so the last scroll position before `Bot` reads as a
/// high percentage rather than as `total - height`'s fraction. This is
/// vim's own arithmetic.
///
/// Spec 0244 S10: `top` is the pane's signed top edge, not a floored row
/// index, and `All` means both ends are on screen rather than merely
/// `total <= height`. A pane may now be over-panned, and a short document
/// panned until only its last row shows still satisfies `total <= height`
/// — which would have reported `All` while most of it was off screen.
/// `top`, `height` and `total` are in whatever row unit the caller
/// counts in, so long as it is the same one for all three.
fn viewport_label(top: isize, height: usize, total: usize) -> String {
    let (height, total) = (height as isize, total as isize);
    let bottom_shown = top + height >= total;
    match (top <= 0, bottom_shown) {
        (true, true) => "All".to_string(),
        (true, false) => "Top".to_string(),
        (false, true) => "Bot".to_string(),
        // Reachable only with `0 < top < total - height`, so the
        // denominator is positive and the quotient lands in `1..100`.
        (false, false) => format!("{}%", top * 100 / (total - height)),
    }
}

/// Resolve a typed command `name` against `COMMANDS`, with **exact match
/// always winning over prefix ambiguity** (spec 0114 §7) — matching vim's
/// own `:command` abbreviation convention and `argparse`'s prefix-matching:
/// a command's full name always resolves to itself even when it's also a
/// prefix of another, longer command name.
fn resolve_command(name: &str) -> Result<&'static str, String> {
    if let Some(&exact) = COMMANDS.iter().find(|&&c| c == name) {
        return Ok(exact);
    }
    match complete_prefix(name, COMMANDS.iter().copied()).as_slice() {
        [] => Err(format!("unknown command: {name}")),
        [only] => Ok(*only),
        many => Err(format!("ambiguous command '{name}': {}", many.join(", "))),
    }
}

/// Active Tab-completion cycle state (spec 0113 D26) — `Some` only while
/// consecutive `Tab`/`Shift-Tab` presses are cycling through a candidate
/// list for the same token; any other key clears it (`handle_command_key`).
struct CompletionState {
    /// Char index into `command_buffer` where the completed token begins.
    token_start: usize,
    /// Text originally following the token (preserved verbatim across
    /// cycling, so repeated `Tab` presses don't drift the rest of the
    /// buffer — today always empty, since only the first token, typed at
    /// the buffer's end, is completed).
    suffix: String,
    candidates: Vec<String>,
    /// `None`: showing the longest-common-prefix, no specific candidate
    /// selected yet. `Some(i)`: cycling, currently showing `candidates[i]`.
    index: Option<usize>,
}

/// Search direction for the override pane's in-pane candidate search (spec
/// 0114 §4), vim-style `/` (forward) / `?` (backward).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SearchDir {
    Forward,
    Backward,
}

impl SearchDir {
    /// The opposite direction — `N` repeats the last search the other
    /// way, exactly as vim's `N` is the counterpart to its `n`. Before
    /// spec 0195 this was bound to `p`, which in vim is *put*.
    fn reverse(self) -> Self {
        match self {
            SearchDir::Forward => SearchDir::Backward,
            SearchDir::Backward => SearchDir::Forward,
        }
    }
}

/// One row of the override management pane's grouped-by-origin listing:
/// an origin's own (unindented, non-selectable) header row, or one of its
/// candidate types (an index into `overrides.entries()`, indented under
/// the header — spec 0117 §3 amendment: origin kind is dropped from the
/// display since it's implicit from the origin's own format, and each
/// origin's types are grouped under a dedicated header line instead of
/// repeating the origin on every row).
enum ManageRow {
    Header(String),
    Entry(usize),
}

/// What the shared `command_buffer`/`command_cursor` text-entry state
/// currently represents (spec 0114 §4, extended to the main pane, override
/// pane, and management pane): a `:`/`x`-triggered ex-command, or a `/`/`?`
/// search pattern. They differ only in how `Enter` is interpreted and
/// whether Tab-completion applies — `Search`'s direction doubles as the
/// direction the pattern was originally requested in. Which pane's search
/// a confirmed `Search` pattern actually runs against is determined at
/// `Enter`-time from `self.override_focus`/`self.manage_focus`, not
/// carried in this enum.
///
/// Spec 0276 S1: `find` tells a `/`/`?` prompt from an `F`/`B` one. The
/// two differ in two keys — `Enter` steps to the next match instead of
/// committing, `Esc` accepts instead of cancelling — and in nothing
/// else, so it rides here as a field rather than as a fourth variant:
/// a variant would leave every existing `matches!(…, Search(_))` test
/// (the history browse, the incremental restart, the pattern tint)
/// quietly answering "no" for the new prompt, while widening this one
/// makes the compiler name every site. `command_kind` is set by
/// `open_command_line` on every prompt, so the mode cannot leak from
/// one to the next.
///
/// Spec 0281 S1 widens that field from a `bool` to carry the find's
/// **default** direction — the one its opening key named — while `dir`
/// becomes the **active** one, where the next `Enter` will step.
/// `Shift-→`/`Shift-←` are the only readers of `find`'s payload;
/// everything downstream (the prefix character, `restart_search_sweep`,
/// `accept_find`'s echo) reads `dir` and is right about "active"
/// without change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandLineKind {
    Command,
    Search {
        /// Where the next step goes — and at a `/`/`?` prompt, what
        /// `Enter` will commit.
        dir: SearchDir,
        /// `Some(d)` for an `F`/`B` prompt opened by the key naming
        /// `d`; `None` for a committing one.
        find: Option<SearchDir>,
    },
}

impl CommandLineKind {
    /// A `/`/`?` prompt — the committing one.
    fn search(dir: SearchDir) -> Self {
        Self::Search { dir, find: None }
    }
}

/// Export-chord leader state (spec 0156 G3): `None` (no chord armed),
/// `Leader` (a lone `x` was just pressed), `Descriptor` (`xd` was just
/// pressed — one more key selects binary vs. prototext).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExportChord {
    None,
    Leader,
    Descriptor,
}

/// Spec 0185: the override selection pane's live preview, as a block of
/// rendered lines standing in for the target's committed rows at draw
/// time. It is *not* part of the document: `tree` and `lines` are both
/// untouched by it, which is what makes a preview cost only the
/// (byte-bounded) decode and render of the target's own interior.
///
/// `first_row`/`covered_rows` are visible-row positions, computed once
/// when the overlay is built. They stay valid because S5 locks focus to
/// the selection pane for the overlay's whole lifetime: the only two
/// things that move a row — folding and splicing — are unreachable
/// while it is up. A terminal resize changes the pane height only, not
/// the row numbering, so it is harmless.
pub(super) struct PreviewOverlay {
    /// The first visible row the committed target's lines cover. For a
    /// packed run that target is the run's leader and the range is the
    /// whole run's (spec 0184: the record is the addressable unit),
    /// since that is what a commit would splice.
    first_row: usize,
    /// How many visible rows those lines cover.
    covered_rows: usize,
    lines: Vec<String>,
    /// Spec 0225 S9: the spans the same render produced, kept so an
    /// overlay row can draw its own wire row. Display data, exactly like
    /// `lines` — they never enter `tree`, so the overlay stays
    /// unselectable, unfoldable and unaddressable (spec 0185 N6).
    spans: Vec<NodeSpan>,
    /// The bytes `spans` index into. A preview's interior may have been
    /// cut to the byte budget (spec 0174), so these are not a sub-slice
    /// of the blob and cannot be re-derived from one.
    bytes: Vec<u8>,
    /// Spec 0318 S5/S6: how much of the node these lines are, and the
    /// column the bar saying so is drawn in.
    ///
    /// The column is `marker_column(&lines[0])` — the same function
    /// `mouse.rs`'s fold hit test uses — so the bar sits exactly where
    /// the node's own fold triangle would, and the two cannot drift
    /// apart. The column is free on every row of the overlay by
    /// construction: `display_row_source` gives an overlay row no owner,
    /// so an overlay row has no fold marker at any depth (S4), whatever
    /// `--indent` is set to.
    tier: preview_truncate::PreviewTier,
    tier_column: usize,
    /// Spec 0328 S6: which of `lines` carries the `...` saying the
    /// preview stops here — the last *content* row, above the node's
    /// closing brace, since `} ...` would say something follows the
    /// node and what was withheld is inside it. `None` when `tier` is
    /// `Whole` and nothing was withheld.
    ///
    /// Decided here rather than in the renderer so that the row is one
    /// index compare per drawn row, not a second reading of `lines`.
    ellipsis_row: Option<usize>,
}

/// Spec 0185 S2: one row of the main pane as actually drawn — either a
/// line of the committed document or a line of the preview overlay
/// standing in for it. Overlay rows have no node, hence no heat cue, no
/// override hint, no fold marker and no selection (S4).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DisplayRow {
    Committed(CommittedRow),
    Overlay(usize),
}

impl DisplayRow {
    /// The absolute line a committed row draws, `None` for an overlay
    /// row.
    ///
    /// Spec 0222 S3: `CommittedRow` carries two derived fields beside
    /// the line, so callers that mean "is this the row for line n"
    /// compare through this rather than building a whole row to compare
    /// against — building one costs a `line_pos` descent they do not
    /// otherwise need.
    pub(super) fn committed_line(self) -> Option<usize> {
        match self {
            DisplayRow::Committed(c) => Some(c.line),
            DisplayRow::Overlay(_) => None,
        }
    }
}

/// Spec 0222 S3: one committed row, as the window walk already knows it.
///
/// `build_window` descends once and then walks, and every row it visits
/// arrives with its owning node in hand. Carrying that here is what lets
/// the passes downstream — the text, the fold marker, the spans, the
/// override hint — stop searching for an owner the walk had already
/// found.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct CommittedRow {
    /// The absolute line number, still needed by the drag selection,
    /// mouse hit-testing and the heat cue, all of which are keyed on it.
    pub(super) line: usize,
    /// Which node owns the line, and which of that node's own lines it
    /// is.
    pub(super) pos: LinePos,
    /// Spec 0222 S4: the byte offset of this row's text within
    /// `node_text[pos.node]`. Zero for every row but the second and
    /// later elements of a packed run, where it is what keeps a frame
    /// from re-scanning the run once per drawn row.
    pub(super) offset: usize,
}

/// Spec 0199 S1: whether the caret's position at an end of its row was
/// chosen or merely arrived at.
///
/// The caret reaches an end two ways that look identical on screen and
/// mean opposite things: the user put it there (`^`, `$`, or walking
/// into it), or a vertical move clamped a longer desired column into a
/// shorter row. Keys that act on the tree at an end — `h` folding, `l`
/// descending — must fire only for the first, or passing over a short
/// row and pressing an arrow folds a node nobody asked about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum CaretAnchor {
    /// In the middle of the row, or pushed onto an end by a clamp.
    Free,
    /// Deliberately on the row's first non-blank character.
    Home,
    /// Deliberately on the row's last reachable column.
    End,
}

/// Spec 0194 S10: a whole cursor position — the node, which of its own
/// lines the cursor rests on (spec 0142's half-coordinate, widened by
/// spec 0216 S7), and the caret's column within that line. What the
/// jumplist stores, so that `Ctrl-o` returns to a *place* rather than
/// to a node.
///
/// Deliberately does **not** carry the caret anchor (spec 0199 S10): it
/// records where the cursor was, and the anchor is not part of where.
///
/// Spec 0242 S1 reuses it for `select_anchor`, which wants the same
/// three numbers for the same reason — a place that survives a fold.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct CursorPos {
    pub(super) node: usize,
    pub(super) line_in_node: u32,
    pub(super) column: usize,
}

/// Owns all cursor/fold/scroll/jumplist state — kept separate from
/// `render()`'s drawing calls (spec 0111 §4, ratatui testability pattern).
pub struct App {
    /// Wrapped blob actually decoded (spec 0114 §1.1) — needed for binary
    /// extraction (`ExtractFormat::Binary` slices `NodeSpan::raw_range`
    /// from this, since every `raw_range` is relative to *this* blob, not
    /// the caller's original one).
    ///
    /// Shared with the heat worker rather than cloned into it (spec 0216
    /// S28) — and shareable at all, which a mapped blob would not be if
    /// this owned its bytes.
    blob: Arc<Blob>,
    /// Width in bytes of the wrapper's own tag+length prefix (spec 0114
    /// §1.1) — subtracted from any displayed `raw_range` coordinate to
    /// recover the caller's original (pre-wrap) numbering.
    wrapper_offset: usize,
    /// Original blob's own path — basis for `default_extract_path()`'s
    /// proposed `:export`/`x` default path.
    blob_path: PathBuf,
    /// Whether the main pane currently shows each line's trailing `#@ ...`
    /// annotation (spec 0133) — a pure *display* attribute, toggled by the
    /// `a` key, decoupled from the underlying `lines`, which always carry
    /// full annotations regardless of this flag.
    annotations: bool,
    /// Spec 0225 S1: which drawn document lines are followed by a second
    /// terminal row showing that line's own bytes in hex.
    ///
    /// Spec 0268 S1 narrowed it from a `bool` to the one span the reader
    /// last asked for; `None` is "no bytes anywhere". Written by `w` and
    /// `W` and — exactly like `annotations` — a pure *display*
    /// attribute: no line count, no `LinePos`, no fold, search or export
    /// knows it exists. It does change the pane's geometry, which is
    /// what `row_heights` answers for.
    wire: Option<WireSpan>,
    /// `wire` resolved to a visible-row range, against the
    /// `structural_version` it was resolved under.
    ///
    /// A fold and a splice both bump `structural_version`; a `w` or `W`
    /// clears this itself, being the only other thing that can move the
    /// answer. `RefCell` rather than `Cell` because a `Range` is not
    /// `Copy`; neither borrow is held across a call.
    wire_rows: std::cell::RefCell<Option<WireRowCache>>,
    /// Spec 0328 S2: where the current node's bar runs, against the
    /// `structural_version` and the caret node it was derived under.
    ///
    /// The range costs an `absolute_start` climb, which sums the line
    /// counts of every preceding sibling at every level, and each row of
    /// the viewport would otherwise ask the same question. Cached on the
    /// same terms `wire_rows` is, plus the caret: moving the caret
    /// changes the answer without changing the document's shape.
    cursor_bar: std::cell::RefCell<Option<render::CursorBarCache>>,
    /// Spec 0273 S10: the last pattern text compiled, and what it
    /// compiled to.
    ///
    /// `search_highlight_pattern` is called from `render` on every
    /// frame, and after spec 0273 building a `SearchPattern` is a regex
    /// compile rather than a `String` clone. One entry is enough — the
    /// caller asks about one pattern, over and over.
    search_compiled: std::cell::RefCell<Option<(String, Rc<SearchPattern>)>>,
    /// Indentation step (spaces per nesting level) this session was decoded
    /// with — reused by `apply_override` (spec 0114 §5) so a splice
    /// re-render matches the rest of the document's own indentation.
    indent_size: usize,
    /// Spec 0222 S1: the text of the lines each arena slot draws
    /// *itself*, its children's excluded, newline-separated and with no
    /// trailing newline. `None` for a slot this interpretation does not
    /// render.
    ///
    /// One entry per slot, parallel to `tree` and `heat_states`. A
    /// bracketed node holds its header line alone — its footer is
    /// `indent + "}"` and is derived from that header (S2) rather than
    /// stored. A flat node holds all of its own rows, which is one for
    /// an ordinary scalar and one per element for a packed run.
    ///
    /// It replaces a `Vec<String>` of the whole document, whose real
    /// cost was not its 126.7 MB of headers but the O(document) memmove
    /// every commit paid to keep absolute line numbers meaning
    /// something. Nothing here is positional, so a splice writes the
    /// slots it re-rendered and stops.
    ///
    /// Spec 0274 S8: shared rather than owned, because a segment scan
    /// reads it from a worker thread. Written through
    /// [`App::node_text_mut`], never directly.
    node_text: Arc<Vec<Option<Box<str>>>>,
    /// Spec 0187 S3: syntax hints for the rows drawn by the *current*
    /// frame's `window`, in window order — recomputed each `render()`,
    /// never retained across frames, never document-sized. Index `i` is
    /// `window[i]`'s.
    ///
    /// It replaces a `Vec<LineStyles>` parallel to `lines`, which cost
    /// one whole-document tree-sitter parse per load and per commit
    /// (85% of a commit, measured) to produce data of which only the
    /// ~50 rows on screen were ever read.
    window_styles: Vec<LineStyles>,
    /// Spec 0245 S3: the `(window text, indent_size)` that produced the
    /// current `window_styles`, or `None` when they were discarded.
    ///
    /// `window_styles_for` is a pure function of exactly these two, so
    /// they are the complete validity key: while it holds, the hints
    /// are current rather than stale, and the frame neither recomputes
    /// them nor drops them — which is what keeps a burst of input that
    /// moves nothing from flickering the viewport monochrome.
    window_styles_key: Option<(Vec<String>, usize)>,
    /// Resolved color theme (spec 0116 §9) — fixed for the session, never
    /// `ThemeKind::System` (resolved once in `main.rs` before `App::new`).
    theme: ThemeKind,
    /// Spec 0274 S8: shared for the same reason `node_text` is — the
    /// scan walks the rendered shape as well as reading the text — and
    /// written through [`App::tree_mut`], never directly.
    tree: Arc<Vec<TreeNode>>,
    /// The blob's structural decomposition (spec 0216 S1), built once at
    /// load and never rebuilt: it is a function of the bytes, and the
    /// bytes do not change. Every interpretation's `tree` is a pruning of
    /// it, which is what lets it be immutable while `tree` is not.
    ///
    /// Shared with a segment scan too, and needing no `make_mut` twin:
    /// there is no writer at all.
    arena: Arc<Arena>,
    /// Spec 0212 S4: the type names every `span.type_fqdn` in `tree`
    /// indexes into, moved out of `Decoded` at construction and shared by
    /// every sub-render of this document for the session's whole life.
    ///
    /// Reach it as `&mut self.fqdns` rather than through a `&mut self`
    /// helper: a method borrows all of `App`, which conflicts with the
    /// `&self.tree` and `&mut self.render_cache` borrows live at the
    /// splice sites that need to intern. Direct field access lets the
    /// borrow checker see the fields are disjoint.
    fqdns: FqdnTable,
    /// Spec 0213: the provenances every `TreeNode::rendered_as` in `tree`
    /// indexes into. Unlike `fqdns` this one is protolens's own and is
    /// never handed to anything — a freshly built node is always
    /// `NOT_RENDERED`, so only the render pass ever interns.
    ///
    /// Reach it as `&mut self.provenance`, for the same disjoint-borrow
    /// reason `fqdns` gives.
    provenance: ProvenanceTable,
    /// Spec 0160 G1: whether a `render_overrides` batch is currently
    /// active — `0` outside of one, `1` while the outer call runs.
    /// `splice_override` reads it to decide whether to self-finalize
    /// immediately (a standalone call) or defer to the batch's finalize.
    /// Production has exactly one `splice_override` caller and it is
    /// always inside a batch, so only tests reach the standalone path.
    ///
    /// `render_overrides_inner`'s self-recursion bypasses the counted
    /// `render_overrides` wrapper, so this only toggles `0`/`1` and
    /// never nests deeper.
    override_batch_depth: u32,
    /// Spec 0183 S2: the descent mark. `descend[i]` is `true` when node
    /// `i` either needs resettling itself or has a descendant that
    /// does, making `render_overrides_inner`'s child gate a single `Vec`
    /// read rather than a blanket descent into every interior node.
    ///
    /// Extended by `compute_descend_marks` at the start of each batch
    /// (when `override_batch_depth` goes `0` -> `1`) and by
    /// `mark_fresh_subtree` after each splice within one. Indices are
    /// arena indices, which is sound because the arena is built once
    /// from the bytes and never mutated (spec 0216) — a splice rewrites
    /// only per-slot overlay state, so no index ever moves.
    ///
    /// Spec 0188 S4: **monotone — never cleared**, and its length
    /// doubles as the watermark for how much of the arena has already
    /// been examined. Rebuilding it per batch meant a scan of every
    /// node in the arena (13.5% of a nested commit at 382 k nodes, and
    /// growing with the arena rather than with the document); keeping
    /// it means each node is examined once, when it first appears.
    ///
    /// Keeping a mark that a later batch would not re-derive is safe
    /// in a way that dropping one is not: a stale mark costs one
    /// wasted descent into a node that turns out to need nothing,
    /// while a missing mark is silent staleness — the node is never
    /// revisited and keeps the text it was last rendered with (spec
    /// 0183 L3). And a mark is only ever stale in the first place if
    /// the override entry that produced it went away, in which case
    /// the node it names was spliced and so carries `rendered_as`,
    /// which would have marked it permanently regardless.
    descend: Vec<bool>,
    /// Test-only escape hatch that widens the recursion gate to
    /// `is_message || is_auto_expand_candidate || <has an active
    /// override> || rendered_as.is_some()`, so a pruned render can be
    /// compared against an unpruned one.
    ///
    /// It exists because pruning fails silently: a subtree the walk
    /// wrongly skipped keeps whatever text it was last rendered with,
    /// with no panic and no bad index, so nothing weaker than byte
    /// equality against the unpruned walk is worth asserting.
    #[cfg(test)]
    pub(super) unpruned_walk: bool,
    /// Spec 0186 G3: whether `finalize_override_batch` walks the whole
    /// document afterwards (`assert_line_counts_are_exact`) to confirm
    /// every node's stored line count still matches the text. Defaults
    /// to `true` so that every splice in the suite is a case without
    /// opting in — the opposite of `unpruned_walk` above, because here
    /// the checked path is the production one and the check is what must
    /// be universal.
    ///
    /// The profiling harness turns it off: the check is the very
    /// O(document) walk the incremental repair exists to avoid, so
    /// leaving it on would have the harness measure both.
    #[cfg(test)]
    pub(super) verify_repair: bool,
    /// Whether any `splice_override` in the current `render_overrides`
    /// batch actually re-rendered something. Always `false` outside of
    /// an active batch.
    ///
    /// Spec 0222 S5: a splice writes its own slots' text and nothing
    /// else, so there is no deferred buffer work left for
    /// `finalize_override_batch` to do — only the bookkeeping that a
    /// batch which changed nothing must still skip, chiefly bumping
    /// `structural_version` and so restarting the read-ahead walk.
    batch_spliced: bool,
    cursor: usize,
    /// Which of `cursor`'s own lines the cursor is visually resting on
    /// — the `line_in_node` half of a [`LinePos`].
    ///
    /// `0` is the header row. For a bracketed node the only other
    /// value is `lines_total - 1`, its closing `}` (spec 0142); for a
    /// flat node the values run over its rows, which is one per element
    /// of a packed run (spec 0216 S7).
    ///
    /// `cursor` is still the node the row belongs to, so every
    /// node-indexed action (fold, override edit, status line) treats a
    /// non-header row exactly like the header — "acts as if on the
    /// opening bracket" needs no extra redirection.
    cursor_line_in_node: u32,
    /// Spec 0194 S1: where the caret rests along the cursor row's
    /// *caret track* — the row's own text (`row_text`, fold margin
    /// excluded) followed by its heat suffix, counted in `char`s.
    ///
    /// Deliberately neither a screen column nor an offset into
    /// `row_content`: the fold gutter's width and the horizontal pan are
    /// display transforms `render` already applies, and folding them in
    /// here would make the caret slide across characters whenever the
    /// user pans (spec 0194 A4).
    cursor_column: usize,
    /// Spec 0194 S5: the column vertical movement tries to return to.
    /// Set by every *horizontal* move and left alone by every vertical
    /// one, so crossing a short row and coming back restores the
    /// original column — vim's rule.
    desired_column: usize,
    /// Spec 0199 S1: whether the caret's position at an end of its row
    /// was *chosen* or merely arrived at.
    ///
    /// Distinct from `desired_column`, and not derivable from it:
    /// `carry_caret`'s `Home` arm pins `cursor_column` to each row's
    /// first non-blank while leaving `desired_column` at its pre-move
    /// value, so the two disagree in exactly the case where the position
    /// is voluntary (S2).
    caret_anchor: CaretAnchor,
    /// Spec 0194 S1/S3: how many characters the heat suffix contributed
    /// to the cursor row's caret track in the last frame drawn, which is
    /// how far right of the text `l`/`$` may go.
    ///
    /// Read off the frame rather than recomputed on demand because
    /// resolving a row's cue is `&mut self` (it populates the heat
    /// caches and can enqueue work), and a keypress must not do that.
    /// The text half of the range is computed exactly, every time.
    caret_suffix_len: usize,
    /// Incremented every time `self.cursor` changes (via `set_cursor`),
    /// regardless of whether the new value differs from any prior one —
    /// a real "did the cursor move since X" signal: comparing `self.
    /// cursor`'s current value against a stashed old value alone misses
    /// a round trip (e.g. Down then Up) that leaves the position
    /// numerically unchanged but is still a real move.
    cursor_moves: u64,
    /// Spec 0242 S1: the **fixed** end of the main-pane selection —
    /// where the drag started, or where the caret was when the first
    /// `Shift`-motion was pressed. `None` when nothing is selected.
    ///
    /// The **moving** end is the caret itself (S2), which is why there
    /// is no second field: the mouse and the `Shift` keys both express
    /// a selection by moving the caret, so they cannot disagree about
    /// where it ends.
    ///
    /// A `CursorPos` because `cursor_pos()` already builds exactly this
    /// — the anchor *is* a remembered caret position — and because a
    /// node-relative line survives a fold, which an absolute line number
    /// would not.
    select_anchor: Option<CursorPos>,
    /// The selection's moving end — the position that `Shift`-motions,
    /// drags, and `script_apply_select` write to when they extend or set
    /// the selection. `selection_span` reads this instead of
    /// `cursor_column` so that plain caret motions do not silently shrink
    /// or grow the span.
    ///
    /// `None` when no selection is engaged (mirrors `select_anchor`).
    /// Set to `cursor_pos()` by every gesture that extends the selection,
    /// and cleared by `clear_selection`.
    select_caret: Option<CursorPos>,
    /// Spec 0242 S2: whether the user has actually *expressed* a
    /// selection since the anchor was set — a `Shift`-motion, a drag, or
    /// a double-click — as opposed to a bare click, which arms the
    /// anchor so that a following drag has somewhere to start from but
    /// selects nothing on its own.
    ///
    /// This is what lets anchor-equal-to-caret mean **one character**
    /// rather than none. Both endpoint cells are in the span, so once a
    /// selection is engaged the caret resting on the anchor is a
    /// perfectly good one-character selection (`Shift-Right`
    /// `Shift-Left` is how the keyboard spells it); it is only the
    /// *unengaged* click that must select nothing.
    select_engaged: bool,
    /// Timestamp + `(line_idx, zone)` of the most recent main-pane
    /// left-click `Down` event that landed on a line — compared against
    /// on the next such `Down` to recognize a double-click (same line
    /// *and* same zone, within `DOUBLE_CLICK_THRESHOLD`). `None` before
    /// the first click.
    ///
    /// The zone is part of the key (spec 0284 S6) because the two zones'
    /// double-clicks do different things: a click on the text followed
    /// by a click on the heat cue beside it is two single clicks, not a
    /// pair that has to choose between them.
    ///
    /// A click on the fold field is deliberately not tracked here and
    /// clears it: the fold toggle is a control, and a control must act
    /// on every click that reaches it, never differently for being the
    /// n-th of a fast run. The heat cue is a control too but acts only
    /// on the *second* of a pair, so it pairs normally (0284 S7).
    last_click: Option<(Instant, (usize, ClickZone))>,
    /// The zone of the click currently in progress (`Down` already
    /// handled, matching `Up` not yet seen) when it was recognized as
    /// the second click of a double-click — consulted by the `Up`
    /// handler to decide what the gesture meant. `None` for a plain
    /// (non-dragged) click, which deselects.
    ///
    /// Recorded rather than re-derived at `Up`, because the gesture was
    /// decided at the second `Down`: a release that moved is a `Drag`,
    /// and hit-testing the release position would let one gesture start
    /// on a control and finish somewhere else.
    pending_double_click: Option<ClickZone>,
    /// The reader's fold intent: a member is a node the reader has not
    /// asked to see inside (spec 0332 S2). Never written by the *bake*,
    /// which is the whole point of it being separate from `auto_folded`
    /// (spec 0249 S3).
    ///
    /// **It is born full** — spec 0338 S1, `FoldSet::full(arena.len())`
    /// in `build_tree`. "A document opens closed" (spec 0323) is that
    /// initial value and not a pass that writes it out slot by slot, and
    /// three consequences follow that are easy to get wrong:
    ///
    /// - The set is *total*: every arena slot already has an answer
    ///   before any render reaches it, so no code downstream has to
    ///   invent a default for a slot it has not seen. That totality is
    ///   what lets a splice leave the set entirely alone (S2) — an
    ///   override is not a fold gesture.
    /// - Membership is therefore **wider than foldability**: a scalar
    ///   leaf is a member too. Excluding leaves would mean asking every
    ///   slot whether it is foldable, which is exactly the arena-wide
    ///   walk S1 exists to delete. [`App::is_folded`] gates the *read*
    ///   on [`App::is_foldable`] instead, so a leaf's bit is never
    ///   consulted. Read-gating, not write-gating.
    /// - `len()` and `iter()` therefore count and yield leaves. Nothing
    ///   in the running program asks; a test or a debug dump that does
    ///   must filter by [`App::is_foldable`] itself.
    ///
    /// Read-gating is also what keeps this honest across a splice, since
    /// foldability is *not* constant: an override can make an empty
    /// message bracketed that was not before. Under a materialized set
    /// that slot would carry no bit and draw open; under a full set it
    /// carries one and draws closed, which is what spec 0323 promises.
    user_folded: FoldSet,
    /// Nodes folded because their body has not been rendered — the ones
    /// a row-bounded render stopped at (spec 0249 S1/S3).
    ///
    /// Separate from `user_folded` in both directions. A bake clears only
    /// this set, so a fold the user made never pops open by itself; and
    /// a fold the user made over a baked node is not silently undone by
    /// a later bake finishing. Reads do not care which set a node is in
    /// — that is what [`App::is_folded`] is for — only writes do.
    auto_folded: FoldSet,
    /// The order the bake works through `auto_folded` in (spec 0255 S3).
    ///
    /// A hint, not the truth: `auto_folded` decides whether a node still
    /// owes a body, and a pop whose node has left that set is discarded.
    /// That is what makes a duplicate entry, a node the user expanded by
    /// hand, and a node whose ancestor was re-overridden underneath it
    /// all harmless without a generation counter.
    ///
    /// It exists because the set alone cannot name a next element
    /// cheaply — `HashSet::iter().next()` scans buckets from the top,
    /// and a drain empties ~84 000 entries without shrinking the table,
    /// so the last steps would each scan all of it.
    bake_queue: VecDeque<usize>,
    /// The stops the last drawn frame actually put on screen (spec 0249
    /// S8's scroll half), in row order — what the bake works through
    /// *before* `bake_queue`.
    ///
    /// Written only by `render_main_pane`, because the frame is the one
    /// place that knows what is visible and it has already built the
    /// window to draw it. That also makes this self-correcting: a jump
    /// re-aims the bake on the very next frame, and no gesture has to
    /// remember to.
    ///
    /// A hint in the same sense `bake_queue` is, and discarded by the
    /// same `auto_folded` check.
    visible_stops: VecDeque<usize>,
    /// Spec 0255 S2: whether a confirm renders a screenful or the whole
    /// subtree. Set by `run_loop` and by nothing else, so `App::new`'s
    /// own startup pass and every headless export stay unbounded (N6).
    ///
    /// Spec 0256 S3 reads it for a second meaning that happens to be the
    /// same fact: an event loop is running, so there is somewhere to
    /// defer work to.
    bounded_confirms: bool,
    /// The previous interpretation's text, moved aside by
    /// `splice_override` rather than dropped, and freed by `run_loop`'s
    /// idle arm a chunk at a time (spec 0256 S1/S2). Never non-empty
    /// without `bounded_confirms`.
    discarded_text: Vec<Box<str>>,
    /// The main pane's vertical viewport (specs 0230, 0244 S2): the first
    /// document line drawn, plus the signed terminal-row remainder that
    /// lets a `w` toggle hold a row still. `scroll.index` counts document
    /// lines, which in wire mode are two terminal rows thick.
    scroll: PaneScroll,
    /// Spec 0286: the wall at either end of the content that `scroll`
    /// must be pushed through to over-pan. One per pannable pane —
    /// `override_resistance` and `manage_resistance` are the other two,
    /// and each is settled by every input event whichever pane it was
    /// aimed at.
    scroll_resistance: EdgeResistance,
    /// Spec 0259 S1: `scroll`, as of the last frame, said in nodes
    /// instead of in row numbers — so that a splice, which renumbers
    /// every row below it, can put the top of the pane back where the
    /// reader last saw it. `None` before the first frame and while a
    /// preview overlay is held (S5).
    scroll_anchor: Option<pane_scroll::ScrollAnchor>,
    /// Spec 0329 S3: the anchor a commit takes on its own primary
    /// target, consumed by the `finalize_override_batch` that closes
    /// the batch which took it.
    ///
    /// Separate from `scroll_anchor` because the two have different
    /// lifetimes: 0259's is a standing fact about the frame last drawn
    /// and is re-taken by every frame, while this one is about one
    /// batch and would otherwise be read — with geometry that has since
    /// moved — by whatever splice came along next.
    target_anchor: Option<pane_scroll::ScrollAnchor>,
    /// `cursor_display_row()`'s value as of the last render pass that
    /// applied `clamp_scroll_to_visible` to `scroll` — compared
    /// against the *current* row at the top of every render, so the
    /// auto-pan-into-view only fires on genuine cursor movement, not on
    /// every frame regardless of cause (which would otherwise fight a
    /// manual vertical pan back into following the cursor). `None`
    /// before the first render, guaranteeing an initial clamp.
    last_cursor_row: Option<usize>,
    /// Horizontal scroll offset (in characters) for the main pane (spec
    /// 0113 D24) — the whole rendered line (fold-marker gutter included)
    /// pans together, the simplest of the layout options the spec left
    /// open.
    pan_offset: usize,
    /// Horizontal scroll offset (in characters) for the override
    /// selection pane's rows (spec 0127 §G1) — reset to `0` whenever the
    /// pane (re)opens or its candidate list is recomputed, mirroring how
    /// `override_scroll` (vertical) is already reset at those points.
    override_pan_offset: usize,
    /// Spec 0330 S1/S3: the caret row each sort order was last left on,
    /// indexed by `SortMode as usize` — `[Lexicographic, Inferred]`.
    ///
    /// Per open-pane session, not per document: cleared when the pane
    /// opens, so `t` on a new node asks a new question and both orders
    /// start at row 0. One row per order rather than one shared, because
    /// row 40 of a 600-entry alphabetic list and row 40 of a 12-entry
    /// inferred one have nothing to do with each other.
    override_sort_highlight: [usize; 2],
    /// Horizontal scroll offset (in characters) for the override
    /// management pane's rows (spec 0127 §G1) — reset to `0` whenever the
    /// pane (re)opens or its entry list changes in a way that already
    /// resets `manage_scroll` (vertical).
    manage_pan_offset: usize,
    /// Horizontal scroll offset (in characters) for the bottom command/
    /// message bar (spec 0127 §G1) — while a command/search/rename buffer
    /// is being typed, `render` auto-adjusts this to keep the cursor
    /// visible (mirroring the main pane's cursor-follow vertical scroll);
    /// otherwise it only changes via Shift+wheel/native horizontal-scroll
    /// pan on the hovered command bar.
    command_pan_offset: usize,
    /// `Some(node_idx)` while the override pane is open, holding the
    /// message/group node whose byte range it targets (spec 0114 §1/§2);
    /// `None` when closed.
    override_target: Option<usize>,
    /// Spec 0185: the live preview of the highlighted candidate, held
    /// beside the committed document and substituted for the target's
    /// rows at render time. `None` when nothing is being previewed.
    /// Dropped by plain assignment — a preview mutates nothing, so
    /// there is nothing to undo (this replaces spec 0161's
    /// `preview_tree_watermark` and the five tree/map fix-ups that
    /// existed only to back a preview splice out again).
    preview_overlay: Option<PreviewOverlay>,
    /// Spec 0174: `splice_override`'s live-preview interior byte budget —
    /// see `OVERRIDE_PREVIEW_BYTE_BUDGET_DEFAULT`'s doc comment. Defaults
    /// to that constant, overridable at startup via `main.rs`'s
    /// `--override-preview-byte-budget`.
    pub(crate) override_preview_byte_budget: usize,
    /// Spec 0217 S6: how many threads a sweep started by this session may
    /// fan out over — the app's whole CPU budget, `--jobs`. The heat
    /// worker is given one less than this (see `run`), since it sweeps
    /// while the main thread is still drawing. Defaults to 1, which is
    /// what every test wants: a sweep that runs where it was called.
    pub(crate) sweep_jobs: usize,
    /// `true` when the override pane has focus (spec 0114 §3's `Tab`
    /// toggle); meaningless while `override_target` is `None`.
    override_focus: bool,
    /// `true` when the override pane was opened via `Enter`/double-click
    /// on an entry in the management pane, rather than via `t`'s smart
    /// open on a main-pane node. Every exit from the selection pane
    /// consults it — `close_override` reopens the management pane
    /// instead of leaving focus on the main pane — and it is cleared as
    /// soon as it's acted on. Spec 0200 S2: `Enter` obeys it too, not
    /// just the cancelling exits (`Esc`/`t`), so that the pane always
    /// returns to whoever called it.
    override_opened_from_manage: bool,
    /// The origin kind to confirm a new type under, when the selection
    /// pane was opened on an *existing* entry (spec 0200 S3). `None` —
    /// the pane opened on a bare main-pane node — means whichever kind
    /// `override_origin_for_kind` derives for that node (spec 0308 S1).
    ///
    /// Set by `open_override_from_manage` from the entry's own origin,
    /// so that retyping an entry keeps the kind the user chose for it
    /// with the management pane's `z`/`Z` (spec 0124 G2), instead of
    /// leaving that entry active and adding a second, differently-kinded
    /// one beside it.
    override_origin_kind: Option<OverrideKind>,
    /// The background scoring worker thread (spec 0152 G1) — `None`
    /// whenever no scoring graph is loaded (mirroring the warm-up
    /// pass's own gate) or before `tui::run()` has spawned it; every
    /// fork that checks `heat_worker.is_some()` falls through to the
    /// existing synchronous logic when it's `None`.
    ///
    /// This field's declaration position is not load-bearing (spec 0180
    /// S2): the worker holds an `Arc<LoadedGraph>`, so it cannot outlive
    /// the mmap in any drop order. Do not re-derive an ordering
    /// requirement from its neighbors.
    ///
    /// Ordering *was* once the fix for a use-after-unmap race at exit —
    /// `heat_worker` borrowed the graph from `ctx`, `Drop` joins the
    /// thread using that borrow, and fields drop in declaration order.
    /// It was the wrong kind of fix: a safety invariant expressed as a
    /// source-line ordering, which the compiler does not check and which
    /// reaches only the thread that happens to have a handle to drop —
    /// the detached root-type sweep in `run()` has the identical
    /// lifetime, and no field order can help it.
    heat_worker: Option<heat_worker::HeatWorkerHandle>,
    /// Type-lookup/scoring context (spec 0114 §3) — owned by `App` after
    /// `decode()` returns it, so the override pane can resolve/score
    /// candidate types for the rest of the session.
    ctx: DescriptorContext,
    /// Session-global, alphabetically-sorted list of every message/group
    /// type FQDN known to `ctx.pool()` (spec 0114 §3.2/§6) — independent
    /// of range, computed once in `App::new`, reused by every
    /// lexicographic sort and by `:type-as`'s FQDN Tab-completion (spec
    /// 0114 §7).
    all_type_fqdns: Vec<String>,
    /// Sort mode for the override pane's ranked candidates (spec 0114
    /// §3.2) — persists across successive `t` invocations for the session
    /// (§8's key-bindings table).
    override_sort: SortMode,
    /// Ranked candidates (excluding the pinned `<raw / no type>` entry —
    /// §3.1) for whichever range `override_target` currently names, in
    /// the currently active `override_sort` order — FQDN plus its
    /// inferred score (`None` in lexicographic mode, which computes no
    /// score).
    override_candidates: Vec<(String, Option<i64>)>,
    /// Highlighted row within the override pane: `0` is the pinned
    /// `<raw / no type>` entry; `1..=override_candidates.len()` is
    /// `override_candidates[row - 1]`.
    override_highlight: usize,
    /// Vertical viewport (in rows, the pinned raw entry included) for the
    /// override pane's candidate list. Its rows are one terminal row
    /// tall, so `skip` is only ever `0` or negative (spec 0244 S3).
    override_scroll: PaneScroll,
    /// Spec 0286: `scroll_resistance`'s counterpart for
    /// `override_scroll`. Its own, and not the main pane's: a run is a
    /// repeat of the same push against the same end of the same
    /// content, and the reader can pan one pane while the other sits at
    /// its bound.
    override_resistance: EdgeResistance,
    /// `override_highlight`'s value as of the last render pass that
    /// applied `clamp_scroll_to_visible` to `override_scroll` (mirrors
    /// `last_cursor_row`) — reset to `None` everywhere `override_scroll`
    /// is itself reset to `0` (opening the pane, recomputing
    /// candidates), guaranteeing a clamp on the next render even if the
    /// new highlight happens to coincide with the old one.
    last_override_highlight: Option<usize>,
    /// Full terminal width (columns) as of the last `render()` call —
    /// basis for the override pane's minimum-width refusal (spec 0114
    /// §2), since `main_area`'s own width shrinks once the pane is open.
    term_width: u16,
    /// Override pane's candidate-list visible row count as of the last
    /// `render_override_pane()` call — basis for `PageUp`/`PageDown`
    /// scrolling by a full page, mirroring `main_area` (used the same way
    /// for the main pane's own `PageUp`/`PageDown`).
    override_list_height: usize,
    /// Shared, mutex-protected scoring cache (spec 0152 G4) — replaces
    /// spec 0151's three separate fields (`heat_range_cache`/
    /// `heat_current_score_cache`/`candidate_cache`), bundled under one
    /// lock since every read/write of them is a handful of `Vec`
    /// operations with no I/O. Both the render/input thread and
    /// `heat_worker` (when spawned) read and write this same structure
    /// directly — see `heat_worker::HeatCaches`.
    heat_caches: Arc<Mutex<heat_worker::HeatCaches>>,
    /// Per-node heat-cue resolution state (spec 0152 G6), parallel to
    /// `tree` — `Pending` until a cache check, only ever attempted when
    /// the node's row is drawn (spec 0224 S1) or when the read-ahead
    /// walk steps over it; `Resolved` nodes are read directly, no cache
    /// lock.
    heat_states: Vec<heat_cue::HeatState>,
    /// Spec 0337 S4: the ratcheted 95th-percentile log-space anchor.
    /// Starts at `ln(144)` (spec 0337 S5) and only ever rises: a falling
    /// anchor would brighten squares that already settled, which is
    /// flicker (spec 0337 G3). Used by `heat_display` as the scale top.
    heat_anchor: f32,
    /// Spec 0337 S2: histogram feeding the anchor above (spec 0337 S3).
    heat_histogram: heat_cue::HeatHistogram,
    /// Spec 0247 S6: the worst thing each node's *own* rows say,
    /// parallel to `tree`.
    status_own: Vec<Status>,
    /// Spec 0247 S6: each node's status including its whole subtree —
    /// what the fold toggle is colored by.
    ///
    /// Two parallel arrays rather than one of pairs: the roll-up's inner
    /// loop is a `max` over a child block, and level order makes that
    /// block a contiguous slice of *this* array alone.
    status_rolled: Vec<Status>,
    /// Spec 0164 G7: main-pane zigzag read-ahead prefetch walk state,
    /// persisted across `run_loop` iterations.
    prefetch_walk: PrefetchWalk,
    /// Per-wave counters for `PROTOLENS_TRACE`. Maintained
    /// unconditionally (a handful of increments per walk step) but only
    /// reported when the trace is on.
    prefetch_trace: PrefetchTrace,
    /// Spec 0191 G3/G4: the activity level the dot should show, decided
    /// by `run_loop` from a high-water sample rather than probed at draw
    /// time. `render_activity_dot` reads this instead of calling
    /// `heat_activity()` directly, because the value that matters spans
    /// the whole interval since the last frame — including the requests
    /// that frame's own `heat_cue_for` calls queued, which a probe taken
    /// before `terminal.draw` can never see.
    activity_shown: Option<tiered::Tier>,
    /// Spec 0223 S1/S3: whether terminal events were still queued when
    /// `run_loop` decided to draw this frame. Set by the loop just
    /// before `terminal.draw`; read by `render_main_pane` to decide
    /// whether to run tree-sitter at all.
    ///
    /// A plain `bool` sampled once per frame rather than the shared
    /// counter itself: highlighting has to be decided once for the whole
    /// frame, and a counter read from inside `render` could answer
    /// differently for two panes of the same one.
    pub(super) input_pending: bool,
    /// Spec 0245 S2: set by the event just dispatched to say that it
    /// left the screen exactly as the user is already seeing it, so
    /// `run_loop` owes it no frame. Cleared by the loop before every
    /// dispatch, so the default is to redraw.
    ///
    /// Only the pan functions ever set it. A pan is the one input the
    /// user generates in long unbroken runs against a hard bound, and
    /// its whole effect is a single number that is trivially compared;
    /// a general "did anything change?" test over `App` would be both
    /// expensive and fragile.
    pub(super) event_changed_nothing: bool,
    /// Incremented on every fold/unfold and every commit — i.e. every
    /// time a rendered line number may have shifted. `App::
    /// prefetch_step`'s staleness signal (spec 0164 G7) for restarting
    /// its walk.
    structural_version: u64,
    /// What the last drawn frame's window was, as `(first row, row
    /// count, structural version)` — spec 0252 S1's change detector for
    /// bumping the request queue's generation.
    ///
    /// Held here rather than derived, because the queue must be told
    /// only when the set of drawn rows actually changes: bumping every
    /// frame would discard every `Visible` request before any worker
    /// could reach it, and the cues would never resolve at all.
    heat_window_key: Option<(usize, usize, u64)>,
    /// `true` while `recompute_override_candidates`'s `SortMode::
    /// Inferred` branch is waiting on a worker request for the
    /// override pane's first page (spec 0152 G7).
    override_candidates_pending: bool,
    /// `true` while `upgrade_active_override_to_complete` is waiting on
    /// a worker request for a wider window (spec 0152 G7).
    override_complete_pending: bool,
    /// How much of the main-pane heat machinery is drawn (specs 0138,
    /// 0331) — rotated by `i`/`I`, and never discarding `heat_caches`.
    heat_cues: heat_cue::HeatCueMode,
    /// Byte-bounded MRU cache of `(range, type) -> (lines, spans, style
    /// hints)` renders (spec 0116 §8) — consulted/populated by
    /// `apply_override`, keyed by the same `payload_range`/type pair
    /// `heat_caches` already keys its own entries on.
    render_cache: RenderCache,
    /// The tag/length-stripped target range whose complete-or-capped
    /// inferred-candidate list `override_inferred_raw` currently holds —
    /// `None` when no override pane is open, or the graph-less/
    /// lexicographic-only case. Distinct from `override_target` (a tree
    /// node index): this is the byte range that list was computed for.
    active_override_range: Option<Range<usize>>,
    /// Raw `(fqdn, score)` list for `active_override_range`, source of
    /// truth `override_candidates` is derived from in `SortMode::Inferred`
    /// — either the complete ranked list, or (right after a
    /// `heat_caches` hit) a capped preview, per
    /// `override_candidates_complete`.
    override_inferred_raw: Vec<(String, i64)>,
    /// Whether `override_inferred_raw` is the complete ranked list for
    /// `active_override_range`, or just a capped preview pulled from
    /// `heat_caches` — an incomplete preview is upgraded to the
    /// complete list (a `heat_lookup` call for a wider window, spec
    /// 0152 G7) the moment the user tries to scroll past it (spec
    /// 0114 §6).
    override_candidates_complete: bool,
    /// The FQDN (or the `None` sentinel string) that an
    /// `open_override_on_type` call is still trying to highlight once
    /// fetched, when a cold cache left it unresolved at open time
    /// (`override_candidates_pending`/`override_complete_pending`).
    /// Consulted by `poll_pending_override_work` after each background
    /// resolution; cleared once the row is found (or, having reached
    /// `override_candidates_complete` without finding it, the pane
    /// falls back to `Lexicographic` mode instead — mirroring
    /// `open_override_on_type`'s own synchronous fallback).
    override_seek_target: Option<String>,
    /// Persistent collection of overrides (spec 0117 §1) — distinct from,
    /// and unrelated to, the one-shot `apply_override` splice-render
    /// mechanism above; see spec 0117's Non-goals.
    overrides: override_pane::OverrideCollection,
    /// `true` while the override management pane (spec 0117 §3, `o`) is
    /// open. Mutually exclusive with `override_target.is_some()`.
    manage_open: bool,
    /// `true` when the management pane has focus (mirroring
    /// `override_focus`); meaningless while `manage_open` is `false`.
    /// A main-pane mouse click while the pane stays open shifts this
    /// back to `false` without closing it.
    manage_focus: bool,
    /// Highlighted row (index into `overrides.entries()`) in the
    /// management pane.
    manage_highlight: usize,
    /// Vertical viewport (in rows) for the management pane's listing.
    /// One terminal row per row, as `override_scroll`.
    manage_scroll: PaneScroll,
    /// Spec 0286: `scroll_resistance`'s counterpart for `manage_scroll`.
    manage_resistance: EdgeResistance,
    /// `manage_highlighted_row()`'s value as of the last render pass that
    /// applied `clamp_scroll_to_visible` to `manage_scroll` (mirrors
    /// `last_cursor_row`) — reset to `None` everywhere `manage_scroll`
    /// is itself reset to `0`.
    last_manage_highlight: Option<usize>,
    /// Timestamp + entry index of the most recent left-click `Down` that
    /// landed on an entry's radio marker — compared against on the next
    /// such click to recognize a double-click (same entry, within
    /// `DOUBLE_CLICK_THRESHOLD`), the mouse-only alternative to
    /// Shift-click for `toggle_active_cascading` (most terminal
    /// emulators intercept Shift-click for native text selection before
    /// it ever reaches the app). `None` before the first marker click.
    last_manage_click: Option<(Instant, usize)>,
    /// Timestamp + entry index of the most recent left-click `Down` that
    /// landed on an entry row *outside* its radio marker — compared
    /// against on the next such click to recognize a double-click (same
    /// entry, within `DOUBLE_CLICK_THRESHOLD`), which opens the override
    /// selection pane on that entry (`open_override_from_manage`), same
    /// as `Enter`. Tracked separately from `last_manage_click`, which is
    /// marker-column-only and drives an unrelated toggle behavior.
    last_manage_row_click: Option<(Instant, usize)>,
    /// Timestamp + candidate index of the most recent left-click `Down`
    /// that landed on a row in the override selection pane — compared
    /// against on the next such click to recognize a double-click (same
    /// row, within `DOUBLE_CLICK_THRESHOLD`), which applies the
    /// highlighted candidate and closes the pane, same as `Enter`.
    last_override_click: Option<(Instant, usize)>,
    /// `Some((origin, kind, cursor_moves))` while a `z`/`Z` attempt in
    /// the management pane is unresolved (spec 0134 G2/G3): `origin` is
    /// the highlighted entry's origin at the time of that attempt,
    /// `kind` is the `OverrideKind` it evaluated, and `cursor_moves` is
    /// `self.cursor_moves`'s value at that time. A same-direction `z`/
    /// `Z` press with `self.cursor_moves` still equal to the stashed
    /// value (i.e. the cursor genuinely hasn't moved since — not just
    /// "ended up at the same position," which a Down-then-Up round trip
    /// would falsely satisfy) advances past `kind`; otherwise it retries
    /// `kind`. Cleared on every successful rotation and whenever
    /// `manage_highlight` moves to a different entry.
    manage_pending_kind: Option<(OverrideOrigin, OverrideKind, u64)>,
    /// Management pane's visible row count as of the last
    /// `render_manage_pane()` call — basis for `PageUp`/`PageDown`.
    manage_list_height: usize,
    /// Spec 0194 S10: whole cursor *positions*, not bare node indices —
    /// so `Ctrl-o` returns to the character the user was reading, and to
    /// the right half of a bracketed node.
    back_stack: Vec<CursorPos>,
    fwd_stack: Vec<CursorPos>,
    /// Document-order first node — `Home`/`gg` target.
    first_node: usize,
    /// Set by a first `g` press, consumed (and cleared) by a second `g`
    /// press within the very next keystroke (`gg` chord, vim-style); any
    /// other key clears it.
    pending_g: bool,
    /// Export-chord leader state (spec 0156 G3): `None` (no chord
    /// armed), `Leader` (a lone `x` was just pressed), `Descriptor`
    /// (`xd` was just pressed — one more key selects binary vs.
    /// prototext).
    pending_x: ExportChord,
    /// `Some(buffer)` while a `:`/`x`-triggered command line, or a `/`/`?`
    /// main-pane search prompt (spec 0114 §4, extended from the override
    /// pane — see `CommandLineKind`), is being edited; `None` in normal
    /// navigation mode.
    command_buffer: Option<String>,
    /// What `command_buffer` currently represents; meaningless while
    /// `command_buffer` is `None`.
    command_kind: CommandLineKind,
    /// Cursor position within `command_buffer`, as a **char** index (not
    /// byte index) — `0..=command_buffer.chars().count()`. Moved by
    /// `Left`/`Right`/`Home`/`End`; edits (`Backspace`/`Delete`/typing)
    /// happen relative to it rather than always at the buffer's end.
    command_cursor: usize,
    /// Spec 0340 S9: each pane's last confirmed search (direction,
    /// pattern), indexed by `SearchScope`. `n` repeats its own pane's in
    /// the same direction, and an empty `/`/`?` confirmation reuses its
    /// pattern (spec 0114 §4).
    ///
    /// One array rather than a field per pane: the three were declared
    /// hundreds of lines apart and read through a three-way match at
    /// each of three call sites, so a fourth pane cost four edits to add
    /// nothing new. Reach it through `last_search_for` /
    /// `set_last_search_for` rather than by index.
    last_searches: [Option<(SearchDir, String)>; search::SCOPE_COUNT],
    /// Spec 0235 S2: the search in flight, or the finished one whose
    /// answer the prompt is still showing. `None` outside a search.
    search_sweep: Option<search::SearchSweep>,
    /// Spec 0277 S1: how many matches the live pattern has in the whole
    /// document, and which of them is on screen. `None` outside a
    /// search, and while there is nothing to count (S12, N1–N3).
    search_tally: Option<search::SearchTally>,
    /// Spec 0235 S6: where the open `/`/`?` prompt was opened from —
    /// what each keystroke searches again from, and what `Esc` restores.
    /// Spec 0246 S19: a rotation does *not* move it.
    search_origin: Option<search::SearchOrigin>,
    /// Spec 0246 S11: every committed search pattern, oldest last,
    /// shared by all three panes as vim shares one across buffers.
    search_history: Vec<String>,
    /// Spec 0246 S14: how far `Up`/`Down` have walked back into
    /// `search_history`, and the text they displaced. `None` when the
    /// buffer is the user's own typing.
    search_browse: Option<search::SearchBrowse>,
    /// Spec 0235 S15: whether search matches are drawn. On while a
    /// `Search` prompt is open, and stays on after a commit and after
    /// `n`/`N`; `Esc` clears it, inside the prompt and outside it.
    search_highlight: bool,
    /// Spec 0235 S5: a sweep's *result* changed and owes a frame.
    /// Consumed by `run_loop` alongside its other redraw forces.
    search_dirty: bool,
    /// Spec 0235 S13: a match needs bringing into view. Set by the
    /// sweep, cleared by the `render` pass that centers it — the pane's
    /// height and width are known nowhere else.
    search_center: bool,
    /// Spec 0274 S9: how a segment scan wakes the loop that is waiting
    /// for it — `run()`'s own event channel, handed over once the loop
    /// exists to drain it.
    ///
    /// `None` before that and in every headless session, and it is the
    /// gate as well as the channel: with nobody listening a report
    /// would never be collected, so a sweep with no sender scans its
    /// segments on this thread instead. Same shape as
    /// `heat_worker.is_some()`.
    search_progress: Option<mpsc::Sender<event::AppEvent>>,
    /// Spec 0343 B4/B6: the structural pass in progress, `None` before
    /// the first idle turn (B6 stage 1) and after the pass completes.
    /// Its links feed B5's filter once `is_complete()` returns true.
    shadow_sweep: Option<shadow_sweep::ShadowSweep>,
    /// Spec 0343 B7: one bit per arena slot, set by the B5 filter when
    /// a slot is shadowed.  Allocated at construction (arena size is
    /// known then) and zeroed; bits are set incrementally by the probe.
    /// Written only by the single-threaded trie walk; read-only after
    /// `shadow_probed` is set.  The `Arc` lets the spawned segment-scan
    /// thread share the bitset without a copy; plain `u64` (not
    /// `AtomicU64`) suffices since no concurrent writes ever happen.
    shadowed: Arc<Vec<u64>>,
    /// Spec 0343 B6: whether the post-completion whole-arena probe has
    /// run.  Reset by `invalidate_shadow_bits` so the probe re-runs
    /// after an override on a fully-rendered document (no bake stops).
    shadow_probed: bool,
    /// Cursor for the incremental post-trie arena probe (spec 0343 B6).
    /// `Some(n)` means the probe is running and has scanned `0..n` so
    /// far; `None` means it is not running.
    shadow_probe_cursor: Option<usize>,
    /// Active Tab-completion cycle state (spec 0113 D26); `None` when not
    /// currently cycling.
    completion: Option<CompletionState>,
    /// `true` on startup until the first keypress dismisses it — a splash
    /// screen telling the user how to reach help (spec 0113 D22).
    splash: bool,
    /// Wall-clock time at which `splash` auto-dismisses, in addition to
    /// its keypress/mouse dismissal — checked only while `splash` is
    /// still `true`, mirroring `message_deadline`'s deadline-based
    /// approach.
    splash_deadline: Instant,
    /// `true` while the `F1` help overlay is open.
    help_open: bool,
    /// Spec 0340 S1: the highlighted `HELP_TEXT` row — the overlay's
    /// cursor, and the row a search lands on.
    help_highlight: usize,
    /// `help_highlight` as of the last frame, so that the auto-pan only
    /// fires on genuine cursor movement and a deliberate pan survives
    /// the next frame — `last_override_highlight`'s twin.
    last_help_highlight: Option<usize>,
    /// The overlay's viewport. A `PaneScroll` rather than the bare
    /// `usize` it was before spec 0340, so that the cursor above can be
    /// kept in view by the same `clamp_scroll_to_visible` the side panes
    /// use, and so that an `Esc`-cancelled search can restore it.
    help_scroll: PaneScroll,
    /// How far the overlay is panned horizontally. The help's longest
    /// lines run past a 70%-wide modal, so it pans like any other pane.
    help_pan_offset: usize,
    /// Rows of `HELP_TEXT` the overlay last drew — the page `f`/`b` move
    /// by, and the window `show_sweep_hit` centers a match in.
    help_list_height: usize,
    /// Spec 0286's wall, for the overlay's own vertical pan.
    help_resistance: EdgeResistance,
    /// Help overlay's inner (bordered-away) `Rect` as of the last
    /// `render_help()` call — used to hit-test mouse wheel/Shift-wheel
    /// events against the overlay instead of letting them fall through
    /// to whichever pane it happens to be drawn on top of. Only
    /// meaningful while `help_open`.
    help_area: Rect,
    /// The open context menu, `None` when there is none. Drawn over
    /// everything including the help overlay, and answered ahead of
    /// every other key tier, so it is the innermost modal the app has.
    menu: Option<Menu>,
    /// The open score box, `None` when there is none (spec 0280 S14).
    /// Drawn last, after the menu, and refused while the menu is open —
    /// the menu stays the innermost modal.
    popup: Option<Popup>,
    /// The annotated type name the pointer is resting on, `None` when it
    /// is on none. Not itself visible: it is what `track_hover_dwell`
    /// opens the box for once `hover_deadline` expires.
    hover: Option<Hover>,
    /// When the resting pointer has earned its box (spec 0280 S11).
    /// Joins `message_deadline` and `splash_deadline` in `run_loop`'s
    /// `ui_deadline`, so an untouched mouse arms nothing and spec 0263's
    /// idle guarantee is untouched.
    hover_deadline: Option<Instant>,
    /// The last breakdown computed, with the `(range start, type key)`
    /// it was computed for (spec 0280 S5).
    ///
    /// One entry rather than a map: only one box can be open, so a keyed
    /// cache would have a single reader and an eviction policy nobody
    /// needs.
    breakdown_memo: Option<((usize, String), Breakdown)>,
    header: String,
    /// Main pane's content `Rect` (the `Min(0)` split above its own
    /// `Length(1)` local statusline row, spec 0147 G1) as of the last
    /// `render()` call — used to hit-test mouse clicks against display
    /// rows/columns.
    main_area: Rect,
    /// Override selection pane's / override management pane's content
    /// `Rect` (the `Min(0)` split above its own `Length(1)` local
    /// statusline row, spec 0147 G1) as of the last render (spec 0113
    /// D30) — used to hit-test mouse clicks the same way `main_area`
    /// does. A single field suffices since the two panes are mutually
    /// exclusive (`override_target.is_some()` XOR `manage_open`).
    side_area: Rect,
    /// Global command/message row's `Rect` as of the last `render()`
    /// call (spec 0147 G4), `None` when `command_buffer` is inactive —
    /// used to hit-test mouse hover for Shift+wheel/native horizontal
    /// pan the same way `main_area`/`side_area` do.
    cmd_area: Option<Rect>,
    /// The script pane's content `Rect` as of the last `render()` call —
    /// what bounds `Up`/`Down` scrolling to the step's own text, since
    /// how far a step reaches is a fact about the wrapped paragraph and
    /// the width it was wrapped at.
    script_area: Rect,
    /// Spec 0355 S4: whether a click on the script pane has given it
    /// focus. While true and navigation is on, `PageDown`/`PageUp` fire
    /// the script's advance/retreat instead of the document's page move.
    /// Cleared by any click outside the script pane, and by `Tab`.
    script_focus: bool,
    pub message: String,
    /// Spec 0278 S1: the pattern a committed search leaves echoed on the
    /// command row, as `(direction, pattern)`.
    ///
    /// Not a `message`: a message auto-dismisses after `MESSAGE_TIMEOUT`
    /// (spec 0147 G6), and an echo that expired on a timer would take
    /// spec 0277's count with it while the reader was still reading the
    /// match. It is dismissed by input instead — the same keypress and
    /// mouse-event clears `message` gets — so the pattern and its count
    /// arrive together and leave together.
    search_echo: Option<(SearchDir, String)>,
    /// Every override still refused when `render_overrides`' most recent
    /// pass ended, as `(node, description)` in visit order (spec 0221
    /// S1). Emptied at the start of each outermost pass, so it always
    /// describes the last one rather than accumulating across a session.
    ///
    /// The node index is carried because a failed splice is not
    /// necessarily final. A pass can visit a node before the thing that
    /// makes its override applicable exists — spec 0120's MessageSet
    /// auto-expansion registers a synthetic type partway through the
    /// same pass that then uses it — and re-settle it successfully
    /// afterwards. `resettle_node` therefore withdraws a node's entry
    /// when a later attempt succeeds; without that, a batch export that
    /// produces exactly the right bytes would report an error and fail.
    ///
    /// Collected rather than printed because `render_overrides` also
    /// runs mid-session, with the terminal in raw mode and the alternate
    /// screen up, where a write to stderr would corrupt the display. The
    /// caller decides: `main.rs` prints these to stderr for a batch
    /// export and for TUI startup (both happen before `tui::run` takes
    /// the terminal), while a pass triggered from inside the session
    /// reports through `self.message` as it always has.
    pub refusals: Vec<(usize, String)>,
    /// Spec 0258 S3: whether the `render_overrides` pass now running was
    /// asked for by the user. `expand_auto_fold`'s resolution pass sets
    /// it, because a refusal deep in a subtree the bake happened to
    /// reveal — or that the user merely opened a fold over — is not an
    /// answer to any question they asked, and a drain runs thousands of
    /// such passes.
    ///
    /// A flag rather than saving and restoring `self.message` around the
    /// pass: restoring would clobber whatever else wrote the status line
    /// while it ran.
    silent_refusals: bool,
    /// Mirrors `self.message` as of the last `track_message_timeout()`
    /// call — used to detect a freshly-set message (`self.message` has
    /// no dedicated setter; it's assigned directly all over this file),
    /// so `message_deadline` gets (re)armed exactly once per message,
    /// not on every frame.
    last_message_seen: String,
    /// Wall-clock time at which the current `self.message` should be
    /// auto-dismissed, `None` while `self.message` is empty. Consulted
    /// (and cleared) only by `track_message_timeout`, which never fires
    /// while the `command_buffer` text-entry prompt is actively awaiting
    /// a keypress.
    message_deadline: Option<Instant>,
    pub should_quit: bool,
    /// `true` right after `Ctrl-Z` (spec 0113 D31, Unix only), checked by
    /// `run_loop` after each `handle_key` call — mirrors `should_quit`'s
    /// own "flag set here, acted on there" split, since actually
    /// suspending the process needs the `Terminal` handle that only
    /// `run_loop` owns.
    pub should_suspend: bool,

    /// `-I`/`--proto-root`'s resolved value, or `:proto-root`'s last
    /// successfully-set value (spec 0144 G4) — `None` until either is set.
    pub proto_root: Option<PathBuf>,

    /// Armed by `open_definition` (G1-G4) once a `v` press has fully
    /// resolved to a real, on-disk `.proto` location; consumed by
    /// `run_loop` right after the `handle_key` call that armed it, which
    /// calls `neovim::open_editor` (G5) with the `Terminal` handle only it
    /// owns. Mirrors `should_suspend`'s own split for the same reason.
    #[cfg(unix)]
    pub pending_editor_open: Option<neovim::EditorRequest>,

    /// `v`'s Neovim handoff state (spec 0144 G5) — `NotRunning` until the
    /// first successful `open_editor` call, `Suspended` whenever a
    /// handed-off Neovim is currently stopped in the background.
    #[cfg(unix)]
    pub(crate) editor_state: neovim::EditorState,

    /// Spec 0271: the loaded script and where the session is in it,
    /// `None` in an ordinary session. Set by `App::set_script` rather
    /// than by `App::new`, because applying step 1 needs a fully built
    /// `App` to apply it to.
    script: Option<script_pane::ScriptState>,
    /// `--script-height`'s value, overriding the computed share of the
    /// terminal (spec 0271 S4).
    pub script_height: Option<u16>,
}

impl App {
    /// The rendered tree, for writing (spec 0274 S8).
    ///
    /// Taking the `&mut` is what ends a segment scan: the scan holds a
    /// clone of this `Arc`, and an answer about a document that no
    /// longer exists is not an answer. Putting the halt here rather
    /// than at the writers is what makes it impossible to forget —
    /// there is no other way to write the tree.
    ///
    /// `get_mut` rather than the spec's `make_mut`: `make_mut` would
    /// need `TreeNode: Clone`, and the clone it would then reach for is
    /// a silent copy of 200 MB. Halting above is what makes the sole
    /// owner, so failing here is a bug and not a slow path.
    pub(super) fn tree_mut(&mut self) -> &mut Vec<TreeNode> {
        self.halt_search_scan();
        Arc::get_mut(&mut self.tree).expect("the halt above leaves the tree unshared")
    }

    /// The per-node text, for writing — spec 0274 S8, exactly as
    /// [`App::tree_mut`].
    pub(super) fn node_text_mut(&mut self) -> &mut Vec<Option<Box<str>>> {
        self.halt_search_scan();
        Arc::get_mut(&mut self.node_text).expect("the halt above leaves the text unshared")
    }

    pub fn new(
        mut decoded: Decoded,
        blob_label: &str,
        blob_path: PathBuf,
        indent_size: usize,
        ctx: DescriptorContext,
        theme: ThemeKind,
        proto_root: Option<PathBuf>,
    ) -> Self {
        // Spec 0197 §S4: on the lazy branch this reads `index.rkyv`'s
        // `type_to_file` instead of walking a decoded pool. Same names,
        // messages and enums alike; it stays eager because it is 11-13 ms
        // on googleapis either way.
        let all_type_fqdns = ctx.all_type_fqdns();
        // Spec 0197 §S3, third channel. The splash pane carries the same
        // warning, but it is dismissed by the first key — a user who
        // starts typing straight away would never read it. The status
        // line survives until the next thing writes there.
        let fallback_warning = match &ctx.fallback {
            Some(fallback) => format!("warning: {}", fallback.message),
            None => String::new(),
        };
        let tree_len = decoded.tree.len();
        let root_candidates = std::mem::take(&mut decoded.root_candidates);
        // Spec 0257 S3: the startup render's own stops, in its document
        // order — the same pair of sets a bounded confirm builds in
        // `splice_override`, seeded here instead because there was no
        // splice. `row_budget` and not `stops.is_empty()`: a document
        // small enough to fit the screen stops nowhere and still asked
        // to be bounded, and its confirms must be too.
        let stops = std::mem::take(&mut decoded.stops);
        // Spec 0338 S1: born full — every slot is a member, so the
        // answer to "does the reader want this open?" is total before
        // any render asks. Spec 0323 S3 opens the root out of it below,
        // and that is the one exception to "only the reader writes this
        // set".
        let user_folded = std::mem::take(&mut decoded.user_folded);
        let mut auto_folded = FoldSet::new(tree_len);
        for &slot in &stops {
            auto_folded.insert(slot);
        }
        let bounded_confirms = decoded.row_budget.is_some();
        let header = format!(
            "protolens — {blob_label} — {}",
            decoded.root_type.as_deref().unwrap_or("(no type)")
        );
        // Spec 0216 S1: the arena is in level order and slot 0 is the
        // wrapper, so the document-order first node is the first slot —
        // no search for the node with no `doc_prev` is needed.
        let cursor = 0;
        // Spec 0117 §1: seed the root `path` override with whatever type
        // was explicitly requested or inferred; if neither is available,
        // seed nothing at all — an untyped root has no override worth
        // recording, and the collection is legitimately empty until the
        // user adds one. Spec 0353: `root_type` is now `Option<String>`,
        // so `None` is the direct signal.
        let root_override_type = decoded.root_type.clone();
        let mut overrides = override_pane::OverrideCollection::new();
        if root_override_type.is_some() {
            overrides.seed_root(root_override_type.clone());
        }
        let arena_len = decoded.arena.len();
        let mut app = App {
            blob: decoded.blob,
            wrapper_offset: decoded.wrapper_offset,
            blob_path,
            // Always on (spec 0133): a pure main-pane display attribute
            // from here on, toggled at runtime by the `a` key.
            annotations: true,
            // Spec 0225 S1: off until the `w` key asks for it.
            wire: None,
            wire_rows: std::cell::RefCell::new(None),
            cursor_bar: std::cell::RefCell::new(None),
            search_compiled: std::cell::RefCell::new(None),
            indent_size,
            node_text: Arc::new(decoded.node_text),
            window_styles: Vec::new(),
            window_styles_key: None,
            theme,
            tree: Arc::new(decoded.tree),
            arena: Arc::new(decoded.arena),
            fqdns: decoded.fqdns,
            provenance: ProvenanceTable::new(),
            override_batch_depth: 0,
            descend: Vec::new(),
            #[cfg(test)]
            unpruned_walk: false,
            #[cfg(test)]
            verify_repair: true,
            batch_spliced: false,
            cursor,
            cursor_line_in_node: 0,
            cursor_column: 0,
            desired_column: 0,
            // Spec 0199 S1: the first frame's caret sits on the first
            // node's first non-blank, put there by the same node-level
            // placement every later `set_cursor` performs.
            caret_anchor: CaretAnchor::Home,
            caret_suffix_len: 0,
            cursor_moves: 0,
            select_anchor: None,
            select_caret: None,
            select_engaged: false,
            last_click: None,
            pending_double_click: None,
            user_folded,
            auto_folded,
            bake_queue: stops.iter().copied().collect(),
            visible_stops: VecDeque::new(),
            bounded_confirms,
            discarded_text: Vec::new(),
            scroll: PaneScroll::default(),
            scroll_resistance: EdgeResistance::default(),
            scroll_anchor: None,
            target_anchor: None,
            last_cursor_row: None,
            pan_offset: 0,
            override_pan_offset: 0,
            override_sort_highlight: [0; 2],
            manage_pan_offset: 0,
            command_pan_offset: 0,
            override_target: None,
            preview_overlay: None,
            override_preview_byte_budget: Self::OVERRIDE_PREVIEW_BYTE_BUDGET_DEFAULT,
            sweep_jobs: 1,
            override_focus: false,
            override_opened_from_manage: false,
            override_origin_kind: None,
            ctx,
            all_type_fqdns,
            override_sort: SortMode::Inferred,
            override_candidates: Vec::new(),
            override_highlight: 0,
            override_scroll: PaneScroll::default(),
            override_resistance: EdgeResistance::default(),
            last_override_highlight: None,
            term_width: 0,
            override_list_height: 0,
            heat_caches: Arc::new(Mutex::new(heat_worker::HeatCaches::new(
                heat_cue::HEAT_CACHE_MAX_ENTRIES,
            ))),
            heat_worker: None,
            heat_states: vec![heat_cue::HeatState::default(); tree_len],
            heat_anchor: heat_cue::HEAT_ANCHOR_DEFAULT,
            heat_histogram: heat_cue::HeatHistogram::default(),
            // Filled by the `rebuild_status` below, once the tree the
            // pass reads is in place.
            status_own: vec![Status::Ok; tree_len],
            status_rolled: vec![Status::Ok; tree_len],
            prefetch_walk: PrefetchWalk::exhausted(),
            prefetch_trace: PrefetchTrace::default(),
            activity_shown: None,
            input_pending: false,
            event_changed_nothing: false,
            structural_version: 0,
            heat_window_key: None,
            override_candidates_pending: false,
            override_complete_pending: false,
            heat_cues: heat_cue::HeatCueMode::default(),
            render_cache: RenderCache::new(RENDER_CACHE_MAX_BYTES),
            active_override_range: None,
            override_inferred_raw: Vec::new(),
            override_candidates_complete: false,
            override_seek_target: None,
            overrides,
            manage_open: false,
            manage_focus: false,
            manage_highlight: 0,
            manage_scroll: PaneScroll::default(),
            manage_resistance: EdgeResistance::default(),
            last_manage_highlight: None,
            last_manage_click: None,
            last_manage_row_click: None,
            last_override_click: None,
            manage_pending_kind: None,
            manage_list_height: 0,
            back_stack: Vec::new(),
            fwd_stack: Vec::new(),
            first_node: cursor,
            pending_g: false,
            pending_x: ExportChord::None,
            command_buffer: None,
            command_kind: CommandLineKind::Command,
            command_cursor: 0,
            last_searches: Default::default(),
            search_sweep: None,
            search_tally: None,
            search_origin: None,
            search_history: Vec::new(),
            search_browse: None,
            search_highlight: false,
            search_dirty: false,
            search_center: false,
            search_progress: None,
            shadow_sweep: None,
            shadowed: Arc::new(vec![0u64; arena_len.div_ceil(64)]),
            shadow_probed: false,
            shadow_probe_cursor: None,
            completion: None,
            splash: true,
            splash_deadline: Instant::now() + SPLASH_TIMEOUT,
            help_open: false,
            help_highlight: 0,
            last_help_highlight: None,
            help_scroll: PaneScroll::default(),
            help_pan_offset: 0,
            help_list_height: 0,
            help_resistance: EdgeResistance::default(),
            menu: None,
            popup: None,
            hover: None,
            hover_deadline: None,
            breakdown_memo: None,
            help_area: Rect::default(),
            header,
            main_area: Rect::default(),
            side_area: Rect::default(),
            cmd_area: None,
            script_area: Rect::default(),
            script_focus: false,
            message: fallback_warning,
            search_echo: None,
            refusals: Vec::new(),
            silent_refusals: false,
            last_message_seen: String::new(),
            message_deadline: None,
            should_quit: false,
            should_suspend: false,
            proto_root,
            #[cfg(unix)]
            pending_editor_open: None,
            #[cfg(unix)]
            editor_state: neovim::EditorState::NotRunning,
            script: None,
            script_height: None,
        };
        // Spec 0257 S3 used to repair each stop's `lines_visible` here:
        // `build_tree` gave it the header-plus-footer pair it actually
        // emitted, where the document shows one row. Since spec 0338 S3
        // `build_tree` writes the collapsed count over the bracketed
        // slots the render reported — a stop is bracketed, so it is
        // folded like every other bracketed slot and its count is right
        // the first time.
        debug_assert!(
            stops.iter().all(|&slot| app.tree[slot].is_bracketed()),
            "only a message recursion can be undescended (spec 0249 S1)"
        );
        // Spec 0323 S3: the document opens the way `Z` then `z` leaves
        // it — everything folded, the root alone open, so disclosure is
        // one level at a time. Spec 0338 S1 does the `Z` by constructing
        // the set full; this is the whole of the `z`, one bit cleared
        // and one climb over the root's own children.
        if !app.tree.is_empty() {
            app.user_folded.remove(cursor);
            app.refresh_line_counts(cursor);
        }
        // Spec 0247 S7: the one full pass. Every later change to the
        // document is a splice, and a splice repairs the two arrays
        // incrementally.
        app.rebuild_status();
        // Spec 0118 §2.1: the wrapper root is already rendered under
        // `root_override_type` by `decode()` itself, matching the
        // `seed_root` entry above (when one was seeded) — mark it as
        // such so the first `render_overrides` pass doesn't treat it as
        // a mismatch and needlessly re-splice the entire tree (which
        // would invalidate every already-computed node index: `cursor`,
        // `folded`, etc.). When no entry was seeded (untyped root), the
        // outer `Option` must be `None` — matching what
        // `resolve_active_override` will itself compute ("no active
        // entry") — not `Some(None)` ("an active entry explicitly says
        // raw"), else the first pass would wrongly conclude nothing
        // needs resettling should the user later seed a real entry.
        // The root's own field name is always "1" (its field number in the
        // virtual encompassing message — mirrors `field_name_for`'s
        // no-parent case), and can't yet carry a §G4 name override at this
        // point (the collection was just seeded, nothing has been
        // renamed). Interned before the node is reached, since the table
        // and the arena are two disjoint borrows of `app`.
        if cursor < app.tree.len() {
            let root_provenance = app
                .provenance
                .intern(&(root_override_type.clone().map(Some), "1".to_string()));
            app.tree_mut()[cursor].rendered_as = root_provenance;
        }
        // Spec 0120: Any/MessageSet auto-expansion is computed by
        // `render_overrides` itself (`auto_expand_type`), not by
        // `decode()`'s own initial paint — run one pass now so the
        // initial view already shows any/MessageSet content expanded.
        // Guarded like the block above: an empty tree has no node at
        // `cursor` to render.
        //
        // Nothing precomputes the row numbering (spec 0210 S2):
        // `build_tree` sets every node's `lines_total` and
        // `lines_visible` as it builds it, so the numbering is already
        // consistent with `lines` and `folded` the moment the tree
        // exists.
        if !app.tree.is_empty() {
            app.render_overrides(cursor);
        }
        app.seed_root_heat(root_candidates);
        app
    }

    /// Spec 0168 G3: the root-type sweep already scored exactly the root
    /// node's interior payload range, so write its result into the heat
    /// caches instead of letting `heat_cue_for` and the override pane
    /// re-run `score_all` over the same bytes. That range is the single
    /// most expensive one in the document and it is the one the cursor
    /// starts on, so it is also the one they ask about first.
    ///
    /// A no-op when the sweep didn't run (`--type`, `--raw`, or no
    /// scoring graph), which is why the empty list is not special-cased
    /// anywhere else.
    ///
    /// `by_range`'s `top_n` is truncated to the same
    /// `max(override_list_height, HEAT_CUE_PREVIEW)` cap
    /// `heat_cue_resolve` applies, rather than holding the full list —
    /// `complete` is where the unbounded list belongs, and a full-width
    /// `by_range` entry is exactly the oversized entry
    /// `rendering-flaws.md` P5 describes.
    fn seed_root_heat(&mut self, candidates: decode::RankedCandidates) {
        if candidates.is_empty() || self.tree.is_empty() {
            return;
        }
        let range = self.heat_scored_range(self.first_node);
        let stats = heat_cue::derive_stats(&candidates);
        let cap = self
            .override_list_height
            .max(1)
            .max(heat_cue::HEAT_CUE_PREVIEW);
        let top_n: Vec<_> = candidates.iter().take(cap).cloned().collect();

        let mut caches = self.heat_caches.lock().unwrap_or_else(|e| e.into_inner());
        caches.by_range.upsert(
            range.start,
            heat_worker::RangeHeatEntry::new(stats, top_n),
            tiered::Tier::User,
        );
        caches.complete.insert(range, candidates);
    }
}

/// Standard ratatui popup-centering recipe: an `area`-relative `Rect`
/// `percent_x`% wide and `percent_y`% tall, centered within `area`.
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}


/// Drop the leading `offset` characters of the rendered line (spec 0113
/// D24's horizontal pan, composed with spec 0116 §7's syntax
/// highlighting) — skips `offset` characters across the whole span
/// sequence, trimming (not dropping the style of) whichever span the
/// skip boundary lands inside. The remainder is left for
/// `ratatui::Paragraph` to clip to the pane's width as usual, same as an
/// un-panned line.
fn pan_spans(spans: Vec<Span<'static>>, offset: usize) -> Vec<Span<'static>> {
    if offset == 0 {
        return spans;
    }
    let mut remaining = offset;
    let mut result = Vec::new();
    for span in spans {
        let char_count = span.content.chars().count();
        if remaining >= char_count {
            remaining -= char_count;
            continue;
        }
        let trimmed: String = span.content.chars().skip(remaining).collect();
        remaining = 0;
        result.push(Span::styled(trimmed, span.style));
    }
    result
}

/// Spec 0129 §G2/0131 §G2: write `text` to the real OS clipboard (plain
/// text only, no ANSI/colors). If `arboard` fails (e.g. no X11/Wayland
/// clipboard provider available, the common case over plain SSH),
/// additionally emits an OSC 52 escape sequence to stdout, best-effort —
/// a terminal-level (not X-server) fallback that many terminal
/// emulators honor transparently over SSH. The original `arboard`
/// error is still returned either way, so a caller distinguishing
/// "confirmed via arboard" from "best-effort via OSC 52" still can.
fn copy_to_clipboard(text: &str) -> Result<(), arboard::Error> {
    let result = arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text));
    if result.is_err() {
        emit_osc52_copy(text);
    }
    result
}

/// Read the OS clipboard as plain text, for the command line's paste.
///
/// There is no OSC 52 counterpart to `emit_osc52_copy` here: reading the
/// clipboard that way means writing a query and then *parsing the
/// terminal's reply out of the input stream*, racing every real
/// keystroke, and most terminals refuse the read half by default anyway.
/// A paste that only works with a local clipboard provider is the honest
/// shape of this.
fn clipboard_text() -> Result<String, arboard::Error> {
    arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text())
}

/// Spec 0131 §G2: emit `ESC ]52;c;{base64(text)}\x07` to stdout — the
/// OSC 52 clipboard-set sequence. No error is surfaced from this: there
/// is no terminal handshake/ack for OSC 52, so whether it was actually
/// honored can never be confirmed either way.
fn emit_osc52_copy(text: &str) {
    use base64::Engine;
    use std::io::Write;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let _ = write!(std::io::stdout(), "\x1b]52;c;{encoded}\x07");
    let _ = std::io::stdout().flush();
}

mod bake;
mod command_line;
mod event;
mod heat_cue;
mod heat_worker;
mod help_text;
mod key_dispatch;
mod lines;
mod manage_pane;
mod menu;
mod mouse;
use mouse::ClickZone;
mod navigation;
#[cfg(unix)]
mod neovim;
mod node_status;
mod override_apply;
mod override_cmd;
mod override_display;
mod override_export;
mod override_resolve;
mod override_select;
mod pane_scroll;
mod popup;
mod popup_doc;
mod popup_wire;
mod prefetch;
mod preview_truncate;
mod render;
mod script_pane;
mod search;
mod search_cursor;
mod selection;
mod shadow_sweep;
mod structure;
mod terminal;
mod tiered;
mod trace;
mod wire;

#[cfg(test)]
mod tests;
