// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

//! protocraft — emit a named fixture's wire bytes to stdout.
//!
//! Usage: protocraft [-f <registry>] <fixture-name>
//!
//! -f <registry>   select fixture registry: craft_a (default)
//!
//! Example:
//!   cargo test --test protocraft -- hidden | prototext -d ...

#[path = "../src/protocraft/mod.rs"]
mod protocraft;

use protocraft::craft_a;

use std::io::Write;

type FixtureRegistry = &'static [(&'static str, fn() -> Vec<u8>)];

fn usage(prog: &str) -> ! {
    eprintln!("Usage: {prog} [-f <registry>] <fixture-name>");
    eprintln!("  -f <registry>   craft_a (default)");
    std::process::exit(2);
}

fn usage_ok(prog: &str) -> ! {
    eprintln!("Usage: {prog} [-f <registry>] <fixture-name>");
    eprintln!("  -f <registry>   craft_a (default)");
    std::process::exit(0);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let prog = &args[0];

    let mut registry = "craft_a";
    let mut fixture_name: Option<&str> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-f" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("{prog}: -f requires an argument");
                    usage(prog);
                }
                registry = &args[i];
            }
            arg if arg.starts_with('-') => {
                eprintln!("{prog}: unknown option '{arg}'");
                usage(prog);
            }
            name => {
                if fixture_name.is_some() {
                    eprintln!("{prog}: unexpected argument '{name}'");
                    usage(prog);
                }
                fixture_name = Some(name);
            }
        }
        i += 1;
    }

    let fixture_name = fixture_name.unwrap_or_else(|| {
        usage_ok(prog);
    });

    let fixtures: FixtureRegistry = match registry {
        "craft_a" => craft_a::ALL_FIXTURES,
        other => {
            eprintln!("{prog}: unknown registry '{other}' (known: craft_a)");
            std::process::exit(2);
        }
    };

    let func = fixtures
        .iter()
        .find(|(name, _)| *name == fixture_name)
        .map(|(_, f)| f)
        .unwrap_or_else(|| {
            eprintln!("{prog}: fixture '{fixture_name}' not found in registry '{registry}'");
            std::process::exit(1);
        });

    let bytes = func();
    std::io::stdout()
        .write_all(&bytes)
        .expect("failed to write to stdout");
}
