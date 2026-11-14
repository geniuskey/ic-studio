//! Exact checks near any-angle geometry.
//!
//! The Manhattan engine sees an any-angle shape as a staircase of [`STAIR`] high bands, which
//! is close enough for areas, connectivity and selections but not for distances. Around such
//! shapes, `width`, `space`, `separation`, `enclosure` and `extension` are recomputed on the
//! exact polygons ([`PolySet`]) and replace the staircase result. Coordinates are scaled by
//! [`S`] so that thresholds between grid points are integer offsets. Facing edges are measured
//! along their normals, corners in the Euclidean metric, as the Manhattan rules do. Integer
//! distances give the Manhattan verdict (below MIN fails); any-angle distances within 1 dbu
//! of MIN pass, since their vertices are rounded to the grid. Pieces thinner than half a dbu
//! (snap rounding of any-angle offsets) are dropped.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};

use rstar::primitives::{GeomWithData, Rectangle};
use rstar::{AABB, RTree};

use super::{Deck, LayerExpr, Rule, grown, split_overlapping};
use crate::geom::IRect;
use crate::layout::Geometry;
use crate::polybool::{Point, PolySet, Polygon};
use crate::region::{BoolOp, Region};

/// Band height of the staircase standing in for any-angle shapes, in dbu.
pub(super) const STAIR: i64 = 10;
/// Scale of the exact computation.
const S: i64 = 4;

/// Any-angle shapes of the checked layers and the Manhattan-only geometry of those layers.
#[derive(Default)]
pub(super) struct AnyAngle {
    pub shapes: Vec<(usize, Geometry)>,
    pub manhattan: HashMap<usize, Region>,
}

/// The result of `rule`, exact near any-angle shapes.
///
/// `base(manhattan_only)` computes the staircase result; local rules on technology layers are
/// replaced wherever any-angle shapes could matter, so they skip the staircase.
pub(super) fn refine(
    deck: &Deck,
    n: usize,
    regions: &[Region],
    any: &AnyAngle,
    rule: &Rule,
    base: impl FnOnce(bool) -> Region,
) -> Region {
    if any.shapes.is_empty()
        || !matches!(
            rule,
            Rule::Width { .. } | Rule::Space { .. } | Rule::Separation { .. } | Rule::Enclosure { .. } | Rule::Extension { .. }
        )
    {
        return base(false);
    }
    let mut feeds = BTreeSet::new();
    if !rule.layers().into_iter().all(|l| exact_inputs(deck, n, l, &mut feeds)) {
        return base(false);
    }
    let growth: i64 = deck.derived.iter().map(|d| if let LayerExpr::Size { by, .. } = d.expr { by.abs() } else { 0 }).sum();
    let reach = rule.reach() + growth + 2 * STAIR + 2;
    let near: Vec<IRect> = any.shapes.iter().filter(|(l, _)| feeds.contains(l)).map(|(_, g)| grown(g.bbox(), reach)).collect();
    if near.is_empty() {
        return base(false);
    }
    let local =
        matches!(rule, Rule::Width { .. } | Rule::Space { .. } | Rule::Extension { .. }) && rule.layers().iter().all(|&l| l < n);
    let base = base(local);
    let zone = Region::from_rects(near);
    let mut out = base.boolean(&zone, BoolOp::Not).rects().to_vec();
    for part in zone.parts() {
        let Some(zb) = part.bbox() else { continue };
        let ctx = Ctx { deck, n, regions, any, win: grown(zb, reach), cache: RefCell::new(HashMap::new()) };
        let exact = ctx.check(rule);
        out.extend_from_slice(unscale(&exact).boolean(&part, BoolOp::And).rects());
    }
    Region::from_rects(out)
}

/// Whether layer `l` can be computed exactly; collects the technology layers it reads.
fn exact_inputs(deck: &Deck, n: usize, l: usize, feeds: &mut BTreeSet<usize>) -> bool {
    if l < n {
        feeds.insert(l);
        return true;
    }
    match deck.derived.get(l - n).map(|d| &d.expr) {
        Some(LayerExpr::Bool { a, b, .. }) => exact_inputs(deck, n, *a, feeds) && exact_inputs(deck, n, *b, feeds),
        Some(LayerExpr::Size { a, .. }) => exact_inputs(deck, n, *a, feeds),
        Some(LayerExpr::Union(v)) => v.iter().all(|&a| exact_inputs(deck, n, a, feeds)),
        _ => false,
    }
}

