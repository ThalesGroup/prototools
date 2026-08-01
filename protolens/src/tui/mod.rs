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

use std::collections::{HashMap, HashSet};
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

/// Fixed horizontal-pan step, in columns (spec 0113 D24) — a generous but
/// simple constant rather than a fraction of the pane's width, so panning
/// speed doesn't change as the pane is resized. Also used for Ctrl-Up/
/// Ctrl-Down vertical panning (`pan_vertical_by_step`).
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

/// How long `warm_up_heat_cues` (spec 0151 G8) waits, from its own
/// start, before drawing its first progress frame — avoids any flicker
/// for small/fast descriptor sets, where the whole pass is already
/// near-instant.
const WARMUP_FIRST_DRAW_DELAY: Duration = Duration::from_millis(300);

/// Minimum elapsed time between successive `warm_up_heat_cues` progress
/// redraws (spec 0151 G8) — time-based, not a fixed line-count interval,
/// so it stays responsive regardless of how expensive each individual
/// line turns out to be.
const WARMUP_REDRAW_INTERVAL: Duration = Duration::from_millis(300);

/// How long protolens may sit unattended before re-examining the
/// heat-cue activity byte (spec 0190 S7/G3). Bounds the activity dot's
/// staleness without requiring every path that drains the request queue
/// to emit an `AppEvent` — an event-maintained dot would freeze in
/// whatever state it was last drawn in, showing "busy" over an idle
/// worker.
///
/// Deliberately a flat interval rather than a timer armed per queue
/// event: queue events are far too frequent, and each one would
/// reschedule. The cost of a tick that changes nothing is two relaxed
/// atomic loads and a comparison — `run_loop` skips the frame entirely
/// (S8).
const ACTIVITY_TICK: Duration = Duration::from_millis(250);

/// Spec 0192 S3: the shortest interval between two frames drawn
/// *solely* because background scoring completed. Keystrokes, mouse
/// events, resizes and message/splash deadlines are never delayed by it
/// — only the repaint that shows a newly arrived cue is, and that
/// repaint is a visual refinement of a frame the user is already
/// looking at.
///
/// The worker notifies once per completed non-`Prefetch` request
/// (`heat_worker.rs`), and a fresh page queues one per unsettled
/// visible row, so without this a single keystroke costs a screenful of
/// full frames. At 50 ms a progressive fill costs at most 20 frames per
/// second, which is a refresh rate the user cannot distinguish from
/// immediate.
const HEAT_REPAINT_INTERVAL: Duration = Duration::from_millis(50);

/// Spec 0192 S4: read-ahead steps guaranteed per main-loop iteration,
/// regardless of how busy the event channel is. Read-ahead is still
/// opportunistic — the `TryRecvError::Empty` arm in `run_loop` spends
/// as many steps as the user's idleness allows — but the guarantee is
/// what stops a steady event stream from starving it entirely. A burst
/// of `HeatWorkerProgress` events does exactly that: without the floor,
/// read-ahead ran twice across a forty-keystroke session, both times
/// before the first keystroke.
///
/// Small enough that the guaranteed work cannot itself add perceptible
/// latency to a keystroke: one step is a walk of a few rows plus at
/// most one `TieredBounded::upsert`.
const PREFETCH_STEPS_PER_ITERATION: usize = 8;

/// Byte budget for `App::render_cache` (spec 0116 §8) — tuned
/// generously, since an interactive session is short-lived.
const RENDER_CACHE_MAX_BYTES: usize = 1 << 20;

/// Single source-of-truth command-name registry (spec 0113 D26) — backs
/// both `resolve_command`'s exact-match-wins prefix dispatch and the
/// command line's Tab-completion (`App::start_tab_completion`). Adding a
/// command here is the only step needed for it to get both, automatically
/// — but *only* those two: it still needs an arm in `run_command`, which
/// otherwise reports it as unimplemented.
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

/// Shared pan-by-`step` arithmetic behind the override and manage
/// panes' own Ctrl-Left/Ctrl-Right (`step == PAN_STEP`) and Shift+wheel/
/// native horizontal scroll (`step == WHEEL_PAN_STEP`) — like
/// `pan_by_step`, but bounded on the right by `max_offset` (each pane's
/// own `..._max_visible_line_len().saturating_sub(width)`), so it stops
/// once the rightmost character of the widest currently-visible row
/// would be shown, never further, as the main pane's `pan_right` does.
fn pan_by_step_clamped(offset: &mut usize, max_offset: usize, step: usize, left: bool) {
    *offset = if left {
        offset.saturating_sub(step)
    } else {
        (*offset + step).min(max_offset)
    };
}

