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
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
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
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use prototext_core::serialize::render_text::{
    decode_and_render, decode_and_render_indexed, DecodeRenderOpts, FqdnId, FqdnTable,
    NO_PACKED_RECORD,
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
pub(crate) use lines::LinePos;

use crate::export_descriptor;
use crate::extract::{self, ExtractFormat};
use crate::override_pane::{self, OverrideKind, OverrideOrigin, SortMode};
use crate::provenance::{ProvenanceTable, NOT_RENDERED};
use crate::render_cache::RenderCache;
use crate::theme::{self, ThemeKind};
use help_text::HELP_TEXT;
use prefetch::{PrefetchStep, PrefetchTrace, PrefetchWalk};
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
/// prompt or a pending `q` quit confirmation (see that function's doc
/// comment).
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

/// Byte budget for `App::render_cache` (spec 0116 §8) — tuned
/// generously, since an interactive session is short-lived.
const RENDER_CACHE_MAX_BYTES: usize = 1 << 20;

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
    "type-as",
    "type-as-raw",
    "save",
    "restore",
    "proto-root",
];

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

/// Shared wrap-around search order behind the override- and manage-pane
/// `/`/`?`/`n` commands (`jump_to_override_match`, `jump_to_manage_match`):
/// checks each of the `n` 0-based indices, starting at `start` and moving
/// in `dir`, wrapping around exactly once; returns the first index for
/// which `matches` returns true, or `None` if none did.
fn search_wrap(
    n: usize,
    start: usize,
    dir: SearchDir,
    mut matches: impl FnMut(usize) -> bool,
) -> Option<usize> {
    for d in 0..n {
        let i = match dir {
            SearchDir::Forward => (start + d) % n,
            SearchDir::Backward => (start + n - d) % n,
        };
        if matches(i) {
            return Some(i);
        }
    }
    None
}

/// Spec 0195 S2: the pattern behind every pane's `/`/`?`/`n`, carrying
/// vim's `smartcase` rule — an all-lowercase pattern matches
/// case-insensitively, a pattern containing any uppercase character
/// matches exactly.
///
/// The rule is decided once, at construction, rather than per candidate:
/// the three searches this replaces each lowercased *both* sides inside
/// their walk, which is how they came to allocate two `String`s per node
/// on the one hot path spec 0195 S1 is about.
pub(super) struct SearchPattern {
    /// Already lowercased when the search is case-insensitive, so the
    /// per-candidate comparison only has to fold the haystack.
    needle: String,
    case_sensitive: bool,
}

impl SearchPattern {
    pub(super) fn new(pattern: &str) -> Self {
        let case_sensitive = pattern.chars().any(char::is_uppercase);
        let needle = if case_sensitive {
            pattern.to_string()
        } else {
            pattern.to_lowercase()
        };
        Self {
            needle,
            case_sensitive,
        }
    }

    pub(super) fn is_match(&self, haystack: &str) -> bool {
        self.find(haystack).is_some()
    }

    /// Byte offset of the first match in `haystack`, which spec 0194 S8
    /// needs so a search hit can put the caret on the match rather than
    /// merely on its row.
    pub(super) fn find(&self, haystack: &str) -> Option<usize> {
        if self.case_sensitive {
            return haystack.find(&self.needle);
        }
        haystack
            .char_indices()
            .find(|&(i, _)| starts_with_folded(&haystack[i..], &self.needle))
            .map(|(i, _)| i)
    }
}

/// Whether `haystack`, lowercased one `char` at a time, begins with
/// `needle` — which the caller has already lowercased.
///
/// Folding as we compare is what keeps `is_match` allocation-free (spec
/// 0195 G4). It goes through `char::to_lowercase`, which yields an
/// iterator rather than a `char`, because a few characters lowercase to
/// more than one (`İ` to `i` plus a combining dot) and a one-to-one fold
/// would silently misalign the comparison from there on.
fn starts_with_folded(haystack: &str, needle: &str) -> bool {
    let mut folded = haystack.chars().flat_map(char::to_lowercase);
    let mut wanted = needle.chars();
    loop {
        let Some(w) = wanted.next() else {
            return true;
        };
        if folded.next() != Some(w) {
            return false;
        }
    }
}

/// Shared clamp arithmetic behind the override- and manage-pane highlight
/// movement (`move_override_highlight`, `move_manage_highlight`): moves
/// `current` by `delta`, staying within `0..=max`.
fn clamp_highlight(current: usize, delta: isize, max: usize) -> usize {
    (current as isize + delta).clamp(0, max as isize) as usize
}

