// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Minimal v1 navigate + extract slice: single scrollable pane, cursor/fold
//! state, document-order / sibling-skip / parent / child movement, a
//! jumplist, mouse wheel/click, and a vim-style `:export`/`x` command line
//! — spec 0111 §2/§4, Annex B, Annex C. No override picker yet.

use std::collections::{HashMap, HashSet};
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

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
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use ratatui::{Frame, Terminal};

use prototext_core::serialize::render_text::{
    decode_and_render, decode_and_render_indexed, DecodeRenderOpts,
};

use crate::colorize::{self, LineStyles, SyntaxRole};
use crate::decode::{self, Decoded, DescriptorContext, TreeNode};
use crate::export_descriptor;
use crate::extract::{self, ExtractFormat};
use crate::override_pane::{self, OverrideKind, OverrideOrigin, SortMode};
use crate::render_cache::RenderCache;
use crate::theme::{self, ThemeKind};

/// Fixed horizontal-pan step, in columns (spec 0113 D24) — a generous but
/// simple constant rather than a fraction of the pane's width, so panning
/// speed doesn't change as the pane is resized. Also used for Ctrl-Up/
/// Ctrl-Down vertical panning (`pan_vertical_by_step`).
const PAN_STEP: usize = 8;

/// Vertical-pan step for the plain mouse wheel (2026-07-19 feedback item
/// 2) — one row per notch, matching the granularity of the cursor/
/// highlight movement the wheel used to perform before this change,
/// unlike `PAN_STEP`'s larger Ctrl-Up/Ctrl-Down/Shift+wheel jump.
const WHEEL_PAN_STEP: usize = 1;

/// Minimum terminal width (columns) below which `t` refuses to open the
/// override pane (spec 0114 §2) — lowered from the original 100 (spec
/// 0111 Annex C's own Phase-5 threshold) to 60 (2026-07-19 feedback),
/// since the borderless-pane layout (spec 0147) needs less horizontal
/// room than the bordered one this threshold was originally set for.
const MIN_OVERRIDE_WIDTH: u16 = 60;

/// Maximum gap between two same-line `Down(MouseButton::Left)` events for
/// the second to count as a double-click (feedback, 2026-07-15) —
/// crossterm reports `Down` identically for single and double clicks, so
/// the app disambiguates them itself by comparing consecutive `Down`
/// timestamps/positions (`App::last_click`).
const DOUBLE_CLICK_THRESHOLD: Duration = Duration::from_millis(500);

/// Whether a click identified by `key` (a main-pane line index, or a
/// manage-pane entry index), arriving now, is the second half of a
/// double-click against `last`'s previously recorded click — same `key`,
/// within `DOUBLE_CLICK_THRESHOLD` — updating `last` to this click either
/// way. Generic form of the main pane's own same-line-within-threshold
/// check (`last_click`/`pending_double_click`, spec-0129-era), reused by
/// the manage pane's radio-marker double-click (2026-07-17 feedback): an
/// alternative to Shift-click, which most terminal emulators intercept
/// for native text selection before it ever reaches the app.
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

/// Auto-dismiss delay for the startup splash screen (2026-07-17
/// feedback, item 13), in addition to its existing keypress/mouse
/// dismissal — mirrors `MESSAGE_TIMEOUT`'s deadline-based approach.
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

/// Stack size for every thread protolens spawns that runs the scorer
/// (spec 0180 S4). Declared *here*, in the parent of both spawn sites,
/// rather than in `heat_worker.rs` where it started life: it lived next
/// to one of its two users, so the other one — the detached root-type
/// sweep in `run` — silently never got it (rendering-flaws C3(b)).
///
/// `score_message_multi` (`prototext-graph`) recurses once per nested
/// message/group level of whatever candidate type it is scoring a range
/// against; on a schema mismatch (e.g. decoding a range against a
/// candidate whose shape doesn't actually match the bytes) this can run
/// noticeably deeper than a well-formed decode of the same bytes would.
/// `MAX_WIRE_DEPTH`'s doc comment (`prototext-core`) records the
/// measurement: the scorer consumes ~576 KiB for a full-cap nest in
/// release, so `std::thread::spawn`'s 2 MiB default leaves a 3.6×
/// margin — the binding margin in the whole workspace, and negative in
/// a debug build. 16 MiB restores the ~28× every other (walker, thread)
/// pair already has, and comfortably exceeds the main thread's own
/// default (commonly 8 MiB, per the process's `RLIMIT_STACK`) rather
/// than merely matching it. A stack reservation costs address space,
/// not resident pages.
pub(super) const SCORING_THREAD_STACK_SIZE: usize = 16 * 1024 * 1024;

/// Byte budget for `App::render_cache` (spec 0116 §8) — tuned generously
/// for a short-lived interactive session (spec 0114 §6's original
/// `candidate_cache` reasoning, since superseded by spec 0152's
/// entry-count-bounded `HeatCaches`, still applies to this one).
const RENDER_CACHE_MAX_BYTES: usize = 1 << 20;

/// Single source-of-truth command-name registry (spec 0113 D26) — backs
/// both `resolve_command`'s exact-match-wins prefix dispatch and the
/// command line's Tab-completion (`App::start_tab_completion`). Adding a
/// command here is the only step needed for it to get both, automatically.
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
/// (2026-07-19 feedback: the wheel always pans by `1`, unlike Ctrl-
/// Left/Ctrl-Right's `PAN_STEP`) for every caller today, since the
/// command bar has no Ctrl-Left/Ctrl-Right binding of its own (it
/// already auto-pans to keep the text cursor in view).
fn pan_by_step(offset: &mut usize, step: usize, left: bool) {
    *offset = if left {
        offset.saturating_sub(step)
    } else {
        offset.saturating_add(step)
    };
}

