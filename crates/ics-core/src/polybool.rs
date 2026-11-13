//! Boolean operations, merging and sizing for general (any-angle) integer polygons with holes.
//!
//! [`PolySet`] is the any-angle counterpart of [`Region`]: a point set bounded by straight edges
//! between integer points, stored as polygons with holes (outlines counter-clockwise, holes
//! clockwise, no collinear vertices, sorted). Every operation runs the same pipeline:
//!
//! 1. *Noding*: all edges of both operands are split at every crossing, touching vertex and
//!    collinear overlap, until edges meet only at shared end points. Coincident edges are merged,
//!    summing the winding contribution of each operand.
//! 2. *Classification*: a scanbeam sweep (sorted distinct y values; edges spanning a beam never
//!    cross inside it) gives the winding numbers of both operands on each side of every edge.
//! 3. *Linking*: edges with the result inside on exactly one side are kept, oriented with the
//!    inside on their left, and linked into rings by taking the first clockwise edge at every
//!    vertex, which keeps regions that touch at a single point apart.
//!
//! All predicates are exact (`i128`) for coordinates up to [`MAX_COORD`] in magnitude (GDSII's
//! 32-bit range). The one approximation is snap rounding: a crossing that is not on the integer
//! grid is rounded to the nearest grid point and both edges are bent through it, so boundaries
//! move by less than one database unit. Octilinear geometry whose vertices are on an even grid
//! crosses only at grid points and is computed exactly.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::f64::consts::SQRT_2;

use crate::geom::IRect;
use crate::layout::Geometry;
use crate::region::{BoolOp, Region};

pub type Point = (i64, i64);

/// Largest coordinate magnitude handled with exact arithmetic.
pub const MAX_COORD: i64 = 1 << 31;

/// Noding normally settles in two or three rounds; this bounds pathological snap cascades.
const MAX_NODING_ROUNDS: usize = 64;

/// A polygon with holes: `outer` is counter-clockwise, every hole clockwise.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Polygon {
    pub outer: Vec<Point>,
    pub holes: Vec<Vec<Point>>,
}

impl Polygon {
    /// Twice the enclosed area (exact).
    pub fn area2(&self) -> i128 {
        ring_area2(&self.outer) + self.holes.iter().map(|h| ring_area2(h)).sum::<i128>()
    }
}

/// A point set bounded by any-angle edges, as merged polygons with holes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolySet {
    polys: Vec<Polygon>,
}

impl PolySet {
    /// The area enclosed by one outline under the nonzero winding rule (orientation does not
    /// matter; self-intersecting outlines fill every loop).
    pub fn from_polygon(points: &[Point]) -> PolySet {
        from_rings([points])
    }

    pub fn from_rect(r: IRect) -> PolySet {
        from_rings([rect_ring(r)])
    }

    /// Rect, polygon (nonzero winding) or path; labels give an empty set.
    ///
    /// Paths are the union of their segments. Manhattan paths extend every segment by half the
    /// width at both ends, like [`Region::from_geometry`]; other paths extend only the two path
    /// ends and fill bends with miter joins (bevelled where the miter would exceed the width).
    pub fn from_geometry(g: &Geometry) -> PolySet {
        from_rings(geometry_rings(g))
    }

    /// Union of several geometries.
    pub fn from_geometries<'a>(geoms: impl IntoIterator<Item = &'a Geometry>) -> PolySet {
        from_rings(geoms.into_iter().flat_map(geometry_rings))
    }

    pub fn from_region(r: &Region) -> PolySet {
        from_rings(r.to_polygons())
    }

    pub fn polygons(&self) -> &[Polygon] {
        &self.polys
    }

    pub fn is_empty(&self) -> bool {
        self.polys.is_empty()
    }

    /// Twice the area (exact: polygons on the integer grid have half-integer areas).
    pub fn area2(&self) -> i128 {
        self.polys.iter().map(Polygon::area2).sum()
    }

    pub fn area(&self) -> f64 {
        self.area2() as f64 / 2.0
    }

    pub fn bbox(&self) -> Option<IRect> {
        self.polys.iter().map(|p| ring_bbox(&p.outer)).reduce(|a, b| a.union(&b))
    }

    /// Whether every edge is horizontal or vertical.
    pub fn is_manhattan(&self) -> bool {
        self.rings().all(ring_is_manhattan)
    }

    pub fn boolean(&self, other: &PolySet, op: BoolOp) -> PolySet {
        let a: Vec<&[Point]> = self.rings().collect();
        let b: Vec<&[Point]> = other.rings().collect();
        build(&a, &b, op.mask())
    }

    /// Grow (d > 0) or shrink (d < 0) by `d`.
    ///
    /// Horizontal, vertical and 45° edges move by `d` along their normal (45° offsets are
    /// rounded to the grid); other edges move by `d` to `1.08 d`. Corners of 90° or more are
    /// mitered, so Manhattan input gives exactly [`Region::sized`]; sharper corners are
    /// chamfered. Implemented as the Minkowski sum with an octagon plus miter patches; shrinking
    /// grows the complement.
    pub fn sized(&self, d: i64) -> PolySet {
        if d == 0 || self.is_empty() {
            return self.clone();
        }
        if d > 0 {
            return grow(self, d);
        }
        let e = -d;
        let Some(bb) = self.bbox() else { return PolySet::default() };
        let frame = PolySet::from_rect(IRect::new(bb.x0 - e - 1, bb.y0 - e - 1, bb.x1 + e + 1, bb.y1 + e + 1));
        let outside = frame.boolean(self, BoolOp::Not);
        self.boolean(&grow(&outside, e), BoolOp::Not)
    }

    pub fn translated(&self, dx: i64, dy: i64) -> PolySet {
        let mv = |r: &Vec<Point>| r.iter().map(|p| (p.0 + dx, p.1 + dy)).collect();
        PolySet {
            polys: self.polys.iter().map(|p| Polygon { outer: mv(&p.outer), holes: p.holes.iter().map(mv).collect() }).collect(),
        }
    }

    /// The same point set as a [`Region`], when it is Manhattan.
    pub fn to_region(&self) -> Option<Region> {
        if !self.is_manhattan() {
            return None;
        }
        let mut rects = Vec::new();
        for p in &self.polys {
            let outer = Region::from_geometry(&Geometry::Polygon { points: p.outer.clone() }).ok()?;
            let holes =
                Region::from_geometries(&p.holes.iter().map(|h| Geometry::Polygon { points: h.clone() }).collect::<Vec<_>>())
                    .ok()?;
            rects.extend_from_slice(outer.boolean(&holes, BoolOp::Not).rects());
        }
        Some(Region::from_rects(rects))
    }

    /// Hole-free outlines (counter-clockwise) whose union is this set. Polygons with holes are
    /// cut into pieces by vertical lines through their holes (horizontal for holes one unit
    /// wide); cuts across any-angle edges snap to the grid.
    pub fn to_polygons(&self) -> Vec<Vec<Point>> {
        let mut out = Vec::new();
        for p in &self.polys {
            split_holes(p, 0, &mut out);
        }
        out
    }

    /// `to_polygons` as geometry: axis-aligned rectangles become `Geometry::Rect`.
    pub fn to_geometry(&self) -> Vec<Geometry> {
        self.to_polygons()
            .into_iter()
            .map(|points| {
                if points.len() == 4 && ring_is_manhattan(&points) {
                    let b = ring_bbox(&points);
                    return Geometry::Rect(b);
                }
                Geometry::Polygon { points }
            })
            .collect()
    }

    fn rings(&self) -> impl Iterator<Item = &[Point]> {
        self.polys.iter().flat_map(|p| std::iter::once(p.outer.as_slice()).chain(p.holes.iter().map(Vec::as_slice)))
    }
}

