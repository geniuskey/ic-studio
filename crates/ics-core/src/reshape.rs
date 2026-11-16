//! Shape editing operations of the layout editor (Virtuoso's Edit > Advanced): split a shape
//! along a line, convert paths and rectangles to polygons, and chamfer the corners of a shape.

use crate::geom::IRect;
use crate::layout::Geometry;
use crate::polybool::{Point, PolySet};
use crate::region::BoolOp;

/// Split a shape along the line through `a` and `b`. Returns the pieces on both sides, or
/// `None` when the line does not cut the shape (or the shape is a label). Paths are split as
/// their outlines; the pieces are rectangles or polygons.
pub fn split(g: &Geometry, a: Point, b: Point) -> Option<Vec<Geometry>> {
    if matches!(g, Geometry::Label { .. }) || a == b {
        return None;
    }
    let set = PolySet::from_geometry(g);
    let bb = set.bbox()?;
    let (dx, dy) = ((b.0 - a.0) as f64, (b.1 - a.1) as f64);
    let len = (dx * dx + dy * dy).sqrt();
    let (ux, uy) = (dx / len, dy / len);
    // Far enough to cover the shape from anywhere on the line.
    let far = |p: Point| ((p.0 - bb.x0).abs().max((p.0 - bb.x1).abs()) + (p.1 - bb.y0).abs().max((p.1 - bb.y1).abs())) as f64;
    let k = 2.0 * (far(a).max(far(b)) + bb.width() as f64 + bb.height() as f64) + 1.0;
    let at = |x: f64, y: f64| (x.round() as i64, y.round() as i64);
    let (ax, ay) = (a.0 as f64 - ux * k, a.1 as f64 - uy * k);
    let (bx, by) = (a.0 as f64 + ux * k, a.1 as f64 + uy * k);
    let side = |s: f64| {
        let (nx, ny) = (-uy * k * s, ux * k * s);
        PolySet::from_polygon(&[at(ax, ay), at(bx, by), at(bx + nx, by + ny), at(ax + nx, ay + ny)])
    };
    let left = set.boolean(&side(1.0), BoolOp::And);
    let right = set.boolean(&side(-1.0), BoolOp::And);
    if left.is_empty() || right.is_empty() {
        return None;
    }
    let mut out = left.to_geometry();
    out.extend(right.to_geometry());
    Some(out)
}

/// The shape as polygons: a rectangle becomes its four corners, a path its outline. `None`
/// for polygons and labels (nothing to convert).
pub fn to_polygon(g: &Geometry) -> Option<Vec<Geometry>> {
    match g {
        Geometry::Rect(r) => {
            Some(vec![Geometry::Polygon { points: vec![(r.x0, r.y0), (r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1)] }])
        }
        Geometry::Path { .. } => {
            let polys = PolySet::from_geometry(g).to_polygons();
            (!polys.is_empty()).then(|| polys.into_iter().map(|points| Geometry::Polygon { points }).collect())
        }
        _ => None,
    }
}

/// Cut every convex corner of a rectangle or polygon by `c` along both of its edges (at most
/// half of each edge, so neighbouring cuts do not cross). `None` when nothing changes.
pub fn chamfer(g: &Geometry, c: i64) -> Option<Geometry> {
    if c <= 0 {
        return None;
    }
    let pts: Vec<Point> = match g {
        Geometry::Rect(r) => vec![(r.x0, r.y0), (r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1)],
        Geometry::Polygon { points } => points.clone(),
        _ => return None,
    };
    let n = pts.len();
    if n < 3 {
        return None;
    }
    let area2: i128 = (0..n)
        .map(|i| {
            let (p, q) = (pts[i], pts[(i + 1) % n]);
            p.0 as i128 * q.1 as i128 - q.0 as i128 * p.1 as i128
        })
        .sum();
    let ccw = area2 > 0;
    let mut out: Vec<Point> = Vec::with_capacity(2 * n);
    let mut changed = false;
    for i in 0..n {
        let (prev, v, next) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
        let (e1, e2) = ((v.0 - prev.0, v.1 - prev.1), (next.0 - v.0, next.1 - v.1));
        let cross = e1.0 as i128 * e2.1 as i128 - e1.1 as i128 * e2.0 as i128;
        let convex = if ccw { cross > 0 } else { cross < 0 };
        let (l1, l2) = (len(e1), len(e2));
        let d = (c as f64).min(l1 / 2.0).min(l2 / 2.0);
        if !convex || d < 1.0 {
            out.push(v);
            continue;
        }
        let back = (v.0 as f64 - e1.0 as f64 / l1 * d, v.1 as f64 - e1.1 as f64 / l1 * d);
        let fwd = (v.0 as f64 + e2.0 as f64 / l2 * d, v.1 as f64 + e2.1 as f64 / l2 * d);
        out.push((back.0.round() as i64, back.1.round() as i64));
        out.push((fwd.0.round() as i64, fwd.1.round() as i64));
        changed = true;
    }
    out.dedup();
    if out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    changed.then_some(Geometry::Polygon { points: out })
}

