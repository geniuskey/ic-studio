//! Manhattan region algebra: Boolean operations, sizing and polygon reconstruction.
//!
//! Regions are half-open point sets `[x0, x1) x [y0, y1)` stored as disjoint rectangles in a
//! canonical form: the maximal horizontal runs of every horizontal cross-section, each extended
//! vertically for as long as exactly that run persists, sorted by `(y0, x0)`. Equal point sets
//! therefore have identical rectangle lists, so `PartialEq` compares geometry.

use std::collections::{BTreeMap, HashMap};

use crate::geom::IRect;
use crate::layout::Geometry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoolOp {
    Or,
    And,
    /// `a` minus `b`.
    Not,
    Xor,
}

impl BoolOp {
    /// Bit `s` is set when state `s = in_a | in_b << 1` belongs to the result.
    fn mask(self) -> u8 {
        match self {
            BoolOp::Or => 0b1110,
            BoolOp::And => 0b1000,
            BoolOp::Not => 0b0010,
            BoolOp::Xor => 0b0110,
        }
    }
}

/// Components whose compressed grid exceeds this many cells are emitted as merged rectangles.
const MAX_GRID_CELLS: usize = 1 << 21;

/// A point set made of disjoint rectangles (canonical form: merged maximal horizontal strips).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Region {
    rects: Vec<IRect>,
}

impl Region {
    pub fn from_rects(rects: impl IntoIterator<Item = IRect>) -> Region {
        let rects: Vec<IRect> = rects.into_iter().collect();
        Region { rects: sweep(&rects, &[], BoolOp::Or.mask()) }
    }

    /// Rect, Manhattan polygon or Manhattan path. Labels give an empty region; non-Manhattan geometry is an error.
    ///
    /// Polygons use the nonzero winding rule (orientation does not matter; self-touching and
    /// self-overlapping outlines are filled as the union of their loops). Paths are the union of
    /// their segments, each extended by half the width at both ends (GDS pathtype 2).
    pub fn from_geometry(g: &Geometry) -> Result<Region, String> {
        match g {
            Geometry::Rect(r) => Ok(Region::from_rects([*r])),
            Geometry::Polygon { points } => Ok(Region::from_rects(polygon_rects(points)?)),
            Geometry::Path { points, width } => Ok(Region::from_rects(path_rects(points, *width)?)),
            Geometry::Label { .. } => Ok(Region::default()),
        }
    }

