<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0463 — git-seriate: commits signed in-process

Status: draft
App: git-seriate
Refs: docs/specs/0452-git-seriate-the-store-and-the-history.md (the
        history commits, signed as git signs them)

## Background

`Signer::sign` (`git-seriate/src/history.rs`) spawns `ssh-keygen -Y sign`
for every commit it writes. A downstream front end measured `settle` on
1,118 snapshots: 77.1 s signed, 7.4 s unsigned, about 62 ms a commit, 90%
of the run. Signatures can't be batched: each commit's covers its
parent's id.

## Goals

- **G1.** SSH signatures made in-process, through the agent or with a key
  loaded once, the same signatures git and `ssh-keygen` make.
- **G2.** `ssh-keygen` kept for what the process can't do itself.

## Non-goals

- **N1.** Keeping a passphrase beyond the command, or in the agent: the
  key is decrypted in memory for one command, then dropped. `ssh-add` is
  the user's to run.
- **N2.** `gpg.ssh.program` and OpenPGP or X.509 signing (`gpg.format`
  other than `ssh`): not read today, not here.

## Specification

- **S1. The key,** from `user.signingKey`, as git reads it: a literal
  public key (`key::ssh-…` or `ssh-…`), a path to a public key (`.pub`),
  or a path to a private key; `~/` expanded, as today.

- **S2. The agent.** With `SSH_AUTH_SOCK` set and a public key from S1
  (literal, or the `.pub` file; for a private key path, its public half),
  the agent is asked to sign, if it holds that key. The signature is
  SSHSIG, namespace `git`, hash SHA-512, armored as `ssh-keygen` armors
  it. The connection is opened once per command.

- **S3. A key file,** when no agent holds the key: read once, signed
  in-process.

- **S3b. Its passphrase,** if the key is encrypted, asked for once per
  command, before the first commit is signed:
  - on the terminal (`/dev/tty`, echo off), `Enter passphrase for KEY:`,
    as `ssh-keygen` asks;
  - with no terminal, from `SSH_ASKPASS` if set (and
    `SSH_ASKPASS_REQUIRE` is not `never`), run once with the same prompt;
  - otherwise, or on a wrong passphrase (asked again up to three times on
    a terminal), the command is refused before anything is written: `KEY
    is encrypted and no passphrase could be read; add it to the agent
    (ssh-add KEY)`.

- **S4. `ssh-keygen`,** as today, otherwise: a key type or a key file
  format S2 and S3 don't support, an agent that refuses. Each fallback is
  per command, decided at the first commit, and said once on stderr under
  `--verbose`.

- **S5. Key types:** Ed25519, RSA (`rsa-sha2-512`) and ECDSA (P-256,
  P-384) in-process; others fall back (S4).

## Alternatives considered

**One `ssh-keygen` for several commits.** It signs one message per run,
and each commit's message holds its parent's id: they can't be signed
ahead.

**Signing in a pool of `ssh-keygen` processes.** Same constraint: the
chain is sequential.

## Test plan

1. `signs_like_ssh_keygen_with_a_key_file` — an Ed25519 and an RSA key
   file: the in-process signature is byte for byte `ssh-keygen`'s (both
   are deterministic).
2. `signs_through_the_agent` — an `ssh-agent` started for the test, the
   key added: a history signed through it, each commit accepted by
   `git verify-commit` against an `allowedSignersFile`; ECDSA verified
   the same way (its signatures are not deterministic).
3. `an_encrypted_key_asks_once` — a passphrase-protected key, no agent,
   no terminal, `SSH_ASKPASS` a script counting its runs: a history of
   several commits signed, the script run once; a wrong passphrase
   refused before anything is written.
4. Measured: `settle` on a generated corpus of about 1,000 snapshots,
   signed against unsigned.

## Measured outcome

Filled in at implementation.