/// Shared vertical pan-by-`step` arithmetic behind Ctrl-Up/Ctrl-Down and
/// the plain mouse wheel in the main, override, and manage panes —
/// mirrors `pan_by_step`'s horizontal pan: bounded only by the content
/// itself (`0` at the top, `max_scroll` — the highest offset that still
/// shows a full page — at the bottom), deliberately not by keeping the
/// cursor/highlight row in view. Bringing it back into view on its own
/// movement is `clamp_scroll_to_visible`'s job alone (see
/// `last_cursor_row` and friends). `step` is `PAN_STEP` for
/// Ctrl-Up/Ctrl-Down, `WHEEL_PAN_STEP` for the wheel.
fn pan_vertical_by_step(scroll: &mut usize, max_scroll: usize, step: usize, up: bool) {
    *scroll = if up {
        scroll.saturating_sub(step)
    } else {
        (*scroll + step).min(max_scroll)
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

/// Static key-binding reference shown by the `F1` help overlay (spec 0111
/// Annex C, spec 0113 D22) — kept as one flat text block rather than
/// generated from the `handle_key` match arms, so it can be phrased for
/// readability independent of the code's own binding order.
const HELP_TEXT: &[&str] = &[
    "protolens — key bindings",
    "",
    "Movement — the text first, then on past its edges into the tree",
    "  h / Left / Backspace",
    "                   caret one character left; at the first non-blank,",
    "                   fold this node, then move to its parent",
    "  l / Right        caret one character right; at the first non-blank",
    "                   of a folded node, unfold it; at the last",
    "                   reachable column, move to the first child",
    "  Alt-Left / Alt-h caret one word left",
    "  Alt-Right / Alt-l  caret one word right",
    "  0 / ^            caret to first non-blank",
    "  $                caret to last reachable column",
    "  %                jump between the cursor node's { and }",
    "  j / Down         next node (document order)",
    "  k / Up           previous node",
    "  J / Shift-Down   next sibling",
    "  K / Shift-Up     previous sibling",
    "  Home / gg        jump to first node",
    "  End / G          jump to last visible node",
    "  PageDown         scroll down one page",
    "  PageUp           scroll up one page",
    "  Ctrl-Left        pan main pane left",
    "  Ctrl-Right       pan main pane right",
    "  Ctrl-Up          pan main pane up, cursor stays in view",
    "  Ctrl-Down        pan main pane down, cursor stays in view",
    "  Shift+wheel / native horizontal scroll",
    "                   pan whichever pane (main, override, manage, or",
    "                   the command bar) the mouse is hovering",
    "  drag (main pane) select whole lines; release copies them to the",
    "                   OS clipboard; Esc clears the selection",
    "",
    "Fold / unfold",
    "  Space / za       toggle fold on the node under the cursor",
    "  zc / zo          close / open that node's fold",
    "  zA / zC / zO     the same three, for all siblings at this level",
    "  H / Shift-Left   fold all siblings at this level (as zC)",
    "  L / Shift-Right  unfold all siblings at this level (as zO)",
    "",
    "Display",
    "  a                toggle main-pane #@ annotation display",
    "",
    "Navigation history",
    "  Ctrl-O           jump back",
    "  Ctrl-I           jump forward",
    "",
    "Export",
    "  x                arm the export chord — xb/xp pre-fill \":export\"",
    "                   for binary/prototext data; xd arms a second",
    "                   chord — xdb/xdp pre-fill \":export\" for binary/",
    "                   prototext schema",
    "  :export [--binary|--prototext|--descriptor-binary|",
    "           --descriptor-prototext] <path>",
    "                   export the cursor node to <path> — default",
    "                   format is #@ prototext text; --descriptor-binary/",
    "                   --descriptor-prototext export a synthetic schema",
    "                   for the cursor's live-tree children instead of",
    "                   its data",
    "",
    "Command line",
    "  Tab              complete the command name (longest common prefix,",
    "                   then cycle through matches)",
    "  Shift-Tab        cycle backward through matches",
    "",
    "Search (main pane, requires main-pane focus)",
    "  /                search forward for a pattern (matches against the",
    "                   current, possibly-overridden rendering)",
    "  ?                search backward",
    "  n                repeat the last search, same direction",
    "  N                repeat the last search, opposite direction",
    "  (a hit puts the caret on the match itself)",
    "  (confirming / or ? with no typed pattern reuses the last one)",
    "",
    "Override pane",
    "  t                open/close the override pane for the message/",
    "                   group node under the cursor",
    "  Tab              move focus between the main pane and the",
    "                   override pane (while it is open)",
    "  i                toggle candidate sort: inferred score (default)",
    "                   or alphanumeric",
    "  /  ?  n  N       search / search backward / repeat / repeat",
    "                   opposite direction (pane focused)",
    "  j/k, PageUp/Down, Home/End   move the highlighted candidate",
    "                   (pane focused)",
    "  Ctrl-Left/Ctrl-Right         pan the pane horizontally",
    "  Ctrl-Up/Ctrl-Down            pan the pane vertically, highlight",
    "                   stays in view",
    "  Enter            apply the highlighted type (pane focused) and",
    "                   close the pane; also records the override in the",
    "                   collection (see \"Override management\" below)",
    "  Esc              cancel and close the override pane",
    "  :type-as <FQDN>  apply <FQDN> as the cursor node's type override,",
    "                   bypassing the pane",
    "  :type-as-raw     mark the cursor node's range as explicitly raw/",
    "                   unschema'd, bypassing the pane",
    "",
    "Override management",
    "  o                open/close the override management pane (closes",
    "                   the override pane, if open)",
    "  Enter            close the management pane",
    "  Tab              move focus between the main pane and the",
    "                   management pane (while it is open)",
    "  j/k, PageUp/Down, Home/End   move the highlighted entry",
    "  Shift-Up / Shift-Down        move the highlighted entry and",
    "                   activate the destination (deactivating any",
    "                   other entry sharing its origin)",
    "  Left/Right       move the main-pane cursor to the prev/next field",
    "                   affected by the highlighted entry (wraps around)",
    "  Ctrl-Left/Ctrl-Right         pan the pane horizontally",
    "  Ctrl-Up/Ctrl-Down            pan the pane vertically, highlight",
    "                   stays in view",
    "  /  ?  n  N       search / search backward / repeat / repeat",
    "                   opposite direction",
    "  a / Space        toggle the highlighted entry active/inactive",
    "  A / Shift-Space  same, but also cascades the new state to every",
    "                   entry whose origin sits at-or-under it (a",
    "                   descendant path, or a path-field/fqdn-field at",
    "                   the same path); when several entries would",
    "                   activate under one origin, only the first",
    "                   (sorted) one does — Shift-Space needs terminal",
    "                   support (Kitty keyboard protocol); also",
    "                   available as Shift-click or double-click on an",
    "                   entry's marker",
    "  z / Z            rotate the highlighted entry's origin kind",
    "                   forward/backward: path, path-field, fqdn-field;",
    "                   auto-resolves from the fields the entry affects,",
    "                   falling back to the main-pane cursor/message line",
    "                   when ambiguous; repeating z/Z with the cursor",
    "                   unchanged advances to the next kind instead of",
    "                   getting stuck",
    "  d / Delete / Backspace  remove the highlighted entry from the",
    "                   collection (an auto-derived entry still in scope",
    "                   is deactivated instead, since deleting it would",
    "                   just recreate it)",
    "  D                duplicate the highlighted entry (the copy starts",
    "                   inactive and is always manual, even if the",
    "                   original was auto-derived)",
    "  entry rows: auto-derived entries are plain, manual entries bold",
    "  s                pre-fill \":save <default path>\"",
    "  r                pre-fill \":restore \"",
    "  :save <path>",
    "                   write the whole override collection to <path>",
    "                   as YAML",
    "  :restore <path>",
    "                   replace the override collection wholesale with",
    "                   <path>'s contents (entries that no longer",
    "                   resolve are silently dropped; a target-hash",
    "                   mismatch warns but does not block)",
    "  Tab              complete a filesystem path (save/restore",
    "                   argument)",
    "  (management-pane actions never change the current rendering —",
    "  only Enter in the override pane does)",
    "",
    "Other",
    "  F1               toggle this help",
    "  q                quit (press again to confirm)",
    "  Ctrl-Z           suspend (fg to resume) — Unix only",
    "",
    "j/k or PageUp/PageDown scroll this help; q, Esc, or F1 closes it.",
];

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
    Committed(usize),
    Overlay(usize),
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
    lines: Vec<String>,
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
    /// Spec 0160 G2: running total of line-count deltas accumulated by
    /// `splice_override` calls in the current `render_overrides` batch.
    /// Always `0` outside of an active batch.
    pending_shift: isize,
    /// Spec 0167 (N1 follow-up to spec 0160): line-buffer patches
    /// collected by `splice_override` calls during the currently-active
    /// `render_overrides` batch — see `override_apply::LinePatch`'s own
    /// doc comment, and `render_overrides_inner`'s `patch_scope`
    /// parameter, for why a patch can be nested inside another,
    /// not-yet-materialized one. Always empty outside of an active
    /// batch; drained and applied to `self.lines` in one pass by
    /// `finalize_override_batch`.
    pending_line_patches: Vec<override_apply::LinePatch>,
    /// Spec 0186 S2: the lowest line index any patch in the current batch
    /// touches, in the batch-corrected frame `materialize_line_patches`
    /// resolves against — i.e. the first line whose *content* or *owner*
    /// this batch can possibly have changed. `None` when the batch queued
    /// no patches at all.
    ///
    /// This is the boundary that makes `finalize_override_batch`'s
    /// repair incremental: everything strictly below it keeps both its
    /// content and its index, so it needs no work. Always `None` outside
    /// of an active batch.
    ///
    /// Note that it is *not* interchangeable with `pending_shift != 0`:
    /// a batch can shift without patching, and can patch with a net
    /// shift of zero while still changing which node owns a line. See
    /// `finalize_override_batch`.
    pending_patch_min_line: Option<usize>,
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
    /// Spec 0210 S3: the `(absolute line, owner)` pairs of the rows the
    /// last frame drew, ascending. A viewport-sized snapshot, not a
    /// document-sized index — it replaces nothing and is authoritative
    /// for nothing; `line_pos` consults it and falls back to its
    /// descent on a miss. See `App::set_window_nodes`.
    window_nodes: Vec<(usize, LinePos)>,
    /// The `structural_version` `window_nodes` was filled at. Anything
    /// older is ignored, since a fold or a commit moves lines out from
    /// under it.
    window_nodes_version: u64,
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
    /// `tree` — `Pending` until a cache check (only ever attempted on a
    /// worker-progress wakeup or a real redraw-triggering input event)
    /// resolves it; `Resolved` nodes are read directly, no cache lock.
    heat_states: Vec<heat_cue::HeatState>,
    /// Node indices with an outstanding worker request (spec 0152 G6/
    /// G8) — populated by `heat_cue_resolve` whenever it finds a node
    /// still unsettled with a worker present, drained by
    /// `recheck_pending_heat_states`. Bounds that recheck to the
    /// handful of nodes actually awaiting an answer instead of the whole
    /// document: a full `0..heat_states.len()` scan on every
    /// `HeatWorkerProgress` event costs over a second per event on a
    /// 635k-node document, compounding into tens of seconds whenever a
    /// burst of requests completes at once.
    pending_heat_recheck: HashSet<usize>,
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
    /// Incremented on every fold/unfold and every commit — i.e. every
    /// time a rendered line number may have shifted. `App::
    /// prefetch_step`'s staleness signal (spec 0164 G7) for restarting
    /// its walk, and (spec 0210 S3) the guard on `window_nodes`.
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
            lines: decoded.lines,
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
            pending_shift: 0,
            pending_line_patches: Vec::new(),
            pending_patch_min_line: None,
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
            window_nodes: Vec::new(),
            window_nodes_version: 0,
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
            pending_heat_recheck: HashSet::new(),
            prefetch_walk: PrefetchWalk::exhausted(),
            prefetch_trace: PrefetchTrace::default(),
            activity_shown: None,
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
            heat_worker::RangeHeatEntry {
                best_score: stats.best_score,
                best_count: stats.best_count,
                top_n,
            },
            tiered::Tier::User,
        );
        caches.complete = Some((range, candidates));
    }
}