/// `a op b` over shapes, as hole-free geometry: computed with [`Region`] when every shape is
/// Manhattan (the faster engine) and with [`PolySet`] otherwise.
pub fn boolean_geometry(a: &[Geometry], b: &[Geometry], op: BoolOp) -> Vec<Geometry> {
    if let (Ok(ra), Ok(rb)) = (Region::from_geometries(a), Region::from_geometries(b)) {
        return ra.boolean(&rb, op).to_geometry();
    }
    PolySet::from_geometries(a).boolean(&PolySet::from_geometries(b), op).to_geometry()
}

/// The union of `shapes` grown (d > 0) or shrunk (d < 0) by `d`; `d = 0` merges. Dispatches like
/// [`boolean_geometry`].
pub fn sized_geometry(shapes: &[Geometry], d: i64) -> Vec<Geometry> {
    match Region::from_geometries(shapes) {
        Ok(r) => r.sized(d).to_geometry(),
        Err(_) => PolySet::from_geometries(shapes).sized(d).to_geometry(),
    }
}

/// Whether a geometry is a rect, a Manhattan polygon / path or a label.
pub fn is_manhattan(g: &Geometry) -> bool {
    match g {
        Geometry::Rect(_) | Geometry::Label { .. } => true,
        Geometry::Polygon { points } => ring_is_manhattan(points),
        Geometry::Path { points, .. } => points.windows(2).all(|w| w[0].0 == w[1].0 || w[0].1 == w[1].1),
    }
}

/// Union of rings under the nonzero winding rule.
fn from_rings<R: AsRef<[Point]>>(rings: impl IntoIterator<Item = R>) -> PolySet {
    let rings: Vec<R> = rings.into_iter().collect();
    let refs: Vec<&[Point]> = rings.iter().map(AsRef::as_ref).collect();
    build(&refs, &[], BoolOp::Or.mask())
}

// ----- geometry input -----

fn rect_ring(r: IRect) -> Vec<Point> {
    vec![(r.x0, r.y0), (r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1)]
}

/// Rings that, united under the nonzero rule, give the geometry. Each ring is either
/// counter-clockwise or the normalized outline of one polygon, so rings of separate shapes
/// never cancel.
fn geometry_rings(g: &Geometry) -> Vec<Vec<Point>> {
    match g {
        Geometry::Rect(r) if r.width() > 0 && r.height() > 0 => vec![rect_ring(*r)],
        Geometry::Rect(_) | Geometry::Label { .. } => vec![],
        Geometry::Polygon { points } if points.len() <= 3 => vec![ccw(points.clone())],
        Geometry::Polygon { points } => {
            let set = PolySet::from_polygon(points);
            set.rings().map(<[Point]>::to_vec).collect()
        }
        Geometry::Path { points, width } => path_rings(points, *width),
    }
}

fn ccw(mut ring: Vec<Point>) -> Vec<Point> {
    if ring_area2(&ring) < 0 {
        ring.reverse();
    }
    ring
}

fn path_rings(points: &[Point], width: i64) -> Vec<Vec<Point>> {
    let h = width.abs() / 2;
    let mut pts: Vec<Point> = points.to_vec();
    pts.dedup();
    if h == 0 || pts.len() < 2 {
        return vec![];
    }
    if pts.windows(2).all(|w| w[0].0 == w[1].0 || w[0].1 == w[1].1) {
        return pts
            .windows(2)
            .map(|w| {
                let (a, b) = (w[0], w[1]);
                rect_ring(IRect::new(a.0.min(b.0) - h, a.1.min(b.1) - h, a.0.max(b.0) + h, a.1.max(b.1) + h))
            })
            .collect();
    }
    let hf = h as f64;
    let unit = |a: Point, b: Point| {
        let (dx, dy) = ((b.0 - a.0) as f64, (b.1 - a.1) as f64);
        let len = dx.hypot(dy);
        (dx / len, dy / len)
    };
    let at = |p: Point, v: (f64, f64)| ((p.0 as f64 + v.0).round() as i64, (p.1 as f64 + v.1).round() as i64);
    let last = pts.len() - 2;
    let mut out = Vec::new();
    for (i, w) in pts.windows(2).enumerate() {
        let u = unit(w[0], w[1]);
        let n = (-u.1 * hf, u.0 * hf);
        let (es, ee) = (if i == 0 { hf } else { 0.0 }, if i == last { hf } else { 0.0 });
        let (p, q) = ((-u.0 * es, -u.1 * es), (u.0 * ee, u.1 * ee));
        out.push(ccw(vec![
            at(w[0], (p.0 - n.0, p.1 - n.1)),
            at(w[1], (q.0 - n.0, q.1 - n.1)),
            at(w[1], (q.0 + n.0, q.1 + n.1)),
            at(w[0], (p.0 + n.0, p.1 + n.1)),
        ]));
        if i == 0 {
            continue;
        }
        let (v, u0) = (w[0], unit(pts[i - 1], w[0]));
        let turn = u0.0 * u.1 - u0.1 * u.0;
        if turn.abs() < 1e-12 && u0.0 * u.0 + u0.1 * u.1 > 0.0 {
            continue;
        }
        let side = if turn > 0.0 { -1.0 } else { 1.0 };
        let (n1, n2) = ((-u0.1 * side, u0.0 * side), (-u.1 * side, u.0 * side));
        let (c1, c2) = (at(v, (n1.0 * hf, n1.1 * hf)), at(v, (n2.0 * hf, n2.1 * hf)));
        let cos = n1.0 * n2.0 + n1.1 * n2.1;
        if 1.0 + cos > 0.5 {
            let k = hf / (1.0 + cos);
            let m = at(v, ((n1.0 + n2.0) * k, (n1.1 + n2.1) * k));
            out.push(ccw(vec![v, c1, m]));
            out.push(ccw(vec![v, m, c2]));
        } else {
            out.push(ccw(vec![v, c1, c2]));
        }
    }
    out
}

// ----- exact helpers -----

fn cross(o: Point, a: Point, b: Point) -> i128 {
    (a.0 - o.0) as i128 * (b.1 - o.1) as i128 - (a.1 - o.1) as i128 * (b.0 - o.0) as i128
}

fn cross_v(a: Point, b: Point) -> i128 {
    a.0 as i128 * b.1 as i128 - a.1 as i128 * b.0 as i128
}

fn dot_v(a: Point, b: Point) -> i128 {
    a.0 as i128 * b.0 as i128 + a.1 as i128 * b.1 as i128
}

fn sub(a: Point, b: Point) -> Point {
    (a.0 - b.0, a.1 - b.1)
}

fn add(a: Point, b: Point) -> Point {
    (a.0 + b.0, a.1 + b.1)
}

fn ring_area2(r: &[Point]) -> i128 {
    let n = r.len();
    (0..n).map(|i| cross_v(r[i], r[(i + 1) % n])).sum()
}

