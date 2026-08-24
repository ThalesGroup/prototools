# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

from __future__ import annotations

import collections.abc
import logging
import sys
from pathlib import Path

import click

from .lib.console import rprint

try:
    from importlib.metadata import version as _pkg_version
    _reproto_version = _pkg_version('reproto')
except Exception:
    _reproto_version = 'dev'
from click.shell_completion import CompletionItem

logger = logging.getLogger()  # root logger
logger.setLevel(logging.INFO)

# Add special CLIWarningHandlers

# Set to True once --quiet is parsed; gates CLIWarningHandler.emit().
_quiet: bool = False

class CLIErrorHandler(logging.Handler):
    def emit(self, record: logging.LogRecord) -> None:
        if getattr(record, 'cli_err', False):
            rprint(record.getMessage(), style='bold red')

class CLIWarningHandler(logging.Handler):
    def emit(self, record: logging.LogRecord) -> None:
        if getattr(record, 'cli_warn', False) and not _quiet:
            rprint(record.getMessage(), style='bold yellow')

class CLIInfoHandler(logging.Handler):
    def emit(self, record: logging.LogRecord) -> None:
        if getattr(record, 'cli_info', False):
            rprint(record.getMessage())

class CLIAttentionHandler(logging.Handler):
    def emit(self, record: logging.LogRecord) -> None:
        if getattr(record, 'cli_attn', False):
            rprint(record.getMessage(), style='bold blue')

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


def _complete_blob_members(blob: Path, incomplete: str) -> list[CompletionItem]:
    '''Complete against the members of a blob -I root (spec 0243 S12).

    Whole member paths, prefix-matched — not one path segment at a time
    as _complete_paths does for a directory. There is no directory to
    descend into here, and the shell script already inserts whole paths
    for -I-relative candidates.
    '''
    from .load import blob_members
    try:
        members = blob_members(blob)
    except Exception:
        return []
    return [
        CompletionItem(value=str(rel_path), type='arg')
        for rel_path, _ in sorted(members)
        if str(rel_path).startswith(incomplete)
    ]


def complete_pb_files(ctx: click.Context, param: click.Parameter, incomplete: str):
    '''Custom completer for DESCRIPTOR_FILES argument (relative to -I).'''
    include_dirs = list(ctx.params.get('pb_path') or ['.'])
    blobs = [Path(d) for d in include_dirs if Path(d).is_file()]
    dirs = [d for d in include_dirs if Path(d) not in blobs]
    items = _complete_paths(incomplete, dirs, suffixes=('.pb', '.textpb'))
    for blob in blobs:
        items.extend(_complete_blob_members(blob, incomplete))
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