    /// Union of several geometries.
    pub fn from_geometries<'a>(geoms: impl IntoIterator<Item = &'a Geometry>) -> Result<Region, String> {
        let mut rects = Vec::new();
        for g in geoms {
            match g {
                Geometry::Rect(r) => rects.push(*r),
                Geometry::Polygon { points } => rects.extend(polygon_rects(points)?),
                Geometry::Path { points, width } => rects.extend(path_rects(points, *width)?),
                Geometry::Label { .. } => {}
            }
        }
        Ok(Region::from_rects(rects))
    }

    pub fn rects(&self) -> &[IRect] {
        &self.rects
    }

    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    pub fn area(&self) -> i128 {
        self.rects.iter().map(|r| r.width() as i128 * r.height() as i128).sum()
    }

    pub fn bbox(&self) -> Option<IRect> {
        self.rects.iter().copied().reduce(|a, b| a.union(&b))
    }

    pub fn boolean(&self, other: &Region, op: BoolOp) -> Region {
        Region { rects: sweep(&self.rects, &other.rects, op.mask()) }
    }

    /// Grow (d > 0) or shrink (d < 0) by `d` with square corners.
    pub fn sized(&self, d: i64) -> Region {
        if d == 0 || self.is_empty() {
            return self.clone();
        }
        if d > 0 {
            return Region::from_rects(self.rects.iter().map(|r| grow(r, d)));
        }
        let e = -d;
        let Some(bb) = self.bbox() else { return Region::default() };
        let outside = Region::from_rects([grow(&bb, e)]).boolean(self, BoolOp::Not);
        self.boolean(&outside.sized(e), BoolOp::Not)
    }

    pub fn translated(&self, dx: i64, dy: i64) -> Region {
        Region { rects: self.rects.iter().map(|r| r.translate(dx, dy)).collect() }
    }

    /// Minkowski sum with the box `[-left, right] x [-bottom, top]` (all >= 0): every rectangle
    /// `[x0, x1) x [y0, y1)` becomes `[x0 - left, x1 + right) x [y0 - bottom, y1 + top)`.
    pub fn dilated(&self, left: i64, bottom: i64, right: i64, top: i64) -> Region {
        if left == 0 && bottom == 0 && right == 0 && top == 0 {
            return self.clone();
        }
        Region::from_rects(self.rects.iter().map(|r| IRect {
            x0: r.x0 - left,
            y0: r.y0 - bottom,
            x1: r.x1 + right,
            y1: r.y1 + top,
        }))
    }

    /// Morphological erosion, the inverse of [`Region::dilated`] with the same arguments: a
    /// rectangle `[x0, x1) x [y0, y1)` becomes `[x0 + left, x1 - right) x [y0 + bottom, y1 - top)`.
    pub fn eroded(&self, left: i64, bottom: i64, right: i64, top: i64) -> Region {
        if left == 0 && bottom == 0 && right == 0 && top == 0 {
            return self.clone();
        }
        let Some(bb) = self.bbox() else { return Region::default() };
        let m = left.max(bottom).max(right).max(top) + 1;
        let outside = Region::from_rects([grow(&bb, m)]).boolean(self, BoolOp::Not);
        self.boolean(&outside.dilated(right, top, left, bottom), BoolOp::Not)
    }

    /// Edge-connected components, each as its own (canonical) region.
    pub fn parts(&self) -> Vec<Region> {
        self.components().into_iter().map(|c| Region { rects: c.into_iter().map(|i| self.rects[i]).collect() }).collect()
    }

    /// Simple, hole-free polygons (counter-clockwise, no repeated or collinear points) whose
    /// union is exactly this region and whose interiors are disjoint.
    ///
    /// Each edge-connected component becomes one outline. A component with holes is cut by
    /// full-height vertical lines through the leftmost edge of each hole, and every resulting
    /// piece is traced separately. Components too large for a compressed grid are emitted as
    /// their canonical rectangles.
    pub fn to_polygons(&self) -> Vec<Vec<(i64, i64)>> {
        let mut out = Vec::new();
        for comp in self.components() {
            let rects: Vec<IRect> = comp.iter().map(|&i| self.rects[i]).collect();
            trace_component(&rects, &mut out);
        }
        out
    }

    /// `to_polygons` as geometry: 4-vertex rectangles become `Geometry::Rect`.
    pub fn to_geometry(&self) -> Vec<Geometry> {
        self.to_polygons()
            .into_iter()
            .map(|points| {
                if points.len() == 4 {
                    let (xa, xb) = (points.iter().map(|p| p.0).min(), points.iter().map(|p| p.0).max());
                    let (ya, yb) = (points.iter().map(|p| p.1).min(), points.iter().map(|p| p.1).max());
                    if let (Some(x0), Some(x1), Some(y0), Some(y1)) = (xa, xb, ya, yb) {
                        return Geometry::Rect(IRect::new(x0, y0, x1, y1));
                    }
                }
                Geometry::Polygon { points }
            })
            .collect()
    }

    /// Rect indices grouped into components connected through edges of positive length.
    fn components(&self) -> Vec<Vec<usize>> {
        let rs = &self.rects;
        let n = rs.len();
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(p: &mut [usize], mut i: usize) -> usize {
            while p[i] != i {
                p[i] = p[p[i]];
                i = p[i];
            }
            i
        }
        for (i, r) in rs.iter().enumerate() {
            let mut j = rs.partition_point(|q| q.y0 < r.y1 || (q.y0 == r.y1 && q.x1 <= r.x0));
            while j < n && rs[j].y0 == r.y1 && rs[j].x0 < r.x1 {
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                parent[a] = b;
                j += 1;
            }
        }
        let mut groups: HashMap<usize, usize> = HashMap::new();
        let mut out: Vec<Vec<usize>> = Vec::new();
        for i in 0..n {
            let root = find(&mut parent, i);
            let g = *groups.entry(root).or_insert_with(|| {
                out.push(Vec::new());
                out.len() - 1
            });
            out[g].push(i);
        }
        out
    }
}

fn grow(r: &IRect, d: i64) -> IRect {
    IRect { x0: r.x0 - d, y0: r.y0 - d, x1: r.x1 + d, y1: r.y1 + d }
}

fn path_rects(points: &[(i64, i64)], width: i64) -> Result<Vec<IRect>, String> {
    let h = width.abs() / 2;
    points
        .windows(2)
        .map(|w| {
            let (a, b) = (w[0], w[1]);
            if a.0 != b.0 && a.1 != b.1 {
                return Err(format!("path segment ({}, {})-({}, {}) is not axis-aligned", a.0, a.1, b.0, b.1));
            }
            Ok(IRect::new(a.0.min(b.0) - h, a.1.min(b.1) - h, a.0.max(b.0) + h, a.1.max(b.1) + h))
        })
        .collect()
}

