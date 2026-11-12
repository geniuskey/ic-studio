//! Maze routing of flylines: connects the device terminals of every schematic net that are not
//! yet connected in the layout, on the technology's metal stack.
//!
//! - Layers: the chain of routing layers joined by vias, without poly (metal1, metal2, ... in the
//!   generic technology).
//! - One wire width for every layer (the largest via plate or minimum width) and one pitch
//!   (width + the largest spacing). Grid lines run through the device terminals first (source /
//!   drain columns, gate rows), and the gaps are filled with lines at least one pitch apart, so
//!   shapes on different nodes never come closer than the spacing.
//! - A node or the wire between two neighbouring nodes is free, owned by one net (it touches only
//!   that net's geometry, within spacing) or blocked. Precomputed from the extracted layout.
//! - Nets are routed shortest first. Each net grows a tree from one extracted piece: an A* search
//!   from the tree to the nearest other piece, repeated until all pieces are joined. Metal1 costs
//!   more (it is the device layer); higher layers alternate horizontal / vertical preference.
//! - Rip-up and reroute: a connection that finds no path searches again across other nets' wires
//!   at a penalty; the nets it crosses are ripped up, the connection is made, and the ripped nets
//!   are queued to be routed again. A net is ripped up at most [`MAX_RIPS`] times, and the penalty
//!   grows with its rip-ups, so the process ends. Incomplete nets get one more attempt at the end.
//! - Wires are drawn as width x width squares on the nodes joined by bars, vias as single cuts.
//!   A connection that still finds no path is reported and left as a flyline.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet, VecDeque};

use rstar::{AABB, RTree, RTreeObject};

use crate::extract::{self, Extraction};
use crate::geom::IRect;
use crate::hier::{LayoutSource, flatten};
use crate::layout::Layout;
use crate::region::Region;
use crate::schematic::Schematic;
use crate::sdl;
use crate::tech::Tech;

const FREE: u32 = 0;
const BLOCKED: u32 = u32::MAX;
const NONE: u32 = u32::MAX;
/// Grid size limit (nodes over all layers).
pub const MAX_NODES: usize = 4_000_000;
/// Empty tracks around the design.
const MARGIN_TRACKS: i64 = 3;
const M1_COST: u64 = 4;
const WRONG_WAY_COST: u64 = 3;

/// Rip-ups allowed per net.
pub const MAX_RIPS: u32 = 3;
/// Cost of crossing another net's wire while looking for nets to rip up, in pitches per node
/// (times the victim's rip-ups + 1).
const RIP_PENALTY_PITCHES: u64 = 20;

type Wires = HashMap<usize, Vec<IRect>>;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RouteReport {
    /// Schematic nets whose terminals are all connected now.
    pub routed: Vec<String>,
    /// Nets left (partly) unconnected, with the reason.
    pub failed: Vec<String>,
    /// Connections made (one per joined piece).
    pub connections: usize,
    /// Total wire length in dbu.
    pub wire_length: i64,
    pub vias: usize,
    /// Wires ripped up to make room for other nets (and routed again).
    pub ripups: usize,
}

impl RouteReport {
    pub fn summary(&self) -> String {
        let mut s = format!("routed {} net(s) with {} connection(s), {} via(s)", self.routed.len(), self.connections, self.vias);
        if self.ripups > 0 {
            s += &format!(", {} rip-up(s)", self.ripups);
        }
        if !self.failed.is_empty() {
            s += &format!("; unrouted: {}", self.failed.join("; "));
        }
        s
    }
}

/// The routing layers, their via cuts and the common wire width / pitch (dbu).
struct Stack {
    layers: Vec<usize>,
    spacing: Vec<i64>,
    /// Cut layer and size between `layers[k]` and `layers[k + 1]`.
    cuts: Vec<(usize, i64)>,
    width: i64,
    pitch: i64,
}

fn snap(v: i64, g: i64) -> i64 {
    (v + g.div_euclid(2)).div_euclid(g) * g
}

fn snap_up(v: i64, g: i64) -> i64 {
    (v + g - 1).div_euclid(g) * g
}

