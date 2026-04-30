# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

from __future__ import annotations

import logging
import sys
from pathlib import Path

import click
from click.shell_completion import CompletionItem

from reproto import Fqdn
from .reproto import DescriptorProtoMissingError, Options, reproto
from . import variant as variant_mod

logger = logging.getLogger()  # root logger
logger.setLevel(logging.INFO)

# Add special CLIWarningHandlers
class CLIErrorHandler(logging.Handler):
    def emit(self, record):
        if getattr(record, 'cli_err', False):
            click.secho(f'{record.getMessage()}',
                        err=True, fg='red', bold=True)

class CLIWarningHandler(logging.Handler):
    def emit(self, record):
        if getattr(record, 'cli_warn', False):
            click.secho(f'{record.getMessage()}',
                        err=True, fg='yellow', bold=True)

class CLIInfoHandler(logging.Handler):
    def emit(self, record):
        if getattr(record, 'cli_info', False):
            click.secho(f'{record.getMessage()}', err=True)

class CLIAttentionHandler(logging.Handler):
    def emit(self, record):
        if getattr(record, 'cli_attn', False):
            click.secho(f'{record.getMessage()}',
                        err=True, fg='blue', bold=True)

# Only suppress cli_warn messages from root StreamHandlers
class NoCliWarnFilter(logging.Filter):
    def filter(self, record):
        return not (getattr(record, 'cli_err', False)
                    or getattr(record, 'cli_warn', False)
                    or getattr(record, 'cli_info', False)
                    or getattr(record, 'cli_attn', False))

for handler in logger.handlers:
    if (isinstance(handler, logging.StreamHandler)
        and handler.stream in (sys.stdout, sys.stderr)):
        handler.addFilter(NoCliWarnFilter())

logger.addHandler(CLIErrorHandler())
logger.addHandler(CLIWarningHandler())
logger.addHandler(CLIInfoHandler())
logger.addHandler(CLIAttentionHandler())


def complete_pb_files(ctx, param, incomplete: str):
    '''Custom completer for PB_FILES argument (relative to -I).'''
    # pb_path is a tuple when multiple=True
    include_dirs = list(ctx.params.get('pb_path') or [])

    if not include_dirs:
        include_dirs = ['.'    ]

    completions = []

    incomplete_path = Path(incomplete)

    for dir_path in include_dirs:
        # Base directory to search
        base_dir = Path(dir_path)

        # If incomplete ends with '/', treat it as a directory
        if incomplete.endswith('/'):
            search_dir = base_dir / incomplete_path
            prefix = incomplete  # keep the original path as prefix
        else:
            search_dir = base_dir / incomplete_path.parent
            prefix = str(incomplete_path.parent) + ('/' if incomplete_path.parent != Path('.') else '')

        if not search_dir.exists() or not search_dir.is_dir():
            continue

        for child in search_dir.iterdir():
            if incomplete.endswith('/'):
                # No need to filter by last part; show everything in the directory
                name = child.name
            else:
                # Filter by last part of incomplete
                name = child.name
                if not name.startswith(incomplete_path.name):
                    continue

            rel_path = Path(prefix) / name
            if child.is_dir():
                value = str(rel_path) + '/'
            else:
                value = str(rel_path)

            completions.append(value)

    completions.sort()
    return [ CompletionItem(value=c, type='arg') for c in completions ]


_USE_VARIANT_CHOICES = ('any', 'empty', 'timestamp', 'duration', 'struct',
                        'wrappers', 'descriptor', 'all')

# Maps each long option name (or 'PB_FILES' for the argument) to its section
# heading.  Options in the same section appear consecutively in --help output
# (guaranteed by declaration order).  A new heading is printed whenever the
# section changes.
_SECTIONS: dict[str, str] = {
    'PB_FILES':              'Input',
    '--pb-path':             'Input',
    '--proto-out':           'Output',
    '--emit-binary':         'Output',
    '--dry-run':             'Output',
    '--proto-variant':       'Variant / Schema',
    '--descriptor-proto':    'Variant / Schema',
    '--use-variant':         'Variant / Schema',
    '--keep-descriptor-path': 'Variant / Schema',
    '--emit-descriptor':     'Variant / Schema',
    '--seed':                'Filtering',
    '--prune':               'Filtering',
    '--redact-comments':     'Rendering',
    '--redact-orphans':      'Rendering',
    '--go-root':             'Rendering',
    '--phase2-plugin':       'Advanced',
    '--quiet':               'Diagnostics',
    '--graph':               'Diagnostics',
    '--debug':               'Diagnostics',
    '--debug-fqdn':          'Diagnostics',
}