/// Decompose a rectilinear polygon (nonzero winding rule) into rectangles by sweeping its
/// vertical edges from left to right.
fn polygon_rects(points: &[(i64, i64)]) -> Result<Vec<IRect>, String> {
    let mut pts: Vec<(i64, i64)> = Vec::with_capacity(points.len());
    for &p in points {
        if pts.last() != Some(&p) {
            pts.push(p);
        }
    }
    while pts.len() > 1 && pts.first() == pts.last() {
        pts.pop();
    }
    let n = pts.len();
    let mut edges: Vec<(i64, i64, i64, i32)> = Vec::new();
    for k in 0..n {
        let (a, b) = (pts[k], pts[(k + 1) % n]);
        if a.0 == b.0 {
            if a.1 != b.1 {
                edges.push((a.0, a.1.min(b.1), a.1.max(b.1), if b.1 > a.1 { 1 } else { -1 }));
            }
        } else if a.1 != b.1 {
            return Err(format!("polygon edge ({}, {})-({}, {}) is not axis-aligned", a.0, a.1, b.0, b.1));
        }
    }
    let mut ys: Vec<i64> = edges.iter().flat_map(|e| [e.1, e.2]).collect();
    ys.sort_unstable();
    ys.dedup();
    if ys.len() < 2 {
        return Ok(Vec::new());
    }
    edges.sort_unstable_by_key(|e| e.0);
    let mut winding = vec![0i32; ys.len() - 1];
    let mut rects = Vec::new();
    let mut prev_x: Option<i64> = None;
    let mut k = 0;
    while k < edges.len() {
        let x = edges[k].0;
        if let Some(px) = prev_x {
            let mut j = 0;
            while j < winding.len() {
                if winding[j] == 0 {
                    j += 1;
                    continue;
                }
                let start = j;
                while j < winding.len() && winding[j] != 0 {
                    j += 1;
                }
                rects.push(IRect::new(px, ys[start], x, ys[j]));
            }
        }
        while k < edges.len() && edges[k].0 == x {
            let (_, lo, hi, sign) = edges[k];
            let (j0, j1) = (ys.partition_point(|&y| y < lo), ys.partition_point(|&y| y < hi));
            for w in &mut winding[j0..j1] {
                *w += sign;
            }
            k += 1;
        }
        prev_x = Some(x);
    }
    Ok(rects)
}

/// Segment tree over compressed x slots holding, per node, the cover counts of both operands
/// and the set of `(in_a, in_b)` states present among the slots below it.
struct CoverTree {
    cnt: Vec<[i32; 2]>,
    mask: Vec<u8>,
}

impl CoverTree {
    fn new(slots: usize) -> Self {
        let size = 4 * slots.max(1);
        CoverTree { cnt: vec![[0, 0]; size], mask: vec![1; size] }
    }

    fn cover(mask: u8, a: bool, b: bool) -> u8 {
        let mut m = mask;
        if a {
            m = ((m & 0b0101) << 1) | (m & 0b1010);
        }
        if b {
            m = ((m & 0b0011) << 2) | (m & 0b1100);
        }
        m
    }

    #[allow(clippy::too_many_arguments)]
    fn update(&mut self, node: usize, lo: usize, hi: usize, ql: usize, qr: usize, which: usize, delta: i32) {
        if qr <= lo || hi <= ql {
            return;
        }
        if ql <= lo && hi <= qr {
            self.cnt[node][which] += delta;
        } else {
            let mid = (lo + hi) / 2;
            self.update(2 * node, lo, mid, ql, qr, which, delta);
            self.update(2 * node + 1, mid, hi, ql, qr, which, delta);
        }
        let base = if hi - lo == 1 { 1 } else { self.mask[2 * node] | self.mask[2 * node + 1] };
        let c = self.cnt[node];
        self.mask[node] = Self::cover(base, c[0] > 0, c[1] > 0);
    }

    /// Append the maximal slot runs in `[ql, qr)` whose state is in `want`.
    #[allow(clippy::too_many_arguments)]
    fn extract(
        &self,
        node: usize,
        lo: usize,
        hi: usize,
        ql: usize,
        qr: usize,
        inherited: (bool, bool),
        want: u8,
        out: &mut Vec<(usize, usize)>,
    ) {
        if qr <= lo || hi <= ql {
            return;
        }
        let m = Self::cover(self.mask[node], inherited.0, inherited.1);
        if m & want == 0 {
            return;
        }
        if m & !want == 0 {
            let (s, e) = (lo.max(ql), hi.min(qr));
            match out.last_mut() {
                Some(last) if last.1 == s => last.1 = e,
                _ => out.push((s, e)),
            }
            return;
        }
        let c = self.cnt[node];
        let inh = (inherited.0 || c[0] > 0, inherited.1 || c[1] > 0);
        let mid = (lo + hi) / 2;
        self.extract(2 * node, lo, mid, ql, qr, inh, want, out);
        self.extract(2 * node + 1, mid, hi, ql, qr, inh, want, out);
    }
}

