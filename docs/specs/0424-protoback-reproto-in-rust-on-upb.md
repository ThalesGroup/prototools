<!--
SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)

SPDX-License-Identifier: MIT
-->

# 0424 — protoback: reproto in Rust, on upb

Status: draft
App: protoback, reproto, prototext-upb
Refs: docs/specs/0415-prototext-on-upb.md (the upb bundle, its
      hand-written FFI and the gate in front of it: what this extends);
      docs/specs/0416-prototext-on-upb-performance.md (upb's load time
      on googleapis, and the benchmark procedure S21 follows);
      docs/specs/0425-hopcroft-sparse-worklist.md (step 0: the
      minimization that takes most of the schema database's time);
      docs/specs/0417-protolens-on-upb.md (the same move for protolens);
      docs/specs/0419-fuzzing-upb-with-clusterfuzzlite.md (the fuzzing
      the new surface joins);
      docs/specs/0266-a-payload-with-any-flaw-in-it-is-not-a-message.md
      (the probe every descriptor passes);
      docs/specs/0426-protocs-text-of-unknown-fields-read-back.md (the
      rules S5 reads protoc's text of unknown fields back by);
      docs/specs/0369-fdp-plugin-applied-at-every-parse.md (the plugin
      contract S9 carries over);
      docs/specs/0063-reproto-instantiate-schema.md (the instance
      generator S10 reproduces);
      docs/specs/0422-reproto-completes-in-bash-zsh-and-fish.md and
      docs/specs/0423-nothing-uploaded-before-nixpkgs-builds.md (Click's
      completion protocol, which S14 replaces)

## Background

reproto is the last Python tool of the repository: 12.1k lines in
`reproto/src/reproto`, 8.5k lines of tests (293 cases), and 47 specs of
recorded behavior. Every other tool is Rust, on upb since specs
0415–0418.

What keeping it in Python costs:

- **Two upb builds resolve the same rules.** reproto reads descriptors
  through Python protobuf's upb backend (7.34.1 in our pin's
  environment, `api_implementation.Type()` = `upb`); prototext and
  protolens through the vendored upb of protobuf v36.2. Packing,
  delimited encoding and enum closedness, which reproto writes into the
  scoring automaton and prototext reads back, are decided by two
  versions of the same code.
- **Packaging that exists only for reproto.** Three PyO3 crates
  (`fdp-scan-pyo3`, `prototext-pyo3`, `prototext-graph-pyo3`), their
  wheels, the `tree-sitter-textproto` wheel, and four nixpkgs
  python-modules wrap Rust crates so that reproto can call them.
- **Click's completion protocol**, which needed spec 0422's classes and
  then spec 0423's fix when Click 8.4 changed what it prints.
- **Time.** On googleapis (the 21.3 MB set `googleapisPbs` builds, 7,771
  files), with `googleapisDb`'s flags, schema database included, reproto
  takes **167.2 s ± 0.6 s** wall clock, on one core, and 1.29 GB of
  memory (`scripts/bench-protoback`, three runs, 2026-11-14, the
  development VM pinned to its E-cores, vCPUs 0–7, on mains power, host
  idle; reproto at 18dce2f, spec 0425 included). Timed per phase, without
  a profiler, one run under the same conditions:

  | Where | Time | Runs in |
  |---|---|---|
  | rendering the `.proto` files (phase 7) | 108.7 s | Python |
  | `yaml.dump`: the scoring graphs, written for `--emit-scoring-yaml` and serialized for `build_graph` | 40.8 s | Python (PyYAML) |
  | phase 3, the graph of nodes | 4.8 s | Python |
  | `build_graph`, in the schema database phase | 1.2 s | Rust (`prototext-graph`) |
  | the rest: loading, pool, phases 4–6, writing | about 9 s | Python |

  `build_graph` took 55 s before spec 0425: 95% of it was
  `hopcroft::minimize` (perf, on the 7,771 scoring graphs), whose
  worklist was seeded with every (block, symbol) pair, 2,903 symbols for
  164,642 edges. That was an algorithm, not a language. The YAML exists
  only to cross from Python to Rust. The rest is Python's speed, on one
  core.

The Python implementation relies on behavior a port must reproduce:

- a lazy, tolerant descriptor pool (`DescriptorPool(DescriptorDatabase)`)
  whose build errors surface at render time and are sorted into warnings
  W4/W5/W6 by their text (`anomalies.py:179-204`); the texts are upb's
  own (`upb.c`: `couldn't resolve name '…'`, `Depends on file '…'`),
  under a prefix of Python's wrapper;
- options re-parsed into dynamic classes from the pool, so that
  `ListFields()`, `FindAllExtensions` and `Extensions[…]` see custom
  options (`option_messages.py`, `option_rendering.py`);
- `descriptor_pb2` messages mutated in place (`CopyFrom`, `ClearField`)
  to rebuild the descriptors written by `--emit-binary` and the schema
  database, custom options kept as unknown bytes;
- text-format input (`.textpb`, `.pbtxt`, `.prototxt`, `.ascii_proto`)
  read with `text_format.Parse`, pure Python;
- Python plugins, `exec()`'d in the process (spec 0369);
- Python's value formatting in the rendered `.proto` (`str` and `repr`
  of floats, `json.dumps` escaping, two bytes escapers), PyYAML's output
  format for `--emit-scoring-yaml`, `PurePosixPath.full_match` globs,
  `rapidfuzz`'s `fuzz.ratio`;