fn stack(tech: &Tech) -> Result<Stack, String> {
    let lid = |n: &str| tech.layer_index(n);
    let usable = |l: usize| tech.layer(l).is_some_and(|d| d.routing && !d.name.eq_ignore_ascii_case("poly"));
    let mut up: BTreeMap<usize, (usize, usize, &crate::tech::ViaDef)> = BTreeMap::new();
    let mut tops = HashSet::new();
    for v in &tech.vias {
        let (Some(b), Some(c), Some(t)) = (lid(&v.bottom), lid(&v.cut), lid(&v.top)) else { continue };
        if usable(b) && usable(t) && b != t {
            up.entry(b).or_insert((t, c, v));
            tops.insert(t);
        }
    }
    let start = up.keys().copied().find(|b| !tops.contains(b)).ok_or("the technology has no via between two routing layers")?;
    let g = tech.grid.max(1);
    let min_w = |l: usize| tech.layer(l).map_or(0, |d| d.min_width);
    let (mut layers, mut cuts) = (vec![start], Vec::new());
    let mut width = min_w(start);
    let mut cut_pitch = 0;
    while let Some(&(t, c, v)) = up.get(layers.last().expect("start layer")) {
        if layers.contains(&t) {
            break;
        }
        cuts.push((c, v.cut_size));
        width = width.max(min_w(t)).max(v.cut_size + 2 * v.enc_bottom.max(v.enc_top));
        cut_pitch = cut_pitch.max(v.cut_size + v.cut_spacing);
        layers.push(t);
    }
    let spacing: Vec<i64> = layers.iter().map(|&l| tech.layer(l).map_or(0, |d| d.min_spacing)).collect();
    let width = snap_up(width.max(g), 2 * g);
    let pitch = snap_up((width + spacing.iter().copied().max().unwrap_or(0)).max(cut_pitch), g);
    Ok(Stack { layers, spacing, cuts, width, pitch })
}

/// Grid lines covering `lo..=hi`: every `pins` coordinate, plus filler lines at least `pitch`
/// from their neighbours.
fn lines(pins: &[i64], lo: i64, hi: i64, pitch: i64, g: i64) -> Vec<i64> {
    let mut base: Vec<i64> = pins.iter().chain([&lo, &hi]).map(|&v| snap(v, g)).collect();
    base.sort_unstable();
    base.dedup();
    let mut out = Vec::with_capacity(base.len() * 2);
    for w in base.windows(2) {
        out.push(w[0]);
        let gap = w[1] - w[0];
        let k = gap / (pitch + g) - 1;
        let step = gap as f64 / (k + 1) as f64;
        for i in 1..=k {
            out.push(snap(w[0] + (i as f64 * step).round() as i64, g));
        }
    }
    out.extend(base.last());
    out
}

struct Item {
    rect: IRect,
    net: usize,
}

impl RTreeObject for Item {
    type Envelope = AABB<[i64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners([self.rect.x0, self.rect.y0], [self.rect.x1, self.rect.y1])
    }
}

fn grow(r: IRect, d: i64) -> IRect {
    IRect::new(r.x0 - d, r.y0 - d, r.x1 + d, r.y1 + d)
}

/// State of a wire shape: free, owned by the only net it touches (`net + 1`), or blocked when it
/// would come within `space` of another net or of its own net without touching it.
fn classify(tree: &RTree<Item>, shape: IRect, space: i64) -> u32 {
    let e = grow(shape, space);
    let (mut owner, mut touches) = (None, false);
    for it in tree.locate_in_envelope_intersecting(AABB::from_corners([e.x0, e.y0], [e.x1, e.y1])) {
        if !it.rect.intersects(&e) {
            continue;
        }
        match owner {
            None => owner = Some(it.net),
            Some(o) if o != it.net => return BLOCKED,
            _ => {}
        }
        touches |= it.rect.intersects(&shape);
    }
    match owner {
        None => FREE,
        Some(n) if touches => n as u32 + 1,
        Some(_) => BLOCKED,
    }
}

#[derive(Clone)]
struct Grid {
    xs: Vec<i64>,
    ys: Vec<i64>,
    nl: usize,
    node: Vec<u32>,
    /// Wire from (ix, iy) to (ix + 1, iy) / (ix, iy + 1).
    right: Vec<u32>,
    up: Vec<u32>,
}

impl Grid {
    fn idx(&self, k: usize, ix: usize, iy: usize) -> usize {
        (k * self.ys.len() + iy) * self.xs.len() + ix
    }

