// SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
//
// SPDX-License-Identifier: MIT

//! Hopcroft DFA minimization over the raw scoring graph (spec 0062), with
//! a sparse worklist (spec 0425).
//!
//! The textbook Hopcroft (1971) algorithm:
//!
//!   P  := initial partition
//!   W  := P   (worklist = every (block, symbol) pair)
//!
//!   while W not empty:
//!       pick and remove A from W
//!       for each symbol (f, l):
//!           X := predecessors of A via (f, l)
//!           for each block Y where Y∩X ≠ ∅ and Y\X ≠ ∅:
//!               split Y into Y₁ = Y∩X and Y₂ = Y\X
//!               for each symbol (f', l'):
//!                   if (Y, f', l') ∈ W:
//!                       replace (Y, f', l') with (Y₁, f', l') and (Y₂, f', l')
//!                   else:
//!                       add smaller(Y₁, Y₂) for (f', l') to W
//!
//! [`minimize`] never queues a pair (A, c) whose block A has no incoming
//! edge labeled c (spec 0425 S1). Such a pair has no predecessors and splits
//! nothing, and it stays so: blocks only shrink. Leaving them out leaves the
//! pairs that do split, and their order, as the textbook has them, so the
//! refinement makes the same splits, in the same order, with the same block
//! IDs. The textbook version stays, for tests, as `minimize_textbook`.

use std::collections::{HashMap, HashSet, VecDeque};

use super::graph::{
    leaf_attrs, LeafRegistry, RawGraph, ANY_NODE_ID, LEAF_I32, LEAF_I64, LEAF_INT32, LEAF_LEN,
    LEAF_STRING, LEAF_UINT32, LEAF_UINT64, MESSAGE_SET_NODE_ID, NUM_FIXED_LEAVES,
};
use super::serial::{NO_EXT_RANGES, WT_NODE_MESSAGE};

/// The three attributes that separate message blocks in the initial partition:
/// framing (`wire_type`), extensibility (`ext_range_idx`, spec 0238 S7) and the
/// outgoing `(field_number, symbol)` signature, where the symbol is the
/// label plus the declared packing (`RawEdge::symbol`, spec 0371 S4).
type MessageBlockKey = (u8, u16, Vec<(u32, u8)>);

/// A letter of the alphabet: `(field_number, RawEdge::symbol())`.
type Letter = (u32, u8);

// ── Partition ─────────────────────────────────────────────────────────────────

pub struct Partition {
    /// block_of[node_index] = block ID.
    /// Indices 0..msg_count are message nodes; msg_count+i is leaf i.
    pub block_of: Vec<u32>,
    num_blocks: usize,
}

impl Partition {
    pub fn block_of(&self, node: u32) -> u32 {
        self.block_of[node as usize]
    }

    pub fn num_blocks(&self) -> usize {
        self.num_blocks
    }
}

// ── Minimization ──────────────────────────────────────────────────────────────

/// Minimize `raw`: the coarsest partition of its nodes stable under every
/// letter, refining the initial partition.
pub fn minimize(
    raw: &RawGraph,
    reg: &LeafRegistry,
    node_wire_types: &HashMap<u32, u8>,
    on_progress: impl FnMut(u64, u64),
) -> Partition {
    let setup = Setup::new(raw, reg, node_wire_types);
    let block_of = refine_sparse(&setup, on_progress);
    renumber(&block_of, setup.msg_count, setup.num_leaves)
}

/// The textbook refinement of spec 0062, kept as the oracle `minimize` is
/// tested against (spec 0425 S6).
#[cfg(test)]
pub(crate) fn minimize_textbook(
    raw: &RawGraph,
    reg: &LeafRegistry,
    node_wire_types: &HashMap<u32, u8>,
) -> Partition {
    let setup = Setup::new(raw, reg, node_wire_types);
    let block_of = refine_textbook(&setup, |_, _| {});
    renumber(&block_of, setup.msg_count, setup.num_leaves)
}

