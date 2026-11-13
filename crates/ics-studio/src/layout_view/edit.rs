//! Layout editing operations: vertex / edge handles, polygon tool, selection transforms,
//! Boolean and sizing operations, flatten / make cell, vias and PCells.

use std::collections::BTreeSet;

use eframe::egui::{self, Pos2, Rect, Stroke, StrokeKind, Vec2};
use ics_core::geom::IRect;
use ics_core::hier::{self, LayoutSource};
use ics_core::layout::{ArraySpec, Geometry, Layout, Shape, Trans};
use ics_core::pcell::{self, PCell};
use ics_core::project::CellRef;
use ics_core::region::{BoolOp, Region};
use ics_core::tech::Tech;

use super::{LayCtx, LayoutEditor, um};
use crate::canvas::View2D;
use crate::theme::{self, Palette};

/// Edits the editor cannot perform itself because they change other cells of the project.
#[derive(Debug, Clone)]
pub enum LayRequest {
    Open(CellRef),
    /// Descend into a placed cell (one array element, `trans` maps it into this cell),
    /// remembering this one for [`LayRequest::Return`]. In place, the parent stays visible
    /// around it; otherwise the cell opens on its own.
    Descend {
        cell: CellRef,
        inst: u64,
        elem: (u32, u32),
        trans: Trans,
        in_place: bool,
    },
    /// Go back to the cell descended from.
    Return,
    PlacePCell {
        pcell: PCell,
        trans: Trans,
        array: Option<ArraySpec>,
    },
    SetPCell {
        inst: u64,
        pcell: PCell,
    },
    MakeCell {
        ids: BTreeSet<u64>,
        origin: (i64, i64),
    },
    /// Store a DRC rule deck in the project's technology.
    SetDeck(String),
    /// Open the DRC Rule Designer window.
    OpenRuleDesigner,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Handle {
    Vertex(usize),
    Edge(usize),
}

/// Editable outline of a shape: (points, closed).
fn outline(g: &Geometry) -> Option<(Vec<(i64, i64)>, bool)> {
    match g {
        Geometry::Rect(r) => Some((vec![(r.x0, r.y0), (r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1)], true)),
        Geometry::Polygon { points } => Some((points.clone(), true)),
        Geometry::Path { points, .. } => Some((points.clone(), false)),
        Geometry::Label { .. } => None,
    }
}

fn edges(n: usize, closed: bool) -> usize {
    if closed { n } else { n.saturating_sub(1) }
}

/// Screen positions of all handles of a shape.
pub(super) fn handles(g: &Geometry, view: &View2D, rect: Rect) -> Vec<(Handle, Pos2)> {
    let Some((pts, closed)) = outline(g) else { return vec![] };
    let s = |p: (i64, i64)| view.to_screen(rect, p.0 as f64, p.1 as f64);
    let mut out: Vec<(Handle, Pos2)> = pts.iter().enumerate().map(|(i, p)| (Handle::Vertex(i), s(*p))).collect();
    for i in 0..edges(pts.len(), closed) {
        let (a, b) = (s(pts[i]), s(pts[(i + 1) % pts.len()]));
        if a.distance(b) > 18.0 {
            out.push((Handle::Edge(i), a + (b - a) * 0.5));
        }
    }
    out
}

pub(super) fn hit_handle(g: &Geometry, view: &View2D, rect: Rect, at: Pos2) -> Option<Handle> {
    handles(g, view, rect)
        .into_iter()
        .filter(|(_, p)| p.distance(at) <= 7.0)
        .min_by(|a, b| a.1.distance(at).total_cmp(&b.1.distance(at)))
        .map(|h| h.0)
}

pub(super) fn draw_handles(painter: &egui::Painter, g: &Geometry, view: &View2D, rect: Rect, pal: &Palette, hot: Option<Handle>) {
    for (h, p) in handles(g, view, rect) {
        let col = if Some(h) == hot { pal.warning } else { pal.selection };
        match h {
            Handle::Vertex(_) => {
                let r = Rect::from_center_size(p, Vec2::splat(7.0));
                painter.rect_filled(r, 0.0, pal.bg_panel);
                painter.rect_stroke(r, 0.0, Stroke::new(1.5, col), StrokeKind::Middle);
            }
            Handle::Edge(_) => {
                painter.circle_filled(p, 3.5, pal.bg_panel);
                painter.circle_stroke(p, 3.5, Stroke::new(1.5, col));
            }
        }
    }
}

fn is_manhattan(pts: &[(i64, i64)], closed: bool) -> bool {
    let n = pts.len();
    (0..edges(n, closed)).all(|i| {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        a.0 == b.0 || a.1 == b.1
    })
}

/// Drop repeated and collinear vertices.
fn cleanup(mut pts: Vec<(i64, i64)>, closed: bool) -> Vec<(i64, i64)> {
    pts.dedup();
    if closed {
        while pts.len() > 1 && pts.first() == pts.last() {
            pts.pop();
        }
    }
    loop {
        let n = pts.len();
        if n < 3 {
            return pts;
        }
        let range = if closed { 0..n } else { 1..n - 1 };
        let bad = range.into_iter().find(|&i| {
            let a = pts[(i + n - 1) % n];
            let b = pts[i];
            let c = pts[(i + 1) % n];
            let cross = (b.0 - a.0) as i128 * (c.1 - b.1) as i128 - (b.1 - a.1) as i128 * (c.0 - b.0) as i128;
            cross == 0
        });
        match bad {
            Some(i) => {
                pts.remove(i);
            }
            None => return pts,
        }
    }
}

/// Points of a closed outline as geometry (rectangles stay rectangles).
fn polygon_geometry(pts: Vec<(i64, i64)>) -> Option<Geometry> {
    let pts = cleanup(pts, true);
    if pts.len() < 3 {
        return None;
    }
    if pts.len() == 4 && is_manhattan(&pts, true) {
        let xs = pts.iter().map(|p| p.0);
        let ys = pts.iter().map(|p| p.1);
        let r = IRect::new(xs.clone().min()?, ys.clone().min()?, xs.max()?, ys.max()?);
        return Some(Geometry::Rect(r));
    }
    Some(Geometry::Polygon { points: pts })
}

/// Shape geometry after dragging handle `h` by `d`. Manhattan outlines stay Manhattan: moving a
/// vertex drags the adjacent edges along, moving an edge shifts it perpendicular to itself.
pub(super) fn drag_handle(g: &Geometry, h: Handle, d: (i64, i64)) -> Geometry {
    let Some((mut pts, closed)) = outline(g) else { return g.clone() };
    let n = pts.len();
    let manhattan = is_manhattan(&pts, closed);
    match h {
        Handle::Vertex(k) if k < n => {
            let p = pts[k];
            if manhattan {
                let neighbours = [(k + n - 1) % n, (k + 1) % n];
                for (side, j) in neighbours.into_iter().enumerate() {
                    if !closed && ((side == 0 && k == 0) || (side == 1 && k == n - 1)) {
                        continue;
                    }
                    if j == k {
                        continue;
                    }
                    if pts[j].0 == p.0 && pts[j].1 != p.1 {
                        pts[j].0 += d.0;
                    } else if pts[j].1 == p.1 && pts[j].0 != p.0 {
                        pts[j].1 += d.1;
                    }
                }
            }
            pts[k] = (p.0 + d.0, p.1 + d.1);
        }
        Handle::Edge(k) if k < edges(n, closed) => {
            let j = (k + 1) % n;
            let (a, b) = (pts[k], pts[j]);
            let d = if a.1 == b.1 {
                (0, d.1)
            } else if a.0 == b.0 {
                (d.0, 0)
            } else {
                d
            };
            for i in [k, j] {
                pts[i] = (pts[i].0 + d.0, pts[i].1 + d.1);
            }
        }
        _ => return g.clone(),
    }
    match g {
        Geometry::Rect(_) | Geometry::Polygon { .. } => polygon_geometry(pts).unwrap_or_else(|| g.clone()),
        Geometry::Path { width, .. } => {
            let pts = cleanup(pts, false);
            if pts.len() < 2 { g.clone() } else { Geometry::Path { points: pts, width: *width } }
        }
        Geometry::Label { .. } => g.clone(),
    }
}

/// Outline vertices of a shape inside `r` (partial selection by box).
pub(super) fn vertices_in(g: &Geometry, r: IRect) -> BTreeSet<usize> {
    outline(g).map_or_else(BTreeSet::new, |(pts, _)| (0..pts.len()).filter(|&i| r.contains(pts[i].0, pts[i].1)).collect())
}

/// Outline vertices of a shape (rectangles as their four corners, from the bottom left).
pub(super) fn vertices(g: &Geometry) -> Vec<(i64, i64)> {
    outline(g).map_or_else(Vec::new, |(pts, _)| pts)
}

/// The vertices of a handle: a vertex, or both ends of an edge.
pub(super) fn handle_vertices(g: &Geometry, h: Handle) -> BTreeSet<usize> {
    let n = outline(g).map_or(0, |(p, _)| p.len());
    match h {
        Handle::Vertex(k) => [k].into(),
        Handle::Edge(k) if n > 0 => [k, (k + 1) % n].into(),
        Handle::Edge(_) => BTreeSet::new(),
    }
}

/// Stretch: move the vertices `sel` of a shape by `d` (partial selection). Manhattan outlines stay
/// Manhattan: an unselected neighbour slides along with the edge it shares with a moved vertex.
pub(super) fn stretch_points(g: &Geometry, sel: &BTreeSet<usize>, d: (i64, i64)) -> Geometry {
    let Some((orig, closed)) = outline(g) else { return g.clone() };
    let n = orig.len();
    let mut pts = orig.clone();
    let manhattan = is_manhattan(&orig, closed);
    for j in 0..n {
        if sel.contains(&j) {
            pts[j] = (orig[j].0 + d.0, orig[j].1 + d.1);
            continue;
        }
        if !manhattan {
            continue;
        }
        let mut nb = vec![];
        if closed || j > 0 {
            nb.push((j + n - 1) % n);
        }
        if closed || j + 1 < n {
            nb.push((j + 1) % n);
        }
        let (mut mx, mut my) = (false, false);
        for i in nb.into_iter().filter(|i| *i != j && sel.contains(i)) {
            if orig[i].0 == orig[j].0 && orig[i].1 != orig[j].1 {
                mx = true;
            } else if orig[i].1 == orig[j].1 && orig[i].0 != orig[j].0 {
                my = true;
            }
        }
        pts[j] = (orig[j].0 + if mx { d.0 } else { 0 }, orig[j].1 + if my { d.1 } else { 0 });
    }
    match g {
        Geometry::Rect(_) | Geometry::Polygon { .. } => polygon_geometry(pts).unwrap_or_else(|| g.clone()),
        Geometry::Path { width, .. } => {
            let pts = cleanup(pts, false);
            if pts.len() < 2 { g.clone() } else { Geometry::Path { points: pts, width: *width } }
        }
        Geometry::Label { .. } => g.clone(),
    }
}

fn seg_dist(p: (i64, i64), a: (i64, i64), b: (i64, i64)) -> f64 {
    let (px, py, ax, ay, bx, by) = (p.0 as f64, p.1 as f64, a.0 as f64, a.1 as f64, b.0 as f64, b.1 as f64);
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 { (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    ((ax + t * dx - px).powi(2) + (ay + t * dy - py).powi(2)).sqrt()
}

/// The vertex within `tol` of `p`, else the nearest edge (stretch).
pub(super) fn nearest_handle(g: &Geometry, p: (i64, i64), tol: i64) -> Option<Handle> {
    let (pts, closed) = outline(g)?;
    let n = pts.len();
    let d = |q: (i64, i64)| seg_dist(p, q, q);
    let vertex = pts.iter().enumerate().map(|(i, q)| (i, d(*q))).filter(|x| x.1 <= tol as f64).min_by(|a, b| a.1.total_cmp(&b.1));
    if let Some((i, _)) = vertex {
        return Some(Handle::Vertex(i));
    }
    (0..edges(n, closed))
        .map(|i| (i, seg_dist(p, pts[i], pts[(i + 1) % n])))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| Handle::Edge(i))
}

/// Closed polygon from clicked points; a final corner is added to keep Manhattan outlines Manhattan.
pub(super) fn finish_polygon(pts: &[(i64, i64)]) -> Option<Geometry> {
    let mut pts = pts.to_vec();
    if pts.len() >= 2 && is_manhattan(&pts, false) {
        let (first, last) = (pts[0], *pts.last()?);
        if first.0 != last.0 && first.1 != last.1 {
            let second = pts[1];
            // Close with the corner that does not run back over the first edge.
            let corner = if second.1 == first.1 { (first.0, last.1) } else { (last.0, first.1) };
            pts.push(corner);
        }
    }
    polygon_geometry(pts)
}

/// Snap a polygon point: Manhattan relative to the previous point unless `free`.
pub(super) fn constrain(prev: Option<(i64, i64)>, c: (i64, i64), free: bool) -> (i64, i64) {
    match prev {
        Some(p) if !free => {
            if (c.0 - p.0).abs() >= (c.1 - p.1).abs() {
                (c.0, p.1)
            } else {
                (p.0, c.1)
            }
        }
        _ => c,
    }
}

/// Merge a set of geometries into a region, reporting how many are not Manhattan.
fn region_of<'a>(geoms: impl IntoIterator<Item = &'a Geometry>) -> (Region, usize) {
    let mut rects = vec![];
    let mut skipped = 0;
    for g in geoms {
        match Region::from_geometry(g) {
            Ok(r) => rects.extend_from_slice(r.rects()),
            Err(_) => skipped += 1,
        }
    }
    (Region::from_rects(rects), skipped)
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LayerOp {
    And,
    Or,
    Not,
    Xor,
    Size,
}

impl LayerOp {
    const ALL: [LayerOp; 5] = [LayerOp::And, LayerOp::Or, LayerOp::Not, LayerOp::Xor, LayerOp::Size];
    fn name(self) -> &'static str {
        match self {
            LayerOp::And => "AND",
            LayerOp::Or => "OR",
            LayerOp::Not => "NOT",
            LayerOp::Xor => "XOR",
            LayerOp::Size => "SIZE",
        }
    }
}

/// Persistent state of the operation panels.
pub struct EditState {
    pub size_um: f64,
    pub op: LayerOp,
    pub op_a: usize,
    pub op_b: usize,
    pub op_out: usize,
    pub op_flat: bool,
    pub via: usize,
    pub pcell: Option<PCell>,
    pub pcell_preview: Option<(PCell, Option<IRect>)>,
    pub msg: Option<(bool, String)>,
}

impl Default for EditState {
    fn default() -> Self {
        Self {
            size_um: 0.1,
            op: LayerOp::Not,
            op_a: 0,
            op_b: 1,
            op_out: 2,
            op_flat: true,
            via: 0,
            pcell: None,
            pcell_preview: None,
            msg: None,
        }
    }
}

impl LayoutEditor {
    pub(super) fn note(&mut self, ok: bool, m: impl Into<String>) {
        self.ed.msg = Some((ok, m.into()));
    }

