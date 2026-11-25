<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0482 — scan_inflating lets the caller decline a scan

Status: implemented
Implemented in: 2026-11-25
App: fdp-scan
Refs: docs/specs/0480-fdp-scan-inflates-zip-and-gzip-members.md (the walk,
        its visits and limits, G5 on memory),
      docs/specs/0481-fdp-scan-reads-java-class-files.md (a class file's
        decoded strings, the buffer scanned in its place)

## Background

`scan_inflating` scans every buffer it reaches, the input and each member,
before the caller sees it. A caller that already knows which buffers can't
hold what it looks for pays for scans it then throws away.

A downstream field search ("which descriptors describe this field of this
message?") prefilters each file with a multi-pattern search for the
query's message names: a file without a match is not scanned. On a 4 GB
package of 46 634 files, members counted, 9 to 29 files pass, and a
search takes about 3 s, mostly reading and inflating. Moved onto
`scan_inflating` as it is, every one of the 46 634 buffers would be
scanned first; the caller estimates that doubles the search's time (its
extractor, which scans everything, takes 7 to 8 s on that package).

The prefilter can't run before the walk: a deflated member's names only
show once it is inflated. Nor can a declined buffer stop the walk: an
outer file without a match can hold a deflated descriptor set that has
one.

## Goals

- **G1. The caller can decline a buffer's scan,** buffer by buffer, once
  its bytes are known and before they are scanned.
- **G2. A declined buffer is still walked:** its members are found, read
  and offered in turn.
- **G3. Nothing else changes:** limits, skips, the visit order, and
  `scan_inflating` itself.

## Non-goals

- **N1. No pruning of the walk.** A caller can't stop the walk from
  descending into a buffer: G2 is what makes declining safe.
- **N2. No change to protoscan or extract,** which scan everything.

## Specification

- **S1. The function:**

  ```rust
  pub fn scan_inflating_filtered(
      data: &[u8],
      graph: &LoadedGraph,
      root: &str,
      options: &ScanOptions,
      limits: &InflateLimits,
      scan: impl FnMut(&Origin, &[u8]) -> bool,
      visit: impl FnMut(Visit<'_>),
  );
  ```

  `scan_inflating_with` is this function with `|_, _| true`, and
  `scan_inflating` that against the embedded schema, as today. A caller
  of the filtered form passes the schema it wants: a filtering caller is
  one that already holds its own, and a variant for each combination
  would double the entry points for no caller.
- **S2. When `scan` is called:** once per buffer the walk would scan,
  with its origin and bytes, before scanning it. That is the input, each
  member read, and for a class file its decoded strings (origin ending
  `class`), never its raw bytes, which are not scanned (0481 S3).
- **S3. Declined** (`false`):
  - the buffer is not scanned, and gets no `Visit::Scanned`: a visit
    still means "scanned";
  - its members are still looked for, checked (0480 S2), read and walked,
    each offered to `scan` in turn;
  - a stored member of a declined buffer has its descriptors only if it
    is accepted itself: a declined parent has none to give it, as an
    accepted parent gives its stored members' to them (0480 S3);
  - a class file whose decoded strings are declined gets no visit at all.
- **S4. Unchanged:** `Visit::Skipped`, every limit and its accounting (a
  declined buffer's members count toward them as before), the order of
  the visits that are made.
- **S5. Cost:** a declined buffer costs the member search (two
  signatures, `memchr::memmem`) and nothing else. The predicate's own
  cost is the caller's.

## Alternatives considered

- **Filtering in the visitor.** The scan is paid before the visitor sees
  the buffer: the cost G1 is about.
- **A field of `InflateLimits`, or of `ScanOptions`.** A closure is not
  a limit, and `ScanOptions` is how one buffer is scanned, passed to
  every buffer; neither type could stay `Copy` and `Default`.
- **A `Scanned` visit with an empty `Explained` for a declined buffer.**
  It would read as "scanned, found nothing", which is not what happened.
  A caller wanting every buffer listed sees each one in its predicate.

## Test plan

1. `declined_buffers_are_walked`: a zip whose outer buffer is declined
   and whose deflated member is accepted: the member's descriptor is
   visited, the outer buffer isn't.
2. `declining_everything_scans_nothing`: `|_, _| false` on the test
   inputs of 0480: no `Scanned` visit, the same `Skipped` visits as
   without filtering.
3. `accepting_everything_is_scan_inflating`: `|_, _| true` gives exactly
   `scan_inflating_with`'s visits.
4. `the_predicate_sees_decoded_class_strings`: on a class file, `scan` is
   offered the decoded strings, origin ending `class`.

## Measured outcome

2026-11-25: tests 1 to 4 pass, on inputs holding every kind of buffer
(clear, gzip, stored and deflated zip members, a class file, an
encrypted member). `scan_inflating_with` is now the filtered form with
`|_, _| true`, and gives the same visits as before, `Debug` for
`Debug`.

Not measured: the time a declining caller saves. It depends on the
caller's predicate and inputs; the downstream user who asked for it will
measure it on their packages.