fn ring_bbox(r: &[Point]) -> IRect {
    let (mut b, rest) = match r.split_first() {
        Some((p, rest)) => (IRect { x0: p.0, y0: p.1, x1: p.0, y1: p.1 }, rest),
        None => return IRect::default(),
    };
    for p in rest {
        b = IRect { x0: b.x0.min(p.0), y0: b.y0.min(p.1), x1: b.x1.max(p.0), y1: b.y1.max(p.1) };
    }
    b
}

fn ring_is_manhattan(r: &[Point]) -> bool {
    let n = r.len();
    (0..n).all(|i| {
        let (a, b) = (r[i], r[(i + 1) % n]);
        a.0 == b.0 || a.1 == b.1
    })
}

/// `n / d` rounded to the nearest integer (halves away from negative infinity).
fn div_round(n: i128, d: i128) -> i64 {
    let (n, d) = if d < 0 { (-n, -d) } else { (n, d) };
    (2 * n + d).div_euclid(2 * d) as i64
}

// ----- noding -----

/// An edge from `a` to `b` with `(a.y, a.x) < (b.y, b.x)`. `w[k]` is the net number of times
/// operand `k` runs along it from `a` to `b` (minus the times from `b` to `a`).
#[derive(Debug, Clone, Copy)]
struct Seg {
    a: Point,
    b: Point,
    w: [i32; 2],
}

fn key(p: Point) -> (i64, i64) {
    (p.1, p.0)
}

fn push_edge(out: &mut Vec<(Seg, bool)>, p: Point, q: Point, w: [i32; 2]) {
    match key(p).cmp(&key(q)) {
        Ordering::Less => out.push((Seg { a: p, b: q, w }, true)),
        Ordering::Greater => out.push((Seg { a: q, b: p, w: [-w[0], -w[1]] }, true)),
        Ordering::Equal => {}
    }
}

/// Merge coincident edges; edges whose windings cancel are dropped.
fn merge_dups(mut segs: Vec<(Seg, bool)>) -> Vec<(Seg, bool)> {
    segs.sort_unstable_by_key(|(s, _)| (key(s.a), key(s.b)));
    let mut out: Vec<(Seg, bool)> = Vec::with_capacity(segs.len());
    for (s, d) in segs {
        match out.last_mut() {
            Some((t, td)) if t.a == s.a && t.b == s.b => {
                t.w = [t.w[0] + s.w[0], t.w[1] + s.w[1]];
                *td |= d;
            }
            _ => out.push((s, d)),
        }
    }
    out.retain(|(s, _)| s.w != [0, 0]);
    out
}

fn strictly_inside(p: Point, s: &Seg) -> bool {
    p != s.a
        && p != s.b
        && p.0 >= s.a.0.min(s.b.0)
        && p.0 <= s.a.0.max(s.b.0)
        && p.1 >= s.a.1.min(s.b.1)
        && p.1 <= s.a.1.max(s.b.1)
}

/// The crossing of two properly crossing edges, rounded to the grid.
fn crossing(s: &Seg, t: &Seg) -> Point {
    let (r, q) = (sub(s.b, s.a), sub(t.b, t.a));
    let den = cross_v(r, q);
    let num = cross_v(sub(t.a, s.a), q);
    (div_round(s.a.0 as i128 * den + r.0 as i128 * num, den), div_round(s.a.1 as i128 * den + r.1 as i128 * num, den))
}

fn intersect(i: usize, s: &Seg, j: usize, t: &Seg, cuts: &mut [Vec<Point>]) {
    let (d1, d2) = (cross(s.a, s.b, t.a), cross(s.a, s.b, t.b));
    let (d3, d4) = (cross(t.a, t.b, s.a), cross(t.a, t.b, s.b));
    if d1 == 0 && d2 == 0 {
        for p in [t.a, t.b] {
            if strictly_inside(p, s) {
                cuts[i].push(p);
            }
        }
        for p in [s.a, s.b] {
            if strictly_inside(p, t) {
                cuts[j].push(p);
            }
        }
        return;
    }
    for (d, p) in [(d1, t.a), (d2, t.b)] {
        if d == 0 && strictly_inside(p, s) {
            cuts[i].push(p);
        }
    }
    for (d, p) in [(d3, s.a), (d4, s.b)] {
        if d == 0 && strictly_inside(p, t) {
            cuts[j].push(p);
        }
    }
    if d1.signum() * d2.signum() < 0 && d3.signum() * d4.signum() < 0 {
        let p = crossing(s, t);
        if p != s.a && p != s.b {
            cuts[i].push(p);
        }
        if p != t.a && p != t.b {
            cuts[j].push(p);
        }
    }
}

/// Split points for every edge, from all pairs with at least one edge that changed.
fn find_cuts(segs: &[(Seg, bool)]) -> Vec<Vec<Point>> {
    let n = segs.len();
    let mut cuts = vec![Vec::new(); n];
    let xmin = |s: &Seg| s.a.0.min(s.b.0);
    let xmax = |s: &Seg| s.a.0.max(s.b.0);
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_unstable_by_key(|&i| xmin(&segs[i].0));
    let mut active: Vec<usize> = Vec::new();
    for &i in &order {
        let (s, ds) = segs[i];
        let x0 = xmin(&s);
        active.retain(|&j| xmax(&segs[j].0) >= x0);
        for &j in &active {
            let (t, dt) = segs[j];
            if (ds || dt) && s.a.1 <= t.b.1 && t.a.1 <= s.b.1 {
                intersect(i, &s, j, &t, &mut cuts);
            }
        }
        active.push(i);
    }
    cuts
}

/// Split edges until they meet only at shared end points.
fn node(segs: Vec<(Seg, bool)>) -> Vec<Seg> {
    let mut segs = merge_dups(segs);
    for _ in 0..MAX_NODING_ROUNDS {
        let cuts = find_cuts(&segs);
        if cuts.iter().all(Vec::is_empty) {
            break;
        }
        let mut next = Vec::with_capacity(segs.len() + cuts.len());
        for ((s, _), mut c) in segs.into_iter().zip(cuts) {
            if c.is_empty() {
                next.push((s, false));
                continue;
            }
            let dir = sub(s.b, s.a);
            c.sort_unstable_by_key(|&p| (dot_v(sub(p, s.a), dir), p));
            c.dedup();
            let mut prev = s.a;
            for p in c.into_iter().chain([s.b]) {
                push_edge(&mut next, prev, p, s.w);
                prev = p;
            }
        }
        segs = merge_dups(next);
    }
    segs.into_iter().map(|(s, _)| s).collect()
}

// ----- classification -----

/// `x` of a non-horizontal edge at height `y`, as `num / den` with `den > 0`.
fn x_at(s: &Seg, y: i64) -> (i128, i128) {
    let dy = (s.b.1 - s.a.1) as i128;
    (s.a.0 as i128 * dy + (s.b.0 - s.a.0) as i128 * (y - s.a.1) as i128, dy)
}

/// Order of two edges spanning the beam `y0..y1` by their `x` at mid-height.
fn cmp_mid(s: &Seg, t: &Seg, y0: i64, y1: i64) -> Ordering {
    let mid = |s: &Seg| {
        let dy = (s.b.1 - s.a.1) as i128;
        (2 * s.a.0 as i128 * dy + (s.b.0 - s.a.0) as i128 * (y0 as i128 + y1 as i128 - 2 * s.a.1 as i128), 2 * dy)
    };
    let ((ns, ds), (nt, dt)) = (mid(s), mid(t));
    (ns * dt).cmp(&(nt * ds))
}

