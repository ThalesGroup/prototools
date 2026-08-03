<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# Pane: command line (global command/message row)

*last verified: 2026-08-03*

## Executive summary

There is exactly one text-entry surface in protolens — a single
`Length(1)` row fixed at the very bottom of the whole screen, shared
across every pane regardless of how many are open — and every pane that
needs to prompt for typed input (ex-commands, searches, the management
pane's rename) shares it rather than growing its own. This same row also
carries a passive `self.message` notice whenever no text entry is
active, so it is never idle-blank except when there is genuinely nothing
to show. What the row currently represents (a `:` command, a `/`/`?`
search, a rename, or a message) is tracked by trying each source in a
fixed priority order each frame; the buffer's own editing model (cursor
position, insert/delete, Tab-completion) is written once and is
identical regardless of which of the first three is currently being
typed. This row never shows any pane's own cursor/position info — that
lives in each pane's own local statusline instead (see
[help-and-chrome.md](help-and-chrome.md)).

## Technical detail

### One buffer, one cursor model, several interpretations

The shared buffer is edited with a real character-index cursor (not
"always append at the end") — `Left`/`Right`/`Home`/`End` move it,
`Backspace`/`Delete`/typed characters act relative to it — because a user
correcting a typo mid-command shouldn't need to retype everything after
it. `Enter`'s behavior branches only at the very last moment, on which
`CommandLineKind` is currently active and which pane currently has
focus: an ex-command is parsed and dispatched; a search pattern is
handed to whichever pane's own search function is appropriate (main pane,
override-select pane, or management pane — determined by focus, not by
anything stored in the search state itself). An empty `/`/`?` confirmation
reuses the last pattern *for that specific pane*, mirroring vim's own
convention, while `n` always repeats in the same direction the pattern
was last searched, independent of which direction a fresh `/`/`?` press
might currently be requesting.

Matching is **smartcase** in all three panes (spec 0195): an
all-lowercase pattern matches case-insensitively, a pattern with any
uppercase character matches exactly. It is one shared helper rather
than three, since the panes differ in *what* they search — document
text, candidate FQDNs, entry labels — but not in how a pattern should
be read.

### Command dispatch: one registry, two consumers

Every ex-command name is declared exactly once, in a single array
constant. That one list is the source of truth for both prefix-matching
dispatch (a user can type an unambiguous prefix of a command name and
have it resolve, with an *exact* full-name match always taking priority
over being a prefix of something longer — matching how both vim's
`:command` abbreviations and `argparse` resolve prefixes) and Tab
completion's candidate list. Adding a new ex-command is a one-line
addition to that array; both dispatch and completion pick it up
automatically, with no second registration point to remember.

### Tab-completion: token-aware, not just command-name

Completion isn't limited to command names. Once the first token has
unambiguously resolved to a command that takes a particular kind of
argument, the following tokens are completed against that argument's own
domain: `override`'s `--as` argument completes against the session's
full list of known type FQDNs — which the on-demand descriptor branch
([descriptor-context.md](descriptor-context.md)) must still answer in
full, and does, by reading names out of the `index.rkyv` sidecar
without decoding the files behind them; `save`/`restore`'s argument
completes against the filesystem, directory by directory, the same way a
shell's own path completion works (each Tab descends one more directory
level rather than trying to complete the whole remaining path at once).
Every other command, and every position past a command's expected
arguments, is a silent no-op — deliberately, since guessing at
what a not-yet-designed future argument might mean would be worse than
doing nothing.

A token beginning with `-` is the one case checked *before* any of
that, and independently of which command it belongs to: it completes
against that command's own option list, held in a registry beside the
command names. The check is safe to make command-independent because no
*value* the command line accepts begins with `-` — not a path, not an
FQDN, not an origin.

`override` is also the one command whose completion dispatches on
the token being completed rather than on a fixed argument position: its
two flags may appear in either order, before or after the positional,
so "the second token" is not a meaningful question to ask about it.

Repeated `Tab` presses cycle through the current candidate list once
completion is already active; the first press instead extends the
in-progress token to the longest common prefix of all candidates (vim/zsh
convention) without yet committing to any one of them, so a user typing
`:export --desc` and pressing Tab once sees `--descriptor-`, the common
prefix of the two options that match, without the pane guessing which
one was meant.

### Two completers, because two kinds of argument

Prefix matching is the right shape for an argument drawn from a
namespace — an FQDN out of tens of thousands, a filename out of a
directory. It is the wrong shape for an argument with a handful of
mutually exclusive spellings, which is what `:override`'s `<origin>`
and `--field-name` are: three origin shapes, four ways to name a field.
Those two are completed by *rotation* instead — the candidates are
offered whole, unfiltered by what is already typed, and the first `Tab`
lands on the one *after* whatever the token currently spells.

Both halves of that matter. Unfiltered, because the token is almost
always the value `o` pre-filled, which prefix-matches none of its
alternatives — filtering would leave `Tab` dead exactly where it is
most wanted. And landing on the *next* candidate, because the prefix
completer deliberately does not select anything on its first press (it
only extends to the common prefix and primes the cycle), which on a
rotation would read as `Tab` doing nothing at all.

`--as` keeps the prefix completer, but consults two lists in sequence
rather than one: the inference graph's ranked candidates for the node
first, in decreasing score order, and only if none of them match, the
lexicographic list. Sequenced, not concatenated — on a large descriptor
set a two-character prefix matches thousands of FQDNs, and the handful
of ranked ones would be buried among them, which is the whole value of
ranking them. A cold score cache yields nothing and the lexicographic
list answers instead, silently: a completer that sometimes ignores a
keystroke is worse than one whose order is sometimes alphabetical.
