//! Virtuoso-style editing aids: gravity snapping, design-rule-driven (DRD) spacing feedback,
//! the yank / paste clipboard, chop, align / distribute and wire layer switching.

use std::collections::BTreeSet;

use eframe::egui::{self, Rect, Stroke, StrokeKind};
use ics_core::geom::IRect;
use ics_core::hier::{CellIndex, Hier, LayoutSource};
use ics_core::layout::{CellInst, Geometry, Layout, Shape, Trans};
use ics_core::project::CellRef;
use ics_core::region::{BoolOp, Region};
use ics_core::tech::Tech;
use serde::{Deserialize, Serialize};

use super::{LayCtx, LayoutEditor, Live, manhattan, screen_rect, um};
use crate::canvas::View2D;
use crate::theme::{self, Palette};

fn rect_outline(r: IRect) -> [(i64, i64); 4] {
    [(r.x0, r.y0), (r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1)]
}

/// Nearest vertex (preferred) or edge point of a visible top-level shape or instance
/// extent within `tol` of `p`. Returns the point and whether it is a vertex.
#[allow(clippy::too_many_arguments)]
pub(super) fn gravity(
    l: &Layout,
    idx: &CellIndex,
    visible: &[bool],
    p: (f64, f64),
    tol: f64,
    grid: i64,
) -> Option<((i64, i64), bool)> {
    let t = tol.ceil() as i64 + 1;
    let area = IRect::new(p.0 as i64 - t, p.1 as i64 - t, p.0 as i64 + t, p.1 as i64 + t);
    let g = grid.max(1);
    let snap = |v: f64| (v / g as f64).round() as i64 * g;
    let dist = |q: (i64, i64)| ((q.0 as f64 - p.0).powi(2) + (q.1 as f64 - p.1).powi(2)).sqrt();
    let mut vert: Option<(f64, (i64, i64))> = None;
    let mut edge: Option<(f64, (i64, i64))> = None;
    let mut outline = |pts: &[(i64, i64)], closed: bool| {
        for &q in pts {
            let d = dist(q);
            if d <= tol && vert.is_none_or(|(bd, _)| d < bd) {
                vert = Some((d, q));
            }
        }
        let n = pts.len();
        let m = if closed { n } else { n.saturating_sub(1) };
        for i in 0..m {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            let q = if a.1 == b.1 {
                (snap(p.0).clamp(a.0.min(b.0), a.0.max(b.0)), a.1)
            } else if a.0 == b.0 {
                (a.0, snap(p.1).clamp(a.1.min(b.1), a.1.max(b.1)))
            } else {
                continue;
            };
            let d = dist(q);
            if d <= tol && edge.is_none_or(|(bd, _)| d < bd) {
                edge = Some((d, q));
            }
        }
    };
    for (li, vis) in visible.iter().enumerate() {
        if !vis {
            continue;
        }
        for i in idx.shapes_in(li, area) {
            let Some(s) = l.shapes.get(i) else { continue };
            match &s.geom {
                Geometry::Rect(r) => outline(&rect_outline(*r), true),
                Geometry::Polygon { points } => outline(points, true),
                Geometry::Path { points, .. } => {
                    outline(points, false);
                    for r in s.geom.rects() {
                        outline(&rect_outline(r), true);
                    }
                }
                Geometry::Label { at, .. } => outline(&[*at], false),
            }
        }
    }
    for i in idx.instances_in(area) {
        if let (Some(b), Some(inst)) = (idx.instance_bbox(l, i), l.instances.get(i)) {
            outline(&rect_outline(b), true);
            outline(&[(inst.trans.x, inst.trans.y)], false);
        }
    }
    vert.map(|v| (v.1, true)).or(edge.map(|e| (e.1, false)))
}

/// Region pieces of a shape for rule checks (Manhattan only).
pub(super) fn shape_rects(g: &Geometry) -> Vec<IRect> {
    match g {
        Geometry::Label { .. } => vec![],
        _ => Region::from_geometry(g).map(|r| r.rects().to_vec()).unwrap_or_else(|_| g.rects()),
    }
}

