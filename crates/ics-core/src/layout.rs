//! Full-custom layout database: shapes on technology layers, in database units (dbu).
//! Layout y grows upwards (GDS convention).

use serde::{Deserialize, Serialize};

use crate::geom::IRect;
use crate::tech::Tech;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Geometry {
    Rect(IRect),
    Polygon { points: Vec<(i64, i64)> },
    Path { points: Vec<(i64, i64)>, width: i64 },
    Label { text: String, at: (i64, i64), size: i64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub id: u64,
    pub layer: usize,
    pub geom: Geometry,
}

impl Shape {
    pub fn bbox(&self) -> IRect {
        match &self.geom {
            Geometry::Rect(r) => *r,
            Geometry::Polygon { points } => bbox_of(points, 0),
            Geometry::Path { points, width } => bbox_of(points, width / 2),
            Geometry::Label { at, size, text } => {
                let w = (text.len() as i64).max(1) * size * 6 / 10;
                IRect::new(at.0, at.1, at.0 + w, at.1 + *size)
            }
        }
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

    /// Manhattan rectangles approximating this shape (used for DRC).
    pub fn rects(&self) -> Vec<IRect> {
        match &self.geom {
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

    pub fn shape(&self, id: u64) -> Option<&Shape> {
        self.shapes.iter().find(|s| s.id == id)
    }

    pub fn shape_mut(&mut self, id: u64) -> Option<&mut Shape> {
        self.shapes.iter_mut().find(|s| s.id == id)
    }

    pub fn remove(&mut self, id: u64) {
        self.shapes.retain(|s| s.id != id);
    }

    pub fn bbox(&self) -> Option<IRect> {
        self.shapes.iter().map(|s| s.bbox()).reduce(|a, b| a.union(&b))
    }
}

/// A design rule violation.
#[derive(Debug, Clone, PartialEq)]
pub struct DrcViolation {
    pub layer: usize,
    pub rule: String,
    pub area: IRect,
    pub shapes: Vec<u64>,
}

/// Very small built-in DRC: minimum width and minimum same-layer spacing on Manhattan shapes.
/// Intended as a placeholder until an external engine (KLayout / Magic / Calibre) is plugged in.
pub fn run_basic_drc(layout: &Layout, tech: &Tech) -> Vec<DrcViolation> {
    let mut out = vec![];
    let mut per_layer: Vec<Vec<(u64, IRect)>> = vec![vec![]; tech.layers.len()];
    for s in &layout.shapes {
        if s.layer < per_layer.len() {
            for r in s.rects() {
                per_layer[s.layer].push((s.id, r));
            }
        }
    }
    for (li, rects) in per_layer.iter().enumerate() {
        let def = &tech.layers[li];
        if def.min_width > 0 {
            for (id, r) in rects {
                let w = r.width().min(r.height());
                if w < def.min_width {
                    out.push(DrcViolation {
                        layer: li,
                        rule: format!("{}.W: width {} < {} nm", def.name, w, def.min_width),
                        area: *r,
                        shapes: vec![*id],
                    });
                }
            }
        }
        if def.min_spacing > 0 {
            for i in 0..rects.len() {
                for j in (i + 1)..rects.len() {
                    let (ia, a) = rects[i];
                    let (ib, b) = rects[j];
                    if ia == ib {
                        continue;
                    }
                    // Touching / overlapping shapes are considered merged.
                    let touching = a.x0 <= b.x1 && b.x0 <= a.x1 && a.y0 <= b.y1 && b.y0 <= a.y1;
                    if touching {
                        continue;
                    }
                    let s = a.spacing(&b);
                    if s < def.min_spacing {
                        let area = IRect::new(a.x1.min(b.x1), a.y1.min(b.y1), a.x0.max(b.x0), a.y0.max(b.y0));
                        out.push(DrcViolation {
                            layer: li,
                            rule: format!("{}.S: spacing {} < {} nm", def.name, s, def.min_spacing),
                            area,
                            shapes: vec![ia, ib],
                        });
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drc_finds_width_and_spacing() {
        let tech = Tech::generic();
        let m1 = tech.layer_index("metal1").unwrap();
        let mut l = Layout::default();
        l.add_rect(m1, 0, 0, 100, 1000); // too narrow
        l.add_rect(m1, 1000, 0, 1300, 1000);
        l.add_rect(m1, 1400, 0, 1700, 1000); // 100nm spacing to previous
        let v = run_basic_drc(&l, &tech);
        assert!(v.iter().any(|v| v.rule.contains(".W")));
        assert!(v.iter().any(|v| v.rule.contains(".S")));
    }
}
