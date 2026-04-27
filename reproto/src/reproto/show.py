# SPDX-FileCopyrightText: 2026 THALES CLOUD SECURISE SAS
#
# SPDX-License-Identifier: MIT

# graph_viewer.py
from pathlib import Path

from pyvis.network import Network

from typing import Any

from .base import NodeBase
from .context import Context

# FQDNs of interest
FOI = [
    'desc:.production_midas.Manifest',
    'fdsc:.production_midas.Manifest.data_governance_annotations',
    'desc:.production_midas.Version',
    'file:production/midas/proto/mantle/package_metadata.proto'
    ''
]

def is_in(node: NodeBase[Any]) -> bool:
    return 'midas' in node.fqdn or 'datapol' in node.fqdn

def show_graph(ctx: Context, notebook=False, output_path=Path('graph.html')):
    """
    Display the interactive graph of ctx.nodes where each node has .targets
    """
    # Create a PyVis network
    net = Network(height="90vh", width="100%", directed=True, bgcolor="#222222", cdn_resources='in_line')
    
    # Improve how physics looks
    net.barnes_hut()

    # Add nodes
    for fqdn, node in ctx.nodes.items():
        if not is_in(node):
            continue
        if node.is_pruned:
            color = "#000000"
        else:
            color = "#97fc9a"
        if node.is_summoned:
            shape = 'square'
        else:
            shape = 'dot'
        label = str(fqdn)
        if label.startswith('file:'):
            size = 20
        elif label == 'desc:.production_midas.Manifest':
            size = 40
            color = "#FF0000"
        elif label == 'desc:.production_midas.Version':
            size = 40
            color = "#FF9D00"
        else:
            size = 10
        net.add_node(fqdn, label=label, title=label, shape=shape, color=color, size=size)

    # Add edges
    for fqdn, node in ctx.nodes.items():
        if not is_in(node):
            continue
        #parent = node.parent
        #if parent is not None and is_in(parent):
        #    net.add_edge(fqdn, parent.fqdn, color="#000000")
        for target in node.targets:
            if not is_in(target):
                continue
            if target not in node.contains:
                net.add_edge(fqdn, target.fqdn, color="#ff0000")
        for child in node.contains:
            if not is_in(child):
                continue
            net.add_edge(fqdn, child.fqdn, color="#000dff")

    # Faster physics convergence
    net.set_options("""
    var options = {
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
    }
    """)

    # Point to local JS/CSS
    net.set_template('local_assets/template.html')

    # Generate and show
    net.write_html(str(output_path), notebook=notebook)

    if notebook:
        from IPython.display import IFrame
        return IFrame(str(output_path), width="100%", height="600px")