/// Spec 0191 G1: how many rows one read-ahead wave may visit before it
/// reports `Idle` and lets both threads park. Without it the walk's only
/// stopping condition is running off *both* ends of the document, so a
/// single cursor move sweeps every expanded row there is — keeping the
/// main thread in `prefetch_step`'s `Progressed` branch (never reaching
/// `recv_timeout`) and the worker in back-to-back `score_all` calls, on
/// a large `FileDescriptorSet` for tens of thousands of rows.
///
/// A budget across both ends, not a reach limit per side, so a cursor
/// near the top of the document still gets its full allowance downward.
///
/// Deliberately *not* derived from `HEAT_REQUEST_QUEUE_MAX_ENTRIES`,
/// which happens to hold the same number (spec 0191 N1): that one bounds
/// requests outstanding against the worker, this one bounds rows visited
/// per wave. Raising one to smooth out stalls must not silently double
/// the other's reach.
const PREFETCH_WALK_MAX_ROWS: usize = 2048;

/// Spec 0191 G2. Past the result cache's capacity a prefetch result
/// evicts *itself* on insert — `evict_one` takes `prefetch_current`'s
/// tail, the same end `upsert` inserts at — so the worker would pay a
/// whole `score_all` for an answer nobody can ever read. This is the
/// one relation the walk budget genuinely has to another constant.
const _: () = assert!(PREFETCH_WALK_MAX_ROWS <= heat_cue::HEAT_CACHE_MAX_ENTRIES);

/// Spec 0164 G7: per-`App` zigzag-walk state, persisted across
/// `run_loop` iterations (not rebuilt on every call) — reset only when
/// the cursor's display row or the document's structural/reflow state
/// (`App::structural_version`) has changed since the walk began.
/// `origin_line`/`above`/`below` are visible-row numbers (rendered-line
/// space), not raw `App::lines` indices — folded/hidden content has no
/// row, so the walk naturally skips it.
struct PrefetchWalk {
    origin_line: usize,
    above: usize,
    below: usize,
    above_done: bool,
    below_done: bool,
    /// Spec 0210 S3: the two ends as *positions* rather than as row
    /// numbers, stepped one visible line at a time by `prefetch_step`.
    ///
    /// The row numbers `next_row` produces would each have to be turned
    /// back into a node by a `visible_row_pos` descent, and a wave
    /// visits up to `PREFETCH_WALK_MAX_ROWS` of them — on the reference
    /// corpus that is 2 048 crossings of the root's 7 771 children, on
    /// the UI thread. Carrying the positions makes the whole wave one
    /// descent (when it is seeded) plus O(1) per row.
    ///
    /// `None` before the first seeding, and on a document with no rows
    /// at the origin at all.
    above_pos: Option<LinePos>,
    below_pos: Option<LinePos>,
    /// `App::structural_version` as of this walk's start — part of
    /// the staleness signal `prefetch_step` checks on entry (the
    /// exact mechanism the spec left TBD during implementation).
    structural_version: u64,
}

impl PrefetchWalk {
    /// Both ends already exhausted, and an `origin_line`/
    /// `structural_version` that can never coincide with a real walk's
    /// — guarantees the very first `prefetch_step` call starts a fresh
    /// walk.
    fn exhausted() -> Self {
        PrefetchWalk {
            origin_line: usize::MAX,
            above: 0,
            below: 0,
            above_done: true,
            below_done: true,
            above_pos: None,
            below_pos: None,
            structural_version: u64::MAX,
        }
    }

