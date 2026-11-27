<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0495 — units: derived from the evidence and the caller's directives

Status: draft
App: git-seriate, git-seriate-fdp
Refs: docs/specs/0494-extract-records-the-evidence.md
        (the evidence units are derived from),
      docs/specs/0452-git-seriate-the-store-and-the-history.md
        (snapshots and history commits),
      docs/specs/0451-seriation-the-sort-as-a-library.md
        (the sort, which then orders units),
      docs/specs/0464-git-seriate-a-canonical-history-taken-as-it-is.md
        (the trust stamp, which directives join)

## Background

The sort assumes that what it orders is the state of one source repository
at one revision. A snapshot, one input as extracted, often isn't, both
ways:

- **too large:** one input holds several sources' copies. A database
  client embeds a schema bundle built elsewhere and earlier; a Go
  executable holds googleapis at one version, the protobuf runtime's
  well-known types at another, its own files at its revision. Measured
  downstream: about 150 of a history's 160 straddles traced to one
  executable holding an embedded bundle's copies beside its own.
- **too small:** one build spreads over many inputs. A downstream's
  runfiles tree, one Python program built at one revision, makes 1,039
  snapshots of one module each; a release tarball's binaries are one
  build. Cut so, they give the sort almost nothing to compare, and no
  lineage evidence (no file imports another).

Packaging is not the boundary: a container is evidence at most. Nor can
the boundaries be fixed once at extraction: evidence improves (a newer
extractor), the caller learns its archives better, the sort shows what
both missed. So units are derived, each time `settle` runs, from what the
snapshots recorded (0494) and from directives the caller gives; nobody
edits a snapshot or a history commit by hand.

## Goals

- **G1.** The sort orders units: the copies of one source repository at
  one revision, as far as the evidence and the directives tell.
- **G2.** Units recomputed at each `settle`, from the stored snapshots and
  the directives alone: no input needed again, unless new evidence has to
  be read from it (then `extract` again, 0494 S6).
- **G3.** The caller decides what only it knows (which inputs are one
  build, which paths are vendored); the evidence decides what the bytes
  tell (which code registers a copy).
- **G4.** Without directives, and without evidence, a unit is a snapshot,
  as today.

## Non-goals

- **N1.** Lineages: a later spec, deciding units' producers the same way.
- **N2.** Ref names for units: units live in history commits' blocks, not
  in refs, so a member's name needs no escaping for git; snapshots keep
  their identities, as today.
- **N3.** Restricting a unit's comparisons in phase 3: a unit split off is
  compared with every copy of its files, as a snapshot is; its relation to
  them (contained, equal, incomparable) is what phase 3, and later the
  lineages, work from.

## Specification

- **S1. Who decides what:**

  | decision | by | from |
  |---|---|---|
  | these inputs are one build | the caller: `group` directive | the archive's structure (a runfiles root, a wheel, a release) |
  | this directory is a unit of its own | the caller: `split` directive | paths it knows (`third_party/…`, a vendored package) |
  | this split is wrong | the caller: `keep` directive | its review, the sort's report |
  | these copies come from another source | the evidence | a copy's registrant (0494 S1): a Go module other than the build's own, a C++ library's Bazel repository other than the build's |
  | these copies are a set embedded in raw bytes | the evidence | contiguity: adjacent ranges (0494 S1) whose copies the build's own don't explain; its rule from Q1's measurement |
  | what neither caught | `settle`, reported | the straddles left after the sort; phase 3's votes against build time |

- **S2. The directives,** `units.yaml` in the history's record
  (`refs/seriate/histories/<history>`), versioned with it: each a pattern
  on snapshot identities or groups (`group`), or on paths within a unit
  (`split`, `keep`), a name, and a reason. A pattern captures: `{field}`
  matches one path segment, `**` the rest, and a directive yields one
  group per distinct value of its captures, named by its name template,
  so one directive serves every release:

  ```yaml
  - group: "{package}/{version}/{part}/{tree}.runfiles/**"
    name: "{package}/{version}/{tree}"
    reason: one runfiles tree, one build
  ```

  A directive is identified by a short digest of its meaning: its kind,
  patterns and name template, parsed, its reason left out, so that an
  edited comment, reason or indentation changes nothing (S9). A changed
  directive is a new record; the next `settle` recomputes the units.
- **S3. Deriving the units,** before the sort: group the snapshots
  `group` names; within each build, split off the copies whose registrant
  names another source, then the sets embedded in raw bytes (contiguity),
  then the paths `split` names, unless `keep` says otherwise; the rest of
  a build, one unit.
- **S4. A unit's identity and record:** a group's name, or a snapshot's
  identity, then `!<part>` for a part split off (a module, a repository, a
  path). Its history commit's block names its constituents: the snapshots,
  and which part of each.
- **S5. A unit's build time:** a zip member's entry time (0494 S2) where
  plausible, later than the known sentinels (the DOS epoch 1980-01-01,
  the Unix epoch, 2010-01-01) and not later than its input's; else its
  inputs' latest, an upper bound; which one, recorded.
- **S6. Format-1 snapshots** carry no evidence: only directives apply to
  them, and without any, each is a unit as today.
- **S7. `settle --explain`** lists the units derived, with what made each
  (directive, evidence), before the sort's report.
- **S8. Directives and the cutoff:** `settle` doesn't re-sort below its
  cutoff; a directive matching commits below it is reported, never
  silently left unapplied: "directive `<name>` matches commits below the
  cutoff: settle with `--no-cutoff` to apply it".
- **S9. Directives in the trust stamp:** a history commit's block records
  the directives that shaped it, `directives: {<name>: <digest>}`, as
  `canon_rules` records rule sets (spec 0464), and the kept-prefix check
  compares them. A changed directive rewrites the commits it matches,
  not every commit since the cutoff.

## Open questions

- **Q1.** Contiguity's rule, the first split a downstream needs: its
  embedded schema bundles sit in raw bytes, not in container members
  (several bundles clustered within 3 MB of a 4 GB archive, each copy
  once, the compiled copies recurring once per executable). To measure on
  0494's ranges, there and on a synthetic bundle in a synthetic
  executable: where each set starts and ends, and whether fdp-scan's
  order cuts (spec 0449) fall at those bounds.
- **Q2.** A unit's upper-bound time in phase 1: deferred until a
  measurement shows a need (phase 3's window already compares an
  embedded set with its input's neighbors).

## Test plan

1. No directive, no evidence: units equal snapshots, the sort's result
   unchanged.
2. `group` over three single-file snapshots: one unit of three files,
   one history commit naming its three constituents.
3. A synthetic Go-like snapshot whose copies record two registrants: two
   units, the foreign module's as `<identity>!<module>`.
4. `split` on a path, then `keep` on it: two units, then one.
5. A directive changed: the next `settle` recomputes, nothing else
   rewritten than what the new units change.
6. S5: a sentinel entry time ignored, a plausible one taken, the choice
   recorded.
7. S2: a capturing `group` over two releases' runfiles trees: two units,
   named by the template.
8. S8: a directive matching only commits below the cutoff: reported, the
   history unchanged; applied with `--no-cutoff`.
9. S9: a directive edited: only the commits it matches get a new stamp.

## Measured outcome

Filled in at implementation.