/// Winding numbers of both operands just left of every edge (above, for horizontal edges).
fn left_windings(segs: &[Seg]) -> Vec<[i32; 2]> {
    let mut ys: Vec<i64> = segs.iter().flat_map(|s| [s.a.1, s.b.1]).collect();
    ys.sort_unstable();
    ys.dedup();
    let (mut rising, mut flat): (Vec<usize>, Vec<usize>) = (0..segs.len()).partition(|&i| segs[i].a.1 != segs[i].b.1);
    rising.sort_unstable_by_key(|&i| segs[i].a.1);
    flat.sort_unstable_by_key(|&i| segs[i].a.1);
    let mut left = vec![[0; 2]; segs.len()];
    let mut active: Vec<usize> = Vec::new();
    let (mut ri, mut fi) = (0, 0);
    let mut prefix: Vec<[i32; 2]> = Vec::new();
    for (k, &y) in ys.iter().enumerate() {
        active.retain(|&i| segs[i].b.1 != y);
        let started = ri;
        while ri < rising.len() && segs[rising[ri]].a.1 == y {
            let (i, y1) = (rising[ri], ys[k + 1]);
            let pos = active.partition_point(|&j| cmp_mid(&segs[j], &segs[i], y, y1) == Ordering::Less);
            active.insert(pos, i);
            ri += 1;
        }
        let f0 = fi;
        while fi < flat.len() && segs[flat[fi]].a.1 == y {
            fi += 1;
        }
        if ri == started && fi == f0 {
            continue;
        }
        prefix.clear();
        let mut acc = [0i32; 2];
        prefix.push(acc);
        for &j in &active {
            acc = [acc[0] - segs[j].w[0], acc[1] - segs[j].w[1]];
            prefix.push(acc);
            if segs[j].a.1 == y {
                left[j] = prefix[prefix.len() - 2];
            }
        }
        for &h in &flat[f0..fi] {
            let s = &segs[h];
            let mx2 = (s.a.0 + s.b.0) as i128;
            let idx = active.partition_point(|&j| {
                let (n, d) = x_at(&segs[j], y);
                2 * n < mx2 * d
            });
            left[h] = prefix[idx];
        }
    }
    left
}

// ----- linking -----

/// Counter-clockwise angle order of directions `a` and `b`, measured from `r`.
fn ccw_from(r: Point, a: Point, b: Point) -> Ordering {
    let half = |d: Point| {
        let c = cross_v(r, d);
        u8::from(!(c > 0 || (c == 0 && dot_v(r, d) > 0)))
    };
    half(a).cmp(&half(b)).then_with(|| 0.cmp(&cross_v(a, b)))
}

fn simplify(mut pts: Vec<Point>) -> Vec<Point> {
    loop {
        let n = pts.len();
        if n < 3 {
            return pts;
        }
        let keep: Vec<bool> = (0..n)
            .map(|i| {
                let (p, c, q) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
                cross(p, c, q) != 0 || dot_v(sub(c, p), sub(q, c)) < 0
            })
            .collect();
        if keep.iter().all(|&k| k) {
            return pts;
        }
        pts = pts.into_iter().zip(keep).filter_map(|(p, k)| k.then_some(p)).collect();
    }
}

/// Rotate a ring to its lexicographically smallest rotation.
fn canonical(ring: Vec<Point>) -> Vec<Point> {
    let n = ring.len();
    let Some(&min) = ring.iter().min() else { return ring };
    let best = (0..n)
        .filter(|&i| ring[i] == min)
        .min_by(|&i, &j| (0..n).map(|k| ring[(i + k) % n]).cmp((0..n).map(|k| ring[(j + k) % n])))
        .unwrap_or(0);
    let mut r = ring;
    r.rotate_left(best);
    r
}

/// Nonzero winding number of `p` (in doubled coordinates) around `ring`.
fn winding2(ring: &[Point], p: Point) -> i32 {
    let n = ring.len();
    let mut w = 0;
    for i in 0..n {
        let (a, b) = ((2 * ring[i].0, 2 * ring[i].1), (2 * ring[(i + 1) % n].0, 2 * ring[(i + 1) % n].1));
        if a.1 <= p.1 {
            if b.1 > p.1 && cross(a, b, p) > 0 {
                w += 1;
            }
        } else if b.1 <= p.1 && cross(a, b, p) < 0 {
            w -= 1;
        }
    }
    w
}

/// Link directed boundary edges (inside on the left) into polygons with holes.
fn assemble(edges: &[(Point, Point)]) -> Vec<Polygon> {
    let mut out_at: HashMap<Point, Vec<usize>> = HashMap::new();
    for (k, e) in edges.iter().enumerate() {
        out_at.entry(e.0).or_default().push(k);
    }
    let next: Vec<usize> = edges
        .iter()
        .enumerate()
        .map(|(k, &(u, v))| {
            let r = sub(u, v);
            out_at
                .get(&v)
                .and_then(|outs| outs.iter().copied().max_by(|&x, &y| ccw_from(r, sub(edges[x].1, v), sub(edges[y].1, v))))
                .unwrap_or(k)
        })
        .collect();
    let mut used = vec![false; edges.len()];
    let (mut outers, mut holes) = (Vec::new(), Vec::new());
    for s in 0..edges.len() {
        let mut ring = Vec::new();
        let mut e = s;
        while !used[e] {
            used[e] = true;
            ring.push(edges[e].0);
            e = next[e];
        }
        let ring = simplify(ring);
        if ring.len() < 3 {
            continue;
        }
        match ring_area2(&ring).cmp(&0) {
            Ordering::Greater => outers.push(canonical(ring)),
            Ordering::Less => holes.push(canonical(ring)),
            Ordering::Equal => {}
        }
    }
    let areas: Vec<i128> = outers.iter().map(|r| ring_area2(r)).collect();
    let boxes: Vec<IRect> = outers.iter().map(|r| ring_bbox(r)).collect();
    let mut polys: Vec<Polygon> = outers.into_iter().map(|outer| Polygon { outer, holes: vec![] }).collect();
    for h in holes {
        let p = (h[0].0 + h[1].0, h[0].1 + h[1].1);
        let parent = (0..polys.len())
            .filter(|&k| {
                let b = &boxes[k];
                2 * b.x0 <= p.0 && p.0 <= 2 * b.x1 && 2 * b.y0 <= p.1 && p.1 <= 2 * b.y1
            })
            .filter(|&k| winding2(&polys[k].outer, p) != 0)
            .min_by_key(|&k| areas[k]);
        if let Some(k) = parent {
            polys[k].holes.push(h);
        }
    }
    for p in &mut polys {
        p.holes.sort();
    }
    polys.sort();
    polys
}

