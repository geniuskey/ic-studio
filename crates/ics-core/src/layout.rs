//! Full-custom layout database: shapes on technology layers and placed cell instances,
//! in database units (dbu). Layout y grows upwards (GDS convention).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::geom::IRect;
use crate::hier::LayoutSource;
use crate::project::CellRef;

/// Placement transform: optional mirror about the x axis, then a counter-clockwise rotation
/// in 90° steps, then a displacement (the GDS STRANS / ANGLE / XY convention).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct Trans {
    pub x: i64,
    pub y: i64,
    #[serde(default)]
    pub rot: u8,
    #[serde(default)]
    pub mirror: bool,
}

impl Trans {
    pub const IDENTITY: Trans = Trans { x: 0, y: 0, rot: 0, mirror: false };

    /// 90° counter-clockwise rotation about `p`.
    pub fn rotate_about(p: (i64, i64)) -> Trans {
        Trans { x: p.0 + p.1, y: p.1 - p.0, rot: 1, mirror: false }
    }

    /// Flip left-right (x -> -x) about the vertical line through `p`.
    pub fn flip_x_about(p: (i64, i64)) -> Trans {
        Trans { x: 2 * p.0, y: 0, rot: 2, mirror: true }
    }

    /// Flip top-bottom (y -> -y) about the horizontal line through `p`.
    pub fn flip_y_about(p: (i64, i64)) -> Trans {
        Trans { x: 0, y: 2 * p.1, rot: 0, mirror: true }
    }

    pub fn new(x: i64, y: i64) -> Self {
        Self { x, y, rot: 0, mirror: false }
    }

    pub fn with(x: i64, y: i64, rot: u8, mirror: bool) -> Self {
        Self { x, y, rot: rot % 4, mirror }
    }

    pub fn is_identity(&self) -> bool {
        *self == Self::IDENTITY
    }

    /// Apply the linear part (mirror + rotation) only.
    pub fn apply_vec(&self, (x, y): (i64, i64)) -> (i64, i64) {
        let y = if self.mirror { -y } else { y };
        match self.rot % 4 {
            0 => (x, y),
            1 => (-y, x),
            2 => (-x, -y),
            _ => (y, -x),
        }
    }

    pub fn apply(&self, p: (i64, i64)) -> (i64, i64) {
        let (x, y) = self.apply_vec(p);
        (x + self.x, y + self.y)
    }

    pub fn apply_f(&self, (x, y): (f64, f64)) -> (f64, f64) {
        let y = if self.mirror { -y } else { y };
        let (x, y) = match self.rot % 4 {
            0 => (x, y),
            1 => (-y, x),
            2 => (-x, -y),
            _ => (y, -x),
        };
        (x + self.x as f64, y + self.y as f64)
    }

    /// Rectangles stay rectangles under 90° transforms.
    pub fn apply_rect(&self, r: IRect) -> IRect {
        let a = self.apply((r.x0, r.y0));
        let b = self.apply((r.x1, r.y1));
        IRect::new(a.0, a.1, b.0, b.1)
    }

    /// `self ∘ inner`: first apply `inner`, then `self`.
    pub fn compose(&self, inner: &Trans) -> Trans {
        let (a, b) = (self.rot % 4, inner.rot % 4);
        let rot = if self.mirror { (a + 4 - b) % 4 } else { (a + b) % 4 };
        let (x, y) = self.apply((inner.x, inner.y));
        Trans { x, y, rot, mirror: self.mirror ^ inner.mirror }
    }

