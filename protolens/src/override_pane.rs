// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Override-pane candidate-list computation and sort modes (spec 0114 §3).
//! `override` itself is a reserved Rust keyword, unusable as a module name
//! (spec 0114 Background) — hence `override_pane`.

use std::sync::atomic::AtomicBool;

use prost_reflect::DescriptorPool;
use prototext_graph::build_scoring_graph::serial::ArchivedCompiledGraph;
use prototext_graph::score::{score_one, ScoringOpts};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Sort mode for the override pane's ranked candidate list (spec 0114
/// §3.2), toggled by `i` while the pane has focus. Applies only to the
/// ranked candidates below the pinned `<raw / no type>` entry (§3.1),
/// which is neither sorted nor affected by this choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    /// All message/group types known to the loaded descriptor set,
    /// alphabetically by FQDN. Cheap — no `score_all` call.
    Lexicographic,
    /// Ranked by `score_all` against the target range, descending score
    /// (ties broken by FQDN) — the default.
    Inferred,
}

/// All message/group/enum type FQDNs known to `pool`, alphabetically
/// sorted (spec 0114 §3.2's lexicographic mode; enums added by spec
/// 0137 §G2). Independent of range — computed once and reused for
/// every override-pane invocation, every range, for the whole session
/// (§6: "needs no per-range caching").
pub fn all_type_fqdns(pool: &DescriptorPool) -> Vec<String> {
    let mut names: Vec<String> = pool
        .all_messages()
        .map(|m| m.full_name().to_string())
        .chain(pool.all_enums().map(|e| e.full_name().to_string()))
        .collect();
    names.sort_unstable();
    names
}

/// Ranked candidate FQDNs (with their score) for `range_bytes`, descending
/// inferred score, ties broken by FQDN (spec 0114 §3.2) — same scoring
/// engine and tie-break rule `decode.rs::determine_root_type` already uses
/// for the document's own root type, applied here per-range instead of
/// corpus-wide. The score is surfaced alongside each FQDN so the override
/// pane can display it next to the candidate.
///
/// Vetoed candidates (a structural wire-format mismatch against the
/// range's actual bytes — see `prototext-graph`'s veto rules) are
/// excluded entirely: a type the wire data already contradicts is not a
/// plausible override target, the same "non_vetoed" filtering
/// `determine_root_type` applies before ranking.
///
/// Spec 0217: the sweep and the ranking both live in `crate::sweep`,
/// shared with startup's root-type sweep so the comparator above exists
/// in exactly one place. `jobs` is a ceiling on the threads it may use,
/// not a target — see `sweep::effective_jobs`.
/// `cancel` is passed straight through to `sweep::ranked`: raising it
/// abandons the sweep, and the list returned is then partial and must not
/// be used. `None` for a caller with no way to change its mind.
pub fn inferred_candidates(
    range_bytes: &[u8],
    graph: &ArchivedCompiledGraph,
    jobs: usize,
    cancel: Option<&AtomicBool>,
) -> Vec<(String, i64)> {
    crate::sweep::ranked(range_bytes, graph, jobs, cancel)
}

/// A single candidate's inferred score, scored alone (spec 0154 G1) —
/// the cheap, single-entry counterpart to `inferred_candidates`, used
/// by the heat-cue worker's fast path when only one type's exact score
/// is missing and the rest of the range's candidate window is already
/// cached. `None` both when `fqdn` isn't a known root type and when it
/// is but is vetoed — same convention `inferred_candidates` applies.
pub fn inferred_score(
    range_bytes: &[u8],
    fqdn: &str,
    graph: &ArchivedCompiledGraph,
) -> Option<i64> {
    let result = score_one(range_bytes, fqdn, graph, &ScoringOpts::default())?;
    if result.vetoed {
        None
    } else {
        Some(result.score())
    }
}

// ── Override collection (spec 0117) ─────────────────────────────────────────

/// One of the three override scopes (spec 0117 §1), in increasing-priority
/// order. Not used for the collection's sort order (which sorts by origin
/// label as a plain string — see `OverrideCollection::sort`), only for
/// `next`/`prev` rotation (`z`/`Z`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OverrideKind {
    Path,
    PathField,
    FqdnField,
}

impl OverrideKind {
    /// Rotates `z` in the override selection pane: `Path -> PathField ->
    /// FqdnField -> Path -> ...` (spec 0117 §2).
    pub fn next(self) -> Self {
        match self {
            OverrideKind::Path => OverrideKind::PathField,
            OverrideKind::PathField => OverrideKind::FqdnField,
            OverrideKind::FqdnField => OverrideKind::Path,
        }
    }

