<!--
SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS

SPDX-License-Identifier: MIT
-->

# Contributing guidelines

Thank you for your interest in contributing to `prototools`!

## How to contribute

### Reporting issues

Open an issue on [GitHub](https://github.com/ThalesGroup/prototools/issues).
Please include a minimal reproducer and, for decoding/encoding bugs, the
exact bytes involved (e.g. as a `printf '\xNN...'` one-liner).

### Submitting a pull request

1. Fork the repository and create a branch from `main`.
2. Make your changes.
3. Check it: in the development shell (`nix-shell dev-shell.nix`), run
   `check` after each edit, and `check --release` before pushing: it runs
   the CI the PR will be held to (formatting, clippy, the tests, the
   Python checks). See [`BUILDING.md`](BUILDING.md).
4. Open a pull request against `main` with a clear description of the
   change and its motivation.

### Coding style

- Rust formatting is enforced by `cargo fmt` (rustfmt defaults).
- Clippy warnings are treated as errors (`-D warnings`).
- Keep changes focused — one logical change per PR.

### Testing

New functionality should be accompanied by tests, in the crate it
changes (`<crate>/tests/`, or a `#[cfg(test)]` module). protoback's
regression suite, ported from reproto's, is `protoback/tests/suite/`.
`check` runs them all; [`BUILDING.md`](BUILDING.md) says how.

## License

By contributing, you agree that your contributions will be licensed
under the MIT license — see [`LICENSES/MIT.txt`](LICENSES/MIT.txt).