fn scale_geom(g: &Geometry) -> Option<Geometry> {
    let pts = |p: &[(i64, i64)]| p.iter().map(|&(x, y)| (x * S, y * S)).collect();
    Some(match g {
        Geometry::Rect(r) => Geometry::Rect(IRect::new(r.x0 * S, r.y0 * S, r.x1 * S, r.y1 * S)),
        Geometry::Polygon { points } => Geometry::Polygon { points: pts(points) },
        Geometry::Path { points, width } => Geometry::Path { points: pts(points), width: width * S },
        Geometry::Label { .. } => return None,
    })
}

/// A scaled result as a dbu region (a staircase covering it), without sub-dbu slivers.
fn unscale(p: &PolySet) -> Region {
    let thick: Vec<Vec<Point>> = p
        .polygons()
        .iter()
        .filter(|q| {
            let perim: f64 = rings(q).map(|r| edges(r).map(|(a, b)| len(a, b)).sum::<f64>()).sum();
            perim > 0.0 && 2.0 * (q.area2() as f64 / 2.0) / perim >= S as f64 / 2.0
        })
        .flat_map(|q| std::iter::once(q.outer.clone()).chain(q.holes.iter().cloned()))
        .collect();
    let stair = PolySet::from_outlines(&thick).manhattanize(S * STAIR);
    Region::from_rects(
        stair.rects().iter().map(|r| {
            IRect::new(r.x0.div_euclid(S), r.y0.div_euclid(S), (r.x1 + S - 1).div_euclid(S), (r.y1 + S - 1).div_euclid(S))
        }),
    )
}

struct Ctx<'a> {
    deck: &'a Deck,
    n: usize,
    regions: &'a [Region],
    any: &'a AnyAngle,
    /// Window (dbu) whose geometry is read.
    win: IRect,
    cache: RefCell<HashMap<usize, PolySet>>,
}

impl Ctx<'_> {
    fn win_s(&self) -> PolySet {
        let w = self.win;
        PolySet::from_rect(IRect::new(w.x0 * S, w.y0 * S, w.x1 * S, w.y1 * S))
    }

    /// Layer `l` in scaled coordinates, clipped to the window.
    fn layer(&self, l: usize) -> PolySet {
        if let Some(p) = self.cache.borrow().get(&l) {
            return p.clone();
        }
        let p = if l < self.n {
            let man = self.any.manhattan.get(&l).or(self.regions.get(l));
            let clip = Region::from_rects([self.win]);
            let mut geoms: Vec<Geometry> = man
                .map(|r| r.boolean(&clip, BoolOp::And).rects().iter().filter_map(|&r| scale_geom(&Geometry::Rect(r))).collect())
                .unwrap_or_default();
            geoms.extend(
                self.any
                    .shapes
                    .iter()
                    .filter(|(k, g)| *k == l && g.bbox().intersects(&self.win))
                    .filter_map(|(_, g)| scale_geom(g)),
            );
            PolySet::from_geometries(&geoms).boolean(&self.win_s(), BoolOp::And)
        } else {
            match &self.deck.derived[l - self.n].expr {
                LayerExpr::Bool { a, op, b } => self.layer(*a).boolean(&self.layer(*b), *op),
                LayerExpr::Size { a, by } => self.layer(*a).sized(by * S).boolean(&self.win_s(), BoolOp::And),
                LayerExpr::Union(v) => v.iter().fold(PolySet::default(), |acc, &a| acc.boolean(&self.layer(a), BoolOp::Or)),
                _ => PolySet::default(),
            }
        };
        self.cache.borrow_mut().insert(l, p.clone());
        p
    }

    /// The staircase pieces `r` (dbu) as a scaled mask, grown past the staircase error.
    fn mask(&self, r: &Region) -> PolySet {
        let near = r.boolean(&Region::from_rects([grown(self.win, STAIR)]), BoolOp::And).sized(STAIR + 1);
        let rects: Vec<Geometry> = near.rects().iter().filter_map(|&r| scale_geom(&Geometry::Rect(r))).collect();
        PolySet::from_geometries(&rects)
    }

    fn check(&self, rule: &Rule) -> PolySet {
        // Integer distances below MIN fail; any-angle ones pass within 3/4 dbu of MIN.
        let lim = (S * (rule.reach() - 1) + 1) as f64;
        let out = |x: &Edge| x.outward();
        let inward = |x: &Edge| x.outward().map(|(a, b)| (-a, -b));
        match *rule {
            Rule::Width { layer, min } if min > 1 => {
                let e = edge_list(&self.layer(layer));
                pairs(&e, &e, Some(true), lim, &inward, &inward)
            }
            Rule::Space { layer, min } if min > 0 => {
                let e = edge_list(&self.layer(layer));
                pairs(&e, &e, Some(false), lim, &out, &out)
            }
            Rule::Separation { a, b, min } if min > 0 => {
                let (_, free) = split_overlapping(&self.regions[b], &self.regions[a]);
                if free.is_empty() {
                    return PolySet::default();
                }
                let lb = self.layer(b).boolean(&self.mask(&free), BoolOp::And);
                pairs(&edge_list(&lb), &edge_list(&self.layer(a)), None, lim, &out, &out)
            }
            Rule::Enclosure { inner, outer, min } => {
                let (hit, _) = split_overlapping(&self.regions[inner], &self.regions[outer]);
                if hit.is_empty() {
                    return PolySet::default();
                }
                let i = self.layer(inner).boolean(&self.mask(&hit), BoolOp::And);
                let o = self.layer(outer);
                let outside = i.boolean(&o, BoolOp::Not);
                if min <= 0 {
                    return outside;
                }
                pairs(&edge_list(&i), &edge_list(&o), None, lim, &out, &inward).boolean(&outside, BoolOp::Or)
            }
            Rule::Extension { a, b, min } if min > 0 => self.extension(a, b, lim),
            _ => PolySet::default(),
        }
    }

    /// Every edge of `A AND B` on the boundary of B needs A beyond it: no A edge other than the
    /// sides of the crossing may come within `depth`, and A must be there at all.
    fn extension(&self, a: usize, b: usize, lim: f64) -> PolySet {
        let (la, lb) = (self.layer(a), self.layer(b));
        let out = la.boolean(&lb, BoolOp::Not);
        let mut marks: Vec<Vec<Point>> = Vec::new();
        for g in la.boolean(&lb, BoolOp::And).polygons() {
            for (p, q) in rings(g).flat_map(edges) {
                let Some((nx, ny)) = normal(p, q) else { continue };
                let (mx, my) = ((p.0 + q.0) as f64 / 2.0 + 2.0 * nx, (p.1 + q.1) as f64 / 2.0 + 2.0 * ny);
                if lb.contains_f(mx, my) {
                    continue;
                }
                let band = |d: f64| {
                    let off = |v: Point| ((v.0 as f64 + nx * d).round() as i64, (v.1 as f64 + ny * d).round() as i64);
                    vec![p, q, off(q), off(p)]
                };
                let Some(piece) = out.polygons().iter().find(|o| contains(o, mx, my)) else {
                    marks.push(band(lim));
                    continue;
                };
                let seg = (p, q);
                for f in rings(piece).flat_map(edges) {
                    let d = seg_dist(seg, f);
                    if d >= 2.0 && d < lim {
                        marks.push(band(d.max(S as f64)));
                    }
                }
            }
        }
        marks.iter().fold(PolySet::default(), |acc, m| acc.boolean(&PolySet::from_polygon(m), BoolOp::Or))
    }
}

