// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! `seriation-gen [--name=value]… OUT`: write a synthetic corpus (spec 0451
//! S5) under OUT. The options are those of `seriation::generate::Params`;
//! `manifest.json` records them.

use std::path::PathBuf;
use std::process::ExitCode;

use seriation::generate::{generate, Params};

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let Some(out) = args.pop().filter(|a| !a.starts_with("--")) else {
        eprintln!("usage: seriation-gen [--name=value]... OUT");
        return ExitCode::from(2);
    };
    let params = match Params::from_args(args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("seriation-gen: {e}");
            return ExitCode::from(2);
        }
    };
    let out = PathBuf::from(out);
    if out.exists() {
        eprintln!(
            "seriation-gen: {} exists; give a new directory",
            out.display()
        );
        return ExitCode::from(2);
    }
    match generate(&params).write(&out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("seriation-gen: writing {}: {e}", out.display());
            ExitCode::FAILURE
        }
    }
}