# Maps each long option name (or 'DESCRIPTOR_FILES' for the argument) to its
# section heading.  Options in the same section appear consecutively in --help
# output (guaranteed by declaration order).  A new heading is printed whenever
# the section changes.
_SECTIONS: dict[str, str] = {
    'DESCRIPTOR_FILES':      'Input',
    '--desc-root':           'Input',
    '--proto-out':           'Output',
    '--no-proto-out':        'Output',
    '--emit-binary':         'Output',
    '--source-info':         'Output',
    '--dry-run':             'Output',
    '--proto-variant':       'Variant / Schema',
    '--use-variant':         'Variant / Schema',
    '--keep-descriptor-path': 'Variant / Schema',
    '--no-emit-descriptor':  'Variant / Schema',
    '--seed':                'Filtering',
    '--prune':               'Filtering',
    '--filter-config':       'Filtering',
    '--force-proto2-output': 'Rendering',
    '--force-proto2-for-editions': 'Rendering',
    '--redact-comments':     'Rendering',
    '--redact-orphans':      'Rendering',
    '--go-root':             'Rendering',
    '--schema-db-out':       'Advanced',
    '--emit-scoring-yaml':   'Advanced',
    '--emit-extension-ranges': 'Advanced',
    '--scoring-html-out':    'Advanced',
    '--with-leaf-nodes':     'Advanced',
    '--hide':                'Advanced',
    '--debug':               'Advanced',
    '--phase2-plugin':       'Advanced',
    '--keep-duplicates':     'Advanced',
    '--detailed-warnings':   'Diagnostics',
    '--quiet':               'Diagnostics',
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

    def format_epilog(self, ctx: click.Context, formatter: click.HelpFormatter) -> None:
        """Write the epilog verbatim (no wrapping) so multi-line examples look correct."""
        if self.epilog:
            formatter.write_paragraph()
            formatter.write(self.epilog + '\n')


@click.command(
    cls=_SectionedCommand,
    help='Parse DESCRIPTOR_FILES and generate output based on the options given.',
    epilog=(
        'Examples:\n\n'
        '  # Decompile a descriptor to .proto source\n'
        '  reproto -O out/ schema.desc\n\n'
        '  # Build a scoring DB from a descriptor slice\n'
        '  reproto --schema-db-out slice.desc \\\n'
        '      --seed desc:.com.example.MyMessage schema.desc\n\n'
        '  # Read the descriptors embedded in a blob, writing the DB to\n'
        '  # out.desc and the .proto files to out/proto/\n'
        '  reproto --schema-db-out out.desc -I some/blob\n\n'
        '  # Decompile all types reachable from a seed, pruning a noisy package\n'
        '  reproto -O out/ \\\n'
        '      --seed desc:.com.example.Root \\\n'
        '      --prune file:google/protobuf/struct.proto \\\n'
        '      schema.desc\n\n'
        '  # Score and visualise the graph (advanced)\n'
        '  reproto --schema-db-out out.desc \\\n'
        '      --scoring-html-out out.html \\\n'
        '      schema.desc'
    ),
)
@click.version_option(
    version=_reproto_version,
    prog_name='reproto',
)
# --- Input ---
@click.argument(
    'pb_files',
    required=False,
    type=click.Path(path_type=Path),
    nargs=-1,
    metavar='DESCRIPTOR_FILES',
    shell_complete=complete_pb_files,
)
@click.option(
    '-I', '--desc-root',
    'pb_path',
    type=click.Path(file_okay=True, dir_okay=True, exists=True, path_type=Path),
    multiple=True,
    help=(
        'Root for resolving relative DESCRIPTOR_FILES paths and for '
        'locating imports (repeatable; like protoc -I). A directory is '
        'read as-is; a file is read as a blob, standing for the '
        'descriptors embedded in it, one member per FileDescriptorProto '
        'found (what protoscan would have extracted). '
        'Defaults to the current directory.'
    ),
)

@click.option(
    '--pb-path',
    'pb_path_deprecated',
    type=click.Path(file_okay=False, dir_okay=True, exists=True, path_type=Path),
    multiple=True,
    hidden=True,
    help='Deprecated alias for --desc-root.',
)

@click.option(
    '-O', '--proto-out',
    'proto_out',
    required=False,
    default=None,
    type=click.Path(file_okay=False, dir_okay=True, path_type=Path),
    shell_complete=complete_dir_path,
    help=(
        'Output directory for generated .proto files (created if absent). '
        'Not required when using --schema-db-out, --scoring-html-out, or --dry-run. '
        "Defaults to the --schema-db-out stub's 'proto' child when that "
        'option is given; --no-proto-out suppresses that default. '
        'When --schema-db-out is also given, DIR may not be inside its stub '
        "directory unless DIR is that stub's immediate 'proto' child."
    ),
)

@click.option(
    '--no-proto-out',
    'no_proto_out',
    is_flag=True,
    help=(
        'Do not write .proto files. Suppresses the -O directory that '
        '--schema-db-out otherwise implies; an error if -O is also given.'
    ),
)

@click.option(
    '--output-root',
    'output_root_deprecated',
    required=False,
    default=None,
    hidden=True,
    type=click.Path(file_okay=False, dir_okay=True, path_type=Path),
    help='Deprecated alias for --proto-out.',
)

@click.option(
    '-b', '--emit-binary',
    is_flag=True,
    help='Also write binary descriptor files (.pb) alongside .proto output',
)

@click.option(
    '--source-info/--no-source-info',
    'source_info',
    default=True,
    help='Embed a SourceCodeInfo synthesized from the reconstructed .proto '
         'text in binary descriptor output (-b, --schema-db-out). '
         'On by default.',
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
    '--no-emit-descriptor',
    'emit_descriptor',
    is_flag=True,
    flag_value=False,
    default=True,
    help='Suppress writing descriptor.proto to the output directory',
)

@click.option(
    '-s',
    '--seed', 'seeds',
    type=str,
    multiple=True,
    help=(
        'FQDN or path pattern to treat as an output root. '
        'FQDN (needs a prefix): desc:my.pkg.MyMsg, file:foo.proto '
        '(the FDP named foo.proto). Path (bare, or path: prefix): '
        'foo.pb (every FDP in that physical file), matched against '
        'the -I-relative path. Glob (either form): one *.proto '
        '(one level), borg/** (recursive).'
    ),
)

@click.option(
    '-p',
    '--prune', 'stumps',
    type=str,
    multiple=True,
    help=(
        'FQDN or path pattern to exclude from output. '
        'FQDN (needs a prefix): desc:my.pkg.MyMsg, file:foo.proto '
        '(the FDP named foo.proto). Path (bare, or path: prefix): '
        'foo.pb (that physical file, never loaded), matched against '
        'the -I-relative path. Glob (either form): one *.proto '
        '(one level), borg/** (recursive).'
    ),
)

@click.option(
    '-C',
    '--filter-config',
    type=click.Path(exists=True, dir_okay=False, path_type=Path),
    default=None,
    help=(
        'Path to a YAML file with "seed:"/"prune:" lists of FQDNs or '
        'path patterns (same syntax as -s/--seed and -p/--prune). '
        'Merged with any -s/-p flags given on the command line.'
    ),
)

@click.option(
    '--force-proto2-output',
    is_flag=True,
    default=False,
    help='Force all output to proto2 syntax, regardless of the input syntax. '
         'Without this flag, output syntax matches the input (polyglot mode).',
)

# Note: --schema-db-out forces --force-proto2-for-editions unconditionally
# because prost-reflect does not yet support editions syntax in compiled graphs.
# See upstream issue #1347.
@click.option(
    '--force-proto2-for-editions',
    is_flag=True,
    default=False,
    help=(
        'Translate editions-syntax files to proto2 in output and binary '
        'descriptors. Symmetric with --force-proto2-output but limited to '
        'editions files only. --schema-db-out forces this unconditionally.'
    ),
)
@click.option(
    '--prost-workaround',
    is_flag=True,
    default=False,
    hidden=True,
    help='Deprecated alias for --force-proto2-for-editions.',
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
    '--schema-db-out',
    'build_schema_db',
    required=False,
    default=None,
    type=click.Path(dir_okay=False, writable=True, path_type=Path),
    help=(
        'Write the schema DB to FILE (must end in .desc). '
        'Writes FILE (FileDescriptorSet of all loaded FDPs), '
        'FILE-stem/hopcroft.rkyv (compiled scoring graph), and '
        'FILE-stem/index.rkyv (lazy-loading FDS index). '
        "If -O is also given, it may not be inside FILE-stem unless it "
        "is exactly FILE-stem/proto."
    ),
)

@click.option(
    '--build-schema-db',
    'build_schema_db_deprecated',
    required=False,
    default=None,
    hidden=True,
    type=click.Path(dir_okay=False, writable=True, path_type=Path),
    help='Deprecated alias for --schema-db-out.',
)

@click.option(
    '--emit-scoring-yaml',
    is_flag=True,
    help='Write per-file scoring-graph YAML files alongside .proto output under --proto-out',
)

@click.option(
    '--emit-extension-ranges',
    is_flag=True,
    help='Record each message\'s extension ranges in the scoring graph, so that '
         'an unknown field number inside a declared range is not counted against '
         'the type',
)

@click.option(
    '--emit-scoring-graphs',
    is_flag=True,
    hidden=True,
    help='Deprecated alias for --emit-scoring-yaml.',
)

@click.option(
    '--scoring-html-out',
    'emit_scoring_html',
    required=False,
    type=click.Path(file_okay=True, dir_okay=False, writable=True, path_type=Path),
    help='Write scoring-graph HTML visualisations to FILE; requires --schema-db-out',
)

@click.option(
    '--emit-scoring-html',
    'emit_scoring_html_deprecated',
    required=False,
    hidden=True,
    type=click.Path(file_okay=True, dir_okay=False, writable=True, path_type=Path),
    help='Deprecated alias for --scoring-html-out.',
)

@click.option(
    '--emit-pyvis',
    required=False,
    hidden=True,
    type=click.Path(file_okay=True, dir_okay=False, writable=True, path_type=Path),
    help='Deprecated alias for --scoring-html-out.',
)

@click.option(
    '--with-leaf-nodes',
    is_flag=True,
    help='Include leaf (wire-type sink) nodes in --scoring-html-out graphs (hidden by default)',
)

@click.option(
    '--hide', 'pyvis_hide',
    type=str,
    multiple=True,
    help=(
        'FQDN or glob pattern to hide from --scoring-html-out graphs (same syntax as --prune). '
        'Matched nodes and their incident edges are dropped from the HTML; '
        'no effect on the schema DB.'
    ),
)

@click.option(
    '--debug',
    is_flag=True,
    help='Print detailed debug information (development only)',
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
    '--quiet', '-q',
    is_flag=True,
    help='Suppress progress messages and warnings.  Errors are always printed.',
)

@click.option(
    '--dump-resolved-features',
    default='',
    hidden=True,
    help='(diagnostic) Dump resolved FeatureSet YAML for the named proto file and exit',
)

@click.option(
    '--debug-fqdn',
    is_flag=True,
    help='Print detailed information about FQDNs (development only)',
)

def main(
        pb_files: list[Path],
        pb_path: tuple[Path, ...],
        pb_path_deprecated: tuple[Path, ...],
        force_proto2_output: bool,
        force_proto2_for_editions: bool,
        prost_workaround: bool,
        proto_out: Path | None,
        no_proto_out: bool,
        output_root_deprecated: Path | None,
        emit_binary: bool,
        source_info: bool,
        dry_run: bool,
        build_schema_db: Path | None,
        build_schema_db_deprecated: Path | None,
        emit_scoring_yaml: bool,
        emit_extension_ranges: bool,
        emit_scoring_graphs: bool,
        proto_variant: Path | None,
        use_variant: tuple[str, ...],
        keep_descriptor_path: bool,
        emit_descriptor: bool,
        seeds: list[str],
        stumps: list[str],
        filter_config: Path | None,
        redact_comments: bool,
        redact_orphans: bool,
        go_root: str,
        keep_duplicates: bool,
        detailed_warnings: bool,
        quiet: bool,
        emit_scoring_html: Path | None,
        emit_scoring_html_deprecated: Path | None,
        emit_pyvis: Path | None,
        with_leaf_nodes: bool,
        pyvis_hide: tuple[str, ...],
        phase2_plugin: str | None,
        dump_resolved_features: str,
        debug: bool,
        debug_fqdn: bool,
):
    '''
    Parse DESCRIPTOR_FILES and generate output based on the options given.
    '''
    global _quiet
    _quiet = quiet

    import sys as _sys

    # Merge deprecated alias values into canonical params.
    pb_path = pb_path + pb_path_deprecated

    # Handle deprecated option aliases (emit warnings, merge into canonical params).
    if output_root_deprecated is not None:
        if not quiet:
            _sys.stderr.write('warning: --output-root is deprecated; use --proto-out\n')
        if proto_out is None:
            proto_out = output_root_deprecated
    if build_schema_db_deprecated is not None:
        if not quiet:
            _sys.stderr.write('warning: --build-schema-db is deprecated; use --schema-db-out\n')
        if build_schema_db is None:
            build_schema_db = build_schema_db_deprecated
    if emit_scoring_html_deprecated is not None:
        if not quiet:
            _sys.stderr.write('warning: --emit-scoring-html is deprecated; use --scoring-html-out\n')
        if emit_scoring_html is None:
            emit_scoring_html = emit_scoring_html_deprecated

    from reproto import Fqdn
    from .reproto import DescriptorProtoMissingError, DescriptorProtoUnresolvedError, DescriptorProtoHasTargetsError, Options, reproto
    from . import variant as variant_mod
    from .lib.warnings import configure_collector
    configure_collector(detailed=detailed_warnings)

    if emit_scoring_graphs:
        _sys.stderr.write('warning: --emit-scoring-graphs is deprecated; use --emit-scoring-yaml\n')
        emit_scoring_yaml = True
    if emit_pyvis is not None:
        _sys.stderr.write('warning: --emit-pyvis is deprecated; use --scoring-html-out\n')
        if emit_scoring_html is None:
            emit_scoring_html = emit_pyvis

    # No DESCRIPTOR_FILES means "everything under the -I roots" — which is
    # what every invocation that uses -I spells out anyway (spec 0243 S13).
    if not pb_files:
        pb_files = [Path('.')]

    # Validated here rather than beside the other --schema-db-out checks
    # below: the S1 default derives a directory from this path, and must
    # not create one from a path that is about to be rejected.
    if build_schema_db is not None and not str(build_schema_db).endswith('.desc'):
        raise click.UsageError('--schema-db-out PATH must end in .desc')

    # --schema-db-out names the .proto directory (spec 0243 S1): the stub
    # directory's 'proto' child is the only place spec 0155 G1 lets -O
    # point inside the stub, so there is nothing else it could mean.
    if no_proto_out and proto_out is not None:
        raise click.UsageError(
            "--no-proto-out contradicts '-O' / '--proto-out'; drop one."
        )
    if proto_out is None and not no_proto_out and build_schema_db is not None:
        proto_out = build_schema_db.with_suffix('') / 'proto'

    # --proto-out is required when .proto file output will be produced.
    # It is legitimately optional for modes that produce no .proto output:
    #   --no-proto-out        → asks for no .proto output
    #   --dry-run             → writes nothing at all
    #   --scoring-html-out    → writes HTML visualisations
    #   --dump-resolved-features → returns early before phase 7
    output_only_mode = bool(
        no_proto_out or dry_run or emit_scoring_html is not None
        or dump_resolved_features
    )
    if proto_out is None and not output_only_mode:
        raise click.UsageError(
            "Missing option '-O' / '--proto-out'.\n"
            "Required when generating .proto output.  Alternatives:\n"
            "  - Add -O DIR                   to write .proto files to DIR\n"
            "  - Add --schema-db-out FILE     to write a schema DB, and .proto\n"
            "                                 files under FILE's 'proto' child\n"
            "  - Add --no-proto-out           to write no .proto files\n"
            "  - Add --scoring-html-out FILE  to write HTML graphs only\n"
            "  - Add --dry-run                to run without writing any files"
        )
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

    # spec 0155 G1: -O may not land inside --schema-db-out's stub
    # directory (reserved for hopcroft.rkyv/index.rkyv) except as that
    # stub's immediate 'proto' child.
    if build_schema_db is not None and proto_out is not None:
        stub_dir = build_schema_db.with_suffix('').resolve()
        resolved_out = proto_out.resolve()
        try:
            rel = resolved_out.relative_to(stub_dir)
        except ValueError:
            rel = None  # proto_out is not inside stub_dir at all — fine
        if rel is not None and rel != Path('proto'):
            raise click.UsageError(
                f"-O/--proto-out ({proto_out}) is inside the --schema-db-out "
                f"stub directory ({stub_dir}); only an immediate 'proto' "
                "child directory is allowed there — it otherwise holds "
                "hopcroft.rkyv/index.rkyv."
            )

    options = Options(
        binary=emit_binary,
        source_info=source_info,
        build_schema_db=build_schema_db,
        emit_scoring_yaml=emit_scoring_yaml,
        emit_extension_ranges=emit_extension_ranges,
        force_proto2_output=force_proto2_output,
        force_proto2_for_editions=force_proto2_for_editions or prost_workaround,
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
        emit_scoring_html=emit_scoring_html,
        with_leaf_nodes=with_leaf_nodes,
        pyvis_hide=pyvis_hide,
        keep_duplicates=keep_duplicates,
        detailed_warnings=detailed_warnings,
        quiet=quiet,
        redact_comments=redact_comments,
        redact_orphans=redact_orphans,
        phase2_plugin=phase2_plugin_function,
    )
    _FQDN_PREFIXES = ('file', 'desc', 'enum', 'serv', 'meth', 'fdsc')
    _VALID_PREFIXES = _FQDN_PREFIXES + ('path',)

    def _normalise_fqdn(s: str) -> Fqdn:
        """Normalise an FQDN pattern for matching.

        - Validates that the pattern has a known FQDN prefix.
        - For file: patterns: name part is left as-is (already uses /).
        - For all other prefixes: replace . with / in the name part and strip
          any leading / so PurePosixPath.match() gives proper segment-level
          semantics.
        """
        if ':' not in s:
            raise click.UsageError(
                f'Invalid FQDN {s!r}: must start with a prefix '
                f'({", ".join(_FQDN_PREFIXES)}).'
            )
        prefix, rest = s.split(':', 1)
        if prefix not in _FQDN_PREFIXES:
            raise click.UsageError(
                f'Invalid FQDN prefix {prefix!r} in {s!r}: '
                f'must be one of {", ".join(_FQDN_PREFIXES)}.'
            )
        if prefix == 'file':
            return Fqdn(s)
        # Non-file: replace . with / and strip leading /
        normalised = rest.replace('.', '/').lstrip('/')
        return Fqdn(f'{prefix}:{normalised}')

    def _split_seed_or_prune(value: str) -> tuple[str, str]:
        """Classify a raw -s/-p/filter-config value as ('fqdn', v) or ('path', v).

        A value with a recognized FQDN prefix (file:, desc:, enum:, serv:,
        meth:, fdsc:) is an FQDN pattern, unchanged from spec 0074. A value
        with the path: prefix, or a bare value with no recognized prefix at
        all, is a filesystem path pattern, matched root-independently
        against each candidate file's -I-root-relative path before it is
        loaded (spec 0149). path: and bare are fully equivalent; path:
        exists for readability and to allow a literal ':' inside a path.
        """
        if ':' in value:
            prefix, _, rest = value.partition(':')
            if prefix == 'path':
                return ('path', rest)
            if prefix in _FQDN_PREFIXES:
                return ('fqdn', value)
            raise click.UsageError(
                f"'{value}': '{prefix}:' is not a recognized prefix "
                f"({', '.join(_VALID_PREFIXES)}); if this is meant to be a "
                f"filesystem path, use the 'path:' prefix"
            )
        return ('path', value)

    def _partition(values: list[str]) -> tuple[list[Fqdn], list[str]]:
        """Split a merged CLI+filter-config list into (FQDNs, path patterns)."""
        fqdns: list[Fqdn] = []
        paths: list[str] = []
        for v in values:
            kind, resolved = _split_seed_or_prune(v)
            if kind == 'fqdn':
                fqdns.append(_normalise_fqdn(resolved))
            else:
                paths.append(resolved)
        return fqdns, paths

    try:
        for h in pyvis_hide:
            prefix = h.split(':', 1)[0] if ':' in h else ''
            if prefix != 'desc':
                raise click.UsageError(
                    f'--hide only accepts desc: patterns (got {h!r})'
                )
        options.pyvis_hide = tuple(_normalise_fqdn(h) for h in pyvis_hide)

        config_seeds: list[str] = []
        config_prunes: list[str] = []
        if filter_config is not None:
            from .filter_config import load as load_filter_config
            try:
                config_seeds, config_prunes = load_filter_config(filter_config)
            except ValueError as e:
                raise click.UsageError(str(e))

        all_seeds = list(seeds) + config_seeds
        all_stumps = list(stumps) + config_prunes

        seed_fqdns, seed_paths = _partition(all_seeds)
        prune_fqdns, prune_paths = _partition(all_stumps)

        roots = list(pb_path) if pb_path else [Path('.')]

        # A file-shaped -I root is an explicit claim that the file holds
        # descriptors, so an empty scan is an error rather than one more
        # W1 among many (spec 0243 S9). Checked up front, and it warms
        # the scan memo the load phase then reuses.
        from .load import blob_members
        for root in roots:
            if root.is_file() and not blob_members(root):
                raise click.ClickException(
                    f'-I {root} holds no FileDescriptorProto; '
                    'a file given to -I is read as a blob of descriptors.'
                )

        reproto(
            roots,
            pb_files,
            seed_fqdns,
            prune_fqdns,
            proto_out,
            options,
            path_seeds=seed_paths,
            path_prunings=prune_paths,
        )
    except DescriptorProtoMissingError:
        raise click.ClickException(
            'descriptor.proto not found in the input set; '
            'add it to your -I tree or use --use-variant descriptor'
        )
    except DescriptorProtoUnresolvedError:
        raise click.ClickException(
            'descriptor.proto is referenced but could not be loaded; '
            'add it to your -I tree or use --use-variant descriptor'
        )
    except DescriptorProtoHasTargetsError:
        raise click.ClickException(
            'descriptor.proto appears corrupted: it has dependencies of its own'
        )

if __name__ == '__main__':
    main()