/// Sweep upwards over y, maintaining the cross-section of `op(a, b)` and emitting canonical strips.
/// Only the x ranges touched by events at a given y (widened to the strips they touch) are re-examined.
fn sweep(a: &[IRect], b: &[IRect], want: u8) -> Vec<IRect> {
    let nonempty = |r: &&IRect| r.x0 < r.x1 && r.y0 < r.y1;
    let mut xs: Vec<i64> = a.iter().chain(b).filter(nonempty).flat_map(|r| [r.x0, r.x1]).collect();
    xs.sort_unstable();
    xs.dedup();
    if xs.len() < 2 {
        return Vec::new();
    }
    let slots = xs.len() - 1;
    let idx = |x: i64| xs.partition_point(|&v| v < x);
    let mut events: Vec<(i64, usize, usize, usize, i32)> = Vec::new();
    for (which, set) in [(0, a), (1, b)] {
        for r in set.iter().filter(nonempty) {
            let (lo, hi) = (idx(r.x0), idx(r.x1));
            events.push((r.y0, which, lo, hi, 1));
            events.push((r.y1, which, lo, hi, -1));
        }
    }
    events.sort_unstable_by_key(|e| e.0);

    let mut tree = CoverTree::new(slots);
    let mut open: BTreeMap<usize, (usize, i64)> = BTreeMap::new();
    let mut out = Vec::new();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut fresh: Vec<(usize, usize)> = Vec::new();
    let mut old: Vec<(usize, usize, i64)> = Vec::new();
    let mut i = 0;
    while i < events.len() {
        let y = events[i].0;
        ranges.clear();
        while i < events.len() && events[i].0 == y {
            let (_, which, lo, hi, delta) = events[i];
            tree.update(1, 0, slots, lo, hi, which, delta);
            ranges.push((lo, hi));
            i += 1;
        }
        ranges.sort_unstable();
        let mut k = 0;
        while k < ranges.len() {
            let (mut l, mut r) = ranges[k];
            k += 1;
            if let Some((&s, &(e, _))) = open.range(..=l).next_back()
                && e >= l
            {
                l = s;
                r = r.max(e);
            }
            loop {
                let mut grown = false;
                while k < ranges.len() && ranges[k].0 <= r {
                    r = r.max(ranges[k].1);
                    k += 1;
                    grown = true;
                }
                if let Some((_, &(e, _))) = open.range(l..=r).next_back()
                    && e > r
                {
                    r = e;
                    grown = true;
                }
                if !grown {
                    break;
                }
            }
            old.clear();
            old.extend(open.range(l..r).map(|(&s, &(e, y0))| (s, e, y0)));
            for &(s, ..) in &old {
                open.remove(&s);
            }
            fresh.clear();
            tree.extract(1, 0, slots, l, r, (false, false), want, &mut fresh);
            let mut close = |(s, e, y0): (usize, usize, i64)| out.push(IRect { x0: xs[s], y0, x1: xs[e], y1: y });
            let mut oi = 0;
            for &(s, e) in &fresh {
                while oi < old.len() && old[oi].0 < s {
                    close(old[oi]);
                    oi += 1;
                }
                if oi < old.len() && old[oi].0 == s && old[oi].1 == e {
                    open.insert(s, (e, old[oi].2));
                    oi += 1;
                } else {
                    open.insert(s, (e, y));
                }
            }
            for &o in &old[oi..] {
                close(o);
            }
        }
    }
    out.sort_unstable_by_key(|r| (r.y0, r.x0));
    out
}

type GridEdge = ((usize, usize), (usize, usize));

fn rect_polygon(r: &IRect) -> Vec<(i64, i64)> {
    vec![(r.x0, r.y0), (r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1)]
}