    fn pos(&self, i: usize) -> (usize, usize, usize) {
        let (nx, ny) = (self.xs.len(), self.ys.len());
        (i / (nx * ny), i / nx % ny, i % nx)
    }
}

/// Nodes on the same layer whose squares would come closer than the spacing to node `i`
/// (centres less than `reach` apart), without `i`.
fn near(g: &Grid, i: usize, reach: i64) -> Vec<usize> {
    let (k, iy, ix) = g.pos(i);
    let (x, y) = (g.xs[ix], g.ys[iy]);
    let span = |v: &[i64], c: usize, at: i64| {
        let lo = v[..c].iter().rposition(|&p| at - p >= reach).map_or(0, |p| p + 1);
        let hi = v[c..].iter().position(|&p| p - at >= reach).map_or(v.len(), |p| c + p);
        lo..hi
    };
    let (xr, yr) = (span(&g.xs, ix, x), span(&g.ys, iy, y));
    yr.flat_map(|jy| xr.clone().map(move |jx| (jx, jy))).map(|(jx, jy)| g.idx(k, jx, jy)).filter(|&j| j != i).collect()
}

/// Layout net owning a static node or wire state.
fn owner(s: u32) -> Option<usize> {
    (s != FREE && s != BLOCKED).then(|| s as usize - 1)
}

/// A schematic net to connect: its extracted pieces (`groups`) and terminals off conductors.
struct Job {
    span: i64,
    net: String,
    groups: Vec<usize>,
    floating: usize,
}

/// Routing state over the static grid: the committed wires of every job, removable job by job.
struct Router<'a> {
    g: &'a Grid,
    st: &'a Stack,
    jobs: &'a [Job],
    /// Group index of each extracted net, per job.
    group_of: Vec<HashMap<usize, usize>>,
    /// Job + 1 of the wire on each node (0 = none).
    occupant: Vec<u32>,
    /// Jobs whose wires come within spacing of a node.
    blockers: HashMap<usize, Vec<u32>>,
    paths: Vec<Vec<Vec<usize>>>,
    connected: Vec<Vec<bool>>,
    rips: Vec<u32>,
    max_rips: u32,
    ripups: usize,
}

/// A node or wire usable by a job: free, or owned by one of its extracted pieces.
fn usable(group_of: &HashMap<usize, usize>, s: u32) -> bool {
    s == FREE || owner(s).is_some_and(|n| group_of.contains_key(&n))
}

