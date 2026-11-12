//! Layout workspace: hierarchical full-custom editor with layer palette, cell placement and DRC.

use std::collections::BTreeSet;

use eframe::egui::{self, Align2, Color32, FontId, Key, Mesh, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use ics_core::geom::IRect;
use ics_core::hier::{self, CellIndex, Hier, LayoutSource, ViewQuery};
use ics_core::layout::{ArraySpec, Geometry, Layout, Shape, Trans};
use ics_core::project::{CellRef, Project};
use ics_core::tech::{FillStyle, LayerDef, Tech};

use crate::canvas::{View2D, nice_step};
use crate::history::History;
use crate::theme::{self, Palette};

mod edit;
mod verify;
pub use edit::LayRequest;
use edit::{EditState, Handle};
pub use verify::Verify;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LayTool {
    Select,
    Rect,
    Polygon,
    Path,
    Label,
    Via,
    Instance,
    Ruler,
}

impl LayTool {
    pub const ALL: [LayTool; 8] = [
        LayTool::Select,
        LayTool::Rect,
        LayTool::Polygon,
        LayTool::Path,
        LayTool::Label,
        LayTool::Via,
        LayTool::Instance,
        LayTool::Ruler,
    ];

    pub fn name(self) -> &'static str {
        match self {
            LayTool::Select => "Select",
            LayTool::Rect => "Rectangle",
            LayTool::Polygon => "Polygon",
            LayTool::Path => "Path",
            LayTool::Label => "Label",
            LayTool::Via => "Via",
            LayTool::Instance => "Instance",
            LayTool::Ruler => "Ruler",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            LayTool::Select => "S",
            LayTool::Rect => "R",
            LayTool::Polygon => "G",
            LayTool::Path => "P",
            LayTool::Label => "L",
            LayTool::Via => "V",
            LayTool::Instance => "I",
            LayTool::Ruler => "K",
        }
    }
}

enum LDrag {
    Move { start: (i64, i64), offset: (i64, i64) },
    Rubber { start: Pos2 },
    Create { start: (i64, i64) },
    Ruler { start: (i64, i64) },
    Handle { id: u64, handle: Handle, start: (i64, i64), orig: Geometry, cur: Geometry },
}

/// Everything the layout editor needs besides the layout being edited.
pub struct LayCtx<'a> {
    pub cell: &'a CellRef,
    /// Resolves instances. The edited cell itself is not reachable through it.
    pub src: &'a Project,
    pub hier: &'a mut Hier,
    pub tech: &'a Tech,
    /// Cells with a layout view, candidates for placement.
    pub cells: &'a [CellRef],
}

impl LayCtx<'_> {
    fn index(&mut self, l: &Layout) -> std::sync::Arc<CellIndex> {
        self.hier.get_for(self.cell, l, self.src)
    }

    fn touched(&mut self) {
        self.hier.invalidate(self.cell);
    }
}

#[derive(Default, Clone, Copy)]
pub struct DrawStats {
    pub drawn: usize,
    pub truncated: bool,
}

pub struct LayoutEditor {
    pub view: View2D,
    needs_fit: bool,
    pub tool: LayTool,
    pub active_layer: usize,
    pub visible: Vec<bool>,
    pub selection: BTreeSet<u64>,
    pub history: History<Layout>,
    drag: Option<LDrag>,
    path_pts: Vec<(i64, i64)>,
    pub path_width: i64,
    pub label_text: String,
    ruler: Option<((i64, i64), (i64, i64))>,
    pub verify: verify::VerifyState,
    pub cursor: Option<(i64, i64)>,
    pub show_grid: bool,
    /// Deepest hierarchy level drawn with its contents.
    pub max_depth: u32,
    pub place: Option<CellRef>,
    pub place_rot: u8,
    pub place_mirror: bool,
    pub place_cols: u32,
    pub place_rows: u32,
    pub stats: DrawStats,
    poly_pts: Vec<(i64, i64)>,
    hot: Option<Handle>,
    ed: EditState,
    inst_pcell: Option<(u64, ics_core::pcell::PCell)>,
    /// Draw unrouted schematic connections.
    pub show_flylines: bool,
    flylines: Option<Vec<ics_core::sdl::Flyline>>,
    /// Edits for the application to carry out (they touch other cells).
    pub requests: Vec<LayRequest>,
}

impl Default for LayoutEditor {
    fn default() -> Self {
        Self {
            view: View2D::new(0.08, true, 1e-7, 50.0),
            needs_fit: true,
            tool: LayTool::Select,
            active_layer: 6,
            visible: vec![],
            selection: BTreeSet::new(),
            history: History::default(),
            drag: None,
            path_pts: vec![],
            path_width: 0,
            label_text: "net".into(),
            ruler: None,
            verify: verify::VerifyState::default(),
            cursor: None,
            show_grid: true,
            max_depth: 32,
            place: None,
            place_rot: 0,
            place_mirror: false,
            place_cols: 1,
            place_rows: 1,
            stats: DrawStats::default(),
            poly_pts: vec![],
            hot: None,
            ed: EditState::default(),
            inst_pcell: None,
            show_flylines: false,
            flylines: None,
            requests: vec![],
        }
    }
}

fn layer_color(def: &LayerDef) -> Color32 {
    Color32::from_rgb(def.color[0], def.color[1], def.color[2])
}

pub fn um(dbu: i64, tech: &Tech) -> String {
    let v = dbu as f64 / tech.dbu_per_um as f64;
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn fill_alpha(def: &LayerDef) -> f32 {
    let a = def.color[3] as f32 / 255.0;
    let base = match def.fill {
        FillStyle::Solid => 0.55,
        FillStyle::Outline => 0.0,
        _ => 0.18,
    };
    base * a
}

/// Fill a screen-space rect with the layer's stipple pattern (screen anchored so adjacent shapes line up).
fn pattern_rect(painter: &egui::Painter, r: Rect, def: &LayerDef, emphasis: f32) {
    let c = layer_color(def);
    let a = def.color[3] as f32 / 255.0;
    let base_alpha = fill_alpha(def);
    if base_alpha > 0.0 {
        painter.rect_filled(r, 0.0, c.gamma_multiply(base_alpha * emphasis));
    }
    if r.width() < 3.0 || r.height() < 3.0 {
        painter.rect_filled(r, 0.0, c.gamma_multiply(0.6 * emphasis));
        return;
    }
    let clip = painter.with_clip_rect(r.intersect(painter.clip_rect()));
    let stroke = Stroke::new(1.0, c.gamma_multiply((0.35 + 0.5 * a) * emphasis));
    let sp = 8.0;
    let diag = |forward: bool| {
        // lines x - y = k*sp  (forward "/" uses x + y)
        let (lo, hi) = if forward { (r.min.x + r.min.y, r.max.x + r.max.y) } else { (r.min.x - r.max.y, r.max.x - r.min.y) };
        let mut k = (lo / sp).floor() * sp;
        let mut n = 0;
        while k <= hi && n < 4000 {
            if forward {
                clip.line_segment([Pos2::new(k - r.max.y, r.max.y), Pos2::new(k - r.min.y, r.min.y)], stroke);
            } else {
                clip.line_segment([Pos2::new(k + r.min.y, r.min.y), Pos2::new(k + r.max.y, r.max.y)], stroke);
            }
            k += sp;
            n += 1;
        }
    };
    match def.fill {
        FillStyle::Hatch => diag(true),
        FillStyle::BackHatch => diag(false),
        FillStyle::Cross => {
            clip.line_segment([r.left_top(), r.right_bottom()], stroke);
            clip.line_segment([r.left_bottom(), r.right_top()], stroke);
        }
        FillStyle::Dots => {
            let step = 6.0;
            let mut y = (r.min.y / step).ceil() * step;
            let mut n = 0;
            while y < r.max.y && n < 20_000 {
                let off = if ((y / step) as i64) % 2 == 0 { 0.0 } else { step / 2.0 };
                let mut x = ((r.min.x - off) / step).ceil() * step + off;
                while x < r.max.x && n < 20_000 {
                    clip.circle_filled(Pos2::new(x, y), 0.8, stroke.color);
                    x += step;
                    n += 1;
                }
                y += step;
            }
        }
        FillStyle::Solid | FillStyle::Outline => {}
    }
    painter.rect_stroke(r, 0.0, Stroke::new(1.0, c.gamma_multiply(emphasis)), StrokeKind::Inside);
}

fn at_least_px(r: Rect, px: f32) -> Rect {
    Rect::from_center_size(r.center(), Vec2::new(r.width().max(px), r.height().max(px)))
}

/// Outline as four thin quads in a mesh (much cheaper than individual stroke shapes).
fn mesh_outline(mesh: &mut Mesh, r: Rect, w: f32, c: Color32) {
    if r.width() <= 2.0 * w || r.height() <= 2.0 * w {
        mesh.add_colored_rect(r, c);
        return;
    }
    mesh.add_colored_rect(Rect::from_min_max(r.min, Pos2::new(r.max.x, r.min.y + w)), c);
    mesh.add_colored_rect(Rect::from_min_max(Pos2::new(r.min.x, r.max.y - w), r.max), c);
    mesh.add_colored_rect(Rect::from_min_max(Pos2::new(r.min.x, r.min.y + w), Pos2::new(r.min.x + w, r.max.y - w)), c);
    mesh.add_colored_rect(Rect::from_min_max(Pos2::new(r.max.x - w, r.min.y + w), Pos2::new(r.max.x, r.max.y - w)), c);
}

fn path_quads(points: &[(i64, i64)], width: i64) -> Vec<[(f64, f64); 4]> {
    let h = width as f64 / 2.0;
    points
        .windows(2)
        .filter_map(|w| {
            let (ax, ay) = (w[0].0 as f64, w[0].1 as f64);
            let (bx, by) = (w[1].0 as f64, w[1].1 as f64);
            let (dx, dy) = (bx - ax, by - ay);
            let len = (dx * dx + dy * dy).sqrt();
            if len == 0.0 {
                return None;
            }
            let (ux, uy) = (dx / len * h, dy / len * h);
            let (nx, ny) = (-uy, ux);
            // extended ends (GDS pathtype 2)
            let (ax, ay, bx, by) = (ax - ux, ay - uy, bx + ux, by + uy);
            Some([(ax + nx, ay + ny), (bx + nx, by + ny), (bx - nx, by - ny), (ax - nx, ay - ny)])
        })
        .collect()
}

pub struct LayoutDrawOpts<'a> {
    pub pal: Palette,
    pub visible: &'a [bool],
    pub selection: Option<&'a BTreeSet<u64>>,
    pub thumbnail: bool,
    pub show_grid: bool,
    pub grid: i64,
    pub max_depth: u32,
}

