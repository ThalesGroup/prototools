# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

from __future__ import annotations

import collections.abc
import logging
import sys
from pathlib import Path

import click

try:
    from importlib.metadata import version as _pkg_version
    _reproto_version = _pkg_version('reproto')
except Exception:
    _reproto_version = 'dev'
from click.shell_completion import CompletionItem

logger = logging.getLogger()  # root logger
logger.setLevel(logging.INFO)

# Add special CLIWarningHandlers
class CLIErrorHandler(logging.Handler):
    def emit(self, record: logging.LogRecord) -> None:
        if getattr(record, 'cli_err', False):
            click.secho(f'{record.getMessage()}',
                        err=True, fg='red', bold=True)

class CLIWarningHandler(logging.Handler):
    def emit(self, record: logging.LogRecord) -> None:
        if getattr(record, 'cli_warn', False):
            click.secho(f'{record.getMessage()}',
                        err=True, fg='yellow', bold=True)

class CLIInfoHandler(logging.Handler):
    def emit(self, record: logging.LogRecord) -> None:
        if getattr(record, 'cli_info', False):
            click.secho(f'{record.getMessage()}', err=True)

class CLIAttentionHandler(logging.Handler):
    def emit(self, record: logging.LogRecord) -> None:
        if getattr(record, 'cli_attn', False):
            click.secho(f'{record.getMessage()}',
                        err=True, fg='blue', bold=True)

# Only suppress cli_warn messages from root StreamHandlers
class NoCliWarnFilter(logging.Filter):
    def filter(self, record: logging.LogRecord) -> bool:
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


def _complete_paths(
    incomplete: str,
    base_dirs: list[str],
    *,
    dirs_only: bool = False,
    suffixes: tuple[str, ...] | None = None,
) -> list[CompletionItem]:
    '''Shared path completion logic.'''
    completions = []
    incomplete_path = Path(incomplete)

    for dir_path in base_dirs:
        base_dir = Path(dir_path)

        if incomplete.endswith('/'):
            search_dir = base_dir / incomplete_path
            prefix = incomplete_path
        else:
            search_dir = base_dir / incomplete_path.parent
            prefix = incomplete_path.parent

        if not search_dir.exists() or not search_dir.is_dir():
            continue

        for child in search_dir.iterdir():
            name = child.name
            if not incomplete.endswith('/') and not name.startswith(incomplete_path.name):
                continue

            rel_path = prefix / name
            if child.is_dir():
                completions.append(str(rel_path) + '/')
            elif not dirs_only:
                if suffixes is None or any(name.endswith(s) for s in suffixes):
                    completions.append(str(rel_path))

    completions.sort()
    return [CompletionItem(value=c, type='arg') for c in completions]


def complete_pb_files(ctx: click.Context, param: click.Parameter, incomplete: str):
    '''Custom completer for PB_FILES argument (relative to -I).'''
    include_dirs = list(ctx.params.get('pb_path') or ['.'])
    items = _complete_paths(incomplete, include_dirs, suffixes=('.pb', '.textpb'))
    # Re-tag as 'arg_I' so the shell script knows these paths are relative to
    # a -I directory (possibly outside CWD) and must not use -o filenames.
    return [CompletionItem(value=c.value, type='arg_I') for c in items]


def complete_any_path(ctx: click.Context, param: click.Parameter, incomplete: str):
    '''Complete any file or directory path relative to cwd.'''
    return _complete_paths(incomplete, ['.'])


def complete_dir_path(ctx: click.Context, param: click.Parameter, incomplete: str):
    '''Complete directory paths only, relative to cwd.'''
    return _complete_paths(incomplete, ['.'], dirs_only=True)


def complete_yaml_path(ctx: click.Context, param: click.Parameter, incomplete: str):
    '''Complete .yaml files and directories relative to cwd.'''
    return _complete_paths(incomplete, ['.'], suffixes=('.yaml',))


def complete_py_path(ctx: click.Context, param: click.Parameter, incomplete: str):
    '''Complete .py files and directories relative to cwd.'''
    return _complete_paths(incomplete, ['.'], suffixes=('.py',))


_USE_VARIANT_CHOICES = ('any', 'empty', 'timestamp', 'duration', 'struct',
                        'wrappers', 'descriptor',
                        'source_context', 'field_mask', 'type', 'api',
                        'all')