/// The Boolean combination of two ring sets (each under the nonzero rule): state
/// `in_a | in_b << 1` belongs to the result when bit `state` of `mask` is set.
fn build(a: &[&[Point]], b: &[&[Point]], mask: u8) -> PolySet {
    let mut raw = Vec::new();
    for (k, rings) in [a, b].into_iter().enumerate() {
        let mut w = [0; 2];
        w[k] = 1;
        for r in rings {
            let n = r.len();
            if n < 3 {
                continue;
            }
            for i in 0..n {
                push_edge(&mut raw, r[i], r[(i + 1) % n], w);
            }
        }
    }
    let segs = node(raw);
    let left = left_windings(&segs);
    let inside = |w: [i32; 2]| mask >> (u8::from(w[0] != 0) | u8::from(w[1] != 0) << 1) & 1 == 1;
    let mut edges = Vec::new();
    for (s, l) in segs.iter().zip(left) {
        let r = [l[0] - s.w[0], l[1] - s.w[1]];
        match (inside(l), inside(r)) {
            (true, false) => edges.push((s.a, s.b)),
            (false, true) => edges.push((s.b, s.a)),
            _ => {}
        }
    }
    PolySet { polys: assemble(&edges) }
}

// ----- sizing -----

fn hull(mut pts: Vec<Point>) -> Vec<Point> {
    pts.sort_unstable();
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let mut h: Vec<Point> = Vec::with_capacity(pts.len() + 1);
    for pass in [&pts[..], &pts.iter().rev().copied().collect::<Vec<_>>()[..]] {
        let start = h.len();
        for &p in pass {
            while h.len() >= start + 2 && cross(h[h.len() - 2], h[h.len() - 1], p) <= 0 {
                h.pop();
            }
            h.push(p);
        }
        h.pop();
    }
    h
}

/// Triangles filling the gap between the octagon sum and the miter at a convex corner `v`
/// (from `u`, towards `w`) of at least 90°.
fn miter_patch(u: Point, v: Point, w: Point, d: i64, k: &[Point]) -> [Vec<Point>; 2] {
    let (e1, e2) = (sub(v, u), sub(w, v));
    if cross_v(e1, e2) <= 0 || dot_v(e1, e2) < 0 {
        return [vec![], vec![]];
    }
    let (n1, n2) = ((e1.1, -e1.0), (e2.1, -e2.0));
    let s1 = k.iter().copied().max_by_key(|&o| (dot_v(o, n1), dot_v(o, e1))).unwrap_or_default();
    let s2 = k.iter().copied().max_by_key(|&o| (dot_v(o, n2), -dot_v(o, e2))).unwrap_or_default();
    let unit = |n: Point| {
        let l = (n.0 as f64).hypot(n.1 as f64);
        (n.0 as f64 / l, n.1 as f64 / l)
    };
    let (a, b) = (unit(n1), unit(n2));
    let f = d as f64 / (1.0 + a.0 * b.0 + a.1 * b.1);
    let m = (v.0 + ((a.0 + b.0) * f).round() as i64, v.1 + ((a.1 + b.1) * f).round() as i64);
    [ccw(vec![v, add(v, s1), m]), ccw(vec![v, m, add(v, s2)])]
}

fn grow(p: &PolySet, d: i64) -> PolySet {
    let a = (d as f64 * (SQRT_2 - 1.0)).round() as i64;
    let k = [(d, -a), (d, a), (a, d), (-a, d), (-d, a), (-d, -a), (-a, -d), (a, -d)];
    let mut out: Vec<Vec<Point>> = Vec::new();
    for poly in &p.polys {
        let mut rings: Vec<Vec<Point>> = vec![poly.outer.clone()];
        rings.extend(poly.holes.iter().cloned());
        for ring in std::iter::once(&poly.outer).chain(&poly.holes) {
            let n = ring.len();
            for i in 0..n {
                let (u, v, w) = (ring[i], ring[(i + 1) % n], ring[(i + 2) % n]);
                rings.push(hull(k.iter().flat_map(|&o| [add(u, o), add(v, o)]).collect()));
                rings.extend(miter_patch(u, v, w, d, &k));
            }
        }
        out.extend(from_rings(rings).rings().map(<[Point]>::to_vec));
    }
    from_rings(out)
}

// ----- hole-free output -----

fn split_holes(p: &Polygon, phase: u8, out: &mut Vec<Vec<Point>>) {
    if p.holes.is_empty() {
        out.push(p.outer.clone());
        return;
    }
    if phase >= 4 {
        out.push(keyhole(p));
        return;
    }
    let horizontal = phase % 2 == 1;
    let c = |q: Point| if horizontal { q.1 } else { q.0 };
    let mut cuts = Vec::new();
    for h in &p.holes {
        let (lo, hi) = (h.iter().map(|&q| c(q)).min().unwrap_or(0), h.iter().map(|&q| c(q)).max().unwrap_or(0));
        let n = h.len();
        let edge_at = |x: i64| (0..n).any(|i| c(h[i]) == x && c(h[(i + 1) % n]) == x);
        if hi - lo >= 2 {
            cuts.push((lo + hi).div_euclid(2));
        } else if edge_at(lo) {
            cuts.push(lo);
        } else if edge_at(hi) {
            cuts.push(hi);
        }
    }
    if cuts.is_empty() {
        split_holes(p, phase + 1, out);
        return;
    }
    cuts.sort_unstable();
    cuts.dedup();
    let bb = ring_bbox(&p.outer);
    let (lo, hi) = if horizontal { (bb.y0 - 1, bb.y1 + 1) } else { (bb.x0 - 1, bb.x1 + 1) };
    let me = PolySet { polys: vec![p.clone()] };
    let bounds: Vec<i64> = std::iter::once(lo).chain(cuts).chain([hi]).collect();
    for w in bounds.windows(2) {
        let slab =
            if horizontal { IRect::new(bb.x0 - 1, w[0], bb.x1 + 1, w[1]) } else { IRect::new(w[0], bb.y0 - 1, w[1], bb.y1 + 1) };
        for q in me.boolean(&PolySet::from_rect(slab), BoolOp::And).polys {
            split_holes(&q, phase + 1, out);
        }
    }
}