fn screen_rect(view: &View2D, rect: Rect, b: IRect) -> Rect {
    Rect::from_two_pos(view.to_screen(rect, b.x0 as f64, b.y0 as f64), view.to_screen(rect, b.x1 as f64, b.y1 as f64))
}

fn world_view(view: &View2D, rect: Rect) -> IRect {
    let (x0, y1) = view.to_world(rect, rect.min);
    let (x1, y0) = view.to_world(rect, rect.max);
    IRect::new(x0.floor() as i64, y0.floor() as i64, x1.ceil() as i64, y1.ceil() as i64)
}

fn draw_grid(painter: &egui::Painter, view: &View2D, rect: Rect, pal: &Palette, grid: i64) {
    let step = nice_step(view.scale, 18.0, grid as f64);
    let (x0, y1) = view.to_world(rect, rect.min);
    let (x1, y0) = view.to_world(rect, rect.max);
    let line =
        |major: bool| Stroke::new(1.0, if major { pal.grid_major.gamma_multiply(0.7) } else { pal.grid.gamma_multiply(0.6) });
    let mut x = (x0 / step).floor() * step;
    let mut n = 0;
    while x <= x1 && n < 400 {
        let p = view.to_screen(rect, x, 0.0);
        painter
            .line_segment([Pos2::new(p.x, rect.min.y), Pos2::new(p.x, rect.max.y)], line(((x / step).round() as i64) % 5 == 0));
        x += step;
        n += 1;
    }
    let mut y = (y0 / step).floor() * step;
    n = 0;
    while y <= y1 && n < 400 {
        let p = view.to_screen(rect, 0.0, y);
        painter
            .line_segment([Pos2::new(rect.min.x, p.y), Pos2::new(rect.max.x, p.y)], line(((y / step).round() as i64) % 5 == 0));
        y += step;
        n += 1;
    }
    let o0 = view.to_screen(rect, 0.0, 0.0);
    let axis = Stroke::new(1.0, pal.text_weak.gamma_multiply(0.35));
    painter.line_segment([Pos2::new(o0.x, rect.min.y), Pos2::new(o0.x, rect.max.y)], axis);
    painter.line_segment([Pos2::new(rect.min.x, o0.y), Pos2::new(rect.max.x, o0.y)], axis);
}

