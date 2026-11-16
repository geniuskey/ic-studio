//! Layer drawing in the manner of Cadence Virtuoso display packets: the stipple bitmap is drawn
//! in the fill colour one bit per screen point, anchored to the window (so the patterns of
//! neighbouring and overlapping shapes line up), the outline in the outline colour and line
//! style, and cut layers (fill style X) as an outline with both diagonals.
//!
//! Stipples and dash patterns are small repeating white masks, tinted by the vertex colour, so a
//! whole layer is two meshes however many shapes it has.

use std::hash::Hash;

use eframe::egui::{self, Color32, Mesh, Pos2, Rect, TextureId, Vec2, epaint::Vertex};
use ics_core::tech::{FillStyle, LayerDef};

const WHITE_UV: Pos2 = egui::epaint::WHITE_UV;

/// Most vertices of a polygon that is filled (larger ones get an outline only).
const MAX_FILL_POINTS: usize = 2000;

#[derive(Clone, Copy)]
enum Fill {
    None,
    Solid,
    Stipple { tex: TextureId, tile: Vec2 },
}

/// A layer's resolved display packet: colours, stipple and line textures.
pub(crate) struct Packet {
    pub fill: Color32,
    pub outline: Color32,
    kind: Fill,
    x: bool,
    width: f32,
    dash: Option<(TextureId, f32)>,
    /// Share of the area the fill covers (0 to 1), for drawing at coarse levels of detail.
    pub density: f32,
}

/// Display colour of a layer colour: as is on dark backgrounds; light ones (white contact / text,
/// light gray boundary, yellows) are darkened on light backgrounds to keep contrast.
pub(crate) fn display_color([r, g, b]: [u8; 3], dark: bool) -> Color32 {
    if dark {
        return Color32::from_rgb(r, g, b);
    }
    let luma = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    let k = (130.0 / luma.max(1.0)).min(1.0);
    let f = |c: u8| (c as f32 * k).round() as u8;
    Color32::from_rgb(f(r), f(g), f(b))
}

/// A repeating white mask texture of `size`, cached in the context under `key`.
fn mask(ctx: &egui::Context, key: impl Hash + std::fmt::Debug, size: [usize; 2], on: impl Fn(usize, usize) -> bool) -> TextureId {
    let id = egui::Id::unique(("ics-layer-mask", key));
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return t.id();
    }
    let pixels = (0..size[1])
        .flat_map(|y| (0..size[0]).map(move |x| (x, y)))
        .map(|(x, y)| if on(x, y) { Color32::WHITE } else { Color32::TRANSPARENT });
    let image = egui::ColorImage::new(size, pixels.collect());
    let t = ctx.load_texture("layer-stipple", image, egui::TextureOptions::NEAREST_REPEAT);
    let tid = t.id();
    ctx.data_mut(|d| d.insert_temp(id, t));
    tid
}

impl Packet {
    pub fn new(ctx: &egui::Context, def: &LayerDef, dark: bool) -> Packet {
        let [r, g, b, _] = def.color;
        let fill = display_color([r, g, b], dark);
        let outline = display_color(def.outline.unwrap_or([r, g, b]), dark);
        // One stipple bit per whole number of physical pixels, so the pattern stays crisp.
        let ppp = ctx.pixels_per_point();
        let unit = ppp.round().max(1.0) / ppp;
        let (kind, density) = match def.fill {
            FillStyle::Solid => (Fill::Solid, 1.0),
            FillStyle::Outline | FillStyle::Cross => (Fill::None, 0.0),
            _ => match def.pattern() {
                Some(s) => {
                    let (w, h) = (s.width(), s.height());
                    let tex = mask(ctx, ("stipple", &s.rows), [w, h], |x, y| s.bit(x, y));
                    (Fill::Stipple { tex, tile: Vec2::new(w as f32, h as f32) * unit }, s.density() as f32)
                }
                None => (Fill::None, 0.0),
            },
        };
        let pattern = def.line.pattern();
        let dash = (!pattern.is_empty()).then(|| {
            let tex = mask(ctx, ("dash", pattern), [pattern.len(), 1], |x, _| pattern[x] == 1);
            (tex, pattern.len() as f32 * unit)
        });
        Packet { fill, outline, kind, x: def.fill == FillStyle::Cross, width: def.line.width(), dash, density }
    }

    /// Whether the fill is a stipple pattern.
    pub fn is_stipple(&self) -> bool {
        matches!(self.kind, Fill::Stipple { .. })
    }