    /// Bounding box of the selected items.
    pub(super) fn selection_bbox(&self, l: &Layout, cx: &mut LayCtx) -> Option<IRect> {
        let idx = cx.index(l);
        let mut b: Option<IRect> = None;
        let mut add = |r: IRect| b = Some(b.map_or(r, |x| x.union(&r)));
        for s in l.shapes.iter().filter(|s| self.selection.contains(&s.id)) {
            add(s.bbox());
        }
        for (i, inst) in l.instances.iter().enumerate() {
            if self.selection.contains(&inst.id)
                && let Some(r) = idx.instance_bbox(l, i)
            {
                add(r);
            }
        }
        b
    }

    fn pivot(&self, l: &Layout, cx: &mut LayCtx) -> Option<(i64, i64)> {
        let b = self.selection_bbox(l, cx)?;
        let g = cx.tech.grid.max(1);
        let snap = |v: i64| (v as f64 / g as f64).round() as i64 * g;
        Some((snap((b.x0 + b.x1) / 2), snap((b.y0 + b.y1) / 2)))
    }

    /// Rotate (0) or flip about x (1) / y (2), as seen on screen, the selection around its centre.
    pub(super) fn transform_selection(&mut self, l: &mut Layout, cx: &mut LayCtx, kind: u8) -> bool {
        let Some(p) = self.pivot(l, cx) else { return false };
        let p = self.view.orient.apply(p);
        let t = self.on_screen(match kind {
            0 => Trans::rotate_about(p),
            1 => Trans::flip_x_about(p),
            _ => Trans::flip_y_about(p),
        });
        self.history.push(l);
        l.transform_items(&self.selection, &t);
        true
    }