fn rings(p: &Polygon) -> impl Iterator<Item = &[Point]> {
    std::iter::once(p.outer.as_slice()).chain(p.holes.iter().map(Vec::as_slice))
}

fn edges(r: &[Point]) -> impl Iterator<Item = (Point, Point)> + '_ {
    r.iter().zip(r.iter().cycle().skip(1)).map(|(&a, &b)| (a, b))
}

fn len(a: Point, b: Point) -> f64 {
    ((b.0 - a.0) as f64).hypot((b.1 - a.1) as f64)
}

/// Unit normal pointing out of the polygon (the inside is on the left of every edge).
fn normal(a: Point, b: Point) -> Option<(f64, f64)> {
    let l = len(a, b);
    (l > 0.0).then(|| ((b.1 - a.1) as f64 / l, -((b.0 - a.0) as f64) / l))
}

fn contains(p: &Polygon, x: f64, y: f64) -> bool {
    let mut inside = false;
    for (a, b) in rings(p).flat_map(edges) {
        let (ay, by) = (a.1 as f64, b.1 as f64);
        if (ay > y) != (by > y) && x < a.0 as f64 + (y - ay) / (by - ay) * (b.0 - a.0) as f64 {
            inside = !inside;
        }
    }
    inside
}

fn point_seg(p: (f64, f64), a: Point, b: Point) -> f64 {
    let (ax, ay, bx, by) = (a.0 as f64, a.1 as f64, b.0 as f64, b.1 as f64);
    let (dx, dy) = (bx - ax, by - ay);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((p.0 - ax) * dx + (p.1 - ay) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (p.0 - ax - t * dx).hypot(p.1 - ay - t * dy)
}

fn seg_dist((a, b): (Point, Point), (c, d): (Point, Point)) -> f64 {
    let cross =
        |o: Point, p: Point, q: Point| (p.0 - o.0) as i128 * (q.1 - o.1) as i128 - (p.1 - o.1) as i128 * (q.0 - o.0) as i128;
    let (d1, d2, d3, d4) = (cross(c, d, a), cross(c, d, b), cross(a, b, c), cross(a, b, d));
    if (d1 > 0) != (d2 > 0) && d1 != 0 && d2 != 0 && (d3 > 0) != (d4 > 0) && d3 != 0 && d4 != 0 {
        return 0.0;
    }
    let f = |p: Point| (p.0 as f64, p.1 as f64);
    point_seg(f(a), c, d).min(point_seg(f(b), c, d)).min(point_seg(f(c), a, b)).min(point_seg(f(d), a, b))
}

/// An edge of a polygon set (the inside on its left) with its place in its ring.
struct Edge {
    a: Point,
    b: Point,
    ring: usize,
    k: usize,
    n: usize,
    bbox: IRect,
}

impl Edge {
    fn outward(&self) -> Option<(f64, f64)> {
        normal(self.a, self.b)
    }
}

fn edge_list(p: &PolySet) -> Vec<Edge> {
    let mut out = Vec::new();
    for (ring, r) in p.polygons().iter().flat_map(rings).enumerate() {
        for (k, (a, b)) in edges(r).enumerate() {
            let bbox = IRect::new(a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1));
            out.push(Edge { a, b, ring, k, n: r.len(), bbox });
        }
    }
    out
}