/// The empty space between two non-overlapping rectangles.
fn gap_rect(a: IRect, b: IRect) -> IRect {
    let span = |a0: i64, a1: i64, b0: i64, b1: i64| {
        if a1 <= b0 {
            (a1, b0)
        } else if b1 <= a0 {
            (b1, a0)
        } else {
            (a0.max(b0), a1.min(b1))
        }
    };
    let (x0, x1) = span(a.x0, a.x1, b.x0, b.x1);
    let (y0, y1) = span(a.y0, a.y1, b.y0, b.y1);
    IRect::new(x0, y0, x1, y1)
}

/// Hierarchy levels searched below a cell for DRD neighbours.
const DRD_DEPTH: u32 = 16;
/// Most neighbour rectangles gathered for one candidate.
const DRD_NEAR: usize = 4096;

/// Rectangles on `layer` touching `area` (in the frame `t` maps `layout` into) from `layout`
/// and the cells it places. At the top level, items in `skip` are left out; `path` names the
/// placement to leave out (the cell edited in place), one instance and element per level.
#[allow(clippy::too_many_arguments)]
fn neighbours(
    src: &dyn LayoutSource,
    hier: &mut Hier,
    cell: &CellRef,
    layout: &Layout,
    t: Trans,
    depth: u32,
    skip: &BTreeSet<u64>,
    path: Option<&[(u64, (u32, u32))]>,
    layer: usize,
    area: IRect,
    out: &mut Vec<IRect>,
) {
    let local = t.inverse().apply_rect(area);
    let idx = hier.get_for(cell, layout, src);
    for i in idx.shapes_in(layer, local) {
        let Some(s) = layout.shapes.get(i).filter(|s| depth > 0 || !skip.contains(&s.id)) else { continue };
        out.extend(shape_rects(&s.geom).into_iter().filter(|r| r.intersects(&local)).map(|r| t.apply_rect(r)));
    }
    if depth >= DRD_DEPTH {
        return;
    }
    for i in idx.instances_in(local) {
        let (Some(inst), Some(cb)) = (layout.instances.get(i), idx.child_bbox(i)) else { continue };
        if depth == 0 && skip.contains(&inst.id) {
            continue;
        }
        let Some(child) = src.layout(&inst.cell) else { continue };
        for e in inst.visible_elements(cb, local) {
            let next = match path {
                Some([(id, el), rest @ ..]) if *id == inst.id && *el == e => {
                    if rest.is_empty() {
                        continue;
                    }
                    Some(rest)
                }
                _ => None,
            };
            if out.len() >= DRD_NEAR {
                return;
            }
            let tc = t.compose(&inst.element_trans(e.0, e.1));
            neighbours(src, hier, &inst.cell, child, tc, depth + 1, skip, next, layer, area, out);
        }
    }
}

/// Design-rule-driven feedback for shapes being drawn or moved: gaps closer than the minimum
/// spacing to same-layer shapes of the cell, of the cells it places and (edited in place) of
/// the surrounding cells, and (if `width`) candidates narrower than the minimum width.
pub(super) fn drd_marks(l: &Layout, cx: &mut LayCtx, cands: &[(usize, IRect)], skip: &BTreeSet<u64>, width: bool) -> Vec<IRect> {
    let tech = cx.tech;
    let live = Live { src: cx.src, cell: cx.cell, layout: l };
    let root = cx.context.and_then(|c| Some((c, live.layout(&c.root)?)));
    let none = BTreeSet::new();
    let mut out = vec![];
    let mut near = vec![];
    for &(layer, r) in cands.iter().take(512) {
        let Some(def) = tech.layer(layer) else { continue };
        if width && def.min_width > 0 && r.width().min(r.height()) < def.min_width {
            out.push(r);
        }
        let sp = def.min_spacing;
        if sp <= 0 {
            continue;
        }
        let area = IRect::new(r.x0 - sp, r.y0 - sp, r.x1 + sp, r.y1 + sp);
        near.clear();
        neighbours(&live, cx.hier, cx.cell, l, Trans::IDENTITY, 0, skip, None, layer, area, &mut near);
        if let Some((c, rl)) = root {
            let path = Some(c.path.as_slice());
            neighbours(&live, cx.hier, &c.root, rl, c.to_root.inverse(), 0, &none, path, layer, area, &mut near);
        }
        for o in &near {
            let d = r.spacing(o);
            if d > 0 && d < sp && !r.intersects(o) {
                out.push(gap_rect(r, *o));
            }
        }
        if out.len() > 500 {
            break;
        }
    }
    out.sort_by_key(|r| (r.x0, r.y0, r.x1, r.y1));
    out.dedup();
    out
}