    fn selected_shapes_by_layer(&self, l: &Layout) -> Vec<(usize, Vec<u64>)> {
        let mut out: Vec<(usize, Vec<u64>)> = vec![];
        for s in l.shapes.iter().filter(|s| self.selection.contains(&s.id) && !matches!(s.geom, Geometry::Label { .. })) {
            match out.iter_mut().find(|(li, _)| *li == s.layer) {
                Some(e) => e.1.push(s.id),
                None => out.push((s.layer, vec![s.id])),
            }
        }
        out
    }

    /// Merge (Shift+M): union of the selected shapes, per layer.
    pub(super) fn merge_selection(&mut self, l: &mut Layout) -> bool {
        self.rewrite_selection(l, |r| r.clone())
    }

    /// Replace the selected shapes of each layer by `f(union of them)`.
    fn rewrite_selection(&mut self, l: &mut Layout, f: impl Fn(&Region) -> Region) -> bool {
        let groups = self.selected_shapes_by_layer(l);
        if groups.is_empty() {
            return false;
        }
        self.history.push(l);
        let mut skipped = 0;
        let mut sel = BTreeSet::new();
        let (mut before, mut after) = (0, 0);
        for (layer, ids) in groups {
            let geoms: Vec<Geometry> = ids.iter().filter_map(|id| l.shape(*id)).map(|s| s.geom.clone()).collect();
            let manhattan: Vec<(u64, Geometry)> =
                ids.iter().zip(geoms).filter(|(_, g)| Region::from_geometry(g).is_ok()).map(|(i, g)| (*i, g)).collect();
            skipped += ids.len() - manhattan.len();
            let (region, _) = region_of(manhattan.iter().map(|(_, g)| g));
            for (id, _) in &manhattan {
                l.remove(*id);
            }
            before += manhattan.len();
            for g in f(&region).to_geometry() {
                sel.insert(l.add(layer, g));
                after += 1;
            }
        }
        self.selection = sel;
        let mut m = format!("{before} shape(s) -> {after}");
        if skipped > 0 {
            m.push_str(&format!(" · {skipped} non-Manhattan shape(s) left unchanged"));
        }
        self.note(skipped == 0, m);
        true
    }

