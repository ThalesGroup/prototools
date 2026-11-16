# SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

"""--scoring-html-out's drawing (spec 0430, stage 3).

`--hide` takes `desc:` patterns, normalised as `--prune`'s are, and drops
the states a root of which they match; a compiled graph whose lists are
empty, which `dump_compiled` writes as nothing (YAML null), draws empty.
"""

import re
from pathlib import Path

from reproto.show import render_scoring_graph

# Two named states, a leaf, and the edges between them.
GRAPH = """\
states:
  - id: 0
    type: message
  - id: 1
    type: message
  - id: 2
    type: int32
transitions:
  - from: 0
    field: 1
    label: optional
    to: 1
  - from: 1
    field: 2
    label: optional
    to: 2
roots:
  - fqdn: a.Keep
    state: 0
  - fqdn: a.FieldMask
    state: 1
"""


def _node_ids(html: str) -> set[int]:
    nodes = re.search(r'nodes = new vis\.DataSet\((\[.*?\])\);', html)
    assert nodes is not None
    return {int(i) for i in re.findall(r'"id": (\d+)', nodes.group(1))}


def _draw(tmp_path: Path, graph: str, hide: tuple[str, ...] = ()) -> str:
    out = tmp_path / 'graph.html'
    render_scoring_graph(graph, out, 'test', with_leaf_nodes=False, hide=hide)
    return out.read_text()


def test_hide_drops_the_states_a_desc_pattern_matches(tmp_path: Path) -> None:
    # As the command line hands it over: `desc:a.Field*`, normalised.
    html = _draw(tmp_path, GRAPH, hide=('desc:a/Field*',))
    assert _node_ids(html) == {0}
    assert '"to": 1' not in html


def test_hide_keeps_the_states_no_pattern_matches(tmp_path: Path) -> None:
    html = _draw(tmp_path, GRAPH, hide=('desc:b/**',))
    assert _node_ids(html) == {0, 1}


def test_a_graph_without_transitions_draws(tmp_path: Path) -> None:
    graph = 'states:\n  - id: 0\n    type: message\ntransitions:\nroots:\n'
    html = _draw(tmp_path, graph)
    assert 'nodes = new vis.DataSet([]);' in html
    assert 'edges = new vis.DataSet([]);' in html


def test_an_empty_graph_draws(tmp_path: Path) -> None:
    html = _draw(tmp_path, 'states:\ntransitions:\nroots:\n')
    assert 'nodes = new vis.DataSet([]);' in html