/// Edges of a magnet candidate that follow the cursor; `None` moves both edges of that axis.
#[derive(Clone, Copy, Default)]
pub(super) struct Edges {
    pub x: Option<i64>,
    pub y: Option<i64>,
}

/// Spacing magnet: the smallest shift (at most `tol` per axis) that puts a candidate exactly
/// at the minimum spacing from a same-layer neighbour, or abutting it. Returns the shift and
/// the gaps at exactly the minimum spacing after it (for display).
pub(super) fn magnet(
    l: &Layout,
    cx: &mut LayCtx,
    cands: &[(usize, IRect)],
    skip: &BTreeSet<u64>,
    edges: Edges,
    tol: i64,
) -> ((i64, i64), Vec<(IRect, i64)>) {
    let tech = cx.tech;
    let live = Live { src: cx.src, cell: cx.cell, layout: l };
    let root = cx.context.and_then(|c| Some((c, live.layout(&c.root)?)));
    let none = BTreeSet::new();
    let mut near: Vec<(usize, i64, IRect)> = vec![];
    let mut buf = vec![];
    let (mut bx, mut by): (Option<i64>, Option<i64>) = (None, None);
    let better = |b: Option<i64>, d: i64| d.abs() <= tol && b.is_none_or(|b| d.abs() < b.abs());
    for &(layer, r) in cands.iter().take(64) {
        let Some(def) = tech.layer(layer) else { continue };
        let sp = def.min_spacing;
        if sp <= 0 {
            continue;
        }
        let g = sp + tol;
        let area = IRect::new(r.x0 - g, r.y0 - g, r.x1 + g, r.y1 + g);
        buf.clear();
        neighbours(&live, cx.hier, cx.cell, l, Trans::IDENTITY, 0, skip, None, layer, area, &mut buf);
        if let Some((c, rl)) = root {
            let path = Some(c.path.as_slice());
            neighbours(&live, cx.hier, &c.root, rl, c.to_root.inverse(), 0, &none, path, layer, area, &mut buf);
        }
        for &o in &buf {
            if r.intersects(&o) {
                continue;
            }
            near.push((layer, sp, o));
            if r.y0 < o.y1 && o.y0 < r.y1 {
                for (edge, d) in [(r.x0, o.x1 + sp - r.x0), (r.x1, o.x0 - sp - r.x1), (r.x0, o.x1 - r.x0), (r.x1, o.x0 - r.x1)] {
                    if edges.x.is_none_or(|e| e == edge) && better(bx, d) {
                        bx = Some(d);
                    }
                }
            }
            if r.x0 < o.x1 && o.x0 < r.x1 {
                for (edge, d) in [(r.y0, o.y1 + sp - r.y0), (r.y1, o.y0 - sp - r.y1), (r.y0, o.y1 - r.y0), (r.y1, o.y0 - r.y1)] {
                    if edges.y.is_none_or(|e| e == edge) && better(by, d) {
                        by = Some(d);
                    }
                }
            }
        }
    }
    let d = (bx.unwrap_or(0), by.unwrap_or(0));
    let mut gaps = vec![];
    if d != (0, 0) {
        for &(layer, r) in cands.iter().take(64) {
            let mv = |v: i64, e: Option<i64>, dv: i64| if e.is_none_or(|e| e == v) { v + dv } else { v };
            let r = IRect::new(mv(r.x0, edges.x, d.0), mv(r.y0, edges.y, d.1), mv(r.x1, edges.x, d.0), mv(r.y1, edges.y, d.1));
            for &(_, sp, o) in near.iter().filter(|n| n.0 == layer) {
                if !r.intersects(&o) && r.spacing(&o) == sp {
                    gaps.push((gap_rect(r, o), sp));
                }
            }
        }
    }
    gaps.sort_by_key(|(r, _)| (r.x0, r.y0, r.x1, r.y1));
    gaps.dedup();
    (d, gaps)
}

