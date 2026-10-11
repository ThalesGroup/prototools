# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""The man page of reproto-instantiate-schema, rendered from its live Click
command. reproto.gen_man writes it beside reproto's own, when the
REPROTO_GEN_MAN variable names a directory (spec 0413 S2)."""

from __future__ import annotations

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


def render(cmd: click.Command) -> str:
    buf: list[str] = []

    from reproto import version as reproto_version
    version = reproto_version()

    buf.append(rf'.TH REPROTO\-INSTANTIATE\-SCHEMA 1 "2026" "reproto {version}" "User Commands"')
    buf.append(r'.SH NAME')
    buf.append(
        r'reproto\-instantiate\-schema \- generate pseudo\-random binary protobuf instances'
        '\nfrom a FileDescriptorSet'
    )
    buf.append(r'.SH SYNOPSIS')
    buf.append(r'.B reproto\-instantiate\-schema')
    buf.append(r'[\fIOPTIONS\fR] \fIFQDNS\fR...')
    buf.append(r'.SH DESCRIPTION')
    buf.append(r'.B reproto\-instantiate\-schema')
    buf.append(r'reads a compiled \fI.desc\fR FileDescriptorSet and generates one pseudo\-random')
    buf.append(r'binary protobuf instance per fully\-qualified message type name supplied.')
    buf.append(r'.PP')
    buf.append(r'Each instance is written to \fI<output\-root>/<fqdn\-with\-dots\-as\-slashes>.pb\fR.')
    buf.append(r'Types that produce empty instances (e.g. messages with no fields) are silently skipped.')

    buf.append(r'.SH ARGUMENTS')
    buf.append(r'.TP')
    buf.append(r'\fBFQDNS\fR')
    buf.append(r'One or more fully\-qualified message type names to instantiate')
    buf.append(r'(e.g. \fIgoogle.type.PostalAddress\fR).')

    options = [p for p in cmd.params
               if isinstance(p, click.Option) and not getattr(p, 'hidden', False)
               and p.name != 'version']

    buf.append(r'.SH OPTIONS')
    for param in options:
        buf.append(r'.TP')
        buf.append(_format_option_synopsis(param))
        buf.append(_roff_escape(param.help or ''))

    buf.append(r'.TP')
    buf.append(r'\fB\-\-version\fR')
    buf.append(r'Print version and exit.')
    buf.append(r'.TP')
    buf.append(r'\fB\-\-help\fR')
    buf.append(r'Show a help message and exit.')

    buf.append(r'.SH ENVIRONMENT')
    buf.append(r'.TP')
    buf.append(r'\fBPROTOTEXT_DESCRIPTOR_SET\fR')
    buf.append(r'Path to a \fI.desc\fR FileDescriptorSet used as the default when')
    buf.append(r'\fB\-\-descriptor\-set\fR is not provided.')
    buf.append(r'\fBPROTOTEXT_DEFAULT_DESCRIPTOR\fR is accepted as a deprecated fallback.')

    buf.append(r'.SH EXAMPLES')
    buf.append(r'.PP')
    buf.append(r'Generate two instances using the googleapis DB:')
    buf.append(r'.PP')
    buf.append(r'.nf')
    buf.append(
        r'reproto\-instantiate\-schema \-\-descriptor\-set googleapis.desc \-\-seed 42 \\'
    )
    buf.append(r'  \-O out/ google.type.PostalAddress google.protobuf.Timestamp')
    buf.append(r'.fi')

    buf.append(r'.SH SEE ALSO')
    buf.append(r'.PP')
    buf.append(r'\fBprototext\fR(1), \fBreproto\fR(1)')

    return '\n'.join(buf) + '\n'