    pub(super) fn flatten_selection(&mut self, l: &mut Layout, src: &dyn LayoutSource) -> bool {
        if !l.instances.iter().any(|i| self.selection.contains(&i.id)) {
            return false;
        }
        self.history.push(l);
        let keep: BTreeSet<u64> = self.selection.iter().copied().filter(|id| l.shape(*id).is_some()).collect();
        let new = l.flatten_instances(&self.selection, src);
        self.selection = keep.into_iter().chain(new).collect();
        true
    }

    /// Run a layer operation on this cell, adding the result to the output layer.
    fn layer_op(&mut self, l: &mut Layout, cx: &mut LayCtx) -> bool {
        let e = &self.ed;
        let (a, b, out, op, flat) = (e.op_a, e.op_b, e.op_out, e.op, e.op_flat);
        let geoms = |layer: usize| -> Vec<Geometry> {
            if flat {
                hier::flatten(l, cx.src, 64).into_iter().filter(|f| f.layer == layer).map(|f| f.geom).collect()
            } else {
                l.shapes.iter().filter(|s| s.layer == layer).map(|s| s.geom.clone()).collect()
            }
        };
        let (ra, sa) = region_of(geoms(a).iter());
        let (rb, sb) = if op == LayerOp::Size { (Region::default(), 0) } else { region_of(geoms(b).iter()) };
        let d = (self.ed.size_um * cx.tech.dbu_per_um as f64).round() as i64;
        let res = match op {
            LayerOp::And => ra.boolean(&rb, BoolOp::And),
            LayerOp::Or => ra.boolean(&rb, BoolOp::Or),
            LayerOp::Not => ra.boolean(&rb, BoolOp::Not),
            LayerOp::Xor => ra.boolean(&rb, BoolOp::Xor),
            LayerOp::Size => ra.sized(d),
        };
        let geoms = res.to_geometry();
        let names = |i: usize| cx.tech.layer(i).map_or("?", |d| d.name.as_str());
        let mut m = match op {
            LayerOp::Size => format!("SIZE {} by {} µm -> {}: {} shape(s)", names(a), um(d, cx.tech), names(out), geoms.len()),
            _ => format!("{} {} {} -> {}: {} shape(s)", names(a), op.name(), names(b), names(out), geoms.len()),
        };
        if sa + sb > 0 {
            m.push_str(&format!(" · {} non-Manhattan shape(s) ignored", sa + sb));
        }
        if geoms.is_empty() {
            self.note(sa + sb == 0, m);
            return false;
        }
        self.history.push(l);
        self.selection = geoms.into_iter().map(|g| l.add(out, g)).collect();
        self.note(sa + sb == 0, m);
        true
    }

