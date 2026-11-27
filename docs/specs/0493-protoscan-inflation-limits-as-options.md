<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0493 — protoscan: the inflation limits as options

Status: implemented
Implemented in: 2026-11-27
App: protoscan
Refs: docs/specs/0480-fdp-scan-inflates-zip-and-gzip-members.md
        (`--inflate` and its limits, `InflateLimits`)

## Background

`protoscan --inflate` reads zip and gzip members within fdp-scan's default
limits: 256 MiB per member, 1 GiB per input. Release archives exceed them:
a `.tar.gz` is one gzip member, and a project's release tarball can inflate
past 1 GiB; a Python wheel can hold a library of more than 256 MiB.
Measured on two: a tarball of 354 MB, all of it skipped (`member-limit`), 0
descriptors found, 436 with the limits raised; a wheel, 629 found, 799 with
them raised. The limits protect a scan from a decompression bomb, and a
caller who knows its inputs can afford more.

## Goals

- **G1.** Let the caller raise or lower the limits, per run.
- **G2.** Unchanged without the options: the defaults stay fdp-scan's.

## Non-goals

- **N1.** The depth limit (3): no input has needed more.
- **N2.** The same options on `git seriate-fdp extract`: when a corpus
  needs them there.

## Specification

- **S1. `--max-member MIB`:** the most one member may inflate to; a larger
  one is skipped, `member-limit`. Default: fdp-scan's, in MiB.
- **S2. `--max-total MIB`:** the most the input may inflate to in all; past
  it, members are skipped, `input-limit`. Default: fdp-scan's, in MiB.
- **S3.** Both require `--inflate`, and are refused without it.

## Alternatives considered

- **Higher defaults:** memory follows the largest member at each level; a
  default sized for the largest release archives would be a bomb's
  invitation for every other caller.

## Test plan

1. `max_member_bounds_what_a_member_may_inflate_to`: a member over the
   limit skipped, the same member read under the default.
2. `the_limits_require_inflate`.

## Measured outcome

With `--max-member 4096 --max-total 8192`, the tarball above: 27 s and
1.2 GB of memory; the wheel: 22 s and 1.0 GB.