/// Draw a cell with its hierarchy. Shapes are culled through the per-cell spatial indices, tiny
/// shapes and cells collapse into boxes, and the work per frame is bounded.
#[allow(clippy::too_many_arguments)]
pub fn draw_layout(
    painter: &egui::Painter,
    view: &View2D,
    rect: Rect,
    cell: &CellRef,
    layout: &Layout,
    src: &dyn LayoutSource,
    hier: &mut Hier,
    tech: &Tech,
    o: &LayoutDrawOpts,
) -> DrawStats {
    let pal = o.pal;
    if o.show_grid && !o.thumbnail {
        draw_grid(painter, view, rect, &pal, o.grid);
    }
    let idx = hier.get_for(cell, layout, src);
    let world = world_view(view, rect);
    let px = 1.0 / view.scale;
    let q = ViewQuery {
        view: world,
        max_depth: o.max_depth,
        min_cell: ((if o.thumbnail { 3.0 } else { 8.0 }) * px) as i64,
        min_shape: (1.5 * px) as i64,
        visible: o.visible,
        limit: if o.thumbnail { 50_000 } else { 400_000 },
    };
    let vis = hier::query(hier, src, layout, &idx, &q);
    let selected = |id: u64| o.selection.is_some_and(|s| s.contains(&id));
    let has_sel = o.selection.is_some_and(|s| !s.is_empty());
    let to_s = |x: f64, y: f64| view.to_screen(rect, x, y);
    let stipple = !o.thumbnail && vis.count < 20_000;
    for li in 0..tech.layers.len() {
        if !o.visible.get(li).copied().unwrap_or(true) {
            continue;
        }
        let def = &tech.layers[li];
        let c = layer_color(def);
        let fa = fill_alpha(def);
        let mesh_slot = painter.add(egui::Shape::Noop);
        let mut mesh = Mesh::default();
        for b in vis.tiny.get(li).into_iter().flatten() {
            mesh.add_colored_rect(at_least_px(screen_rect(view, rect, *b), 1.0), c.gamma_multiply(0.7));
        }
        for p in vis.shapes.get(li).into_iter().flatten() {
            let emphasis = if has_sel && !(p.depth == 0 && selected(p.shape.id)) { 0.75 } else { 1.0 };
            match &p.shape.geom {
                Geometry::Rect(r) => {
                    let sr = screen_rect(view, rect, p.trans.apply_rect(*r));
                    if !sr.intersects(rect) {
                        continue;
                    }
                    if sr.width() < 3.0 || sr.height() < 3.0 {
                        mesh.add_colored_rect(at_least_px(sr, 1.0), c.gamma_multiply(0.6 * emphasis));
                    } else if stipple {
                        pattern_rect(painter, sr, def, emphasis);
                    } else {
                        let clipped = sr.intersect(rect.expand(2.0));
                        if fa > 0.0 {
                            mesh.add_colored_rect(clipped, c.gamma_multiply(fa.max(0.25) * emphasis));
                        }
                        mesh_outline(&mut mesh, clipped, 1.0, c.gamma_multiply(emphasis));
                    }
                }
                Geometry::Polygon { points } => {
                    let pts: Vec<Pos2> = points
                        .iter()
                        .map(|q| {
                            let (x, y) = p.trans.apply_f((q.0 as f64, q.1 as f64));
                            to_s(x, y)
                        })
                        .collect();
                    painter.add(egui::Shape::closed_line(pts, Stroke::new(1.2, c.gamma_multiply(emphasis))));
                }
                Geometry::Path { points, width } => {
                    for quad in path_quads(points, *width) {
                        let pts: Vec<Pos2> = quad
                            .iter()
                            .map(|q| {
                                let (x, y) = p.trans.apply_f(*q);
                                to_s(x, y)
                            })
                            .collect();
                        painter.add(egui::Shape::convex_polygon(
                            pts,
                            c.gamma_multiply(0.35 * emphasis),
                            Stroke::new(1.0, c.gamma_multiply(emphasis)),
                        ));
                    }
                    if !o.thumbnail && p.depth == 0 {
                        let center: Vec<Pos2> = points.iter().map(|q| to_s(q.0 as f64, q.1 as f64)).collect();
                        painter.add(egui::Shape::dashed_line(&center, Stroke::new(1.0, c.gamma_multiply(0.6)), 4.0, 4.0));
                    }
                }
                Geometry::Label { text, at, size } => {
                    let fs = (*size as f64 * view.scale) as f32;
                    if o.thumbnail || (p.depth > 0 && fs < 6.0) {
                        continue;
                    }
                    let (x, y) = p.trans.apply_f((at.0 as f64, at.1 as f64));
                    let sp = to_s(x, y);
                    painter.text(
                        sp,
                        Align2::LEFT_BOTTOM,
                        text,
                        FontId::proportional(fs.clamp(8.0, 64.0)),
                        c.gamma_multiply(emphasis).to_opaque(),
                    );
                    painter.line_segment([sp - Vec2::new(3.0, 0.0), sp + Vec2::new(3.0, 0.0)], Stroke::new(1.0, c));
                    painter.line_segment([sp - Vec2::new(0.0, 3.0), sp + Vec2::new(0.0, 3.0)], Stroke::new(1.0, c));
                }
            }
        }
        painter.set(mesh_slot, egui::Shape::mesh(mesh));
    }

    // Instances shown as boxes (beyond the level limit, too small, or missing).
    let mut mesh = Mesh::default();
    let mut names = 0;
    for b in &vis.boxes {
        let sr = screen_rect(view, rect, b.rect);
        let col = if b.missing { pal.danger } else { pal.text_weak.gamma_multiply(0.8) };
        mesh_outline(&mut mesh, sr.intersect(rect.expand(2.0)), 1.0, col);
        if !o.thumbnail && sr.width() > 70.0 && sr.height() > 18.0 && names < 400 {
            names += 1;
            let label = if b.missing { format!("{} (missing)", b.cell) } else { b.cell.cell.clone() };
            painter.text(sr.center(), Align2::CENTER_CENTER, label, FontId::proportional(11.0), col);
        }
    }
    painter.add(egui::Shape::mesh(mesh));

    // Frames of the top cell's own instances.
    if !o.thumbnail {
        for i in idx.instances_in(world) {
            let (Some(b), Some(inst)) = (idx.instance_bbox(layout, i), layout.instances.get(i)) else { continue };
            let sr = screen_rect(view, rect, b);
            let sel = selected(inst.id);
            let st = if sel { Stroke::new(2.0, pal.selection) } else { Stroke::new(1.0, pal.text_weak.gamma_multiply(0.9)) };
            painter.rect_stroke(sr, 0.0, st, if sel { StrokeKind::Outside } else { StrokeKind::Inside });
            if sr.width() > 50.0 && sr.height() > 16.0 {
                let (c, r) = inst.dims();
                let name = if c * r > 1 { format!("{} [{c}×{r}]", inst.cell.cell) } else { inst.cell.cell.clone() };
                let at = Pos2::new(sr.min.x.max(rect.min.x) + 4.0, sr.min.y.max(rect.min.y) + 3.0);
                painter.text(
                    at,
                    Align2::LEFT_TOP,
                    name,
                    FontId::proportional(11.0),
                    if sel { pal.selection } else { pal.text_weak },
                );
            }
        }
    }

    // Selected top-level shapes.
    if has_sel {
        for s in layout.shapes.iter().filter(|s| selected(s.id)) {
            let sr = screen_rect(view, rect, s.bbox());
            painter.rect_stroke(sr.expand(1.5), 0.0, Stroke::new(2.0, pal.selection), StrokeKind::Outside);
        }
    }
    DrawStats { drawn: vis.count, truncated: vis.truncated }
}

/// Topmost item (shape or instance) of the top cell under the cursor.
fn hit(layout: &Layout, idx: &CellIndex, visible: &[bool], x: i64, y: i64, tol: i64) -> Option<u64> {
    let area = IRect::new(x - tol, y - tol, x + tol, y + tol);
    let mut best: Option<(i128, usize, u64)> = None;
    let mut consider = |b: IRect, layer: usize, id: u64| {
        let a = b.width().max(1) as i128 * b.height().max(1) as i128;
        // Prefer smaller items (more specific), then higher layers.
        if best.is_none_or(|(ba, bl, _)| a < ba || (a == ba && layer > bl)) {
            best = Some((a, layer, id));
        }
    };
    for (li, vis) in visible.iter().enumerate() {
        if !vis {
            continue;
        }
        for i in idx.shapes_in(li, area) {
            if let Some(s) = layout.shapes.get(i)
                && s.hit(x, y, tol)
            {
                consider(s.bbox(), s.layer, s.id);
            }
        }
    }
    for i in idx.instances_in(area) {
        if let (Some(b), Some(inst)) = (idx.instance_bbox(layout, i), layout.instances.get(i))
            && b.contains(x, y)
        {
            consider(b, 0, inst.id);
        }
    }
    best.map(|b| b.2)
}

#[derive(Default)]
pub struct LayOutput {
    pub changed: bool,
}

impl LayoutEditor {
    pub fn sync_layers(&mut self, tech: &Tech) {
        if self.visible.len() != tech.layers.len() {
            self.visible = vec![true; tech.layers.len()];
        }
        if self.active_layer >= tech.layers.len() {
            self.active_layer = 0;
        }
    }

    /// Drop results derived from the layout (verification, flylines).
    pub fn layout_changed(&mut self) {
        self.verify.layout_changed();
        self.flylines = None;
    }

    pub fn request_fit(&mut self) {
        self.needs_fit = true;
    }

    pub fn set_tool(&mut self, t: LayTool) {
        self.tool = t;
        self.path_pts.clear();
        self.poly_pts.clear();
        if t != LayTool::Ruler {
            self.ruler = None;
        }
    }

    pub fn undo(&mut self, l: &mut Layout) -> bool {
        self.selection.clear();
        self.layout_changed();
        self.history.undo(l)
    }

    pub fn redo(&mut self, l: &mut Layout) -> bool {
        self.selection.clear();
        self.layout_changed();
        self.history.redo(l)
    }

    pub fn delete_selection(&mut self, l: &mut Layout) -> bool {
        if self.selection.is_empty() {
            return false;
        }
        self.history.push(l);
        for id in std::mem::take(&mut self.selection) {
            l.remove(id);
        }
        true
    }

    fn duplicate(&mut self, l: &mut Layout, grid: i64) -> bool {
        if self.selection.is_empty() {
            return false;
        }
        self.history.push(l);
        let off = grid * 50;
        let mut sel = BTreeSet::new();
        for id in self.selection.clone() {
            if let Some(s) = l.shape(id).cloned() {
                let mut g = Shape { id: 0, ..s };
                g.translate(off, -off);
                sel.insert(l.add(g.layer, g.geom));
            } else if let Some(i) = l.instance(id).cloned() {
                let t = Trans { x: i.trans.x + off, y: i.trans.y - off, ..i.trans };
                sel.insert(l.add_instance(i.cell, t, i.array));
            }
        }
        self.selection = sel;
        true
    }

    fn nudge(&mut self, l: &mut Layout, dx: i64, dy: i64) -> bool {
        if self.selection.is_empty() {
            return false;
        }
        self.history.push(l);
        for id in &self.selection {
            l.translate_item(*id, dx, dy);
        }
        true
    }

    fn snap(&self, w: (f64, f64), grid: i64) -> (i64, i64) {
        let g = grid.max(1) as f64;
        (((w.0 / g).round() * g) as i64, ((w.1 / g).round() * g) as i64)
    }

    fn finish_path(&mut self, l: &mut Layout, tech: &Tech) -> bool {
        let pts = std::mem::take(&mut self.path_pts);
        if pts.len() < 2 {
            return false;
        }
        self.history.push(l);
        let w = if self.path_width > 0 { self.path_width } else { tech.layers[self.active_layer].min_width.max(tech.grid * 10) };
        let id = l.add(self.active_layer, Geometry::Path { points: pts, width: w });
        self.selection = [id].into();
        true
    }

