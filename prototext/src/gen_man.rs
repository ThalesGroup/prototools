// SPDX-FileCopyrightText: 2025-2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
// SPDX-FileCopyrightText: 2025-2026 THALES CLOUD SECURISE SAS
//
// SPDX-License-Identifier: MIT

//! `prototext-gen-man` — generate a man page from the live clap definition.
//!
//! Usage:
//!   cargo run -p prototext --bin prototext-gen-man [-- <output-dir>]
//!
//! Generates:
//!   <output-dir>/prototext.1
//!
//! Default output directory: man/man1

use std::path::PathBuf;

fn main() {
    let out_dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "man/man1".to_owned()),
    );

    std::fs::create_dir_all(&out_dir).expect("cannot create output directory");

    let cmd = prototext::command();

    let man = clap_mangen::Man::new(cmd)
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
}

const EXTRA_SECTIONS: &str = r#"
.SH ENVIRONMENT
.TP
\fBPROTOTEXT_COMPLETE\fR
When set to \fBbash\fR, \fBzsh\fR, or \fBfish\fR, print a shell completion
script to stdout and exit.
Used by the shell completion setup described below.
.SH EXAMPLES
.SS Decode a binary protobuf file (schemaless)
.PP
.nf
prototext decode message.binpb
.fi
.SS Decode with a schema
.PP
.nf
prototext --descriptor descriptor.pb decode --type pkg.MyMessage message.binpb
.fi
.SS Decode with inline annotations (required for lossless round-trip)
.PP
.nf
prototext --descriptor descriptor.pb decode -a --type pkg.MyMessage message.binpb
.fi
.SS Encode text back to binary
.PP
.nf
prototext encode message.pb > message.binpb
.fi
.SS Pipe from protoc
.PP
.nf
protoc --encode=pkg.MyMessage descriptor.proto < input.txt | prototext --descriptor descriptor.pb decode --type pkg.MyMessage
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
