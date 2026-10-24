# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

"""reproto's man pages, rendered from the live Click commands.

``REPROTO_GEN_MAN=<dir> reproto`` writes ``reproto.1`` and
``reproto-instantiate-schema.1`` into ``<dir>`` and exits (spec 0413 S2),
as protoscan and protolens do with their own variables.
"""

from __future__ import annotations

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
    from reproto.cli import _SECTIONS  # import here to avoid heavy imports at module level

    buf: list[str] = []

    from reproto import version as reproto_version
    version = reproto_version()

    buf.append(rf'.TH REPROTO 1 "2026" "reproto {version}" "User Commands"')
    buf.append(r'.SH NAME')
    buf.append(r'reproto \- reconstruct .proto source files from compiled protobuf descriptor sets')
    buf.append(r'.SH SYNOPSIS')
    buf.append(r'.B reproto')
    buf.append(r'[\fIOPTIONS\fR] \fIDESCRIPTOR_FILES\fR...')
    buf.append(r'.SH DESCRIPTION')
    buf.append(
        r'.B reproto'
        '\nreads one or more binary or text\\-format protobuf descriptor sets and'
        '\nregenerates the original \\fI.proto\\fR source files with correct syntax,'
        '\nfield types, and options.'
        '\n.PP'
        '\nIt supports proto2, proto3, and editions syntax, and accepts both'
        '\n\\fI.pb\\fR (binary \\fBFileDescriptorSet\\fR) and \\fI.textpb\\fR'
        '\n(text\\-format) descriptor files as input.'
    )

    # Arguments section
    buf.append(r'.SH ARGUMENTS')
    for param in cmd.params:
        if isinstance(param, click.Argument):
            buf.append(r'.TP')
            buf.append(r'\fBDESCRIPTOR_FILES\fR')
            buf.append(
                r'One or more descriptor files to process.'
                '\nAccepts binary \\fBFileDescriptorSet\\fR, binary \\fBFileDescriptorProto\\fR,'
                '\n\\fB#@\\fR prototext\\-format descriptors, or directories (processed recursively).'
                '\nAny file extension is accepted; \\fI.pb\\fR and \\fI.desc\\fR are conventional.'
                '\nPaths are resolved relative to directories given by \\fB\\-I\\fR (\\fB\\-\\-desc\\-root\\fR),'
                '\nor relative to the current working directory if \\fB\\-I\\fR is not provided.'
            )

    # Options — grouped by _SECTIONS, then ungrouped remainder
    options = [p for p in cmd.params
               if isinstance(p, click.Option) and not getattr(p, 'hidden', False)
               and p.name != 'version']

    # Build ordered list of (section_name, [params]) preserving _SECTIONS order
    seen_sections: list[str] = []
    section_params: dict[str, list[click.Option]] = {}
    ungrouped: list[click.Option] = []

    for param in options:
        long_opts = [o for o in param.opts if o.startswith('--')]
        key = long_opts[0] if long_opts else None
        section = _SECTIONS.get(key) if key else None
        if section:
            if section not in seen_sections:
                seen_sections.append(section)
                section_params[section] = []
            section_params[section].append(param)
        else:
            ungrouped.append(param)

    buf.append(r'.SH OPTIONS')
    for section in seen_sections:
        buf.append(r'.SS ' + section)
        for param in section_params[section]:
            buf.append(r'.TP')
            buf.append(_format_option_synopsis(param))
            help_text = param.help or ''
            buf.append(_roff_escape(help_text))

    if ungrouped:
        buf.append(r'.SS Other')
        for param in ungrouped:
            buf.append(r'.TP')
            buf.append(_format_option_synopsis(param))
            help_text = param.help or ''
            buf.append(_roff_escape(help_text))

    # Version / help always at end
    buf.append(r'.TP')
    buf.append(r'\fB\-\-version\fR')
    buf.append(r'Print version and exit.')
    buf.append(r'.TP')
    buf.append(r'\fB\-\-help\fR')
    buf.append(r'Show a help message and exit.')

    buf.append(r'.SH ENVIRONMENT')
    buf.append(r'.TP')
    buf.append(r'\fBREPROTO_VARIANT\fR')
    buf.append(r'Path to a variant YAML file used when \fB\-\-proto\-variant\fR is not given.')
    buf.append(r'.TP')
    buf.append(r'\fBREPROTO_GEN_MAN\fR')
    buf.append(r'When set to a directory, write the man pages of \fBreproto\fR and')
    buf.append(r'\fBreproto\-instantiate\-schema\fR into it and exit.')
    buf.append(r'.TP')
    buf.append(r'\fB_REPROTO_COMPLETE\fR')
    buf.append(
        r'When set to \fBbash_source\fR, \fBzsh_source\fR or \fBfish_source\fR,'
        '\nprint the completion script for that shell and exit. The scripts'
        r' set it to \fBbash_complete\fR, \fBzsh_complete\fR or'
        r' \fBfish_complete\fR to get the candidates.'
    )

    buf.append(r'.SH NOTES')
    buf.append(
        r'\fBreproto\fR does not support multi\-file \fBFileDescriptorSet\fR inputs'
        '\n'
        r'(i.e.\& \fI.pb\fR files produced with \fBprotoc \-\-include_imports\fR).'
        '\n'
        r'Each \fI.pb\fR file must contain exactly one \fBFileDescriptorProto\fR.'
        '\n'
        r'Pass all \fI.pb\fR files together on the command line and let reproto'
        '\n'
        r'resolve cross\-file imports.'
    )

    buf.append(r'.SH EXAMPLES')
    buf.append(r'.SS Reconstruct a descriptor set')
    buf.append(r'.PP')
    buf.append(
        r'Compile each \fI.proto\fR to its own descriptor set (without \fB\-\-include_imports\fR),'
        '\n'
        r'then pass all \fI.pb\fR files to reproto:'
    )
    buf.append(r'.PP')
    buf.append(r'.nf')
    buf.append(r'protoc \-\-descriptor_set_out=phone_number.pb \-\-proto_path=. phone_number.proto')
    buf.append(r'protoc \-\-descriptor_set_out=address_book.pb \-\-proto_path=. address_book.proto')
    buf.append(r'reproto \-\-use\-variant descriptor \-I . \-O out/ phone_number.pb address_book.pb')
    buf.append(r'.fi')
    buf.append(r'.SS Selective output (one message and its dependencies)')
    buf.append(r'.PP')
    buf.append(r'.nf')
    buf.append(r'reproto \-\-use\-variant descriptor \-I . \-O out/ \-\-seed desc:.tutorial.Person \\')
    buf.append(r'  phone_number.pb address_book.pb')
    buf.append(r'.fi')
    buf.append(r'.SS Using the embedded descriptor variant')
    buf.append(r'.PP')
    buf.append(
        r'The \fB\-\-use\-variant descriptor\fR flag supplies \fIdescriptor.proto\fR from the'
        '\n'
        r'built\-in variant bundle, so no separate descriptor \fI.pb\fR is needed:'
    )
    buf.append(r'.PP')
    buf.append(r'.nf')
    buf.append(r'reproto \-\-use\-variant descriptor \-I . \-O out/ phone_number.pb address_book.pb')
    buf.append(r'.fi')
    buf.append(r'.SS Enable shell completion')
    buf.append(r'.PP')
    buf.append(r'Packages usually install the scripts. Otherwise, in bash, zsh or fish:')
    buf.append(r'.PP')
    buf.append(r'.nf')
    buf.append(r'source <(_REPROTO_COMPLETE=bash_source reproto)')
    buf.append(r'source <(_REPROTO_COMPLETE=zsh_source reproto)')
    buf.append(r'_REPROTO_COMPLETE=fish_source reproto | source')
    buf.append(r'.fi')

    buf.append(r'.SH SEE ALSO')
    buf.append(r'.PP')
    buf.append(r'\fBprotoc\fR(1), \fBprototext\fR(1)')

    return '\n'.join(buf) + '\n'


def write_man_pages(out_dir: Path) -> list[Path]:
    """Write reproto's two man pages into ``out_dir``, creating it, and
    return their paths."""
    from reproto.cli import main as reproto_main
    from reproto.gen_man_instantiate import render as render_instantiate
    from reproto.instantiate_cli import main as instantiate_main

    out_dir.mkdir(parents=True, exist_ok=True)
    pages = {
        'reproto.1': _render(reproto_main),
        'reproto-instantiate-schema.1': render_instantiate(instantiate_main),
    }
    written = []
    for name, text in pages.items():
        dest = out_dir / name
        dest.write_text(text, encoding='utf-8')
        written.append(dest)
    return written