    /// Transform and array used when placing the chosen cell at `at`.
    fn placement(&self, cx: &mut LayCtx, at: (i64, i64)) -> Option<(CellRef, Trans, Option<ArraySpec>, IRect)> {
        let cell = self.place.clone()?;
        let cb = cx.hier.get(&cell, cx.src)?.bbox?;
        let (t, array) = self.place_trans(at, cb);
        Some((cell, t, array, cb))
    }

    fn place_trans(&self, at: (i64, i64), cb: IRect) -> (Trans, Option<ArraySpec>) {
        let t = Trans::with(at.0, at.1, self.place_rot, self.place_mirror);
        let lin = Trans::with(0, 0, self.place_rot, self.place_mirror).apply_rect(cb);
        let array = (self.place_cols * self.place_rows > 1).then(|| ArraySpec {
            cols: self.place_cols.max(1),
            rows: self.place_rows.max(1),
            col: (lin.width(), 0),
            row: (0, lin.height()),
        });
        (t, array)
    }

    fn finish_polygon(&mut self, l: &mut Layout) -> bool {
        let pts = std::mem::take(&mut self.poly_pts);
        let Some(g) = edit::finish_polygon(&pts) else { return false };
        self.history.push(l);
        let id = l.add(self.active_layer, g);
        self.selection = [id].into();
        true
    }