    /// Advances the walk to the next unexplored row, alternating above/
    /// below (always the nearer of the two unexplored ends), and
    /// returns its visible-row number. `None` once both ends
    /// are exhausted, or once the wave has spent its
    /// `PREFETCH_WALK_MAX_ROWS` budget (spec 0191 S1).
    ///
    /// The budget bounds rows *visited*, not requests *pushed*:
    /// `prefetch_step` skips non-header, non-overridable and
    /// already-settled rows without pushing, and it is the visiting
    /// loop — not the pushing — that holds the main thread.
    fn next_row(&mut self, visible_len: usize) -> Option<usize> {
        loop {
            if self.above_done && self.below_done {
                return None;
            }
            // `above`/`below` count steps taken on each end, so their
            // sum is exactly the number of rows returned so far. Both
            // `_done` flags are set rather than returning `None`
            // directly, so the walk has a single "exhausted" state and
            // the next call takes the early-out above without
            // re-deriving the budget.
            if self.above + self.below >= PREFETCH_WALK_MAX_ROWS {
                self.above_done = true;
                self.below_done = true;
                return None;
            }
            let try_above = if self.above_done {
                false
            } else if self.below_done {
                true
            } else {
                self.above <= self.below
            };
            if try_above {
                let next = self.above + 1;
                if next > self.origin_line {
                    self.above_done = true;
                    continue;
                }
                self.above = next;
                return Some(self.origin_line - next);
            } else {
                let next = self.below + 1;
                let row = self.origin_line + next;
                if row >= visible_len {
                    self.below_done = true;
                    continue;
                }
                self.below = next;
                return Some(row);
            }
        }
    }
}

pub(super) enum PrefetchStep {
    Progressed,
    Idle,
}

/// What one read-ahead wave did, for `PROTOLENS_TRACE`. A wave is the
/// span between two `prefetch_walk` resets, which is exactly the span
/// between two cursor moves — so one line per wave answers "how much did
/// that keystroke actually cost the read-ahead".
///
/// `rows` counts candidates the zigzag visited; `skipped` those it
/// dismissed without a lookup (no node on the line, not overridable, or
/// already settled — the last of which is the "already known" case);
/// `hits` those already in the result cache; `pushes` those it had to
/// queue for the worker. `hits` + `skipped` versus `pushes` is the
/// number the "most of it should already be cached" intuition is about.
///
/// `busy` is the sum of the time actually spent inside `prefetch_step`,
/// not the wall time the wave spanned: the walk is interleaved with
/// drawing and event handling, so wall time mostly measures the rest of
/// the loop and says nothing about what read-ahead costs.
#[derive(Default)]
struct PrefetchTrace {
    busy: Duration,
    rows: u32,
    skipped: u32,
    hits: u32,
    pushes: u32,
    live: bool,
    reported: bool,
}

impl PrefetchTrace {
    fn restart(&mut self) {
        *self = Self {
            live: true,
            ..Self::default()
        };
    }

    fn report(&mut self, why: &str) {
        if self.reported || !self.live {
            return;
        }
        self.reported = true;
        trace::trace!(
            "wave {why} rows={} skipped={} hits={} pushes={} busy_ms={:.1}",
            self.rows,
            self.skipped,
            self.hits,
            self.pushes,
            self.busy.as_secs_f64() * 1000.0,
        );
    }
}

impl App {
    /// Advances the zigzag prefetch walk by one candidate, pushing it
    /// at `Tier::Prefetch` if it isn't already settled/cached (spec
    /// 0164 G7). First resets `self.prefetch_walk` to a fresh walk
    /// from the cursor's current display row if either the cursor's
    /// row or `self.structural_version` has changed since the walk
    /// began — superseding the in-progress wave in the request queue
    /// and both `HeatCaches` maps before the reset; otherwise resumes
    /// exactly where the previous call left off. Returns `Idle` once
    /// the document is fully walked, the last push returned
    /// `UpsertOutcome::Rejected` (G6), or no worker is running at all
    /// (nothing to prefetch into).
    /// Spec 0190 S4: what the activity dot reports — the highest-
    /// priority tier the heat-cue subsystem is working for, or `None`
    /// when it is idle or no worker is running at all. Lock-free: two
    /// relaxed atomic loads.
    pub(in crate::tui) fn heat_activity(&self) -> Option<tiered::Tier> {
        self.heat_worker.as_ref().and_then(|w| w.activity())
    }

    pub(super) fn prefetch_step(&mut self) -> PrefetchStep {
        let entered_at = Instant::now();
        let step = self.prefetch_step_inner();
        self.prefetch_trace.busy += entered_at.elapsed();
        step
    }

