//! Layered graph layout (Sugiyama style), the engine behind flowcharts, state and class
//! diagrams: break cycles, assign ranks, order nodes within ranks to reduce crossings, then
//! place them. Subgraphs are kept together by border nodes that reserve space on each rank, the
//! way dagre does it.
//!
//! Every phase is a bounded loop over vectors: no recursion, no `while` without a counter, and
//! hard caps on the working set, so no document can make it hang or overflow the stack.

use super::common::Direction;
use super::scene::{pt, Pt, Rect};
use super::{Unsupported, MAX_CLUSTERS, MAX_EDGES, MAX_LAYOUT_NODES, MAX_NESTING, MAX_NODES, MAX_RANKS};

pub struct LNode {
    pub w: f32,
    pub h: f32,
    pub cluster: Option<usize>,
}

pub struct LEdge {
    pub from: usize,
    pub to: usize,
    /// Label box; zero when unlabelled.
    pub label_w: f32,
    pub label_h: f32,
    /// Minimum number of ranks the edge spans (1 normally; `---->` asks for more).
    pub min_len: u8,
}

pub struct LCluster {
    pub parent: Option<usize>,
    pub title_w: f32,
    pub title_h: f32,
}

pub struct LGraph {
    pub nodes: Vec<LNode>,
    pub edges: Vec<LEdge>,
    pub clusters: Vec<LCluster>,
    pub dir: Direction,
    /// Gap between neighbouring nodes in a rank.
    pub node_sep: f32,
    /// Gap between ranks.
    pub rank_sep: f32,
    /// Padding inside a cluster outline.
    pub cluster_pad: f32,
}

pub struct EdgeRoute {
    /// Centre-to-centre polyline through the dummy nodes; empty for a self-loop.
    pub points: Vec<Pt>,
    /// Where the label box is centred, when the edge has one.
    pub label: Option<Pt>,
}

pub struct LayoutOut {
    /// Node centres.
    pub pos: Vec<Pt>,
    pub routes: Vec<EdgeRoute>,
    /// One per cluster (zero-sized for empty clusters), title space included.
    pub cluster_rects: Vec<Rect>,
    /// Only the tests read the size; the emitters measure their own scene bounds.
    #[cfg_attr(not(test), allow(dead_code))]
    pub width: f32,
    #[cfg_attr(not(test), allow(dead_code))]
    pub height: f32,
}

const ORDER_SWEEPS: usize = 12;
const STALE_SWEEPS: usize = 3;
const TRANSPOSE_PASSES: usize = 4;
const X_SWEEPS: usize = 8;
const MAX_TRANSPOSE_PRODUCT: usize = 10_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Real(usize),
    Dummy,
    Label,
    BorderL(usize),
    BorderR(usize),
}

struct WNode {
    kind: Kind,
    rank: usize,
    w: f32,
    h: f32,
    /// Cluster chain, outermost first.
    path: Vec<usize>,
}

#[derive(Clone, Copy)]
struct WEdge {
    from: usize,
    to: usize,
    weight: f32,
}