    /// Via at a point (single cut) or filling a dragged area.
    pub(super) fn place_via(&mut self, l: &mut Layout, tech: &Tech, at: (i64, i64), area: Option<IRect>) -> bool {
        let Some(def) = tech.vias.get(self.ed.via) else {
            self.note(false, "The technology defines no vias");
            return false;
        };
        let res = match area {
            Some(a) => pcell::via_array(tech, def, a),
            None => pcell::via_at(tech, def, at),
        };
        match res {
            Ok(items) => {
                self.history.push(l);
                self.selection = items.into_iter().map(|(layer, g)| l.add(layer, g)).collect();
                true
            }
            Err(e) => {
                self.note(false, e);
                false
            }
        }
    }

    /// Extent of the PCell being placed (generated once per parameter set).
    pub(super) fn pcell_bbox(&mut self, tech: &Tech) -> Option<IRect> {
        let p = self.ed.pcell.clone()?;
        if self.ed.pcell_preview.as_ref().map(|x| &x.0) != Some(&p) {
            let b = p.generate(tech).ok().and_then(|l| l.shapes_bbox());
            self.ed.pcell_preview = Some((p, b));
        }
        self.ed.pcell_preview.as_ref().and_then(|x| x.1)
    }

    /// Selection operations shown in the inspector. Returns true if the layout changed.
    pub(super) fn ops_panel(&mut self, ui: &mut egui::Ui, l: &mut Layout, cx: &mut LayCtx) -> bool {
        let mut changed = false;
        let has_shapes = l.shapes.iter().any(|s| self.selection.contains(&s.id));
        let has_insts = l.instances.iter().any(|i| self.selection.contains(&i.id));
        ui.add_space(8.0);
        theme::section(ui, "Edit");
        ui.horizontal_wrapped(|ui| {
            if ui.button("Rotate 90°").on_hover_text("Counter-clockwise about the selection centre (Shift+O)").clicked() {
                changed |= self.transform_selection(l, cx, 0);
            }
            if ui.button("Flip X").on_hover_text("Mirror left-right").clicked() {
                changed |= self.transform_selection(l, cx, 1);
            }
            if ui.button("Flip Y").on_hover_text("Mirror top-bottom").clicked() {
                changed |= self.transform_selection(l, cx, 2);
            }
        });
        ui.horizontal_wrapped(|ui| {
            if has_insts && ui.button("Flatten").on_hover_text("Replace instances by their contents, one level").clicked() {
                changed |= self.flatten_selection(l, cx.src);
            }
            if ui.button("Make cell").on_hover_text("Move the selection into a new cell and place it here").clicked()
                && let Some(b) = self.selection_bbox(l, cx)
            {
                let g = cx.tech.grid.max(1);
                let origin = (b.x0.div_euclid(g) * g, b.y0.div_euclid(g) * g);
                self.requests.push(LayRequest::MakeCell { ids: self.selection.clone(), origin });
            }
        });
        if has_shapes {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Merge").on_hover_text("Union of the selected shapes, per layer (Shift+M)").clicked() {
                    changed |= self.merge_selection(l);
                }
                let lb = ui.label("Size");
                ui.add(egui::DragValue::new(&mut self.ed.size_um).speed(0.01).max_decimals(3).suffix(" µm")).labelled_by(lb.id);
                if ui.button("Apply").on_hover_text("Grow (positive) or shrink (negative) the selected shapes").clicked() {
                    let d = (self.ed.size_um * cx.tech.dbu_per_um as f64).round() as i64;
                    changed |= self.rewrite_selection(l, |r| r.sized(d));
                }
            });
        }
        changed |= self.align_panel(ui, l, cx);
        self.message(ui);
        changed
    }

    fn message(&mut self, ui: &mut egui::Ui) {
        let pal = theme::palette(ui.ctx());
        if let Some((ok, m)) = &self.ed.msg {
            ui.label(egui::RichText::new(m).small().color(if *ok { pal.text_weak } else { pal.warning }));
        }
    }

    /// Layer Boolean / sizing operations on the whole cell.
    pub(super) fn layer_ops_panel(&mut self, ui: &mut egui::Ui, l: &mut Layout, cx: &mut LayCtx) -> bool {
        let tech = cx.tech;
        let mut changed = false;
        ui.add_space(12.0);
        theme::section(ui, "Layer operations");
        let layer_combo = |ui: &mut egui::Ui, id: &str, label: &str, v: &mut usize| {
            let lb = ui.label(label);
            egui::ComboBox::from_id_salt(id)
                .selected_text(tech.layer(*v).map_or("?", |d| d.name.as_str()))
                .show_ui(ui, |ui| {
                    for (i, d) in tech.layers.iter().enumerate() {
                        ui.selectable_value(v, i, &d.name);
                    }
                })
                .response
                .labelled_by(lb.id);
            ui.end_row();
        };
        egui::Grid::new("layer_ops").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
            layer_combo(ui, "op_a", "A", &mut self.ed.op_a);
            let lb = ui.label("Operation");
            egui::ComboBox::from_id_salt("op_kind")
                .selected_text(self.ed.op.name())
                .show_ui(ui, |ui| {
                    for op in LayerOp::ALL {
                        ui.selectable_value(&mut self.ed.op, op, op.name());
                    }
                })
                .response
                .labelled_by(lb.id);
            ui.end_row();
            if self.ed.op == LayerOp::Size {
                let lb = ui.label("By");
                ui.add(egui::DragValue::new(&mut self.ed.size_um).speed(0.01).max_decimals(3).suffix(" µm")).labelled_by(lb.id);
                ui.end_row();
            } else {
                layer_combo(ui, "op_b", "B", &mut self.ed.op_b);
            }
            layer_combo(ui, "op_out", "Output", &mut self.ed.op_out);
        });
        ui.checkbox(&mut self.ed.op_flat, "Include hierarchy (flattened)");
        if ui.button("Run").clicked() {
            changed |= self.layer_op(l, cx);
        }
        if self.selection.is_empty() {
            self.message(ui);
        }
        changed
    }

    /// Via picker for the via tool.
    pub(super) fn via_panel(&mut self, ui: &mut egui::Ui, tech: &Tech) {
        let pal = theme::palette(ui.ctx());
        theme::section(ui, "Via");
        if tech.vias.is_empty() {
            ui.label(egui::RichText::new("The technology defines no vias").small().color(pal.warning));
            return;
        }
        self.ed.via = self.ed.via.min(tech.vias.len() - 1);
        for (i, v) in tech.vias.iter().enumerate() {
            ui.selectable_value(&mut self.ed.via, i, format!("{}  ({} / {} / {})", v.name, v.bottom, v.cut, v.top));
        }
        ui.label(egui::RichText::new("Click for a single cut, drag to fill an area").small().color(pal.text_weak));
        self.message(ui);
        ui.add_space(4.0);
    }

    /// PCell picker and parameters for the instance tool.
    pub(super) fn pcell_panel(&mut self, ui: &mut egui::Ui, tech: &Tech) {
        let pal = theme::palette(ui.ctx());
        theme::section(ui, "PCell");
        ui.horizontal_wrapped(|ui| {
            for d in PCell::defaults() {
                let on = self.ed.pcell.as_ref().map(PCell::kind_name) == Some(d.kind_name());
                if ui.selectable_label(on, d.kind_name()).clicked() {
                    self.ed.pcell = if on { None } else { Some(d) };
                    self.place = None;
                }
            }
        });
        if let Some(p) = self.ed.pcell.as_mut() {
            pcell_params(ui, p, tech, "place");
            match p.generate(tech) {
                Ok(_) => {
                    ui.label(egui::RichText::new(format!("Click to place {}", p.variant_name())).small().color(pal.text_weak));
                }
                Err(e) => {
                    ui.label(egui::RichText::new(e).small().color(pal.warning));
                }
            }
        }
        ui.add_space(4.0);
    }
}

