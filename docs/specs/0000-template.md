<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0000 — template

Status: draft | implemented | superseded | informational
Implemented in: YYYY-MM-DD (omit while draft; never a commit SHA — a
        rebase invalidates it)
App: protolens | prototext | reproto | prototext-core | …
Refs: docs/specs/NNNN-….md (what this spec takes from it, in a few
        words — a bare number tells the reader nothing)

## How to write one of these

Delete this section when filling the template in. It is the editorial
rule, kept here because it is the one thing that stops a spec growing
without bound.

**A spec records a decision, not the journey to it.** Keep:

- the decision, stated once;
- the constraint that forced it — what would break under the obvious
  alternative;
- the options dismissed, and *why* they were dismissed (this is the
  part a future reader most needs, and the part most often cut);
- the evidence behind any constant or threshold, next to the constant.

Drop: how the design got where it is, which draft said what, what was
believed before it was measured, and any narration of the order in
which things were tried. That is for historians. We are developers.

**The test for a paragraph:** would a developer changing this code next
month act differently if the paragraph were gone? If not, cut it.

**When a spec is implemented, trim it — do not append to it.** A
prediction the implementation refuted should be replaced by what
actually happened, not left standing beside it. `git log` keeps the
prediction if anyone ever wants it.

**One home per fact.** Evidence justifying a constant belongs in a doc
comment next to the constant, where someone changing it will see it.
The spec may state the conclusion and point at the code. Do not
maintain both copies.

## Background

What is wrong, or what is missing, and how it shows up. Include the
reproduction or the measurement that establishes the problem is real.

## Goals

- **G1.** …
- **G2.** …

## Non-goals

What this deliberately does not do, and why — so that the next reader
does not re-propose it.

- **N1.** …

## Specification

- **S1.** …
- **S2.** …

## Alternatives considered

One short subsection per rejected option: what it was, and the specific
thing that ruled it out. Say plainly if an option was built and failed —
that is worth more than an argument.

## Test plan

1. `a_named_test` — what it establishes.

## Measured outcome

Filled in at implementation. Numbers, the machine and the corpus they
were taken on, and anything the implementation contradicted. State what
was *not* achieved as plainly as what was.