/// What both refinements start from: the node index space, the reverse
/// adjacency, and the initial partition.
struct Setup {
    n: usize,
    msg_count: usize,
    num_leaves: usize,
    /// rev[di] = the edges entering node di, as (field, symbol, source
    /// index), sorted: the predecessors through one letter are a range
    /// (spec 0425 S2).
    rev: Vec<Vec<(u32, u8, usize)>>,
    /// The initial partition P₀.
    block_of: Vec<usize>,
    num_blocks: usize,
}

impl Setup {
    fn new(raw: &RawGraph, reg: &LeafRegistry, node_wire_types: &HashMap<u32, u8>) -> Self {
        let num_leaves = reg.num_leaves();
        let n = raw.num_nodes as usize;
        let msg_count = n - num_leaves;

        // ── node_index: map raw node ID to contiguous index 0..n ─────────────
        // Dynamic range sentinels descend from u32::MAX-8:
        //   sentinel for range leaf i = u32::MAX - 8 - i
        // With num_dynamic = num_leaves - NUM_FIXED_LEAVES dynamic leaves, the
        // lowest dynamic sentinel is u32::MAX - 8 - (num_dynamic - 1).
        let num_dynamic = if num_leaves > NUM_FIXED_LEAVES {
            (num_leaves - NUM_FIXED_LEAVES) as u32
        } else {
            0
        };
        // Inclusive lower bound of the dynamic sentinel range (wraps to 0 when
        // num_dynamic == 0, but then `x <= u32::MAX - 8` can't hold because all
        // fixed-leaf arms fire first).
        let dynamic_sentinel_min = (u32::MAX - 8).wrapping_sub(num_dynamic.wrapping_sub(1));

        let node_index = |node: u32| -> usize {
            match node {
                x if x == LEAF_UINT64 => msg_count,
                x if x == LEAF_UINT32 => msg_count + 1,
                x if x == LEAF_INT32 => msg_count + 2,
                x if x == LEAF_I64 => msg_count + 3,
                x if x == LEAF_LEN => msg_count + 4,
                x if x == LEAF_STRING => msg_count + 5,
                x if x == LEAF_I32 => msg_count + 6,
                x if num_dynamic > 0 && x >= dynamic_sentinel_min && x <= u32::MAX - 8 => {
                    let i = (u32::MAX - 8).wrapping_sub(x) as usize;
                    msg_count + NUM_FIXED_LEAVES + i
                }
                x => x as usize,
            }
        };

        // ── Reverse adjacency ─────────────────────────────────────────────────
        let mut rev: Vec<Vec<(u32, u8, usize)>> = vec![Vec::new(); n];
        for edge in &raw.edges {
            let si = node_index(edge.src);
            let di = node_index(edge.dst);
            rev[di].push((edge.field_number, edge.symbol(), si));
        }
        for r in rev.iter_mut() {
            r.sort_unstable();
        }

        // ── Outgoing signature per node ───────────────────────────────────────
        let mut sig: Vec<Vec<Letter>> = vec![Vec::new(); n];
        for edge in &raw.edges {
            let si = node_index(edge.src);
            sig[si].push((edge.field_number, edge.symbol()));
        }
        for s in sig.iter_mut() {
            s.sort_unstable();
            s.dedup();
        }

        // ── Initial partition P₀ ─────────────────────────────────────────────
        // The message key carries `ext_range_idx` (spec 0238 S7): extensibility is
        // an attribute of the state, which no edge carries, so it cannot join Σ —
        // seeding the partition with it is what keeps an extensible message and a
        // structurally identical closed one in separate blocks forever.
        let mut sig_to_block: HashMap<MessageBlockKey, usize> = HashMap::new();
        let mut leaf_attr_to_block: HashMap<(u8, bool, u16), usize> = HashMap::new();
        // Note: leaf partition key is (wire_type, is_string, range_idx).
        // wire_type 8 = UINT32, 9 = INT32 ensure they start in distinct classes.
        // Message states now key on 10 = MESSAGE (spec 0324 S2) where they used
        // to key on 2. Every message moves together and no leaf moves at all, so
        // the initial partition is renamed, not repartitioned.
        let mut block_of: Vec<usize> = vec![0usize; n];
        let mut num_blocks: usize = 0;

        for i in 0..msg_count {
            // Reserved nodes (spec 0089 §9): ANY_NODE_ID and MESSAGE_SET_NODE_ID each
            // get their own unconditional singleton block so they are never merged
            // with any other node regardless of structural equivalence.
            let node_id = i as u32;
            if node_id == ANY_NODE_ID || node_id == MESSAGE_SET_NODE_ID {
                let b = num_blocks;
                num_blocks += 1;
                block_of[i] = b;
                continue;
            }
            let wt = node_wire_types
                .get(&node_id)
                .copied()
                .unwrap_or(WT_NODE_MESSAGE);
            let ext = raw
                .node_ext_range_idx
                .get(&node_id)
                .copied()
                .unwrap_or(NO_EXT_RANGES);
            let s = sig[i].clone();
            let b = sig_to_block.entry((wt, ext, s)).or_insert_with(|| {
                let b = num_blocks;
                num_blocks += 1;
                b
            });
            block_of[i] = *b;
        }
        for li in 0..num_leaves {
            let sentinel = leaf_sentinel_for_index(li, reg);
            let attrs = leaf_attrs(sentinel, reg);
            let key = (attrs.wire_type, attrs.is_string, attrs.range_idx);
            let b = leaf_attr_to_block.entry(key).or_insert_with(|| {
                let b = num_blocks;
                num_blocks += 1;
                b
            });
            block_of[msg_count + li] = *b;
        }

        Setup {
            n,
            msg_count,
            num_leaves,
            rev,
            block_of,
            num_blocks,
        }
    }

