// SPDX-FileCopyrightText: Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Interactive HTML visualisation of the compiled scoring graph (vis-network).

use std::fmt::Write as FmtWrite;
use std::path::Path;

use super::graph::RawGraph;
use super::hopcroft::Partition;
use super::serial::CompiledGraph;

// ── Node label/title helpers ──────────────────────────────────────────────────

/// Returns `(label, title)` for a leaf node given its wire_type and attributes.
fn leaf_label_title(
    wire_type: u8,
    is_string: bool,
    enum_range_idx: u16,
    enum_ranges: &[(i32, i32)],
) -> (String, String) {
    if enum_range_idx != 0xFFFF {
        if let Some(&(min, max)) = enum_ranges.get(enum_range_idx as usize) {
            return ("ENUM".into(), format!("{min}..{max}"));
        }
        return ("ENUM?".into(), String::new());
    }
    match wire_type {
        0 => ("VARINT".into(), String::new()),
        1 => ("FIXED64".into(), String::new()),
        2 if is_string => ("STRING".into(), String::new()),
        2 => ("LENDEL".into(), String::new()),
        3 => ("GROUP".into(), String::new()),
        5 => ("FIXED32".into(), String::new()),
        _ => ("?".into(), String::new()),
    }
}

// ── ScoringKind colour palette ────────────────────────────────────────────────

fn wire_type_color(wire_type: u8) -> &'static str {
    match wire_type {
        0 => "#888888", // VARINT / ENUM
        1 => "#884400", // I64
        2 => "#0066cc", // LEN_*
        3 => "#cc6600", // GROUP
        5 => "#446600", // I32
        _ => "#ffffff",
    }
}

// ── Entry ─────────────────────────────────────────────────────────────────────

pub fn write_html(
    path: &Path,
    raw: &RawGraph,
    partition: &Partition,
    compiled: &CompiledGraph,
) -> Result<(), Box<dyn std::error::Error>> {
    let html = build_html(raw, partition, compiled);
    std::fs::write(path, html)?;
    Ok(())
}

const SHOW_LEAVES: bool = false;