impl Router<'_> {
    /// Other jobs whose wires sit on or near node `i`.
    fn conflicts(&self, j: usize, i: usize) -> impl Iterator<Item = usize> + '_ {
        let occ = self.occupant[i].checked_sub(1).map(|o| o as usize).filter(|&o| o != j);
        let near = self.blockers.get(&i).into_iter().flatten().map(|&b| b as usize).filter(move |&b| b != j);
        occ.into_iter().chain(near)
    }

    /// Shortest path (target first) from job `j`'s tree to one of its unconnected pieces, and the
    /// jobs it crosses. With `relaxed`, nodes of other jobs that may still be ripped up are
    /// allowed at a penalty.
    fn search(&self, j: usize, relaxed: bool) -> Option<(Vec<usize>, BTreeSet<usize>)> {
        let (g, groups, done) = (self.g, &self.group_of[j], &self.connected[j]);
        let target = |i: usize| owner(g.node[i]).and_then(|n| groups.get(&n)).is_some_and(|&k| !done[k]);
        let penalty = |i: usize| -> Option<u64> {
            let mut worst = None;
            for v in self.conflicts(j, i) {
                if !relaxed || self.rips[v] >= self.max_rips {
                    return None;
                }
                worst = worst.max(Some(self.rips[v]));
            }
            Some(worst.map_or(0, |r| RIP_PENALTY_PITCHES * (1 + r as u64) * self.st.pitch as u64))
        };
        let mut bb: Option<(i64, i64, i64, i64)> = None;
        let mut sources = Vec::new();
        for i in 0..g.node.len() {
            let tree =
                self.occupant[i] == j as u32 + 1 || owner(g.node[i]).and_then(|n| groups.get(&n)).is_some_and(|&k| done[k]);
            if tree {
                sources.push(i);
            } else if target(i) && penalty(i).is_some() {
                let (_, iy, ix) = g.pos(i);
                let (x, y) = (g.xs[ix], g.ys[iy]);
                bb = Some(bb.map_or((x, y, x, y), |(a, b, c, d)| (a.min(x), b.min(y), c.max(x), d.max(y))));
            }
        }
        let (bx0, by0, bx1, by1) = bb?;
        let h = |i: usize| {
            let (_, iy, ix) = g.pos(i);
            let (x, y) = (g.xs[ix], g.ys[iy]);
            ((bx0 - x).max(x - bx1).max(0) + (by0 - y).max(y - by1).max(0)) as u64
        };
        let n = g.node.len();
        let mut dist = vec![u64::MAX; n];
        let mut prev = vec![NONE; n];
        let mut heap = BinaryHeap::new();
        for &s in &sources {
            dist[s] = 0;
            heap.push(Reverse((h(s), s)));
        }
        let via_cost = 2 * self.st.pitch as u64;
        let (nx, ny) = (g.xs.len(), g.ys.len());
        while let Some(Reverse((f, i))) = heap.pop() {
            if f != dist[i] + h(i) {
                continue;
            }
            if target(i) {
                let mut path = vec![i];
                let mut c = i;
                while prev[c] != NONE {
                    c = prev[c] as usize;
                    path.push(c);
                }
                let crossed = path.iter().flat_map(|&p| self.conflicts(j, p)).collect();
                return Some((path, crossed));
            }
            let (k, iy, ix) = g.pos(i);
            let horiz = if k == 0 {
                M1_COST
            } else if k % 2 == 1 {
                1
            } else {
                WRONG_WAY_COST
            };
            let vert = if k == 0 {
                M1_COST
            } else if k % 2 == 0 {
                1
            } else {
                WRONG_WAY_COST
            };
            let mut step = |to: usize, cost: u64, wire: Option<u32>| {
                if !usable(groups, g.node[to]) || wire.is_some_and(|w| !usable(groups, w)) {
                    return;
                }
                let Some(extra) = penalty(to) else { return };
                let d = dist[i] + cost + extra;
                if d < dist[to] {
                    dist[to] = d;
                    prev[to] = i as u32;
                    heap.push(Reverse((d + h(to), to)));
                }
            };
            if ix + 1 < nx {
                step(g.idx(k, ix + 1, iy), (g.xs[ix + 1] - g.xs[ix]) as u64 * horiz, Some(g.right[i]));
            }
            if ix > 0 {
                let to = g.idx(k, ix - 1, iy);
                step(to, (g.xs[ix] - g.xs[ix - 1]) as u64 * horiz, Some(g.right[to]));
            }
            if iy + 1 < ny {
                step(g.idx(k, ix, iy + 1), (g.ys[iy + 1] - g.ys[iy]) as u64 * vert, Some(g.up[i]));
            }
            if iy > 0 {
                let to = g.idx(k, ix, iy - 1);
                step(to, (g.ys[iy] - g.ys[iy - 1]) as u64 * vert, Some(g.up[to]));
            }
            if k + 1 < g.nl {
                step(g.idx(k + 1, ix, iy), via_cost, None);
            }
            if k > 0 {
                step(g.idx(k - 1, ix, iy), via_cost, None);
            }
        }
        None
    }

    fn commit(&mut self, j: usize, path: Vec<usize>) {
        let piece = owner(self.g.node[path[0]]).and_then(|n| self.group_of[j].get(&n).copied()).expect("target piece");
        self.connected[j][piece] = true;
        for &i in &path {
            if self.occupant[i] == 0 {
                self.occupant[i] = j as u32 + 1;
            }
            let (k, _, _) = self.g.pos(i);
            for n in near(self.g, i, self.st.width + self.st.spacing[k]) {
                self.blockers.entry(n).or_default().push(j as u32);
            }
        }
        self.paths[j].push(path);
    }

    fn rip(&mut self, v: usize) {
        for path in std::mem::take(&mut self.paths[v]) {
            for i in path {
                if self.occupant[i] == v as u32 + 1 {
                    self.occupant[i] = 0;
                }
                let (k, _, _) = self.g.pos(i);
                for n in near(self.g, i, self.st.width + self.st.spacing[k]) {
                    if let Some(b) = self.blockers.get_mut(&n) {
                        if let Some(p) = b.iter().position(|&x| x == v as u32) {
                            b.swap_remove(p);
                        }
                        if b.is_empty() {
                            self.blockers.remove(&n);
                        }
                    }
                }
            }
        }
        self.connected[v].iter_mut().enumerate().for_each(|(k, c)| *c = k == 0);
        self.rips[v] += 1;
        self.ripups += 1;
    }

    /// Connect the remaining pieces of job `j`, ripping up other jobs when it is blocked.
    /// Returns the ripped jobs.
    fn route_job(&mut self, j: usize) -> Vec<usize> {
        let mut ripped = Vec::new();
        while self.connected[j].contains(&false) {
            if let Some((path, _)) = self.search(j, false) {
                self.commit(j, path);
                continue;
            }
            let Some((path, crossed)) = self.search(j, true) else { break };
            if crossed.is_empty() {
                break;
            }
            for v in crossed {
                self.rip(v);
                ripped.push(v);
            }
            self.commit(j, path);
        }
        ripped
    }
}