class _SectionedHelpFormatter(click.HelpFormatter):
    """HelpFormatter that inserts section headings between option groups."""

    def write_dl(self, rows, col_max=30, col_spacing=2):  # type: ignore[override]
        # rows is a list of (first_col, second_col) pairs.
        # Scan for options whose leading token matches a _SECTIONS key and
        # inject a blank line + bold heading whenever the section changes.
        current_section: str | None = None
        grouped: list[tuple[str, str]] = []

        for term, help_text in rows:
            # term is e.g. '-I, --pb-path PATH' or 'PB_FILES'
            # Extract the --xxx token; fall back to first word for positionals.
            key = None
            for token in term.replace(',', ' ').split():
                if token.startswith('--'):
                    key = token.split('=')[0]  # strip =VALUE suffix
                    break
            if key is None:
                key = term.split()[0] if term.split() else term

            section = _SECTIONS.get(key)
            if section and section != current_section:
                if grouped:
                    super().write_dl(grouped, col_max=col_max,
                                     col_spacing=col_spacing)
                    grouped = []
                    self.write_paragraph()
                self.write(f'\x1b[1m{section}:\x1b[0m\n')
                current_section = section

            grouped.append((term, help_text))

        if grouped:
            super().write_dl(grouped, col_max=col_max, col_spacing=col_spacing)


class _SectionedContext(click.Context):
    def make_formatter(self) -> click.HelpFormatter:
        return _SectionedHelpFormatter(
            width=self.terminal_width,
            max_width=self.max_content_width,
        )


class _SectionedCommand(click.Command):
    context_class = _SectionedContext


@click.command(
    cls=_SectionedCommand,
    help='Parse PB_FILES and generate output based on the options given.',
)
# --- Input ---
@click.argument(
    'pb_files',
    required=True,
    type=click.Path(path_type=Path),
    nargs=-1,
    shell_complete=complete_pb_files,
)
@click.option(
    '-I',
    '--pb-path',
    type=click.Path(file_okay=False, dir_okay=True, exists=True, path_type=Path),
    multiple=True,
    help='Search path for .pb files (like protoc -I); PB_FILES are resolved relative to these directories',
)

@click.option(
    '--force-proto2-output',
    is_flag=True,
    default=False,
    help='Force all output to proto2 syntax, regardless of the input syntax. '
         'Without this flag, output syntax matches the input (polyglot mode).',
)

@click.option(
    '-o', '--proto-out',
    required=True,
    type=click.Path(exists=True, file_okay=False, dir_okay=True, writable=True, path_type=Path),
    help='Output directory for generated proto files',
)

@click.option(
    '-b', '--emit-binary',
    is_flag=True,
    help='Also write binary descriptor files (.pb) alongside .proto output',
)

@click.option(
    '--dry-run',
    is_flag=True,
    help='Do not actually create .proto files',
)

@click.option(
    '--proto-variant',
    required=False,
    type=click.Path(exists=True, dir_okay=False, path_type=Path),
    default=None,
    help=(
        'Path to a variant specification YAML file. '
        'If omitted, $REPROTO_VARIANT is used; '
        'otherwise the built-in google.protobuf default applies.'
    ),
)

@click.option(
    '-d', '--descriptor-proto',
    is_flag=True,
    help='Shorthand for --use-variant descriptor',
)

@click.option(
    '--use-variant',
    'use_variant',
    type=click.Choice(_USE_VARIANT_CHOICES),
    multiple=True,
    help=(
        'Use the variant\'s embedded copy of a well-known file instead of '
        'whatever the input .pb files contain. '
        'Accepts: any, empty, timestamp, duration, struct, wrappers, descriptor, all. '
        'May be repeated.'
    ),
)

@click.option(
    '--keep-descriptor-path',
    is_flag=True,
    help='Keep the variant descriptor import path as-is (do not rewrite to google/protobuf/descriptor.proto)',
)

@click.option(
    '--emit-descriptor',
    is_flag=True,
    help='Write descriptor.proto to the output directory (suppressed by default)',
)

@click.option(
    '-s',
    '--seed', 'seeds',
    type=str,
    multiple=True,
    help='Fully-qualified name to treat as an output root (e.g. .my.package.MyMessage)',
)

@click.option(
    '-p',
    '--prune', 'stumps',
    type=str,
    multiple=True,
    help='Fully-qualified name to exclude from output (e.g. .my.package.MyMessage)',
)

@click.option(
    '--redact-comments',
    is_flag=True,
    help='Redact comments from reconstructed .proto files',
)

