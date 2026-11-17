// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! The man pages, from the live clap definitions (spec 0433 S7, spec 0424
//! S14), as protoscan's and protolens's are: `PROTOBACK_GEN_MAN=<dir>
//! protoback` writes `protoback.1` and `protoback-instantiate.1`, the
//! sections clap cannot derive taken from reproto's pages (`gen_man.py`,
//! `gen_man_instantiate.py`).

use std::path::{Path, PathBuf};

/// Write both pages into `$PROTOBACK_GEN_MAN` and exit, or return when the
/// variable is unset. Called before the command line is parsed.
pub fn generate_and_exit_if_requested() {
    let Some(dir) = std::env::var_os("PROTOBACK_GEN_MAN") else {
        return;
    };
    let dir = PathBuf::from(dir);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("Error: {}: {e}", dir.display());
        std::process::exit(1);
    }
    let main = <crate::cli::Cli as clap::CommandFactory>::command();
    let instantiate = crate::instantiate::command();
    for (cmd, name, extra) in [
        (main, "protoback", MAIN_EXTRA),
        (instantiate, "protoback-instantiate", INSTANTIATE_EXTRA),
    ] {
        if let Err(e) = write(&dir, cmd, name, extra) {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
    std::process::exit(0);
}

fn write(dir: &Path, cmd: clap::Command, name: &'static str, extra: &str) -> Result<(), String> {
    let man = clap_mangen::Man::new(cmd.name(name))
        .title(name.to_uppercase())
        .section("1")
        .source("protoback")
        .manual("User Commands");
    let mut buf = Vec::new();
    man.render(&mut buf).map_err(|e| e.to_string())?;
    if name == "protoback" {
        buf.extend_from_slice(warnings_section().as_bytes());
    }
    buf.extend_from_slice(extra.as_bytes());
    let dest = dir.join(format!("{name}.1"));
    std::fs::write(&dest, &buf).map_err(|e| format!("{}: {e}", dest.display()))?;
    eprintln!("wrote {}", dest.display());
    Ok(())
}

/// The warning codes, which `--fail-on` takes (spec 0438).
fn warnings_section() -> String {
    let mut out = String::from(
        ".SH WARNINGS\nEach warning line reads \\fBWarning [\\fICODE\\fB]:\\fR \\fImessage\\fR; \
         \\fB\\-\\-fail\\-on\\fR \\fICODE\\fR makes a code fail the run (exit status 3).\n",
    );
    for (code, meaning) in crate::events::CODES {
        out.push_str(&format!(
            ".TP\n\\fB{code}\\fR\n{}\n",
            meaning.replace('-', "\\-")
        ));
    }
    out.push_str(
        ".TP\n\\fBA1\\fR to \\fBD2\\fR\nthe anomalies a reconstructed \\fI.proto\\fR \
         records in its comments\n",
    );
    out
}

/// `protoback.1`'s sections clap cannot derive.
const MAIN_EXTRA: &str = r#".SH ENVIRONMENT
.TP
\fBREPROTO_VARIANT\fR
Path to a variant YAML file used when \fB\-\-proto\-variant\fR is not given.
.TP
\fBPROTOBACK_PYTHON\fR
The Python interpreter \fB\-\-fdp\-plugin\fR runs plugins with (default:
\fBpython3\fR).
.TP
\fBPROTOBACK_GEN_MAN\fR
When set to a directory, write the man pages of \fBprotoback\fR and
\fBprotoback instantiate\fR into it and exit.
.TP
\fBPROTOBACK_COMPLETE\fR
When set to \fBbash\fR, \fBzsh\fR or \fBfish\fR, print the completion script
for that shell and exit. DESCRIPTOR_FILES complete relative to the \fB\-I\fR
roots already on the command line, or to the members of an \fB\-I\fR blob.
.SH NOTES
\fBprotoback\fR does not support multi\-file \fBFileDescriptorSet\fR inputs
(i.e.\& \fI.pb\fR files produced with \fBprotoc \-\-include_imports\fR).
Each \fI.pb\fR file must contain exactly one \fBFileDescriptorProto\fR.
Pass all \fI.pb\fR files together on the command line and let protoback
resolve cross\-file imports.
.SH EXAMPLES
.SS Reconstruct a descriptor set
Compile each \fI.proto\fR to its own descriptor set (without \fB\-\-include_imports\fR),
then pass all \fI.pb\fR files to protoback:
.PP
.nf
protoc \-\-descriptor_set_out=phone_number.pb \-\-proto_path=. phone_number.proto
protoc \-\-descriptor_set_out=address_book.pb \-\-proto_path=. address_book.proto
protoback \-\-use\-variant descriptor \-I . \-O out/ phone_number.pb address_book.pb
.fi
.SS Selective output (one message and its dependencies)
.nf
protoback \-\-use\-variant descriptor \-I . \-O out/ \-\-seed desc:.tutorial.Person \\
  phone_number.pb address_book.pb
.fi
.SS Using the embedded descriptor variant
The \fB\-\-use\-variant descriptor\fR flag supplies \fIdescriptor.proto\fR from the
built\-in variant bundle, so no separate descriptor \fI.pb\fR is needed:
.PP
.nf
protoback \-\-use\-variant descriptor \-I . \-O out/ phone_number.pb address_book.pb
.fi
.SS Enable shell completion
Packages usually install the scripts. Otherwise, in bash, zsh or fish:
.PP
.nf
source <(PROTOBACK_COMPLETE=bash protoback)
source <(PROTOBACK_COMPLETE=zsh protoback)
PROTOBACK_COMPLETE=fish protoback | source
.fi
.SH SEE ALSO
\fBprotoback\-instantiate\fR(1), \fBreproto\fR(1), \fBprotoc\fR(1), \fBprototext\fR(1)
"#;

/// `protoback-instantiate.1`'s sections clap cannot derive.
const INSTANTIATE_EXTRA: &str = r#".SH ENVIRONMENT
.TP
\fBPROTOTEXT_DESCRIPTOR_SET\fR
Path to a \fI.desc\fR FileDescriptorSet used as the default when
\fB\-\-descriptor\-set\fR is not provided.
\fBPROTOTEXT_DEFAULT_DESCRIPTOR\fR is accepted as a deprecated fallback.
.SH EXAMPLES
Generate two instances using the googleapis DB:
.PP
.nf
protoback instantiate \-\-descriptor\-set googleapis.desc \-\-seed 42 \\
  \-O out/ google.type.PostalAddress google.protobuf.Timestamp
.fi
.SH SEE ALSO
\fBprotoback\fR(1), \fBreproto\-instantiate\-schema\fR(1), \fBprototext\fR(1)
"#;
