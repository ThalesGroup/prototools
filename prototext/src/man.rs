// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

//! Man page generation from the live clap definition (spec 0413 S3).
//!
//! Driven by an environment variable, as protoscan's and protolens's are,
//! rather than a separate `prototext-gen-man` binary that users would never
//! run.
//!
//! Usage:
//!   PROTOTEXT_GEN_MAN=man/man1 prototext

use std::path::PathBuf;

/// Write `<$PROTOTEXT_GEN_MAN>/prototext.1` and exit, or return if the
/// variable is unset.
///
/// Called before `Cli::parse()`, so no subcommand has to be supplied to
/// generate the page.
pub fn generate_and_exit_if_requested() {
    let Some(dir) = std::env::var_os("PROTOTEXT_GEN_MAN") else {
        return;
    };
    let out_dir = PathBuf::from(dir);
    std::fs::create_dir_all(&out_dir).expect("cannot create output directory");

    let man = clap_mangen::Man::new(prototext::command())
        .title("PROTOTEXT")
        .section("1")
        .source("prototext")
        .manual("User Commands");

    let mut buf = Vec::new();
    man.render(&mut buf).expect("man page rendering failed");

    // Append extra roff sections.
    buf.extend_from_slice(EXTRA_SECTIONS.as_bytes());

    let dest = out_dir.join("prototext.1");
    std::fs::write(&dest, &buf).unwrap_or_else(|e| panic!("cannot write {}: {e}", dest.display()));

    eprintln!("wrote {}", dest.display());
    std::process::exit(0);
}

const EXTRA_SECTIONS: &str = r#"
.SH ENVIRONMENT
.TP
\fBPROTOTEXT_COMPLETE\fR
When set to \fBbash\fR, \fBzsh\fR, or \fBfish\fR, print a shell completion
script to stdout and exit.
Used by the shell completion setup described below.
.TP
\fBPROTOTEXT_GEN_MAN\fR
When set to a directory, write this man page there as \fBprototext.1\fR
and exit.
.SH EXAMPLES
.SS Decode a binary protobuf file (schemaless)
.PP
.nf
prototext decode message.binpb
.fi
.SS Decode with a schema
.PP
.nf
prototext --descriptor-set descriptor.pb decode --type pkg.MyMessage message.binpb
.fi
.SS Decode with inline annotations (on by default; use --no-annotations for clean output)
.PP
.nf
prototext --descriptor-set descriptor.pb decode --type pkg.MyMessage message.binpb
.fi
.SS Encode text back to binary
.PP
.nf
prototext encode message.pb > message.binpb
.fi
.SS Pipe from protoc
.PP
.nf
protoc --encode=pkg.MyMessage descriptor.proto < input.txt | prototext --descriptor-set descriptor.pb decode --type pkg.MyMessage
.fi
.SS Decode without annotations but still show unknown fields (default since spec 0103)
.PP
.nf
prototext --descriptor-set my.desc decode --type pkg.MyMessage --no-annotations message.binpb
.fi
.SS Suppress unknown fields for protoc-compatible output
.PP
.nf
prototext --descriptor-set my.desc decode --type pkg.MyMessage \\
    --no-annotations --hide-unknown-fields message.binpb
.fi
.SS Suppress MessageSet expansion only (Any fields still expand)
.PP
.nf
prototext --descriptor-set my.desc decode --type pkg.MyMessage \\
    --no-expand-message-set message.binpb
.fi
.SS Enable bash completion
.PP
.nf
source <(PROTOTEXT_COMPLETE=bash prototext)
.fi
.SH SEE ALSO
.PP
\fBprotoc\fR(1)
"#;