/// Trace one edge-connected component on its compressed grid, cutting it into hole-free pieces.
fn trace_component(rects: &[IRect], out: &mut Vec<Vec<(i64, i64)>>) {
    let sorted_unique = |mut v: Vec<i64>| {
        v.sort_unstable();
        v.dedup();
        v
    };
    let xs = sorted_unique(rects.iter().flat_map(|r| [r.x0, r.x1]).collect());
    let ys = sorted_unique(rects.iter().flat_map(|r| [r.y0, r.y1]).collect());
    let (nx, ny) = (xs.len() - 1, ys.len() - 1);
    if rects.len() == 1 || nx.saturating_mul(ny) > MAX_GRID_CELLS {
        out.extend(rects.iter().map(rect_polygon));
        return;
    }
    let ix = |x: i64| xs.partition_point(|&v| v < x);
    let iy = |y: i64| ys.partition_point(|&v| v < y);
    let at = |i: usize, j: usize| j * nx + i;
    let mut filled = vec![false; nx * ny];
    for r in rects {
        for j in iy(r.y0)..iy(r.y1) {
            filled[at(ix(r.x0), j)..at(ix(r.x1), j)].fill(true);
        }
    }
    let neighbors = |c: usize| {
        let (i, j) = (c % nx, c / nx);
        [(i > 0).then(|| c - 1), (i + 1 < nx).then(|| c + 1), (j > 0).then(|| c - nx), (j + 1 < ny).then(|| c + nx)]
    };

    let mut seen = filled.clone();
    let mut stack: Vec<usize> = (0..nx * ny)
        .filter(|&c| {
            let (i, j) = (c % nx, c / nx);
            !filled[c] && (i == 0 || j == 0 || i + 1 == nx || j + 1 == ny)
        })
        .collect();
    for &c in &stack {
        seen[c] = true;
    }
    while let Some(c) = stack.pop() {
        for n in neighbors(c).into_iter().flatten() {
            if !seen[n] {
                seen[n] = true;
                stack.push(n);
            }
        }
    }
    let mut cut = vec![false; nx + 1];
    for start in 0..nx * ny {
        if seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let mut min_i = start % nx;
        while let Some(c) = stack.pop() {
            min_i = min_i.min(c % nx);
            for n in neighbors(c).into_iter().flatten() {
                if !seen[n] {
                    seen[n] = true;
                    stack.push(n);
                }
            }
        }
        cut[min_i] = true;
    }
    let mut slab = vec![0u32; nx];
    let mut s = 0;
    for (i, sl) in slab.iter_mut().enumerate() {
        s += cut[i] as u32;
        *sl = s;
    }

    let mut label = vec![u32::MAX; nx * ny];
    let mut count = 0u32;
    for start in 0..nx * ny {
        if !filled[start] || label[start] != u32::MAX {
            continue;
        }
        label[start] = count;
        stack.push(start);
        while let Some(c) = stack.pop() {
            for n in neighbors(c).into_iter().flatten() {
                if filled[n] && label[n] == u32::MAX && slab[n % nx] == slab[c % nx] {
                    label[n] = count;
                    stack.push(n);
                }
            }
        }
        count += 1;
    }

    let mut edges: Vec<Vec<GridEdge>> = vec![Vec::new(); count as usize];
    for c in 0..nx * ny {
        let l = label[c];
        if l == u32::MAX {
            continue;
        }
        let (i, j) = (c % nx, c / nx);
        let other = |n: Option<usize>| n.is_none_or(|n| label[n] != l);
        let [left, right, down, up] = neighbors(c);
        let e = &mut edges[l as usize];
        if other(down) {
            e.push(((i, j), (i + 1, j)));
        }
        if other(right) {
            e.push(((i + 1, j), (i + 1, j + 1)));
        }
        if other(up) {
            e.push(((i + 1, j + 1), (i, j + 1)));
        }
        if other(left) {
            e.push(((i, j + 1), (i, j)));
        }
    }

    for (l, e) in edges.iter().enumerate() {
        match trace_loop(e) {
            Some(lp) => out.push(lp.into_iter().map(|(i, j)| (xs[i], ys[j])).collect()),
            None => {
                for j in 0..ny {
                    let mut i = 0;
                    while i < nx {
                        if label[at(i, j)] != l as u32 {
                            i += 1;
                            continue;
                        }
                        let s = i;
                        while i < nx && label[at(i, j)] == l as u32 {
                            i += 1;
                        }
                        out.push(rect_polygon(&IRect::new(xs[s], ys[j], xs[i], ys[j + 1])));
                    }
                }
            }
        }
    }
}