    /// Block membership: blocks[id] = set of node indices.
    fn blocks(&self) -> Vec<HashSet<usize>> {
        let mut blocks: Vec<HashSet<usize>> = vec![HashSet::new(); self.num_blocks];
        for i in 0..self.n {
            blocks[self.block_of[i]].insert(i);
        }
        blocks
    }

    /// The sources of the edges entering `di` labeled `letter`.
    fn predecessors(&self, di: usize, letter: Letter) -> impl Iterator<Item = usize> + '_ {
        let r = &self.rev[di];
        let start = r.partition_point(|&(f, l, _)| (f, l) < letter);
        r[start..]
            .iter()
            .take_while(move |&&(f, l, _)| (f, l) == letter)
            .map(|&(_, _, si)| si)
    }

    /// The letters of the edges entering a node of `nodes`, sorted.
    fn letters_into<'a>(&self, nodes: impl IntoIterator<Item = &'a usize>) -> Vec<Letter> {
        let mut v: Vec<Letter> = nodes
            .into_iter()
            .flat_map(|&di| self.rev[di].iter().map(|&(f, l, _)| (f, l)))
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}

/// The progress budget: Σ_blocks log₂(|block|), the Hopcroft upper bound
/// on splits. Progress is reported as worklist drain scaled onto
/// [0, budget].
fn progress_budget(blocks: &[HashSet<usize>]) -> u64 {
    let log2 = |x: usize| -> f64 {
        if x > 1 {
            (x as f64).log2()
        } else {
            0.0
        }
    };
    blocks.iter().map(|b| log2(b.len())).sum::<f64>().ceil() as u64
}