    pub fn inverse(&self) -> Trans {
        let rot = if self.mirror { self.rot % 4 } else { (4 - self.rot % 4) % 4 };
        let lin = Trans { x: 0, y: 0, rot, mirror: self.mirror };
        let (dx, dy) = lin.apply_vec((self.x, self.y));
        Trans { x: -dx, y: -dy, rot, mirror: self.mirror }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Geometry {
    Rect(IRect),
    Polygon { points: Vec<(i64, i64)> },
    Path { points: Vec<(i64, i64)>, width: i64 },
    Label { text: String, at: (i64, i64), size: i64 },
}

impl Geometry {
    pub fn transformed(&self, t: &Trans) -> Geometry {
        let pts = |p: &[(i64, i64)]| p.iter().map(|&q| t.apply(q)).collect();
        match self {
            Geometry::Rect(r) => Geometry::Rect(t.apply_rect(*r)),
            Geometry::Polygon { points } => Geometry::Polygon { points: pts(points) },
            Geometry::Path { points, width } => Geometry::Path { points: pts(points), width: *width },
            Geometry::Label { text, at, size } => Geometry::Label { text: text.clone(), at: t.apply(*at), size: *size },
        }
    }

    pub fn bbox(&self) -> IRect {
        match self {
            Geometry::Rect(r) => *r,
            Geometry::Polygon { points } => bbox_of(points, 0),
            Geometry::Path { points, width } => bbox_of(points, width / 2),
            Geometry::Label { at, size, text } => {
                let w = (text.len() as i64).max(1) * size * 6 / 10;
                IRect::new(at.0, at.1, at.0 + w, at.1 + *size)
            }
        }
    }

    /// Manhattan rectangles approximating this geometry (used for DRC).
    pub fn rects(&self) -> Vec<IRect> {
        match self {
            Geometry::Rect(r) => vec![*r],
            Geometry::Path { points, width } => {
                let h = width / 2;
                points
                    .windows(2)
                    .filter_map(|w| {
                        let (a, b) = (w[0], w[1]);
                        if a.0 == b.0 || a.1 == b.1 {
                            Some(IRect::new(a.0.min(b.0) - h, a.1.min(b.1) - h, a.0.max(b.0) + h, a.1.max(b.1) + h))
                        } else {
                            None
                        }
                    })
                    .collect()
            }
            _ => vec![],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub id: u64,
    pub layer: usize,
    pub geom: Geometry,
}

impl Shape {
    pub fn bbox(&self) -> IRect {
        self.geom.bbox()
    }

    pub fn translate(&mut self, dx: i64, dy: i64) {
        match &mut self.geom {
            Geometry::Rect(r) => *r = r.translate(dx, dy),
            Geometry::Polygon { points } | Geometry::Path { points, .. } => {
                for p in points {
                    p.0 += dx;
                    p.1 += dy;
                }
            }
            Geometry::Label { at, .. } => {
                at.0 += dx;
                at.1 += dy;
            }
        }
    }

    /// Hit test with a tolerance in dbu.
    pub fn hit(&self, x: i64, y: i64, tol: i64) -> bool {
        match &self.geom {
            Geometry::Rect(r) => IRect::new(r.x0 - tol, r.y0 - tol, r.x1 + tol, r.y1 + tol).contains(x, y),
            Geometry::Polygon { points } => point_in_polygon(points, x, y),
            Geometry::Path { points, width } => points.windows(2).any(|w| {
                let d = dist_to_segment((x as f64, y as f64), w[0], w[1]);
                d <= (*width as f64 / 2.0) + tol as f64
            }),
            Geometry::Label { .. } => {
                let b = self.bbox();
                IRect::new(b.x0 - tol, b.y0 - tol, b.x1 + tol, b.y1 + tol).contains(x, y)
            }
        }
    }

    pub fn rects(&self) -> Vec<IRect> {
        self.geom.rects()
    }
}

/// Regular array of placements: element (i, j) sits at `trans + i * col + j * row`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArraySpec {
    pub cols: u32,
    pub rows: u32,
    pub col: (i64, i64),
    pub row: (i64, i64),
}

/// A placement of another cell's layout (GDS SREF, or AREF when `array` is set).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CellInst {
    pub id: u64,
    pub cell: CellRef,
    pub trans: Trans,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub array: Option<ArraySpec>,
}

impl CellInst {
    pub fn dims(&self) -> (u32, u32) {
        self.array.map_or((1, 1), |a| (a.cols.max(1), a.rows.max(1)))
    }

    pub fn count(&self) -> u64 {
        let (c, r) = self.dims();
        c as u64 * r as u64
    }

    fn offset(&self, i: u32, j: u32) -> (i64, i64) {
        match self.array {
            Some(a) => (i as i64 * a.col.0 + j as i64 * a.row.0, i as i64 * a.col.1 + j as i64 * a.row.1),
            None => (0, 0),
        }
    }

    pub fn element_trans(&self, i: u32, j: u32) -> Trans {
        let (dx, dy) = self.offset(i, j);
        Trans { x: self.trans.x + dx, y: self.trans.y + dy, ..self.trans }
    }

    /// Bounding box of the whole placement given the child cell's bounding box.
    pub fn bbox(&self, child: IRect) -> IRect {
        let b = self.trans.apply_rect(child);
        let (c, r) = self.dims();
        [(0, 0), (c - 1, 0), (0, r - 1), (c - 1, r - 1)].iter().fold(b, |acc, &(i, j)| {
            let (dx, dy) = self.offset(i, j);
            acc.union(&b.translate(dx, dy))
        })
    }

    /// Array elements whose box intersects `view` (all in parent coordinates).
    pub fn visible_elements(&self, child: IRect, view: IRect) -> Vec<(u32, u32)> {
        let b = self.trans.apply_rect(child);
        let (c, r) = self.dims();
        if let Some(a) = self.array
            && a.col.1 == 0
            && a.row.0 == 0
            && a.col.0 != 0
            && a.row.1 != 0
        {
            let range = |lo: i64, hi: i64, vlo: i64, vhi: i64, step: i64, n: u32| -> (u32, u32) {
                // element k spans [lo + k*step, hi + k*step]
                let (k0, k1) = if step > 0 {
                    (((vlo - hi) as f64 / step as f64).floor() as i64, ((vhi - lo) as f64 / step as f64).ceil() as i64)
                } else {
                    (((vhi - lo) as f64 / step as f64).floor() as i64, ((vlo - hi) as f64 / step as f64).ceil() as i64)
                };
                (k0.clamp(0, n as i64) as u32, (k1 + 1).clamp(0, n as i64) as u32)
            };
            let (i0, i1) = range(b.x0, b.x1, view.x0, view.x1, a.col.0, c);
            let (j0, j1) = range(b.y0, b.y1, view.y0, view.y1, a.row.1, r);
            let mut out = Vec::with_capacity(((i1 - i0) * (j1 - j0)) as usize);
            for j in j0..j1 {
                for i in i0..i1 {
                    let (dx, dy) = self.offset(i, j);
                    if b.translate(dx, dy).intersects(&view) {
                        out.push((i, j));
                    }
                }
            }
            return out;
        }
        let mut out = vec![];
        for j in 0..r {
            for i in 0..c {
                let (dx, dy) = self.offset(i, j);
                if b.translate(dx, dy).intersects(&view) {
                    out.push((i, j));
                }
            }
        }
        out
    }

    pub fn translate(&mut self, dx: i64, dy: i64) {
        self.trans.x += dx;
        self.trans.y += dy;
    }
}

fn bbox_of(points: &[(i64, i64)], grow: i64) -> IRect {
    let mut r = IRect { x0: i64::MAX, y0: i64::MAX, x1: i64::MIN, y1: i64::MIN };
    for &(x, y) in points {
        r.x0 = r.x0.min(x - grow);
        r.y0 = r.y0.min(y - grow);
        r.x1 = r.x1.max(x + grow);
        r.y1 = r.y1.max(y + grow);
    }
    if points.is_empty() { IRect::default() } else { r }
}

fn point_in_polygon(pts: &[(i64, i64)], x: i64, y: i64) -> bool {
    let mut inside = false;
    let n = pts.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (pts[i].0 as f64, pts[i].1 as f64);
        let (xj, yj) = (pts[j].0 as f64, pts[j].1 as f64);
        if (yi > y as f64) != (yj > y as f64) && (x as f64) < (xj - xi) * (y as f64 - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn dist_to_segment(p: (f64, f64), a: (i64, i64), b: (i64, i64)) -> f64 {
    let (ax, ay, bx, by) = (a.0 as f64, a.1 as f64, b.0 as f64, b.1 as f64);
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 { 0.0 } else { (((p.0 - ax) * dx + (p.1 - ay) * dy) / len2).clamp(0.0, 1.0) };
    let (cx, cy) = (ax + t * dx, ay + t * dy);
    ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt()
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Layout {
    #[serde(default)]
    pub shapes: Vec<Shape>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instances: Vec<CellInst>,
    #[serde(default)]
    pub next_id: u64,
}

impl Layout {
    pub fn alloc_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn add(&mut self, layer: usize, geom: Geometry) -> u64 {
        let id = self.alloc_id();
        self.shapes.push(Shape { id, layer, geom });
        id
    }

    pub fn add_rect(&mut self, layer: usize, x0: i64, y0: i64, x1: i64, y1: i64) -> u64 {
        self.add(layer, Geometry::Rect(IRect::new(x0, y0, x1, y1)))
    }

    pub fn add_instance(&mut self, cell: CellRef, trans: Trans, array: Option<ArraySpec>) -> u64 {
        let id = self.alloc_id();
        self.instances.push(CellInst { id, cell, trans, array });
        id
    }

    pub fn shape(&self, id: u64) -> Option<&Shape> {
        self.shapes.iter().find(|s| s.id == id)
    }

    pub fn shape_mut(&mut self, id: u64) -> Option<&mut Shape> {
        self.shapes.iter_mut().find(|s| s.id == id)
    }

    pub fn instance(&self, id: u64) -> Option<&CellInst> {
        self.instances.iter().find(|s| s.id == id)
    }

    pub fn instance_mut(&mut self, id: u64) -> Option<&mut CellInst> {
        self.instances.iter_mut().find(|s| s.id == id)
    }

    /// Remove a shape or an instance.
    pub fn remove(&mut self, id: u64) {
        self.shapes.retain(|s| s.id != id);
        self.instances.retain(|s| s.id != id);
    }

    /// Move a shape or an instance.
    pub fn translate_item(&mut self, id: u64, dx: i64, dy: i64) {
        if let Some(s) = self.shape_mut(id) {
            s.translate(dx, dy);
        } else if let Some(i) = self.instance_mut(id) {
            i.translate(dx, dy);
        }
    }

    pub fn ids(&self) -> impl Iterator<Item = u64> + '_ {
        self.shapes.iter().map(|s| s.id).chain(self.instances.iter().map(|i| i.id))
    }

    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty() && self.instances.is_empty()
    }

    /// Bounding box of this cell's own shapes (instances are not resolved; see
    /// [`crate::hier::Hier`] for the full hierarchical extent).
    pub fn shapes_bbox(&self) -> Option<IRect> {
        self.shapes.iter().map(|s| s.bbox()).reduce(|a, b| a.union(&b))
    }

    /// Apply `t` to the given shapes and instances (e.g. a rotation about a pivot).
    pub fn transform_items(&mut self, ids: &BTreeSet<u64>, t: &Trans) {
        for s in self.shapes.iter_mut().filter(|s| ids.contains(&s.id)) {
            s.geom = s.geom.transformed(t);
        }
        for i in self.instances.iter_mut().filter(|i| ids.contains(&i.id)) {
            i.trans = t.compose(&i.trans);
            if let Some(a) = i.array.as_mut() {
                a.col = t.apply_vec(a.col);
                a.row = t.apply_vec(a.row);
            }
        }
    }

    /// Replace the given instances by the contents of their cells, one level deep (nested
    /// instances become instances of this cell). Returns the ids of the new items.
    pub fn flatten_instances(&mut self, ids: &BTreeSet<u64>, src: &dyn LayoutSource) -> Vec<u64> {
        let targets: Vec<CellInst> = self.instances.iter().filter(|i| ids.contains(&i.id)).cloned().collect();
        let mut out = vec![];
        for inst in targets {
            let Some(child) = src.layout(&inst.cell) else { continue };
            self.remove(inst.id);
            let (c, r) = inst.dims();
            for j in 0..r {
                for i in 0..c {
                    let t = inst.element_trans(i, j);
                    for s in &child.shapes {
                        out.push(self.add(s.layer, s.geom.transformed(&t)));
                    }
                    for ci in &child.instances {
                        let array = ci.array.map(|a| ArraySpec { col: t.apply_vec(a.col), row: t.apply_vec(a.row), ..a });
                        out.push(self.add_instance(ci.cell.clone(), t.compose(&ci.trans), array));
                    }
                }
            }
        }
        out
    }

    /// Move the given items out into a new layout whose origin is `origin` in this layout.
    pub fn extract(&mut self, ids: &BTreeSet<u64>, origin: (i64, i64)) -> Layout {
        let mut out = Layout::default();
        let t = Trans::new(-origin.0, -origin.1);
        for s in self.shapes.iter().filter(|s| ids.contains(&s.id)) {
            out.add(s.layer, s.geom.transformed(&t));
        }
        for i in self.instances.iter().filter(|i| ids.contains(&i.id)) {
            out.add_instance(i.cell.clone(), t.compose(&i.trans), i.array);
        }
        self.shapes.retain(|s| !ids.contains(&s.id));
        self.instances.retain(|i| !ids.contains(&i.id));
        out
    }

    /// Point instances of `from` at `to` (used when cells are renamed or imported).
    pub fn rename_refs(&mut self, from: &CellRef, to: &CellRef) -> bool {
        let mut any = false;
        for i in &mut self.instances {
            if &i.cell == from {
                i.cell = to.clone();
                any = true;
            }
        }
        any
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hier::flatten;

    #[test]
    fn trans_compose_and_inverse() {
        let pts = [(3, 7), (-5, 2), (0, 0), (11, -4)];
        for rot in 0..4 {
            for mirror in [false, true] {
                let a = Trans::with(100, -50, rot, mirror);
                for rot2 in 0..4 {
                    for m2 in [false, true] {
                        let b = Trans::with(-7, 13, rot2, m2);
                        let c = a.compose(&b);
                        for p in pts {
                            assert_eq!(c.apply(p), a.apply(b.apply(p)), "{a:?} ∘ {b:?}");
                        }
                    }
                }
                let inv = a.inverse();
                for p in pts {
                    assert_eq!(inv.apply(a.apply(p)), p);
                }
            }
        }
        // GDS: 90° rotates +x onto +y; mirror flips y before rotating.
        assert_eq!(Trans::with(0, 0, 1, false).apply((1, 0)), (0, 1));
        assert_eq!(Trans::with(0, 0, 0, true).apply((1, 2)), (1, -2));
    }

    #[test]
    fn pivot_transforms() {
        let p = (100, 50);
        assert_eq!(Trans::rotate_about(p).apply(p), p);
        assert_eq!(Trans::rotate_about(p).apply((110, 50)), (100, 60));
        assert_eq!(Trans::flip_x_about(p).apply((110, 70)), (90, 70));
        assert_eq!(Trans::flip_y_about(p).apply((110, 70)), (110, 30));
    }

    #[test]
    fn flatten_extract_and_transform_items() {
        use std::collections::HashMap;
        let leaf = CellRef::new("w", "leaf");
        let mut child = Layout::default();
        child.add_rect(1, 0, 0, 10, 20);
        let mut src = HashMap::new();
        src.insert(leaf.clone(), child);
        let mut top = Layout::default();
        let id = top.add_instance(
            leaf.clone(),
            Trans::with(100, 0, 1, false),
            Some(ArraySpec { cols: 2, rows: 1, col: (50, 0), row: (0, 0) }),
        );
        let before = flatten(&top, &src, 8).into_iter().map(|f| f.geom).collect::<Vec<_>>();
        let new = top.flatten_instances(&[id].into(), &src);
        assert_eq!(new.len(), 2);
        assert!(top.instances.is_empty());
        assert_eq!(top.shapes.iter().map(|s| s.geom.clone()).collect::<Vec<_>>(), before);

        let ids: BTreeSet<u64> = top.ids().collect();
        let sub = top.extract(&ids, (80, 0));
        assert!(top.is_empty());
        assert_eq!(sub.shapes[0].geom, Geometry::Rect(IRect::new(0, 0, 20, 10)));

        let mut l = Layout::default();
        let r = l.add_rect(1, 0, 0, 30, 10);
        l.transform_items(&[r].into(), &Trans::rotate_about((0, 0)));
        assert_eq!(l.shapes[0].geom, Geometry::Rect(IRect::new(-10, 0, 0, 30)));
    }

    #[test]
    fn array_bbox_and_visible_elements() {
        let inst = CellInst {
            id: 1,
            cell: CellRef::new("work", "c"),
            trans: Trans::new(0, 0),
            array: Some(ArraySpec { cols: 10, rows: 5, col: (1000, 0), row: (0, 2000) }),
        };
        let child = IRect::new(0, 0, 500, 500);
        assert_eq!(inst.bbox(child), IRect::new(0, 0, 9500, 8500));
        let v = inst.visible_elements(child, IRect::new(2100, 1900, 3200, 2600));
        assert_eq!(v, vec![(2, 1), (3, 1)]);
        assert_eq!(inst.visible_elements(child, IRect::new(-10_000, -10_000, 100_000, 100_000)).len(), 50);
    }
}