    /// Rotates `Z` — the reverse of `next()`.
    pub fn prev(self) -> Self {
        match self {
            OverrideKind::Path => OverrideKind::FqdnField,
            OverrideKind::PathField => OverrideKind::Path,
            OverrideKind::FqdnField => OverrideKind::PathField,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            OverrideKind::Path => "path",
            OverrideKind::PathField => "path-field",
            OverrideKind::FqdnField => "fqdn-field",
        }
    }
}

/// The `(kind, ...)` key identifying an override, independent of its
/// candidate type (spec 0117 §1). At most one active entry exists per
/// distinct `OverrideOrigin` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverrideOrigin {
    /// e.g. `/1/2` — canonical `positional_path` form, no trailing slash.
    Path { path: String },
    /// e.g. `/1`, field `2`.
    PathField { path: String, field: u64 },
    /// e.g. `pkg.Msg`, field `2`.
    FqdnField { fqdn: String, field: u64 },
}

impl OverrideOrigin {
    pub fn kind(&self) -> OverrideKind {
        match self {
            OverrideOrigin::Path { .. } => OverrideKind::Path,
            OverrideOrigin::PathField { .. } => OverrideKind::PathField,
            OverrideOrigin::FqdnField { .. } => OverrideKind::FqdnField,
        }
    }

    /// User-facing display of the origin (kind is shown separately) —
    /// `path`, `path:field`, or `fqdn:field`.
    pub fn label(&self) -> String {
        match self {
            OverrideOrigin::Path { path } => path.clone(),
            OverrideOrigin::PathField { path, field } => format!("{path}:{field}"),
            OverrideOrigin::FqdnField { fqdn, field } => format!("{fqdn}:{field}"),
        }
    }
}

/// True when `candidate`'s origin `label()` is `origin`'s own `label()`,
/// or has it as a genuine prefix (`toggle_active_cascading`'s notion of
/// "under"). Comparing plain label strings (`path`, `path:field`, or
/// `fqdn:field`) works uniformly across all three origin kinds without
/// special-casing them: a `path:field` origin's label extends its
/// `path` origin's label with a `:field` suffix (so `/3` matches
/// `/3:5`, the same node via a different override kind), and a deeper
/// `path` origin's label extends it with a `/segment` suffix (so `/3`
/// matches `/3/2` but not `/30` — a genuine path-segment boundary, not
/// a raw string prefix).
/// The root path `/` is a special case: every path-rooted label starts
/// with `/`, but `format!("{origin}/")` would double up its own
/// trailing slash, so it's checked directly instead. An `fqdn:field`
/// origin never prefixes, nor is prefixed by, anything but its own
/// label — no other origin's label is ever built by extending an FQDN.
pub(crate) fn origin_is_at_or_under(candidate: &OverrideOrigin, origin: &OverrideOrigin) -> bool {
    let origin_label = origin.label();
    let candidate_label = candidate.label();
    if candidate_label == origin_label {
        return true;
    }
    if origin_label == "/" {
        return candidate_label.starts_with('/');
    }
    candidate_label.starts_with(&format!("{origin_label}/"))
        || candidate_label.starts_with(&format!("{origin_label}:"))
}

/// One entry of the collection: an origin, its candidate type (`None` =
/// raw/no type), and whether it is the currently-active entry for that
/// origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverrideEntry {
    pub origin: OverrideOrigin,
    pub r#type: Option<String>,
    pub active: bool,
    /// Display-name override (spec 0119 G4): `None` keeps the
    /// schema-derived real field name (or its own fallback chain);
    /// `Some` takes priority over it wherever the real field name would
    /// otherwise be resolved.
    pub name: Option<String>,
    /// `true` when this entry was created by `render_overrides`'s
    /// internal Any/MessageSet auto-expansion seeding (`activate_auto`),
    /// as opposed to an explicit user action (`activate`, via the
    /// override pane or `type-as`) — spec 0120. Provenance only, purely
    /// for display (`manage_entry_style` colors auto entries differently
    /// from manual ones): it has no effect whatsoever on how an active
    /// entry is resolved or rendered — an override is an override,
    /// regardless of how it came to exist. Round-trips through the YAML
    /// save/restore format (spec 0125 §G3) like every other field; a
    /// file with no `auto` key defaults to `false`.
    pub auto: bool,
}

/// The persistent collection of overrides (spec 0117 §1). Always kept
/// sorted lexicographically by origin label (`OverrideOrigin::label`:
/// `path`, `path:field`, or `fqdn:field`), then type (`None` first) — the
/// same order used for the management pane's listing and the YAML file's
/// entry order. Deliberately by origin path as a plain string, not by
/// kind first.
#[derive(Debug, Default)]
pub struct OverrideCollection {
    entries: Vec<OverrideEntry>,
}