/// Report progress after an iteration: scale worklist drain onto
/// [0, budget] and fire only when the integer percentage advances by ≥ 1,
/// to avoid overwhelming Python with callbacks.
fn report_progress(
    on_progress: &mut impl FnMut(u64, u64),
    iterations: u64,
    budget: u64,
    initial_worklist_size: u64,
    last_reported: &mut u64,
) {
    if budget > 0 && initial_worklist_size > 0 {
        let current = (iterations * budget / initial_worklist_size).min(budget - 1);
        let pct = current * 100 / budget;
        if pct > *last_reported {
            on_progress(current, budget);
            *last_reported = pct;
        }
    }
}

/// Group the predecessor set X by current block, in block order.
///
/// Sorted, because each split allocates `y1_id = blocks.len()`: iterating
/// the `HashMap` directly made block *numbering* differ between processes
/// (spec 0177 S3). Which nodes share a block is unaffected either way — the
/// refinement's fixed point is the unique coarsest partition.
fn group_by_block(x: &HashSet<usize>, block_of: &[usize]) -> Vec<(usize, Vec<usize>)> {
    let mut x_in_block: HashMap<usize, Vec<usize>> = HashMap::new();
    for &node in x {
        x_in_block.entry(block_of[node]).or_default().push(node);
    }
    let mut x_in_block: Vec<(usize, Vec<usize>)> = x_in_block.into_iter().collect();
    x_in_block.sort_unstable_by_key(|(y_id, _)| *y_id);
    x_in_block
}

/// Split `y1_nodes` out of block `y_id` into a new block; return its ID.
/// Y₂, the rest, keeps `y_id`.
fn split(
    blocks: &mut Vec<HashSet<usize>>,
    block_of: &mut [usize],
    y_id: usize,
    y1_nodes: &[usize],
) -> usize {
    let y1_id = blocks.len();
    let mut y1_block: HashSet<usize> = HashSet::with_capacity(y1_nodes.len());
    for &node in y1_nodes {
        blocks[y_id].remove(&node);
        y1_block.insert(node);
        block_of[node] = y1_id;
    }
    blocks.push(y1_block);
    y1_id
}

/// The refinement with a sparse worklist (spec 0425 S1).
fn refine_sparse(setup: &Setup, mut on_progress: impl FnMut(u64, u64)) -> Vec<usize> {
    let mut block_of = setup.block_of.clone();
    let mut blocks = setup.blocks();

    // ── Worklist W: (block_id, field, label) ─────────────────────────────────
    // Only the letters entering each block. in_worklist mirrors the deque
    // contents for O(1) membership queries.
    let mut worklist: VecDeque<(usize, u32, u8)> = VecDeque::new();
    let mut in_worklist: HashSet<(usize, u32, u8)> = HashSet::new();
    for (b, block) in blocks.iter().enumerate() {
        for (f, l) in setup.letters_into(block) {
            worklist.push_back((b, f, l));
            in_worklist.insert((b, f, l));
        }
    }

    let budget = progress_budget(&blocks);
    let initial_worklist_size = worklist.len() as u64;
    let mut iterations: u64 = 0;
    let mut last_reported: u64 = 0;

    // Fire once before the loop so Python learns the total immediately.
    on_progress(0, budget);

    // ── Refinement loop ───────────────────────────────────────────────────────
    while let Some((a_id, field, label)) = worklist.pop_front() {
        iterations += 1;
        in_worklist.remove(&(a_id, field, label));

        // X = predecessors of block A via (field, label).
        let x: HashSet<usize> = blocks[a_id]
            .iter()
            .flat_map(|&di| setup.predecessors(di, (field, label)))
            .collect();

        for (y_id, y1_nodes) in group_by_block(&x, &block_of) {
            if y1_nodes.len() == blocks[y_id].len() {
                // Y ⊆ X: no split needed.
                continue;
            }
            let y1_id = split(&mut blocks, &mut block_of, y_id, &y1_nodes);
            let y2_id = y_id;
            let y1_smaller = blocks[y1_id].len() <= blocks[y2_id].len();

            // The textbook rule, for the letters that can split: a pair it
            // would add for a block with no incoming edge of that letter is
            // left out. Y₂'s letters matter only when Y₂ is the smaller half.
            let y1_letters = setup.letters_into(&blocks[y1_id]);
            let y2_letters = if y1_smaller {
                Vec::new()
            } else {
                setup.letters_into(&blocks[y2_id])
            };
            let mut letters = [y1_letters.as_slice(), y2_letters.as_slice()].concat();
            letters.sort_unstable();
            letters.dedup();

            for (fp, lp) in letters {
                let (target, target_letters) = if in_worklist.contains(&(y_id, fp, lp)) {
                    // (Y, f', l') ∈ W: replace Y with Y₁ and Y₂. Y₂ keeps the
                    // original y_id slot, still queued; add Y₁.
                    (y1_id, &y1_letters)
                } else if y1_smaller {
                    // (Y, f', l') ∉ W: add only the smaller of Y₁ and Y₂.
                    (y1_id, &y1_letters)
                } else {
                    (y2_id, &y2_letters)
                };
                if target_letters.binary_search(&(fp, lp)).is_ok()
                    && in_worklist.insert((target, fp, lp))
                {
                    worklist.push_back((target, fp, lp));
                }
            }
        }

        report_progress(
            &mut on_progress,
            iterations,
            budget,
            initial_worklist_size,
            &mut last_reported,
        );
    }
    on_progress(budget, budget);
    block_of
}