- CPython's `random.Random` in `reproto-instantiate-schema`;
- `set` iteration order, which reaches the order of some warnings and
  outputs.

## Why upb

The port stays on upb, the runtime prototext and protolens use, for the
reasons spec 0415 chose it, and because upb already provides what
reproto needs beyond them:

- **Editions**, resolved by Google's own code
  (`upb_FieldDef_ResolvedFeatures`), where prost-reflect fails and
  rust-protobuf reads editions files as proto2 without an error.
- **One engine for the whole workspace.** protoback writes the scoring
  automaton from upb's resolution of packing, delimited encoding and
  enum closedness, and prototext reads messages with the same build of
  the same code.
- **The same mechanism as reproto.** Python protobuf runs on upb: the
  reflection reproto uses today is upb's, called through Python. The
  bundle's public API (`UPB_API`) has every piece:
  - messages that keep unknown fields and extensions, decoded and
    encoded with `upb_Decode` / `upb_Encode`, cloned with
    `upb_Message_DeepClone`;
  - reflection: `upb_Message_GetFieldByDef`, `_SetFieldByDef`,
    `_ClearFieldByDef`, and `upb_Message_Next`, which lists set fields
    and extensions as `ListFields()` does;
  - extensions: `upb_DefPool_ExtensionRegistry`, so that decoded
    options resolve custom options, and `upb_DefPool_GetAllExtensions`
    for `FindAllExtensions`;
  - service and method definitions, and the options of every
    definition;
  - the build error texts reproto already classifies.
- **Shipping.** Two vendored files compiled by the `cc` crate; no cmake,
  nothing installed beforehand for `cargo install`; a surface small
  enough to write and review by hand (spec 0415 S2).

What upb lacks is a text-format parser: upb has never had one, and the
single-file build has only a debug printer that prints field numbers.
S5 writes one, to Google's published grammar.

## Goals

- **G1.** `protoback`, a Rust program of the workspace, on
  `prototext-upb`, with reproto's command line, and
  `protoback instantiate` with `reproto-instantiate-schema`'s.
- **G2.** On the corpora of S11, protoback produces reproto's outputs
  byte for byte, the instances of `instantiate`, stdout, stderr and the
  exit status included, with only the normalizations of S12.
- **G3.** Text-format input is kept.
- **G4.** Plugins written for reproto work with protoback unchanged.
- **G5.** protoback needs no Python to build or run; only a plugin
  written in Python needs it, to run that plugin.
- **G6.** What protoback needs from upb lives in `prototext-upb`, as a
  safe API every tool can use: reflection, extensions, services and
  methods, and the text-format parser.
- **G7.** Seconds, not minutes: googleapis with `googleapisDb`'s flags
  in under 10 s wall clock under the Background measurement's conditions,
  using its eight E-cores, with less memory than reproto's 1.29 GB (S16).
- **G8.** The port proceeds in the steps of the work plan, each ending
  with its tests green and its timings recorded.

## Non-goals

- **N1.** libprotobuf, the C++ runtime. It has a text-format parser, a
  lazy pool over a database and full reflection, but it brings a C++ and
  abseil build to every `cargo install`, CI run and nixpkgs build, a
  binding through `cxx` or a C wrapper over its classes, an ABI tied to
  exact protobuf and abseil versions, and a second runtime beside the
  upb of prototext and protolens. The one piece upb lacks, text format,
  costs about a thousand lines of Rust (S5).
- **N2.** prost, prost-reflect, rust-protobuf, and Google's `protobuf`
  crate 4.x: ruled out by spec 0415, for the reasons in its
  alternatives. prost also drops unknown fields, which carry custom
  options.
- **N3.** Rendering with protoc's printer (`FileDescriptor::DebugString`
  in C++). reproto's output (variants, orphans, anomalies, the
  `--force-proto2-*` conversions, pruning) is its own; changing it is a
  rewrite, not a port.
- **N4.** Releasing protoback. It is not published to crates.io, PyPI
  or nixpkgs, and reproto stays the released tool. Releasing protoback,
  retiring reproto, and the fate of the PyPI packages and nixpkgs
  python-modules that exist for reproto are a later spec's.
- **N5.** New features or output changes during the port. Every bug
  found is fixed in reproto first (S1), so that the reference and the
  port agree.
- **N6.** A Rust library API. protoback is a program.

## Specification

### The reference (S1–S2)