fn build_html(raw: &RawGraph, partition: &Partition, compiled: &CompiledGraph) -> String {
    // Determine which state IDs are leaf nodes (no outgoing transitions).
    let non_leaf_states: std::collections::HashSet<u32> =
        compiled.transitions.iter().map(|t| t.state_id).collect();
    let num_msg_blocks = compiled
        .nodes
        .iter()
        .filter(|n| non_leaf_states.contains(&n.state_id))
        .count();

    // ── Build nodes JSON ──────────────────────────────────────────────────────
    // One node per state (post-Hopcroft), plus leaf nodes, plus one entry
    // node per root FQDN (with IDs starting beyond the state/leaf range).
    let mut state_labels: Vec<Vec<String>> = vec![Vec::new(); partition.num_blocks()];
    for (fqdn, &node_id) in &raw.node_ids {
        let block = partition.block_of(node_id) as usize;
        state_labels[block].push(fqdn.clone());
    }

    let mut nodes_js = String::new();
    // Message states — plain blue dots
    for (block, block_labels) in state_labels[..num_msg_blocks].iter().enumerate() {
        let mut labels = block_labels.clone();
        labels.sort();
        let short = labels
            .iter()
            .min_by_key(|s| s.len())
            .and_then(|s| s.rsplit('.').next())
            .unwrap_or("?");
        let label = if labels.len() > 1 {
            format!("{short}+")
        } else {
            short.to_string()
        };
        let title = {
            let mut sorted = labels.clone();
            sorted.sort();
            format!("{block}<br>{}", sorted.join("<br>"))
        };
        let _ = writeln!(
            nodes_js,
            "  {{id:{block}, label:{label:?}, title:{title:?}, \
             color:'#aaaaff', shape:'dot', size:15}},"
        );
    }
    // Leaf nodes — derived from compiled.nodes where no outgoing transitions exist
    if SHOW_LEAVES {
        for node in compiled.nodes.iter() {
            let id = node.state_id as usize;
            if non_leaf_states.contains(&(id as u32)) {
                continue;
            }
            let (label, title) = leaf_label_title(
                node.wire_type,
                node.is_string,
                node.enum_range_idx,
                &compiled.enum_ranges,
            );
            let _ = writeln!(
                nodes_js,
                "  {{id:{id}, label:{label:?}, title:{title:?}, \
                 color:'#ffcc44', shape:'square', size:10}},"
            );
        }
    }
    // Entry nodes — one diamond per root FQDN
    let entry_id_base = partition.num_blocks(); // IDs beyond state/leaf range
    for (i, root) in compiled.roots.iter().enumerate() {
        let id = entry_id_base + i;
        let short = root.fqdn.rsplit('.').next().unwrap_or(&root.fqdn);
        let _ = writeln!(
            nodes_js,
            "  {{id:{id}, label:{short:?}, title:{fqdn:?}, \
             color:'#ff9944', shape:'diamond', size:20}},",
            fqdn = root.fqdn,
        );
    }

    // ── Build edges JSON ──────────────────────────────────────────────────────
    let mut edges_js = String::new();
    // Transition edges
    for t in &compiled.transitions {
        let child = t.child_state_id;
        if !SHOW_LEAVES && !non_leaf_states.contains(&child) {
            continue;
        }
        let field_num = t.field_number;
        // Derive edge color from child node's wire_type.
        let child_wt = compiled
            .nodes
            .iter()
            .find(|n| n.state_id == child)
            .map_or(0u8, |n| n.wire_type);
        let color = wire_type_color(child_wt);
        let dst = child;
        let _ = writeln!(
            edges_js,
            "  {{from:{src}, to:{dst}, label:'f{field_num}', \
             color:{{color:{color:?}}}, arrows:'to', font:{{size:10}}}},",
            src = t.state_id,
        );
    }
    // Entry → state edges (dashed, no label)
    for (i, root) in compiled.roots.iter().enumerate() {
        let from = entry_id_base + i;
        let to = root.state_id;
        let _ = writeln!(
            edges_js,
            "  {{from:{from}, to:{to}, label:'', \
             color:{{color:'#ff9944'}}, arrows:'to', dashes:true}},",
        );
    }

    // ── Build legend entries ──────────────────────────────────────────────────
    let stats = format!(
        "States: {} &nbsp;|&nbsp; Transitions: {} &nbsp;|&nbsp; Roots: {}",
        compiled.num_states,
        compiled.transitions.len(),
        compiled.roots.len(),
    );

    // ── Assemble HTML ─────────────────────────────────────────────────────────
    format!(
        r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<title>Scoring Graph</title>
<script src="https://cdnjs.cloudflare.com/ajax/libs/vis/4.21.0/vis.min.js"></script>
<link rel="stylesheet" href="https://cdnjs.cloudflare.com/ajax/libs/vis/4.21.0/vis.min.css">
<style>
  body {{ margin:0; background:#222; color:#eee; font-family:sans-serif; }}
  #graph {{ width:100%; height:92vh; border:none; }}
  #bar {{ padding:6px 12px; background:#333; font-size:13px; }}
  #legend {{ display:inline-block; margin-left:20px; }}
  .dot {{ display:inline-block; width:12px; height:12px; border-radius:50%;
           margin-right:4px; vertical-align:middle; }}
  .sq  {{ display:inline-block; width:12px; height:12px;
           margin-right:4px; vertical-align:middle; }}
  .dia {{ display:inline-block; width:12px; height:12px;
           transform:rotate(45deg); margin-right:6px; vertical-align:middle; }}
</style>
</head>
<body>
<div id="bar">
  {stats}
  &nbsp;&nbsp;
  <span id="legend">
    <span class="dia" style="background:#ff9944"></span>Entry point&nbsp;&nbsp;
    <span class="dot" style="background:#aaaaff"></span>Message state&nbsp;&nbsp;
    <span class="sq"  style="background:#ffcc44"></span>Leaf (scalar)&nbsp;&nbsp;
    <span style="color:#888888">&#9644;</span> VARINT &nbsp;
    <span style="color:#884400">&#9644;</span> I64 &nbsp;
    <span style="color:#0066cc">&#9644;</span> LEN &nbsp;
    <span style="color:#cc6600">&#9644;</span> GROUP &nbsp;
    <span style="color:#446600">&#9644;</span> I32
  </span>
</div>
<div id="graph"></div>
<script>
var nodes = new vis.DataSet([
{nodes_js}]);
var edges = new vis.DataSet([
{edges_js}]);
var container = document.getElementById('graph');
var data = {{ nodes: nodes, edges: edges }};
var options = {{
  physics: {{
    enabled: true,
    barnesHut: {{
      gravitationalConstant: -8000,
      centralGravity: 0.3,
      springLength: 120,
      springConstant: 0.04,
      damping: 0.09,
      avoidOverlap: 1
    }},
    minVelocity: 0.75,
    stabilization: {{ enabled: false }}
  }},
  nodes: {{ font: {{ color: '#eee' }} }}
}};
var network = new vis.Network(container, data, options);
network.on('stabilizationIterationsDone', function() {{
  network.setOptions({{ physics: {{ enabled: false }} }});
}});
</script>
</body>
</html>
"#,
    )
}