    fn handle_keys(&mut self, ui: &egui::Ui, l: &mut Layout, cx: &mut LayCtx) -> bool {
        let tech = cx.tech;
        if ui.ctx().egui_wants_keyboard_input() {
            return false;
        }
        let mut changed = false;
        let (keys, cmd, shift) = ui.input(|i| {
            let k: Vec<Key> = i
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key { key, pressed: true, .. } => Some(*key),
                    _ => None,
                })
                .collect();
            (k, i.modifiers.command, i.modifiers.shift)
        });
        let g = tech.grid * if shift { 100 } else { 10 };
        for k in keys {
            if cmd {
                match k {
                    Key::A => self.selection = l.ids().collect(),
                    Key::D => changed |= self.duplicate(l, tech.grid),
                    _ => {}
                }
                continue;
            }
            if shift && matches!(k, Key::R | Key::X | Key::Y) {
                let kind = match k {
                    Key::R => 0,
                    Key::X => 1,
                    _ => 2,
                };
                changed |= self.transform_selection(l, cx, kind);
                continue;
            }
            match k {
                Key::Escape => {
                    if !self.path_pts.is_empty() || !self.poly_pts.is_empty() {
                        self.path_pts.clear();
                        self.poly_pts.clear();
                    } else if self.tool != LayTool::Select {
                        self.set_tool(LayTool::Select);
                    } else {
                        self.selection.clear();
                    }
                }
                Key::Enter => changed |= self.finish_path(l, tech) || self.finish_polygon(l),
                Key::Delete | Key::Backspace => {
                    if !self.path_pts.is_empty() {
                        self.path_pts.pop();
                    } else if !self.poly_pts.is_empty() {
                        self.poly_pts.pop();
                    } else {
                        changed |= self.delete_selection(l);
                    }
                }
                Key::R => self.set_tool(LayTool::Rect),
                Key::G => self.set_tool(LayTool::Polygon),
                Key::P => self.set_tool(LayTool::Path),
                Key::L => self.set_tool(LayTool::Label),
                Key::V => self.set_tool(LayTool::Via),
                Key::I => self.set_tool(LayTool::Instance),
                Key::K => self.set_tool(LayTool::Ruler),
                Key::S => self.set_tool(LayTool::Select),
                Key::F => self.needs_fit = true,
                Key::ArrowLeft => changed |= self.nudge(l, -g, 0),
                Key::ArrowRight => changed |= self.nudge(l, g, 0),
                Key::ArrowUp => changed |= self.nudge(l, 0, g),
                Key::ArrowDown => changed |= self.nudge(l, 0, -g),
                Key::Num0 | Key::Num1 | Key::Num2 | Key::Num3 | Key::Num4 | Key::Num5 | Key::Num6 | Key::Num7 | Key::Num8 => {
                    self.max_depth = k.name().parse().unwrap_or(32);
                }
                Key::Num9 => self.max_depth = 32,
                _ => {}
            }
        }
        changed
    }

    pub fn canvas(&mut self, ui: &mut egui::Ui, l: &mut Layout, cx: &mut LayCtx) -> LayOutput {
        let tech = cx.tech;
        self.sync_layers(tech);
        let pal = theme::palette(ui.ctx());
        let mut out = LayOutput::default();
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = resp.rect;
        let painter = painter.with_clip_rect(rect);
        painter.rect_filled(rect, 0.0, if pal.dark { Color32::from_rgb(0x0e, 0x10, 0x13) } else { pal.bg_canvas });
        if self.needs_fit {
            match cx.index(l).bbox {
                Some(b) => self.view.fit(rect, (b.x0 as f64, b.y0 as f64), (b.x1 as f64, b.y1 as f64), 50.0),
                None => {
                    self.view.scale = 0.08;
                    self.view.center = (2500.0, 2500.0);
                }
            }
            self.needs_fit = false;
        }
        self.view.handle_navigation(ui, &resp, rect);
        if resp.hovered() && self.handle_keys(ui, l, cx) {
            out.changed = true;
            cx.touched();
        }
        let hover = resp.hover_pos();
        let world = hover.map(|p| self.view.to_world(rect, p));
        self.cursor = world.map(|w| self.snap(w, tech.grid));
        let tol = (5.0 / self.view.scale) as i64;
        let shift = ui.input(|i| i.modifiers.shift || i.modifiers.command);
        // The single selected shape exposes vertex / edge handles.
        let handle_shape: Option<Shape> = match (self.tool, self.selection.len()) {
            (LayTool::Select, 1) => l.shape(*self.selection.first().unwrap()).cloned(),
            _ => None,
        };
        self.hot = match (&handle_shape, hover, &self.drag) {
            (Some(s), Some(h), None) => edit::hit_handle(&s.geom, &self.view, rect, h),
            (_, _, Some(LDrag::Handle { handle, .. })) => Some(*handle),
            _ => None,
        };

        match self.tool {
            LayTool::Select => {
                if resp.drag_started_by(egui::PointerButton::Primary) {
                    let press = ui.input(|i| i.pointer.press_origin()).unwrap_or(rect.center());
                    let pw = self.view.to_world(rect, press);
                    let idx = cx.index(l);
                    let on_handle =
                        handle_shape.as_ref().and_then(|s| edit::hit_handle(&s.geom, &self.view, rect, press).map(|h| (s, h)));
                    if let Some((s, h)) = on_handle {
                        self.drag = Some(LDrag::Handle {
                            id: s.id,
                            handle: h,
                            start: self.snap(pw, tech.grid),
                            orig: s.geom.clone(),
                            cur: s.geom.clone(),
                        });
                    } else {
                        match hit(l, &idx, &self.visible, pw.0 as i64, pw.1 as i64, tol) {
                            Some(id) => {
                                if !self.selection.contains(&id) {
                                    if !shift {
                                        self.selection.clear();
                                    }
                                    self.selection.insert(id);
                                }
                                self.drag = Some(LDrag::Move { start: self.snap(pw, tech.grid), offset: (0, 0) });
                            }
                            None => self.drag = Some(LDrag::Rubber { start: press }),
                        }
                    }
                }
                if let Some(w) = world
                    && (resp.clicked() || resp.double_clicked())
                    && !self.pick_net((w.0 as i64, w.1 as i64))
                {
                    let idx = cx.index(l);
                    let h = hit(l, &idx, &self.visible, w.0 as i64, w.1 as i64, tol);
                    if resp.double_clicked() {
                        if let Some(i) = h.and_then(|id| l.instance(id)) {
                            self.requests.push(LayRequest::Open(i.cell.clone()));
                        }
                    } else {
                        match h {
                            Some(id) => {
                                if shift {
                                    if !self.selection.remove(&id) {
                                        self.selection.insert(id);
                                    }
                                } else {
                                    self.selection = [id].into();
                                }
                            }
                            None if !shift => self.selection.clear(),
                            None => {}
                        }
                    }
                }
            }
            LayTool::Rect | LayTool::Via => {
                if resp.drag_started_by(egui::PointerButton::Primary) {
                    let press = ui.input(|i| i.pointer.press_origin()).unwrap_or(rect.center());
                    self.drag = Some(LDrag::Create { start: self.snap(self.view.to_world(rect, press), tech.grid) });
                }
                if self.tool == LayTool::Via
                    && resp.clicked()
                    && let Some(c) = self.cursor
                    && self.place_via(l, tech, c, None)
                {
                    out.changed = true;
                    cx.touched();
                }
            }
            LayTool::Polygon => {
                if let Some(c) = self.cursor {
                    if resp.clicked() {
                        let p = edit::constrain(self.poly_pts.last().copied(), c, shift);
                        if self.poly_pts.last() != Some(&p) {
                            self.poly_pts.push(p);
                        }
                    }
                    if (resp.double_clicked() || resp.secondary_clicked()) && self.finish_polygon(l) {
                        out.changed = true;
                        cx.touched();
                    }
                }
            }
            LayTool::Ruler => {
                if resp.drag_started_by(egui::PointerButton::Primary) {
                    let press = ui.input(|i| i.pointer.press_origin()).unwrap_or(rect.center());
                    let s = self.snap(self.view.to_world(rect, press), tech.grid);
                    self.drag = Some(LDrag::Ruler { start: s });
                    self.ruler = Some((s, s));
                }
            }
            LayTool::Path => {
                if let Some(c) = self.cursor {
                    if resp.clicked() {
                        let p = match self.path_pts.last() {
                            Some(&last) => manhattan(last, c),
                            None => c,
                        };
                        if self.path_pts.last() != Some(&p) {
                            self.path_pts.push(p);
                        }
                    }
                    if resp.double_clicked() && self.finish_path(l, tech) {
                        out.changed = true;
                        cx.touched();
                    }
                }
                if resp.secondary_clicked() && self.finish_path(l, tech) {
                    out.changed = true;
                    cx.touched();
                }
            }
            LayTool::Label => {
                if resp.clicked()
                    && let Some(c) = self.cursor
                {
                    let t = self.label_text.trim().to_string();
                    if !t.is_empty() {
                        self.history.push(l);
                        let id = l.add(self.active_layer, Geometry::Label { text: t, at: c, size: 250 });
                        self.selection = [id].into();
                        out.changed = true;
                        cx.touched();
                    }
                }
            }
            LayTool::Instance => {
                if resp.clicked()
                    && let Some(c) = self.cursor
                    && self.ed.pcell.is_some()
                {
                    if let Some(cb) = self.pcell_bbox(tech) {
                        let (trans, array) = self.place_trans(c, cb);
                        let pcell = self.ed.pcell.clone().unwrap();
                        self.requests.push(LayRequest::PlacePCell { pcell, trans, array });
                    }
                } else if resp.clicked()
                    && let Some(c) = self.cursor
                    && let Some((cell, t, array, _)) = self.placement(cx, c)
                {
                    self.history.push(l);
                    let id = l.add_instance(cell, t, array);
                    self.selection = [id].into();
                    out.changed = true;
                    cx.touched();
                }
            }
        }

        // Drag updates
        if let Some(c) = self.cursor {
            match &mut self.drag {
                Some(LDrag::Move { start, offset }) => *offset = (c.0 - start.0, c.1 - start.1),
                Some(LDrag::Ruler { start }) => self.ruler = Some((*start, c)),
                Some(LDrag::Handle { handle, start, orig, cur, .. }) => {
                    *cur = edit::drag_handle(orig, *handle, (c.0 - start.0, c.1 - start.1));
                }
                _ => {}
            }
        }
        if resp.drag_stopped() {
            match self.drag.take() {
                Some(LDrag::Handle { id, orig, cur, .. }) => {
                    if cur != orig
                        && let Some(s) = l.shape(id).cloned()
                    {
                        self.history.push(l);
                        if let Some(t) = l.shape_mut(id) {
                            *t = edit::with_geom(&s, cur);
                        }
                        out.changed = true;
                        cx.touched();
                    }
                }
                Some(LDrag::Create { start }) if self.tool == LayTool::Via => {
                    if let Some(c) = self.cursor
                        && c.0 != start.0
                        && c.1 != start.1
                        && self.place_via(l, tech, c, Some(IRect::new(start.0, start.1, c.0, c.1)))
                    {
                        out.changed = true;
                        cx.touched();
                    }
                }
                Some(LDrag::Move { offset, .. }) => {
                    if offset != (0, 0) {
                        self.history.push(l);
                        for id in &self.selection {
                            l.translate_item(*id, offset.0, offset.1);
                        }
                        out.changed = true;
                        cx.touched();
                    }
                }
                Some(LDrag::Create { start }) => {
                    if let Some(c) = self.cursor
                        && c.0 != start.0
                        && c.1 != start.1
                    {
                        self.history.push(l);
                        let id = l.add_rect(self.active_layer, start.0, start.1, c.0, c.1);
                        self.selection = [id].into();
                        out.changed = true;
                        cx.touched();
                    }
                }
                Some(LDrag::Rubber { start }) => {
                    if let Some(end) = hover {
                        let a = self.view.to_world(rect, start);
                        let b = self.view.to_world(rect, end);
                        let r = IRect::new(a.0 as i64, a.1 as i64, b.0 as i64, b.1 as i64);
                        if !shift {
                            self.selection.clear();
                        }
                        let inside = |bb: IRect| bb.x0 >= r.x0 && bb.x1 <= r.x1 && bb.y0 >= r.y0 && bb.y1 <= r.y1;
                        let idx = cx.index(l);
                        for (li, vis) in self.visible.iter().enumerate() {
                            if *vis {
                                for i in idx.shapes_in(li, r) {
                                    if let Some(s) = l.shapes.get(i)
                                        && inside(s.bbox())
                                    {
                                        self.selection.insert(s.id);
                                    }
                                }
                            }
                        }
                        for i in idx.instances_in(r) {
                            if let (Some(b), Some(inst)) = (idx.instance_bbox(l, i), l.instances.get(i))
                                && inside(b)
                            {
                                self.selection.insert(inst.id);
                            }
                        }
                    }
                }
                Some(LDrag::Ruler { .. }) | None => {}
            }
        }

        self.stats = draw_layout(
            &painter,
            &self.view,
            rect,
            cx.cell,
            l,
            cx.src,
            cx.hier,
            tech,
            &LayoutDrawOpts {
                pal,
                visible: &self.visible,
                selection: Some(&self.selection),
                thumbnail: false,
                show_grid: self.show_grid,
                grid: tech.grid,
                max_depth: self.max_depth,
            },
        );

        if out.changed {
            self.layout_changed();
        }
        self.draw_verify(&painter, &self.view, rect, &pal);
        if self.show_flylines {
            if self.flylines.is_none()
                && let Some(sch) = cx.src.cell(cx.cell).and_then(|c| c.schematic.as_ref())
            {
                self.flylines = Some(ics_core::sdl::ratsnest(sch, l, cx.src, tech));
            }
            let st = Stroke::new(1.2, pal.warning.gamma_multiply(0.9));
            for f in self.flylines.iter().flatten() {
                let a = self.view.to_screen(rect, f.a.0 as f64, f.a.1 as f64);
                let b = self.view.to_screen(rect, f.b.0 as f64, f.b.1 as f64);
                painter.add(egui::Shape::dashed_line(&[a, b], st, 5.0, 3.0));
                painter.circle_filled(a, 2.5, st.color);
                painter.circle_filled(b, 2.5, st.color);
                if a.distance(b) > 60.0 {
                    painter.text(a + (b - a) * 0.5, Align2::CENTER_BOTTOM, &f.net, FontId::proportional(11.0), st.color);
                }
            }
        }
        // Move preview: ghost outlines of the selection at the drag offset.
        if let Some(LDrag::Move { offset, .. }) = &self.drag
            && *offset != (0, 0)
        {
            let idx = cx.index(l);
            let ghost = Stroke::new(1.5, pal.selection.gamma_multiply(0.9));
            for (i, inst) in l.instances.iter().enumerate() {
                if self.selection.contains(&inst.id)
                    && let Some(b) = idx.instance_bbox(l, i)
                {
                    painter.rect_stroke(
                        screen_rect(&self.view, rect, b.translate(offset.0, offset.1)),
                        0.0,
                        ghost,
                        StrokeKind::Middle,
                    );
                }
            }
            for s in l.shapes.iter().filter(|s| self.selection.contains(&s.id)) {
                painter.rect_stroke(
                    screen_rect(&self.view, rect, s.bbox().translate(offset.0, offset.1)),
                    0.0,
                    ghost,
                    StrokeKind::Middle,
                );
            }
        }
        // Vertex / edge handles and the shape being reshaped.
        if let Some(LDrag::Handle { cur, .. }) = &self.drag {
            let ghost = ics_core::layout::Shape { id: 0, layer: 0, geom: cur.clone() };
            let b = screen_rect(&self.view, rect, ghost.bbox());
            match cur {
                Geometry::Polygon { points } => {
                    let pts: Vec<Pos2> = points.iter().map(|p| self.view.to_screen(rect, p.0 as f64, p.1 as f64)).collect();
                    painter.add(egui::Shape::closed_line(pts, Stroke::new(1.5, pal.warning)));
                }
                Geometry::Path { points, width } => {
                    for q in path_quads(points, *width) {
                        let sp: Vec<Pos2> = q.iter().map(|p| self.view.to_screen(rect, p.0, p.1)).collect();
                        painter.add(egui::Shape::closed_line(sp, Stroke::new(1.5, pal.warning)));
                    }
                }
                _ => {
                    painter.rect_stroke(b, 0.0, Stroke::new(1.5, pal.warning), StrokeKind::Middle);
                }
            }
            edit::draw_handles(&painter, cur, &self.view, rect, &pal, self.hot);
        } else if let Some(s) = &handle_shape {
            edit::draw_handles(&painter, &s.geom, &self.view, rect, &pal, self.hot);
        }
        if !self.poly_pts.is_empty() {
            let mut pts: Vec<Pos2> = self.poly_pts.iter().map(|p| self.view.to_screen(rect, p.0 as f64, p.1 as f64)).collect();
            if let Some(c) = self.cursor {
                let p = edit::constrain(self.poly_pts.last().copied(), c, shift);
                pts.push(self.view.to_screen(rect, p.0 as f64, p.1 as f64));
            }
            edit::draw_polygon_preview(&painter, &pts, layer_color(&tech.layers[self.active_layer]));
        }
        // Tool previews
        let def = &tech.layers[self.active_layer];
        if let (Some(LDrag::Create { start }), Some(c)) = (&self.drag, self.cursor) {
            let r = Rect::from_two_pos(
                self.view.to_screen(rect, start.0 as f64, start.1 as f64),
                self.view.to_screen(rect, c.0 as f64, c.1 as f64),
            );
            pattern_rect(&painter, r, def, 1.0);
            let txt = format!("{} × {} µm", um((c.0 - start.0).abs(), tech), um((c.1 - start.1).abs(), tech));
            painter.text(r.right_bottom() + Vec2::new(6.0, 6.0), Align2::LEFT_TOP, txt, FontId::monospace(11.0), pal.text);
        }
        if self.tool == LayTool::Instance
            && let Some(c) = self.cursor
            && hover.is_some()
        {
            let pending = match self.ed.pcell.clone() {
                Some(p) => self.pcell_bbox(tech).map(|cb| {
                    let (t, array) = self.place_trans(c, cb);
                    (CellRef::new(ics_core::pcell::PCELL_LIB, p.variant_name()), t, array, cb)
                }),
                None => self.placement(cx, c),
            };
            match pending {
                Some((cell, t, array, cb)) => {
                    let inst = ics_core::layout::CellInst { id: 0, cell, trans: t, array, name: None };
                    let b = screen_rect(&self.view, rect, inst.bbox(cb));
                    painter.rect_filled(b, 0.0, pal.layout.gamma_multiply(0.08));
                    painter.rect_stroke(b, 0.0, Stroke::new(1.5, pal.layout), StrokeKind::Middle);
                    let e = screen_rect(&self.view, rect, t.apply_rect(cb));
                    painter.rect_stroke(e, 0.0, Stroke::new(1.0, pal.layout.gamma_multiply(0.7)), StrokeKind::Middle);
                    painter.text(
                        e.left_top() + Vec2::new(4.0, 3.0),
                        Align2::LEFT_TOP,
                        &inst.cell.cell,
                        FontId::proportional(11.0),
                        pal.layout,
                    );
                }
                None => {
                    if let Some(h) = hover {
                        painter.text(
                            h + Vec2::new(14.0, 14.0),
                            Align2::LEFT_TOP,
                            "Choose a cell to place in the left panel",
                            FontId::proportional(12.0),
                            pal.text_weak,
                        );
                    }
                }
            }
        }
        if let Some(start) = self.drag.as_ref().and_then(|d| if let LDrag::Rubber { start } = d { Some(*start) } else { None })
            && let Some(h) = hover
        {
            let r = Rect::from_two_pos(start, h);
            painter.rect_filled(r, 0.0, pal.selection.gamma_multiply(0.08));
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, pal.selection), StrokeKind::Inside);
        }
        if !self.path_pts.is_empty() {
            let mut pts = self.path_pts.clone();
            if let Some(c) = self.cursor {
                pts.push(manhattan(*pts.last().unwrap(), c));
            }
            let w = if self.path_width > 0 { self.path_width } else { def.min_width.max(tech.grid * 10) };
            for q in path_quads(&pts, w) {
                let sp: Vec<Pos2> = q.iter().map(|p| self.view.to_screen(rect, p.0, p.1)).collect();
                painter.add(egui::Shape::convex_polygon(
                    sp,
                    layer_color(def).gamma_multiply(0.3),
                    Stroke::new(1.0, layer_color(def)),
                ));
            }
        }
        if let Some((a, b)) = self.ruler {
            let pa = self.view.to_screen(rect, a.0 as f64, a.1 as f64);
            let pb = self.view.to_screen(rect, b.0 as f64, b.1 as f64);
            let st = Stroke::new(1.5, pal.warning);
            painter.line_segment([pa, pb], st);
            for p in [pa, pb] {
                painter.circle_stroke(p, 3.0, st);
            }
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let d = ((dx * dx + dy * dy) as f64).sqrt() as i64;
            let txt = format!("{} µm  (dx {} , dy {})", um(d, tech), um(dx, tech), um(dy, tech));
            let mid = pa + (pb - pa) * 0.5;
            let g = painter.layout_no_wrap(txt, FontId::monospace(12.0), pal.warning);
            let r = Rect::from_min_size(mid + Vec2::new(8.0, -24.0), g.size() + Vec2::new(10.0, 4.0));
            painter.rect_filled(r, 4.0, pal.bg_panel.gamma_multiply(0.92));
            painter.galley(r.min + Vec2::new(5.0, 2.0), g, pal.warning);
        }
        if l.is_empty() {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "Empty layout — choose a layer on the left, then press R and drag to draw a rectangle.\nP draws paths, L places labels, I places cells, K measures, scroll to zoom, right-drag to pan.",
                FontId::proportional(14.0),
                pal.text_weak,
            );
        }
        // Scale bar
        let bar_um = nice_step(self.view.scale, 80.0, 1.0) as i64;
        let bar_px = (bar_um as f64 * self.view.scale) as f32;
        let base = rect.left_bottom() + Vec2::new(16.0, -16.0);
        painter.line_segment([base, base + Vec2::new(bar_px, 0.0)], Stroke::new(2.0, pal.text_weak));
        painter.text(
            base + Vec2::new(bar_px / 2.0, -4.0),
            Align2::CENTER_BOTTOM,
            format!("{} µm", um(bar_um, tech)),
            FontId::monospace(11.0),
            pal.text_weak,
        );

        if resp.hovered() {
            ui.ctx().set_cursor_icon(if self.tool == LayTool::Select {
                egui::CursorIcon::Default
            } else {
                egui::CursorIcon::Crosshair
            });
        }
        out
    }

    /// Left panel: tool options, cell placement, hierarchy tree and layers. Returns a cell to open.
    pub fn layer_palette(&mut self, ui: &mut egui::Ui, l: &Layout, cx: &mut LayCtx) -> Option<CellRef> {
        let tech = cx.tech;
        self.sync_layers(tech);
        let pal = theme::palette(ui.ctx());
        let mut open = None;
        if self.tool == LayTool::Path {
            theme::section(ui, "Path");
            ui.horizontal(|ui| {
                let l = ui.label("Width (nm)");
                ui.add(egui::DragValue::new(&mut self.path_width).range(0..=100_000).speed(10)).labelled_by(l.id);
            });
            ui.label(
                egui::RichText::new("0 = layer minimum width · Enter / double-click to finish").small().color(pal.text_weak),
            );
        }
        if self.tool == LayTool::Label {
            theme::section(ui, "Label");
            ui.horizontal(|ui| {
                let l = ui.label("Text");
                ui.text_edit_singleline(&mut self.label_text).labelled_by(l.id);
            });
        }
        if self.tool == LayTool::Polygon {
            theme::section(ui, "Polygon");
            ui.label(
                egui::RichText::new("Click vertices (Manhattan; hold Shift for any angle) · Enter / double-click to close")
                    .small()
                    .color(pal.text_weak),
            );
        }
        if self.tool == LayTool::Via {
            self.via_panel(ui, tech);
        }
        if self.tool == LayTool::Instance {
            self.pcell_panel(ui, tech);
            self.placement_panel(ui, cx);
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            theme::section(ui, "Hierarchy");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let dv = ui
                    .add(egui::DragValue::new(&mut self.max_depth).range(0..=32))
                    .on_hover_text("Hierarchy levels drawn with their contents (keys 0-9)");
                let lv = ui.label(egui::RichText::new("levels").small().color(pal.text_weak));
                dv.labelled_by(lv.id);
            });
        });
        egui::ScrollArea::vertical().id_salt("hier_tree").max_height(180.0).show(ui, |ui| {
            hier_node(ui, cx.src, cx.cell, Some(l), None, 0, &mut open);
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            theme::section(ui, &format!("Layers · {}", tech.name));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("none").clicked() {
                    self.visible.iter_mut().for_each(|v| *v = false);
                }
                if ui.small_button("all").clicked() {
                    self.visible.iter_mut().for_each(|v| *v = true);
                }
            });
        });
        egui::ScrollArea::vertical().id_salt("layers").show(ui, |ui| {
            for (i, def) in tech.layers.iter().enumerate() {
                let active = i == self.active_layer;
                let row_h = 24.0;
                let (r, painter) = ui.allocate_painter(Vec2::new(ui.available_width(), row_h), Sense::click());
                if active {
                    painter.rect_filled(r.rect, 4.0, pal.accent.gamma_multiply(0.22));
                } else if r.hovered() {
                    painter.rect_filled(r.rect, 4.0, pal.bg_hover);
                }
                let sw = Rect::from_min_size(r.rect.min + Vec2::new(6.0, 4.0), Vec2::new(26.0, 16.0));
                pattern_rect(&painter, sw, def, if self.visible[i] { 1.0 } else { 0.3 });
                let tc = if self.visible[i] { pal.text } else { pal.text_weak };
                painter.text(
                    sw.right_center() + Vec2::new(8.0, 0.0),
                    Align2::LEFT_CENTER,
                    &def.name,
                    FontId::proportional(13.0),
                    tc,
                );
                painter.text(
                    r.rect.right_center() - Vec2::new(30.0, 0.0),
                    Align2::RIGHT_CENTER,
                    format!("{}/{}", def.gds_layer, def.gds_datatype),
                    FontId::monospace(10.5),
                    pal.text_weak,
                );
                let eye = Rect::from_center_size(r.rect.right_center() - Vec2::new(14.0, 0.0), Vec2::splat(18.0));
                let eye_resp = ui.interact(eye, ui.scope_id().with(("eye", i)), Sense::click());
                painter.text(
                    eye.center(),
                    Align2::CENTER_CENTER,
                    if self.visible[i] { "👁" } else { "–" },
                    FontId::proportional(12.0),
                    if eye_resp.hovered() { pal.text } else { pal.text_weak },
                );
                if eye_resp.clicked() {
                    self.visible[i] = !self.visible[i];
                } else if r.clicked() {
                    self.active_layer = i;
                    self.visible[i] = true;
                }
                r.on_hover_text(format!(
                    "{}  GDS {}/{}\nmin width {} nm · min spacing {} nm",
                    def.name, def.gds_layer, def.gds_datatype, def.min_width, def.min_spacing
                ));
            }
        });
        open
    }

    fn placement_panel(&mut self, ui: &mut egui::Ui, cx: &mut LayCtx) {
        let pal = theme::palette(ui.ctx());
        theme::section(ui, "Place cell");
        let candidates: Vec<&CellRef> =
            cx.cells.iter().filter(|c| *c != cx.cell && !hier::would_cycle(cx.src, cx.cell, c)).collect();
        if candidates.is_empty() {
            ui.label(egui::RichText::new("No other cells with a layout").small().color(pal.text_weak));
        }
        egui::ScrollArea::vertical().id_salt("place_cells").max_height(140.0).show(ui, |ui| {
            for c in candidates {
                let on = self.place.as_ref() == Some(c);
                if ui.selectable_label(on, c.to_string()).clicked() {
                    self.place = Some(c.clone());
                    self.ed.pcell = None;
                }
            }
        });
        egui::Grid::new("place_opts").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
            let l = ui.label("Rotation");
            egui::ComboBox::from_id_salt("place_rot")
                .selected_text(format!("{}°", self.place_rot as u32 * 90))
                .show_ui(ui, |ui| {
                    for r in 0..4u8 {
                        ui.selectable_value(&mut self.place_rot, r, format!("{}°", r as u32 * 90));
                    }
                })
                .response
                .labelled_by(l.id);
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut self.place_mirror, "Mirror about x");
            ui.end_row();
            let l = ui.label("Columns");
            ui.add(egui::DragValue::new(&mut self.place_cols).range(1..=10_000)).labelled_by(l.id);
            ui.end_row();
            let l = ui.label("Rows");
            ui.add(egui::DragValue::new(&mut self.place_rows).range(1..=10_000)).labelled_by(l.id);
            ui.end_row();
        });
        ui.label(egui::RichText::new("Click to place · arrays abut at the cell extent").small().color(pal.text_weak));
        ui.add_space(4.0);
    }

    /// Right panel. Returns (changed, cell to open).
    pub fn inspector(&mut self, ui: &mut egui::Ui, l: &mut Layout, cx: &mut LayCtx) -> (bool, Option<CellRef>) {
        let tech = cx.tech;
        let pal = theme::palette(ui.ctx());
        let mut changed = false;
        let mut open = None;
        let k = tech.dbu_per_um as f64;
        let um_edit = |ui: &mut egui::Ui, label: &str, v: &mut i64, focus: &mut bool| {
            let l = ui.label(label);
            let mut f = *v as f64 / k;
            let r = ui.add(egui::DragValue::new(&mut f).speed(0.01).max_decimals(3).suffix(" µm")).labelled_by(l.id);
            *focus |= r.drag_started() || r.gained_focus();
            let nv = ((f * k / tech.grid as f64).round() as i64) * tech.grid;
            if nv != *v {
                *v = nv;
            }
            ui.end_row();
        };
        let single = (self.selection.len() == 1).then(|| *self.selection.iter().next().unwrap());
        if let Some(id) = single
            && let Some(before) = l.instance(id).cloned()
        {
            let mut s = before.clone();
            let mut focus = false;
            theme::section(ui, "Instance");
            egui::Grid::new("inst_props").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label("Cell");
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(s.cell.to_string()).strong());
                    if ui.small_button("Open").on_hover_text("Open this cell (double-click the instance)").clicked() {
                        open = Some(s.cell.clone());
                    }
                });
                ui.end_row();
                um_edit(ui, "x", &mut s.trans.x, &mut focus);
                um_edit(ui, "y", &mut s.trans.y, &mut focus);
                let ll = ui.label("Rotation");
                egui::ComboBox::from_id_salt("inst_rot")
                    .selected_text(format!("{}°", s.trans.rot as u32 * 90))
                    .show_ui(ui, |ui| {
                        for r in 0..4u8 {
                            focus |= ui.selectable_value(&mut s.trans.rot, r, format!("{}°", r as u32 * 90)).clicked();
                        }
                    })
                    .response
                    .labelled_by(ll.id);
                ui.end_row();
                ui.label("");
                focus |= ui.checkbox(&mut s.trans.mirror, "Mirror about x").clicked();
                ui.end_row();
                ui.label("");
                let mut is_array = s.array.is_some();
                if ui.checkbox(&mut is_array, "Array").clicked() {
                    focus = true;
                    let cb = cx.hier.get(&s.cell, cx.src).and_then(|c| c.bbox).unwrap_or_default();
                    let lin = Trans { x: 0, y: 0, ..s.trans }.apply_rect(cb);
                    s.array = is_array.then_some(ArraySpec { cols: 2, rows: 1, col: (lin.width(), 0), row: (0, lin.height()) });
                }
                ui.end_row();
                if let Some(a) = s.array.as_mut() {
                    let l1 = ui.label("Columns");
                    let r = ui.add(egui::DragValue::new(&mut a.cols).range(1..=100_000)).labelled_by(l1.id);
                    focus |= r.drag_started() || r.gained_focus();
                    ui.end_row();
                    let l2 = ui.label("Rows");
                    let r = ui.add(egui::DragValue::new(&mut a.rows).range(1..=100_000)).labelled_by(l2.id);
                    focus |= r.drag_started() || r.gained_focus();
                    ui.end_row();
                    um_edit(ui, "Column pitch", &mut a.col.0, &mut focus);
                    um_edit(ui, "Row pitch", &mut a.row.1, &mut focus);
                }
                if let Some(ci) = cx.hier.get(&s.cell, cx.src) {
                    if let Some(b) = ci.bbox {
                        ui.label("Cell size");
                        ui.label(format!("{} × {} µm", um(b.width(), tech), um(b.height(), tech)));
                        ui.end_row();
                    }
                    ui.label("Flat shapes");
                    ui.label(fmt_count(ci.flat_shapes * s.count()));
                    ui.end_row();
                } else {
                    ui.label("Status");
                    ui.colored_label(pal.danger, "cell not found");
                    ui.end_row();
                }
            });
            if focus {
                self.history.push(l);
            }
            let pcell = cx.src.cell(&s.cell).and_then(|c| c.pcell.clone());
            if s != before {
                if let Some(t) = l.instance_mut(id) {
                    *t = s;
                }
                changed = true;
            }
            if let Some(p) = pcell {
                if self.inst_pcell.as_ref().is_none_or(|(i, _)| *i != id) {
                    self.inst_pcell = Some((id, p.clone()));
                }
                ui.add_space(6.0);
                theme::section(ui, &format!("PCell · {}", p.kind_name()));
                if let Some((_, edited)) = self.inst_pcell.as_mut() {
                    edit::pcell_params(ui, edited, tech, "inst");
                    let valid = edited.generate(tech);
                    if let Err(e) = &valid {
                        ui.label(egui::RichText::new(e).small().color(pal.warning));
                    }
                    if ui.add_enabled(*edited != p && valid.is_ok(), egui::Button::new("Apply")).clicked() {
                        self.requests.push(LayRequest::SetPCell { inst: id, pcell: edited.clone() });
                    }
                }
            }
            ui.add_space(8.0);
            if ui.button("🗑 Delete").clicked() {
                changed |= self.delete_selection(l);
            }
        } else if let Some(id) = single
            && let Some(before) = l.shape(id).cloned()
        {
            let mut s = before.clone();
            theme::section(ui, "Shape");
            let mut focus = false;
            egui::Grid::new("shape_props").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                let ll = ui.label("Layer");
                egui::ComboBox::from_id_salt("shape_layer")
                    .selected_text(&tech.layers[s.layer].name)
                    .show_ui(ui, |ui| {
                        for (i, d) in tech.layers.iter().enumerate() {
                            if ui.selectable_value(&mut s.layer, i, &d.name).clicked() {
                                focus = true;
                            }
                        }
                    })
                    .response
                    .labelled_by(ll.id);
                ui.end_row();
                match &mut s.geom {
                    Geometry::Rect(r) => {
                        um_edit(ui, "x0", &mut r.x0, &mut focus);
                        um_edit(ui, "y0", &mut r.y0, &mut focus);
                        um_edit(ui, "x1", &mut r.x1, &mut focus);
                        um_edit(ui, "y1", &mut r.y1, &mut focus);
                        ui.label("Size");
                        ui.label(format!("{} × {} µm", um(r.width(), tech), um(r.height(), tech)));
                        ui.end_row();
                    }
                    Geometry::Path { points, width } => {
                        um_edit(ui, "Width", width, &mut focus);
                        ui.label("Points");
                        ui.label(points.len().to_string());
                        ui.end_row();
                    }
                    Geometry::Label { text, size, .. } => {
                        let l = ui.label("Text");
                        let r = ui.text_edit_singleline(text).labelled_by(l.id);
                        focus |= r.gained_focus();
                        ui.end_row();
                        um_edit(ui, "Size", size, &mut focus);
                    }
                    Geometry::Polygon { points } => {
                        ui.label("Vertices");
                        ui.label(points.len().to_string());
                        ui.end_row();
                    }
                }
            });
            if let Geometry::Rect(r) = &mut s.geom {
                *r = IRect::new(r.x0, r.y0, r.x1, r.y1);
            }
            if focus {
                self.history.push(l);
            }
            if s != before {
                if let Some(t) = l.shape_mut(id) {
                    *t = s;
                }
                changed = true;
            }
            ui.add_space(8.0);
            if ui.button("🗑 Delete").clicked() {
                changed |= self.delete_selection(l);
            }
        } else if !self.selection.is_empty() {
            self.selection.retain(|id| l.shape(*id).is_some() || l.instance(*id).is_some());
            theme::section(ui, "Selection");
            ui.label(format!("{} items selected", self.selection.len()));
            if ui.button("🗑 Delete").clicked() {
                changed |= self.delete_selection(l);
            }
        } else {
            let idx = cx.index(l);
            theme::section(ui, "Cell");
            egui::Grid::new("lay_stats").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                ui.label("Shapes");
                ui.label(fmt_count(l.shapes.len() as u64));
                ui.end_row();
                ui.label("Instances");
                ui.label(fmt_count(l.instances.len() as u64));
                ui.end_row();
                ui.label("Flat shapes");
                ui.label(fmt_count(idx.flat_shapes));
                ui.end_row();
                if let Some(b) = idx.bbox {
                    ui.label("Extent");
                    ui.label(format!("{} × {} µm", um(b.width(), tech), um(b.height(), tech)));
                    ui.end_row();
                    ui.label("Area");
                    ui.label(format!("{:.3} µm²", b.width() as f64 * b.height() as f64 / 1e6));
                    ui.end_row();
                }
                ui.label("Grid");
                ui.label(format!("{} nm", tech.grid));
                ui.end_row();
            });
        }
        if changed {
            cx.touched();
        }
        let ops_changed = if self.selection.is_empty() { self.layer_ops_panel(ui, l, cx) } else { self.ops_panel(ui, l, cx) };
        if ops_changed {
            changed = true;
            cx.touched();
        }
        if changed {
            self.layout_changed();
        }
        self.verify_panel(ui, l, cx);
        (changed, open)
    }
}