type Normal<'a> = &'a dyn Fn(&Edge) -> Option<(f64, f64)>;

/// Markers between edges of `xs` and `ys` closer than `lim` whose sides `fx` / `fy` (unit
/// normals) face each other: the normals point against each other and towards the other edge.
/// With `same = Some(convex)`, both lists are one set; adjacent edges count only where they
/// meet at an acute angle on the checked side (inside at a convex corner, outside at a
/// concave one).
fn pairs(xs: &[Edge], ys: &[Edge], same: Option<bool>, lim: f64, fx: Normal, fy: Normal) -> PolySet {
    let reach = lim.ceil() as i64;
    let tree = RTree::bulk_load(
        ys.iter()
            .enumerate()
            .map(|(i, e)| GeomWithData::new(Rectangle::from_corners([e.bbox.x0, e.bbox.y0], [e.bbox.x1, e.bbox.y1]), i))
            .collect(),
    );
    let mut marks = Vec::new();
    for (i, x) in xs.iter().enumerate() {
        let Some(nx) = fx(x) else { continue };
        let q = grown(x.bbox, reach);
        for hit in tree.locate_in_envelope_intersecting(AABB::from_corners([q.x0, q.y0], [q.x1, q.y1])) {
            let (j, y) = (hit.data, &ys[hit.data]);
            if same.is_some() && j <= i {
                continue;
            }
            let Some(ny) = fy(y) else { continue };
            if nx.0 * ny.0 + nx.1 * ny.1 >= 0.0 {
                continue;
            }
            if let Some(convex) = same.filter(|_| x.ring == y.ring) {
                let (first, second) = match ((x.k + 1) % x.n == y.k, (y.k + 1) % x.n == x.k) {
                    (true, _) => (x, y),
                    (_, true) => (y, x),
                    _ => (x, x),
                };
                if !std::ptr::eq(first, second) {
                    let turn = (first.b.0 - first.a.0) as i128 * (second.b.1 - second.a.1) as i128
                        - (first.b.1 - first.a.1) as i128 * (second.b.0 - second.a.0) as i128;
                    if (turn > 0) == convex {
                        marks.push(corner(first, second, lim));
                    }
                    continue;
                }
            }
            let (d, cx, cy) = closest((x.a, x.b), (y.a, y.b));
            if d >= lim {
                continue;
            }
            let v = (cy.0 - cx.0, cy.1 - cx.1);
            if d > 0.0 && (v.0 * nx.0 + v.1 * nx.1 <= 0.0 || v.0 * ny.0 + v.1 * ny.1 >= 0.0) {
                continue;
            }
            marks.push(marker(x, y));
        }
    }
    marks.iter().fold(PolySet::default(), |acc, m| acc.boolean(m, BoolOp::Or))
}