/// Shared pan-by-`step` arithmetic behind the override and manage
/// panes' own Ctrl-Left/Ctrl-Right (`step == PAN_STEP`) and Shift+wheel/
/// native horizontal scroll (`step == WHEEL_PAN_STEP`, 2026-07-19
/// feedback) — like `pan_by_step`, but bounded on the right by
/// `max_offset` (each pane's own `..._max_visible_line_
/// len().saturating_sub(width)`), so it stops once the rightmost
/// character of the widest currently-visible row would be shown, never
/// further — mirroring the main pane's own `pan_right`, which already
/// had this bound.
fn pan_by_step_clamped(offset: &mut usize, max_offset: usize, step: usize, left: bool) {
    *offset = if left {
        offset.saturating_sub(step)
    } else {
        (*offset + step).min(max_offset)
    };
}

/// Shared vertical pan-by-`step` arithmetic behind Ctrl-Up/Ctrl-Down and
/// the plain mouse wheel in the main, override, and manage panes
/// (2026-07-19 feedback item 1) — mirrors `pan_by_step`'s horizontal pan:
/// bounded only by the content itself (`0` at the top, `max_scroll` — the
/// highest offset that still shows a full page — at the bottom), no
/// longer tied to keeping the cursor/highlight row in view. Supersedes
/// the previous 2026-07-18 "cursor must never leave view" bound —
/// bringing the cursor/highlight back into view on its own movement is
/// now `clamp_scroll_to_visible`'s job alone (see `last_cursor_row` and
/// friends). `step` is `PAN_STEP` for Ctrl-Up/Ctrl-Down, `WHEEL_PAN_STEP`
/// for the wheel (item 2).
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
/// active/inactive statusline highlight groups, rather than a border
/// color as this crate used before spec 0147.
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
/// truncated; if `left` would overlap it, `left` is cut short with a
/// single trailing `<` marker at the cut point instead (mirrors vim's
/// `%<`).
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
        let mut line: String = left_chars.into_iter().take(budget - 1).collect();
        line.push('<');
        line.push_str(&right);
        line
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
enum SearchDir {
    Forward,
    Backward,
}