- **S1.** Before any Rust, reproto is fixed, so that it can serve as the
  reference without exceptions. Each fix has a test that fails without
  it, and is its own commit:
  - **stripped imports:** an import left out before the pool (missing or
    pruned file) shifted the `public` and `weak` imports after it onto
    other files, and upb then refused the file's options
    (`public_dep 2 is out of range`); the indices are renumbered;
  - **message-valued options:** a map field crashed reproto (an
    `AssertionError`), an extension printed without brackets, a group
    without braces. Map entries print in wire order, as protoc keeps the
    order of the source: option values are parsed with classes from a
    second pool over the same files, `map_entry` cleared, where a map is
    the repeated message it is on the wire (`option_messages.py`);
    Python's maps iterate in hash order;
  - **custom options on oneofs and extension ranges** were left out;
  - **feature inheritance:** a nested message, its fields and its enums
    ignored the features of the messages around them; they resolve
    through every enclosing message, as upb does. Only `json_format` can
    be set on a message (protoc refuses the five others there), and no
    rendering decision reads it: the one visible effect was
    `--dump-resolved-features`.

  Suspected and not reproduced: an output depending on the iteration
  order of a `set` of nodes or files. Running reproto with their hashes
  salted, every set in another order, changes no output and no stderr on
  any fixture (three salts) or on googleapis.
- **S2.** The test suite starts the tool through one launcher, set by a
  `REPROTO` environment variable (reproto by default), instead of
  `sys.executable -m reproto.cli` in each file. The CLI tests then run
  unchanged against reproto or protoback.

### prototext-upb (S3–S6)

- **S3. Reflection.** The hand-written FFI of spec 0415 S2 grows to
  messages: `upb_Message_New`, `upb_Decode` and `upb_Encode` (with the
  message's MiniTable and the pool's extension registry),
  `upb_Message_GetFieldByDef`, `_SetFieldByDef`, `_ClearFieldByDef`,
  `_HasFieldByDef`, `upb_Message_Next`, `upb_Message_DeepClone`, arrays
  and maps. Only public symbols, as before; what upb declares
  `static inline` goes through the shim. A message owns its arena on
  the Rust side; the safe API exposes no raw pointer.
- **S4. Definitions.** Service and method definitions; extension
  ranges; `upb_DefPool_GetAllExtensions`, every extension of a message.
  Options are not read through `upb_XxxDef_Options`: those return them in
  upb's built-in `descriptor.proto` (v36), where custom options are
  unknown bytes, and whose version is not the input's or the variant's.
  They come from the descriptors read as messages (S6): decoded with the
  pool's own types and its extension registry, a `FileDescriptorProto`'s
  `options` fields carry the custom options as extensions, in the input's
  version.
- **S5. Text format.** A parser of the text format, to protobuf.dev's
  Text Format Language Specification, over S3's reflection: any message
  type of a pool. It covers `#` comments, `{}` and `<>`, the optional
  `:` before a message, `[a, b]` lists, adjacent string literals, C
  escapes, float forms (`1f`, `inf`, `-nan`), enums by name or number,
  `[pkg.ext]` extensions resolved in the pool, expanded `Any`
  (`[type.googleapis.com/…]`). Errors carry the line and column.
  - **Fields by number are kept.** `protoc --decode` prints a field it
    does not know, custom options among them, by number. Python's
    `text_format`, which reproto uses, cannot keep a field it does not
    know, and drops it: on the options fixture of `prototext-upb`,
    reproto renders 11 custom options from the binary set and none from
    its text form. The parser keeps such a field as unknown bytes, as
    protoc printed it: a decimal number is a varint, `0x` and 8 or 16
    hex digits a fixed32 or a fixed64, a string length-delimited.
  - **In import order, with the input's schema.** protoc, without the
    options' schema, prints a string that happens to read as a message
    as a message, and a group as it prints a length-delimited message,
    both `N { … }`; with it, it prints an option by name, `[pkg.ext]`,
    which only the declaration can parse. Once every file of the input
    is loaded, binary and text (a text file only scanned for its
    imports), all go into one pool, imports first, and each text file is
    parsed when its turn comes, with the files it imports: names
    resolve, values by number are framed as declared, and where nothing
    is, the likely framing is taken, with a warning. Spec 0427 has the
    reading, spec 0426 the framing rules, their rationale and their
    limits.
  - **One implementation.** reproto calls the parser and the input's
    pool through `prototext_codec_lib`, in place of Python's
    `text_format`, so both tools read text identically; the variants'
    import section, which loaded `_pb2` modules so that `text_format`
    would find their extensions, goes: the declarations come from the
    input (spec 0427 G3).
- **S6. Descriptors as messages.** protoback reads and writes
  descriptors as upb messages of `descriptor.proto`'s types, through
  S3, with typed accessors over the reflection; not through upb's
  generated inline accessors (1,312 inline functions, each needing a
  shim). The accessors are generated, one per field of every message of
  the vendored upb's `descriptor.proto`, and committed; a check in `ci`
  fails when they are out of date, and `scripts/update-upb` regenerates
  them. Each looks its field up by number in the pool's own
  `descriptor.proto` and answers "absent" when that version lacks it, so
  an input built with an older or a variant `descriptor.proto` works. A
  field the pool's `descriptor.proto` does not know is kept as unknown
  bytes, and lost by no read or write; upb re-encodes unknown fields after
  a message's known ones, so where it lacks a field the input has, the
  bytes come back complete but in another order (Python's upb backend
  does the same). The plain structs of spec 0415 S5 stay for the code that uses
  them.