# Maps each long option name (or 'PB_FILES' for the argument) to its section
# heading.  Options in the same section appear consecutively in --help output
# (guaranteed by declaration order).  A new heading is printed whenever the
# section changes.
_SECTIONS: dict[str, str] = {
    'PB_FILES':              'Input',
    '--pb-path':             'Input',
    '--output-root':         'Output',
    '--emit-binary':         'Output',
    '--dry-run':             'Output',
    '--proto-variant':       'Variant / Schema',
    '--use-variant':         'Variant / Schema',
    '--keep-descriptor-path': 'Variant / Schema',
    '--emit-descriptor':     'Variant / Schema',
    '--seed':                'Filtering',
    '--prune':               'Filtering',
    '--force-proto2-output': 'Rendering',
    '--prost-workaround':    'Rendering',
    '--redact-comments':     'Rendering',
    '--redact-orphans':      'Rendering',
    '--go-root':             'Rendering',
    '--build-schema-db':      'Advanced',
    '--emit-scoring-graphs':  'Advanced',
    '--phase2-plugin':       'Advanced',
    '--keep-duplicates':     'Advanced',
    '--detailed-warnings':   'Diagnostics',
    '--quiet':               'Diagnostics',
    '--graph':               'Diagnostics',
    '--debug':               'Diagnostics',
    '--debug-fqdn':          'Diagnostics',
}


class _SectionedHelpFormatter(click.HelpFormatter):
    """HelpFormatter that inserts section headings between option groups."""

    def write_dl(self, rows: collections.abc.Sequence[tuple[str, str]], col_max: int = 30, col_spacing: int = 2) -> None:
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
@click.version_option(
    version=_reproto_version,
    prog_name='reproto',
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
    '-O', '--output-root',
    'proto_out',
    required=False,
    default=None,
    type=click.Path(file_okay=False, dir_okay=True, path_type=Path),
    shell_complete=complete_dir_path,
    help='Output directory for generated proto files (created if absent)',
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
    '--build-schema-db',
    'build_schema_db',
    required=False,
    default=None,
    type=click.Path(dir_okay=False, writable=True, path_type=Path),
    help=(
        'Build the full schema DB at PATH (must end in .desc): '
        'writes PATH (FileDescriptorSet of all loaded FDPs), '
        'PATH-stem/hopcroft.rkyv (compiled scoring graph), and '
        'PATH-stem/index.rkyv (lazy-loading FDS index). '
        'YAML and FDPs stay in memory; no intermediate files are written.'
    ),
)

@click.option(
    '--emit-scoring-graphs',
    is_flag=True,
    help='Write per-file scoring-graph YAML files alongside .proto output under --output-root',
)

@click.option(
    '--proto-variant',
    required=False,
    type=click.Path(exists=True, dir_okay=False, path_type=Path),
    default=None,
    shell_complete=complete_yaml_path,
    help=(
        'Path to a variant specification YAML file. '
        'If omitted, $REPROTO_VARIANT is used; '
        'otherwise the built-in google.protobuf default applies.'
    ),
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
    help='Fully-qualified name to treat as an output root (e.g. desc:my.pkg.MyMsg or file:foo.proto)',
)

@click.option(
    '-p',
    '--prune', 'stumps',
    type=str,
    multiple=True,
    help='Fully-qualified name to exclude from output (e.g. desc:my.pkg.MyMsg or file:foo.proto)',
)

@click.option(
    '--force-proto2-output',
    is_flag=True,
    default=False,
    help='Force all output to proto2 syntax, regardless of the input syntax. '
         'Without this flag, output syntax matches the input (polyglot mode).',
)

@click.option(
    '--prost-workaround',
    is_flag=True,
    default=False,
    help=(
        'Patch editions-syntax FDPs to appear as proto2 in output and '
        'schemas.pb, working around a prost-reflect limitation '
        '(upstream PR #1347). Editions fields using non-default features '
        '(LEGACY_REQUIRED, DELIMITED, EXPANDED, IMPLICIT) will be '
        'rendered incorrectly. Remove once prost-reflect supports editions.'
    ),
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
    shell_complete=complete_py_path,
    help=(
        'Python file executed as a transformation hook during '
        'phase 2. Must define phase2_plugin(ctx, fdp).'
    ),
)

@click.option(
    '--keep-duplicates',
    is_flag=True,
    help=(
        'Do not prune files whose symbols conflict with already-loaded files '
        '(legacy behaviour; by default conflicting files are silently pruned '
        'and a warning is emitted).'
    ),
)