impl SearchDir {
    /// The opposite direction — `p` (previous) repeats the last search
    /// in the reverse direction, vim's `N` counterpart to `n`.
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
    "Movement",
    "  j / Down         next node (document order)",
    "  k / Up           previous node",
    "  J / Shift-Down   next sibling",
    "  K / Shift-Up     previous sibling",
    "  h / Left         close node, or move to parent",
    "  l / Right        open node, or move to first child",
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
    "  z / Space        toggle fold on the node under the cursor",
    "  H / Shift-Left   fold all siblings at this level",
    "  Shift-Right      unfold all siblings at this level",
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
    "  p                repeat the last search, opposite direction",
    "  (confirming / or ? with no typed pattern reuses the last one)",
    "",
    "Override pane",
    "  t                open/close the override pane for the message/",
    "                   group node under the cursor",
    "  Tab              move focus between the main pane and the",
    "                   override pane (while it is open)",
    "  i                toggle candidate sort: inferred score (default)",
    "                   or alphanumeric",
    "  /  ?  n  p       search / search backward / repeat / repeat",
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
    "  /  ?  n  p       search / search backward / repeat / repeat",
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
/// time. It is *not* part of the document: `tree`, `lines`,
/// `line_styles`, `line_to_node` and `visible_rows` are all untouched by
/// it, which is what makes a preview cost only the (byte-bounded) decode
/// and render of the target's own interior.
///
/// `first_row`/`covered_rows` are positions in `visible_rows`, computed
/// once when the overlay is built. They stay valid because S5 locks
/// focus to the selection pane for the overlay's whole lifetime: the
/// only two things that rebuild `visible_rows` — folding and splicing —
/// are unreachable while it is up. A terminal resize changes the pane
/// height only, not `visible_rows`, so it is harmless.
pub(super) struct PreviewOverlay {
    /// Index into `visible_rows` of the first row the committed target's
    /// `text_range` covers. For a packed run that target is the run's
    /// leader and the range is the whole run's (spec 0184: the record is
    /// the addressable unit), since that is what a commit would splice.
    first_row: usize,
    /// How many `visible_rows` entries that `text_range` covers.
    covered_rows: usize,
    lines: Vec<String>,
    line_styles: Vec<LineStyles>,
}

/// Spec 0185 S2: one row of the main pane as actually drawn — either a
/// line of the committed document or a line of the preview overlay
/// standing in for it. Overlay rows have no node, hence no heat cue, no
/// override hint, no fold marker and no selection (S4).
#[derive(Clone, Copy)]
pub(super) enum DisplayRow {
    Committed(usize),
    Overlay(usize),
}

/// Owns all cursor/fold/scroll/jumplist state — kept separate from
/// `render()`'s drawing calls (spec 0111 §4, ratatui testability pattern).
pub struct App {
    /// Wrapped blob actually decoded (spec 0114 §1.1) — needed for binary
    /// extraction (`ExtractFormat::Binary` slices `NodeSpan::raw_range`
    /// from this, since every `raw_range` is relative to *this* blob, not
    /// the caller's original one).
    blob: Vec<u8>,
    /// Width in bytes of the wrapper's own tag+length prefix (spec 0114
    /// §1.1) — subtracted from any displayed `raw_range` coordinate to
    /// recover the caller's original (pre-wrap) numbering.
    wrapper_offset: usize,
    /// Original blob's own path — basis for `default_extract_path()`'s
    /// proposed `:export`/`x` default path.
    blob_path: PathBuf,
    /// Whether the main pane currently shows each line's trailing `#@ ...`
    /// annotation (spec 0133) — a pure *display* attribute, toggled by the
    /// `a` key, decoupled from the underlying `lines`/`line_styles`, which
    /// always carry full annotations regardless of this flag.
    annotations: bool,
    /// Indentation step (spaces per nesting level) this session was decoded
    /// with — reused by `apply_override` (spec 0114 §5) so a splice
    /// re-render matches the rest of the document's own indentation.
    indent_size: usize,
    lines: Vec<String>,
    /// Syntax-highlighting spans (spec 0116 §7), index-parallel to `lines`
    /// — each entry holds that line's `(column range, role)` pairs.
    /// Spliced in lockstep with `lines` by `apply_override`, so no
    /// separate offset-shifting bookkeeping is needed.
    line_styles: Vec<LineStyles>,
    /// Resolved color theme (spec 0116 §9) — fixed for the session, never
    /// `ThemeKind::System` (resolved once in `main.rs` before `App::new`).
    theme: ThemeKind,
    tree: Vec<TreeNode>,
    /// Line index (in `lines`) -> node index, for nodes whose text starts
    /// on that line. Used for the fold-indicator gutter.
    line_to_node: HashMap<usize, usize>,
    /// Line index -> node index, for message/group nodes' own closing
    /// (`}`) line (`text_range.end - 1`) — the counterpart to
    /// `line_to_node`'s opening-line mapping, both maintained in lockstep
    /// at the same two sites (`App::new`, `splice_override`'s rebuild).
    /// Used by spec 0113 D33's bold override hint, which needs to
    /// recognize a node's own closing line as directly "its own" (not a
    /// descendant's), the same way `line_to_node` already recognizes its
    /// own opening line.
    footer_line_to_node: HashMap<usize, usize>,
    /// Spec 0160 G1: whether a `render_overrides` batch is currently
    /// active — `0` outside of one, `1` while the outer (caller-
    /// initiated) call is running. `splice_override` uses this to decide
    /// whether it must self-finalize immediately (a standalone call,
    /// e.g. `override_select.rs`'s live-preview splice) or defer to the
    /// active batch's own finalize. Self-recursion inside
    /// `render_overrides_inner` never goes through the counted
    /// `render_overrides` wrapper, so this only ever toggles `0`/`1`,
    /// never nesting deeper.
    override_batch_depth: u32,
    /// Spec 0183 S2: the descent mark. `descend[i]` is `true` when node
    /// `i` either needs resettling itself or has a descendant that
    /// does, so `render_overrides_inner`'s child gate is a single
    /// `Vec` read instead of the old `is_message` blanket that
    /// descended into every interior node of the document.
    ///
    /// Recomputed by `compute_descend_marks` at the start of each
    /// batch (when `override_batch_depth` goes `0` -> `1`) and read
    /// only during it. Indices are arena indices, which is sound
    /// because the arena is append-only: `splice_override` translates
    /// fresh nodes by `base = tree.len()` and abandons superseded ones
    /// in place, so no existing index ever moves. Nodes created
    /// *during* a batch therefore sit past the end of this `Vec` and
    /// read as unmarked — correctly, since freshly spliced content is
    /// descended by `render_overrides_inner`'s `fresh` flag (S3)
    /// rather than by a mark.
    descend: Vec<bool>,
    /// Test-only escape hatch restoring the pre-spec-0183 recursion
    /// gate (`is_message || is_auto_expand_candidate || <has an active
    /// override> || rendered_as.is_some()`), so that a pruned render
    /// can be compared against an unpruned one.
    ///
    /// It exists because pruning fails silently: a subtree the walk
    /// wrongly skipped keeps whatever text it was last rendered with,
    /// with no panic and no bad index, so nothing weaker than byte
    /// equality against the unpruned walk is worth asserting. Not
    /// reachable from a release build.
    #[cfg(test)]
    pub(super) unpruned_walk: bool,
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
    /// batch; drained and applied to `self.lines`/`self.line_styles` in
    /// one pass by `finalize_override_batch`.
    pending_line_patches: Vec<override_apply::LinePatch>,
    cursor: usize,
    /// `true` when the cursor is visually resting on `cursor`'s own
    /// closing `}` line rather than its header line (spec 0142).
    /// `cursor` itself is unaffected — still the node whose bracket
    /// pair the cursor belongs to, so every existing node-indexed
    /// action (fold, override edit, status line, etc.) already treats
    /// a footer-resting cursor exactly like its header, satisfying the
    /// "acts as if on the opening bracket" requirement with no extra
    /// redirection.
    cursor_footer: bool,
    /// Incremented every time `self.cursor` changes (via `set_cursor`),
    /// regardless of whether the new value differs from any prior one —
    /// a real "did the cursor move since X" signal (spec-0117-adjacent
    /// `z`/`Z` rework, 2026-07-16 feedback), since comparing `self.
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
    /// `Down` event (feedback, 2026-07-15) — compared against on the next
    /// `Down` to recognize a double-click (same line, within
    /// `DOUBLE_CLICK_THRESHOLD`). `None` before the first click.
    last_click: Option<(Instant, usize)>,
    /// Whether the click currently in progress (`Down` already handled,
    /// matching `Up` not yet seen) was recognized as the second click of
    /// a double-click — consulted by the `Up` handler to decide whether a
    /// plain (non-dragged) click should deselect (`false`, the default)
    /// or keep the single-line selection `Down` just set (`true`).
    pending_double_click: bool,
    folded: HashSet<usize>,
    /// Rows currently visible (line indices in `lines`, folded-away lines
    /// excluded) — rebuilt only on fold-state changes, not every frame.
    visible_rows: Vec<usize>,
    scroll_offset: usize,
    /// `cursor_display_row()`'s value as of the last render pass that
    /// applied `clamp_scroll_to_visible` to `scroll_offset` (2026-07-19
    /// feedback item 3) — compared against the *current* row at the top
    /// of every render, so the auto-pan-into-view only fires on genuine
    /// cursor movement, not on every frame regardless of cause (which
    /// would otherwise fight a manual vertical pan back into following
    /// the cursor). `None` before the first render, guaranteeing an
    /// initial clamp.
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
    /// `true` when the override pane has focus (spec 0114 §3's `Tab`
    /// toggle); meaningless while `override_target` is `None`.
    override_focus: bool,
    /// `true` when the override pane was opened via `Enter`/double-click
    /// on an entry in the management pane (item 11, 2026-07-17
    /// feedback), rather than via `t`/item 3's smart open on a main-pane
    /// node. Confirming (`Enter`) already lands back in the management
    /// pane unconditionally (spec 0119 G3); this flag makes cancelling
    /// (`Esc`/`t`/`q`) do the same — `close_override` reopens the
    /// management pane instead of leaving focus on the main pane — and
    /// is cleared as soon as it's acted on.
    override_opened_from_manage: bool,
    /// The background scoring worker thread (spec 0152 G1) — `None`
    /// whenever no scoring graph is loaded (mirroring the warm-up
    /// pass's own gate) or before `tui::run()` has spawned it; every
    /// fork that checks `heat_worker.is_some()` falls through to the
    /// existing synchronous logic when it's `None`.
    ///
    /// **This field's position is no longer load-bearing** (spec 0180
    /// S2), but it is left where it is, with the history, because the
    /// history is the argument for the fix.
    ///
    /// It used to matter. `heat_worker` held a `&'static
    /// ArchivedCompiledGraph` borrowed from `ctx`'s mmap-backed
    /// `LoadedGraph`, `HeatWorkerHandle`'s `Drop` is what stops and joins
    /// the background thread using that reference, and Rust drops struct
    /// fields in declaration order — so `ctx` declared before
    /// `heat_worker` would unmap the graph out from under a still-running
    /// worker. That was a real use-after-unmap race, hit intermittently
    /// at process exit (2026-07-25 bug report), and reordering these two
    /// fields is what fixed it.
    ///
    /// It was the wrong kind of fix: a safety invariant expressed as a
    /// source-line ordering, which the compiler does not check and which
    /// covered only the thread that happened to have a handle to drop —
    /// the detached root-type sweep in `run()` had the identical bug and
    /// this fix did nothing for it. The worker now holds an
    /// `Arc<LoadedGraph>`, so it cannot outlive the mapping in any drop
    /// order at all. Do not re-derive an ordering requirement from this
    /// comment.
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
    /// applied `clamp_scroll_to_visible` to `override_scroll`
    /// (2026-07-19 feedback item 3, mirrors `last_cursor_row`) — reset to
    /// `None` everywhere `override_scroll` is itself reset to `0`
    /// (opening the pane, recomputing candidates), guaranteeing a clamp
    /// on the next render even if the new highlight happens to coincide
    /// with the old one.
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
    /// `true` from `App::new` until the one-shot background root-type
    /// resolver (spec NNNN) reports back via `AppEvent::RootTypeResolved`
    /// — set from `Decoded::root_type_deferred`. `run()` only spawns the
    /// resolver thread while this is `true`; the run-loop's event handler
    /// clears it unconditionally on `RootTypeResolved`, whether or not a
    /// winner was found, since either way there's nothing left to wait
    /// for.
    root_type_pending: bool,
    /// Per-node heat-cue resolution state (spec 0152 G6), parallel to
    /// `tree` — `Pending` until a cache check (only ever attempted on a
    /// worker-progress wakeup or a real redraw-triggering input event)
    /// resolves it; `Resolved` nodes are read directly, no cache lock.
    heat_states: Vec<heat_cue::HeatState>,
    /// Node indices with an outstanding worker request (spec 0152 G6/
    /// G8) — populated by `heat_cue_resolve` whenever it finds a node
    /// still unsettled with a worker present, drained by
    /// `recheck_pending_heat_states`. Bounds that recheck to the
    /// handful of nodes actually awaiting an answer instead of the
    /// whole document (2026-07-24 bugfix: a full `0..heat_states.len()`
    /// scan on every `HeatWorkerProgress` event took over a second per
    /// event on a 635k-node document, compounding into tens of seconds
    /// whenever a burst of requests completed at once).
    pending_heat_recheck: HashSet<usize>,
    /// Spec 0164 G7: main-pane zigzag read-ahead prefetch walk state,
    /// persisted across `run_loop` iterations.
    prefetch_walk: PrefetchWalk,
    /// Incremented every time `rebuild_visible_rows` runs (fold/unfold,
    /// content-shape change) — `App::prefetch_step`'s staleness signal
    /// (spec 0164 G7) for restarting its walk when rendered line
    /// numbers may have shifted.
    structural_version: u64,
    /// `true` while `recompute_override_candidates`'s `SortMode::
    /// Inferred` branch is waiting on a worker request for the
    /// override pane's first page (spec 0152 G7).
    override_candidates_pending: bool,
    /// `true` while `upgrade_active_override_to_complete` is waiting on
    /// a worker request for a wider window (spec 0152 G7).
    override_complete_pending: bool,
    /// `true` while the main-pane heat cue (spec 0138) is toggled off by
    /// `i` (item 12, 2026-07-17 feedback) — hides the cue without
    /// discarding `heat_caches`.
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
    /// 2026-07-20 feedback: the FQDN (or the `None` sentinel string) an
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
    /// back to `false` without closing it (2026-07-14 feedback).
    manage_focus: bool,
    /// Highlighted row (index into `overrides.entries()`) in the
    /// management pane.
    manage_highlight: usize,
    /// Scroll offset (in rows) for the management pane's listing.
    manage_scroll: usize,
    /// `manage_highlighted_row()`'s value as of the last render pass that
    /// applied `clamp_scroll_to_visible` to `manage_scroll` (2026-07-19
    /// feedback item 3, mirrors `last_cursor_row`) — reset to `None`
    /// everywhere `manage_scroll` is itself reset to `0`.
    last_manage_highlight: Option<usize>,
    /// Timestamp + entry index of the most recent left-click `Down` that
    /// landed on an entry's radio marker (2026-07-17 feedback) — compared
    /// against on the next such click to recognize a double-click (same
    /// entry, within `DOUBLE_CLICK_THRESHOLD`), the mouse-only
    /// alternative to Shift-click for `toggle_active_cascading` (most
    /// terminal emulators intercept Shift-click for native text
    /// selection before it ever reaches the app). `None` before the
    /// first marker click.
    last_manage_click: Option<(Instant, usize)>,
    /// Timestamp + entry index of the most recent left-click `Down` that
    /// landed on an entry row *outside* its radio marker (item 11,
    /// 2026-07-17 feedback) — compared against on the next such click to
    /// recognize a double-click (same entry, within
    /// `DOUBLE_CLICK_THRESHOLD`), which opens the override selection
    /// pane on that entry (`open_override_from_manage`), same as
    /// `Enter`. Tracked separately from `last_manage_click`, which is
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
    back_stack: Vec<usize>,
    fwd_stack: Vec<usize>,
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
    /// Wall-clock time at which `splash` auto-dismisses (2026-07-17
    /// feedback, item 13), in addition to its existing keypress/mouse
    /// dismissal — checked only while `splash` is still `true`, mirroring
    /// `message_deadline`'s deadline-based approach.
    splash_deadline: Instant,
    /// `true` while the `F1` help overlay is open.
    help_open: bool,
    /// Scroll offset (in `HELP_TEXT` lines) while the help overlay is open.
    help_scroll: usize,
    /// Help overlay's inner (bordered-away) `Rect` as of the last
    /// `render_help()` call (feedback, 2026-07-15) — used to hit-test
    /// mouse wheel/Shift-wheel events against the overlay instead of
    /// letting them fall through to whichever pane it happens to be
    /// drawn on top of. Only meaningful while `help_open`.
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
        decoded: Decoded,
        blob_label: &str,
        blob_path: PathBuf,
        indent_size: usize,
        ctx: DescriptorContext,
        theme: ThemeKind,
        proto_root: Option<PathBuf>,
    ) -> Self {
        let all_type_fqdns = override_pane::all_type_fqdns(ctx.pool());
        let tree_len = decoded.tree.len();
        let mut line_to_node = HashMap::new();
        let mut footer_line_to_node = HashMap::new();
        for (idx, node) in decoded.tree.iter().enumerate() {
            line_to_node.insert(node.span.text_range.start, idx);
            // A distinct footer line only exists when the node's own
            // header and closing `}` aren't the same line — not the
            // same as `first_child.is_some()` (spec 0142 fix, 2026-07-
            // 18 feedback): an empty-but-bracketed message (decoded
            // with zero populated fields, still rendered as `Name {`
            // then `}` on the next line) has no children yet still has
            // a real, distinct footer line that must be a reachable
            // cursor stop.
            if node.span.text_range.end - 1 > node.span.text_range.start {
                footer_line_to_node.insert(node.span.text_range.end - 1, idx);
            }
        }
        let header = format!("protolens — {blob_label} — {}", decoded.root_type);
        // Document-order first node (doc_prev == None) — not array index 0,
        // which is post-order (see decode::TreeNode's doc comment).
        let cursor = decoded
            .tree
            .iter()
            .position(|n| n.doc_prev.is_none())
            .unwrap_or(0);
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
            line_styles: decoded.style_hints,
            theme,
            tree: decoded.tree,
            line_to_node,
            footer_line_to_node,
            override_batch_depth: 0,
            descend: Vec::new(),
            #[cfg(test)]
            unpruned_walk: false,
            pending_shift: 0,
            pending_line_patches: Vec::new(),
            cursor,
            cursor_footer: false,
            cursor_moves: 0,
            select_anchor: None,
            select_end: None,
            last_click: None,
            pending_double_click: false,
            folded: HashSet::new(),
            visible_rows: Vec::new(),
            scroll_offset: 0,
            last_cursor_row: None,
            pan_offset: 0,
            override_pan_offset: 0,
            manage_pan_offset: 0,
            command_pan_offset: 0,
            override_target: None,
            preview_overlay: None,
            override_preview_byte_budget: Self::OVERRIDE_PREVIEW_BYTE_BUDGET_DEFAULT,
            override_focus: false,
            override_opened_from_manage: false,
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
            root_type_pending: decoded.root_type_deferred,
            heat_states: vec![heat_cue::HeatState::default(); tree_len],
            pending_heat_recheck: HashSet::new(),
            prefetch_walk: PrefetchWalk::exhausted(),
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
            message: String::new(),
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
        if let Some(node) = app.tree.get_mut(cursor) {
            // The root's own field name is always "1" (its field number
            // in the virtual encompassing message — mirrors
            // `field_name_for`'s no-parent case), and can't yet carry a
            // §G4 name override at this point (the collection was just
            // seeded, nothing has been renamed).
            node.rendered_as = Some((root_override_type.clone().map(Some), "1".to_string()));
        }
        // Spec 0120: Any/MessageSet auto-expansion is computed by
        // `render_overrides` itself (`auto_expand_type`), not by
        // `decode()`'s own initial paint — run one pass now so the
        // initial view already shows any/MessageSet content expanded,
        // matching the pre-spec-0120 behavior where `decode()` expanded
        // it directly via prototext-core. Guarded like the block above:
        // an empty tree has no node at `cursor` to render.
        if !app.tree.is_empty() {
            app.render_overrides(cursor);
        }
        app.rebuild_visible_rows();
        app
    }
}