Every descriptor protoback reads passes spec 0415 S4's gate before upb
parses it. S3–S5 join the fuzzing of spec 0419.

### The port (S7–S10)

- **S7.** A crate, `protoback/`, in the workspace, with one binary,
  `protoback`; `instantiate` is its subcommand. Its structure follows
  reproto's: the loader, the nine phases, the renderers. In detail:
  - **The pool.** Files are kept by name, and added to a upb pool on
    demand, dependencies first, as Python's pool does from its
    database. A failed build keeps upb's status text, and the warning
    classification keeps its rules, with Python's prefix reproduced.
  - **Nodes.** One arena of nodes, addressed by index; the `parent`,
    `contains`, `targets` and `seeder` links are indices.
  - **Text input.** S5 parses each `file { … }` entry of a set on its
    own, so that one bad file fails alone, as the tree-sitter splitter
    makes it today; tree-sitter is not used at run time.
  - **The Rust crates**, `fdp-scan`, `prototext-core` and
    `prototext-graph`, are called directly. The scoring graphs go to
    `prototext-graph` as structures, without YAML.
- **S8. Python's formatting, reproduced.** One module formats values as
  Python does: `str` and `repr` of floats, a float32 widened to double,
  `json.dumps` with `ensure_ascii`, and the two bytes escapers.
  `--emit-scoring-yaml` writes PyYAML's format (block style, key order,
  quoting). Globs follow `PurePosixPath.full_match`; the fuzzy
  suggestion is the normalized Indel similarity, above 85, the first
  maximum in node order. The HTML of `--scoring-html-out` is written
  with vis.js inline, as pyvis writes it.
- **S9. Plugins, out of process.** The contract of spec 0369 is kept: a
  plugin receives each descriptor freshly parsed, before anything reads
  it, and returns it changed; a failure is a `PluginError`. What changes
  is where it runs.
  - **The protocol.** `--fdp-plugin-cmd CMD` starts `CMD` once per run.
    protoback writes each `FileDescriptorProto` to its stdin as a
    length-prefixed frame (a varint length, then the serialized
    message), and reads one frame back from its stdout: the changed
    descriptor. A plugin that exits, writes a malformed frame, or a frame
    the gate rejects is a `PluginError` naming the file. Its stderr goes
    to protoback's. Any language can implement it.
  - **Python plugins, unchanged.** `--fdp-plugin FILE` is kept.
    protoback runs `python3` (or `$PROTOBACK_PYTHON`) on an adapter it
    embeds, which `exec()`s `FILE`, checks for `fdp_plugin(fdp)` as
    reproto does at startup, and serves the protocol through the
    installed Python `protobuf`. Python is needed only then.
  - **The deprecated `--phase2-plugin`** and its `phase2_plugin(ctx,
    fdp)` work through the adapter, with `ctx` given as `None` and the
    deprecation warning extended to say so: reproto's `Context` does not
    exist outside reproto.
  - **One call per distinct descriptor.** A plugin's result depends only
    on the descriptor it receives (spec 0369 S1), and the same file is
    parsed several times. protoback calls the plugin once per distinct
    serialized input and reuses the result, so the process boundary
    costs one round trip per file.
- **S10. Instances, reproduced.** `protoback instantiate` keeps the
  per-type seed (SHA-256 of `"<seed>:<fqdn>"`) and draws from a
  reimplementation of CPython's `random.Random`: MT19937, seeded by
  `init_by_array` over the 32-bit words of the seed integer, with
  `getrandbits`, `_randbelow`'s rejection sampling, `random()` from two
  draws (53 bits), `uniform`, `randint` and `choice` as CPython defines
  them. The traversal draws in reproto's order; serialization is upb's
  on both sides.

### Equivalence (S11–S12)

- **S11. A differential harness**, a Nix derivation in `ci`, runs
  reproto and protoback on each input and flag set, each in an empty
  directory, and compares byte for byte: the `--proto-out` tree, the
  `.pb` files, `buf.yaml`, the scoring YAML, the schema database
  (`.desc`, `hopcroft.rkyv`, `index.rkyv`), the instances, stdout, stderr
  and the exit status. The corpora:
  1. each fixture of `reproto/src/reproto/tests/fixtures`, alone and
     with its imports;
  2. the WKT set, whose outputs must also reproduce
     `prototext/wkt/prebuilt`;
  3. opentelemetry-proto and the custom fixtures, with `customDb`'s
     flags;
  4. googleapis at its pin, with `googleapisDb`'s flags, including its
     400 instances (in `full-tests`);
  5. the demos' inputs: the life-client blob, grpconf2026's `app.desc`,
     bobapp's descriptors, `anomalies.pb`;
  6. text-format versions of 1 and 2;
  7. a plugin fixture, run as `--fdp-plugin` by both, and as
     `--fdp-plugin-cmd` by protoback.

  Each corpus also runs with the flags that change the output:
  `--use-variant` values, `--force-proto2-output`,
  `--force-proto2-for-editions`, `--emit-extension-ranges`, seeds and
  prunes, `-I` directories and blobs.