/// Route every job: shortest first, ripped-up jobs are queued again; jobs left incomplete get one
/// more attempt at the end.
fn run<'a>(grid: &'a Grid, st: &'a Stack, jobs: &'a [Job], max_rips: u32) -> Router<'a> {
    let mut r = Router {
        g: grid,
        st,
        jobs,
        group_of: jobs.iter().map(|j| j.groups.iter().enumerate().map(|(k, &n)| (n, k)).collect()).collect(),
        occupant: vec![0; grid.node.len()],
        blockers: HashMap::new(),
        paths: vec![Vec::new(); jobs.len()],
        connected: jobs.iter().map(|j| (0..j.groups.len()).map(|k| k == 0).collect()).collect(),
        rips: vec![0; jobs.len()],
        max_rips,
        ripups: 0,
    };
    let routable: Vec<usize> = (0..jobs.len()).filter(|&j| jobs[j].floating == 0 && jobs[j].groups.len() > 1).collect();
    let mut queue: VecDeque<usize> = routable.iter().copied().collect();
    for _ in 0..2 {
        let mut budget = routable.len() * (max_rips as usize + 2);
        while let Some(j) = queue.pop_front() {
            if budget == 0 {
                break;
            }
            budget -= 1;
            for v in r.route_job(j) {
                if !queue.contains(&v) {
                    queue.push_back(v);
                }
            }
        }
        queue = routable.iter().copied().filter(|&j| r.connected[j].contains(&false)).collect();
        if queue.is_empty() {
            break;
        }
    }
    r
}

impl Router<'_> {
    /// The report and the wire / via shapes per layer of the committed paths.
    fn finish(&self, g: i64) -> (RouteReport, Wires) {
        let (grid, st) = (self.g, self.st);
        let half = st.width / 2;
        let mut report = RouteReport { ripups: self.ripups, ..Default::default() };
        let mut wires: Wires = HashMap::new();
        for (j, job) in self.jobs.iter().enumerate() {
            if job.floating > 0 {
                report.failed.push(format!("{}: {} terminal(s) not on conducting geometry", job.net, job.floating));
                continue;
            }
            if job.groups.len() < 2 {
                continue;
            }
            for path in &self.paths[j] {
                report.connections += 1;
                for w in path.windows(2) {
                    let ((ka, ya, xa), (kb, yb, xb)) = (grid.pos(w[0]), grid.pos(w[1]));
                    let (pa, pb) = ((grid.xs[xa], grid.ys[ya]), (grid.xs[xb], grid.ys[yb]));
                    if ka == kb {
                        let bar = IRect::new(
                            pa.0.min(pb.0) - half,
                            pa.1.min(pb.1) - half,
                            pa.0.max(pb.0) + half,
                            pa.1.max(pb.1) + half,
                        );
                        wires.entry(st.layers[ka]).or_default().push(bar);
                        report.wire_length += (pa.0 - pb.0).abs() + (pa.1 - pb.1).abs();
                    } else {
                        let (cut, size) = st.cuts[ka.min(kb)];
                        let c0 = (snap(pa.0 - size / 2, g), snap(pa.1 - size / 2, g));
                        wires.entry(cut).or_default().push(IRect::new(c0.0, c0.1, c0.0 + size, c0.1 + size));
                        report.vias += 1;
                    }
                }
                for &i in path {
                    let (k, iy, ix) = grid.pos(i);
                    let (x, y) = (grid.xs[ix], grid.ys[iy]);
                    wires.entry(st.layers[k]).or_default().push(IRect::new(x - half, y - half, x + half, y + half));
                }
            }
            let missing = self.connected[j].iter().filter(|c| !**c).count();
            if missing == 0 {
                report.routed.push(job.net.clone());
            } else {
                report.failed.push(format!("{}: {missing} of {} pieces not reached", job.net, job.groups.len()));
            }
        }
        (report, wires)
    }
}