    fn prefetch_step_inner(&mut self) -> PrefetchStep {
        if self.heat_worker.is_none() {
            return PrefetchStep::Idle;
        }
        let origin_row = self.cursor_display_row();
        if self.prefetch_walk.origin_line != origin_row
            || self.prefetch_walk.structural_version != self.structural_version
        {
            // All three supersede with the same O(1) splice, and spec
            // 0189 keeps it that way deliberately: this runs on the UI
            // thread, so the restart path must not walk a wave. What
            // differs is the fate of the demoted entries. A cache entry
            // is a *result* — a later hit on it saves a whole sweep, so
            // it stays servable. A queue entry is *pending work* — an
            // unpaid sweep on a range ranked from an origin the cursor
            // has left — so the worker discards it rather than scoring
            // it (`pop_highest` never reaches `prefetch_previous`).
            self.prefetch_trace.report("superseded");
            self.prefetch_trace.restart();
            if let Some(worker) = &self.heat_worker {
                worker.start_new_wave();
            }
            {
                let mut caches = self.heat_caches.lock().unwrap_or_else(|e| e.into_inner());
                caches.by_range.start_new_wave();
                caches.current_score.start_new_wave();
            }
            // Spec 0210 S3: the one descent this wave pays. Both ends
            // start on the origin's own row and step outward from it,
            // so nothing below has to resolve a row number again.
            let origin_pos = self.visible_row_pos(origin_row).map(|(pos, _)| pos);
            self.prefetch_walk = PrefetchWalk {
                origin_line: origin_row,
                above: 0,
                below: 0,
                above_done: false,
                below_done: false,
                above_pos: origin_pos,
                below_pos: origin_pos,
                structural_version: self.structural_version,
            };
        }

        loop {
            let Some(row) = self.prefetch_walk.next_row(self.visible_row_count()) else {
                self.prefetch_trace.report("exhausted");
                return PrefetchStep::Idle;
            };
            self.prefetch_trace.rows += 1;
            // `next_row` advances exactly one end per call, and always
            // by one row, so which end it just moved is readable from
            // the row alone — and stepping that end's position by one
            // visible line reproduces the row it named.
            let going_up = row < self.prefetch_walk.origin_line;
            let from = if going_up {
                self.prefetch_walk.above_pos
            } else {
                self.prefetch_walk.below_pos
            };
            let stepped = from.and_then(|pos| {
                if going_up {
                    self.prev_visible(pos)
                } else {
                    self.next_visible(pos)
                }
            });
            let Some((pos, _)) = stepped else {
                // The walk's own bounds should have stopped it first;
                // if they somehow did not, close that end rather than
                // spinning on it.
                if going_up {
                    self.prefetch_walk.above_done = true;
                } else {
                    self.prefetch_walk.below_done = true;
                }
                self.prefetch_trace.skipped += 1;
                continue;
            };
            if going_up {
                self.prefetch_walk.above_pos = Some(pos);
            } else {
                self.prefetch_walk.below_pos = Some(pos);
            }
            // Heat is a property of the node, so only its header row
            // asks for it: a closing brace has none of its own, and the
            // later rows of a packed run would each re-ask the same
            // question (spec 0216 S7).
            if pos.line_in_node != 0 {
                self.prefetch_trace.skipped += 1;
                continue;
            }
            let idx = pos.node;
            if !self.can_override(idx) || self.heat_states[idx].settled() {
                self.prefetch_trace.skipped += 1;
                continue;
            }
            let range = {
                let node = &self.tree[idx].span;
                extract::message_payload_range(&self.blob, &node.raw_range)
            };
            let current_key = self.current_type_key(idx);
            let (_, outcome) = self.heat_lookup_ex(
                &range,
                current_key.as_deref(),
                0,
                heat_cue::HEAT_CUE_PREVIEW,
                tiered::Tier::Prefetch,
            );
            match outcome {
                None => self.prefetch_trace.hits += 1,
                Some(_) => self.prefetch_trace.pushes += 1,
            }
            return match outcome {
                Some(tiered::UpsertOutcome::Rejected) => {
                    self.prefetch_trace.report("rejected");
                    PrefetchStep::Idle
                }
                _ => PrefetchStep::Progressed,
            };
        }
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

/// Drain any input events already queued in the terminal's input buffer
/// before disabling raw mode.
///
/// `EnableMouseCapture` always turns on any-motion reporting (crossterm
/// gives no way to opt out short of hand-rolling the escape sequences — see
/// the comment on `handle_mouse`'s `Moved` guard), so a mouse move happening
/// in the split second between the app's last `event::read()` and raw mode
/// being disabled here would otherwise sit unread in the pty's input queue.
/// Once cooked-mode echo comes back on, the tty driver echoes those queued
/// bytes straight to the screen as raw escape-sequence garbage (e.g.
/// `^[[<35;60;17M`) that the shell then needs an Enter/Ctrl-C to clear.
/// Reading them here, while raw mode (no echo) is still active, discards
/// them silently instead.
fn drain_pending_input() {
    let deadline = Instant::now() + Duration::from_millis(60);
    while Instant::now() < deadline {
        match term_event::poll(Duration::from_millis(15)) {
            Ok(true) => {
                let _ = term_event::read();
            }
            _ => break,
        }
    }
}

/// Set by `push_keyboard_enhancement` once it has actually pushed Kitty
/// keyboard-protocol enhancement flags, so `pop_keyboard_enhancement`
/// knows whether the matching pop is needed — `restore_terminal`/
/// `suspend` are free functions with no `App` to carry this as ordinary
/// state, and popping when nothing was pushed would either do nothing
/// (unsupported terminals just ignore the unknown escape sequence) or,
/// worse, pop a flag set some other way. Single-threaded (one terminal,
/// one event loop), so `Ordering::Relaxed` is enough.
static KITTY_KEYBOARD_ENHANCED: AtomicBool = AtomicBool::new(false);

/// Push `DISAMBIGUATE_ESCAPE_CODES` (Kitty keyboard protocol) if the
/// terminal supports it — without it, legacy terminal escape sequences
/// carry no modifier parameter for printable keys, so Shift-Space is
/// reported identically to plain Space (unlike arrow/function keys,
/// which already carry one, e.g. `ESC [1;2A` for Shift-Up).
/// `supports_keyboard_enhancement` queries the terminal and
/// blocks briefly waiting for its response — fine here since it only
/// ever runs before the main event loop starts (`run`) or during a
/// suspend/resume cycle (`suspend`), never concurrently with
/// `event::read`/`poll`. On terminals that don't support it this is a
/// no-op: `handle_manage_key`'s guarded `Char(' ') if SHIFT` arm simply
/// never fires there.
fn push_keyboard_enhancement() -> io::Result<()> {
    if supports_keyboard_enhancement().unwrap_or(false) {
        execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
        KITTY_KEYBOARD_ENHANCED.store(true, Ordering::Relaxed);
    }
    Ok(())
}

/// Undo `push_keyboard_enhancement`, if it actually pushed anything.
fn pop_keyboard_enhancement() {
    if KITTY_KEYBOARD_ENHANCED.swap(false, Ordering::Relaxed) {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
}

/// Restore the terminal to its normal (cooked, main-screen, no mouse
/// capture, visible cursor) state — shared by `run`'s own cleanup and the
/// panic hook below, so a panic mid-session doesn't leave the user's
/// terminal unusable.
///
/// The body is the exact inverse of the setup sequence in `run` and in
/// `enable_raw_mode_and_reenter` — mouse capture and the alternate screen
/// undone first, then the keyboard enhancement, then raw mode — so that
/// each undo runs in the state its counterpart was issued in.
/// `drain_pending_input` is the exception, and comes first: it has to read
/// while raw mode is still on.
///
/// `Show` is here rather than at the call sites because `Terminal::draw`
/// leaves the hardware cursor hidden unless the frame set a position, so
/// *any* path out of the TUI owes the shell a visible cursor — including
/// the panic hook, which has no `Terminal` to ask.
fn restore_terminal() {
    drain_pending_input();
    let _ = execute!(
        io::stdout(),
        DisableMouseCapture,
        LeaveAlternateScreen,
        Show
    );
    pop_keyboard_enhancement();
    let _ = disable_raw_mode();
}

/// `Ctrl-Z` suspend (spec 0113 D31): leave the terminal in the same clean
/// state a normal exit would (mirroring `restore_terminal`/the panic
/// hook), raise `SIGTSTP` on this process, and — once `fg` sends
/// `SIGCONT` and execution resumes right here — re-enter the alternate
/// screen/mouse-capture/raw-mode trio and force a full redraw, since the
/// terminal's actual contents are unknown after a suspend/resume cycle
/// (another program may have used the same terminal in between).
#[cfg(unix)]
fn suspend<B: Backend>(terminal: &mut Terminal<B>) -> io::Result<()>
where
    io::Error: From<B::Error>,
{
    restore_terminal();
    // SAFETY: raising a signal on our own process is always sound.
    unsafe {
        libc::raise(libc::SIGTSTP);
    }
    enable_raw_mode_and_reenter(terminal)
}

/// Re-enter raw mode/keyboard-enhancement/alternate-screen/mouse-capture
/// and force a full redraw — the shared tail of `suspend()`'s own
/// resume path (spec 0113 D31) and `neovim::open_editor`'s Neovim-
/// handoff resume path (spec 0144 G5): both leave the terminal via
/// `restore_terminal()`, hand control to something else, then need this
/// exact same re-entry sequence once control returns.
#[cfg(unix)]
pub(crate) fn enable_raw_mode_and_reenter<B: Backend>(terminal: &mut Terminal<B>) -> io::Result<()>
where
    io::Error: From<B::Error>,
{
    enable_raw_mode()?;
    push_keyboard_enhancement()?;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    terminal.clear()?;
    Ok(())
}

/// Where a panic on a thread other than the UI thread is left for
/// `run_loop` to find. Holds the *first* one; a later panic on another
/// background thread is usually the first one's consequence.
static BACKGROUND_PANIC: Mutex<Option<String>> = Mutex::new(None);

/// The one line of a panic worth putting in a message row: what was said
/// and where. The default hook's backtrace has nowhere to go here — the
/// alternate screen is still up and belongs to the document.
fn describe_panic(info: &std::panic::PanicHookInfo<'_>) -> String {
    let what = info
        .payload()
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
        .unwrap_or("(no message)");
    let thread = std::thread::current().name().unwrap_or("?").to_string();
    match info.location() {
        Some(loc) => format!("thread '{thread}' panicked at {loc}: {what}"),
        None => format!("thread '{thread}' panicked: {what}"),
    }
}

/// Take whatever the panic hook recorded, if anything.
fn take_background_panic() -> Option<String> {
    BACKGROUND_PANIC
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
}

/// Run the interactive TUI loop against a real terminal.
pub fn run(app: &mut App) -> io::Result<()> {
    // A panic mid-session (e.g. an indexing bug) would otherwise unwind
    // straight out of this function, skipping the cleanup below and
    // leaving the terminal stuck in raw/alt-screen/mouse-capture mode.
    // Restore it first, then hand off to the default panic printer.
    //
    // Flaw C4: installed *before* the first fallible call, not after
    // terminal setup — otherwise a panic during setup unwinds with raw
    // mode already on and no hook installed to undo it.
    //
    // A panic hook is process-global, so this one runs on whichever
    // thread panicked — including the heat worker and the sweep shards it
    // spawns. Only the UI thread owns the terminal, and only its panic is
    // actually unwinding towards the cleanup below; restoring the
    // terminal for anyone else would drop a live session out of the
    // alternate screen and then print over what replaced it. So the two
    // cases are separated by thread id.
    let ui_thread = std::thread::current().id();
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().id() == ui_thread {
            restore_terminal();
            default_hook(info);
            return;
        }
        let mut slot = BACKGROUND_PANIC.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            *slot = Some(describe_panic(info));
        }
    }));

    // Terminal setup, up to and including the `Terminal` itself. Every
    // line after the first is fallible *with raw mode already on*, and
    // `terminal` does not exist yet, so this window cannot be covered by
    // the cleanup block at the end of this function — it gets its own
    // captured `Result` instead.
    let setup = (|| -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
        enable_raw_mode()?;
        push_keyboard_enhancement()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        Terminal::new(CrosstermBackend::new(stdout))
    })();
    let mut terminal = match setup {
        Ok(terminal) => terminal,
        Err(e) => {
            restore_terminal();
            let _ = std::panic::take_hook();
            return Err(e);
        }
    };

    let (tx, rx) = mpsc::channel();
    // `Option`-wrapped so `run_loop` can `take()` it and shut it down
    // around the Neovim handoff below — see `run_loop`'s own doc comment
    // on that block for why.
    let mut input_reader = Some(event::InputReaderHandle::spawn(tx.clone()));

    // Flaw C4: everything fallible from here to `run_loop` lives inside
    // this closure, whose `Result` is *captured* rather than propagated.
    // A `?` in here returns from the closure, so the cleanup block below
    // is reached on every path by construction rather than by review —
    // a bare `terminal.size()?` or `warm_up_heat_cues(..)?` would return
    // straight out of `run`, leaving the terminal in raw/alternate-
    // screen/mouse-capture mode for whatever ran next. Any future `?`
    // added to this region is covered for free, which is the point.
    let result = (|| -> io::Result<()> {
        // Safe upper bound (spec 0151 G6/G8): the real, render-computed
        // `override_list_height` (set by the override pane's own first
        // render, `render.rs`) is always <= the raw terminal height, since
        // it's an inner-area height net of borders/header rows. Setting it
        // eagerly here means G6's cross-population cap isn't stuck at `1`
        // (its `App::new` default) for the warm-up pass or any ordinary
        // browsing before the user's first `t` press.
        app.override_list_height = terminal.size()?.height.max(1) as usize;

        // Spec 0152 G1: spawned only when a scoring graph is loaded — with
        // no graph, `app.heat_worker` stays `None` for the whole session,
        // and every fork that checks `heat_worker.is_some()` falls through
        // to the existing synchronous logic. Spawned *before* `warm_up_
        // heat_cues` below so that its initial-viewport pass can push
        // requests onto the worker's queue and return immediately
        // instead of scoring synchronously on this thread — warming up
        // with no worker to hand work off to costs a multi-second black
        // screen at startup against a large scoring graph.
        if let Some(graph) = &app.ctx.graph {
            // Spec 0180 S2: an owning handle, not a `&'static` copied out of the
            // mapping. Each `Arc::clone` below is a refcount bump, and it is what
            // makes both spawns below independent of when `App` drops.
            let graph = Arc::clone(graph);
            // Spec 0216 S28: a refcount bump, not a copy of the whole
            // blob, since the worker can share ownership of it.
            let blob = Arc::clone(&app.blob);

            // Spec 0168: no second, detached "root-type" thread here.
            // `decode::decode` resolves the type before rendering, so by
            // the time `App` exists the document is already what it is —
            // nothing re-scores the blob and re-renders the whole
            // document through the splice machinery underneath a reader
            // who has already started browsing.
            // Spec 0217 S6: the worker sweeps while the main thread is
            // drawing, so it gets the budget less the one thread the
            // main loop is already spending — never less than 1, which
            // is the un-sharded sweep this has always been.
            let worker_jobs = app.sweep_jobs.saturating_sub(1).max(1);
            app.heat_worker = Some(heat_worker::HeatWorkerHandle::spawn(
                Arc::clone(&app.heat_caches),
                graph,
                blob,
                tx.clone(),
                worker_jobs,
            ));
        }

        warm_up_heat_cues(&mut terminal, app)?;

        run_loop(&mut terminal, app, &rx, &mut input_reader, &tx)
    })();

    // Spec 0152 G9: both threads joined, unconditionally (see "Shutdown
    // and safety"). Not load-bearing for *memory* safety — the worker
    // owns an `Arc<LoadedGraph>` (spec 0180 S2), so it cannot observe an
    // unmapped page whether or not this runs. Joining is what stops the
    // worker writing to the terminal after `restore_terminal` below, and
    // what keeps the process from lingering on a background sweep the
    // user has already quit.
    if let Some(worker) = app.heat_worker.take() {
        worker.shutdown();
    }
    if let Some(reader) = input_reader.take() {
        reader.shutdown();
    }
    let _ = std::panic::take_hook();
    restore_terminal();

    result
}