- **S12. Normalizations**, the only differences allowed:
  - the program name in messages (`reproto`, `reproto-instantiate-schema`
    against `protoback`, `protoback instantiate`);
  - the HTML of `--scoring-html-out`, compared by its nodes and edges,
    not its bytes;
  - `--help`, where clap's layout differs from Click's, compared by its
    options and their texts.

  A build error whose text differs between Python protobuf's upb and
  the bundle's is listed under "Measured outcome", with the version
  that changed it. Any other difference is a bug of the port, or a bug
  of the reference fixed by S1.

### The command line and the build (S13–S15)

- **S13. The name.** `protoback`: it gives you back the `.proto`
  sources, which compile to the same descriptor set. `reproto` on
  crates.io belongs to an unrelated project (github.com/reproto/reproto);
  `protoback` is free on crates.io, PyPI and nixpkgs (checked
  2026-11-14).
- **S14.** clap, with reproto's options, their deprecated spellings and
  their warnings, and the help in the same sections. Man pages come
  from `PROTOBACK_GEN_MAN=<dir> protoback`, as for the other tools,
  through `clap_mangen`. Completion through `prototools-complete`, as
  protoscan's: bash, zsh and fish, `-I`-relative completion and blob
  members as spec 0422 has them. Progress bars (indicatif) on a terminal
  only.
- **S15. Side by side.**
  - reproto stays as it is: released, packaged, the reference.
  - protoback has `publish = false`, is not in
    `scripts/publishable-crates`, the publish workflow or the staged
    nixpkgs recipes. `ci` builds and tests it, and runs the harness; the
    dev shell has both.
  - The CLI tests run against both (S2). Tests of reproto's Python
    internals stay with reproto; protoback has its own unit tests.

### Performance (S16–S21)

- **S16. The target.** googleapis, with `googleapisDb`'s flags, schema
  database included: under 10 s wall clock and under 1.29 GB peak memory,
  under the Background measurement's conditions (vCPUs 0–7). The estimate behind it,
  to be replaced by measurements: the pool about 0.7 s (spec 0416's
  eager load of googleapis), phases 1–6 under 1 s, rendering about 1 s
  across the cores, the scoring graphs and their YAML about 1 s, the
  minimization about 1 s once spec 0425 is in, writing the 134 MB of
  output under 1 s.
- **S17. Shared state, then per-file work.** reproto passes per-file
  state through the shared `Context` (`current_file`, `out_desc`,
  `out_sci`, `syntax`, set and restored around each `render()`).
  protoback separates them:
  - phases 1–6 run on one thread and produce a state that is never
    written again: the node arena and its flags, the upb pool, the
    variant, the options;
  - each file is then rendered, its scoring graph built, and its outputs
    written, with a render context of its own, on a thread pool
    (rayon), files in parallel;
  - text-format sets are parsed per `file { … }` entry in parallel
    (S7), before the pool is built.
- **S18. Byte-identical whatever the threads.** Each file's outputs and
  warnings are collected with it, then written and printed in reproto's
  file order; the warning summary is merged in that order. `--jobs N`
  sets the thread count (all cores by default); `--jobs 1` and any other
  count give the same bytes, and the harness checks both.
- **S19. upb across threads.** The pool is built before any parallel
  phase and only read afterwards; every message a thread creates lives
  in that thread's arena. A test reads one googleapis pool from every
  core at once, under ThreadSanitizer.
- **S20. No YAML between protoback and prototext-graph.**
  `prototext-graph` gains an entry point taking the scoring graphs as
  structures; its YAML loader becomes one producer of them.
  `--emit-scoring-yaml` writes the same structures in PyYAML's format
  (S8), per file, in parallel.
- **S21. Measured at every step.** `scripts/bench-protoback` times
  reproto and protoback on googleapis (and the WKT and custom corpora),
  as spec 0416 benchmarks: same machine, pinned cores, `hyperfine`,
  mean and spread, peak memory, each phase's time, `--jobs 1` and all
  cores. Each step of the work plan records its results under
  "Measured outcome".

## Work plan

Each step ends with its tests green and its timings recorded (S21); a
step starts only once the previous one is done. A difference the harness
finds is fixed in the step that introduced it.