/// Last resort: join each hole to the nearest outline vertex with a zero-width slit.
fn keyhole(p: &Polygon) -> Vec<Point> {
    let mut ring = p.outer.clone();
    for h in &p.holes {
        let Some((hi, &hv)) = h.iter().enumerate().min_by_key(|&(_, p)| *p) else { continue };
        let dist = |q: Point| dot_v(sub(q, hv), sub(q, hv));
        let Some(oi) = (0..ring.len()).min_by_key(|&i| dist(ring[i])) else { continue };
        let ov = ring[oi];
        let mut ins: Vec<Point> = (0..=h.len()).map(|k| h[(hi + k) % h.len()]).collect();
        ins.push(ov);
        ring.splice(oi + 1..oi + 1, ins);
    }
    ring
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.0 >> 33
        }
        fn range(&mut self, lo: i64, hi: i64) -> i64 {
            lo + (self.next() % (hi - lo + 1) as u64) as i64
        }
    }

    fn poly(pts: &[Point]) -> PolySet {
        PolySet::from_polygon(pts)
    }

    fn rect(x0: i64, y0: i64, x1: i64, y1: i64) -> PolySet {
        PolySet::from_rect(IRect::new(x0, y0, x1, y1))
    }

    /// Star-shaped (hence simple) polygon with `n` vertices around `c`.
    fn star(rng: &mut Lcg, c: Point, rmax: i64, n: usize) -> Vec<Point> {
        let mut angles: Vec<f64> = (0..n).map(|_| rng.next() as f64 / (1u64 << 31) as f64 * std::f64::consts::TAU).collect();
        angles.sort_by(f64::total_cmp);
        angles
            .into_iter()
            .map(|a| {
                let r = rng.range(rmax / 4, rmax) as f64;
                (c.0 + (r * a.cos()).round() as i64, c.1 + (r * a.sin()).round() as i64)
            })
            .collect()
    }

    /// Random polygon with arbitrary (possibly self-intersecting) outline.
    fn scribble(rng: &mut Lcg, n: usize, span: i64) -> Vec<Point> {
        (0..n).map(|_| (rng.range(0, span), rng.range(0, span))).collect()
    }

    /// Random octilinear polygon on an even grid: a Manhattan staircase with chamfered corners.
    fn octilinear(rng: &mut Lcg) -> Vec<Point> {
        let (x0, y0) = (2 * rng.range(0, 20), 2 * rng.range(0, 20));
        let (w, h) = (2 * rng.range(6, 30), 2 * rng.range(6, 30));
        let c = [2 * rng.range(0, 2), 2 * rng.range(0, 2), 2 * rng.range(0, 2), 2 * rng.range(0, 2)];
        let mut pts = vec![
            (x0 + c[0], y0),
            (x0 + w - c[1], y0),
            (x0 + w, y0 + c[1]),
            (x0 + w, y0 + h - c[2]),
            (x0 + w - c[2], y0 + h),
            (x0 + c[3], y0 + h),
            (x0, y0 + h - c[3]),
            (x0, y0 + c[0]),
        ];
        pts.dedup();
        pts
    }

    fn point_in(set: &PolySet, x: f64, y: f64) -> bool {
        let wind = |r: &[Point]| {
            let n = r.len();
            let mut w = 0;
            for i in 0..n {
                let (a, b) = (r[i], r[(i + 1) % n]);
                let (ax, ay, bx, by) = (a.0 as f64, a.1 as f64, b.0 as f64, b.1 as f64);
                let c = (bx - ax) * (y - ay) - (by - ay) * (x - ax);
                if ay <= y {
                    if by > y && c > 0.0 {
                        w += 1;
                    }
                } else if by <= y && c < 0.0 {
                    w -= 1;
                }
            }
            w
        };
        set.rings().map(wind).sum::<i32>() != 0
    }

    fn near_edge(rings: &[&[Point]], x: f64, y: f64, tol: f64) -> bool {
        rings.iter().any(|r| {
            let n = r.len();
            (0..n).any(|i| {
                let (a, b) = (r[i], r[(i + 1) % n]);
                let (ax, ay, bx, by) = (a.0 as f64, a.1 as f64, b.0 as f64, b.1 as f64);
                let (dx, dy) = (bx - ax, by - ay);
                let l2 = dx * dx + dy * dy;
                let t = if l2 == 0.0 { 0.0 } else { (((x - ax) * dx + (y - ay) * dy) / l2).clamp(0.0, 1.0) };
                let (px, py) = (ax + t * dx - x, ay + t * dy - y);
                px * px + py * py < tol * tol
            })
        })
    }

    fn perimeter(s: &PolySet) -> i64 {
        s.rings()
            .map(|r| {
                (0..r.len()).map(|i| sub(r[(i + 1) % r.len()], r[i])).map(|d| (d.0 as f64).hypot(d.1 as f64) as i64).sum::<i64>()
            })
            .sum()
    }

    fn check_valid(s: &PolySet) {
        for p in s.polygons() {
            assert!(ring_area2(&p.outer) > 0, "outer not ccw: {p:?}");
            for h in &p.holes {
                assert!(ring_area2(h) < 0, "hole not cw: {p:?}");
            }
            for r in std::iter::once(&p.outer).chain(&p.holes) {
                assert_eq!(simplify(r.clone()).len(), r.len(), "collinear vertex in {r:?}");
            }
        }
    }

    /// Point samples away from all input edges agree with the Boolean of the inputs.
    fn check_samples(rng: &mut Lcg, a: &PolySet, b: &PolySet, span: i64) {
        let rings: Vec<&[Point]> = a.rings().chain(b.rings()).collect();
        let results: Vec<(BoolOp, PolySet)> =
            [BoolOp::And, BoolOp::Or, BoolOp::Not, BoolOp::Xor].into_iter().map(|op| (op, a.boolean(b, op))).collect();
        for (_, r) in &results {
            check_valid(r);
        }
        for _ in 0..300 {
            let x = rng.range(-10, span * 10 + 10) as f64 / 10.0 + 0.031;
            let y = rng.range(-10, span * 10 + 10) as f64 / 10.0 + 0.017;
            if near_edge(&rings, x, y, 1.5) {
                continue;
            }
            let (ia, ib) = (point_in(a, x, y), point_in(b, x, y));
            for (op, r) in &results {
                let want = match op {
                    BoolOp::And => ia && ib,
                    BoolOp::Or => ia || ib,
                    BoolOp::Not => ia && !ib,
                    BoolOp::Xor => ia != ib,
                };
                assert_eq!(point_in(r, x, y), want, "{op:?} at ({x}, {y})\na = {a:?}\nb = {b:?}\nr = {r:?}");
            }
        }
    }

    fn identities_exact(a: &PolySet, b: &PolySet) {
        let and = a.boolean(b, BoolOp::And);
        let or = a.boolean(b, BoolOp::Or);
        let xor = a.boolean(b, BoolOp::Xor);
        let anb = a.boolean(b, BoolOp::Not);
        let bna = b.boolean(a, BoolOp::Not);
        assert_eq!(or.area2(), a.area2() + b.area2() - and.area2(), "a = {a:?}\nb = {b:?}");
        assert_eq!(xor.area2(), or.area2() - and.area2());
        assert_eq!(anb.area2(), a.area2() - and.area2());
        assert_eq!(xor, anb.boolean(&bna, BoolOp::Or));
        assert_eq!(xor, or.boolean(&and, BoolOp::Not));
        assert_eq!(or, b.boolean(a, BoolOp::Or));
        assert_eq!(and, b.boolean(a, BoolOp::And));
        assert!(anb.boolean(b, BoolOp::And).is_empty());
    }

    #[test]
    fn triangles() {
        let a = poly(&[(0, 0), (10, 0), (0, 10)]);
        let b = poly(&[(0, 0), (10, 10), (0, 10)]);
        assert_eq!(a.area2(), 100);
        assert_eq!(a.boolean(&b, BoolOp::And), poly(&[(0, 0), (5, 5), (0, 10)]));
        assert_eq!(a.boolean(&b, BoolOp::Or).polygons()[0].outer.len(), 5);
        assert_eq!(a.boolean(&b, BoolOp::Or).area2(), 150);
        assert_eq!(a.boolean(&b, BoolOp::Xor).polygons().len(), 2);
        assert_eq!(a.boolean(&b, BoolOp::Not), poly(&[(0, 0), (10, 0), (5, 5)]));
        identities_exact(&a, &b);
        assert_eq!(poly(&[(0, 10), (10, 0), (0, 0)]), a);
        assert_eq!(a.boolean(&PolySet::default(), BoolOp::Or), a);
        assert!(a.boolean(&a, BoolOp::Xor).is_empty());
    }

    #[test]
    fn diamond_and_square() {
        let d = poly(&[(10, 0), (20, 10), (10, 20), (0, 10)]);
        let s = rect(0, 0, 20, 20);
        assert_eq!(d.boolean(&s, BoolOp::And), d);
        assert_eq!(d.boolean(&s, BoolOp::Or), s);
        let corners = s.boolean(&d, BoolOp::Not);
        assert_eq!(corners.polygons().len(), 4);
        assert_eq!(corners.area2(), 2 * 200);
        identities_exact(&d, &s);
        let shifted = d.translated(10, 0);
        let and = d.boolean(&shifted, BoolOp::And);
        assert_eq!(and, poly(&[(15, 5), (20, 10), (15, 15), (10, 10)]));
        identities_exact(&d, &shifted);
    }

    #[test]
    fn self_intersecting_and_orientation() {
        let bow = poly(&[(0, 0), (10, 10), (10, 0), (0, 10)]);
        assert_eq!(bow.polygons().len(), 2);
        assert_eq!(bow.area2(), 100);
        let fig8 = poly(&[(0, 0), (4, 0), (4, 4), (8, 4), (8, 8), (4, 8), (4, 4), (0, 4)]);
        assert_eq!(fig8.polygons().len(), 2);
        assert_eq!(fig8.area2(), 64);
        let cw = PolySet::from_polygon(&[(0, 0), (0, 5), (5, 5), (5, 0)]);
        assert_eq!(cw, rect(0, 0, 5, 5));
        let twice = PolySet::from_geometries(&[
            Geometry::Polygon { points: vec![(0, 0), (0, 5), (5, 5), (5, 0)] },
            Geometry::Polygon { points: vec![(0, 0), (5, 0), (5, 5), (0, 5)] },
        ]);
        assert_eq!(twice, rect(0, 0, 5, 5));
        assert_eq!(poly(&[(0, 0), (5, 0), (5, 0), (10, 0), (10, 5), (0, 5), (0, 0)]), rect(0, 0, 10, 5));
        assert!(poly(&[(0, 0), (5, 5), (10, 10)]).is_empty());
    }

    #[test]
    fn collinear_and_touching() {
        let a = rect(0, 0, 10, 10);
        let b = rect(10, 0, 20, 10);
        assert_eq!(a.boolean(&b, BoolOp::Or), rect(0, 0, 20, 10));
        assert!(a.boolean(&b, BoolOp::And).is_empty());
        let c = rect(10, 10, 20, 20);
        let corner = a.boolean(&c, BoolOp::Or);
        assert_eq!(corner.polygons().len(), 2, "corner-touching squares stay apart");
        let point = poly(&[(10, 5), (20, 0), (20, 10)]);
        assert_eq!(a.boolean(&point, BoolOp::Or).polygons().len(), 2);
        let tri = poly(&[(10, 0), (20, 5), (10, 10)]);
        let t = a.boolean(&tri, BoolOp::Or);
        assert_eq!(t.polygons().len(), 1);
        assert_eq!(t.polygons()[0].outer.len(), 5);
        assert_eq!(t.area2(), 200 + 100);
        let overlap = poly(&[(5, 0), (15, 0), (15, 4)]);
        identities_exact(&a, &overlap);
        identities_exact(&a, &tri);
        let along = poly(&[(0, 0), (10, 10), (20, 0)]);
        identities_exact(&along, &a);
    }

    #[test]
    fn holes() {
        let frame = rect(0, 0, 30, 30).boolean(&poly(&[(15, 5), (25, 15), (15, 25), (5, 15)]), BoolOp::Not);
        assert_eq!(frame.polygons().len(), 1);
        assert_eq!(frame.polygons()[0].holes.len(), 1);
        assert_eq!(frame.area2(), 2 * 900 - 2 * 200);
        let island = rect(13, 13, 17, 17);
        let both = frame.boolean(&island, BoolOp::Or);
        assert_eq!(both.polygons().len(), 2);
        assert_eq!(both.area2(), frame.area2() + 32);
        identities_exact(&frame, &island);
        let pieces = frame.to_polygons();
        assert!(pieces.len() >= 2);
        let back = PolySet::from_geometries(&pieces.iter().map(|p| Geometry::Polygon { points: p.clone() }).collect::<Vec<_>>());
        assert_eq!(back, frame);
        for p in pieces {
            assert!(ring_area2(&p) > 0);
        }
        let touching = rect(0, 0, 10, 10).boolean(&poly(&[(0, 5), (5, 2), (5, 8)]), BoolOp::Not);
        assert_eq!(touching.polygons().len(), 1);
        assert_eq!(touching.area2(), 200 - 30);
        let pinch = rect(0, 0, 10, 10).boolean(&poly(&[(2, 5), (5, 2), (8, 5), (5, 8)]), BoolOp::Not);
        let pinch = pinch.boolean(&poly(&[(0, 0), (5, 2), (2, 5)]), BoolOp::Not);
        assert_eq!(pinch.area2(), 200 - 36 - 21);
        let back = PolySet::from_geometries(&pinch.to_geometry());
        assert_eq!(back, pinch);
    }

    #[test]
    fn manhattan_matches_region() {
        let mut rng = Lcg(7);
        for _ in 0..60 {
            let rects = |rng: &mut Lcg| -> Vec<IRect> {
                (0..rng.range(1, 8))
                    .map(|_| {
                        let (x, y) = (rng.range(0, 40), rng.range(0, 40));
                        IRect::new(x, y, x + rng.range(1, 15), y + rng.range(1, 15))
                    })
                    .collect()
            };
            let (ra, rb) = (Region::from_rects(rects(&mut rng)), Region::from_rects(rects(&mut rng)));
            let (pa, pb) = (PolySet::from_region(&ra), PolySet::from_region(&rb));
            assert_eq!(pa.area2(), 2 * ra.area());
            assert_eq!(pa.to_region().as_ref(), Some(&ra));
            for op in [BoolOp::And, BoolOp::Or, BoolOp::Not, BoolOp::Xor] {
                let want = ra.boolean(&rb, op);
                let got = pa.boolean(&pb, op);
                assert_eq!(got, PolySet::from_region(&want), "{op:?}");
                assert_eq!(got.to_region(), Some(want));
            }
            for d in [1, 3, -1, -2, 5] {
                assert_eq!(pa.sized(d).to_region(), Some(ra.sized(d)), "size {d} of {ra:?}");
            }
            assert_eq!(PolySet::from_geometries(&ra.to_geometry()), pa);
            assert_eq!(Region::from_geometries(&pa.to_geometry()).unwrap(), ra);
        }
    }

    #[test]
    fn random_octilinear_exact() {
        let mut rng = Lcg(11);
        for _ in 0..80 {
            let a = PolySet::from_geometries(
                &(0..rng.range(1, 3)).map(|_| Geometry::Polygon { points: octilinear(&mut rng) }).collect::<Vec<_>>(),
            );
            let b = PolySet::from_geometries(
                &(0..rng.range(1, 3)).map(|_| Geometry::Polygon { points: octilinear(&mut rng) }).collect::<Vec<_>>(),
            );
            identities_exact(&a, &b);
            check_samples(&mut rng, &a, &b, 100);
        }
    }

    #[test]
    fn random_any_angle() {
        let mut rng = Lcg(3);
        for i in 0..120 {
            let (a, b) = if i % 2 == 0 {
                (poly(&star(&mut rng, (500, 500), 450, 12)), poly(&star(&mut rng, (600, 450), 400, 9)))
            } else {
                (poly(&scribble(&mut rng, 7, 1000)), poly(&scribble(&mut rng, 6, 1000)))
            };
            check_samples(&mut rng, &a, &b, 1000);
            let and = a.boolean(&b, BoolOp::And);
            let or = a.boolean(&b, BoolOp::Or);
            let slack = 6 * (perimeter(&a) + perimeter(&b)) as i128;
            assert!((or.area2() + and.area2() - a.area2() - b.area2()).abs() < slack, "{a:?} {b:?}");
            let xor = a.boolean(&b, BoolOp::Xor);
            assert!((xor.area2() - (or.area2() - and.area2())).abs() < slack);
            if i % 10 == 0 {
                let (big, small) = (a.sized(8), a.sized(-8));
                check_valid(&big);
                check_valid(&small);
                assert!(a.boolean(&big, BoolOp::Not).is_empty());
                assert!(small.boolean(&a, BoolOp::Not).is_empty());
                assert!(big.area2() > a.area2() && small.area2() < a.area2());
            }
        }
    }

    #[test]
    fn many_shapes_merge() {
        let mut rng = Lcg(5);
        let geoms: Vec<Geometry> = (0..40)
            .map(|_| {
                let c = (rng.range(0, 2000), rng.range(0, 2000));
                Geometry::Polygon { points: star(&mut rng, c, 200, 7) }
            })
            .collect();
        let merged = PolySet::from_geometries(&geoms);
        check_valid(&merged);
        let one_by_one = geoms.iter().fold(PolySet::default(), |acc, g| acc.boolean(&PolySet::from_geometry(g), BoolOp::Or));
        assert!((merged.area2() - one_by_one.area2()).abs() < merged.area2() / 1000);
        let inputs: Vec<PolySet> = geoms.iter().map(PolySet::from_geometry).collect();
        let rings: Vec<&[Point]> = inputs.iter().flat_map(|s| s.rings()).collect();
        for _ in 0..500 {
            let (x, y) = (rng.range(0, 2000) as f64 + 0.37, rng.range(0, 2000) as f64 + 0.61);
            if near_edge(&rings, x, y, 1.5) {
                continue;
            }
            let want = inputs.iter().any(|s| point_in(s, x, y));
            assert_eq!(point_in(&merged, x, y), want);
        }
    }

    #[test]
    fn paths() {
        let p = PolySet::from_geometry(&Geometry::Path { points: vec![(0, 0), (100, 0), (100, 50)], width: 10 });
        assert_eq!(
            p.to_region(),
            Region::from_geometry(&Geometry::Path { points: vec![(0, 0), (100, 0), (100, 50)], width: 10 }).ok()
        );
        let diag = PolySet::from_geometry(&Geometry::Path { points: vec![(0, 0), (100, 100), (200, 100)], width: 20 });
        check_valid(&diag);
        assert_eq!(diag.polygons().len(), 1);
        assert!(diag.polygons()[0].holes.is_empty());
        let len = 100.0 * SQRT_2 + 100.0 + 20.0;
        let area = diag.area();
        assert!((area - len * 20.0).abs() < 0.06 * len * 20.0, "{area}");
        assert!(point_in(&diag, 100.0, 109.0));
        assert!(point_in(&diag, -5.0, 0.0));
        assert!(!point_in(&diag, 100.0, 112.0));
        assert!(PolySet::from_geometry(&Geometry::Label { text: "x".into(), at: (0, 0), size: 5 }).is_empty());
    }

    #[test]
    fn octilinear_sizing() {
        let diamond = poly(&[(0, -100), (100, 0), (0, 100), (-100, 0)]);
        let g = diamond.sized(10);
        check_valid(&g);
        let r = 100.0 + 10.0 * SQRT_2;
        assert!((g.area() - 2.0 * r * r).abs() < 2.0 * r * 4.0, "{} vs {}", g.area(), 2.0 * r * r);
        assert_eq!(g.polygons()[0].outer.len(), 4, "90° corners of a diamond are mitered: {g:?}");
        let s = g.sized(-10);
        assert!((s.area() - diamond.area()).abs() < 800.0, "{}", s.area());
        let oct = poly(&[(20, 0), (80, 0), (100, 20), (100, 80), (80, 100), (20, 100), (0, 80), (0, 20)]);
        let big = oct.sized(10);
        assert_eq!(big.polygons()[0].outer.len(), 8);
        assert!(big.polygons()[0].outer.iter().all(|&(x, y)| (-10..=110).contains(&x) && (-10..=110).contains(&y)));
        assert!(big.polygons()[0].outer.contains(&(-10, 16)), "{big:?}");
        let small = oct.sized(-10);
        assert_eq!(small.polygons()[0].outer.len(), 8);
        assert!(small.polygons()[0].outer.contains(&(10, 24)), "{small:?}");
        assert!(small.polygons()[0].outer.iter().all(|&(x, y)| (10..=90).contains(&x) && (10..=90).contains(&y)));
        assert!(oct.sized(-60).is_empty());
        let notch = poly(&[(0, 0), (100, 0), (100, 100), (50, 50), (0, 100)]);
        let grown = notch.sized(5);
        assert!(grown.boolean(&notch, BoolOp::Not).area() > 0.0);
        assert!(notch.boolean(&grown, BoolOp::Not).is_empty());
        assert!(notch.sized(-5).boolean(&notch, BoolOp::Not).is_empty());
    }

    #[test]
    fn geometry_output() {
        let a = rect(0, 0, 10, 10).boolean(&poly(&[(5, 5), (15, 5), (15, 15)]), BoolOp::Or);
        let g = a.to_geometry();
        assert_eq!(g.len(), 1);
        assert!(matches!(&g[0], Geometry::Polygon { points } if points.len() == 7));
        assert!(!a.is_manhattan());
        assert!(a.to_region().is_none());
        assert_eq!(rect(0, 0, 4, 4).to_geometry(), vec![Geometry::Rect(IRect::new(0, 0, 4, 4))]);
        assert!(is_manhattan(&Geometry::Path { points: vec![(0, 0), (0, 5), (3, 5)], width: 2 }));
        assert!(!is_manhattan(&Geometry::Polygon { points: vec![(0, 0), (5, 0), (0, 5)] }));
        assert_eq!(a.bbox(), Some(IRect::new(0, 0, 15, 15)));
    }

    #[test]
    fn snapped_crossings() {
        let a = poly(&[(0, 0), (7, 3), (0, 3)]);
        let b = poly(&[(0, 1), (5, 0), (5, 2)]);
        for op in [BoolOp::And, BoolOp::Or, BoolOp::Not, BoolOp::Xor] {
            check_valid(&a.boolean(&b, op));
        }
        let mut rng = Lcg(19);
        for _ in 0..200 {
            let a = poly(&scribble(&mut rng, 5, 12));
            let b = poly(&scribble(&mut rng, 5, 12));
            for op in [BoolOp::And, BoolOp::Or, BoolOp::Not, BoolOp::Xor] {
                let r = a.boolean(&b, op);
                check_valid(&r);
                assert!((PolySet::from_geometries(&r.to_geometry()).area2() - r.area2()).abs() <= 100);
            }
        }
    }
}