/// The textbook refinement of spec 0062: every (block, letter) pair.
#[cfg(test)]
fn refine_textbook(setup: &Setup, mut on_progress: impl FnMut(u64, u64)) -> Vec<usize> {
    let mut block_of = setup.block_of.clone();
    let mut blocks = setup.blocks();

    // ── Alphabet Σ: all (field_number, symbol) pairs in the graph ────────────
    // Every edge enters some node, so the letters of the reverse adjacency
    // are the letters of the edges.
    let alphabet: Vec<Letter> = setup.letters_into(&(0..setup.n).collect::<Vec<_>>());
    let alphabet = &alphabet;

    // ── Worklist W: (block_id, field, label) ─────────────────────────────────
    // in_worklist mirrors the deque contents for O(1) membership queries.
    let mut worklist: VecDeque<(usize, u32, u8)> = VecDeque::new();
    let mut in_worklist: HashSet<(usize, u32, u8)> = HashSet::new();

    for b in 0..setup.num_blocks {
        for &(f, l) in alphabet {
            worklist.push_back((b, f, l));
            in_worklist.insert((b, f, l));
        }
    }

    let budget = progress_budget(&blocks);
    let initial_worklist_size = worklist.len() as u64;
    let mut iterations: u64 = 0;
    let mut last_reported: u64 = 0;
    on_progress(0, budget);

    // ── Refinement loop ───────────────────────────────────────────────────────
    while let Some((a_id, field, label)) = worklist.pop_front() {
        iterations += 1;
        in_worklist.remove(&(a_id, field, label));

        if a_id >= blocks.len() || blocks[a_id].is_empty() {
            continue;
        }

        // X = predecessors of block A via (field, label).
        let x: HashSet<usize> = blocks[a_id]
            .iter()
            .flat_map(|&di| {
                setup.rev[di]
                    .iter()
                    .filter(|(f, l, _)| *f == field && *l == label)
                    .map(|(_, _, si)| *si)
            })
            .collect();

        if x.is_empty() {
            continue;
        }

        for (y_id, y1_nodes) in group_by_block(&x, &block_of) {
            // Y₁ = Y ∩ X = y1_nodes; Y₂ = Y \ X = blocks[y_id] after removal.
            if y1_nodes.len() == blocks[y_id].len() {
                // Y ⊆ X: no split needed.
                continue;
            }
            let y1_id = split(&mut blocks, &mut block_of, y_id, &y1_nodes);
            let y2_id = y_id;
            let y1_len = y1_nodes.len();
            let y2_len = blocks[y2_id].len();

            // Update worklist for every symbol (f', l') per textbook rule.
            for &(fp, lp) in alphabet {
                if in_worklist.contains(&(y_id, fp, lp)) {
                    // (Y, f', l') ∈ W: replace Y with Y₁ and Y₂.
                    // Y₂ keeps the original y_id slot — its entry is already
                    // (y_id=y2_id, fp, lp) which remains in in_worklist.
                    // Just add Y₁.
                    if in_worklist.insert((y1_id, fp, lp)) {
                        worklist.push_back((y1_id, fp, lp));
                    }
                } else {
                    // (Y, f', l') ∉ W: add only the smaller of Y₁ and Y₂.
                    let smaller_id = if y1_len <= y2_len { y1_id } else { y2_id };
                    if in_worklist.insert((smaller_id, fp, lp)) {
                        worklist.push_back((smaller_id, fp, lp));
                    }
                }
            }
        }

        report_progress(
            &mut on_progress,
            iterations,
            budget,
            initial_worklist_size,
            &mut last_reported,
        );
    }
    on_progress(budget, budget);
    block_of
}