/// One-time warm-up pass (spec 0151 G8) priming heat cues for the
/// initial viewport before the first `run_loop` iteration. With the
/// spec-0152 worker already spawned by the time this runs, each
/// `heat_cue_for` call below either hits the cache or pushes a request
/// onto the worker's queue and returns immediately — this loop scores
/// nothing itself, which is what keeps startup against a large scoring
/// graph from being a multi-second black screen. Still runs while heat
/// cues are hidden (`i`): the background fetch/cache is worth priming
/// regardless of whether a cue is currently shown — only
/// `heat_cue_for`'s return value is suppressed while hidden, not the
/// underlying work.
fn warm_up_heat_cues<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> io::Result<()>
where
    io::Error: From<B::Error>,
{
    if app.ctx.graph.is_none() {
        return Ok(());
    }
    let rows = terminal.size()?.height as usize;
    let lines: Vec<usize> = app
        .visible_window(0, rows)
        .into_iter()
        .map(|(line, _)| line)
        .collect();
    let start = Instant::now();
    let mut last_draw = start;
    for (i, &line_idx) in lines.iter().enumerate() {
        app.heat_cue_for(line_idx); // populates the caches; return value unused here
        let now = Instant::now();
        if now.duration_since(start) > WARMUP_FIRST_DRAW_DELAY
            && now.duration_since(last_draw) > WARMUP_REDRAW_INTERVAL
        {
            app.message = format!(
                "Computing inference cues for the initial view: {}/{} lines scored...",
                i + 1,
                lines.len()
            );
            terminal.draw(|frame| app.render(frame))?;
            last_draw = now;
        }
    }
    if Instant::now().duration_since(start) > WARMUP_FIRST_DRAW_DELAY {
        app.message.clear();
    }
    Ok(())
}