    /// Start drawing shapes with this packet.
    pub fn begin(&self) -> LayerPaint<'_> {
        let fill = match self.kind {
            Fill::Stipple { tex, .. } => Mesh::with_texture(tex),
            _ => Mesh::default(),
        };
        let line = self.dash.map_or_else(Mesh::default, |(tex, _)| Mesh::with_texture(tex));
        LayerPaint { p: self, fill, line }
    }

    /// One rectangle drawn with this packet.
    pub fn rect_shape(&self, r: Rect, emphasis: f32) -> egui::Shape {
        let mut lp = self.begin();
        lp.rect(r, emphasis);
        lp.finish()
    }
}

/// Fill and outline meshes of the shapes drawn with one packet.
pub(crate) struct LayerPaint<'a> {
    p: &'a Packet,
    fill: Mesh,
    line: Mesh,
}

impl LayerPaint<'_> {
    fn fill_uv(&self, p: Pos2) -> Pos2 {
        match self.p.kind {
            Fill::Stipple { tile, .. } => Pos2::new(p.x / tile.x, p.y / tile.y),
            _ => WHITE_UV,
        }
    }

    fn fill_vertex(&mut self, p: Pos2, color: Color32) {
        let uv = self.fill_uv(p);
        self.fill.vertices.push(Vertex { pos: p, uv, color });
    }

    /// A line from `a` to `b` of the packet's width, extended by `ext` at both ends; dashes are
    /// anchored to the window along the dominant axis.
    fn segment(&mut self, mut a: Pos2, mut b: Pos2, ext: f32, color: Color32) {
        let w = self.p.width;
        let d = b - a;
        let len = d.length();
        if w <= 0.0 || len < 1e-3 {
            return;
        }
        let horizontal = d.x.abs() >= d.y.abs();
        let along = |p: Pos2| if horizontal { p.x } else { p.y };
        if along(b) < along(a) {
            std::mem::swap(&mut a, &mut b);
        }
        let u = (b - a) / len;
        let n = u.rot90() * (w / 2.0);
        let (a, b) = (a - u * ext, b + u * ext);
        let (u0, u1) = match self.p.dash {
            Some((_, period)) => (along(a) / period, (along(a) + len + 2.0 * ext) / period),
            None => (0.0, 0.0),
        };
        let uv = |u: f32| if self.p.dash.is_some() { Pos2::new(u, 0.5) } else { WHITE_UV };
        let base = self.line.vertices.len() as u32;
        for (pos, t) in [(a + n, u0), (a - n, u0), (b - n, u1), (b + n, u1)] {
            self.line.vertices.push(Vertex { pos, uv: uv(t), color });
        }
        self.line.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// The fill of a screen rectangle, without outline.
    pub fn fill_rect(&mut self, r: Rect, emphasis: f32) {
        if matches!(self.p.kind, Fill::None) {
            return;
        }
        let c = self.p.fill.gamma_multiply(emphasis);
        let base = self.fill.vertices.len() as u32;
        for p in [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()] {
            self.fill_vertex(p, c);
        }
        self.fill.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// A screen rectangle: fill, outline inside its edge, and the X of cut layers.
    pub fn rect(&mut self, r: Rect, emphasis: f32) {
        self.fill_rect(r, emphasis);
        let w = self.p.width;
        if w <= 0.0 {
            return;
        }
        let c = self.p.outline.gamma_multiply(emphasis);
        let i = r.shrink(w / 2.0);
        if i.width() <= 0.0 || i.height() <= 0.0 {
            // Thinner than the outline: draw it as one line along its length.
            let (a, b) = if r.width() >= r.height() {
                (Pos2::new(r.min.x, r.center().y), Pos2::new(r.max.x, r.center().y))
            } else {
                (Pos2::new(r.center().x, r.min.y), Pos2::new(r.center().x, r.max.y))
            };
            self.segment(a, b, 0.0, c);
            return;
        }
        // Horizontal edges cover the corners, vertical ones fill in between.
        self.segment(i.left_top(), i.right_top(), w / 2.0, c);
        self.segment(i.left_bottom(), i.right_bottom(), w / 2.0, c);
        self.segment(i.left_top() + Vec2::new(0.0, w / 2.0), i.left_bottom() - Vec2::new(0.0, w / 2.0), 0.0, c);
        self.segment(i.right_top() + Vec2::new(0.0, w / 2.0), i.right_bottom() - Vec2::new(0.0, w / 2.0), 0.0, c);
        if self.p.x {
            self.segment(i.left_top(), i.right_bottom(), 0.0, c);
            self.segment(i.left_bottom(), i.right_top(), 0.0, c);
        }
    }

    /// A closed screen polygon (any simple polygon; convex or not).
    pub fn polygon(&mut self, pts: &[Pos2], emphasis: f32) {
        if pts.len() < 3 {
            return;
        }
        if !matches!(self.p.kind, Fill::None) && pts.len() <= MAX_FILL_POINTS {
            let c = self.p.fill.gamma_multiply(emphasis);
            let base = self.fill.vertices.len() as u32;
            for &p in pts {
                self.fill_vertex(p, c);
            }
            for t in triangulate(pts) {
                self.fill.indices.extend(t.iter().map(|&k| base + k as u32));
            }
        }
        let c = self.p.outline.gamma_multiply(emphasis);
        let ext = self.p.width / 2.0;
        for k in 0..pts.len() {
            self.segment(pts[k], pts[(k + 1) % pts.len()], ext, c);
        }
        if self.p.x {
            let b = Rect::from_points(pts);
            if pts.len() == 4 {
                self.segment(pts[0], pts[2], 0.0, c);
                self.segment(pts[1], pts[3], 0.0, c);
            } else {
                self.segment(b.left_top(), b.right_bottom(), 0.0, c);
                self.segment(b.left_bottom(), b.right_top(), 0.0, c);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.fill.is_empty() && self.line.is_empty()
    }

    /// The fill under the outline, as one shape.
    pub fn finish(self) -> egui::Shape {
        let shapes: Vec<egui::Shape> =
            [self.fill, self.line].into_iter().filter(|m| !m.is_empty()).map(egui::Shape::mesh).collect();
        egui::Shape::Vec(shapes)
    }
}

fn cross(o: Pos2, a: Pos2, b: Pos2) -> f32 {
    (a - o).x * (b - o).y - (a - o).y * (b - o).x
}

/// Triangles (vertex indices) covering a simple polygon, by ear clipping. Collinear and
/// repeated points are skipped; a polygon that cannot be clipped further keeps what was found.
pub(crate) fn triangulate(pts: &[Pos2]) -> Vec<[usize; 3]> {
    let n = pts.len();
    let mut out = Vec::with_capacity(n.saturating_sub(2));
    if n < 3 {
        return out;
    }
    let area: f32 = (0..n).map(|k| cross(Pos2::ZERO, pts[k], pts[(k + 1) % n])).sum();
    let s = if area >= 0.0 { 1.0 } else { -1.0 };
    let mut idx: Vec<usize> = (0..n).collect();
    let (mut i, mut stall) = (0, 0);
    while idx.len() > 3 {
        let m = idx.len();
        if stall > m {
            break;
        }
        let (ia, ib, ic) = (idx[(i + m - 1) % m], idx[i % m], idx[(i + 1) % m]);
        let (a, b, c) = (pts[ia], pts[ib], pts[ic]);
        let turn = cross(a, b, c) * s;
        if turn.abs() <= 1e-6 * (1.0 + (b - a).length() * (c - b).length()) {
            // Collinear or repeated: drop the middle point.
            idx.remove(i % m);
            stall = 0;
            continue;
        }
        let ear = turn > 0.0
            && idx.iter().all(|&k| {
                let p = pts[k];
                k == ia
                    || k == ib
                    || k == ic
                    || p == a
                    || p == b
                    || p == c
                    || !(cross(a, b, p) * s > 0.0 && cross(b, c, p) * s > 0.0 && cross(c, a, p) * s > 0.0)
            });
        if ear {
            out.push([ia, ib, ic]);
            idx.remove(i % m);
            stall = 0;
        } else {
            i += 1;
            stall += 1;
        }
        if !idx.is_empty() {
            i %= idx.len();
        }
    }
    if idx.len() == 3 && cross(pts[idx[0]], pts[idx[1]], pts[idx[2]]).abs() > 1e-6 {
        out.push([idx[0], idx[1], idx[2]]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(pts: &[Pos2], tris: &[[usize; 3]]) -> f32 {
        tris.iter().map(|t| cross(pts[t[0]], pts[t[1]], pts[t[2]]).abs() / 2.0).sum()
    }

    #[test]
    fn triangulates_concave_polygons() {
        let p = |x: f32, y: f32| Pos2::new(x, y);
        // An L shape (area 3), both orientations, with a collinear point.
        let l = [p(0.0, 0.0), p(2.0, 0.0), p(2.0, 1.0), p(1.0, 1.0), p(1.0, 2.0), p(0.0, 2.0), p(0.0, 1.0)];
        assert!((area(&l, &triangulate(&l)) - 3.0).abs() < 1e-4);
        let rev: Vec<Pos2> = l.iter().rev().copied().collect();
        assert!((area(&rev, &triangulate(&rev)) - 3.0).abs() < 1e-4);
        // A U shape (area 7).
        let u = [p(0.0, 0.0), p(3.0, 0.0), p(3.0, 3.0), p(2.0, 3.0), p(2.0, 1.0), p(1.0, 1.0), p(1.0, 3.0), p(0.0, 3.0)];
        assert!((area(&u, &triangulate(&u)) - 7.0).abs() < 1e-4);
        assert!(triangulate(&[p(0.0, 0.0), p(1.0, 0.0)]).is_empty());
    }
}