pub fn fmt_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// One row of the hierarchy tree: the cell link, its placement count and its children.
fn hier_node(
    ui: &mut egui::Ui,
    src: &dyn LayoutSource,
    cell: &CellRef,
    layout: Option<&Layout>,
    count: Option<u64>,
    depth: u32,
    open: &mut Option<CellRef>,
) {
    let mut kids: Vec<(CellRef, u64)> = vec![];
    let layout = layout.or_else(|| src.layout(cell));
    if let Some(l) = layout {
        for inst in &l.instances {
            match kids.iter_mut().find(|(c, _)| c == &inst.cell) {
                Some(k) => k.1 += inst.count(),
                None => kids.push((inst.cell.clone(), inst.count())),
            }
        }
    }
    let header = |ui: &mut egui::Ui, open: &mut Option<CellRef>| {
        let missing = layout.is_none();
        let text = egui::RichText::new(&cell.cell);
        let text = if depth == 0 { text.strong() } else { text };
        if depth == 0 {
            ui.label(text);
        } else if missing {
            ui.label(text.strikethrough()).on_hover_text("cell not found");
        } else if ui.link(text).on_hover_text(format!("Open {cell}")).clicked() {
            *open = Some(cell.clone());
        }
        if let Some(n) = count {
            ui.label(egui::RichText::new(format!("×{}", fmt_count(n))).small().weak());
        }
    };
    if kids.is_empty() || depth >= 12 {
        ui.horizontal(|ui| {
            ui.add_space(18.0);
            header(ui, open);
        });
        return;
    }
    let id = ui.make_persistent_id(("hier", cell, depth));
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, depth == 0)
        .show_header(ui, |ui| header(ui, open))
        .body(|ui| {
            for (c, n) in &kids {
                hier_node(ui, src, c, None, Some(*n), depth + 1, open);
            }
        });
}

fn manhattan(from: (i64, i64), to: (i64, i64)) -> (i64, i64) {
    if (to.0 - from.0).abs() >= (to.1 - from.1).abs() { (to.0, from.1) } else { (from.0, to.1) }
}