fn len(e: (i64, i64)) -> f64 {
    ((e.0 as f64).powi(2) + (e.1 as f64).powi(2)).sqrt()
}

/// Whether the segment from `a` to `b` touches the box (for picking the shapes a split line
/// crosses).
pub fn segment_hits(r: IRect, a: Point, b: Point) -> bool {
    // Liang-Barsky clipping of the segment to the box.
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    let (dx, dy) = ((b.0 - a.0) as f64, (b.1 - a.1) as f64);
    for (p, q) in [(-dx, (a.0 - r.x0) as f64), (dx, (r.x1 - a.0) as f64), (-dy, (a.1 - r.y0) as f64), (dy, (r.y1 - a.1) as f64)] {
        if p == 0.0 {
            if q < 0.0 {
                return false;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
        }
    }
    t0 <= t1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(gs: &[Geometry]) -> f64 {
        PolySet::from_geometries(gs).area()
    }

    #[test]
    fn split_rect_and_path() {
        let r = Geometry::Rect(IRect::new(0, 0, 1000, 400));
        let parts = split(&r, (300, -100), (300, 500)).unwrap();
        assert_eq!(parts.len(), 2);
        assert!(parts.contains(&Geometry::Rect(IRect::new(0, 0, 300, 400))), "{parts:?}");
        assert!(parts.contains(&Geometry::Rect(IRect::new(300, 0, 1000, 400))), "{parts:?}");
        // A line beside the shape, or along an edge, does not cut it.
        assert!(split(&r, (1200, 0), (1200, 10)).is_none());
        assert!(split(&r, (0, 0), (0, 400)).is_none());
        // A diagonal cut keeps the area.
        let parts = split(&r, (0, 0), (1000, 400)).unwrap();
        assert_eq!(parts.len(), 2);
        assert!((area(&parts) - 400_000.0).abs() < 1.0);
        // An L-shaped path cut across its corner leg.
        let p = Geometry::Path { points: vec![(0, 0), (1000, 0), (1000, 1000)], width: 100 };
        let parts = split(&p, (500, -200), (500, 200)).unwrap();
        assert_eq!(parts.len(), 2);
        assert!((area(&parts) - area(std::slice::from_ref(&p))).abs() < 1.0);
        assert!(split(&Geometry::Label { text: "x".into(), at: (0, 0), size: 10 }, (0, 0), (1, 1)).is_none());
    }

    #[test]
    fn convert_and_chamfer() {
        let r = Geometry::Rect(IRect::new(0, 0, 1000, 400));
        let poly = to_polygon(&r).unwrap();
        assert!(matches!(&poly[0], Geometry::Polygon { points } if points.len() == 4));
        let p = Geometry::Path { points: vec![(0, 0), (1000, 0)], width: 100 };
        let pp = to_polygon(&p).unwrap();
        // The outline the editor draws: square ends extended by half the width.
        assert!((area(&pp) - 110_000.0).abs() < 1.0, "{pp:?}");
        assert!(to_polygon(&poly[0]).is_none());

        // Every corner of a rectangle is cut: 8 vertices, 4 triangles of c^2/2 removed.
        let c = chamfer(&r, 100).unwrap();
        let Geometry::Polygon { points } = &c else { panic!() };
        assert_eq!(points.len(), 8);
        assert!((area(std::slice::from_ref(&c)) - (400_000.0 - 2.0 * 100.0 * 100.0)).abs() < 1.0);
        // A large chamfer stops at half an edge.
        let Geometry::Polygon { points } = chamfer(&r, 10_000).unwrap() else { panic!() };
        assert!(points.len() <= 8);
        // The inner corner of an L stays (only convex corners are cut), clockwise or not.
        let l = vec![(0, 0), (1000, 0), (1000, 400), (400, 400), (400, 1000), (0, 1000)];
        for pts in [l.clone(), l.iter().rev().copied().collect()] {
            let Geometry::Polygon { points } = chamfer(&Geometry::Polygon { points: pts }, 50).unwrap() else { panic!() };
            assert!(points.contains(&(400, 400)), "{points:?}");
            assert_eq!(points.len(), 11);
        }
        assert!(chamfer(&r, 0).is_none());
    }

    #[test]
    fn segment_box_test() {
        let b = IRect::new(0, 0, 10, 10);
        assert!(segment_hits(b, (-5, 5), (15, 5)));
        assert!(segment_hits(b, (5, 5), (6, 6)));
        assert!(!segment_hits(b, (-5, 20), (15, 20)));
        assert!(!segment_hits(b, (-5, -5), (-1, 20)));
    }
}