/// Route the flylines of `layout` (see [`sdl::ratsnest`]) and add the wires to it.
pub fn route(sch: &Schematic, layout: &mut Layout, src: &dyn LayoutSource, tech: &Tech) -> Result<RouteReport, String> {
    route_with(sch, layout, src, tech, MAX_RIPS)
}

/// [`route`] with at most `max_rips` rip-ups per net (0 = greedy, nothing is ripped up).
pub fn route_with(
    sch: &Schematic,
    layout: &mut Layout,
    src: &dyn LayoutSource,
    tech: &Tech,
    max_rips: u32,
) -> Result<RouteReport, String> {
    let st = stack(tech)?;
    let pins = sdl::terminal_pins(sch, layout, src);
    let Some(mut bb) = flatten(layout, src, 64).iter().map(|s| s.geom.bbox()).reduce(|a, b| a.union(&b)) else {
        return Ok(RouteReport::default());
    };
    let ex: Extraction = extract::extract(layout, src, tech);

    let (mut px, mut py) = (vec![], vec![]);
    for &(_, p) in pins.values().flatten() {
        bb = bb.union(&IRect::new(p.0, p.1, p.0, p.1));
        px.push(p.0);
        py.push(p.1);
    }
    let g = tech.grid.max(1);
    let margin = MARGIN_TRACKS * st.pitch;
    let xs = lines(&px, bb.x0 - margin, bb.x1 + margin, st.pitch, g);
    let ys = lines(&py, bb.y0 - margin, bb.y1 + margin, st.pitch, g);
    let nl = st.layers.len();
    let total = xs.len() * ys.len() * nl;
    if total > MAX_NODES {
        return Err(format!("the routing grid would have {total} nodes (limit {MAX_NODES})"));
    }
    let mut grid = Grid { xs, ys, nl, node: vec![FREE; total], right: vec![BLOCKED; total], up: vec![BLOCKED; total] };
    let half = st.width / 2;
    for (k, &l) in st.layers.iter().enumerate() {
        let items: Vec<Item> = ex
            .nets
            .iter()
            .enumerate()
            .flat_map(|(net, n)| {
                n.shapes
                    .iter()
                    .filter(|(sl, _)| *sl == l)
                    .flat_map(move |(_, r)| r.rects().iter().map(move |&rect| Item { rect, net }))
            })
            .collect();
        let tree = RTree::bulk_load(items);
        let s = st.spacing[k];
        for iy in 0..grid.ys.len() {
            for ix in 0..grid.xs.len() {
                let (x, y) = (grid.xs[ix], grid.ys[iy]);
                let i = grid.idx(k, ix, iy);
                grid.node[i] = classify(&tree, IRect::new(x - half, y - half, x + half, y + half), s);
                if let Some(&xn) = grid.xs.get(ix + 1) {
                    grid.right[i] = classify(&tree, IRect::new(x - half, y - half, xn + half, y + half), s);
                }
                if let Some(&yn) = grid.ys.get(iy + 1) {
                    grid.up[i] = classify(&tree, IRect::new(x - half, y - half, x + half, yn + half), s);
                }
            }
        }
    }

    let mut jobs: Vec<Job> = Vec::new();
    for (net, ps) in &pins {
        let mut groups: Vec<usize> = Vec::new();
        let mut floating = 0;
        for &(_, p) in ps {
            match sdl::pin_net(&ex, tech, p) {
                Some(n) if !groups.contains(&n) => groups.push(n),
                Some(_) => {}
                None => floating += 1,
            }
        }
        let (mut lo, mut hi) = ((i64::MAX, i64::MAX), (i64::MIN, i64::MIN));
        for &(_, p) in ps {
            (lo, hi) = ((lo.0.min(p.0), lo.1.min(p.1)), (hi.0.max(p.0), hi.1.max(p.1)));
        }
        jobs.push(Job { span: (hi.0 - lo.0) + (hi.1 - lo.1), net: net.clone(), groups, floating });
    }
    jobs.sort_by(|a, b| (a.span, &a.net).cmp(&(b.span, &b.net)));

    let (report, wires) = run(&grid, &st, &jobs, max_rips).finish(g);

    let mut layers: Vec<(usize, Vec<IRect>)> = wires.into_iter().collect();
    layers.sort_by_key(|(l, _)| *l);
    for (l, rects) in layers {
        for geom in Region::from_rects(rects).to_geometry() {
            layout.add(l, geom);
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drc::run_tech_deck;
    use crate::lvs::{LvsOptions, compare};
    use crate::project::Project;
    use crate::schematic::DeviceKind;

    fn place(project: &mut Project, sch: &Schematic) -> Layout {
        let mut layout = Layout::default();
        sdl::apply(project, &mut layout, &sdl::plan(sch, &project.tech)).unwrap();
        layout
    }

    fn check_routed(name: &str, project: &Project, sch: &Schematic, layout: &Layout, rep: &RouteReport) {
        assert!(rep.failed.is_empty(), "{name}: {rep:?}");
        let fl = sdl::ratsnest(sch, layout, project, &project.tech);
        assert!(fl.is_empty(), "{name}: flylines left {fl:?}");
        let v = run_tech_deck(layout, project, &project.tech).unwrap().violations;
        assert!(v.is_empty(), "{name}: {:?}", v.iter().map(|v| (&v.rule, v.marker)).collect::<Vec<_>>());
        let opts = LvsOptions {
            ignore_kinds: vec![DeviceKind::VSource, DeviceKind::ISource, DeviceKind::Capacitor, DeviceKind::Resistor],
            ..Default::default()
        };
        let x = extract::extract(layout, project, &project.tech);
        let lvs = compare(sch, &x, &opts);
        assert!(lvs.matched, "{name}: {:?}", lvs.errors);
        let lvs = compare(sch, &x, &LvsOptions { ignore_bulk: false, ..opts });
        assert!(lvs.matched, "{name} with bulks: {:?}", lvs.errors);
    }

    #[test]
    fn stack_and_lines() {
        let st = stack(&Tech::generic()).unwrap();
        let t = Tech::generic();
        let names: Vec<&str> = st.layers.iter().map(|&l| t.layers[l].name.as_str()).collect();
        assert_eq!(names, ["metal1", "metal2", "metal3"]);
        assert_eq!((st.width, st.pitch), (380, 660));
        assert_eq!(st.cuts.len(), 2);

        let pins = [0, 720, 300, 2000, 355];
        let ls = lines(&pins, -1000, 5000, 660, 10);
        for v in [-1000, 0, 300, 360, 720, 2000, 5000] {
            assert!(ls.contains(&v), "{v} missing in {ls:?}");
        }
        assert!(ls.windows(2).all(|w| w[0] < w[1]), "{ls:?}");
        for (i, v) in ls.iter().enumerate().filter(|(_, v)| ![-1000, 0, 300, 360, 720, 2000, 5000].contains(*v)) {
            assert!(v - ls[i - 1] >= 660 && ls[i + 1] - v >= 660, "filler {v} in {ls:?}");
        }
        assert!(ls.len() > 9, "the 2000..5000 gap is filled: {ls:?}");

        let mut no_vias = Tech::generic();
        no_vias.vias.retain(|v| !v.name.starts_with("via"));
        assert!(stack(&no_vias).is_err());
    }

    #[test]
    fn routes_demo_circuits() {
        let mut project = Project::new("t");
        let mut cells = vec![crate::examples::cmos_inverter()];
        for f in [
            crate::examples::nand2,
            crate::examples::nor2,
            crate::examples::ring_osc5,
            crate::examples::current_mirror,
            crate::examples::ota5t,
        ] {
            cells.push(f(&mut project));
        }
        for cell in cells {
            let sch = cell.schematic.clone().unwrap();
            let mut layout = place(&mut project, &sch);
            assert!(!sdl::ratsnest(&sch, &layout, &project, &project.tech).is_empty());
            let rep = route(&sch, &mut layout, &project, &project.tech).unwrap();
            assert!(rep.connections > 0 && rep.vias > 0 && rep.wire_length > 0, "{}: {rep:?}", cell.name);
            check_routed(&cell.name, &project, &sch, &layout, &rep);
            let again = route(&sch, &mut layout, &project, &project.tech).unwrap();
            assert_eq!((again.connections, again.failed.len()), (0, 0), "{}: routing twice adds nothing", cell.name);
        }
    }

    #[test]
    fn detours_around_obstacles() {
        let mut project = Project::new("t");
        let sch = crate::examples::cmos_inverter().schematic.unwrap();
        let mut layout = place(&mut project, &sch);
        let bb = flatten(&layout, &project, 8).iter().map(|s| s.geom.bbox()).reduce(|a, b| a.union(&b)).unwrap();
        let (m3, text) = (project.tech.layer_index("metal3").unwrap(), project.tech.layer_index("text").unwrap());
        layout.add_rect(m3, bb.x0 - 5000, bb.y0 - 5000, bb.x1 + 5000, bb.y1 + 5000);
        layout.add(text, crate::layout::Geometry::Label { text: "shield".into(), at: (bb.x0 - 4000, bb.y0 - 4000), size: 200 });
        let rep = route(&sch, &mut layout, &project, &project.tech).unwrap();
        check_routed("shielded inverter", &project, &sch, &layout, &rep);
    }

    #[test]
    fn rips_up_blocking_wires() {
        let mut project = Project::new("t");
        let sch = crate::examples::ring_osc5(&mut project).schematic.unwrap();
        let mut two_layers = project.tech.clone();
        let m3 = two_layers.layer_index("metal3").unwrap();
        two_layers.layers[m3].routing = false;
        let mut shielded = place(&mut project, &sch);
        let bb = flatten(&shielded, &project, 8).iter().map(|s| s.geom.bbox()).reduce(|a, b| a.union(&b)).unwrap();
        shielded.add_rect(m3, bb.x0 - 5000, bb.y0 - 5000, bb.x1 + 5000, bb.y1 + 5000);
        for (name, base, tech) in
            [("metal1/2 only", place(&mut project, &sch), &two_layers), ("under a metal3 plate", shielded, &project.tech)]
        {
            let mut greedy = base.clone();
            assert!(!route_with(&sch, &mut greedy, &project, tech, 0).unwrap().failed.is_empty(), "{name}: greedy succeeds");
            let mut layout = base;
            let rep = route(&sch, &mut layout, &project, tech).unwrap();
            assert!(rep.ripups > 0 && rep.summary().contains("rip-up"), "{name}: {rep:?}");
            check_routed(name, &project, &sch, &layout, &rep);
        }
    }

    #[test]
    fn reports_unreachable_pieces() {
        let mut project = Project::new("t");
        let sch = crate::examples::cmos_inverter().schematic.unwrap();
        let mut layout = place(&mut project, &sch);
        // Fence the NMOS in with a ring on every routing layer.
        let nmos = sch.instances.iter().find(|i| i.kind == DeviceKind::Nmos).unwrap();
        let inst = layout.instances.iter().find(|i| i.name.as_deref() == Some(nmos.name.as_str())).unwrap();
        let b = inst.trans.apply_rect(project.cell(&inst.cell).unwrap().layout.as_ref().unwrap().shapes_bbox().unwrap());
        let (o, w) = (500, 400);
        for name in ["metal1", "metal2", "metal3"] {
            let l = project.tech.layer_index(name).unwrap();
            let (x0, y0, x1, y1) = (b.x0 - o - w, b.y0 - o - w, b.x1 + o + w, b.y1 + o + w);
            layout.add_rect(l, x0, y0, x1, y0 + w);
            layout.add_rect(l, x0, y1 - w, x1, y1);
            layout.add_rect(l, x0, y0, x0 + w, y1);
            layout.add_rect(l, x1 - w, y0, x1, y1);
        }
        let rep = route(&sch, &mut layout, &project, &project.tech).unwrap();
        assert!(rep.failed.iter().any(|f| f.contains("not reached")), "{rep:?}");
        assert!(rep.summary().contains("unrouted"), "{}", rep.summary());
        assert!(run_tech_deck(&layout, &project, &project.tech).unwrap().violations.is_empty());
    }
}