/// Renumber: non-leaf blocks first, by their first node in node order, then
/// leaf blocks. The result depends on the partition alone, not on the
/// refinement's internal block IDs (spec 0425 S4).
fn renumber(block_of: &[usize], msg_count: usize, num_leaves: usize) -> Partition {
    let leaf_block_ids: Vec<usize> = (0..num_leaves).map(|li| block_of[msg_count + li]).collect();
    let leaf_block_set: HashSet<usize> = leaf_block_ids.iter().copied().collect();

    let mut old_to_new: HashMap<usize, u32> = HashMap::new();
    let mut next_msg_id: u32 = 0;
    for &b in block_of {
        if !leaf_block_set.contains(&b) && !old_to_new.contains_key(&b) {
            old_to_new.insert(b, next_msg_id);
            next_msg_id += 1;
        }
    }
    let num_msg_blocks = next_msg_id as usize;
    for (slot, &old_leaf_id) in leaf_block_ids.iter().enumerate() {
        old_to_new
            .entry(old_leaf_id)
            .or_insert(num_msg_blocks as u32 + slot as u32);
    }

    let block_of_remapped: Vec<u32> = block_of.iter().map(|b| old_to_new[b]).collect();

    Partition {
        block_of: block_of_remapped,
        num_blocks: num_msg_blocks + num_leaves,
    }
}