| Step | Content | Done when |
|---|---|---|
| 0 | The sparse minimization (spec 0425), in `prototext-graph` | Spec 0425's test plan: the same partition and the same `.rkyv` bytes on every fixture and googleapis; `build_graph` timed before and after. |
| 1 | The reference: S1's fixes, S2's launcher, the harness (S11) with reproto on both sides, `scripts/bench-protoback` with reproto alone | reproto's suite green after each fix; the harness runs reproto against itself on every corpus with no difference; the baseline timings recorded. |
| 2 | `prototext-upb`: reflection and definitions (S3, S4, S6), threads (S19) | Test plan 2 and 4; the ThreadSanitizer test. |
| 3 | `prototext-upb`: the text-format parser and the reading in import order (S5, spec 0427); their PyO3 binding; reproto switched to them, the variants' import section removed | Test plan 3; reproto's 11 options from the text form of the options fixture, by number and by name; a group-typed option declared in its own file; warnings for options declared nowhere, by number and by name; reproto's suite and the harness. |
| 4 | protoback: loading, the pool, phases 1–6, the clap skeleton (S7) | A hidden `--dump-graph`, added to both, identical on every corpus: nodes, flags, seeds, summoned files, in order. These phases timed. |
| 5 | Rendering, in stages: structure; built-in options; custom and message-valued options; editions and `--force-proto2-*`; comments and SourceCodeInfo; `--emit-binary` (S8, S17, S18) | After each stage, the harness's `--proto-out` and `.pb` comparison passes on every corpus, with `--jobs 1` and all cores. Rendering timed. |
| 6 | Scoring graphs and the schema database (S20), `--emit-scoring-yaml`, the HTML | The schema database byte-identical, WKT's reproducing `prototext/wkt/prebuilt`; googleapis timed against S16. |
| 7 | Plugins (S9) | Test plan 5. |
| 8 | `instantiate` (S10) | Test plan 6. |
| 9 | The command line (S14): help, man pages, completion, warnings, progress | Test plan 7, 8 and 9: the harness clean on every corpus and flag set, reproto's CLI tests passing against protoback; the final timings under "Measured outcome". |

## Alternatives considered

### libprotobuf (C++)

N1. It would provide the text-format parser S5 writes, at the cost of a
C++ build and a second runtime.

### Our own descriptor model and options decoder

Plain Rust structs for all of `descriptor.proto` keeping unknown fields,
and a schema-driven decoder for options. It is what upb already does,
and reproto with it: S3 binds it instead of rewriting it.

### upb's generated accessors for `descriptor.proto`

Typed and fast, but `static inline`: each one needs a shim function, and
the shim would grow with every field protoback touches. Reflection (S6)
reaches every field through a few exported functions.

### Plugins: embed Python

PyO3 embedding runs plugins in the process, `ctx` included, but makes
Python a build and run-time dependency of protoback for everyone (G5),
and ties it to one Python version.

### Plugins: WebAssembly

Sandboxed and portable, but plugins would have to be rewritten and
compiled to WebAssembly (G4), and protoback would carry a WebAssembly
runtime.

### Plugins: one process per call

Simpler than a long-running process, but spec 0369 applies the plugin at
every parse: a Python start per descriptor, thousands on googleapis.

### tree-sitter to split text-format sets

Kept today to slice each `file { … }` entry before `text_format.Parse`.
S5's parser gives the same isolation, per entry, without a grammar at
run time.

### Fix bugs in the port rather than in the reference

The harness would then need an exception per bug, and a difference would
no longer tell a port error from an intended change. S1 fixes them in
reproto, where the fix is tested against the current behavior.

### Speed up reproto instead

Spec 0425 is that, for the minimization. The rest does not yield to the
same treatment: rendering is 70 s of Python on one core, and Python's
processes would each need the whole graph and pool (the global
interpreter lock rules out threads), at 1.3 GB each.

### Parallel phases 1–6, or a parallel pool

upb adds files to a pool one at a time, and phases 3–6 are graph walks
under a second in Rust. Parallelism goes where the time is: per file,
after them (S17).

### Keep YAML between protoback and prototext-graph

Simplest, since the loader exists, but 41 MB of text written and parsed
back for nothing; S20 keeps the YAML only where a user asks for it.

### Other names