@click.option(
    '--redact-orphans',
    is_flag=True,
    help='Redact orphans from reconstructed .proto files',
)

@click.option(
    '--go-root',
    default=None,
    required=False,
    type=str,
    help='Force go_package option in reconstructed .proto files',
)

@click.option(
    '--phase2-plugin',
    type=click.Path(path_type=Path),
    help=(
        'Python file executed as a transformation hook during '
        'phase 2. Must define phase2_plugin(ctx, fdp).'
    ),
)

@click.option(
    '--quiet',
    is_flag=True,
    help='Suppress progress messages',
)

@click.option(
    '--graph',
    required=False,
    type=click.Path(file_okay=True, dir_okay=False, writable=True, path_type=Path),
    help='Write the FQDN dependency graph to FILE (HTML/pyvis format)',
)

@click.option(
    '--debug',
    is_flag=True,
    help='Print detailed debug information (development only)',
)

@click.option(
    '--debug-fqdn',
    is_flag=True,
    help='Print detailed information about FQDNs (development only)',
)

def main(
        pb_files: list[Path],
        pb_path: list[Path],
        force_proto2_output: bool,
        proto_out: Path,
        emit_binary: bool,
        dry_run: bool,
        proto_variant: Path | None,
        descriptor_proto: bool,
        use_variant: tuple[str, ...],
        keep_descriptor_path: bool,
        emit_descriptor: bool,
        seeds: list[str],
        stumps: list[str],
        redact_comments: bool,
        redact_orphans: bool,
        go_root: str,
        quiet: bool,
        graph: Path | None,
        phase2_plugin: str | None,
        debug: bool,
        debug_fqdn: bool,
):
    '''
    Parse PB_FILES and generate output based on the options given.
    '''
    # Load variant (path arg > REPROTO_VARIANT env > built-in google-protobuf.yaml)
    variant = variant_mod.load(str(proto_variant) if proto_variant else None)

    if phase2_plugin:
        source = open(phase2_plugin).read()
        phase2_plugin_function = compile(source, phase2_plugin, 'exec')
    else:
        phase2_plugin_function = None

    well_known = variant['variant_well_known']  # maps canonical path -> variant path
    # Reverse map: canonical name -> variant path (or canonical if not remapped)
    def _wk(canonical: str) -> str:
        return well_known.get(canonical, canonical)

    # Expand --use-variant / -d into the fallback_protos list
    use_variant_set: set[str] = set(use_variant)
    if descriptor_proto:
        use_variant_set.add('descriptor')
    if 'all' in use_variant_set:
        use_variant_set = {'any', 'empty', 'timestamp', 'duration',
                           'struct', 'wrappers', 'descriptor'}

    fallback_protos: list[str] = []
    if 'descriptor' in use_variant_set:
        fallback_protos.append(variant['variant_descriptor_proto'])
    if 'any' in use_variant_set:
        fallback_protos.append(_wk('google/protobuf/any.proto'))
    if 'empty' in use_variant_set:
        fallback_protos.append(_wk('google/protobuf/empty.proto'))
    if 'timestamp' in use_variant_set:
        fallback_protos.append(_wk('google/protobuf/timestamp.proto'))
    if 'duration' in use_variant_set:
        fallback_protos.append(_wk('google/protobuf/duration.proto'))
    if 'struct' in use_variant_set:
        fallback_protos.append(_wk('google/protobuf/struct.proto'))
    if 'wrappers' in use_variant_set:
        fallback_protos.append(_wk('google/protobuf/wrappers.proto'))

    options = Options(
        binary=emit_binary,
        force_proto2_output=force_proto2_output,
        debug=debug,
        debug_fqdn=debug_fqdn,
        descriptor_proto=variant['variant_descriptor_proto'],
        fallback_protos=fallback_protos,
        keep_variant_descriptor=keep_descriptor_path,
        **{k: v for k, v in variant.items()},
        write_variant_descriptor=emit_descriptor,
        dry_run=dry_run,
        go_root=go_root,
        graph=graph,
        quiet=quiet,
        redact_comments=redact_comments,
        redact_orphans=redact_orphans,
        phase2_plugin=phase2_plugin_function,
    )
    try:
        reproto(
            list(pb_path) if pb_path else [Path('.')],
            pb_files,
            [Fqdn(s) for s in seeds],
            [Fqdn(p) for p in stumps],
            proto_out,
            options,
        )
    except DescriptorProtoMissingError:
        raise click.ClickException('Could not find descriptor.proto')

if __name__ == '__main__':
    main()