/// Parameter editors for a PCell. Returns true if a value changed.
pub(super) fn pcell_params(ui: &mut egui::Ui, p: &mut PCell, tech: &Tech, salt: &str) -> bool {
    let k = tech.dbu_per_um as f64;
    let before = p.clone();
    let len = |ui: &mut egui::Ui, label: &str, v: &mut i64| {
        let lb = ui.label(label);
        let mut f = *v as f64 / k;
        ui.add(egui::DragValue::new(&mut f).speed(0.01).range(0.0..=1000.0).max_decimals(3).suffix(" µm")).labelled_by(lb.id);
        let nv = ((f * k / tech.grid as f64).round() as i64) * tech.grid;
        if nv != *v {
            *v = nv;
        }
        ui.end_row();
    };
    egui::Grid::new(("pcell_params", salt)).num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| match p {
        PCell::Mos { pmos, w, l, fingers, device } => {
            ui.label("Type");
            ui.label(if *pmos { "PMOS" } else { "NMOS" });
            ui.end_row();
            let kind = if *pmos { ics_core::tech::MosType::Pmos } else { ics_core::tech::MosType::Nmos };
            let options: Vec<&str> = tech.devices.iter().filter(|d| d.kind == kind).map(|d| d.name.as_str()).collect();
            if !options.is_empty() {
                let lb = ui.label("Device");
                let shown = if device.is_empty() { format!("default ({})", options[0]) } else { device.clone() };
                egui::ComboBox::from_id_salt(("pcell_device", salt))
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(device, String::new(), format!("default ({})", options[0]));
                        for o in options.iter().skip(1) {
                            ui.selectable_value(device, o.to_string(), *o);
                        }
                    })
                    .response
                    .labelled_by(lb.id);
                ui.end_row();
            }
            len(ui, "W", w);
            len(ui, "L", l);
            let lb = ui.label("Fingers");
            ui.add(egui::DragValue::new(fingers).range(1..=64)).labelled_by(lb.id);
            ui.end_row();
        }
        PCell::PolyRes { w, l } => {
            len(ui, "W", w);
            len(ui, "L", l);
        }
        PCell::MomCap { w, h } => {
            len(ui, "W", w);
            len(ui, "H", h);
        }
        PCell::Tap { w, .. } => len(ui, "H", w),
    });
    *p != before
}

