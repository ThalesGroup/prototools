# SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
#
# SPDX-License-Identifier: MIT

from pathlib import Path
from typing import cast

import yaml
from pyvis.network import Network


def _fqdn_matches_any(fqdn: str, patterns: tuple[str, ...]) -> bool:
    """Whether a root's FQDN matches a `--hide` pattern: `desc:` patterns,
    normalised as `--prune`'s are, matched as `--prune` matches them."""
    from .phases import fqdn_match
    from .context import Fqdn
    return any(fqdn_match(Fqdn(p), Fqdn(f'desc:{fqdn}')) for p in patterns)


def _make_network(title: str) -> Network:
    """Create a pyvis Network with shared settings."""
    net = Network(
        height="90vh",
        width="100%",
        directed=True,
        bgcolor="#222222",
        cdn_resources='in_line',
    )
    net.heading = ''
    net.barnes_hut()
    net.set_options("""{
  "nodes": {
    "font": { "face": "sans-serif", "color": "#88cc88", "size": 13 }
  },
  "edges": {
    "font": { "face": "sans-serif", "color": "#888888", "size": 11,
              "align": "middle" }
  },
  "physics": {
    "enabled": true,
    "barnesHut": {
      "gravitationalConstant": -8000,
      "centralGravity": 0.3,
      "springLength": 100,
      "springConstant": 0.04,
      "damping": 0.09,
      "avoidOverlap": 1
    },
    "minVelocity": 0.75
  }
}""")
    return net


# 5-step Fibonacci brightness scale for top-level (named) nodes.
# Each step covers a Fibonacci range of merged-type counts:
#   step 1: count == 1          (single)
#   step 2: count == 2
#   step 3: count in 3..4
#   step 4: count in 5..7
#   step 5: count >= 8
# Colours are amber/gold, evenly spread so adjacent steps are clearly distinct.
_BRIGHTNESS_STEPS = [
    (1, '#664400'),   # step 1 — darkest amber
    (2, '#995500'),   # step 2
    (3, '#cc8800'),   # step 3
    (5, '#ffaa00'),   # step 4
    (8, '#ffdd44'),   # step 5 — brightest gold
]


def _node_colour(fqdns: list[str]) -> str:
    """Compute node colour for a non-leaf message node.

    Top-level nodes (have at least one FQDN) use an amber/gold hue on a
    5-step Fibonacci brightness scale keyed on merged-type count.
    Internal nodes (no FQDN) use dim blue.
    """
    count = len(fqdns)
    if count == 0:
        return '#3355aa'  # dim blue — internal node with no named root
    colour = _BRIGHTNESS_STEPS[0][1]
    for threshold, c in _BRIGHTNESS_STEPS:
        if count >= threshold:
            colour = c
    return colour


def render_scoring_graph(
    compiled_yaml: str,
    output_path: Path,
    title: str,
    with_leaf_nodes: bool = False,
    hide: tuple[str, ...] = (),
) -> None:
    """Render a compiled scoring graph (states/transitions/roots YAML) to HTML."""
    data = cast(dict, yaml.safe_load(compiled_yaml))
    # `dump_compiled` writes an empty list as nothing, which YAML reads as
    # null: a graph without transitions draws empty.
    states = data.get('states') or []
    transitions = data.get('transitions') or []
    roots = data.get('roots') or []
    net = _make_network(title)

    # Determine which states have outgoing transitions (non-leaf) vs leaf.
    states_with_outgoing: set[int] = {t['from'] for t in transitions}
    leaf_states: set[int] = {
        s['id'] for s in states if s['id'] not in states_with_outgoing
    }

    # Build tooltip: state_id -> sorted list of FQDNs from roots.
    state_fqdns: dict[int, list[str]] = {}
    for root in roots:
        sid = root['state']
        state_fqdns.setdefault(sid, []).append(root['fqdn'])
    for fqdns in state_fqdns.values():
        fqdns.sort()

    # Compute hidden state IDs from --hide patterns.
    hidden_ids: set[int] = set()
    if hide:
        for sid, fqdns in state_fqdns.items():
            if any(_fqdn_matches_any(f, hide) for f in fqdns):
                hidden_ids.add(sid)

    for state in states:
        sid = state['id']
        wt = state['type']
        is_leaf = sid in leaf_states

        if sid in hidden_ids:
            continue
        if is_leaf and not with_leaf_nodes:
            continue

        fqdns = state_fqdns.get(sid, [])
        tooltip = '\n'.join(fqdns) if fqdns else ''
        # Label: short type name for named nodes, "+N" suffix when merged, blank for internal.
        if len(fqdns) == 1:
            node_label = fqdns[0].rsplit('.', 1)[-1]
        elif len(fqdns) > 1:
            node_label = f'{fqdns[0].rsplit(".", 1)[-1]}+{len(fqdns) - 1}'
        else:
            node_label = ''

        if not is_leaf:
            net.add_node(
                sid,
                label=node_label,
                title=tooltip,
                shape='dot',
                color=_node_colour(fqdns),
                size=20,
            )
        else:
            if wt == 'string':
                label = 'string'
                colour = '#ff8844'
            elif wt in ('range', 'uint32', 'int32', 'uint64'):
                label = wt
                colour = '#ffcc44'
            elif wt == 'double':
                label = 'i64'
                colour = '#ffcc44'
            elif wt == 'bytes':
                label = 'len'
                colour = '#ffcc44'
            elif wt == 'float':
                label = 'i32'
                colour = '#ffcc44'
            else:
                label = str(wt)
                colour = '#ffcc44'
            net.add_node(
                sid,
                label=label,
                title=tooltip,
                shape='square',
                color=colour,
                size=12,
            )

    edge_colours = {
        'optional': '#4444ff',
        'repeated': '#44aaff',
        'packed':   '#884488',
        'required': '#cc2222',
    }
    for t in transitions:
        if t['from'] in hidden_ids or t['to'] in hidden_ids:
            continue
        if not with_leaf_nodes and t['to'] in leaf_states:
            continue
        colour = edge_colours.get(t['label'], '#888888')
        net.add_edge(
            t['from'],
            t['to'],
            label=str(t['field']),
            color=colour,
        )

    net.write_html(str(output_path))
    # Patch tooltip CSS: pyvis uses white-space:nowrap which prevents line
    # breaks in multi-FQDN tooltips.  Switch to pre-wrap so \n is respected.
    html = output_path.read_text()
    html = html.replace('white-space:nowrap', 'white-space:pre-wrap', 1)
    output_path.write_text(html)