`protoinverse` states the guarantee formally (protoc's inverse), but is
longer; `protobuild` says the opposite direction and is a known
containerd tool; `protosalvage` and `protoscribe` describe the context,
not the exact round trip.

## Test plan

1. **S1:** each fix has a regression test in reproto's suite.
2. **S3–S4:** unit tests of reflection on `descriptor.proto` and on a
   message with custom options: decode, list (`upb_Message_Next` against
   Python's `ListFields()` on the same bytes), set, clear, clone, encode
   back to the same bytes.
3. **S5:** each grammar case, and the googleapis descriptor set in text
   format, parsed by S5, by Python's `text_format` and by
   `protoc --encode`: the same binary where they all know every field.
   Where only S5 keeps a field (by number or by name), protoc's text of
   a binary set read in import order (spec 0427) gives back that binary
   set, byte for byte, groups included when their extension is declared
   in the input.
4. **S3–S5, fuzzing:** the new entry points in spec 0419's targets,
   under AddressSanitizer.
5. **S9:** a plugin that changes a descriptor, through the Python
   adapter and through a command, with the same outputs as reproto; a
   plugin that fails, exits early, or writes a malformed frame, each
   reported as a `PluginError` naming the file; the cache, by counting
   calls.
6. **S10:** a million draws of each method against CPython for a few
   seeds, then the instances of corpus 4.
7. **S11, S18:** the harness, every corpus, with `--jobs 1` and all
   cores, no difference outside S12.
8. **S2, S15:** reproto's CLI tests pass against protoback.
9. **S14:** completion tests in `prototools-complete`'s style; the man
   pages generated.
10. **S16, S21:** `scripts/bench-protoback` on googleapis: under 10 s and
    under 1.29 GB, on vCPUs 0–7; `--jobs 1` reported beside it.

## Measured outcome

Filled in step by step. Timings: `scripts/bench-protoback`, the
development VM pinned to vCPUs 0–7, on mains power, host idle.

- **Step 0** (spec 0425): `build_graph` on googleapis from 55 s to
  1.2 s, `hopcroft.rkyv` byte-identical. The 55 s was taken unpinned:
  indicative only.
- **Step 1** (2026-11-14):
  - S1: four bugs reproduced by a failing test each, and fixed in
    reproto; the fifth suspected one, set-order dependence, not
    reproduced (S1's text). reproto's googleapis output is unchanged by
    the fixes.
  - S2: the CLI tests run through `REPROTO_CMD`; `REPROTO` overrides it.
  - S11: `protoback-harness` (`ci`, 521 cases) and
    `protoback-harness-full` (`full-tests`, 522 with googleapis), reproto
    on both sides: no case differs, none exits with an error. A first
    version found that, without `--use-variant`, 67 of the 73 fixtures
    only exercised reproto's refusal to run without `descriptor.proto`;
    every case now has a variant, and the report lists the cases where
    the reference fails.
  - S21, reproto's baseline at 18dce2f:

    | Case | Wall clock | Peak memory |
    |---|---|---|
    | WKT (`wktRkyv`'s flags) | 0.48 s ± 0.01, 6 runs | 42 MB |
    | custom (`customDb`'s flags) | 0.64 s ± 0.01, 5 runs | 45 MB |
    | googleapis (`googleapisDb`'s flags) | 167.2 s ± 0.6, 3 runs | 1.29 GB |

    Unpinned runs earlier that day, with spec 0425 in, ranged from
    2 min 43 s to 3 min 33 s; rendering, whose code did not change all
    day, took 70 s in one unpinned run and 109 s in another: the guest
    scheduler puts the process on an E-core or on a P-core's thread
    (vCPUs 8–11), twice as fast for 9 and 11
    (docs/prototext/bench-process.md). Only pinned numbers compare.
- **Step 2** (2026-11-14), in `prototext-upb`:
  - S3: `DynamicMessage`, `MessageRef`, `MessageMut`, over 21 upb
    functions. The options fixture (custom options of every shape, on
    every kind of definition) decodes and encodes back to protoc's bytes;
    `set_fields` lists what Python's `ListFields()` lists for each of its
    options messages: `upb_Message_Next` walks the MiniTable, built in
    field-number order, then the extensions.
  - S4: services, methods, extension ranges, `all_extensions` sorted by
    number, over 23 more. Options through descriptors read as messages,
    not `upb_XxxDef_Options` (S4's text).
  - S19: `tests/threads.rs` builds a pool, then reads it from every core
    at once, each thread decoding its own messages, and all of them one
    shared message: every thread's result equals the single-threaded
    one. `scripts/upb-tsan` runs it with upb's C under
    ThreadSanitizer: no report, on the fixture and on googleapis (12
    threads, 107 s). What TSan cannot see, being uninstrumented Rust,
    is ordering made by atomics and futexes, which it reports as races:
    the checks spawn and join (`pthread_join`, which it sees) rather than
    use `std::thread::scope`, and the file runs without libtest
    (`harness = false`), whose own threads it would report. A test that
    adds files while others read, ordered by the pool's `RwLock`, is
    outside what TSan can check.
  - Every test passes, also under AddressSanitizer (`scripts/upb-asan`).
  - S6: `src/descriptor_access.rs`, 4,600 lines, a reader and a writer
    for each of `descriptor.proto`'s 35 messages and constants for its
    enums, documented with its comments; generated by
    `tests/descriptor_access.rs` from `vendor/descriptor.pb`, laid out by
    rustfmt. The input is protobuf v36.2's `descriptor.proto`, the
    vendored upb's: each of its 177 fields has its generated accessor in
    `upb.h`. protoc 34.1 refuses it (newer removal metadata on
    `cc_enable_arenas`); 36.2, from the nixos-unstable pin, compiles it.
    `scripts/update-upb`, which `vendor/VERSION` cited but did not exist,
    vendors upb, that file and its compilation; run on 0.36.2 it
    reproduced every vendored file byte for byte. The up-to-date check is
    a test ignored by a downstream `cargo test`, which would otherwise need
    rustfmt, and run by `ci` (`descriptorAccessCheck`): a stale file fails
    it, with the command to regenerate. In use, on the options fixture
    and on a cut-down `descriptor.proto` in package `proto2`, where what
    the version lacks reads as absent, cannot be written, and comes back
    complete in its unknown fields.
- **Step 3** (2026-11-14/15): the text format (S5), read in import order
  (specs 0426, 0427); the variants' import section removed.
  - Text input: googleapis' text (`protoc --decode`, descriptor.proto
    alone, 56 MB) comes back byte for byte; read in 2.04 s on one E-core,
    against 3.35 s for the two passes it replaced (spec 0427's measured
    outcome). Python's `text_format` took 75 s on the same text and
    dropped every option by number.
  - S21, reproto at 7268428 (step 3 and the pragma cleanup) against
    18dce2f (the baseline), 2026-11-15, the same session, back to back,
    the same libraries (the dev shell's `prototext_codec_lib`,
    `prototext_graph_lib`, `fdp_scan_lib`, built from 7268428; only
    reproto's Python differs, from a worktree for 18dce2f),
    `scripts/bench-protoback "python3 -m reproto.cli"`, default pinning
    (vCPUs 0–7), host idle:

    | Case | 18dce2f | 7268428 | Peak memory, both |
    |---|---|---|---|
    | WKT | 0.48 s ± 0.01, 6 runs | 0.47 s ± 0.01, 6 runs | 42 MB |
    | custom | 0.63 s ± 0.01, 5 runs | 0.62 s ± 0.01, 5 runs | 45 MB |
    | googleapis | 168.2 s ± 0.3 (168.5, 168.0, 168.0) | 170.2 s ± 0.8 (170.3, 171.0, 169.4) | 1.29 GB |

    googleapis is 2.0 s (1.2 %) slower. One run of each under
    `scripts/reproto-phase-times`, same flags and pinning, puts all of
    it in rendering: `_phase7_output` 109.6 s against 111.2 s (+1.6 s;
    wall 165.7 s against 167.2 s); every other phase within 0.2 s
    (`_phase_build_schema_db` 24.1, `_phase_emit_scoring_graphs` 23.1,
    `yaml.dump` 41.0 and 40.9 in all, counted in its callers' phases too,
    `_phase3_build_graph` 4.8–4.9,
    `_phase6_summoning` 2.2, `_phase2_build_pool` 1.1,
    `_phase5_reachability` 0.5, `_phase1_load_files` 0.4). Not
    explained: no code rendering runs changed between the two commits
    (`_phase7_output` and the `re_*` modules are identical); on binary
    input the text path returns at once; the changes on the path are a
    `QualFile.imports` property the topology reads (phase 1, unchanged
    in time), a `None` check in `QualFile.desc`, which the `re_*`
    modules do not read, and the progress callback, a named function
    instead of a lambda, called once per file (7,771 calls). Left: an
    effect of the same order as Python's per-run hash seed and memory
    layout. Recorded, not pursued: rendering is replaced by protoback's
    in later steps.
  - To come back to it: rebuild the corpus (`scripts/protoback-harness
    prepare DIR --googleapis googleapis.pb --otel OTEL`, OTEL the
    `corpusOtel` fetch of nix/python.nix), add a worktree for each
    commit, and run `scripts/bench-protoback` with
    `PYTHONPATH=WORKTREE/reproto/src` for each, back to back; then
    `scripts/reproto-phase-times` with the googleapis case's flags for
    each, and, if a phase differs, a profile of that phase in both
    (`python -m cProfile -o …`), several runs each, `PYTHONHASHSEED`
    fixed to separate the seed's effect from the code's.
- **Step 4** (2026-11-15), spec 0428: protoback's loading, pool and
  phases 1 to 6, against reproto's by the graph dump (`--dump-graph`,
  hidden, in both): identical on all 542 harness cases, googleapis
  included, every section, in `ci` (`protobackDumpHarness`) and
  `full-tests`. Four reproto bugs fixed on the way, each with a failing
  test first (spec 0428 S7, S8). Phases 1 to 6 and the dump on
  googleapis, one thread: 2.22 s against reproto's 12.58 s, 443 MB
  against 674 MB (spec 0428, measured outcome).
- **Step 5** (2026-11-16), spec 0429: rendering, in six stages
  (structure; built-in options; custom options and editions; comments;
  `--emit-binary` with its SourceCodeInfo), then files rendered in
  parallel on a rayon pool (`--jobs`): every `.proto` and `.pb` file
  identical to reproto's, on all cores and with `--jobs 1`. Rendering
  googleapis under `--emit-binary`, without the schema database: 5.54 s
  on 8 threads, 6.11 s on one, against reproto's 122 s.
- **Step 6** (2026-11-16), spec 0430: the scoring graphs and
  `--emit-scoring-yaml`, the schema database (`prototext-graph` takes
  the graphs as structures, S20), `--scoring-html-out`. The harness,
  every output compared, every corpus: 1,567 cases, all identical, on
  all cores and with `--jobs 1`; `ci`'s `protobackHarness` now runs
  protoback against reproto. WKT's database reproduces
  `prototext/wkt/prebuilt`. S16 met: googleapis with `googleapisDb`'s
  flags, 7.49 s and 1.25 GB, against reproto's 176 s and 1.30 GB (spec
  0430, measured outcome).
