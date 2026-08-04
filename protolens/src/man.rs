// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Man page generation from the live clap definition (spec 0228 S10).
//!
//! Driven by an environment variable rather than a `protolens-gen-man`
//! binary (which is what `prototext` uses): `protolens` is a bin-only
//! crate whose `Cli` is private to `main.rs`, so a second `[[bin]]`
//! would redeclare every module and compile the whole TUI again.
//!
//! Usage:
//!   PROTOLENS_GEN_MAN=man/man1 protolens

use std::path::PathBuf;

/// Write `<$PROTOLENS_GEN_MAN>/protolens.1` and exit, or return if the
/// variable is unset.
///
/// Called before `Cli::parse()`, so the required `blob` positional does
/// not have to be supplied to generate the page.
pub fn generate_and_exit_if_requested(cmd: clap::Command) {
    let Some(dir) = std::env::var_os("PROTOLENS_GEN_MAN") else {
        return;
    };
    let out_dir = PathBuf::from(dir);
    std::fs::create_dir_all(&out_dir).expect("cannot create output directory");

    let man = clap_mangen::Man::new(cmd)
        .title("PROTOLENS")
        .section("1")
        .source("protolens")
        .manual("User Commands");

    let mut buf = Vec::new();
    man.render(&mut buf).expect("man page rendering failed");
    buf.extend_from_slice(EXTRA_SECTIONS.as_bytes());

    let dest = out_dir.join("protolens.1");
    std::fs::write(&dest, &buf).unwrap_or_else(|e| panic!("cannot write {}: {e}", dest.display()));

    eprintln!("wrote {}", dest.display());
    std::process::exit(0);
}

/// Sections clap cannot derive. The variables that back an option are
/// already rendered as `[env: ...]` in OPTIONS, so ENVIRONMENT lists
/// only the two that are not arguments.
const EXTRA_SECTIONS: &str = r#"
.SH ENVIRONMENT
.TP
\fBPROTOLENS_COMPLETE\fR
When set to \fBbash\fR, \fBzsh\fR, or \fBfish\fR, print a shell completion
script to stdout and exit.
.TP
\fBPROTOLENS_GEN_MAN\fR
When set to a directory, write this man page there as \fBprotolens.1\fR
and exit.
.SH EXAMPLES
.SS Open a blob, inferring its root type from the descriptor set
.PP
.nf
protolens --descriptor-set schemas.desc message.binpb
.fi
.SS Open a blob as raw wire bytes, skipping inference
.PP
.nf
protolens --raw message.binpb
.fi
.SS Export one node without entering the TUI
.PP
.nf
protolens --descriptor-set schemas.desc message.binpb export /1/2
.fi
.SS Enable bash completion
.PP
.nf
source <(PROTOLENS_COMPLETE=bash protolens)
.fi
.SH SEE ALSO
.PP
\fBprototext\fR(1), \fBreproto\fR(1), \fBprotoscan\fR(1)
"#;