impl OverrideCollection {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn entries(&self) -> &[OverrideEntry] {
        &self.entries
    }

    fn sort(&mut self) {
        self.entries.sort_by(|a, b| {
            a.origin
                .label()
                .cmp(&b.origin.label())
                .then_with(|| a.r#type.cmp(&b.r#type))
        });
    }

    /// Seeds a root entry: `path: "/"`, active, typed as given. Spec
    /// 0117 §1: `App::new` calls this at startup only when `--type` or
    /// inference actually resolved a root type — an untyped root gets no
    /// entry at all, leaving the collection empty until the user adds
    /// one. `load_overrides` also calls this (with `Some`/`None` alike)
    /// to re-seed a root baseline when a restored file lacks one.
    pub fn seed_root(&mut self, r#type: Option<String>) {
        self.entries.push(OverrideEntry {
            origin: OverrideOrigin::Path {
                path: "/".to_string(),
            },
            r#type,
            active: true,
            name: None,
            auto: false,
        });
        self.sort();
    }

    /// Creates (or reactivates, if an entry with this exact origin and
    /// type already exists) an override, deactivating every other entry
    /// sharing `origin` (spec 0117 §1's per-origin active invariant).
    /// Always a deliberate, user-driven action (override pane, `type-as`
    /// command) — pins the entry's `auto` flag to `false`, even if it was
    /// previously auto-seeded, since an explicit re-selection through
    /// this path is the user endorsing it. Internal auto-expansion
    /// seeding uses `activate_auto` instead.
    pub fn activate(&mut self, origin: OverrideOrigin, r#type: Option<String>) {
        self.activate_impl(origin, r#type, false);
    }

    /// Like `activate`, but for `render_overrides`'s internal Any/
    /// MessageSet auto-expansion seeding (spec 0120 follow-up): marks the
    /// entry `auto: true`, purely as provenance (see `OverrideEntry::auto`)
    /// — it applies exactly like a manually-activated entry.
    pub fn activate_auto(&mut self, origin: OverrideOrigin, r#type: Option<String>) {
        self.activate_impl(origin, r#type, true);
    }

    fn activate_impl(&mut self, origin: OverrideOrigin, r#type: Option<String>, auto: bool) {
        for e in self.entries.iter_mut() {
            if e.origin == origin {
                e.active = false;
            }
        }
        if let Some(e) = self
            .entries
            .iter_mut()
            .find(|e| e.origin == origin && e.r#type == r#type)
        {
            e.active = true;
            e.auto = auto;
        } else {
            self.entries.push(OverrideEntry {
                origin,
                r#type,
                active: true,
                name: None,
                auto,
            });
        }
        self.sort();
    }

    /// Sets the entry at `idx`'s display-name override (spec 0119 G4's
    /// `e` key) — a direct in-place mutation, not a remove-and-recreate
    /// (unlike `activate`): `name` is not part of an entry's identity,
    /// so this can never create a duplicate or change sort order.
    pub fn rename(&mut self, idx: usize, name: Option<String>) {
        if let Some(entry) = self.entries.get_mut(idx) {
            entry.name = name;
        }
    }

    /// Toggles the entry at `idx` (an index into `entries()`) between
    /// active/inactive (spec 0117 §3's `a` key). Activating deactivates
    /// every other entry sharing its origin. A no-op sort — `active`
    /// isn't part of the sort key, so entry order is unaffected.
    pub fn toggle_active(&mut self, idx: usize) {
        let Some(entry) = self.entries.get(idx) else {
            return;
        };
        let target_active = !entry.active;
        let origin = entry.origin.clone();
        if target_active {
            for e in self.entries.iter_mut() {
                if e.origin == origin {
                    e.active = false;
                }
            }
        }
        self.entries[idx].active = target_active;
    }

    /// Like `toggle_active`, but also cascades the same new active/
    /// inactive state to every entry whose origin sits at-or-under
    /// `idx`'s origin (`origin_is_at_or_under`) — the manage pane's `A`/
    /// Shift-Space/Shift-click.
    ///
    /// Deactivating is unambiguous: every affected entry (regardless of
    /// how many share a given origin) is simply set inactive, same as
    /// `toggle_active` already guarantees no per-origin conflict when
    /// deactivating.
    ///
    /// Activating is not: two entries sharing one origin can never both
    /// be active (the same per-origin invariant `toggle_active`/
    /// `activate` already enforce). Since the collection is always kept
    /// sorted by origin (`sort`), entries sharing an origin are always a
    /// contiguous run — for `idx`'s own run, `idx` itself wins (the
    /// entry the user actually acted on, exactly like `toggle_active`);
    /// for every other affected run, only its first entry (the one
    /// sorted first, i.e. the one the manage pane displays first) wins,
    /// leaving the rest of that run inactive.
    pub fn toggle_active_cascading(&mut self, idx: usize) {
        let Some(entry) = self.entries.get(idx) else {
            return;
        };
        let origin = entry.origin.clone();
        let target_active = !entry.active;

        if !target_active {
            for e in self.entries.iter_mut() {
                if origin_is_at_or_under(&e.origin, &origin) {
                    e.active = false;
                }
            }
            return;
        }

        let mut i = 0;
        while i < self.entries.len() {
            let run_origin = self.entries[i].origin.clone();
            let mut j = i + 1;
            while j < self.entries.len() && self.entries[j].origin == run_origin {
                j += 1;
            }
            if origin_is_at_or_under(&run_origin, &origin) {
                let winner = if run_origin == origin { idx } else { i };
                for (k, e) in self.entries[i..j].iter_mut().enumerate() {
                    e.active = i + k == winner;
                }
            }
            i = j;
        }
    }

    /// Unconditionally activates the entry at `idx` — unlike
    /// `toggle_active`, never flips it off — deactivating every other
    /// entry sharing its origin (same per-origin invariant). The manage
    /// pane's Shift-Up/Shift-Down: moving the highlight and selecting
    /// the destination in one gesture.
    pub fn set_active(&mut self, idx: usize) {
        let Some(entry) = self.entries.get(idx) else {
            return;
        };
        let origin = entry.origin.clone();
        for e in self.entries.iter_mut() {
            if e.origin == origin {
                e.active = false;
            }
        }
        self.entries[idx].active = true;
    }

    /// Removes the entry at `idx` (spec 0117 §3's `Delete`/`Backspace`).
    pub fn remove(&mut self, idx: usize) {
        if idx < self.entries.len() {
            self.entries.remove(idx);
        }
    }

    /// Rotates the origin of the entry at `idx` in place (spec 0124 G2's
    /// `z` key): installs `new_origin` (the caller is responsible for
    /// having already rotated the `OverrideKind` and rederived the
    /// origin — this just installs it) and resets `auto` to `false` (an
    /// explicit user action pins an entry manual, same rule `activate`/
    /// `toggle_active` already apply elsewhere). If the entry is
    /// currently `active`, every *other* entry that now shares its (new)
    /// origin is deactivated — reusing `activate_impl`'s existing
    /// per-origin invariant, not new logic; an inactive entry rotating
    /// onto an origin with an active entry elsewhere leaves that other
    /// entry untouched (duplicates coexist, spec 0124 G3). Returns the
    /// entry's post-`sort()` index (same stability argument as
    /// `duplicate`: the entry is removed then re-pushed last before
    /// sorting, so it lands last among any group sharing its new sort
    /// key).
    pub fn rotate_origin(&mut self, idx: usize, new_origin: OverrideOrigin) -> usize {
        let mut entry = self.entries.remove(idx);
        let active = entry.active;
        entry.origin = new_origin.clone();
        entry.auto = false;
        let r#type = entry.r#type.clone();
        if active {
            for e in self.entries.iter_mut() {
                if e.origin == new_origin {
                    e.active = false;
                }
            }
        }
        self.entries.push(entry);
        self.sort();
        self.entries
            .iter()
            .rposition(|e| e.origin == new_origin && e.r#type == r#type)
            .unwrap_or_else(|| self.entries.len() - 1)
    }

    /// Duplicates the entry at `idx` (manage pane's `D` key): pushes a
    /// raw clone with `active` forced to `false` — bypassing
    /// `activate_impl`'s `(origin, type)` look-up, which would otherwise
    /// just reactivate the existing entry instead of adding a new one —
    /// while keeping `name`/`r#type` as-is. `auto` is forced to `false`:
    /// a duplicate is always a deliberate manual entry, whatever the
    /// original's own auto/manual status — the same rule `activate`/
    /// `rotate_origin` apply to explicit user actions. Returns the new
    /// entry's post-`sort()` index: `sort()` (via `Vec::sort_by`) is
    /// stable and `active`/`name`/`auto` aren't part of the sort key, so
    /// the pushed clone — originally last in the vec — is guaranteed to
    /// land as the *last* entry among those sharing its `origin`/`type`
    /// after sorting.
    pub fn duplicate(&mut self, idx: usize) -> usize {
        let mut clone = self.entries[idx].clone();
        let origin = clone.origin.clone();
        let r#type = clone.r#type.clone();
        clone.active = false;
        clone.auto = false;
        self.entries.push(clone);
        self.sort();
        self.entries
            .iter()
            .rposition(|e| e.origin == origin && e.r#type == r#type)
            .unwrap_or(idx)
    }

    /// Drops every entry whose origin `resolves` rejects (spec 0117 §4:
    /// per-entry drop on restore for origins that no longer match the
    /// current tree/descriptor pool), and returns how many it dropped.
    ///
    /// The count exists because the drop used to be *silent* (spec 0221
    /// S6): a file written against another blob could lose most of its
    /// entries and say nothing. `load_overrides` turns a non-zero count
    /// into one more of the warnings it already returns.
    pub fn retain_resolvable(
        &mut self,
        mut resolves: impl FnMut(&OverrideOrigin) -> bool,
    ) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| resolves(&e.origin));
        before - self.entries.len()
    }
}

// ── YAML save/restore (spec 0117 §4) ────────────────────────────────────────

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Serialize, Deserialize)]
struct YamlFile {
    version: u32,
    target: YamlTarget,
    overrides: Vec<YamlEntry>,
}

#[derive(Serialize, Deserialize)]
pub struct YamlTarget {
    pub blob_sha256: String,
    pub descriptor_set_sha256: String,
}

/// Spec 0128: no `kind` tag — the three variants are structurally
/// disjoint (`Path` has only `path`; `PathField` has `path`+`field`;
/// `FqdnField` has `fqdn`+`field`, no `path` at all), so serde can
/// already tell them apart from which fields are present. Each variant
/// wraps its own named struct (rather than an inline struct-variant)
/// purely so `#[serde(deny_unknown_fields)]` can be applied to it —
/// serde doesn't support that attribute directly on an enum variant.
/// It matters here: without it, `untagged` would happily match a
/// `PathField` mapping (`path`+`field`) against `Path` first (silently
/// dropping the unrecognized `field` key instead of falling through to
/// try `PathField`), since `Path`'s own fields are all present/
/// optional. `deny_unknown_fields` makes that first attempt fail on the
/// stray `field` key, so serde correctly falls through to `PathField`
/// instead. A newtype variant over an untagged enum is transparent on
/// the wire — the inner struct's fields still appear directly in the
/// YAML mapping, no extra nesting.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum YamlEntry {
    Path(YamlPathEntry),
    PathField(YamlPathFieldEntry),
    FqdnField(YamlFqdnFieldEntry),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct YamlPathEntry {
    path: String,
    r#type: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    auto: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct YamlPathFieldEntry {
    path: String,
    field: u64,
    r#type: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    auto: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct YamlFqdnFieldEntry {
    fqdn: String,
    field: u64,
    r#type: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    auto: bool,
}

/// SHA-256 hex digest of `bytes` (spec 0117 §4's `blob_sha256`/
/// `descriptor_set_sha256`, computed over canonicalized binary bytes).
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

impl OverrideCollection {
    /// Serializes the collection to the spec 0117 §4 YAML format.
    pub fn to_yaml(&self, blob_sha256: String, descriptor_set_sha256: String) -> String {
        let overrides = self
            .entries
            .iter()
            .map(|e| match &e.origin {
                OverrideOrigin::Path { path } => YamlEntry::Path(YamlPathEntry {
                    path: path.clone(),
                    r#type: e.r#type.clone(),
                    active: e.active,
                    name: e.name.clone(),
                    auto: e.auto,
                }),
                OverrideOrigin::PathField { path, field } => {
                    YamlEntry::PathField(YamlPathFieldEntry {
                        path: path.clone(),
                        field: *field,
                        r#type: e.r#type.clone(),
                        active: e.active,
                        name: e.name.clone(),
                        auto: e.auto,
                    })
                }
                OverrideOrigin::FqdnField { fqdn, field } => {
                    YamlEntry::FqdnField(YamlFqdnFieldEntry {
                        fqdn: fqdn.clone(),
                        field: *field,
                        r#type: e.r#type.clone(),
                        active: e.active,
                        name: e.name.clone(),
                        auto: e.auto,
                    })
                }
            })
            .collect();
        let file = YamlFile {
            version: 1,
            target: YamlTarget {
                blob_sha256,
                descriptor_set_sha256,
            },
            overrides,
        };
        serde_norway::to_string(&file).expect("OverrideCollection YAML serialization cannot fail")
    }

    /// Parses the spec 0117 §4 YAML format. The entries are re-sorted
    /// here, so the file's own order need not be trusted. Also returns
    /// the recorded target hashes, for the caller to compare against the
    /// currently-loaded blob/descriptor set.
    pub fn from_yaml(text: &str) -> Result<(Self, YamlTarget), String> {
        let file: YamlFile = serde_norway::from_str(text).map_err(|e| {
            format!(
                "malformed overrides file (expected a list of path/field/fqdn \
                 override entries): {e}"
            )
        })?;
        let entries = file
            .overrides
            .into_iter()
            .map(|y| match y {
                YamlEntry::Path(YamlPathEntry {
                    path,
                    r#type,
                    active,
                    name,
                    auto,
                }) => OverrideEntry {
                    origin: OverrideOrigin::Path { path },
                    r#type,
                    active,
                    name,
                    auto,
                },
                YamlEntry::PathField(YamlPathFieldEntry {
                    path,
                    field,
                    r#type,
                    active,
                    name,
                    auto,
                }) => OverrideEntry {
                    origin: OverrideOrigin::PathField { path, field },
                    r#type,
                    active,
                    name,
                    auto,
                },
                YamlEntry::FqdnField(YamlFqdnFieldEntry {
                    fqdn,
                    field,
                    r#type,
                    active,
                    name,
                    auto,
                }) => OverrideEntry {
                    origin: OverrideOrigin::FqdnField { fqdn, field },
                    r#type,
                    active,
                    name,
                    auto,
                },
            })
            .collect();
        let mut collection = OverrideCollection { entries };
        collection.sort();
        Ok((collection, file.target))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_type_fqdns_of_an_empty_pool_is_empty() {
        let pool = DescriptorPool::new();
        assert!(all_type_fqdns(&pool).is_empty());
    }

    #[test]
    fn seed_root_creates_a_single_active_path_entry() {
        let mut collection = OverrideCollection::new();
        collection.seed_root(Some("pkg.Root".to_string()));
        let entries = collection.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].origin,
            OverrideOrigin::Path {
                path: "/".to_string()
            }
        );
        assert_eq!(entries[0].r#type.as_deref(), Some("pkg.Root"));
        assert!(entries[0].active);
    }

    #[test]
    fn activate_deactivates_other_entries_sharing_the_same_origin() {
        let mut collection = OverrideCollection::new();
        let origin = OverrideOrigin::Path {
            path: "/1".to_string(),
        };
        collection.activate(origin.clone(), Some("pkg.A".to_string()));
        collection.activate(origin.clone(), Some("pkg.B".to_string()));
        let entries = collection.entries();
        assert_eq!(entries.len(), 2);
        let a = entries
            .iter()
            .find(|e| e.r#type.as_deref() == Some("pkg.A"))
            .unwrap();
        let b = entries
            .iter()
            .find(|e| e.r#type.as_deref() == Some("pkg.B"))
            .unwrap();
        assert!(!a.active);
        assert!(b.active);

        // Reactivating the first (already-existing) entry flips them back.
        collection.activate(origin, Some("pkg.A".to_string()));
        let entries = collection.entries();
        assert_eq!(entries.len(), 2); // no duplicate created
        let a = entries
            .iter()
            .find(|e| e.r#type.as_deref() == Some("pkg.A"))
            .unwrap();
        let b = entries
            .iter()
            .find(|e| e.r#type.as_deref() == Some("pkg.B"))
            .unwrap();
        assert!(a.active);
        assert!(!b.active);
    }

    #[test]
    fn toggle_active_deactivates_siblings_sharing_the_same_origin() {
        let mut collection = OverrideCollection::new();
        let origin = OverrideOrigin::Path {
            path: "/1".to_string(),
        };
        collection.activate(origin.clone(), Some("pkg.A".to_string()));
        collection.activate(origin, Some("pkg.B".to_string()));
        // After the two `activate` calls above, pkg.B is active, pkg.A is not.
        let idx_a = collection
            .entries()
            .iter()
            .position(|e| e.r#type.as_deref() == Some("pkg.A"))
            .unwrap();
        collection.toggle_active(idx_a);
        let entries = collection.entries();
        assert!(
            entries
                .iter()
                .find(|e| e.r#type.as_deref() == Some("pkg.A"))
                .unwrap()
                .active
        );
        assert!(
            !entries
                .iter()
                .find(|e| e.r#type.as_deref() == Some("pkg.B"))
                .unwrap()
                .active
        );
    }

    /// Activating an entry via `A`/Shift-Space/Shift-click also
    /// activates every entry whose origin sits at-or-under it — a
    /// descendant `Path` origin, a `PathField`
    /// origin at the same path, but not an unrelated sibling path, and
    /// not an `FqdnField` origin (no tree-path relationship to cascade
    /// through). Where a descendant origin has more than one entry
    /// (multiple candidate types), only the one sorted first activates,
    /// keeping the per-origin "at most one active" invariant intact.
    #[test]
    fn toggle_active_cascading_activates_descendants_only_first_per_origin() {
        let mut collection = OverrideCollection::new();
        collection.activate(
            OverrideOrigin::Path {
                path: "/1".to_string(),
            },
            Some("pkg.Root".to_string()),
        );
        collection.activate(
            OverrideOrigin::Path {
                path: "/1/2".to_string(),
            },
            Some("pkg.A".to_string()),
        );
        collection.activate(
            OverrideOrigin::Path {
                path: "/1/2".to_string(),
            },
            Some("pkg.B".to_string()),
        );
        collection.activate(
            OverrideOrigin::PathField {
                path: "/1".to_string(),
                field: 5,
            },
            Some("pkg.Field".to_string()),
        );
        collection.activate(
            OverrideOrigin::Path {
                path: "/10".to_string(),
            },
            Some("pkg.Sibling".to_string()),
        );
        collection.activate(
            OverrideOrigin::FqdnField {
                fqdn: "pkg.Root".to_string(),
                field: 2,
            },
            Some("pkg.Unrelated".to_string()),
        );
        // Deactivate everything first so the cascade's own activation is
        // what's actually under test.
        for i in 0..collection.entries().len() {
            if collection.entries()[i].active {
                collection.toggle_active(i);
            }
        }
        assert!(collection.entries().iter().all(|e| !e.active));

        let root_idx = collection
            .entries()
            .iter()
            .position(|e| e.r#type.as_deref() == Some("pkg.Root"))
            .unwrap();
        collection.toggle_active_cascading(root_idx);

        let active_types: Vec<&str> = collection
            .entries()
            .iter()
            .filter(|e| e.active)
            .map(|e| e.r#type.as_deref().unwrap())
            .collect();
        assert_eq!(
            active_types,
            vec!["pkg.Root", "pkg.A", "pkg.Field"],
            "pkg.Root itself, its descendant /1/2 (first-sorted \
             candidate pkg.A, not pkg.B), and the same-node path-field \
             /1:5 must activate; the unrelated sibling /10 and the \
             fqdn-field origin must not: {:#?}",
            collection.entries()
        );
    }

    /// Deactivating via the cascading toggle deactivates every entry
    /// at-or-under the origin, with no per-origin ambiguity to resolve
    /// (unlike activating).
    #[test]
    fn toggle_active_cascading_deactivates_every_descendant() {
        let mut collection = OverrideCollection::new();
        collection.activate(
            OverrideOrigin::Path {
                path: "/1".to_string(),
            },
            Some("pkg.Root".to_string()),
        );
        collection.activate(
            OverrideOrigin::Path {
                path: "/1/2".to_string(),
            },
            Some("pkg.A".to_string()),
        );
        collection.activate(
            OverrideOrigin::Path {
                path: "/10".to_string(),
            },
            Some("pkg.Sibling".to_string()),
        );

        let root_idx = collection
            .entries()
            .iter()
            .position(|e| e.r#type.as_deref() == Some("pkg.Root"))
            .unwrap();
        collection.toggle_active_cascading(root_idx);

        assert!(
            !collection.entries()[root_idx].active,
            "pkg.Root must deactivate"
        );
        let a_idx = collection
            .entries()
            .iter()
            .position(|e| e.r#type.as_deref() == Some("pkg.A"))
            .unwrap();
        assert!(
            !collection.entries()[a_idx].active,
            "descendant pkg.A must deactivate alongside its ancestor"
        );
        let sibling_idx = collection
            .entries()
            .iter()
            .position(|e| e.r#type.as_deref() == Some("pkg.Sibling"))
            .unwrap();
        assert!(
            collection.entries()[sibling_idx].active,
            "unrelated sibling /10 must be untouched"
        );

        // Reactivate: root and its descendant come back; sibling was
        // never touched by either pass, so it's still active too.
        let root_idx = collection
            .entries()
            .iter()
            .position(|e| e.r#type.as_deref() == Some("pkg.Root"))
            .unwrap();
        collection.toggle_active_cascading(root_idx);
        let active_types: Vec<&str> = collection
            .entries()
            .iter()
            .filter(|e| e.active)
            .map(|e| e.r#type.as_deref().unwrap())
            .collect();
        assert_eq!(active_types, vec!["pkg.Root", "pkg.A", "pkg.Sibling"]);
    }

    #[test]
    fn origin_is_at_or_under_examples() {
        let path = |p: &str| OverrideOrigin::Path {
            path: p.to_string(),
        };
        let path_field = |p: &str, f: u64| OverrideOrigin::PathField {
            path: p.to_string(),
            field: f,
        };
        let fqdn_field = |f: &str, n: u64| OverrideOrigin::FqdnField {
            fqdn: f.to_string(),
            field: n,
        };

        assert!(origin_is_at_or_under(&path("/3/2"), &path("/3")));
        assert!(!origin_is_at_or_under(&path("/30"), &path("/3")));
        assert!(origin_is_at_or_under(&path_field("/3", 5), &path("/3")));
        assert!(origin_is_at_or_under(&path("/3"), &path("/")));
        assert!(!origin_is_at_or_under(
            &fqdn_field("pkg.Msg", 2),
            &path("/3")
        ));
        assert!(!origin_is_at_or_under(
            &fqdn_field("pkg.Msg", 20),
            &fqdn_field("pkg.Msg", 2)
        ));
        assert!(origin_is_at_or_under(
            &fqdn_field("pkg.Msg", 2),
            &fqdn_field("pkg.Msg", 2)
        ));
    }

    #[test]
    fn entries_are_sorted_lexicographically_by_origin_path_not_by_kind_first() {
        // A `PathField` origin ("/1") sorts before a `Path` origin ("/2")
        // that is lexicographically later, even though `Path < PathField`
        // by kind — proving kind is not the primary sort key.
        let mut collection = OverrideCollection::new();
        collection.activate(
            OverrideOrigin::Path {
                path: "/2".to_string(),
            },
            Some("pkg.B".to_string()),
        );
        collection.activate(
            OverrideOrigin::PathField {
                path: "/1".to_string(),
                field: 3,
            },
            Some("pkg.A".to_string()),
        );
        let entries = collection.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].origin,
            OverrideOrigin::PathField {
                path: "/1".to_string(),
                field: 3
            }
        );
        assert_eq!(
            entries[1].origin,
            OverrideOrigin::Path {
                path: "/2".to_string()
            }
        );
    }

    #[test]
    fn remove_drops_the_entry_at_index() {
        let mut collection = OverrideCollection::new();
        collection.seed_root(Some("pkg.Root".to_string()));
        collection.remove(0);
        assert!(collection.entries().is_empty());
    }

    #[test]
    fn retain_resolvable_drops_entries_the_predicate_rejects() {
        let mut collection = OverrideCollection::new();
        collection.activate(
            OverrideOrigin::Path {
                path: "/1".to_string(),
            },
            None,
        );
        collection.activate(
            OverrideOrigin::Path {
                path: "/2".to_string(),
            },
            None,
        );
        collection.retain_resolvable(|origin| match origin {
            OverrideOrigin::Path { path } => path == "/1",
            _ => false,
        });
        assert_eq!(collection.entries().len(), 1);
        assert_eq!(
            collection.entries()[0].origin,
            OverrideOrigin::Path {
                path: "/1".to_string()
            }
        );
    }

    #[test]
    fn yaml_round_trip_preserves_entries_and_target_hashes() {
        let mut collection = OverrideCollection::new();
        collection.seed_root(Some("pkg.Root".to_string()));
        collection.activate(
            OverrideOrigin::Path {
                path: "/1".to_string(),
            },
            None,
        );
        collection.activate(
            OverrideOrigin::PathField {
                path: "/1".to_string(),
                field: 2,
            },
            Some("pkg.Sub".to_string()),
        );
        collection.activate(
            OverrideOrigin::FqdnField {
                fqdn: "pkg.Root".to_string(),
                field: 3,
            },
            Some("pkg.Other".to_string()),
        );

        let yaml = collection.to_yaml("blobhash".to_string(), "deschash".to_string());
        let (restored, target) = OverrideCollection::from_yaml(&yaml).unwrap();
        assert_eq!(target.blob_sha256, "blobhash");
        assert_eq!(target.descriptor_set_sha256, "deschash");
        assert_eq!(restored.entries(), collection.entries());
    }

    #[test]
    fn yaml_omits_active_key_for_inactive_entries() {
        let mut collection = OverrideCollection::new();
        collection.activate(
            OverrideOrigin::Path {
                path: "/1".to_string(),
            },
            None,
        );
        collection.toggle_active(0); // deactivate it
        let yaml = collection.to_yaml("b".to_string(), "d".to_string());
        assert!(!yaml.contains("active"));
    }

    #[test]
    fn sha256_hex_matches_known_digest() {
        // SHA-256 of the empty byte string.
        assert_eq!(
            sha256_hex(&[]),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