/// Shared scroll-to-keep-target-visible arithmetic behind the main,
/// override, and manage panes' own render passes: nudges `*scroll` by the
/// minimum amount needed to keep `target` within the `height`-row visible
/// window. No-op when `height` is `0`.
fn clamp_scroll_to_visible(scroll: &mut usize, target: usize, height: usize) {
    if height == 0 {
        return;
    }
    if target < *scroll {
        *scroll = target;
    } else if target >= *scroll + height {
        *scroll = target + 1 - height;
    }
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

/// Shared pan-by-`step` arithmetic for every pan bounded at both ends:
/// the override and manage panes' Ctrl-Left/Ctrl-Right, and
/// Ctrl-Up/Ctrl-Down plus the plain wheel in the main, override and
/// manage panes. `step` is `PAN_STEP` for the keys, `WHEEL_PAN_STEP`
/// for the wheel; `backward` is left or up depending on the axis.
///
/// Like `pan_by_step`, but bounded on the far side by `max_offset` —
/// horizontally each pane's own `..._max_visible_line_len().
/// saturating_sub(width)`, so it stops once the rightmost character of
/// the widest currently-visible row would be shown and never further,
/// as the main pane's `pan_right` does; vertically the highest scroll
/// offset that still shows a full page.
///
/// Bounded by the content and nothing else — deliberately *not* by
/// keeping the cursor/highlight row in view. Bringing it back into
/// view on its own movement is `clamp_scroll_to_visible`'s job alone
/// (see `last_cursor_row` and friends).
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
fn viewport_label(first_visible: usize, height: usize, total: usize) -> String {
    if total <= height {
        return "All".to_string();
    }
    if first_visible == 0 {
        return "Top".to_string();
    }
    if first_visible + height >= total {
        return "Bot".to_string();
    }
    format!("{}%", first_visible * 100 / (total - height))
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
enum SearchDir {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandLineKind {
    Command,
    Search(SearchDir),
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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct CursorPos {
    node: usize,
    line_in_node: u32,
    column: usize,
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
    node_text: Vec<Option<Box<str>>>,
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
    /// Resolved color theme (spec 0116 §9) — fixed for the session, never
    /// `ThemeKind::System` (resolved once in `main.rs` before `App::new`).
    theme: ThemeKind,
    tree: Vec<TreeNode>,
    /// The blob's structural decomposition (spec 0216 S1), built once at
    /// load and never rebuilt: it is a function of the bytes, and the
    /// bytes do not change. Every interpretation's `tree` is a pruning of
    /// it, which is what lets it be immutable while `tree` is not.
    arena: Arena,
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
    /// Mouse-driven whole-line selection in the main pane (spec 0129
    /// §G1) — `line_idx` of the row a drag started on; `None` when no
    /// selection is active. Never affects `self.cursor`, which only ever
    /// moves via the initial `Down` click (`handle_click`, unchanged).
    select_anchor: Option<usize>,
    /// `line_idx` of the row the drag is currently over (or ended on);
    /// `None`/`None` together with `select_anchor` means no selection.
    /// Equal to `select_anchor` for a plain click with no drag.
    select_end: Option<usize>,
    /// Timestamp + `line_idx` of the most recent main-pane left-click
    /// `Down` event — compared against on the next `Down` to recognize a
    /// double-click (same line, within `DOUBLE_CLICK_THRESHOLD`). `None`
    /// before the first click.
    last_click: Option<(Instant, usize)>,
    /// Whether the click currently in progress (`Down` already handled,
    /// matching `Up` not yet seen) was recognized as the second click of
    /// a double-click — consulted by the `Up` handler to decide whether a
    /// plain (non-dragged) click should deselect (`false`, the default)
    /// or keep the single-line selection `Down` just set (`true`).
    pending_double_click: bool,
    folded: HashSet<usize>,
    scroll_offset: usize,
    /// `cursor_display_row()`'s value as of the last render pass that
    /// applied `clamp_scroll_to_visible` to `scroll_offset` — compared
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
    /// the pane opened on a bare main-pane node — means the plain
    /// `path` default of `override_origin_for_kind` (spec 0208 S2).
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
    /// Scroll offset (in rows, the pinned raw entry included) for the
    /// override pane's candidate list.
    override_scroll: usize,
    /// `override_highlight`'s value as of the last render pass that
    /// applied `clamp_scroll_to_visible` to `override_scroll` (mirrors
    /// `last_cursor_row`) — reset to `None` everywhere `override_scroll`
    /// is itself reset to `0` (opening the pane, recomputing
    /// candidates), guaranteeing a clamp on the next render even if the
    /// new highlight happens to coincide with the old one.
    last_override_highlight: Option<usize>,
    /// Last confirmed in-pane search (direction, pattern) — `n` repeats it
    /// in the same direction.
    last_override_search: Option<(SearchDir, String)>,
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
    /// Incremented on every fold/unfold and every commit — i.e. every
    /// time a rendered line number may have shifted. `App::
    /// prefetch_step`'s staleness signal (spec 0164 G7) for restarting
    /// its walk.
    structural_version: u64,
    /// `true` while `recompute_override_candidates`'s `SortMode::
    /// Inferred` branch is waiting on a worker request for the
    /// override pane's first page (spec 0152 G7).
    override_candidates_pending: bool,
    /// `true` while `upgrade_active_override_to_complete` is waiting on
    /// a worker request for a wider window (spec 0152 G7).
    override_complete_pending: bool,
    /// `true` while the main-pane heat cue (spec 0138) is toggled off by
    /// `i` — hides the cue without discarding `heat_caches`.
    heat_cues_hidden: bool,
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
    /// Scroll offset (in rows) for the management pane's listing.
    manage_scroll: usize,
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
    /// Last confirmed management-pane in-pane search — `n` repeats it.
    last_manage_search: Option<(SearchDir, String)>,
    /// `Some` while `f` in the management pane is editing the highlighted
    /// entry's display-name override (spec 0119 G4) — pre-filled with its
    /// current `name` (empty if `None`), mutually exclusive with an
    /// in-progress `command_buffer` search.
    manage_rename: Option<String>,
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
    /// Spec 0194 S6: set by a `z` press, consumed by the next key —
    /// vim's fold prefix (`za`/`zc`/`zo` and their sibling-wide
    /// capitals). Same shape as `pending_g` above.
    pending_z: bool,
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
    /// Last confirmed main-pane in-pane search (direction, pattern) — `n`
    /// repeats it in the same direction; an empty `/`/`?` confirmation
    /// reuses the pattern (spec 0114 §4, mirroring `last_override_search`).
    last_search: Option<(SearchDir, String)>,
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
    /// Scroll offset (in `HELP_TEXT` lines) while the help overlay is open.
    help_scroll: usize,
    /// Help overlay's inner (bordered-away) `Rect` as of the last
    /// `render_help()` call — used to hit-test mouse wheel/Shift-wheel
    /// events against the overlay instead of letting them fall through
    /// to whichever pane it happens to be drawn on top of. Only
    /// meaningful while `help_open`.
    help_area: Rect,
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
    /// call (spec 0147 G4), `None` when neither `command_buffer` nor
    /// `manage_rename` is active — used to hit-test mouse hover for
    /// Shift+wheel/native horizontal pan the same way `main_area`/
    /// `side_area` do.
    cmd_area: Option<Rect>,
    pub message: String,
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
    /// Mirrors `self.message` as of the last `track_message_timeout()`
    /// call — used to detect a freshly-set message (`self.message` has
    /// no dedicated setter; it's assigned directly all over this file),
    /// so `message_deadline` gets (re)armed exactly once per message,
    /// not on every frame.
    last_message_seen: String,
    /// Wall-clock time at which the current `self.message` should be
    /// auto-dismissed, `None` while `self.message` is empty. Consulted
    /// (and cleared) only by `track_message_timeout`, which never fires
    /// while a text-entry prompt (`command_buffer`/`manage_rename`) or a
    /// pending `q` quit confirmation is actively awaiting a keypress.
    message_deadline: Option<Instant>,
    pub should_quit: bool,
    /// `true` right after a first `q` press asks for confirmation; a
    /// second `q` press (any mode) actually quits, any other key cancels.
    /// Checked centrally at the top of `handle_key`, ahead of every other
    /// dispatch, so it applies uniformly regardless of focus.
    quit_confirm: bool,
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
}

impl App {
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
        let header = format!("protolens — {blob_label} — {}", decoded.root_type);
        // Spec 0216 S1: the arena is in level order and slot 0 is the
        // wrapper, so the document-order first node is the first slot —
        // no search for the node with no `doc_prev` is needed.
        let cursor = 0;
        // Spec 0117 §1: seed the root `path` override with whatever type
        // was explicitly requested or inferred; if neither is available,
        // seed nothing at all — an untyped root has no override worth
        // recording, and the collection is legitimately empty until the
        // user adds one. `decode::decode` uses the "<raw / no type>"
        // sentinel for the "neither" case rather than an `Option`.
        let root_override_type = if decoded.root_type == "<raw / no type>" {
            None
        } else {
            Some(decoded.root_type.clone())
        };
        let mut overrides = override_pane::OverrideCollection::new();
        if root_override_type.is_some() {
            overrides.seed_root(root_override_type.clone());
        }
        let mut app = App {
            blob: decoded.blob,
            wrapper_offset: decoded.wrapper_offset,
            blob_path,
            // Always on (spec 0133): a pure main-pane display attribute
            // from here on, toggled at runtime by the `a` key.
            annotations: true,
            indent_size,
            node_text: decoded.node_text,
            window_styles: Vec::new(),
            theme,
            tree: decoded.tree,
            arena: decoded.arena,
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
            select_end: None,
            last_click: None,
            pending_double_click: false,
            folded: HashSet::new(),
            scroll_offset: 0,
            last_cursor_row: None,
            pan_offset: 0,
            override_pan_offset: 0,
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
            override_scroll: 0,
            last_override_highlight: None,
            last_override_search: None,
            term_width: 0,
            override_list_height: 0,
            heat_caches: Arc::new(Mutex::new(heat_worker::HeatCaches::new(
                heat_cue::HEAT_CACHE_MAX_ENTRIES,
            ))),
            heat_worker: None,
            heat_states: vec![heat_cue::HeatState::default(); tree_len],
            prefetch_walk: PrefetchWalk::exhausted(),
            prefetch_trace: PrefetchTrace::default(),
            activity_shown: None,
            input_pending: false,
            structural_version: 0,
            override_candidates_pending: false,
            override_complete_pending: false,
            heat_cues_hidden: false,
            render_cache: RenderCache::new(RENDER_CACHE_MAX_BYTES),
            active_override_range: None,
            override_inferred_raw: Vec::new(),
            override_candidates_complete: false,
            override_seek_target: None,
            overrides,
            manage_open: false,
            manage_focus: false,
            manage_highlight: 0,
            manage_scroll: 0,
            last_manage_highlight: None,
            last_manage_click: None,
            last_manage_row_click: None,
            last_manage_search: None,
            manage_rename: None,
            manage_pending_kind: None,
            manage_list_height: 0,
            back_stack: Vec::new(),
            fwd_stack: Vec::new(),
            first_node: cursor,
            pending_g: false,
            pending_z: false,
            pending_x: ExportChord::None,
            command_buffer: None,
            command_kind: CommandLineKind::Command,
            command_cursor: 0,
            last_search: None,
            completion: None,
            splash: true,
            splash_deadline: Instant::now() + SPLASH_TIMEOUT,
            help_open: false,
            help_scroll: 0,
            help_area: Rect::default(),
            header,
            main_area: Rect::default(),
            side_area: Rect::default(),
            cmd_area: None,
            message: fallback_warning,
            refusals: Vec::new(),
            last_message_seen: String::new(),
            message_deadline: None,
            should_quit: false,
            quit_confirm: false,
            should_suspend: false,
            proto_root,
            #[cfg(unix)]
            pending_editor_open: None,
            #[cfg(unix)]
            editor_state: neovim::EditorState::NotRunning,
        };
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
            app.tree[cursor].rendered_as = root_provenance;
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
        caches.complete = Some((range, candidates));
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

/// Fills the gaps between `hints` (sorted, non-overlapping byte ranges
/// into `content`) with `None`-tagged segments, so the result covers all
/// of `content` — the input `App::spans_with_insertions` needs to build a
/// complete `Vec<Span>` for a line (spec 0116 §7).
fn segment_line(
    content: &str,
    hints: &[(Range<usize>, SyntaxRole)],
) -> Vec<(Range<usize>, Option<SyntaxRole>)> {
    let mut segments = Vec::new();
    let mut pos = 0;
    for (range, role) in hints {
        if range.start > pos {
            segments.push((pos..range.start, None));
        }
        segments.push((range.clone(), Some(*role)));
        pos = range.end;
    }
    if pos < content.len() {
        segments.push((pos..content.len(), None));
    }
    segments
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

mod command_line;
mod event;
mod heat_cue;
mod heat_worker;
mod help_text;
mod key_dispatch;
mod lines;
mod manage_pane;
mod mouse;
mod navigation;
#[cfg(unix)]
mod neovim;
mod override_apply;
mod override_display;
mod override_export;
mod override_resolve;
mod override_select;
mod prefetch;
mod preview_truncate;
mod render;
mod structure;
mod terminal;
mod tiered;
mod trace;

#[cfg(test)]
mod tests;