/// Walk unit grid edges forming a single simple loop, dropping collinear vertices.
/// `None` if the edges do not form exactly one loop with unique vertices.
fn trace_loop(edges: &[GridEdge]) -> Option<Vec<(usize, usize)>> {
    let mut next: HashMap<(usize, usize), (usize, usize)> = HashMap::with_capacity(edges.len());
    for &(a, b) in edges {
        if next.insert(a, b).is_some() {
            return None;
        }
    }
    let start = edges.first()?.0;
    let mut pts = vec![start];
    let mut cur = next[&start];
    while cur != start {
        pts.push(cur);
        cur = *next.get(&cur)?;
        if pts.len() > edges.len() {
            return None;
        }
    }
    if pts.len() != edges.len() {
        return None;
    }
    let n = pts.len();
    let dir = |a: (usize, usize), b: (usize, usize)| (b.0 as i64 - a.0 as i64, b.1 as i64 - a.1 as i64);
    let mut corners: Vec<(usize, usize)> =
        (0..n).filter(|&k| dir(pts[(k + n - 1) % n], pts[k]) != dir(pts[k], pts[(k + 1) % n])).map(|k| pts[k]).collect();
    let first = (0..corners.len()).min_by_key(|&k| (corners[k].1, corners[k].0))?;
    corners.rotate_left(first);
    Some(corners)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x0: i64, y0: i64, x1: i64, y1: i64) -> IRect {
        IRect::new(x0, y0, x1, y1)
    }

    fn reg(rs: &[(i64, i64, i64, i64)]) -> Region {
        Region::from_rects(rs.iter().map(|&(a, b, c, d)| r(a, b, c, d)))
    }

    fn shoelace2(p: &[(i64, i64)]) -> i128 {
        (0..p.len())
            .map(|k| {
                let (a, b) = (p[k], p[(k + 1) % p.len()]);
                a.0 as i128 * b.1 as i128 - b.0 as i128 * a.1 as i128
            })
            .sum()
    }

    fn check_polygons(region: &Region) -> Vec<Vec<(i64, i64)>> {
        let polys = region.to_polygons();
        let mut total = 0i128;
        let mut union = Region::default();
        for p in &polys {
            assert!(p.len() >= 4 && p.len() % 2 == 0, "{p:?}");
            let n = p.len();
            for k in 0..n {
                let (a, b, c) = (p[k], p[(k + 1) % n], p[(k + 2) % n]);
                assert!(a != b, "repeated vertex {p:?}");
                assert!(a.0 == b.0 || a.1 == b.1, "diagonal edge {p:?}");
                assert!(!((a.0 == b.0 && b.0 == c.0) || (a.1 == b.1 && b.1 == c.1)), "collinear {p:?}");
            }
            let mut uniq = p.clone();
            uniq.sort_unstable();
            uniq.dedup();
            assert_eq!(uniq.len(), n, "pinched polygon {p:?}");
            let area2 = shoelace2(p);
            assert!(area2 > 0, "not CCW {p:?}");
            let pr = Region::from_geometry(&Geometry::Polygon { points: p.clone() }).unwrap();
            assert_eq!(pr.area() * 2, area2, "self-intersecting {p:?}");
            total += pr.area();
            union = union.boolean(&pr, BoolOp::Or);
        }
        assert_eq!(total, region.area(), "overlapping pieces");
        assert_eq!(&union, region);
        polys
    }

    fn assert_canonical(region: &Region) {
        let rs = region.rects();
        for w in rs.windows(2) {
            assert!((w[0].y0, w[0].x0) < (w[1].y0, w[1].x0));
        }
        for (i, a) in rs.iter().enumerate() {
            assert!(a.width() > 0 && a.height() > 0);
            for b in &rs[i + 1..] {
                assert!(!a.intersects(b), "{a:?} {b:?}");
            }
        }
    }

    #[test]
    fn booleans_on_overlapping_rects() {
        let a = reg(&[(0, 0, 10, 10)]);
        let b = reg(&[(5, 5, 15, 15)]);
        let or = a.boolean(&b, BoolOp::Or);
        let and = a.boolean(&b, BoolOp::And);
        let not = a.boolean(&b, BoolOp::Not);
        let xor = a.boolean(&b, BoolOp::Xor);
        assert_eq!((or.area(), and.area(), not.area(), xor.area()), (175, 25, 75, 150));
        assert_eq!(and, reg(&[(5, 5, 10, 10)]));
        assert_eq!(not, reg(&[(0, 0, 5, 10), (5, 0, 10, 5)]));
        assert_eq!(not, reg(&[(0, 0, 10, 5), (0, 5, 5, 10)]));
        assert_eq!(or, reg(&[(0, 0, 10, 10), (5, 5, 15, 15)]));
        assert_eq!(or, reg(&[(0, 0, 10, 5), (0, 5, 15, 10), (5, 10, 15, 15), (6, 6, 7, 7)]));
        assert_eq!(xor, or.boolean(&and, BoolOp::Not));
        assert_eq!(or.rects(), &[r(0, 0, 10, 5), r(0, 5, 15, 10), r(5, 10, 15, 15)]);
        assert_eq!(or.bbox(), Some(r(0, 0, 15, 15)));
        assert_eq!(Region::default().bbox(), None);
        assert!(a.boolean(&a, BoolOp::Not).is_empty());
        assert!(reg(&[(0, 0, 0, 10), (3, 3, 9, 3)]).is_empty());
        assert_eq!(reg(&[(0, 0, 5, 10), (5, 0, 10, 10)]), reg(&[(0, 0, 10, 10)]));
        assert_eq!(reg(&[(0, 0, 10, 5), (0, 5, 10, 10)]).rects(), &[r(0, 0, 10, 10)]);
    }

    #[test]
    fn l_u_t_shapes_become_single_polygons() {
        let l = reg(&[(0, 0, 30, 10), (0, 10, 10, 30)]);
        let u = reg(&[(0, 0, 30, 10), (0, 10, 10, 30), (20, 10, 30, 30)]);
        let t = reg(&[(0, 20, 30, 30), (10, 0, 20, 20)]);
        for (shape, verts, area) in [(&l, 6, 500), (&u, 8, 700), (&t, 8, 500)] {
            let polys = check_polygons(shape);
            assert_eq!(polys.len(), 1);
            assert_eq!(polys[0].len(), verts);
            assert_eq!(shape.area(), area);
            let geoms = shape.to_geometry();
            assert!(matches!(geoms[0], Geometry::Polygon { .. }));
            assert_eq!(&Region::from_geometries(&geoms).unwrap(), shape);
            assert_eq!(&Region::from_geometry(&geoms[0]).unwrap(), shape);
        }
        assert_eq!(l.to_polygons()[0], vec![(0, 0), (30, 0), (30, 10), (10, 10), (10, 30), (0, 30)]);
        assert_eq!(reg(&[(1, 2, 3, 4)]).to_geometry(), vec![Geometry::Rect(r(1, 2, 3, 4))]);
    }

    #[test]
    fn polygon_orientation_and_fill_rule() {
        let ccw = vec![(0, 0), (30, 0), (30, 10), (10, 10), (10, 30), (0, 30)];
        let mut cw = ccw.clone();
        cw.reverse();
        let a = Region::from_geometry(&Geometry::Polygon { points: ccw.clone() }).unwrap();
        let b = Region::from_geometry(&Geometry::Polygon { points: cw }).unwrap();
        assert_eq!(a, b);
        let mut closed = ccw.clone();
        closed.push(ccw[0]);
        closed.insert(2, ccw[2]);
        assert_eq!(Region::from_geometry(&Geometry::Polygon { points: closed }).unwrap(), a);
        let doubled = vec![(0, 0), (10, 0), (10, 10), (0, 10), (0, 0), (10, 0), (10, 10), (0, 10)];
        assert_eq!(Region::from_geometry(&Geometry::Polygon { points: doubled }).unwrap(), reg(&[(0, 0, 10, 10)]));
        let touching = vec![(0, 0), (10, 0), (10, 10), (20, 10), (20, 20), (10, 20), (10, 10), (0, 10)];
        assert_eq!(
            Region::from_geometry(&Geometry::Polygon { points: touching }).unwrap(),
            reg(&[(0, 0, 10, 10), (10, 10, 20, 20)])
        );
    }

    #[test]
    fn path_with_corner() {
        let g = Geometry::Path { points: vec![(0, 0), (100, 0), (100, 100)], width: 20 };
        let p = Region::from_geometry(&g).unwrap();
        assert_eq!(p.area(), 120 * 20 + 20 * 100);
        assert_eq!(p, reg(&[(-10, -10, 110, 10), (90, -10, 110, 110)]));
        let polys = check_polygons(&p);
        assert_eq!(polys.len(), 1);
        assert_eq!(polys[0].len(), 6);
        let label = Geometry::Label { text: "x".into(), at: (0, 0), size: 10 };
        assert!(Region::from_geometry(&label).unwrap().is_empty());
    }

    #[test]
    fn non_manhattan_is_an_error() {
        let tri = Geometry::Polygon { points: vec![(0, 0), (10, 0), (5, 8)] };
        assert!(Region::from_geometry(&tri).unwrap_err().contains("not axis-aligned"));
        let diag = Geometry::Path { points: vec![(0, 0), (10, 10)], width: 2 };
        assert!(Region::from_geometry(&diag).unwrap_err().contains("not axis-aligned"));
        assert!(Region::from_geometries([&Geometry::Rect(r(0, 0, 1, 1)), &tri]).is_err());
    }

    #[test]
    fn sizing_round_trip_and_vanishing() {
        let u = reg(&[(0, 0, 30, 10), (0, 10, 10, 30), (20, 10, 30, 30)]);
        let grown = u.sized(4);
        assert_eq!(grown, reg(&[(-4, -4, 34, 14), (-4, 14, 14, 34), (16, 14, 34, 34)]));
        assert_eq!(grown.sized(-4), u);
        assert_eq!(u.sized(0), u);
        let bar = reg(&[(0, 0, 100, 6)]);
        assert!(bar.sized(-3).is_empty());
        assert_eq!(bar.sized(-2), reg(&[(2, 2, 98, 4)]));
        let dumbbell = reg(&[(0, 0, 20, 20), (20, 8, 40, 12), (40, 0, 60, 20)]);
        assert_eq!(dumbbell.sized(-3), reg(&[(3, 3, 17, 17), (43, 3, 57, 17)]));
    }

    #[test]
    fn asymmetric_morphology_and_parts() {
        let mut rng = Lcg(99);
        for case in 0..100 {
            let ra = rng.rects(16, 9);
            let a = Region::from_rects(ra.clone());
            let (l, b, rt, t) = (rng.next(3), rng.next(3), rng.next(4), rng.next(2));
            let (dil, ero) = (a.dilated(l, b, rt, t), a.eroded(l, b, rt, t));
            assert_canonical(&ero);
            for y in -8..30 {
                for x in -8..30 {
                    let mut offsets = (-b..=t).flat_map(|dy| (-l..=rt).map(move |dx| (dx, dy)));
                    let any = offsets.clone().any(|(dx, dy)| inside(&ra, x - dx, y - dy));
                    let all = offsets.all(|(dx, dy)| inside(&ra, x + dx, y + dy));
                    assert_eq!(inside(dil.rects(), x, y), any, "case {case} dilate at ({x}, {y})");
                    assert_eq!(inside(ero.rects(), x, y), all, "case {case} erode at ({x}, {y})");
                }
            }
            let parts = a.parts();
            assert_eq!(Region::from_rects(parts.iter().flat_map(|p| p.rects().to_vec())), a);
            for p in &parts {
                assert_eq!(&Region::from_rects(p.rects().to_vec()), p);
            }
            assert_eq!(a.translated(3, -2).bbox(), a.bbox().map(|r| r.translate(3, -2)));
        }
        assert_eq!(reg(&[(0, 0, 10, 10), (10, 10, 20, 20), (30, 0, 40, 5)]).parts().len(), 3);
    }

    #[test]
    fn donut_splits_into_hole_free_pieces() {
        let donut = reg(&[(0, 0, 30, 30)]).boolean(&reg(&[(10, 10, 20, 20)]), BoolOp::Not);
        assert_eq!(donut.area(), 800);
        let polys = check_polygons(&donut);
        assert!(polys.len() >= 2);
        let two_holes = reg(&[(0, 0, 50, 20)]).boolean(&reg(&[(5, 5, 10, 10), (30, 8, 40, 12)]), BoolOp::Not);
        check_polygons(&two_holes);
        let pinch = reg(&[(0, 0, 30, 10), (0, 10, 10, 30), (10, 20, 20, 30), (20, 10, 30, 20)]);
        check_polygons(&pinch);
        let corners = reg(&[(0, 0, 10, 10), (10, 10, 20, 20)]);
        assert_eq!(check_polygons(&corners).len(), 2);
    }

    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self, n: i64) -> i64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 33) % n as u64) as i64
        }
        fn rects(&mut self, max: i64, size: i64) -> Vec<IRect> {
            let count = 1 + self.next(6);
            (0..count)
                .map(|_| {
                    let (x, y) = (self.next(max), self.next(max));
                    r(x, y, x + self.next(size), y + self.next(size))
                })
                .collect()
        }
    }

    fn inside(rs: &[IRect], x: i64, y: i64) -> bool {
        rs.iter().any(|r| r.x0 <= x && x < r.x1 && r.y0 <= y && y < r.y1)
    }

    #[test]
    fn randomized_against_brute_force() {
        let mut rng = Lcg(12345);
        let n = 24;
        for case in 0..200 {
            let (ra, rb) = (rng.rects(16, 9), rng.rects(16, 9));
            let (a, b) = (Region::from_rects(ra.clone()), Region::from_rects(rb.clone()));
            assert_canonical(&a);
            for op in [BoolOp::Or, BoolOp::And, BoolOp::Not, BoolOp::Xor] {
                let res = a.boolean(&b, op);
                assert_canonical(&res);
                let mut cells = 0;
                for y in -2..n {
                    for x in -2..n {
                        let (ia, ib) = (inside(&ra, x, y), inside(&rb, x, y));
                        let expect = match op {
                            BoolOp::Or => ia || ib,
                            BoolOp::And => ia && ib,
                            BoolOp::Not => ia && !ib,
                            BoolOp::Xor => ia != ib,
                        };
                        assert_eq!(inside(res.rects(), x, y), expect, "case {case} {op:?} at ({x}, {y})");
                        cells += expect as i128;
                    }
                }
                assert_eq!(res.area(), cells);
                check_polygons(&res);
            }
            assert_eq!(a.boolean(&b, BoolOp::Or), Region::from_rects(ra.iter().chain(&rb).copied()));
            assert_eq!(a.boolean(&b, BoolOp::Xor), a.boolean(&b, BoolOp::Or).boolean(&a.boolean(&b, BoolOp::And), BoolOp::Not));
            let d = 1 + rng.next(3);
            let (grown, shrunk) = (a.sized(d), a.sized(-d));
            for y in -6..n + 6 {
                for x in -6..n + 6 {
                    let near = (-d..=d).flat_map(|dy| (-d..=d).map(move |dx| (dx, dy)));
                    let any = near.clone().any(|(dx, dy)| inside(&ra, x + dx, y + dy));
                    let all = near.clone().all(|(dx, dy)| inside(&ra, x + dx, y + dy));
                    assert_eq!(inside(grown.rects(), x, y), any, "case {case} grow {d} at ({x}, {y})");
                    assert_eq!(inside(shrunk.rects(), x, y), all, "case {case} shrink {d} at ({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn many_rects_are_fast() {
        let mut rng = Lcg(7);
        let rects: Vec<IRect> = (0..20_000)
            .map(|_| {
                let (x, y) = (rng.next(200_000), rng.next(200_000));
                r(x, y, x + 1 + rng.next(2_000), y + 1 + rng.next(2_000))
            })
            .collect();
        let start = std::time::Instant::now();
        let all = Region::from_rects(rects.clone());
        let a = Region::from_rects(rects[..10_000].iter().copied());
        let b = Region::from_rects(rects[10_000..].iter().copied());
        assert_eq!(a.boolean(&b, BoolOp::Or), all);
        let and = a.boolean(&b, BoolOp::And);
        assert_eq!(a.area() + b.area() - and.area(), all.area());
        let polys = all.to_polygons();
        assert!(!polys.is_empty());
        assert!(start.elapsed().as_secs() < 60);
    }
}