/// Outline preview of the polygon being drawn.
pub(super) fn draw_polygon_preview(painter: &egui::Painter, pts: &[Pos2], color: egui::Color32) {
    if pts.len() >= 2 {
        painter.add(egui::Shape::line(pts.to_vec(), Stroke::new(1.5, color)));
        painter.add(egui::Shape::dashed_line(
            &[*pts.last().unwrap(), pts[0]],
            Stroke::new(1.0, color.gamma_multiply(0.6)),
            4.0,
            4.0,
        ));
    }
    for p in pts {
        painter.circle_filled(*p, 2.5, color);
    }
}

/// Shape with its geometry replaced, keeping id and layer.
pub(super) fn with_geom(s: &Shape, g: Geometry) -> Shape {
    Shape { id: s.id, layer: s.layer, geom: g }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manhattan_vertex_drag_keeps_edges_axis_aligned() {
        let g = Geometry::Polygon { points: vec![(0, 0), (100, 0), (100, 50), (50, 50), (50, 100), (0, 100)] };
        let Geometry::Polygon { points } = drag_handle(&g, Handle::Vertex(3), (10, 20)) else { panic!() };
        assert!(is_manhattan(&points, true));
        assert!(points.contains(&(60, 70)));
    }

    #[test]
    fn rect_edge_drag_resizes() {
        let g = Geometry::Rect(IRect::new(0, 0, 100, 50));
        assert_eq!(drag_handle(&g, Handle::Edge(1), (30, 999)), Geometry::Rect(IRect::new(0, 0, 130, 50)));
        assert_eq!(drag_handle(&g, Handle::Vertex(2), (10, 10)), Geometry::Rect(IRect::new(0, 0, 110, 60)));
    }

    #[test]
    fn partial_stretch_moves_selected_vertices_only() {
        let g = Geometry::Rect(IRect::new(0, 0, 100, 50));
        let right = vertices_in(&g, IRect::new(90, -10, 200, 60));
        assert_eq!(right, [1, 2].into());
        assert_eq!(stretch_points(&g, &right, (30, 0)), Geometry::Rect(IRect::new(0, 0, 130, 50)));
        assert_eq!(stretch_points(&g, &[2].into(), (10, 10)), Geometry::Rect(IRect::new(0, 0, 110, 60)));
        let l = Geometry::Polygon { points: vec![(0, 0), (100, 0), (100, 50), (50, 50), (50, 100), (0, 100)] };
        let Geometry::Polygon { points } = stretch_points(&l, &[3, 4].into(), (20, 0)) else { panic!() };
        assert!(is_manhattan(&points, true));
        assert!(points.contains(&(70, 50)) && points.contains(&(70, 100)) && points.contains(&(0, 100)));
        let p = Geometry::Path { points: vec![(0, 0), (100, 0), (100, 100)], width: 10 };
        assert_eq!(
            stretch_points(&p, &[2].into(), (0, 50)),
            Geometry::Path { points: vec![(0, 0), (100, 0), (100, 150)], width: 10 }
        );
    }

    #[test]
    fn polygon_tool_closes_manhattan() {
        let g = finish_polygon(&[(0, 0), (100, 0), (100, 50), (50, 50), (50, 100)]).unwrap();
        let Geometry::Polygon { points } = g else { panic!("expected polygon") };
        assert_eq!(points.len(), 6);
        assert!(is_manhattan(&points, true));
        assert_eq!(finish_polygon(&[(0, 0), (100, 0), (100, 50)]), Some(Geometry::Rect(IRect::new(0, 0, 100, 50))));
    }
}