pub fn layout(g: &LGraph) -> Result<LayoutOut, Unsupported> {
    let n = g.nodes.len();
    if n == 0 {
        return Err(Unsupported("no nodes"));
    }
    if n > MAX_NODES || g.edges.len() > MAX_EDGES || g.clusters.len() > MAX_CLUSTERS {
        return Err(Unsupported("graph too large"));
    }
    for e in &g.edges {
        if e.from >= n || e.to >= n {
            return Err(Unsupported("edge index"));
        }
    }
    for node in &g.nodes {
        if node.cluster.is_some_and(|c| c >= g.clusters.len()) {
            return Err(Unsupported("cluster index"));
        }
    }
    let horizontal = g.dir.horizontal();
    let sane = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0e6) } else { 0.0 };
    let node_sep = sane(g.node_sep).max(1.0);
    let rank_sep = sane(g.rank_sep).max(1.0);
    let pad = sane(g.cluster_pad);

    // Cluster paths (outermost first), validated against cycles and depth.
    let mut cluster_path: Vec<Vec<usize>> = Vec::with_capacity(g.clusters.len());
    for c in 0..g.clusters.len() {
        let mut chain = vec![c];
        let mut cur = g.clusters[c].parent;
        for _ in 0..=MAX_NESTING {
            match cur {
                Some(p) if p < g.clusters.len() => {
                    if chain.contains(&p) {
                        return Err(Unsupported("cluster cycle"));
                    }
                    chain.push(p);
                    cur = g.clusters[p].parent;
                }
                Some(_) => return Err(Unsupported("cluster parent index")),
                None => break,
            }
        }
        if cur.is_some() || chain.len() > MAX_NESTING {
            return Err(Unsupported("clusters nested too deep"));
        }
        chain.reverse();
        cluster_path.push(chain);
    }
    let node_path = |i: usize| -> Vec<usize> { g.nodes[i].cluster.and_then(|c| cluster_path.get(c).cloned()).unwrap_or_default() };

    // Sizes in the working (top-to-bottom) frame.
    let size_of = |w: f32, h: f32| if horizontal { (sane(h), sane(w)) } else { (sane(w), sane(h)) };
    let has_labels = g.edges.iter().any(|e| e.label_w > 0.0 && e.from != e.to);
    let len_of = |e: &LEdge| (e.min_len.max(1) as usize) * if has_labels { 2 } else { 1 };
    let rank_sep = if has_labels { rank_sep / 2.0 } else { rank_sep };

    // Phase 1: cycle removal by iterative DFS; back edges are flipped for layout only.
    let real_edges: Vec<usize> = (0..g.edges.len()).filter(|&e| g.edges[e].from != g.edges[e].to).collect();
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &e in &real_edges {
        out[g.edges[e].from].push(e);
    }
    let mut reversed = vec![false; g.edges.len()];
    let mut color = vec![0u8; n]; // 0 white, 1 gray, 2 black
    let mut order = Vec::with_capacity(n);
    let mut indeg = vec![0usize; n];
    for &e in &real_edges {
        indeg[g.edges[e].to] += 1;
    }
    let starts: Vec<usize> = (0..n).filter(|&v| indeg[v] == 0).chain(0..n).collect();
    for start in starts {
        if color[start] != 0 {
            continue;
        }
        color[start] = 1;
        order.push(start);
        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        while let Some(&mut (v, ref mut i)) = stack.last_mut() {
            if let Some(&e) = out[v].get(*i) {
                *i += 1;
                let w = g.edges[e].to;
                match color[w] {
                    0 => {
                        color[w] = 1;
                        order.push(w);
                        stack.push((w, 0));
                    }
                    1 => reversed[e] = true,
                    _ => {}
                }
            } else {
                color[v] = 2;
                stack.pop();
            }
        }
    }
    let ends = |e: usize| if reversed[e] { (g.edges[e].to, g.edges[e].from) } else { (g.edges[e].from, g.edges[e].to) };

    // Phase 2: ranking by longest path over a topological order, then pull sources down next
    // to their targets so a dangling source doesn't sit at the top with a long edge.
    //
    // Clusters rank as a whole: an edge into a cluster from outside pushes every member below
    // its source (and an edge out of one pulls the target below every member), through virtual
    // top/bottom nodes per cluster. If those constraints create a cycle (edges both into and out
    // of a cluster in a loop), ranking falls back to the plain graph.
    let paths: Vec<Vec<usize>> = (0..n).map(node_path).collect();
    let cluster_count = g.clusters.len();
    let rank_with = |with_clusters: bool| -> Option<Vec<usize>> {
        let total = n + if with_clusters { 2 * cluster_count } else { 0 };
        let top = |c: usize| n + c;
        let bottom = |c: usize| n + cluster_count + c;
        let mut aug_out: Vec<Vec<(usize, usize)>> = vec![Vec::new(); total];
        let mut aug_in: Vec<Vec<(usize, usize)>> = vec![Vec::new(); total];
        let mut add = |u: usize, v: usize, len: usize| {
            aug_out[u].push((v, len));
            aug_in[v].push((u, len));
        };
        for &e in &real_edges {
            let (u, v) = ends(e);
            let len = len_of(&g.edges[e]);
            add(u, v, len);
            if with_clusters {
                let (pu, pv) = (&paths[u], &paths[v]);
                let common = pu.iter().zip(pv.iter()).take_while(|(a, b)| a == b).count();
                if let Some(&c) = pv.get(common) {
                    add(u, top(c), len);
                }
                if let Some(&c) = pu.get(common) {
                    add(bottom(c), v, len);
                }
            }
        }
        if with_clusters {
            for (i, path) in paths.iter().enumerate() {
                for &c in path {
                    add(top(c), i, 0);
                    add(i, bottom(c), 0);
                }
            }
        }
        let mut indeg: Vec<usize> = (0..total).map(|v| aug_in[v].len()).collect();
        let mut queue: Vec<usize> = order.iter().copied().filter(|&v| indeg[v] == 0).collect();
        queue.extend((n..total).filter(|&v| indeg[v] == 0));
        let mut topo = Vec::with_capacity(total);
        let mut head = 0;
        while let Some(&v) = queue.get(head) {
            head += 1;
            topo.push(v);
            for &(w, _) in &aug_out[v] {
                indeg[w] -= 1;
                if indeg[w] == 0 {
                    queue.push(w);
                }
            }
        }
        if topo.len() != total {
            return None;
        }
        let mut rank = vec![0usize; total];
        for &v in &topo {
            for &(u, len) in &aug_in[v] {
                rank[v] = rank[v].max(rank[u] + len);
            }
        }
        for &v in topo.iter().rev() {
            if v < n && aug_in[v].is_empty() && !aug_out[v].is_empty() {
                let mut best = usize::MAX;
                for &(w, len) in &aug_out[v] {
                    best = best.min(rank[w].saturating_sub(len));
                }
                rank[v] = best;
            }
        }
        rank.truncate(n);
        Some(rank)
    };
    let mut rank = match rank_with(cluster_count > 0).or_else(|| rank_with(false)) {
        Some(r) => r,
        None => return Err(Unsupported("cycle survived")),
    };
    let min_rank = rank.iter().copied().min().unwrap_or(0);
    for r in &mut rank {
        *r -= min_rank;
    }
    let rank_count = rank.iter().copied().max().unwrap_or(0) + 1;
    if rank_count > MAX_RANKS {
        return Err(Unsupported("too many ranks"));
    }

    // Phase 3: working nodes: real ones, dummies along long edges, borders around clusters.
    let mut wn: Vec<WNode> = Vec::with_capacity(n);
    for (i, node) in g.nodes.iter().enumerate() {
        let (w, h) = size_of(node.w, node.h);
        wn.push(WNode { kind: Kind::Real(i), rank: rank[i], w, h, path: node_path(i) });
    }
    let mut we: Vec<WEdge> = Vec::new();
    let mut chains: Vec<Vec<usize>> = Vec::with_capacity(g.edges.len());
    for (e, edge) in g.edges.iter().enumerate() {
        if edge.from == edge.to {
            chains.push(Vec::new());
            continue;
        }
        let (u, v) = ends(e);
        let span = rank[v].saturating_sub(rank[u]);
        let (lw, lh) = size_of(edge.label_w, edge.label_h);
        let common: Vec<usize> = wn[u].path.iter().zip(&wn[v].path).take_while(|(a, b)| a == b).map(|(a, _)| *a).collect();
        let mut chain = vec![u];
        let mut prev = u;
        for k in 1..span {
            let r = rank[u] + k;
            let is_label = lw > 0.0 && k == span / 2;
            let (w, h) = if is_label { (lw, lh) } else { (0.0, 0.0) };
            wn.push(WNode { kind: if is_label { Kind::Label } else { Kind::Dummy }, rank: r, w, h, path: common.clone() });
            let d = wn.len() - 1;
            we.push(WEdge { from: prev, to: d, weight: 2.0 });
            chain.push(d);
            prev = d;
            if wn.len() > MAX_LAYOUT_NODES {
                return Err(Unsupported("too many layout nodes"));
            }
        }
        we.push(WEdge { from: prev, to: v, weight: if span > 1 { 2.0 } else { 1.0 } });
        chain.push(v);
        chains.push(chain);
    }
    // Cluster rank spans over real members (nested clusters included through the path).
    let mut span: Vec<Option<(usize, usize)>> = vec![None; g.clusters.len()];
    for node in wn.iter().filter(|w| matches!(w.kind, Kind::Real(_))) {
        for &c in &node.path {
            span[c] = Some(span[c].map_or((node.rank, node.rank), |(lo, hi)| (lo.min(node.rank), hi.max(node.rank))));
        }
    }
    let title_h: Vec<f32> = g.clusters.iter().map(|c| sane(c.title_h)).collect();
    let title_w: Vec<f32> = g.clusters.iter().map(|c| sane(c.title_w)).collect();
    for c in 0..g.clusters.len() {
        let Some((lo, hi)) = span[c] else { continue };
        let left_w = pad + if horizontal { title_h[c] } else { 0.0 };
        let mut prev: Option<(usize, usize)> = None;
        for r in lo..=hi {
            wn.push(WNode { kind: Kind::BorderL(c), rank: r, w: left_w, h: 0.0, path: cluster_path[c].clone() });
            let l = wn.len() - 1;
            wn.push(WNode { kind: Kind::BorderR(c), rank: r, w: pad, h: 0.0, path: cluster_path[c].clone() });
            let rr = wn.len() - 1;
            if let Some((pl, pr)) = prev {
                we.push(WEdge { from: pl, to: l, weight: 4.0 });
                we.push(WEdge { from: pr, to: rr, weight: 4.0 });
            }
            prev = Some((l, rr));
            if wn.len() > MAX_LAYOUT_NODES {
                return Err(Unsupported("too many layout nodes"));
            }
        }
    }
    let m = wn.len();
    let mut up: Vec<Vec<(usize, f32)>> = vec![Vec::new(); m];
    let mut down: Vec<Vec<(usize, f32)>> = vec![Vec::new(); m];
    for e in &we {
        down[e.from].push((e.to, e.weight));
        up[e.to].push((e.from, e.weight));
    }

    // Phase 4: ordering. Seed from the DFS order, keep clusters contiguous through
    // lexicographic (cluster value…, own value) keys, then sweep barycenters.
    let mut seed = vec![0.0f32; m];
    let mut dfs_pos = vec![0usize; n];
    for (i, &v) in order.iter().enumerate() {
        dfs_pos[v] = i;
    }
    for (i, node) in wn.iter().enumerate() {
        seed[i] = match node.kind {
            Kind::Real(v) => dfs_pos[v] as f32,
            Kind::BorderL(_) => f32::NEG_INFINITY,
            Kind::BorderR(_) => f32::INFINITY,
            Kind::Dummy | Kind::Label => f32::NAN, // filled from the chain source below
        };
    }
    for chain in &chains {
        let Some(&src) = chain.first() else { continue };
        for &d in chain.iter().skip(1) {
            if seed[d].is_nan() {
                seed[d] = seed[src] + 0.5;
            }
        }
    }
    let mut ranks: Vec<Vec<usize>> = vec![Vec::new(); rank_count];
    for (i, node) in wn.iter().enumerate() {
        ranks[node.rank].push(i);
    }
    let mut pos = vec![0usize; m];
    let sort_rank =|rank: &mut Vec<usize>, value: &[f32], pos: &mut Vec<usize>, wn: &[WNode]| {
        // Cluster value: mean of its members' values on this rank.
        let mut sum = vec![0.0f32; cluster_count];
        let mut cnt = vec![0usize; cluster_count];
        for &i in rank.iter() {
            let v = value[i];
            if v.is_finite() {
                for &c in &wn[i].path {
                    sum[c] += v;
                    cnt[c] += 1;
                }
            }
        }
        let cval: Vec<f32> = (0..cluster_count).map(|c| if cnt[c] > 0 { sum[c] / cnt[c] as f32 } else { 0.0 }).collect();
        let key = |i: usize| -> Vec<f32> {
            let mut k: Vec<f32> = wn[i].path.iter().map(|&c| cval[c]).collect();
            k.push(value[i]);
            k
        };
        let mut keyed: Vec<(Vec<f32>, usize)> = rank.iter().map(|&i| (key(i), i)).collect();
        keyed.sort_by(|a, b| {
            let mut it = a.0.iter().zip(b.0.iter());
            loop {
                match it.next() {
                    Some((x, y)) => match x.total_cmp(y) {
                        std::cmp::Ordering::Equal => continue,
                        o => return o,
                    },
                    None => return a.0.len().cmp(&b.0.len()).then(pos[a.1].cmp(&pos[b.1])),
                }
            }
        });
        for (p, (_, i)) in keyed.iter().enumerate() {
            pos[*i] = p;
        }
        *rank = keyed.into_iter().map(|(_, i)| i).collect();
    };
    for r in 0..rank_count {
        for (p, &i) in ranks[r].iter().enumerate() {
            pos[i] = p;
        }
        let mut rank_nodes = std::mem::take(&mut ranks[r]);
        sort_rank(&mut rank_nodes, &seed, &mut pos, &wn);
        ranks[r] = rank_nodes;
    }

    let crossings = |ranks: &[Vec<usize>], pos: &[usize]| -> u64 {
        let mut total = 0u64;
        for r in 1..ranks.len() {
            let lower_len = ranks[r].len();
            if lower_len == 0 {
                continue;
            }
            let mut pairs: Vec<(usize, usize)> = Vec::new();
            for &u in &ranks[r - 1] {
                for &(v, _) in &down[u] {
                    pairs.push((pos[u], pos[v]));
                }
            }
            pairs.sort_unstable();
            let mut tree = vec![0u64; lower_len + 1];
            for (k, &(_, pl)) in pairs.iter().enumerate() {
                // Edges already inserted with a lower position greater than pl cross this one.
                let mut idx = pl + 1;
                let mut before = 0u64;
                while idx > 0 {
                    before += tree[idx];
                    idx &= idx - 1;
                }
                total += k as u64 - before;
                let mut idx = pl + 1;
                while idx <= lower_len {
                    tree[idx] += 1;
                    idx += idx & idx.wrapping_neg();
                }
            }
        }
        total
    };

    let mut best_pos = pos.clone();
    let mut best_ranks = ranks.clone();
    let mut best = crossings(&ranks, &pos);
    let mut stale = 0;
    let mut value = vec![0.0f32; m];
    for sweep in 0..ORDER_SWEEPS {
        if best == 0 {
            break;
        }
        let downward = sweep % 2 == 0;
        let rank_order: Vec<usize> = if downward { (1..rank_count).collect() } else { (0..rank_count.saturating_sub(1)).rev().collect() };
        for r in rank_order {
            let mut rank_nodes = std::mem::take(&mut ranks[r]);
            for &i in &rank_nodes {
                let neigh = if downward { &up[i] } else { &down[i] };
                let (mut s, mut wsum) = (0.0f32, 0.0f32);
                for &(j, wt) in neigh {
                    s += pos[j] as f32 * wt;
                    wsum += wt;
                }
                value[i] = match wn[i].kind {
                    Kind::BorderL(_) => f32::NEG_INFINITY,
                    Kind::BorderR(_) => f32::INFINITY,
                    _ if wsum > 0.0 => s / wsum,
                    _ => pos[i] as f32,
                };
            }
            sort_rank(&mut rank_nodes, &value, &mut pos, &wn);
            // Transpose: swap neighbours in the same cluster when it removes crossings.
            for _ in 0..TRANSPOSE_PASSES {
                let mut swapped = false;
                for k in 1..rank_nodes.len() {
                    let (a, b) = (rank_nodes[k - 1], rank_nodes[k]);
                    if wn[a].path != wn[b].path || !matches!(wn[a].kind, Kind::Real(_) | Kind::Dummy | Kind::Label) || !matches!(wn[b].kind, Kind::Real(_) | Kind::Dummy | Kind::Label) {
                        continue;
                    }
                    let product = (up[a].len() + down[a].len()) * (up[b].len() + down[b].len());
                    if product > MAX_TRANSPOSE_PRODUCT {
                        continue;
                    }
                    let count = |x: usize, y: usize| -> usize {
                        let mut c = 0;
                        for (nx, ny) in [(&up[x], &up[y]), (&down[x], &down[y])] {
                            for &(p, _) in nx {
                                for &(q, _) in ny {
                                    if pos[p] > pos[q] {
                                        c += 1;
                                    }
                                }
                            }
                        }
                        c
                    };
                    if count(b, a) < count(a, b) {
                        rank_nodes.swap(k - 1, k);
                        pos[a] = k;
                        pos[b] = k - 1;
                        swapped = true;
                    }
                }
                if !swapped {
                    break;
                }
            }
            ranks[r] = rank_nodes;
        }
        let c = crossings(&ranks, &pos);
        if c < best {
            best = c;
            best_pos = pos.clone();
            best_ranks = ranks.clone();
            stale = 0;
        } else {
            stale += 1;
            if stale >= STALE_SWEEPS {
                break;
            }
        }
    }
    let _ = best_pos;
    let ranks = best_ranks;

    // Phase 5: coordinates. Ranks stack vertically with room for cluster padding and titles;
    // within a rank, nodes move towards the median of their neighbours without ever crossing
    // or crowding the nodes beside them.
    let mut extra_before = vec![0.0f32; rank_count];
    let mut extra_after = vec![0.0f32; rank_count];
    for c in 0..cluster_count {
        if let Some((lo, hi)) = span[c] {
            extra_before[lo] += pad + if horizontal { 0.0 } else { title_h[c] };
            extra_after[hi] += pad;
        }
    }
    let mut rank_h = vec![0.0f32; rank_count];
    for (i, node) in wn.iter().enumerate() {
        let _ = i;
        rank_h[node.rank] = rank_h[node.rank].max(node.h);
    }
    let mut rank_y = vec![0.0f32; rank_count];
    let mut cursor = 0.0f32;
    for r in 0..rank_count {
        cursor += extra_before[r];
        rank_y[r] = cursor + rank_h[r] / 2.0;
        cursor += rank_h[r] + extra_after[r] + rank_sep;
    }
    let sep = |a: usize, b: usize| -> f32 {
        let real = |i: usize| matches!(wn[i].kind, Kind::Real(_));
        if real(a) && real(b) {
            node_sep
        } else {
            node_sep / 2.0
        }
    };
    let mut x = vec![0.0f32; m];
    for rank_nodes in &ranks {
        let mut cur = 0.0f32;
        for (k, &i) in rank_nodes.iter().enumerate() {
            if k > 0 {
                cur += sep(rank_nodes[k - 1], i);
            }
            x[i] = cur + wn[i].w / 2.0;
            cur += wn[i].w;
        }
    }
    let priority = |i: usize| -> usize {
        match wn[i].kind {
            Kind::BorderL(_) | Kind::BorderR(_) => 3,
            Kind::Dummy | Kind::Label => 2,
            Kind::Real(_) => 1,
        }
    };
    for sweep in 0..X_SWEEPS {
        let downward = sweep % 2 == 0;
        let rank_order: Vec<usize> = if downward { (1..rank_count).collect() } else { (0..rank_count.saturating_sub(1)).rev().collect() };
        for r in rank_order {
            let rank_nodes = &ranks[r];
            let mut by_priority: Vec<usize> = (0..rank_nodes.len()).collect();
            by_priority.sort_by_key(|&k| {
                let i = rank_nodes[k];
                let deg = if downward { up[i].len() } else { down[i].len() };
                (std::cmp::Reverse(priority(i)), std::cmp::Reverse(deg))
            });
            let mut xs: Vec<f32> = Vec::new();
            for k in by_priority {
                let i = rank_nodes[k];
                let neigh = if downward { &up[i] } else { &down[i] };
                if neigh.is_empty() {
                    continue;
                }
                xs.clear();
                for &(j, wt) in neigh {
                    let reps = wt.round().clamp(1.0, 4.0) as usize;
                    for _ in 0..reps {
                        xs.push(x[j]);
                    }
                }
                xs.sort_by(|a, b| a.total_cmp(b));
                let mid = xs.len() / 2;
                let target = if xs.len() % 2 == 1 { xs[mid] } else { (xs[mid - 1] + xs[mid]) / 2.0 };
                let lo = if k > 0 {
                    let l = rank_nodes[k - 1];
                    x[l] + wn[l].w / 2.0 + sep(l, i) + wn[i].w / 2.0
                } else {
                    f32::NEG_INFINITY
                };
                let hi = if k + 1 < rank_nodes.len() {
                    let rgt = rank_nodes[k + 1];
                    x[rgt] - wn[rgt].w / 2.0 - sep(i, rgt) - wn[i].w / 2.0
                } else {
                    f32::INFINITY
                };
                if lo <= hi {
                    x[i] = target.clamp(lo, hi);
                }
            }
        }
    }

    // Cluster rectangles, innermost first so parents can include their children.
    let mut by_depth: Vec<usize> = (0..cluster_count).collect();
    by_depth.sort_by_key(|&c| std::cmp::Reverse(cluster_path[c].len()));
    let compute_rects = |x: &[f32]| -> Vec<Option<Rect>> {
        let mut rects: Vec<Option<Rect>> = vec![None; cluster_count];
        for &c in &by_depth {
            let mut acc: Option<Rect> = None;
            for (i, node) in wn.iter().enumerate() {
                if node.path.last() == Some(&c) && !matches!(node.kind, Kind::BorderL(_) | Kind::BorderR(_)) {
                    let b = Rect::centered(pt(x[i], rank_y[node.rank]), node.w, node.h);
                    acc = Some(acc.map_or(b, |a| a.union(&b)));
                }
            }
            for (k, path) in cluster_path.iter().enumerate() {
                if path.len() >= 2 && path[path.len() - 2] == c {
                    if let Some(child) = rects[k] {
                        acc = Some(acc.map_or(child, |a| a.union(&child)));
                    }
                }
            }
            rects[c] = acc.map(|r| {
                let top_extra = if horizontal { 0.0 } else { title_h[c] };
                let left_extra = if horizontal { title_h[c] } else { 0.0 };
                let mut rect = Rect { x: r.x - pad - left_extra, y: r.y - pad - top_extra, w: r.w + 2.0 * pad + left_extra, h: r.h + 2.0 * pad + top_extra };
                // A title wider than the members would stick out of a top-titled box; widen it
                // around the members (the push passes below move neighbours out of the way).
                // In the horizontal frame the title runs along the rank axis, which is not pushed.
                if !horizontal && rect.w < title_w[c] + 2.0 * pad {
                    let grow = title_w[c] + 2.0 * pad - rect.w;
                    rect.x -= grow / 2.0;
                    rect.w += grow;
                }
                rect
            });
        }
        rects
    };
    // Border nodes keep outsiders away from a cluster's members on each rank, but the cluster
    // rectangle is as wide as its widest rank, so a node beside a narrower rank can still sit
    // inside it. Push such nodes (and everything beyond them) outward; nesting needs a pass per
    // level, bounded by the depth cap.
    let mut rects = compute_rects(&x);
    let gap = node_sep / 2.0;
    for _ in 0..MAX_NESTING {
        let mut moved = false;
        for &c in &by_depth {
            let (Some(rect), Some((lo, hi))) = (rects[c], span[c]) else { continue };
            for r in lo..=hi {
                let rank_nodes = &ranks[r];
                let inside = |i: usize| wn[i].path.contains(&c);
                let Some(first_in) = rank_nodes.iter().position(|&i| inside(i)) else { continue };
                let Some(last_in) = rank_nodes.iter().rposition(|&i| inside(i)) else { continue };
                if let Some(&left) = first_in.checked_sub(1).and_then(|k| rank_nodes.get(k)) {
                    let right_edge = x[left] + wn[left].w / 2.0;
                    if right_edge + gap > rect.x {
                        let shift = right_edge + gap - rect.x;
                        for &i in &rank_nodes[..first_in] {
                            x[i] -= shift;
                        }
                        moved = true;
                    }
                }
                if let Some(&right) = rank_nodes.get(last_in + 1) {
                    let left_edge = x[right] - wn[right].w / 2.0;
                    if left_edge - gap < rect.right() {
                        let shift = rect.right() + gap - left_edge;
                        for &i in &rank_nodes[last_in + 1..] {
                            x[i] += shift;
                        }
                        moved = true;
                    }
                }
            }
        }
        if !moved {
            break;
        }
        rects = compute_rects(&x);
    }
    let node_box = |i: usize| Rect::centered(pt(x[i], rank_y[wn[i].rank]), wn[i].w, wn[i].h);

    // Normalise to the origin, then map the working frame onto the requested direction.
    let mut bounds: Option<Rect> = None;
    for i in 0..m {
        let b = node_box(i);
        bounds = Some(bounds.map_or(b, |a| a.union(&b)));
    }
    for r in rects.iter().flatten() {
        bounds = Some(bounds.map_or(*r, |a| a.union(r)));
    }
    let b = bounds.unwrap_or_default();
    let (ox, oy) = (b.x, b.y);
    let (ww, wh) = (b.w.max(1.0), b.h.max(1.0));
    let map = |p: Pt| -> Pt {
        let p = pt(p.x - ox, p.y - oy);
        match g.dir {
            Direction::TB => p,
            Direction::BT => pt(p.x, wh - p.y),
            Direction::LR => pt(p.y, p.x),
            Direction::RL => pt(wh - p.y, p.x),
        }
    };
    let map_rect = |r: Rect| -> Rect {
        let a = map(pt(r.x, r.y));
        let b = map(pt(r.right(), r.bottom()));
        Rect { x: a.x.min(b.x), y: a.y.min(b.y), w: (a.x - b.x).abs(), h: (a.y - b.y).abs() }
    };
    let center = |i: usize| map(pt(x[i], rank_y[wn[i].rank]));
    let pos_out: Vec<Pt> = (0..n).map(center).collect();
    let routes: Vec<EdgeRoute> = chains
        .iter()
        .enumerate()
        .map(|(e, chain)| {
            let mut points: Vec<Pt> = chain.iter().map(|&i| center(i)).collect();
            if reversed[e] {
                points.reverse();
            }
            let label = chain.iter().find(|&&i| wn[i].kind == Kind::Label).map(|&i| center(i));
            EdgeRoute { points, label }
        })
        .collect();
    let cluster_rects: Vec<Rect> = rects.iter().map(|r| r.map(map_rect).unwrap_or_default()).collect();
    let (width, height) = if horizontal { (wh, ww) } else { (ww, wh) };
    Ok(LayoutOut { pos: pos_out, routes, cluster_rects, width, height })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(n: usize, edges: &[(usize, usize)], dir: Direction) -> LGraph {
        LGraph {
            nodes: (0..n).map(|_| LNode { w: 40.0, h: 20.0, cluster: None }).collect(),
            edges: edges.iter().map(|&(a, b)| LEdge { from: a, to: b, label_w: 0.0, label_h: 0.0, min_len: 1 }).collect(),
            clusters: Vec::new(),
            dir,
            node_sep: 10.0,
            rank_sep: 30.0,
            cluster_pad: 5.0,
        }
    }

    fn boxes(g: &LGraph, out: &LayoutOut) -> Vec<Rect> {
        g.nodes.iter().zip(&out.pos).map(|(n, p)| Rect::centered(*p, n.w, n.h)).collect()
    }

    fn check(g: &LGraph, out: &LayoutOut) {
        let bx = boxes(g, out);
        for (i, a) in bx.iter().enumerate() {
            assert!(a.x >= -0.5 && a.y >= -0.5 && a.right() <= out.width + 0.5 && a.bottom() <= out.height + 0.5, "node {i} {a:?} outside {}x{}", out.width, out.height);
            for (j, b) in bx.iter().enumerate().skip(i + 1) {
                let overlap = a.x < b.right() - 0.01 && b.x < a.right() - 0.01 && a.y < b.bottom() - 0.01 && b.y < a.bottom() - 0.01;
                assert!(!overlap, "nodes {i} and {j} overlap: {a:?} {b:?}");
            }
        }
        for (e, route) in out.routes.iter().enumerate() {
            let edge = &g.edges[e];
            if edge.from == edge.to {
                assert!(route.points.is_empty());
                continue;
            }
            assert_eq!(route.points.first(), Some(&out.pos[edge.from]), "edge {e} starts at its source");
            assert_eq!(route.points.last(), Some(&out.pos[edge.to]), "edge {e} ends at its target");
            for p in &route.points {
                assert!(p.x.is_finite() && p.y.is_finite());
            }
        }
        for (c, r) in out.cluster_rects.iter().enumerate() {
            for (i, n) in g.nodes.iter().enumerate() {
                let inside = {
                    let mut cur = n.cluster;
                    let mut found = false;
                    for _ in 0..MAX_NESTING {
                        match cur {
                            Some(k) if k == c => {
                                found = true;
                                break;
                            }
                            Some(k) => cur = g.clusters[k].parent,
                            None => break,
                        }
                    }
                    found
                };
                let b = bx[i];
                let contained = b.x >= r.x - 0.5 && b.y >= r.y - 0.5 && b.right() <= r.right() + 0.5 && b.bottom() <= r.bottom() + 0.5;
                let disjoint = b.right() <= r.x + 0.5 || b.x >= r.right() - 0.5 || b.bottom() <= r.y + 0.5 || b.y >= r.bottom() - 0.5;
                if inside {
                    assert!(contained, "node {i} should be inside cluster {c}: {b:?} vs {r:?}");
                } else {
                    assert!(disjoint, "node {i} should be outside cluster {c}: {b:?} vs {r:?}");
                }
            }
        }
    }

    #[test]
    fn chain_and_diamond_lay_out_in_every_direction() {
        for dir in [Direction::TB, Direction::BT, Direction::LR, Direction::RL] {
            let g = graph(4, &[(0, 1), (1, 2), (2, 3)], dir);
            let out = layout(&g).unwrap();
            check(&g, &out);
            let p = &out.pos;
            match dir {
                Direction::TB => assert!(p[0].y < p[1].y && p[1].y < p[2].y && p[2].y < p[3].y, "{p:?}"),
                Direction::BT => assert!(p[0].y > p[1].y && p[1].y > p[2].y, "{p:?}"),
                Direction::LR => assert!(p[0].x < p[1].x && p[1].x < p[2].x, "{p:?}"),
                Direction::RL => assert!(p[0].x > p[1].x && p[1].x > p[2].x, "{p:?}"),
            }
            let g = graph(4, &[(0, 1), (0, 2), (1, 3), (2, 3)], dir);
            let out = layout(&g).unwrap();
            check(&g, &out);
        }
    }

    #[test]
    fn cycles_self_loops_and_long_edges_are_fine() {
        let g = graph(5, &[(0, 1), (1, 2), (2, 0), (3, 3), (0, 4), (4, 4), (1, 4)], Direction::TB);
        let out = layout(&g).unwrap();
        check(&g, &out);
        assert!(out.routes[3].points.is_empty() && out.routes[5].points.is_empty());
        // Edge 2 (2 -> 0) was reversed for layout but is reported from 2 to 0.
        assert_eq!(out.routes[2].points.first(), Some(&out.pos[2]));
        // A long edge bends through dummies: 0 -> 4 spans more than one rank or is direct.
        assert!(out.routes[4].points.len() >= 2);
        // Labelled edges double rank lengths and get a label point.
        let mut g = graph(2, &[(0, 1)], Direction::TB);
        g.edges[0].label_w = 30.0;
        g.edges[0].label_h = 10.0;
        let out = layout(&g).unwrap();
        check(&g, &out);
        assert!(out.routes[0].label.is_some());
        assert_eq!(out.routes[0].points.len(), 3);
    }

    #[test]
    fn clusters_contain_their_members_and_nothing_else() {
        let mut g = graph(7, &[(0, 1), (1, 2), (1, 3), (2, 4), (3, 4), (4, 5), (0, 6), (6, 5)], Direction::TB);
        g.clusters = vec![LCluster { parent: None, title_w: 30.0, title_h: 12.0 }, LCluster { parent: Some(0), title_w: 20.0, title_h: 12.0 }];
        for i in [1, 2, 3, 4] {
            g.nodes[i].cluster = Some(0);
        }
        g.nodes[2].cluster = Some(1);
        g.nodes[3].cluster = Some(1);
        for dir in [Direction::TB, Direction::LR, Direction::BT, Direction::RL] {
            g.dir = dir;
            let out = layout(&g).unwrap();
            check(&g, &out);
            let (outer, inner) = (out.cluster_rects[0], out.cluster_rects[1]);
            assert!(inner.x >= outer.x && inner.y >= outer.y && inner.right() <= outer.right() && inner.bottom() <= outer.bottom(), "{dir:?}: {inner:?} in {outer:?}");
        }
        // Empty clusters get an empty rectangle, bad parents are rejected.
        g.clusters.push(LCluster { parent: None, title_w: 0.0, title_h: 0.0 });
        let out = layout(&g).unwrap();
        assert_eq!(out.cluster_rects[2].w, 0.0);
        g.clusters[0].parent = Some(1);
        assert!(layout(&g).is_err(), "cluster cycle");
        g.clusters[0].parent = Some(99);
        assert!(layout(&g).is_err(), "bad parent");
    }

    #[test]
    fn rejects_bad_input_and_survives_big_graphs() {
        assert!(layout(&graph(0, &[], Direction::TB)).is_err());
        assert!(layout(&graph(2, &[(0, 5)], Direction::TB)).is_err());
        assert!(layout(&graph(MAX_NODES + 1, &[], Direction::TB)).is_err());
        let mut g = graph(3, &[(0, 1)], Direction::TB);
        g.nodes[0].w = f32::NAN;
        g.nodes[1].h = f32::INFINITY;
        let out = layout(&g).unwrap();
        assert!(out.width.is_finite() && out.height.is_finite());

        let started = std::time::Instant::now();
        let chain: Vec<(usize, usize)> = (0..1999).map(|i| (i, i + 1)).collect();
        let g = graph(2000, &chain, Direction::LR);
        let out = layout(&g).unwrap();
        check(&g, &out);
        let mut seed = 0x9E37_79B9u64;
        let mut rnd = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        // 200 layers of 10 nodes, edges one or two layers down (what real charts look like).
        let edges: Vec<(usize, usize)> = (0..4000)
            .map(|_| {
                let a = rnd(2000);
                let b = ((a / 10 + 1 + rnd(2)) * 10 + rnd(10)).min(1999);
                (a, b)
            })
            .collect();
        let g = graph(2000, &edges, Direction::TB);
        let out = layout(&g).unwrap();
        check(&g, &out);
        let star: Vec<(usize, usize)> = (1..2000).map(|i| (0, i)).collect();
        let g = graph(2000, &star, Direction::TB);
        check(&g, &layout(&g).unwrap());
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "layouts too slow: {:?}", started.elapsed());
        // Dummy explosion: every node linked to the far end makes rank spans huge.
        let far: Vec<(usize, usize)> = (0..1999).map(|i| (i, i + 1)).chain((0..1990).map(|i| (i, 1999))).collect();
        let g = graph(2000, &far, Direction::TB);
        assert!(layout(&g).is_err());
    }
}