pub(super) fn draw_magnet(painter: &egui::Painter, view: &View2D, rect: Rect, pal: &Palette, gaps: &[(IRect, i64)], tech: &Tech) {
    for &(g, d) in gaps.iter().take(16) {
        let r = screen_rect(view, rect, g).expand(1.0);
        painter.rect_filled(r, 0.0, pal.ok.gamma_multiply(0.25));
        painter.rect_stroke(r, 0.0, Stroke::new(1.2, pal.ok), StrokeKind::Middle);
        painter.text(
            r.center_top() - egui::vec2(0.0, 2.0),
            egui::Align2::CENTER_BOTTOM,
            um(d, tech),
            egui::FontId::monospace(10.0),
            pal.ok,
        );
    }
}

/// DRD candidates of a placement moved by `t`: the shapes of its cell within the largest
/// minimum spacing of its outline (only those can come close to its surroundings).
pub(super) fn inst_cands(cx: &mut LayCtx, inst: &CellInst, t: Trans, out: &mut Vec<(usize, IRect)>) {
    let Some(child) = cx.src.layout(&inst.cell) else { return };
    let Some(cb) = cx.hier.get_for(&inst.cell, child, cx.src).bbox else { return };
    let (c, r) = inst.dims();
    let none = BTreeSet::new();
    let mut near = vec![];
    for (layer, def) in cx.tech.layers.iter().enumerate() {
        let sp = def.min_spacing;
        if sp <= 0 {
            continue;
        }
        for (i, j) in (0..r).flat_map(|j| (0..c).map(move |i| (i, j))).take(64) {
            let tc = t.compose(&inst.element_trans(i, j));
            let b = tc.apply_rect(cb);
            let bands = [
                IRect::new(b.x0, b.y0, b.x0 + sp, b.y1),
                IRect::new(b.x1 - sp, b.y0, b.x1, b.y1),
                IRect::new(b.x0, b.y0, b.x1, b.y0 + sp),
                IRect::new(b.x0, b.y1 - sp, b.x1, b.y1),
            ];
            for band in bands {
                near.clear();
                neighbours(cx.src, cx.hier, &inst.cell, child, tc, 1, &none, None, layer, band, &mut near);
                out.extend(near.iter().map(|&o| (layer, o)));
            }
            if out.len() > 512 {
                return;
            }
        }
    }
    out.sort_by_key(|&(l, r)| (l, r.x0, r.y0, r.x1, r.y1));
    out.dedup();
}

pub(super) fn draw_drd(painter: &egui::Painter, view: &View2D, rect: Rect, pal: &Palette, marks: &[IRect]) {
    for m in marks {
        let r = screen_rect(view, rect, *m).expand(1.0);
        painter.rect_filled(r, 0.0, pal.danger.gamma_multiply(0.3));
        painter.rect_stroke(r, 0.0, Stroke::new(1.5, pal.danger), StrokeKind::Middle);
    }
}

/// Dashed outline of a world rectangle.
pub(super) fn dashed_rect(painter: &egui::Painter, r: Rect, st: Stroke) {
    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    painter.add(egui::Shape::dashed_line(&pts, st, 5.0, 4.0));
}

/// Items copied with Ctrl+C, relative to their reference point (lower-left of their extent).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Clip {
    pub shapes: Vec<Shape>,
    pub instances: Vec<CellInst>,
    /// Extents of the instances, relative like the items.
    #[serde(default)]
    pub inst_boxes: Vec<IRect>,
    pub bbox: IRect,
}

const CLIP_FORMAT: &str = "ics-layout-clipboard/1";

#[derive(Serialize, Deserialize)]
struct ClipDoc {
    format: String,
    clip: Clip,
}

impl Clip {
    pub fn to_text(&self) -> String {
        serde_json::to_string(&ClipDoc { format: CLIP_FORMAT.into(), clip: self.clone() }).unwrap_or_default()
    }

    pub fn from_text(s: &str) -> Option<Clip> {
        serde_json::from_str::<ClipDoc>(s).ok().filter(|d| d.format == CLIP_FORMAT).map(|d| d.clip)
    }