fn leaf_sentinel_for_index(li: usize, _reg: &LeafRegistry) -> u32 {
    match li {
        0 => LEAF_UINT64,
        1 => LEAF_UINT32,
        2 => LEAF_INT32,
        3 => LEAF_I64,
        4 => LEAF_LEN,
        5 => LEAF_STRING,
        6 => LEAF_I32,
        i => u32::MAX - 7 - 1 - (i - NUM_FIXED_LEAVES) as u32,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    //! `minimize` against the textbook refinement (spec 0425 test plan 2).

    use std::fmt::Write as _;
    use std::path::Path;

    use super::super::{graph, load};
    use super::*;

    /// Minimize `yamls` both ways; panic if the partitions differ.
    fn assert_same_partition(yamls: &[String], what: &str) {
        let merged = load::merge_from_strings(yamls).unwrap_or_else(|e| panic!("{what}: {e}"));
        let (raw, reg) = graph::build(&merged);
        let sparse = minimize(&raw, &reg, &raw.node_wire_types, |_, _| {});
        let textbook = minimize_textbook(&raw, &reg, &raw.node_wire_types);
        assert_eq!(
            sparse.num_blocks, textbook.num_blocks,
            "{what}: block count"
        );
        assert_eq!(sparse.block_of, textbook.block_of, "{what}: block_of");
    }

    /// Every `*.yaml` under `dir`, recursively, sorted by path.
    fn yamls_under(dir: &Path) -> Vec<String> {
        let mut paths: Vec<_> = walkdir::WalkDir::new(dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "yaml"))
            .map(|e| e.into_path())
            .collect();
        paths.sort();
        paths
            .iter()
            .map(|p| std::fs::read_to_string(p).expect("read yaml"))
            .collect()
    }

    #[test]
    fn fixtures_minimize_as_the_textbook_does() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hopcroft");
        let mut cases: Vec<_> = std::fs::read_dir(&root)
            .expect("fixtures")
            .map(|e| e.expect("entry").path())
            .collect();
        cases.sort();
        assert!(!cases.is_empty());
        for case in cases {
            let yamls = yamls_under(&case.join("input"));
            assert_same_partition(&yamls, &case.display().to_string());
        }
        let scoring = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scoring");
        assert_same_partition(&yamls_under(&scoring), "fixtures/scoring");
    }

    /// xorshift64*: a seeded generator, so a failing case can be replayed.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    /// A random scoring graph in reproto's YAML, one message per file:
    /// few field numbers and types, so that many messages are equivalent
    /// and many are not; cycles, empty messages, groups, extension ranges.
    fn random_graph(rng: &mut Rng) -> Vec<String> {
        let count = 2 + rng.below(40) as usize;
        let mut files = Vec::with_capacity(count);
        for i in 0..count {
            let mut y = String::new();
            let entry = i == 0 || rng.below(4) == 0;
            if entry {
                writeln!(y, "entries: [M{i}]").unwrap();
            } else {
                writeln!(y, "entries: []").unwrap();
            }
            writeln!(y, "messages:\n  M{i}:").unwrap();
            if rng.below(8) == 0 {
                writeln!(y, "    kind: GROUP").unwrap();
            }
            let fields = rng.below(4);
            if fields == 0 {
                writeln!(y, "    fields: []").unwrap();
            } else {
                writeln!(y, "    fields:").unwrap();
            }
            let mut number = 0;
            for _ in 0..fields {
                number += 1 + rng.below(2) as u32;
                writeln!(y, "    - number: {number}").unwrap();
                let label = ["optional", "repeated", "required"][rng.below(3) as usize];
                match rng.below(6) {
                    0..=2 => {
                        let child = rng.below(count as u64);
                        writeln!(y, "      type: message\n      child: M{child}").unwrap();
                    }
                    3 => writeln!(y, "      type: string").unwrap(),
                    4 => {
                        writeln!(y, "      type: int32").unwrap();
                        if label == "repeated" && rng.below(2) == 0 {
                            writeln!(y, "      packed: true").unwrap();
                        }
                    }
                    _ => {
                        let max = rng.below(3);
                        writeln!(y, "      type: enum\n      range: [0, {max}]").unwrap();
                    }
                }
                writeln!(y, "      label: {label}").unwrap();
            }
            if rng.below(6) == 0 {
                writeln!(y, "    ext_ranges: [[100, 199]]").unwrap();
            }
            files.push(y);
        }
        files
    }

    #[test]
    fn random_graphs_minimize_as_the_textbook_does() {
        for seed in 1..=500u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
            let yamls = random_graph(&mut rng);
            assert_same_partition(&yamls, &format!("random graph, seed {seed}"));
        }
    }

    #[test]
    fn progress_keeps_its_contract() {
        let mut rng = Rng(42);
        let yamls = random_graph(&mut rng);
        let merged = load::merge_from_strings(&yamls).unwrap();
        let (raw, reg) = graph::build(&merged);
        let mut calls: Vec<(u64, u64)> = Vec::new();
        minimize(&raw, &reg, &raw.node_wire_types, |c, t| calls.push((c, t)));
        let budget = calls[0].1;
        assert_eq!(calls.first(), Some(&(0, budget)));
        assert_eq!(calls.last(), Some(&(budget, budget)));
        assert!(calls
            .windows(2)
            .all(|w| w[0].0 <= w[1].0 && w[1].1 == budget));
    }
}
