# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""protoscan-gen-man — generate a man page from the live Click command definition.

Usage:
    protoscan-gen-man [<output-dir>]

Generates:
    <output-dir>/protoscan.1

Default output directory: man/man1
"""

from __future__ import annotations

import sys
from pathlib import Path

import click


def _roff_escape(text: str) -> str:
    """Escape characters special in roff."""
    return text.replace('\\', '\\\\').replace('-', r'\-').replace("'", r"\'")


def _format_option_synopsis(param: click.Option) -> str:
    """Format option declaration for the .TP term line."""
    parts = []
    for opt in param.opts:
        parts.append(r'\fB' + _roff_escape(opt) + r'\fR')
    term = ', '.join(parts)
    if not param.is_flag:
        mv = param.metavar or param.type.name.upper()
        term += r' \fI' + mv + r'\fR'
    return term


def _render(cmd: click.Command) -> str:
    buf: list[str] = []

    import importlib.metadata
    try:
        version = importlib.metadata.version('protoscan')
    except importlib.metadata.PackageNotFoundError:
        version = 'dev'

    buf.append(rf'.TH PROTOSCAN 1 "2026" "protoscan {version}" "User Commands"')
    buf.append(r'.SH NAME')
    buf.append(r'protoscan \- scan binary files for embedded protobuf FileDescriptorProto blobs')
    buf.append(r'.SH SYNOPSIS')
    buf.append(r'.B protoscan')
    buf.append(r'[\fIOPTIONS\fR] \fIFILE\fR')
    buf.append(r'.SH DESCRIPTION')

    description = (cmd.help or '').strip()
    buf.append(_roff_escape(description))

    # Arguments
    buf.append(r'.SH ARGUMENTS')
    for param in cmd.params:
        if isinstance(param, click.Argument):
            buf.append(r'.TP')
            buf.append(r'\fB' + (param.name or 'FILE').upper() + r'\fR')
            buf.append(r'Binary file to scan.')

    # Options
    options = [p for p in cmd.params
               if isinstance(p, click.Option) and not getattr(p, 'hidden', False)]

    buf.append(r'.SH OPTIONS')
    for param in options:
        buf.append(r'.TP')
        buf.append(_format_option_synopsis(param))
        buf.append(_roff_escape(param.help or ''))

    buf.append(r'.TP')
    buf.append(r'\fB\-\-help\fR')
    buf.append(r'Show a help message and exit.')

    buf.append(r'.SH SEE ALSO')
    buf.append(r'.PP')
    buf.append(r'\fBprototext\fR(1), \fBreproto\fR(1)')

    return '\n'.join(buf) + '\n'


def _build_stub_cmd() -> click.Command:
    """Build a stub Click command that matches protoscan's CLI signature.

    Avoids importing protoscan.cli (which pulls in the compiled fdp_scan_lib
    extension, unavailable in plain-Python environments such as CI man-page
    generation).
    """
    @click.command()
    @click.argument("file", type=click.Path(exists=False, dir_okay=False))
    @click.option(
        "--proto_out",
        type=click.Path(file_okay=False),
        default=None,
        help="Directory to write extracted .pb files.",
    )
    def _cmd(file: str, proto_out: str | None) -> None:
        """Scan file for embedded FileDescriptorProto (.proto) blobs and optionally
        write them under PROTO_OUT directory."""

    return _cmd  # type: ignore[return-value]


def main() -> None:
    out_dir = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("man/man1")
    out_dir.mkdir(parents=True, exist_ok=True)

    text = _render(_build_stub_cmd())

    dest = out_dir / "protoscan.1"
    dest.write_text(text, encoding="utf-8")
    print(f"wrote {dest}", file=sys.stderr)


if __name__ == "__main__":
    main()