/// The wedge of an acute corner between consecutive edges `first` and `second`, `lim` deep.
fn corner(first: &Edge, second: &Edge, lim: f64) -> PolySet {
    let v = first.b;
    let along = |from: Point, to: Point| {
        let l = len(from, to).max(1.0);
        let t = lim.min(l) / l;
        ((from.0 as f64 + t * (to.0 - from.0) as f64).round() as i64, (from.1 as f64 + t * (to.1 - from.1) as f64).round() as i64)
    };
    let pts = hull(&[along(v, first.a), v, along(v, second.b)]);
    let area2: i128 = edges(&pts).map(|(p, q)| p.0 as i128 * q.1 as i128 - q.0 as i128 * p.1 as i128).sum();
    if pts.len() >= 3 && area2.abs() >= (2 * S * S) as i128 {
        return PolySet::from_polygon(&pts);
    }
    PolySet::from_rect(grown(IRect::new(v.0, v.1, v.0, v.1), S))
}

/// The region between the parts of two edges that face each other.
fn marker(x: &Edge, y: &Edge) -> PolySet {
    let proj = |p: Point, (a, b): (Point, Point)| {
        let (dx, dy) = ((b.0 - a.0) as f64, (b.1 - a.1) as f64);
        (((p.0 - a.0) as f64 * dx + (p.1 - a.1) as f64 * dy) / (dx * dx + dy * dy).max(1.0)).clamp(0.0, 1.0)
    };
    let sub = |(a, b): (Point, Point), t0: f64, t1: f64| {
        let at =
            |t: f64| ((a.0 as f64 + t * (b.0 - a.0) as f64).round() as i64, (a.1 as f64 + t * (b.1 - a.1) as f64).round() as i64);
        (at(t0.min(t1)), at(t0.max(t1)))
    };
    let (ex, ey) = ((x.a, x.b), (y.a, y.b));
    let (x0, x1) = sub(ex, proj(y.a, ex), proj(y.b, ex));
    let (y0, y1) = sub(ey, proj(x.a, ey), proj(x.b, ey));
    let pts = hull(&[x0, x1, y0, y1]);
    let area2: i128 = edges(&pts).map(|(p, q)| p.0 as i128 * q.1 as i128 - q.0 as i128 * p.1 as i128).sum();
    if pts.len() >= 3 && area2.abs() >= (2 * S * S) as i128 {
        return PolySet::from_polygon(&pts);
    }
    let b = pts.iter().fold(IRect::new(x0.0, x0.1, x0.0, x0.1), |b, p| b.union(&IRect::new(p.0, p.1, p.0, p.1)));
    PolySet::from_rect(grown(b, S))
}

/// Convex hull, counter-clockwise.
fn hull(pts: &[Point]) -> Vec<Point> {
    let mut p = pts.to_vec();
    p.sort_unstable();
    p.dedup();
    if p.len() < 3 {
        return p;
    }
    let cross =
        |o: Point, a: Point, b: Point| (a.0 - o.0) as i128 * (b.1 - o.1) as i128 - (a.1 - o.1) as i128 * (b.0 - o.0) as i128;
    let mut h: Vec<Point> = Vec::new();
    for pass in [false, true] {
        let start = h.len();
        let order: Vec<Point> = if pass { p.iter().rev().copied().collect() } else { p.clone() };
        for q in order {
            while h.len() >= start + 2 && cross(h[h.len() - 2], h[h.len() - 1], q) <= 0 {
                h.pop();
            }
            h.push(q);
        }
        h.pop();
    }
    h
}

/// Distance between two segments and the closest points on each.
fn closest((a, b): (Point, Point), (c, d): (Point, Point)) -> (f64, (f64, f64), (f64, f64)) {
    if seg_dist((a, b), (c, d)) == 0.0 {
        let m = ((a.0 + b.0) as f64 / 2.0, (a.1 + b.1) as f64 / 2.0);
        return (0.0, m, m);
    }
    let f = |p: Point| (p.0 as f64, p.1 as f64);
    let foot = |p: (f64, f64), a: Point, b: Point| {
        let (ax, ay, dx, dy) = (a.0 as f64, a.1 as f64, (b.0 - a.0) as f64, (b.1 - a.1) as f64);
        let l2 = dx * dx + dy * dy;
        let t = if l2 > 0.0 { (((p.0 - ax) * dx + (p.1 - ay) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
        (ax + t * dx, ay + t * dy)
    };
    let cands = [(f(a), foot(f(a), c, d)), (f(b), foot(f(b), c, d)), (foot(f(c), a, b), f(c)), (foot(f(d), a, b), f(d))];
    let dist = |u: &((f64, f64), (f64, f64))| (u.0.0 - u.1.0).hypot(u.0.1 - u.1.1);
    let (p, q) = cands.into_iter().min_by(|u, v| dist(u).total_cmp(&dist(v))).expect("four candidates");
    ((p.0 - q.0).hypot(p.1 - q.1), p, q)
}