    pub fn count(&self) -> usize {
        self.shapes.len() + self.instances.len()
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum AlignOp {
    Left,
    HCenter,
    Right,
    Bottom,
    VCenter,
    Top,
    DistributeH,
    DistributeV,
}

impl AlignOp {
    const ROW1: [(AlignOp, &'static str, &'static str); 4] = [
        (AlignOp::Left, "Left", "Align left edges"),
        (AlignOp::HCenter, "Center X", "Align horizontal centres"),
        (AlignOp::Right, "Right", "Align right edges"),
        (AlignOp::DistributeH, "Space X", "Equal horizontal gaps (3 or more items)"),
    ];
    const ROW2: [(AlignOp, &'static str, &'static str); 4] = [
        (AlignOp::Bottom, "Bottom", "Align bottom edges"),
        (AlignOp::VCenter, "Center Y", "Align vertical centres"),
        (AlignOp::Top, "Top", "Align top edges"),
        (AlignOp::DistributeV, "Space Y", "Equal vertical gaps (3 or more items)"),
    ];
}

/// Via that connects `layer` to the routing layer above (`up`) or below it: (via, new layer).
pub(super) fn wire_via(tech: &Tech, layer: usize, up: bool) -> Option<(usize, usize)> {
    let name = &tech.layers.get(layer)?.name;
    tech.vias.iter().enumerate().find_map(|(i, v)| {
        let (from, to) = if up { (&v.bottom, &v.top) } else { (&v.top, &v.bottom) };
        if from == name { tech.layer_index(to).map(|t| (i, t)) } else { None }
    })
}

impl LayoutEditor {
    /// Extent of every selected item.
    pub(super) fn item_boxes(&self, l: &Layout, cx: &mut LayCtx) -> Vec<(u64, IRect)> {
        let idx = cx.index(l);
        let mut out: Vec<(u64, IRect)> =
            l.shapes.iter().filter(|s| self.selection.contains(&s.id)).map(|s| (s.id, s.bbox())).collect();
        for (i, inst) in l.instances.iter().enumerate() {
            if self.selection.contains(&inst.id)
                && let Some(b) = idx.instance_bbox(l, i)
            {
                out.push((inst.id, b));
            }
        }
        out
    }

    /// Copy the selection to the clipboard. Returns its text form for the system clipboard.
    pub(super) fn yank(&mut self, l: &Layout, cx: &mut LayCtx) -> Option<String> {
        let boxes = self.item_boxes(l, cx);
        let bb = boxes.iter().map(|b| b.1).reduce(|a, b| a.union(&b))?;
        let g = cx.tech.grid.max(1);
        let (ox, oy) = (bb.x0.div_euclid(g) * g, bb.y0.div_euclid(g) * g);
        let idx = cx.index(l);
        let mut clip = Clip { shapes: vec![], instances: vec![], inst_boxes: vec![], bbox: bb.translate(-ox, -oy) };
        for s in l.shapes.iter().filter(|s| self.selection.contains(&s.id)) {
            let mut s = s.clone();
            s.translate(-ox, -oy);
            clip.shapes.push(s);
        }
        for (i, inst) in l.instances.iter().enumerate().filter(|(_, i)| self.selection.contains(&i.id)) {
            let mut inst = inst.clone();
            inst.translate(-ox, -oy);
            clip.instances.push(inst);
            clip.inst_boxes.push(idx.instance_bbox(l, i).unwrap_or_default().translate(-ox, -oy));
        }
        let text = clip.to_text();
        self.note(true, format!("Copied {} item(s)", clip.count()));
        self.clip = Some(clip);
        Some(text)
    }

    /// Start pasting: the clipboard follows the cursor until a click drops it.
    pub(super) fn begin_paste(&mut self, text: Option<&str>) -> bool {
        let clip = text.and_then(Clip::from_text).or_else(|| self.clip.clone());
        match clip {
            Some(c) if c.count() > 0 => {
                self.clip = Some(c.clone());
                self.paste = Some(c);
                true
            }
            _ => false,
        }
    }

    pub(super) fn drop_paste(&mut self, l: &mut Layout, at: (i64, i64), src: &ics_core::project::Project) -> bool {
        let Some(clip) = self.paste.take() else { return false };
        self.history.push(l);
        let mut sel = BTreeSet::new();
        for s in &clip.shapes {
            let mut s = s.clone();
            s.translate(at.0, at.1);
            sel.insert(l.add(s.layer, s.geom));
        }
        let mut skipped = 0;
        for i in &clip.instances {
            if src.cell(&i.cell).is_none() {
                skipped += 1;
                continue;
            }
            let mut i = i.clone();
            i.translate(at.0, at.1);
            let id = l.add_instance(i.cell, i.trans, i.array);
            if let (Some(name), Some(n)) = (i.name, l.instance_mut(id)) {
                n.name = Some(name);
            }
            sel.insert(id);
        }
        self.selection = sel;
        let mut m = format!("Pasted {} item(s)", clip.count() - skipped);
        if skipped > 0 {
            m.push_str(&format!(" · {skipped} instance(s) of cells missing from this project skipped"));
        }
        self.note(skipped == 0, m);
        true
    }

    /// Copy the selection offset by `d`. Returns the ids of the copies.
    pub(super) fn copy_by(&mut self, l: &mut Layout, d: (i64, i64)) -> BTreeSet<u64> {
        let mut out = BTreeSet::new();
        for id in self.selection.clone() {
            if let Some(s) = l.shape(id).cloned() {
                let mut s = s;
                s.translate(d.0, d.1);
                out.insert(l.add(s.layer, s.geom));
            } else if let Some(i) = l.instance(id).cloned() {
                let mut i = i;
                i.translate(d.0, d.1);
                out.insert(l.add_instance(i.cell, i.trans, i.array));
            }
        }
        out
    }

    /// Cut the box `b` out of the selected shapes, or of all selectable shapes it touches.
    pub(super) fn chop(&mut self, l: &mut Layout, cx: &mut LayCtx, b: IRect) -> bool {
        let sel_shapes = l.shapes.iter().any(|s| self.selection.contains(&s.id));
        let pick = self.pickable();
        let targets: Vec<Shape> = l
            .shapes
            .iter()
            .filter(|s| !matches!(s.geom, Geometry::Label { .. }) && s.bbox().intersects(&b))
            .filter(|s| if sel_shapes { self.selection.contains(&s.id) } else { pick.get(s.layer).copied().unwrap_or(false) })
            .cloned()
            .collect();
        if targets.is_empty() {
            self.note(false, "Chop: no shape under the box");
            return false;
        }
        self.history.push(l);
        let cut = Region::from_rects([b]);
        let (mut done, mut skipped) = (0, 0);
        let mut sel = BTreeSet::new();
        for s in targets {
            let Ok(r) = Region::from_geometry(&s.geom) else {
                skipped += 1;
                continue;
            };
            l.remove(s.id);
            self.selection.remove(&s.id);
            for g in r.boolean(&cut, BoolOp::Not).to_geometry() {
                sel.insert(l.add(s.layer, g));
            }
            done += 1;
        }
        if sel_shapes {
            self.selection.extend(sel);
        }
        cx.touched();
        let mut m = format!("Chopped {done} shape(s)");
        if skipped > 0 {
            m.push_str(&format!(" · {skipped} non-Manhattan shape(s) left unchanged"));
        }
        self.note(skipped == 0, m);
        done > 0
    }

    /// Align or distribute the selected items within their common extent, as seen on screen.
    pub(super) fn align(&mut self, l: &mut Layout, cx: &mut LayCtx, op: AlignOp) -> bool {
        let o = self.view.orient;
        let mut boxes: Vec<(u64, IRect)> = self.item_boxes(l, cx).into_iter().map(|(id, b)| (id, o.apply_rect(b))).collect();
        let min = if matches!(op, AlignOp::DistributeH | AlignOp::DistributeV) { 3 } else { 2 };
        if boxes.len() < min {
            self.note(false, format!("Select at least {min} items"));
            return false;
        }
        let g = cx.tech.grid.max(1);
        let snap = |v: i64| (v as f64 / g as f64).round() as i64 * g;
        let bb = boxes.iter().map(|b| b.1).reduce(|a, b| a.union(&b)).unwrap_or_default();
        let mut moves: Vec<(u64, i64, i64)> = vec![];
        match op {
            AlignOp::DistributeH | AlignOp::DistributeV => {
                let h = op == AlignOp::DistributeH;
                let lo = |r: &IRect| if h { r.x0 } else { r.y0 };
                let len = |r: &IRect| if h { r.width() } else { r.height() };
                boxes.sort_by_key(|b| lo(&b.1));
                let total: i64 = boxes.iter().map(|b| len(&b.1)).sum();
                let span = if h { bb.width() } else { bb.height() };
                let gap = ((span - total) / (boxes.len() as i64 - 1)).div_euclid(g) * g;
                let mut at = if h { bb.x0 } else { bb.y0 };
                for (id, r) in &boxes {
                    let d = at - lo(r);
                    moves.push(if h { (*id, d, 0) } else { (*id, 0, d) });
                    at += len(r) + gap;
                }
            }
            _ => {
                for (id, r) in &boxes {
                    let d = match op {
                        AlignOp::Left => (bb.x0 - r.x0, 0),
                        AlignOp::Right => (bb.x1 - r.x1, 0),
                        AlignOp::HCenter => (snap((bb.x0 + bb.x1) / 2 - (r.x0 + r.x1) / 2), 0),
                        AlignOp::Bottom => (0, bb.y0 - r.y0),
                        AlignOp::Top => (0, bb.y1 - r.y1),
                        _ => (0, snap((bb.y0 + bb.y1) / 2 - (r.y0 + r.y1) / 2)),
                    };
                    moves.push((*id, d.0, d.1));
                }
            }
        }
        moves.retain(|m| m.1 != 0 || m.2 != 0);
        if moves.is_empty() {
            return false;
        }
        self.history.push(l);
        for (id, dx, dy) in moves {
            let (dx, dy) = o.inverse().apply_vec((dx, dy));
            l.translate_item(id, dx, dy);
        }
        true
    }

    /// While drawing a path, finish it on the current layer, drop a via at the current point and
    /// continue on the next routing layer up or down.
    pub(super) fn switch_wire_layer(&mut self, l: &mut Layout, tech: &Tech, up: bool) -> bool {
        let Some((via, next)) = wire_via(tech, self.active_layer, up) else {
            let name = tech.layer(self.active_layer).map_or("?", |d| d.name.as_str());
            self.note(false, format!("No via from {name} {}", if up { "up" } else { "down" }));
            return false;
        };
        let Some(&last) = self.path_pts.last() else {
            self.active_layer = next;
            self.visible[next] = true;
            return false;
        };
        let p = self.cursor.map_or(last, |c| manhattan(last, c));
        if p != last {
            self.path_pts.push(p);
        }
        let mut changed = self.finish_path(l, tech);
        self.ed.via = via;
        changed |= self.place_via(l, tech, p, None);
        let min = tech.layer(next).map_or(0, |d| d.min_width);
        if self.path_width > 0 && self.path_width < min {
            self.path_width = 0;
        }
        self.active_layer = next;
        self.visible[next] = true;
        self.path_pts = vec![p];
        self.note(
            true,
            format!("{} at ({}, {}) µm, now on {}", tech.vias[via].name, um(p.0, tech), um(p.1, tech), tech.layers[next].name),
        );
        changed
    }

    /// Align / distribute buttons of the inspector.
    pub(super) fn align_panel(&mut self, ui: &mut egui::Ui, l: &mut Layout, cx: &mut LayCtx) -> bool {
        if self.selection.len() < 2 {
            return false;
        }
        let mut changed = false;
        ui.add_space(6.0);
        theme::section(ui, "Align");
        for row in [AlignOp::ROW1, AlignOp::ROW2] {
            ui.horizontal_wrapped(|ui| {
                for (op, name, tip) in row {
                    if ui.small_button(name).on_hover_text(tip).clicked() {
                        changed |= self.align(l, cx, op);
                    }
                }
            });
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gap_between_rects() {
        let a = IRect::new(0, 0, 100, 100);
        assert_eq!(gap_rect(a, IRect::new(130, 20, 200, 80)), IRect::new(100, 20, 130, 80));
        assert_eq!(gap_rect(a, IRect::new(20, -50, 60, -10)), IRect::new(20, -10, 60, 0));
    }

    #[test]
    fn clipboard_round_trips_through_text() {
        let clip = Clip {
            shapes: vec![Shape { id: 3, layer: 2, geom: Geometry::Rect(IRect::new(0, 0, 10, 20)) }],
            instances: vec![],
            inst_boxes: vec![],
            bbox: IRect::new(0, 0, 10, 20),
        };
        assert_eq!(Clip::from_text(&clip.to_text()), Some(clip));
        assert_eq!(Clip::from_text("hello"), None);
    }
}