@click.option(
    '--detailed-warnings',
    is_flag=True,
    help=(
        'Print every warning occurrence immediately as it is emitted '
        '(default: squash repeated warnings and show a count summary at the end).'
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
    '--dump-resolved-features',
    default='',
    hidden=True,
    help='(diagnostic) Dump resolved FeatureSet YAML for the named proto file and exit',
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
        prost_workaround: bool,
        proto_out: Path | None,
        emit_binary: bool,
        dry_run: bool,
        build_schema_db: Path | None,
        emit_scoring_graphs: bool,
        proto_variant: Path | None,
        use_variant: tuple[str, ...],
        keep_descriptor_path: bool,
        emit_descriptor: bool,
        seeds: list[str],
        stumps: list[str],
        redact_comments: bool,
        redact_orphans: bool,
        go_root: str,
        keep_duplicates: bool,
        detailed_warnings: bool,
        quiet: bool,
        graph: Path | None,
        phase2_plugin: str | None,
        dump_resolved_features: str,
        debug: bool,
        debug_fqdn: bool,
):
    '''
    Parse PB_FILES and generate output based on the options given.
    '''
    from reproto import Fqdn
    from .reproto import DescriptorProtoMissingError, Options, reproto
    from . import variant as variant_mod
    from .lib.warnings import configure_collector
    configure_collector(detailed=detailed_warnings)

    # --output-root is required unless --dump-resolved-features is set.
    if proto_out is None and not dump_resolved_features:
        raise click.UsageError('Missing option \'-O\' / \'--output-root\'.')
    if proto_out is not None:
        proto_out.mkdir(parents=True, exist_ok=True)

    # Load variant (path arg > REPROTO_VARIANT env > built-in google-protobuf.yaml)
    variant = variant_mod.load(str(proto_variant) if proto_variant else None)

    if phase2_plugin:
        source = Path(phase2_plugin).read_text(encoding='utf-8')
        phase2_plugin_function = compile(source, phase2_plugin, 'exec')
    else:
        phase2_plugin_function = None

    well_known = variant['variant_well_known']  # maps canonical path -> variant path
    # Reverse map: canonical name -> variant path (or canonical if not remapped)
    def _wk(canonical: str) -> str:
        return well_known.get(canonical, canonical)

    # Expand --use-variant / -d into the fallback_protos list
    use_variant_set: set[str] = set(use_variant)
    if 'all' in use_variant_set:
        use_variant_set = {
            'any', 'empty', 'timestamp', 'duration',
            'struct', 'wrappers', 'descriptor',
            'source_context', 'field_mask', 'type', 'api',
        }

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
    # Leaves before dependents so the topo-sort sees them in the right order.
    if 'source_context' in use_variant_set:
        fallback_protos.append(_wk('google/protobuf/source_context.proto'))
    if 'field_mask' in use_variant_set:
        fallback_protos.append(_wk('google/protobuf/field_mask.proto'))
    if 'type' in use_variant_set:
        fallback_protos.append(_wk('google/protobuf/type.proto'))
    if 'api' in use_variant_set:
        fallback_protos.append(_wk('google/protobuf/api.proto'))

    if build_schema_db is not None and not str(build_schema_db).endswith('.desc'):
        raise click.UsageError('--build-schema-db PATH must end in .desc')

    options = Options(
        binary=emit_binary,
        build_schema_db=build_schema_db,
        emit_scoring_graphs=emit_scoring_graphs,
        force_proto2_output=force_proto2_output,
        prost_workaround=prost_workaround,
        debug=debug,
        debug_fqdn=debug_fqdn,
        descriptor_proto=variant['variant_descriptor_proto'],
        dump_resolved_features=dump_resolved_features,
        fallback_protos=fallback_protos,
        keep_variant_descriptor=keep_descriptor_path,
        variant_descriptor_proto=variant['variant_descriptor_proto'],
        variant_well_known=variant['variant_well_known'],
        variant_import_rules=variant['variant_import_rules'],
        variant_ns_rules=variant['variant_ns_rules'],
        variant_orphans=variant['variant_orphans'],
        variant_root=variant['variant_root'],
        variant_stem=variant['variant_stem'],
        variant_annotation_modules=variant['variant_annotation_modules'],
        write_variant_descriptor=emit_descriptor,
        dry_run=dry_run,
        go_root=go_root,
        graph=graph,
        keep_duplicates=keep_duplicates,
        detailed_warnings=detailed_warnings,
        quiet=quiet,
        redact_comments=redact_comments,
        redact_orphans=redact_orphans,
        phase2_plugin=phase2_plugin_function,
    )
    def _normalise_fqdn(s: str) -> Fqdn:
        """Accept FQDNs without a leading dot after the prefix for non-file nodes.

        E.g. 'desc:my.pkg.Foo' → 'desc:.my.pkg.Foo'; 'file:foo.proto' unchanged.
        """
        if ':' in s:
            prefix, rest = s.split(':', 1)
            if prefix != 'file' and rest and not rest.startswith('.'):
                return Fqdn(f'{prefix}:.{rest}')
        return Fqdn(s)

    try:
        reproto(
            list(pb_path) if pb_path else [Path('.')],
            pb_files,
            [_normalise_fqdn(s) for s in seeds],
            [_normalise_fqdn(p) for p in stumps],
            proto_out if proto_out is not None else Path('.'),
            options,
        )
    except DescriptorProtoMissingError:
        raise click.ClickException('Could not find descriptor.proto')

if __name__ == '__main__':
    main()