fn run_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    rx: &mpsc::Receiver<event::AppEvent>,
    input_reader: &mut Option<event::InputReaderHandle>,
    tx: &mpsc::Sender<event::AppEvent>,
) -> io::Result<()>
where
    io::Error: From<B::Error>,
{
    // Spec 0190 S8: an activity tick that changes nothing must not cost
    // a frame. The gate has to sit *above* `terminal.draw` — skipping
    // work inside `render` would not work, because ratatui resets the
    // back buffer every frame, so a span that is not re-emitted is
    // erased rather than preserved.
    let mut redraw = true;
    // `PROTOLENS_TRACE` only: which of the three gate clauses below
    // forced this frame. Without it a trace full of back-to-back draws
    // says how much they cost but not who asked for them.
    let mut redraw_why = "initial";
    // Spec 0191 S3: a two-window sliding maximum of the activity level.
    // `Tier` is `Ord` (`Prefetch < Visible < User`) and `Option<T>`
    // orders `None < Some(_)`, so "highest seen" is just `max`. One
    // window is one iteration of this loop; the dot shows
    // `max(previous, current)`, so a rise lands on the very next frame
    // while a fall has to be confirmed by two consecutive quiet windows.
    //
    // Both windows are *reset*, never merely accumulated. An earlier
    // draft kept a single high-water mark reset only at a draw, which
    // deadlocked: with the mark stuck high and the dot already showing
    // that level, the gate below saw no difference, so no draw
    // happened, so the mark was never reset — the dot stayed lit
    // forever after a read-ahead wave finished.
    let mut activity_window: Option<tiered::Tier> = None;
    let mut activity_prev_window: Option<tiered::Tier> = None;
    // Spec 0192 S3: a completed heat request marks the frame dirty
    // instead of forcing one. `last_heat_frame` is stamped by *any*
    // draw, not only a heat-driven one — a frame the user just caused
    // has already shown whatever cues had arrived, so the interval
    // should run from it.
    let mut heat_dirty = false;
    let mut last_heat_frame = Instant::now();
    loop {
        // A background thread died (see the hook in `run`). Say so, and
        // let go of the worker: its thread is gone, so every request
        // still queued and every one pushed from here on would go
        // unanswered and leave the cue at `[?]` forever. Dropped, the
        // cue path falls back to computing on this thread — slower, but
        // it answers.
        if let Some(panic) = take_background_panic() {
            app.message = format!("background thread failed, cues are now inline: {panic}");
            if let Some(worker) = app.heat_worker.take() {
                worker.shutdown();
            }
            redraw = true;
        }
        if redraw {
            app.activity_shown = activity_prev_window.max(activity_window);
            let drawn_at = Instant::now();
            terminal.draw(|frame| app.render(frame))?;
            heat_dirty = false;
            last_heat_frame = drawn_at;
            trace::trace!("draw {redraw_why} us={}", drawn_at.elapsed().as_micros());
            // Sample again immediately after the draw. `render` is
            // itself a producer — `heat_cue_for` pushes a `Visible`
            // request per unsettled visible row — so the requests this
            // draw just queued belong to the current window. Sampling
            // only before the draw is what made the dot emit one dark
            // frame per completed request (G4).
            activity_window = activity_window.max(app.heat_activity());
        }
        // While a status message is pending auto-dismissal
        // (`message_deadline`, `track_message_timeout`) or the splash
        // screen hasn't yet auto-dismissed (`splash_deadline`), the
        // receive below must wake by that deadline, so that the next
        // `render()` — which is what actually clears an expired message
        // or splash — runs even with no further event. Spec 0152 G8:
        // the input-reader thread owns the `event::poll`/`event::read()`
        // pair and forwards through `rx` alongside the worker thread's
        // own progress notifications, so this loop sleeps until there's
        // a reason to wake instead of polling on a fixed schedule.
        let splash_deadline = app.splash.then_some(app.splash_deadline);
        let ui_deadline = match (app.message_deadline, splash_deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(d), None) | (None, Some(d)) => Some(d),
            (None, None) => None,
        };
        // Spec 0190 S7: the activity tick is a third candidate deadline
        // alongside those two. Because it is always present the receive
        // below is always a `recv_timeout` and never a bare `rx.recv()`:
        // the loop wakes four times a second forever rather than ever
        // being genuinely idle. Each such wake costs two relaxed loads
        // and a comparison, and no frame.
        let activity_deadline = Instant::now() + ACTIVITY_TICK;
        let mut deadline = ui_deadline.map_or(activity_deadline, |d| d.min(activity_deadline));
        // Spec 0192 S3: a deferred heat repaint is a fourth candidate
        // deadline. Without it, a completion arriving inside the
        // interval would be shown only when some unrelated event next
        // woke the loop — the same class of self-deadlock spec 0191 S3
        // hit, and the activity tick would merely bound it at 250 ms
        // rather than remove it.
        if heat_dirty {
            deadline = deadline.min(last_heat_frame + HEAT_REPAINT_INTERVAL);
        }
        // Spec 0192 S4: read-ahead's guaranteed slice, taken before the
        // receive loop below rather than only in its `Empty` arm.
        for _ in 0..PREFETCH_STEPS_PER_ITERATION {
            if matches!(app.prefetch_step(), PrefetchStep::Idle) {
                break;
            }
        }
        // Spec 0164 G7: interleave one unit of inline prefetch work
        // with a non-blocking channel check on every idle-wait
        // iteration, so read-ahead never holds the thread for more
        // than a single `TieredBounded::upsert` push before yielding
        // to a pending event. Falls back to the deadline-aware receive
        // above only once `prefetch_step` reports `Idle` (walk
        // exhausted or capacity-rejected).
        let received = loop {
            match rx.try_recv() {
                // A bare mouse-move carries no user intent (`handle_
                // mouse` already discards it after dequeuing), but
                // `EnableMouseCapture` makes the terminal send one on
                // essentially every pixel the pointer crosses. Treating
                // it as "a real event" here would starve prefetching
                // any time the mouse merely hovers over the window, so
                // it's discarded transparently at this level too,
                // without breaking out of the loop.
                Ok(event::AppEvent::Term(Event::Mouse(m))) if m.kind == MouseEventKind::Moved => {
                    continue
                }
                Ok(ev) => break Some(ev),
                Err(mpsc::TryRecvError::Empty) => match app.prefetch_step() {
                    PrefetchStep::Progressed => {
                        // Yielding to a pending event is not enough:
                        // every deadline computed above — an expiring
                        // message, the splash's auto-dismiss, a due
                        // heat repaint, the activity tick — comes due
                        // with no event to announce it, and this loop
                        // is the only thing between them and the frame
                        // that honors them. Without the check,
                        // read-ahead holds the thread until it runs dry
                        // and all four are simply late. Breaking with
                        // no event is exactly the timeout case the
                        // `Idle` arm below produces, and the
                        // `*_forces` tests just past the loop already
                        // know what to do with it.
                        if Instant::now() >= deadline {
                            break None;
                        }
                        continue;
                    }
                    PrefetchStep::Idle => {
                        // Spec 0203 ran incremental arena compaction
                        // here, strictly behind read-ahead. Spec 0216
                        // deletes it: the arena is a function of the
                        // bytes and never grows, so there is nothing
                        // left to compact.
                        let timeout = deadline.saturating_duration_since(Instant::now());
                        break rx.recv_timeout(timeout).ok(); // timeout elapsed => None
                    }
                },
                // Cannot fire as the code stands: this loop holds `tx`
                // for the Neovim handoff's reader respawn below, so a
                // sender outlives every receive here. Reported as an
                // error rather than as `Ok(())` all the same — if that
                // invariant is ever broken the session has lost its
                // input, and exiting 0 in silence is the one answer that
                // is certainly wrong.
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "the input channel disconnected",
                    ));
                }
            }
        };
        // Spec 0190 S8. Three reasons to draw, checked here rather than
        // after the dispatch below because that block can `return`, and
        // because nothing in it can change the activity byte without
        // also having produced the event that already forces a redraw.
        //
        // Spec 0191 S4: the third reason compares the debounced value,
        // not a fresh probe. A fresh probe forces a frame on every
        // trough between two requests; folding them into the sliding
        // maximum keeps high-frequency toggling from costing main-thread
        // frames during exactly the period when the worker wants the
        // CPU.
        activity_window = activity_window.max(app.heat_activity());
        // Spec 0192 S3: a completed heat request is the one event that
        // does not force its own frame. Its *state* is still applied
        // immediately by the dispatch below — only the repaint waits.
        if matches!(received, Some(event::AppEvent::HeatWorkerProgress)) {
            heat_dirty = true;
        }
        let event_forces =
            received.is_some() && !matches!(received, Some(event::AppEvent::HeatWorkerProgress));
        let heat_forces = heat_dirty && Instant::now() >= last_heat_frame + HEAT_REPAINT_INTERVAL;
        let deadline_forces = ui_deadline.is_some_and(|d| Instant::now() >= d);
        let activity_forces = activity_prev_window.max(activity_window) != app.activity_shown;
        redraw = event_forces || heat_forces || deadline_forces || activity_forces;
        redraw_why = if event_forces {
            match &received {
                Some(event::AppEvent::Term(Event::Key(_))) => "key",
                Some(event::AppEvent::Term(Event::Mouse(_))) => "mouse",
                _ => "term",
            }
        } else if heat_forces {
            "heat"
        } else if deadline_forces {
            "deadline"
        } else {
            "activity"
        };
        // Close the window. Everything above sampled into
        // `activity_window`; from here it is history, and the next
        // iteration starts clean. This is the reset the earlier
        // single-high-water-mark draft lacked.
        activity_prev_window = activity_window;
        activity_window = None;
        match received {
            // Some Kitty-protocol-aware terminals report a `Release`
            // event for a keystroke in addition to `Press`, even though
            // this app only requests `DISAMBIGUATE_ESCAPE_CODES` (not
            // `REPORT_EVENT_TYPES`). A `Release` event dispatched through
            // `handle_key` after its matching `Press` already changed
            // focus (e.g. `t`/`q`/`Esc`/`Tab` closing the override pane)
            // would land in the *new* focus's handler instead of being a
            // no-op — surfacing as a keypress "leaking" into both panes.
            // Ignore anything but `Press`/`Repeat`.
            Some(event::AppEvent::Term(Event::Key(key))) if key.kind != KeyEventKind::Release => {
                let dispatched_at = Instant::now();
                app.handle_key(key);
                trace::trace!(
                    "key {:?} us={}",
                    key.code,
                    dispatched_at.elapsed().as_micros()
                );
            }
            Some(event::AppEvent::Term(Event::Mouse(mouse))) => app.handle_mouse(mouse),
            Some(event::AppEvent::Term(_)) => {}
            Some(event::AppEvent::HeatWorkerProgress) => {
                app.recheck_pending_heat_states();
                app.poll_pending_override_work();
            }
            None => {} // deadline elapsed with nothing received
        }
        if app.should_quit {
            return Ok(());
        }
        #[cfg(unix)]
        if app.should_suspend {
            app.should_suspend = false;
            suspend(terminal)?;
        }
        #[cfg(unix)]
        if let Some(req) = app.pending_editor_open.take() {
            // `open_editor` backgrounds this process's own process group
            // relative to the terminal (`tcsetpgrp(io::stdin(),
            // nvim_pgid)`) for as long as Neovim owns the foreground.
            // The input-reader thread (spec 0152 G8) is otherwise
            // permanently blocked in `event::read()` on that same stdin
            // — a background process's read from its controlling
            // terminal draws SIGTTIN, whose default disposition stops
            // the *whole process* (every thread, not just this one),
            // with nothing left to `SIGCONT` it back. Shutting it down
            // before the handoff and respawning a fresh one right after
            // `open_editor` reclaims the terminal is what keeps anything
            // else from touching stdin while Neovim has it.
            if let Some(reader) = input_reader.take() {
                reader.shutdown();
            }
            neovim::open_editor(terminal, app, req)?;
            *input_reader = Some(event::InputReaderHandle::spawn(tx.clone()));
        }
    }
}

mod command_line;
mod event;
mod heat_cue;
mod heat_worker;
mod key_dispatch;
mod lines;
mod manage_pane;
mod mouse;
mod navigation;
#[cfg(unix)]
mod neovim;
mod override_apply;
mod override_select;
mod render;
mod structure;
mod tiered;
mod trace;

#[cfg(test)]
mod tests;
