<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0472 — git-seriate: sign from the key file first

Status: draft
App: git-seriate
Refs: docs/specs/0463-git-seriate-signing-in-process.md (S2 to S5: the
        agent, the key file, its passphrase, `ssh-keygen`; the order this
        changes)

## Background

0463 signs through the SSH agent whenever it holds `user.signingKey`, and
from the key file only when no agent does. A downstream front end
measured `settle` of 1,118 commits, signed:

| | elapsed | CPU |
|---|---|---|
| `ssh-keygen` per commit (before 0463) | 77.1 s | |
| in-process, through the agent | 27.2 s | 5.0 s |
| in-process, from the key file (agent unset) | 4.5 s | 4.0 s |

The 20 ms a commit through the agent is its round trip: the agent is
forwarded over SSH from the user's own machine, so every signature crosses
the network. The key is also a file the process can read without a
passphrase.

## Goals

- **G1.** A key file readable without a passphrase is signed from
  directly; the agent is asked only for what the file can't do.

## Non-goals

- **N1.** A setting choosing between the agent and the file: S1's order
  serves both cases.
- **N2.** RSA key files in-process (0463 S5, RUSTSEC-2023-0071): they
  still go to the agent, then `ssh-keygen`.

## Specification

- **S1. The order** (replacing 0463's S2 to S4 order; their mechanics
  unchanged):
  1. `user.signingKey` names a private key file, unencrypted, of a type
     signed in-process (Ed25519, ECDSA P-256 and P-384): signed from it.
  2. Else, an agent (`SSH_AUTH_SOCK`) holding the key: signed through it.
     That covers a literal or `.pub` key, an encrypted key file whose key
     the agent holds (no passphrase asked), and an RSA key file.
  3. Else, an encrypted key file of an in-process type: its passphrase
     asked for once (0463 S3b), then signed from it.
  4. Else `ssh-keygen` (0463 S4).

- **S2. Why the file first is safe.** A private key readable without a
  passphrase gives whoever can read it what the agent would; what the file
  skips is an agent's per-use confirmation (`ssh-add -c`), which can't
  protect a key readable on disk. A key that should only be used through
  the agent is kept encrypted, or out of `user.signingKey` as a file.

## Test plan

1. `an_unencrypted_key_file_is_used_before_the_agent` — an agent holding
   the key, and the unencrypted key file in `user.signingKey`: no request
   reaches the agent (an agent socket that counts connections), the
   history signed, `git verify-commit` good.
2. `an_encrypted_key_file_prefers_the_agent` — the same key encrypted,
   held by the agent: signed through it, no passphrase asked (an
   `SSH_ASKPASS` that fails if run).
3. `an_rsa_key_file_goes_to_the_agent` — as 0463's.
4. 0463's tests, unchanged.

## Measured outcome

Filled in at implementation.