/// Spec 0164 G7: per-`App` zigzag-walk state, persisted across
/// `run_loop` iterations (not rebuilt on every call) — reset only when
/// the cursor's display row or the document's structural/reflow state
/// (`App::structural_version`) has changed since the walk began.
/// `origin_line`/`above`/`below` are indices into `App::visible_rows`
/// (rendered-line space), not raw `App::lines` indices — folded/hidden
/// content never appears there, so the walk naturally skips it.
struct PrefetchWalk {
    origin_line: usize,
    above: usize,
    below: usize,
    above_done: bool,
    below_done: bool,
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
            structural_version: u64::MAX,
        }
    }

    /// Advances the walk to the next unexplored row, alternating above/
    /// below (always the nearer of the two unexplored ends), and
    /// returns its index into `visible_rows`. `None` once both ends
    /// are exhausted.
    fn next_row(&mut self, visible_len: usize) -> Option<usize> {
        loop {
            if self.above_done && self.below_done {
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

impl App {
    /// Advances the zigzag prefetch walk by one candidate, pushing it
    /// at `Tier::Prefetch` if it isn't already settled/cached (spec
    /// 0164 G7). First resets `self.prefetch_walk` to a fresh walk
    /// from the cursor's current display row if either the cursor's
    /// row or `self.structural_version` has changed since the walk
    /// began — calling `start_new_wave()` (G2) on the request queue
    /// and both `HeatCaches` maps before the reset; otherwise resumes
    /// exactly where the previous call left off. Returns `Idle` once
    /// the document is fully walked, the last push returned
    /// `UpsertOutcome::Rejected` (G6), or no worker is running at all
    /// (nothing to prefetch into).
    pub(super) fn prefetch_step(&mut self) -> PrefetchStep {
        if self.heat_worker.is_none() {
            return PrefetchStep::Idle;
        }
        let origin_row = self.cursor_display_row();
        if self.prefetch_walk.origin_line != origin_row
            || self.prefetch_walk.structural_version != self.structural_version
        {
            if let Some(worker) = &self.heat_worker {
                worker.start_new_wave();
            }
            {
                let mut caches = self.heat_caches.lock().unwrap_or_else(|e| e.into_inner());
                caches.by_range.start_new_wave();
                caches.current_score.start_new_wave();
            }
            self.prefetch_walk = PrefetchWalk {
                origin_line: origin_row,
                above: 0,
                below: 0,
                above_done: false,
                below_done: false,
                structural_version: self.structural_version,
            };
        }

        loop {
            let Some(row) = self.prefetch_walk.next_row(self.visible_rows.len()) else {
                return PrefetchStep::Idle;
            };
            let line = self.visible_rows[row];
            let Some(&idx) = self.line_to_node.get(&line) else {
                continue;
            };
            if !self.can_override(idx) || self.heat_states[idx].settled() {
                continue;
            }
            let range = {
                let node = &self.tree[idx].span;
                extract::message_payload_range(
                    &self.blob,
                    &node.raw_range,
                    node.packed_record_start,
                )
            };
            let current_key = self.current_type_key(idx);
            let (_, outcome) = self.heat_lookup_ex(
                &range,
                current_key.as_deref(),
                0,
                heat_cue::HEAT_CUE_PREVIEW,
                tiered::Tier::Prefetch,
            );
            return match outcome {
                Some(tiered::UpsertOutcome::Rejected) => PrefetchStep::Idle,
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

/// Column where a fold marker is inserted by `App::render_line_content` —
/// right after the line's own leading indentation (column 0 for an
/// unindented, root-level line).
fn marker_column(line: &str) -> u16 {
    let indent_len = line.len() - line.trim_start().len();
    indent_len as u16
}

/// Drain any input events already queued in the terminal's input buffer
/// before disabling raw mode (feedback, 2026-07-16).
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
/// terminal supports it (2026-07-17 feedback) — without it, legacy
/// terminal escape sequences carry no modifier parameter for printable
/// keys, so Shift-Space is reported identically to plain Space (unlike
/// arrow/function keys, which already carry one, e.g. `ESC [1;2A` for
/// Shift-Up). `supports_keyboard_enhancement` queries the terminal and
/// blocks briefly waiting for its response — fine here since it only
/// ever runs before the main event loop starts (`run`) or during a
/// suspend/resume cycle (`suspend`), never concurrently with
/// `event::read`/`poll`. Terminals that don't support it are left
/// exactly as before (no-op): `handle_manage_key`'s guarded
/// `Char(' ') if SHIFT` arm simply never fires there, same as today.
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
/// capture) state — shared by `run`'s own cleanup and the panic hook below,
/// so a panic mid-session doesn't leave the user's terminal unusable.
fn restore_terminal() {
    drain_pending_input();
    pop_keyboard_enhancement();
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
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
    // `Terminal::draw()` hides the hardware cursor unless the render
    // callback calls `Frame::set_cursor_position()` — which protolens never
    // does — so the last `draw()` call before this suspend left it hidden.
    // Without this, the shell prompt gets no visible cursor after `fg`
    // (feedback, 2026-07-16).
    terminal.show_cursor()?;
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

/// Run the interactive TUI loop against a real terminal.
pub fn run(app: &mut App) -> io::Result<()> {
    enable_raw_mode()?;
    push_keyboard_enhancement()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    // Safe upper bound (spec 0151 G6/G8): the real, render-computed
    // `override_list_height` (set by the override pane's own first
    // render, `render.rs`) is always <= the raw terminal height, since
    // it's an inner-area height net of borders/header rows. Setting it
    // eagerly here means G6's cross-population cap isn't stuck at `1`
    // (its `App::new` default) for the warm-up pass or any ordinary
    // browsing before the user's first `t` press.
    app.override_list_height = terminal.size()?.height.max(1) as usize;

    // A panic mid-session (e.g. an indexing bug) would otherwise unwind
    // straight out of this function, skipping the cleanup below and
    // leaving the terminal stuck in raw/alt-screen/mouse-capture mode.
    // Restore it first, then hand off to the default panic printer.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));

    let (tx, rx) = mpsc::channel();
    // `Option`-wrapped (2026-07-20 feedback) so `run_loop` can `take()`
    // it and shut it down around the Neovim handoff below — see
    // `run_loop`'s own doc comment on that block for why.
    let mut input_reader = Some(event::InputReaderHandle::spawn(tx.clone()));

    // Spec 0152 G1: spawned only when a scoring graph is loaded — with
    // no graph, `app.heat_worker` stays `None` for the whole session,
    // and every fork that checks `heat_worker.is_some()` falls through
    // to the existing synchronous logic. Spawned *before* `warm_up_
    // heat_cues` below (2026-07-19 feedback) so its initial-viewport
    // pass can push requests onto the worker's queue and return
    // immediately instead of scoring synchronously on this thread —
    // fixes the multi-second "black screen" startup freeze against a
    // large scoring graph, which the warm-up pass used to cause by
    // always running before any worker existed to hand work off to.
    if let Some(graph) = &app.ctx.graph {
        // Spec 0180 S2: an owning handle, not a `&'static` copied out of the
        // mapping. Each `Arc::clone` below is a refcount bump, and it is what
        // makes both spawns below independent of when `App` drops.
        let graph = Arc::clone(graph);
        let blob = Arc::new(app.blob.clone()); // one-time clone, G2

        // Spec NNNN: root-type inference is a single, one-shot `score_
        // all` sweep over the *whole* blob — unlike heat-cue/override
        // requests, it's never repeated, so it gets its own detached,
        // fire-and-forget thread rather than a `HeatRequestQueue` entry.
        // Not joined anywhere (deliberately, unlike `heat_worker`/
        // `input_reader` below): it holds only `Arc`-owned data, so an
        // early `should_quit` simply lets the OS reclaim it mid-sweep —
        // joining would otherwise make quitting wait out the very sweep
        // this whole feature exists to get off the critical path.
        //
        // That claim was **false until spec 0180 S2**, and this was the
        // worst place for it to be false: the graph was a `&'static`
        // transmuted out of `ctx`'s mmap, so `App` dropping while the
        // longest scoring operation in the program was still running
        // unmapped the pages underneath it. `graph` is an owning
        // `Arc<LoadedGraph>` now, so the mapping survives exactly as long
        // as this thread needs it and the sentence above is true as
        // written. Do not turn it back into a borrow.
        if app.root_type_pending {
            let root_type_tx = tx.clone();
            let root_type_graph = Arc::clone(&graph);
            // `determine_root_type` (spec NNNN's synchronous ancestor)
            // scores the caller's original, unwrapped blob — recover it
            // from the wrapped one `Decoded::blob` carries (see `Decoded::
            // wrapper_offset`'s doc comment).
            let original_blob = app.blob[app.wrapper_offset..].to_vec();
            // Spec 0180 S4: a `Builder`, not a bare `thread::spawn`. This
            // thread runs `score_all` over the whole blob — the deepest
            // input the scorer ever sees — and used to take `std`'s 2 MiB
            // default while the heat worker running the identical code got
            // 16 MiB, purely because the constant lived in `heat_worker.rs`.
            thread::Builder::new()
                .name("root-type".to_string())
                .stack_size(SCORING_THREAD_STACK_SIZE)
                .spawn(move || {
                    let fqdn =
                        decode::resolve_root_winner_fqdn(&original_blob, root_type_graph.graph());
                    let _ = root_type_tx.send(event::AppEvent::RootTypeResolved(fqdn));
                })
                .expect("spawn root-type thread");
        }

        app.heat_worker = Some(heat_worker::HeatWorkerHandle::spawn(
            Arc::clone(&app.heat_caches),
            graph,
            blob,
            tx.clone(),
        ));
    }

    warm_up_heat_cues(&mut terminal, app)?;

    let result = run_loop(&mut terminal, app, &rx, &mut input_reader, &tx);

    // Spec 0152 G9: both threads joined, unconditionally (see "Shutdown
    // and safety"). No longer load-bearing for *memory* safety — spec
    // 0180 S2 gave the worker an owning `Arc<LoadedGraph>`, so it cannot
    // observe an unmapped page whether or not this runs. It is still
    // wanted: joining is what stops the worker writing to the terminal
    // after `restore_terminal` below, and what keeps the process from
    // lingering on a background sweep the user has already quit.
    if let Some(worker) = app.heat_worker.take() {
        worker.shutdown();
    }
    if let Some(reader) = input_reader.take() {
        reader.shutdown();
    }
    let _ = std::panic::take_hook();
    restore_terminal();
    terminal.show_cursor()?;

    result
}

/// One-time warm-up pass (spec 0151 G8) priming heat cues for the
/// initial viewport before the first `run_loop` iteration. With the
/// spec-0152 worker already spawned by the time this runs, each
/// `heat_cue_for` call below either hits the cache or pushes a request
/// onto the worker's queue and returns immediately — this loop no
/// longer scores anything itself (that was the original, now-fixed,
/// multi-second "black screen" startup freeze against a large scoring
/// graph). Still runs while heat cues are hidden (`i`, 2026-07-19
/// feedback): the background fetch/cache is worth priming regardless
/// of whether a cue is currently shown — only `heat_cue_for`'s return
/// value is suppressed while hidden, not the underlying work.
fn warm_up_heat_cues<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> io::Result<()>
where
    io::Error: From<B::Error>,
{
    if app.ctx.graph.is_none() {
        return Ok(());
    }
    let rows = terminal.size()?.height as usize;
    let lines: Vec<usize> = app.visible_rows.iter().take(rows).copied().collect();
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
    loop {
        terminal.draw(|frame| app.render(frame))?;
        // While a status message is pending auto-dismissal
        // (`message_deadline`, `track_message_timeout`) or the splash
        // screen hasn't yet auto-dismissed (`splash_deadline`, item 13
        // of 2026-07-17 feedback), receive with a timeout instead of
        // blocking indefinitely, so the next `render()` (which actually
        // clears an expired message/splash) runs even with no further
        // event. No behavior change — same indefinite block as before —
        // once both have elapsed, which is most of the time in ordinary
        // navigation. Spec 0152 G8: this replaces a direct `event::
        // poll`/`event::read()` pair — the input-reader thread now owns
        // those calls, forwarding through `rx` alongside the worker
        // thread's own progress notifications, so this loop genuinely
        // sleeps until there's a reason to wake instead of polling on a
        // fixed schedule.
        let splash_deadline = app.splash.then_some(app.splash_deadline);
        let deadline = match (app.message_deadline, splash_deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(d), None) | (None, Some(d)) => Some(d),
            (None, None) => None,
        };
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
                    PrefetchStep::Progressed => continue,
                    PrefetchStep::Idle => {
                        break match deadline {
                            Some(deadline) => {
                                let timeout = deadline.saturating_duration_since(Instant::now());
                                rx.recv_timeout(timeout).ok() // timeout elapsed => None
                            }
                            None => rx.recv().ok(),
                        };
                    }
                },
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        };
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
                app.handle_key(key)
            }
            Some(event::AppEvent::Term(Event::Mouse(mouse))) => app.handle_mouse(mouse),
            Some(event::AppEvent::Term(_)) => {}
            Some(event::AppEvent::HeatWorkerProgress) => {
                app.recheck_pending_heat_states();
                app.poll_pending_override_work();
            }
            Some(event::AppEvent::RootTypeResolved(fqdn_opt)) => {
                app.root_type_pending = false;
                if let Some(fqdn) = fqdn_opt {
                    app.apply_resolved_root_type(fqdn);
                }
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
            // 2026-07-20 feedback ("`v` is broken"): `open_editor`
            // backgrounds this process's own process group relative to
            // the terminal (`tcsetpgrp(io::stdin(), nvim_pgid)`) for as
            // long as Neovim owns the foreground. The input-reader
            // thread (spec 0152 G8) is otherwise permanently blocked in
            // `event::read()` on that same stdin — a background
            // process's read from its controlling terminal draws
            // SIGTTIN, whose default disposition stops the *whole
            // process* (every thread, not just this one), with nothing
            // left to `SIGCONT` it back. Shutting it down before the
            // handoff and respawning a fresh one right after
            // `open_editor` reclaims the terminal avoids that entirely
            // — exactly what the single-threaded code (before spec
            // 0152's input-reader thread existed) did implicitly, since
            // back then nothing else ever touched stdin during the
            // handoff.
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
mod manage_pane;
mod mouse;
mod navigation;
#[cfg(unix)]
mod neovim;
mod override_apply;
mod override_select;
mod render;
mod tiered;

#[cfg(test)]
mod tests;
