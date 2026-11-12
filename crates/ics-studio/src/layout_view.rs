//! Layout workspace: hierarchical full-custom editor with layer palette, cell placement and DRC.

use std::collections::BTreeSet;

use eframe::egui::{self, Align2, Color32, FontId, Key, Mesh, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use ics_core::geom::IRect;
use ics_core::hier::{self, CellIndex, Hier, LayoutSource, PLANE_EDGE, PLANE_FILL, PLANE_SMALL, Sprite, ViewQuery};
use ics_core::layout::{ArraySpec, Geometry, Layout, Shape, Trans};
use ics_core::project::{CellRef, Project};
use ics_core::tech::{FillStyle, LayerDef, Tech};

use crate::canvas::{View2D, nice_step};
use crate::history::History;
use crate::theme::{self, Palette};

mod assist;
mod edit;
mod verify;
pub use assist::Clip;
pub use edit::LayRequest;
use edit::{EditState, Handle};
pub use verify::Verify;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LayTool {
    Select,
    Stretch,
    Move,
    Copy,
    Rect,
    Polygon,
    Path,
    Label,
    Via,
    Instance,
    Chop,
    Ruler,
}

impl LayTool {
    pub const ALL: [LayTool; 12] = [
        LayTool::Select,
        LayTool::Stretch,
        LayTool::Move,
        LayTool::Copy,
        LayTool::Rect,
        LayTool::Polygon,
        LayTool::Path,
        LayTool::Label,
        LayTool::Via,
        LayTool::Instance,
        LayTool::Chop,
        LayTool::Ruler,
    ];

    pub fn name(self) -> &'static str {
        match self {
            LayTool::Select => "Select",
            LayTool::Stretch => "Stretch",
            LayTool::Move => "Move",
            LayTool::Copy => "Copy",
            LayTool::Rect => "Rectangle",
            LayTool::Polygon => "Polygon",
            LayTool::Path => "Path",
            LayTool::Label => "Label",
            LayTool::Via => "Via",
            LayTool::Instance => "Instance",
            LayTool::Chop => "Chop",
            LayTool::Ruler => "Ruler",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            LayTool::Select => "Esc",
            LayTool::Stretch => "S",
            LayTool::Move => "M",
            LayTool::Copy => "C",
            LayTool::Rect => "R",
            LayTool::Polygon => "Shift+P",
            LayTool::Path => "P",
            LayTool::Label => "L",
            LayTool::Via => "O",
            LayTool::Instance => "I",
            LayTool::Chop => "Shift+C",
            LayTool::Ruler => "K",
        }
    }

    /// Tools that take a box: drag it, or click two corners.
    fn takes_box(self) -> bool {
        matches!(self, LayTool::Rect | LayTool::Via | LayTool::Chop | LayTool::Ruler)
    }
}

enum LDrag {
    Move {
        start: (i64, i64),
        offset: (i64, i64),
    },
    Rubber {
        start: Pos2,
    },
    Create {
        start: (i64, i64),
    },
    /// Move / copy tool: press at the reference point, release at the destination.
    Xfer,
    Handle {
        id: u64,
        handle: Handle,
        start: (i64, i64),
        orig: Geometry,
        cur: Geometry,
    },
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
    /// Set while the cell is edited in place inside a parent.
    pub context: Option<&'a EipContext>,
}

/// Edit-in-place: the outermost cell the edited one was entered from, shown dimmed around it.
#[derive(Debug, Clone, PartialEq)]
pub struct EipContext {
    pub root: CellRef,
    /// Edited cell to root coordinates (also the editor's display transform).
    pub to_root: Trans,
    /// Instance id and array element at each level from the root down to the edited placement.
    pub path: Vec<(u64, (u32, u32))>,
}

/// The project with the edited cell (taken out of it while the editor runs) put back.
pub struct Live<'a> {
    pub src: &'a Project,
    pub cell: &'a CellRef,
    pub layout: &'a Layout,
}

impl LayoutSource for Live<'_> {
    fn layout(&self, cell: &CellRef) -> Option<&Layout> {
        if cell == self.cell { Some(self.layout) } else { self.src.layout(cell) }
    }
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
    /// Per layer: shapes can be picked (Virtuoso LSW "selectable").
    pub selectable: Vec<bool>,
    pub inst_selectable: bool,
    layer_filter: String,
    /// Measurements kept on screen until Shift+K.
    pub rulers: Vec<((i64, i64), (i64, i64))>,
    /// Snap spacing in dbu (0 = technology grid).
    pub snap: i64,
    /// Snap the cursor to nearby vertices and edges.
    pub gravity: bool,
    snapped: Option<bool>,
    /// Show spacing / width violations while drawing and moving.
    pub drd: bool,
    /// First corner of a box entered with two clicks.
    box_start: Option<(i64, i64)>,
    /// Next box zooms the view (Z).
    zoom_box: bool,
    /// Reference point of the move / copy tool.
    xfer: Option<(i64, i64)>,
    /// Rotation / mirror picked up during move / copy (right-click), about the reference point.
    xfer_t: Trans,
    /// Stretch in progress: shape, grabbed edge or vertex, reference point, original geometry.
    stretch: Option<(u64, Handle, (i64, i64), Geometry)>,
    clip: Option<Clip>,
    /// Clipboard following the cursor until a click drops it.
    pub paste: Option<Clip>,
    view_back: Vec<(f64, f64, f64)>,
    view_fwd: Vec<(f64, f64, f64)>,
    /// Last plain click and the index picked among the items under it (repeat clicks cycle).
    cycle: Option<((i64, i64), usize)>,
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
            selectable: vec![],
            inst_selectable: true,
            layer_filter: String::new(),
            rulers: vec![],
            snap: 0,
            gravity: true,
            snapped: None,
            drd: true,
            box_start: None,
            zoom_box: false,
            xfer: None,
            xfer_t: Trans::IDENTITY,
            stretch: None,
            clip: None,
            paste: None,
            view_back: vec![],
            view_fwd: vec![],
            cycle: None,
        }
    }
}

/// Tech layer colors are tuned for a dark canvas; on a light one, pale colors (white
/// contact / text, light gray boundary, yellows) are darkened to keep contrast.
fn layer_color(def: &LayerDef, dark: bool) -> Color32 {
    let [r, g, b, _] = def.color;
    if dark {
        return Color32::from_rgb(r, g, b);
    }
    let luma = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    let k = (130.0 / luma.max(1.0)).min(1.0);
    let f = |c: u8| (c as f32 * k).round() as u8;
    Color32::from_rgb(f(r), f(g), f(b))
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
    let c = layer_color(def, painter.ctx().global_style().visuals.dark_mode);
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

/// Most labels and instance frames drawn per frame; beyond this they are skipped.
const MAX_LABELS: usize = 400;
const MAX_FRAMES: usize = 5_000;

/// Coverage grid at 2 px resolution for shapes too small to draw individually, with a few
/// planes drawn in different strengths. Marked cells are drawn as a texture (or merged runs),
/// so the cost is bounded by the screen size rather than the number of shapes.
struct Raster {
    origin: Pos2,
    cell: f32,
    w: usize,
    h: usize,
    planes: Vec<Vec<u64>>,
}

impl Raster {
    /// Cell size (points) of the coverage grids for shapes.
    const CELL: f32 = 2.0;

    fn new(rect: Rect, planes: usize, cell: f32) -> Self {
        let w = (rect.width() / cell).ceil().max(1.0) as usize;
        let h = (rect.height() / cell).ceil().max(1.0) as usize;
        Self { origin: rect.min, cell, w, h, planes: vec![vec![]; planes] }
    }

    fn words(&self) -> usize {
        self.w.div_ceil(64)
    }

    fn plane_mut(&mut self, plane: usize) -> &mut [u64] {
        let n = self.words() * self.h;
        let bits = &mut self.planes[plane];
        if bits.is_empty() {
            *bits = vec![0; n];
        }
        bits
    }

    fn mark(&mut self, plane: usize, r: Rect) {
        let to = |v: f32, o: f32| ((v - o) / self.cell).floor() as f64;
        self.fill(
            plane,
            (to(r.min.x, self.origin.x), to(r.min.y, self.origin.y)),
            (to(r.max.x, self.origin.x), to(r.max.y, self.origin.y)),
        );
    }

    /// Mark the cells from `a` to `b` (cell coordinates, inclusive, clipped).
    fn fill(&mut self, plane: usize, a: (f64, f64), b: (f64, f64)) {
        let (w, h) = (self.w as f64, self.h as f64);
        let (x0, x1) = (a.0.min(b.0).floor().max(0.0), a.0.max(b.0).floor().min(w - 1.0));
        let (y0, y1) = (a.1.min(b.1).floor().max(0.0), a.1.max(b.1).floor().min(h - 1.0));
        if x0 > x1 || y0 > y1 {
            return;
        }
        let words = self.words();
        let bits = self.plane_mut(plane);
        for y in y0 as usize..=y1 as usize {
            hier::set_bits(&mut bits[y * words..(y + 1) * words], x0 as usize, x1 as usize + 1);
        }
    }

    /// Mark the cells holding the centres of the set pixels of `bits` (one plane of `sp`)
    /// placed by `t`.
    #[allow(clippy::too_many_arguments)]
    fn stamp(&mut self, plane: usize, sp: &Sprite, bits: &[u64], t: &Trans, view: &View2D, rect: Rect) {
        let p = sp.pixel() as f64;
        let t = &view.orient.compose(t);
        let k = view.scale / self.cell as f64;
        let ys = if view.y_up { -1.0 } else { 1.0 };
        let c0 = ((rect.center().x - self.origin.x) / self.cell, (rect.center().y - self.origin.y) / self.cell);
        let cell = |i: f64, j: f64| {
            let (x, y) = t.apply_f((sp.origin.0 as f64 + (i + 0.5) * p, sp.origin.1 as f64 + (j + 0.5) * p));
            (c0.0 as f64 + (x - view.center.0) * k, c0.1 as f64 + ys * (y - view.center.1) * k)
        };
        let o = cell(0.0, 0.0);
        let (di, dj) = {
            let (a, b) = (cell(1.0, 0.0), cell(0.0, 1.0));
            ((a.0 - o.0, a.1 - o.1), (b.0 - o.0, b.1 - o.1))
        };
        let (w, h) = (self.w as f64, self.h as f64);
        let words = self.words();
        if di.1 == 0.0 && dj.0 == 0.0 {
            let cols: Vec<i64> = (0..sp.w).map(|i| (o.0 + i as f64 * di.0).floor() as i64).collect();
            let (lo, hi) = cols.iter().enumerate().fold((sp.w, 0), |(lo, hi), (i, &x)| {
                if (0..self.w as i64).contains(&x) { (lo.min(i), hi.max(i + 1)) } else { (lo, hi) }
            });
            if lo >= hi {
                return;
            }
            let last = self.w as i64 - 1;
            let out = self.plane_mut(plane);
            for j in 0..sp.h {
                let y = (o.1 + j as f64 * dj.1).floor();
                if y < 0.0 || y >= h {
                    continue;
                }
                let row = &mut out[y as usize * words..(y as usize + 1) * words];
                hier::bit_runs(sp.row(bits, j), lo, hi, |a, b| {
                    let (x0, x1) = (cols[a].min(cols[b - 1]).max(0), cols[a].max(cols[b - 1]).min(last));
                    hier::set_bits(row, x0 as usize, x1 as usize + 1);
                });
            }
            return;
        }
        let span = |b: f64, d: f64, n: f64| {
            if d.abs() < 1e-12 {
                if (-1.0..n + 1.0).contains(&b) { (f64::NEG_INFINITY, f64::INFINITY) } else { (1.0, 0.0) }
            } else {
                let (u, v) = ((-1.0 - b) / d, (n + 1.0 - b) / d);
                (u.min(v), u.max(v))
            }
        };
        for j in 0..sp.h {
            let base = (o.0 + j as f64 * dj.0, o.1 + j as f64 * dj.1);
            let (ax, bx) = span(base.0, di.0, w);
            let (ay, by) = span(base.1, di.1, h);
            let (lo, hi) = (ax.max(ay).max(0.0), bx.min(by).min(sp.w as f64));
            if lo >= hi {
                continue;
            }
            hier::bit_runs(sp.row(bits, j), lo.floor() as usize, hi.ceil() as usize, |a, b| {
                let at = |i: usize| (base.0 + i as f64 * di.0, base.1 + i as f64 * di.1);
                self.fill(plane, at(a), at(b - 1));
            });
        }
    }

    /// One-cell wide outline of the screen rectangle `r` (plane 0), covering the cells whose
    /// centres a one-cell wide outline mesh would cover.
    fn outline(&mut self, r: Rect) {
        let to = |v: f32, o: f32, d: f32| ((v - o) / self.cell + d).floor() as f64;
        let (x0, y0) = (to(r.min.x, self.origin.x, 0.5), to(r.min.y, self.origin.y, 0.5));
        let (x1, y1) = (to(r.max.x, self.origin.x, -0.5).max(x0), to(r.max.y, self.origin.y, -0.5).max(y0));
        self.fill(0, (x0, y0), (x1, y0));
        self.fill(0, (x0, y1), (x1, y1));
        self.fill(0, (x0, y0), (x0, y1));
        self.fill(0, (x1, y0), (x1, y1));
    }

    fn runs(&self, plane: usize, y: usize, f: impl FnMut(usize, usize)) {
        let words = self.words();
        hier::bit_runs(&self.planes[plane][y * words..(y + 1) * words], 0, self.w, f);
    }

    /// Draw the marked cells as one nearest-filtered texture, `colors` lists (plane, colour)
    /// with the strongest plane first. The texture is only re-uploaded when it changed.
    fn paint(&self, painter: &egui::Painter, key: egui::Id, colors: &[(usize, Color32)]) {
        if colors.iter().all(|&(p, _)| self.planes[p].is_empty()) {
            return;
        }
        let hash = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (self.w, self.h, colors, &self.planes).hash(&mut h);
            h.finish()
        };
        let image = || {
            let mut pixels = vec![Color32::TRANSPARENT; self.w * self.h];
            for &(plane, color) in colors.iter().rev().filter(|(p, _)| !self.planes[*p].is_empty()) {
                for y in 0..self.h {
                    let row = &mut pixels[y * self.w..(y + 1) * self.w];
                    self.runs(plane, y, |a, b| row[a..b].fill(color));
                }
            }
            egui::ColorImage::new([self.w, self.h], pixels)
        };
        let ctx = painter.ctx();
        let tex = match ctx.data(|d| d.get_temp::<(egui::TextureHandle, u64)>(key)) {
            Some((t, h)) if h == hash => t,
            Some((mut t, _)) => {
                t.set(image(), egui::TextureOptions::NEAREST);
                ctx.data_mut(|d| d.insert_temp(key, (t.clone(), hash)));
                t
            }
            None => {
                let t = ctx.load_texture("layout-raster", image(), egui::TextureOptions::NEAREST);
                ctx.data_mut(|d| d.insert_temp(key, (t.clone(), hash)));
                t
            }
        };
        let size = Vec2::new(self.w as f32, self.h as f32) * self.cell;
        let mut mesh = Mesh::with_texture(tex.id());
        mesh.add_rect_with_uv(
            Rect::from_min_size(self.origin, size),
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        painter.add(egui::Shape::mesh(mesh));
    }

    /// Add the marked cells to `mesh` as rectangles (runs identical to the row above extend
    /// the rectangle started there), weakest plane first.
    fn emit(&self, mesh: &mut Mesh, colors: &[(usize, Color32)]) {
        let cell = self.cell;
        for &(plane, color) in colors.iter().rev().filter(|(p, _)| !self.planes[*p].is_empty()) {
            let quad = |mesh: &mut Mesh, (x0, x1, y0): (usize, usize, usize), y1: usize| {
                let min = self.origin + Vec2::new(x0 as f32 * cell, y0 as f32 * cell);
                mesh.add_colored_rect(
                    Rect::from_min_size(min, Vec2::new((x1 - x0) as f32 * cell, (y1 - y0) as f32 * cell)),
                    color,
                );
            };
            let mut open: Vec<(usize, usize, usize)> = vec![];
            for y in 0..=self.h {
                let mut runs: Vec<(usize, usize)> = vec![];
                if y < self.h {
                    self.runs(plane, y, |a, b| runs.push((a, b)));
                }
                let mut next = Vec::with_capacity(runs.len());
                let mut k = 0;
                for (a, b) in runs {
                    while k < open.len() && open[k].0 < a {
                        quad(mesh, open[k], y);
                        k += 1;
                    }
                    if k < open.len() && open[k].0 == a && open[k].1 == b {
                        next.push(open[k]);
                        k += 1;
                    } else {
                        if k < open.len() && open[k].0 == a {
                            quad(mesh, open[k], y);
                            k += 1;
                        }
                        next.push((a, b, y));
                    }
                }
                for &o in &open[k..] {
                    quad(mesh, o, y);
                }
                open = next;
            }
        }
    }

    /// Draw as a texture on the canvas, or into `mesh` for thumbnails.
    fn draw(&self, painter: &egui::Painter, mesh: &mut Mesh, key: Option<egui::Id>, colors: &[(usize, Color32)]) {
        match key {
            Some(key) => self.paint(painter, key, colors),
            None => self.emit(mesh, colors),
        }
    }
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
    let (a, b) = (view.to_world(rect, rect.min), view.to_world(rect, rect.max));
    IRect::new(a.0.min(b.0).floor() as i64, a.1.min(b.1).floor() as i64, a.0.max(b.0).ceil() as i64, a.1.max(b.1).ceil() as i64)
}

fn canvas_bg(pal: &Palette) -> Color32 {
    if pal.dark { Color32::from_rgb(0x0e, 0x10, 0x13) } else { pal.bg_canvas }
}

/// Grid in display coordinates (the parent's frame during edit-in-place).
fn draw_grid(painter: &egui::Painter, view: &View2D, rect: Rect, pal: &Palette, grid: i64) {
    let view = &View2D { orient: Trans::IDENTITY, ..*view };
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
        limit: if o.thumbnail { 50_000 } else { 2_000_000 },
        lod: hier::lod_level(Raster::CELL as f64 * px),
    };
    let vis = hier::query(hier, src, layout, &idx, &q);
    let selected = |id: u64| o.selection.is_some_and(|s| s.contains(&id));
    let has_sel = o.selection.is_some_and(|s| !s.is_empty());
    let to_s = |x: f64, y: f64| view.to_screen(rect, x, y);
    let drawn = vis.count as u64 + vis.stamped;
    let stipple = !o.thumbnail && drawn < 20_000;
    let thin = |g: &Geometry, t: &Trans| {
        let b = t.apply_rect(g.bbox());
        (b.width().min(b.height()) as f64 * view.scale) < 3.0
    };
    let raster_key = |slot: usize| (!o.thumbnail).then(|| egui::Id::unique(("layout-raster", painter.layer_id(), slot)));
    let mut labels = 0;
    for li in 0..tech.layers.len() {
        if !o.visible.get(li).copied().unwrap_or(true) {
            continue;
        }
        let def = &tech.layers[li];
        let c = layer_color(def, pal.dark);
        let fa = fill_alpha(def);
        let mesh_slot = painter.add(egui::Shape::Noop);
        let mut mesh = Mesh::default();
        let mut raster = Raster::new(rect, 3, Raster::CELL);
        for b in vis.tiny.get(li).into_iter().flatten() {
            raster.mark(PLANE_SMALL, screen_rect(view, rect, *b));
        }
        for s in &vis.stamps {
            for plane in [PLANE_SMALL, PLANE_FILL, PLANE_EDGE] {
                if let Some(bits) = s.sprite.plane(li, plane) {
                    raster.stamp(plane, &s.sprite, bits, &s.trans, view, rect);
                }
            }
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
                        raster.mark(PLANE_SMALL, sr);
                    } else if stipple {
                        pattern_rect(painter, sr, def, emphasis);
                    } else {
                        let clipped = sr.intersect(rect.expand(2.0));
                        if fa > 0.0 {
                            mesh.add_colored_rect(clipped, c.gamma_multiply(fa.max(0.25) * emphasis));
                        }
                        if sr.width() >= 6.0 && sr.height() >= 6.0 {
                            mesh_outline(&mut mesh, clipped, 1.0, c.gamma_multiply(emphasis));
                        } else if fa == 0.0 {
                            mesh.add_colored_rect(clipped, c.gamma_multiply(0.6 * emphasis));
                        }
                    }
                }
                Geometry::Polygon { .. } if thin(&p.shape.geom, &p.trans) => {
                    raster.mark(PLANE_SMALL, screen_rect(view, rect, p.trans.apply_rect(p.shape.bbox())));
                }
                Geometry::Path { points, width } if (*width as f64 * view.scale) < 3.0 => {
                    for w in points.windows(2) {
                        let seg = Geometry::Path { points: w.to_vec(), width: *width }.bbox();
                        raster.mark(PLANE_SMALL, screen_rect(view, rect, p.trans.apply_rect(seg)));
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
                    if o.thumbnail || fs < if p.depth > 0 { 6.0 } else { 5.0 } || labels >= MAX_LABELS {
                        continue;
                    }
                    labels += 1;
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
        let mut colors = vec![(PLANE_EDGE, c.gamma_multiply(0.8)), (PLANE_SMALL, c.gamma_multiply(0.7))];
        if fa > 0.0 {
            colors.push((PLANE_FILL, c.gamma_multiply(fa.max(0.25))));
        }
        raster.draw(painter, &mut mesh, raster_key(li), &colors);
        painter.set(mesh_slot, egui::Shape::mesh(mesh));
    }

    // Instances shown as boxes (beyond the level limit, too small, or missing).
    let mut mesh = Mesh::default();
    let mut frames = Raster::new(rect, 1, 1.0);
    for s in &vis.stamps {
        if let Some((grid, bits)) = s.sprite.frames() {
            frames.stamp(0, grid, bits, &s.trans, view, rect);
        }
    }
    let mut names = 0;
    for b in &vis.boxes {
        let sr = screen_rect(view, rect, b.rect);
        let col = if b.missing { pal.danger } else { pal.text_weak.gamma_multiply(0.8) };
        if b.missing {
            mesh_outline(&mut mesh, sr.intersect(rect.expand(2.0)), 1.0, col);
        } else {
            frames.outline(sr);
        }
        if !o.thumbnail && sr.width() > 70.0 && sr.height() > 18.0 && names < 400 {
            names += 1;
            let label = if b.missing { format!("{} (missing)", b.cell) } else { b.cell.cell.clone() };
            painter.text(sr.center(), Align2::CENTER_CENTER, label, FontId::proportional(11.0), col);
        }
    }
    frames.draw(painter, &mut mesh, raster_key(usize::MAX), &[(0, pal.text_weak.gamma_multiply(0.8))]);
    painter.add(egui::Shape::mesh(mesh));

    // Frames of the top cell's own instances.
    if !o.thumbnail {
        let mut frames = Mesh::default();
        let (mut n_frames, mut n_names) = (0, 0);
        for i in idx.instances_in(world) {
            let (Some(b), Some(inst)) = (idx.instance_bbox(layout, i), layout.instances.get(i)) else { continue };
            let sr = screen_rect(view, rect, b);
            let sel = selected(inst.id);
            if sel {
                painter.rect_stroke(sr, 0.0, Stroke::new(2.0, pal.selection), StrokeKind::Outside);
            } else if sr.width().min(sr.height()) >= 6.0 && n_frames < MAX_FRAMES {
                n_frames += 1;
                mesh_outline(&mut frames, sr.intersect(rect.expand(2.0)), 1.0, pal.text_weak.gamma_multiply(0.9));
            }
            if sr.width() > 50.0 && sr.height() > 16.0 && n_names < MAX_LABELS {
                n_names += 1;
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
        painter.add(egui::Shape::mesh(frames));
    }

    // Selected top-level shapes.
    if has_sel {
        for s in layout.shapes.iter().filter(|s| selected(s.id)) {
            let sr = screen_rect(view, rect, s.bbox());
            painter.rect_stroke(sr.expand(1.5), 0.0, Stroke::new(2.0, pal.selection), StrokeKind::Outside);
        }
    }
    DrawStats { drawn: drawn as usize, truncated: vis.truncated }
}

/// Items (shapes or instances) of the top cell under the cursor, most specific first: smaller
/// items, then higher layers. `pick` gives the pickable layers.
fn hits(layout: &Layout, idx: &CellIndex, pick: &[bool], insts: bool, x: i64, y: i64, tol: i64) -> Vec<u64> {
    let area = IRect::new(x - tol, y - tol, x + tol, y + tol);
    let mut found: Vec<(i128, usize, u64)> = vec![];
    let area_of = |b: IRect| b.width().max(1) as i128 * b.height().max(1) as i128;
    for (li, ok) in pick.iter().enumerate() {
        if !ok {
            continue;
        }
        for i in idx.shapes_in(li, area) {
            if let Some(s) = layout.shapes.get(i)
                && s.hit(x, y, tol)
            {
                found.push((area_of(s.bbox()), s.layer, s.id));
            }
        }
    }
    if insts {
        for i in idx.instances_in(area) {
            if let (Some(b), Some(inst)) = (idx.instance_bbox(layout, i), layout.instances.get(i))
                && b.contains(x, y)
            {
                found.push((area_of(b), 0, inst.id));
            }
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    found.dedup_by_key(|f| f.2);
    found.into_iter().map(|f| f.2).collect()
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
        if self.selectable.len() != tech.layers.len() {
            self.selectable = vec![true; tech.layers.len()];
        }
        if self.active_layer >= tech.layers.len() {
            self.active_layer = 0;
        }
    }

    /// Layers whose shapes can be picked: visible and selectable.
    fn pickable(&self) -> Vec<bool> {
        self.visible.iter().zip(&self.selectable).map(|(v, s)| *v && *s).collect()
    }

    /// Effective snap spacing: the snap setting rounded to the technology grid.
    pub fn snap_step(&self, tech: &Tech) -> i64 {
        let g = tech.grid.max(1);
        if self.snap <= g { g } else { self.snap / g * g }
    }

    /// Drop results derived from the layout (verification, flylines).
    pub fn layout_changed(&mut self) {
        self.verify.layout_changed();
        self.flylines = None;
    }

    /// Take over a view (entering or leaving edit-in-place keeps the picture in place).
    pub fn adopt_view(&mut self, v: View2D) {
        self.view = v;
        self.needs_fit = false;
    }

    pub fn request_fit(&mut self) {
        self.needs_fit = true;
    }

    pub fn set_tool(&mut self, t: LayTool) {
        self.tool = t;
        self.path_pts.clear();
        self.poly_pts.clear();
        self.box_start = None;
        self.xfer = None;
        self.xfer_t = Trans::IDENTITY;
        self.stretch = None;
        self.zoom_box = false;
        self.paste = None;
    }

    /// Arm zoom box: the next box (drag or two clicks) becomes the view.
    pub fn arm_zoom_box(&mut self) {
        self.zoom_box = true;
        self.box_start = None;
    }

    fn push_view(&mut self) {
        self.view_back.push((self.view.center.0, self.view.center.1, self.view.scale));
        if self.view_back.len() > 64 {
            self.view_back.remove(0);
        }
        self.view_fwd.clear();
    }

    pub fn can_step_view(&self, back: bool) -> bool {
        !if back { &self.view_back } else { &self.view_fwd }.is_empty()
    }

    /// Previous (W) or next (Shift+W) view.
    pub fn step_view(&mut self, back: bool) -> bool {
        let cur = (self.view.center.0, self.view.center.1, self.view.scale);
        let (from, to) = if back { (&mut self.view_back, &mut self.view_fwd) } else { (&mut self.view_fwd, &mut self.view_back) };
        let Some(v) = from.pop() else { return false };
        to.push(cur);
        self.view.center = (v.0, v.1);
        self.view.scale = v.2;
        true
    }

    fn zoom_box_to(&mut self, rect: Rect, b: IRect) {
        if b.width() <= 0 || b.height() <= 0 {
            return;
        }
        self.push_view();
        self.view.fit(rect, (b.x0 as f64, b.y0 as f64), (b.x1 as f64, b.y1 as f64), 10.0);
    }

    /// The command prompt shown on the canvas (what the next click does).
    pub fn prompt(&self) -> Option<String> {
        if self.paste.is_some() {
            return Some("Paste: click to place · Esc cancels".into());
        }
        let second = self.box_start.is_some() || matches!(self.drag, Some(LDrag::Create { .. }));
        if self.zoom_box {
            return Some(
                if second { "Zoom: click the opposite corner" } else { "Zoom: drag a box or click its first corner" }.into(),
            );
        }
        let s = match self.tool {
            LayTool::Select => return None,
            LayTool::Stretch if self.stretch.is_some() => "Stretch: click the destination · Esc cancels".into(),
            LayTool::Stretch => "Stretch: click an edge or vertex of a shape".into(),
            LayTool::Move | LayTool::Copy => {
                let n = self.tool.name();
                match self.xfer {
                    None if self.selection.is_empty() => format!("{n}: click an object at its reference point"),
                    None => format!("{n}: click the reference point"),
                    Some(_) if self.tool == LayTool::Copy => {
                        "Copy: click the destination (repeats) · right-click rotates, Shift+right-click flips".into()
                    }
                    Some(_) => "Move: click the destination · right-click rotates, Shift+right-click flips".into(),
                }
            }
            LayTool::Rect if second => "Rectangle: click the opposite corner".into(),
            LayTool::Rect => "Rectangle: drag, or click the first corner".into(),
            LayTool::Via => "Via: click for one cut, drag to fill an area".into(),
            LayTool::Chop if second => "Chop: click the opposite corner".into(),
            LayTool::Chop => "Chop: box to cut out of the selected shapes (or any shape under it)".into(),
            LayTool::Ruler if second => "Ruler: click the end point".into(),
            LayTool::Ruler => "Ruler: click two points · Shift+K clears rulers".into(),
            LayTool::Path if self.path_pts.is_empty() => "Path: click the start point · +/- change metal".into(),
            LayTool::Path => "Path: click points · +/- change metal with a via · Enter finishes".into(),
            LayTool::Polygon => "Polygon: click vertices (Shift: any angle) · Enter closes".into(),
            LayTool::Label => format!("Label '{}': click to place", self.label_text.trim()),
            LayTool::Instance => "Instance: click to place".into(),
        };
        Some(s)
    }

    /// Reference point of the command in progress, for dX / dY readouts.
    pub fn anchor(&self) -> Option<(i64, i64)> {
        if let Some(LDrag::Create { start } | LDrag::Move { start, .. } | LDrag::Handle { start, .. }) = &self.drag {
            return Some(*start);
        }
        self.xfer
            .or(self.stretch.as_ref().map(|s| s.2))
            .or(self.box_start)
            .or(self.path_pts.last().copied())
            .or(self.poly_pts.last().copied())
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

    /// A rotation / flip given in display coordinates as the same operation in cell
    /// coordinates (they differ while editing a rotated or mirrored placement in place).
    fn on_screen(&self, k: Trans) -> Trans {
        let o = self.view.orient;
        o.inverse().compose(&k).compose(&o)
    }

    /// [`Self::on_screen`] for an operation about the origin, such as the move / copy turn.
    fn on_screen_linear(&self, k: Trans) -> Trans {
        let o = Trans { x: 0, y: 0, ..self.view.orient };
        o.inverse().compose(&k).compose(&o)
    }

    /// Move the selection by a vector given in display coordinates.
    fn nudge(&mut self, l: &mut Layout, dx: i64, dy: i64) -> bool {
        if self.selection.is_empty() {
            return false;
        }
        let (dx, dy) = self.view.orient.inverse().apply_vec((dx, dy));
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

    /// A world point entered with the mouse: gravity-snapped to nearby geometry when enabled,
    /// else snapped to the snap grid.
    fn pick_point(&mut self, l: &Layout, cx: &mut LayCtx, w: (f64, f64)) -> (i64, i64) {
        let step = self.snap_step(cx.tech);
        self.snapped = None;
        if self.gravity && self.tool != LayTool::Select && !self.zoom_box {
            let idx = cx.index(l);
            if let Some((p, vertex)) = assist::gravity(l, &idx, &self.visible, w, 8.0 / self.view.scale, step) {
                self.snapped = Some(vertex);
                return p;
            }
        }
        self.snap(w, step)
    }

    fn finish_path(&mut self, l: &mut Layout, tech: &Tech) -> bool {
        let pts = std::mem::take(&mut self.path_pts);
        if pts.len() < 2 {
            return false;
        }
        self.history.push(l);
        let w = self.wire_width(tech);
        let id = l.add(self.active_layer, Geometry::Path { points: pts, width: w });
        self.selection = [id].into();
        true
    }

    fn wire_width(&self, tech: &Tech) -> i64 {
        if self.path_width > 0 { self.path_width } else { tech.layers[self.active_layer].min_width.max(tech.grid * 10) }
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

    fn select_all(&mut self, l: &Layout) {
        let pick = self.pickable();
        self.selection = l.shapes.iter().filter(|s| pick.get(s.layer).copied().unwrap_or(false)).map(|s| s.id).collect();
        if self.inst_selectable {
            self.selection.extend(l.instances.iter().map(|i| i.id));
        }
    }

    fn handle_keys(&mut self, ui: &egui::Ui, l: &mut Layout, cx: &mut LayCtx, rect: Rect) -> bool {
        let tech = cx.tech;
        if ui.ctx().egui_wants_keyboard_input() {
            return false;
        }
        let mut changed = false;
        let (keys, clip_events, hover) = ui.input(|i| {
            let mut keys = vec![];
            let mut clip = vec![];
            for e in &i.events {
                match e {
                    egui::Event::Key { key, pressed: true, modifiers, .. } => keys.push((*key, *modifiers)),
                    egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_) => clip.push(e.clone()),
                    _ => {}
                }
            }
            (keys, clip, i.pointer.hover_pos())
        });
        let did_clip = !clip_events.is_empty();
        for e in clip_events {
            match e {
                egui::Event::Copy | egui::Event::Cut => {
                    if let Some(text) = self.yank(l, cx) {
                        ui.ctx().copy_text(text);
                        if matches!(e, egui::Event::Cut) {
                            changed |= self.delete_selection(l);
                        }
                    }
                }
                egui::Event::Paste(text) => {
                    self.begin_paste(Some(&text));
                }
                _ => {}
            }
        }
        let g = self.snap_step(tech) * 10;
        let anchor = hover.filter(|p| rect.contains(*p)).unwrap_or(rect.center());
        let at = hover.filter(|p| rect.contains(*p)).map(|p| {
            let w = self.view.to_world(rect, p);
            (w.0 as i64, w.1 as i64)
        });
        let tol = (5.0 / self.view.scale) as i64;
        for (k, m) in keys {
            if m.command {
                match k {
                    Key::A => self.select_all(l),
                    Key::D => self.selection.clear(),
                    Key::F => self.max_depth = 0,
                    Key::C | Key::X if !did_clip => {
                        if let Some(text) = self.yank(l, cx) {
                            ui.ctx().copy_text(text);
                            if k == Key::X {
                                changed |= self.delete_selection(l);
                            }
                        }
                    }
                    Key::V if !did_clip => {
                        self.begin_paste(None);
                    }
                    _ => {}
                }
                continue;
            }
            if m.shift {
                match k {
                    Key::C => self.set_tool(LayTool::Chop),
                    Key::P => self.set_tool(LayTool::Polygon),
                    Key::M => changed |= self.merge_selection(l),
                    Key::K => self.rulers.clear(),
                    Key::Z => {
                        self.push_view();
                        self.view.zoom_at(rect, rect.center(), 0.5);
                    }
                    Key::W => {
                        self.step_view(false);
                    }
                    Key::F => self.max_depth = 32,
                    Key::U => changed |= self.redo(l),
                    Key::Y => {
                        self.begin_paste(None);
                    }
                    Key::O => {
                        if self.xfer.is_some() {
                            self.xfer_t = self.on_screen_linear(Trans::rotate_about((0, 0))).compose(&self.xfer_t);
                        } else {
                            changed |= self.transform_selection(l, cx, 0);
                        }
                    }
                    Key::X => self.descend(l, cx, at, tol),
                    Key::B => self.requests.push(LayRequest::Return),
                    Key::N => {
                        self.verify.pick_nets = !self.verify.pick_nets;
                        let m =
                            if self.verify.pick_nets { "Mark net: click a shape to highlight its net" } else { "Mark net off" };
                        self.note(true, m);
                    }
                    Key::ArrowLeft => changed |= self.nudge(l, -g, 0),
                    Key::ArrowRight => changed |= self.nudge(l, g, 0),
                    Key::ArrowUp => changed |= self.nudge(l, 0, g),
                    Key::ArrowDown => changed |= self.nudge(l, 0, -g),
                    Key::Plus | Key::Equals if self.tool == LayTool::Path => {
                        changed |= self.switch_wire_layer(l, tech, true);
                    }
                    _ => {}
                }
                continue;
            }
            match k {
                Key::Escape => {
                    if self.paste.is_some() || self.zoom_box {
                        self.paste = None;
                        self.zoom_box = false;
                        self.box_start = None;
                    } else if self.box_start.is_some() || self.xfer.is_some() || self.stretch.is_some() {
                        self.box_start = None;
                        self.xfer = None;
                        self.xfer_t = Trans::IDENTITY;
                        self.stretch = None;
                    } else if !self.path_pts.is_empty() || !self.poly_pts.is_empty() {
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
                Key::S => self.set_tool(LayTool::Stretch),
                Key::M => self.set_tool(LayTool::Move),
                Key::C => self.set_tool(LayTool::Copy),
                Key::R => self.set_tool(LayTool::Rect),
                Key::P => self.set_tool(LayTool::Path),
                Key::L => self.set_tool(LayTool::Label),
                Key::O => self.set_tool(LayTool::Via),
                Key::I => self.set_tool(LayTool::Instance),
                Key::K => self.set_tool(LayTool::Ruler),
                Key::U => changed |= self.undo(l),
                Key::Y => {
                    if let Some(text) = self.yank(l, cx) {
                        ui.ctx().copy_text(text);
                    }
                }
                Key::T => self.layer_tap(l, cx, at, tol),
                Key::G => {
                    self.gravity = !self.gravity;
                    self.note(true, if self.gravity { "Gravity on" } else { "Gravity off" });
                }
                Key::Z => self.arm_zoom_box(),
                Key::W => {
                    self.step_view(true);
                }
                Key::F => {
                    self.push_view();
                    self.needs_fit = true;
                }
                Key::CloseBracket => {
                    self.push_view();
                    self.view.zoom_at(rect, anchor, 2.0);
                }
                Key::OpenBracket => {
                    self.push_view();
                    self.view.zoom_at(rect, anchor, 0.5);
                }
                Key::Plus | Key::Equals | Key::PageUp if self.tool == LayTool::Path => {
                    changed |= self.switch_wire_layer(l, tech, true);
                }
                Key::Minus | Key::PageDown if self.tool == LayTool::Path => {
                    changed |= self.switch_wire_layer(l, tech, false);
                }
                Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp | Key::ArrowDown => {
                    let (w, h) = (rect.width() / 4.0, rect.height() / 4.0);
                    self.view.pan_pixels(match k {
                        Key::ArrowLeft => Vec2::new(w, 0.0),
                        Key::ArrowRight => Vec2::new(-w, 0.0),
                        Key::ArrowUp => Vec2::new(0.0, h),
                        _ => Vec2::new(0.0, -h),
                    });
                }
                Key::Num0 | Key::Num1 | Key::Num2 | Key::Num3 | Key::Num4 | Key::Num5 | Key::Num6 | Key::Num7 | Key::Num8 => {
                    self.max_depth = k.name().parse().unwrap_or(32);
                }
                Key::Num9 => self.max_depth = 32,
                _ => {}
            }
        }
        changed
    }

    /// Layer tap (T): make the layer of the shape under the cursor active.
    fn layer_tap(&mut self, l: &Layout, cx: &mut LayCtx, at: Option<(i64, i64)>, tol: i64) {
        let Some(p) = at else { return };
        let idx = cx.index(l);
        match hits(l, &idx, &self.visible, false, p.0, p.1, tol).into_iter().find_map(|id| l.shape(id)) {
            Some(s) => {
                self.active_layer = s.layer;
                let name = cx.tech.layer(s.layer).map_or("?", |d| d.name.as_str());
                self.note(true, format!("Active layer: {name}"));
            }
            None => self.note(false, "Layer tap: no shape under the cursor"),
        }
    }

    /// Edit in place (Shift+X) the selected instance or the one under the cursor.
    fn descend(&mut self, l: &Layout, cx: &mut LayCtx, at: Option<(i64, i64)>, tol: i64) {
        let sel = (self.selection.len() == 1).then(|| l.instance(*self.selection.first().unwrap())).flatten();
        let under = || {
            let p = at?;
            let idx = cx.index(l);
            hits(l, &idx, &[], true, p.0, p.1, tol).into_iter().find_map(|id| l.instance(id))
        };
        let Some(i) = sel.or_else(under) else {
            self.note(false, "Edit in place: select an instance or point at one");
            return;
        };
        let child = cx.src.layout(&i.cell).and_then(|c| cx.hier.get_for(&i.cell, c, cx.src).bbox);
        let (c, r) = i.dims();
        let elem = match (at, child) {
            (Some(p), Some(b)) => (0..r)
                .flat_map(|j| (0..c).map(move |k| (k, j)))
                .find(|&(k, j)| i.element_trans(k, j).apply_rect(b).contains(p.0, p.1))
                .unwrap_or((0, 0)),
            _ => (0, 0),
        };
        let trans = i.element_trans(elem.0, elem.1);
        self.requests.push(LayRequest::Descend { cell: i.cell.clone(), inst: i.id, elem, trans });
    }

    /// Box entry for box tools and zoom: drag it, or click two corners.
    fn box_input(&mut self, ui: &egui::Ui, resp: &egui::Response, rect: Rect, l: &mut Layout, cx: &mut LayCtx) -> bool {
        let mut changed = false;
        if resp.drag_started_by(egui::PointerButton::Primary) {
            let press = ui.input(|i| i.pointer.press_origin()).unwrap_or(rect.center());
            let snapped = self.snapped;
            let s = self.pick_point(l, cx, self.view.to_world(rect, press));
            self.snapped = snapped;
            self.drag = Some(LDrag::Create { start: s });
            self.box_start = None;
        }
        if resp.clicked()
            && let Some(c) = self.cursor
        {
            if self.tool == LayTool::Via && !self.zoom_box {
                changed |= self.place_via(l, cx.tech, c, None);
            } else {
                match self.box_start.take() {
                    Some(a) => changed |= self.finish_box(l, cx, rect, a, c),
                    None => self.box_start = Some(c),
                }
            }
        }
        changed
    }

    fn finish_box(&mut self, l: &mut Layout, cx: &mut LayCtx, rect: Rect, a: (i64, i64), b: (i64, i64)) -> bool {
        let r = IRect::new(a.0, a.1, b.0, b.1);
        if self.zoom_box {
            self.zoom_box = false;
            self.zoom_box_to(rect, r);
            return false;
        }
        let flat = r.width() == 0 || r.height() == 0;
        match self.tool {
            LayTool::Ruler => {
                if a != b {
                    self.rulers.push((a, b));
                }
                false
            }
            _ if flat => false,
            LayTool::Rect => {
                self.history.push(l);
                let id = l.add_rect(self.active_layer, r.x0, r.y0, r.x1, r.y1);
                self.selection = [id].into();
                true
            }
            LayTool::Via => self.place_via(l, cx.tech, b, Some(r)),
            LayTool::Chop => self.chop(l, cx, r),
            _ => false,
        }
    }

    /// First click of move / copy: pick the object under the point if nothing is selected.
    fn begin_xfer(&mut self, l: &Layout, cx: &mut LayCtx, p: (i64, i64), tol: i64) {
        if self.selection.is_empty() {
            let idx = cx.index(l);
            if let Some(id) = hits(l, &idx, &self.pickable(), self.inst_selectable, p.0, p.1, tol).first() {
                self.selection = [*id].into();
            }
        }
        if self.selection.is_empty() {
            self.note(false, format!("{}: nothing selected under the cursor", self.tool.name()));
            return;
        }
        self.xfer = Some(p);
    }

    /// Transform of the move / copy in progress that takes the reference point to `to`.
    fn xfer_trans(&self, to: (i64, i64)) -> Option<Trans> {
        let from = self.xfer?;
        let r = self.xfer_t.apply(from);
        Some(Trans::with(to.0 - r.0, to.1 - r.1, self.xfer_t.rot, self.xfer_t.mirror))
    }

    fn finish_xfer(&mut self, l: &mut Layout, to: (i64, i64)) -> bool {
        let Some(t) = self.xfer_trans(to) else { return false };
        if t.is_identity() {
            return false;
        }
        self.history.push(l);
        if self.tool == LayTool::Copy {
            let ids = self.copy_by(l, (0, 0));
            l.transform_items(&ids, &t);
            self.note(true, format!("Copied {} item(s)", ids.len()));
        } else {
            l.transform_items(&self.selection, &t);
            self.xfer = None;
            self.xfer_t = Trans::IDENTITY;
        }
        true
    }

    /// First click of stretch: grab the nearest edge or vertex of the shape under the point.
    fn begin_stretch(&mut self, l: &Layout, cx: &mut LayCtx, p: (i64, i64), tol: i64) {
        let idx = cx.index(l);
        let near = |s: &Shape| {
            let b = s.bbox();
            IRect::new(b.x0 - 3 * tol, b.y0 - 3 * tol, b.x1 + 3 * tol, b.y1 + 3 * tol).contains(p.0, p.1)
        };
        let sel = (self.selection.len() == 1).then(|| l.shape(*self.selection.first().unwrap())).flatten().filter(|s| near(s));
        let shape =
            sel.or_else(|| hits(l, &idx, &self.pickable(), false, p.0, p.1, 3 * tol).into_iter().find_map(|id| l.shape(id)));
        let Some((s, h)) = shape.and_then(|s| edit::nearest_handle(&s.geom, p, 3 * tol).map(|h| (s, h))) else {
            self.note(false, "Stretch: no shape edge under the cursor");
            return;
        };
        self.selection = [s.id].into();
        self.stretch = Some((s.id, h, p, s.geom.clone()));
    }

    fn finish_stretch(&mut self, l: &mut Layout, to: (i64, i64)) -> bool {
        let Some((id, h, from, orig)) = self.stretch.take() else { return false };
        let cur = edit::drag_handle(&orig, h, (to.0 - from.0, to.1 - from.1));
        let Some(s) = l.shape(id).cloned().filter(|_| cur != orig) else { return false };
        self.history.push(l);
        if let Some(t) = l.shape_mut(id) {
            *t = edit::with_geom(&s, cur);
        }
        true
    }

    /// Outlines of the selection displaced by `d` (move / copy preview), with spacing feedback.
    #[allow(clippy::too_many_arguments)]
    fn draw_ghosts(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        l: &Layout,
        cx: &mut LayCtx,
        pal: &Palette,
        t: Trans,
        moving: bool,
    ) {
        let ghost = Stroke::new(1.5, pal.selection.gamma_multiply(0.9));
        for (_, b) in self.item_boxes(l, cx) {
            painter.rect_stroke(screen_rect(&self.view, rect, t.apply_rect(b)), 0.0, ghost, StrokeKind::Middle);
        }
        if self.drd {
            let mut cands: Vec<(usize, IRect)> = l
                .shapes
                .iter()
                .filter(|s| self.selection.contains(&s.id))
                .flat_map(|s| assist::shape_rects(&s.geom).into_iter().map(move |r| (s.layer, t.apply_rect(r))))
                .collect();
            for inst in l.instances.iter().filter(|i| self.selection.contains(&i.id)) {
                assist::inst_cands(cx, inst, t, &mut cands);
            }
            let skip = if moving { self.selection.clone() } else { BTreeSet::new() };
            assist::draw_drd(painter, &self.view, rect, pal, &assist::drd_marks(l, cx, &cands, &skip, false));
        }
    }

    /// Edit-in-place surroundings: the root cell drawn dimmed in the edited cell's frame, with
    /// the edited placement outlined.
    fn draw_context(&self, painter: &egui::Painter, rect: Rect, l: &Layout, cx: &mut LayCtx, ctx: &EipContext, pal: &Palette) {
        let key = CellRef::new("<in-place>", &ctx.root.cell);
        let mut wrap = Layout::default();
        wrap.add_instance(ctx.root.clone(), ctx.to_root.inverse(), None);
        let live = Live { src: cx.src, cell: cx.cell, layout: l };
        cx.hier.invalidate(&key);
        draw_layout(
            painter,
            &self.view,
            rect,
            &key,
            &wrap,
            &live,
            cx.hier,
            cx.tech,
            &LayoutDrawOpts {
                pal: *pal,
                visible: &self.visible,
                selection: None,
                thumbnail: false,
                show_grid: self.show_grid,
                grid: self.snap_step(cx.tech),
                max_depth: self.max_depth.saturating_add(1),
            },
        );
        cx.hier.invalidate(&key);
        painter.rect_filled(rect, 0.0, canvas_bg(pal).gamma_multiply(0.7));
        if let Some(b) = cx.index(l).bbox {
            assist::dashed_rect(painter, screen_rect(&self.view, rect, b).expand(2.0), Stroke::new(1.5, pal.accent));
        }
    }

    pub fn canvas(&mut self, ui: &mut egui::Ui, l: &mut Layout, cx: &mut LayCtx) -> LayOutput {
        let tech = cx.tech;
        self.sync_layers(tech);
        let pal = theme::palette(ui.ctx());
        let mut out = LayOutput::default();
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = resp.rect;
        let painter = painter.with_clip_rect(rect);
        painter.rect_filled(rect, 0.0, canvas_bg(&pal));
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
        if resp.hovered() && self.handle_keys(ui, l, cx, rect) {
            out.changed = true;
            cx.touched();
        }
        let hover = resp.hover_pos();
        let world = hover.map(|p| self.view.to_world(rect, p));
        self.snapped = None;
        self.cursor = world.map(|w| self.pick_point(l, cx, w));
        let tol = (5.0 / self.view.scale) as i64;
        let shift = ui.input(|i| i.modifiers.shift || i.modifiers.command);
        let pick = self.pickable();
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

        let press = || ui.input(|i| i.pointer.press_origin()).unwrap_or(rect.center());
        if self.paste.is_some() {
            if resp.clicked()
                && let Some(c) = self.cursor
                && self.drop_paste(l, c, cx.src)
            {
                out.changed = true;
                cx.touched();
            }
        } else if self.zoom_box || self.tool.takes_box() {
            if self.box_input(ui, &resp, rect, l, cx) {
                out.changed = true;
                cx.touched();
            }
        } else {
            match self.tool {
                LayTool::Select => {
                    if resp.drag_started_by(egui::PointerButton::Primary) {
                        let press = press();
                        let pw = self.view.to_world(rect, press);
                        let idx = cx.index(l);
                        let on_handle = handle_shape
                            .as_ref()
                            .and_then(|s| edit::hit_handle(&s.geom, &self.view, rect, press).map(|h| (s, h)));
                        if let Some((s, h)) = on_handle {
                            self.drag = Some(LDrag::Handle {
                                id: s.id,
                                handle: h,
                                start: self.snap(pw, self.snap_step(tech)),
                                orig: s.geom.clone(),
                                cur: s.geom.clone(),
                            });
                        } else {
                            match hits(l, &idx, &pick, self.inst_selectable, pw.0 as i64, pw.1 as i64, tol).first() {
                                Some(&id) => {
                                    if !self.selection.contains(&id) {
                                        if !shift {
                                            self.selection.clear();
                                        }
                                        self.selection.insert(id);
                                    }
                                    self.drag = Some(LDrag::Move { start: self.snap(pw, self.snap_step(tech)), offset: (0, 0) });
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
                        let p = (w.0 as i64, w.1 as i64);
                        let list = hits(l, &idx, &pick, self.inst_selectable, p.0, p.1, tol);
                        if resp.double_clicked() {
                            if let Some(i) = list.iter().find_map(|id| l.instance(*id)) {
                                self.requests.push(LayRequest::Open(i.cell.clone()));
                            }
                        } else {
                            // Clicking the same spot again cycles through the overlapping items.
                            let near = |q: (i64, i64)| (q.0 - p.0).abs() <= tol && (q.1 - p.1).abs() <= tol;
                            let n = match self.cycle {
                                Some((q, k)) if !shift && near(q) && list.len() > 1 => (k + 1) % list.len(),
                                _ => 0,
                            };
                            let h = list.get(n).copied();
                            self.cycle = h.map(|_| (p, n));
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
                LayTool::Stretch => {
                    if resp.drag_started_by(egui::PointerButton::Primary) && self.stretch.is_none() {
                        let snapped = self.snapped;
                        let p = self.pick_point(l, cx, self.view.to_world(rect, press()));
                        self.snapped = snapped;
                        self.begin_stretch(l, cx, p, tol);
                        if self.stretch.is_some() {
                            self.drag = Some(LDrag::Xfer);
                        }
                    }
                    if resp.clicked()
                        && let Some(c) = self.cursor
                    {
                        if self.stretch.is_some() {
                            if self.finish_stretch(l, c) {
                                out.changed = true;
                                cx.touched();
                            }
                        } else {
                            self.begin_stretch(l, cx, c, tol);
                        }
                    }
                }
                LayTool::Move | LayTool::Copy => {
                    // Virtuoso: right-click rotates what is being moved, Shift+right-click flips it sideways.
                    if resp.secondary_clicked() && self.xfer.is_some() {
                        let k = if shift { Trans::flip_x_about((0, 0)) } else { Trans::rotate_about((0, 0)) };
                        self.xfer_t = self.on_screen_linear(k).compose(&self.xfer_t);
                    }
                    if resp.drag_started_by(egui::PointerButton::Primary) && self.xfer.is_none() {
                        let snapped = self.snapped;
                        let p = self.pick_point(l, cx, self.view.to_world(rect, press()));
                        self.snapped = snapped;
                        self.begin_xfer(l, cx, p, tol);
                        if self.xfer.is_some() {
                            self.drag = Some(LDrag::Xfer);
                        }
                    }
                    if resp.clicked()
                        && let Some(c) = self.cursor
                    {
                        if self.xfer.is_some() {
                            if self.finish_xfer(l, c) {
                                out.changed = true;
                                cx.touched();
                            }
                        } else {
                            self.begin_xfer(l, cx, c, tol);
                        }
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
                LayTool::Rect | LayTool::Via | LayTool::Chop | LayTool::Ruler => {}
            }
        }

        // Drag updates
        if let Some(c) = self.cursor {
            match &mut self.drag {
                Some(LDrag::Move { start, offset }) => *offset = (c.0 - start.0, c.1 - start.1),
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
                        && self.finish_box(l, cx, rect, start, c)
                    {
                        out.changed = true;
                        cx.touched();
                    }
                }
                Some(LDrag::Xfer) => {
                    let done = match self.cursor {
                        Some(c) if self.tool == LayTool::Stretch => self.finish_stretch(l, c),
                        Some(c) => self.finish_xfer(l, c),
                        None => false,
                    };
                    if done {
                        out.changed = true;
                        cx.touched();
                    }
                    self.xfer = None;
                    self.xfer_t = Trans::IDENTITY;
                    self.stretch = None;
                }
                Some(LDrag::Rubber { start }) => {
                    if let Some(end) = hover {
                        let a = self.view.to_world(rect, start);
                        let b = self.view.to_world(rect, end);
                        let r = IRect::new(a.0 as i64, a.1 as i64, b.0 as i64, b.1 as i64);
                        if !shift {
                            self.selection.clear();
                        }
                        // Right-to-left boxes select everything they touch, left-to-right only what they enclose.
                        let crossing = end.x < start.x;
                        let inside = |bb: IRect| bb.x0 >= r.x0 && bb.x1 <= r.x1 && bb.y0 >= r.y0 && bb.y1 <= r.y1;
                        let take = |bb: IRect| inside(bb) || (crossing && bb.intersects(&r));
                        let idx = cx.index(l);
                        for (li, ok) in pick.iter().enumerate() {
                            if *ok {
                                for i in idx.shapes_in(li, r) {
                                    if let Some(s) = l.shapes.get(i)
                                        && take(s.bbox())
                                    {
                                        self.selection.insert(s.id);
                                    }
                                }
                            }
                        }
                        if self.inst_selectable {
                            for i in idx.instances_in(r) {
                                if let (Some(b), Some(inst)) = (idx.instance_bbox(l, i), l.instances.get(i))
                                    && take(b)
                                {
                                    self.selection.insert(inst.id);
                                }
                            }
                        }
                    }
                }
                None => {}
            }
        }

        if let Some(ctx) = cx.context {
            self.draw_context(&painter, rect, l, cx, ctx, &pal);
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
                show_grid: self.show_grid && cx.context.is_none(),
                grid: self.snap_step(tech),
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
            self.draw_ghosts(&painter, rect, l, cx, &pal, Trans::new(offset.0, offset.1), true);
        }
        // Move / copy tool: rubber line from the reference point and the displaced selection.
        if let (Some(from), Some(c)) = (self.xfer, self.cursor)
            && let Some(t) = self.xfer_trans(c)
            && hover.is_some()
        {
            let d = (c.0 - from.0, c.1 - from.1);
            let a = self.view.to_screen(rect, from.0 as f64, from.1 as f64);
            let b = self.view.to_screen(rect, c.0 as f64, c.1 as f64);
            painter.add(egui::Shape::dashed_line(&[a, b], Stroke::new(1.0, pal.selection), 4.0, 3.0));
            painter.circle_stroke(a, 3.5, Stroke::new(1.5, pal.selection));
            self.draw_ghosts(&painter, rect, l, cx, &pal, t, self.tool == LayTool::Move);
            let txt = format!("dx {}  dy {} µm", um(d.0, tech), um(d.1, tech));
            painter.text(b + Vec2::new(10.0, 10.0), Align2::LEFT_TOP, txt, FontId::monospace(11.0), pal.text);
        }
        // Stretch preview: the reshaped outline with its spacing / width feedback.
        if let (Some((id, h, from, orig)), Some(c)) = (&self.stretch, self.cursor) {
            let cur = edit::drag_handle(orig, *h, (c.0 - from.0, c.1 - from.1));
            draw_outline(&painter, &self.view, rect, &cur, Stroke::new(1.5, pal.warning));
            if self.drd
                && let Some(layer) = l.shape(*id).map(|s| s.layer)
            {
                let cands: Vec<(usize, IRect)> = assist::shape_rects(&cur).into_iter().map(|r| (layer, r)).collect();
                let marks = assist::drd_marks(l, cx, &cands, &[*id].into(), true);
                assist::draw_drd(&painter, &self.view, rect, &pal, &marks);
            }
        }
        // Paste preview.
        if let (Some(clip), Some(c)) = (&self.paste, self.cursor)
            && hover.is_some()
        {
            let ghost = Stroke::new(1.5, pal.selection.gamma_multiply(0.9));
            for b in clip.shapes.iter().map(|s| s.bbox()).chain(clip.inst_boxes.iter().copied()) {
                painter.rect_stroke(screen_rect(&self.view, rect, b.translate(c.0, c.1)), 0.0, ghost, StrokeKind::Middle);
            }
            if self.drd {
                let mut cands: Vec<(usize, IRect)> = clip
                    .shapes
                    .iter()
                    .flat_map(|s| assist::shape_rects(&s.geom).into_iter().map(move |r| (s.layer, r.translate(c.0, c.1))))
                    .collect();
                for inst in &clip.instances {
                    assist::inst_cands(cx, inst, Trans::new(c.0, c.1), &mut cands);
                }
                let marks = assist::drd_marks(l, cx, &cands, &BTreeSet::new(), false);
                assist::draw_drd(&painter, &self.view, rect, &pal, &marks);
            }
        }
        // Vertex / edge handles and the shape being reshaped.
        if let Some(LDrag::Handle { cur, .. }) = &self.drag {
            draw_outline(&painter, &self.view, rect, cur, Stroke::new(1.5, pal.warning));
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
            edit::draw_polygon_preview(&painter, &pts, layer_color(&tech.layers[self.active_layer], pal.dark));
        }
        // Box previews (rectangle, via area, chop, zoom, ruler).
        let def = &tech.layers[self.active_layer];
        let box_from = match &self.drag {
            Some(LDrag::Create { start }) => Some(*start),
            _ => self.box_start,
        };
        if let (Some(start), Some(c)) = (box_from, self.cursor) {
            let r = Rect::from_two_pos(
                self.view.to_screen(rect, start.0 as f64, start.1 as f64),
                self.view.to_screen(rect, c.0 as f64, c.1 as f64),
            );
            let dims = format!("{} × {} µm", um((c.0 - start.0).abs(), tech), um((c.1 - start.1).abs(), tech));
            let dims_at = r.right_bottom() + Vec2::new(6.0, 6.0);
            if self.zoom_box {
                assist::dashed_rect(&painter, r, Stroke::new(1.5, pal.accent));
            } else {
                match self.tool {
                    LayTool::Rect => {
                        pattern_rect(&painter, r, def, 1.0);
                        if self.drd {
                            let w = IRect::new(start.0, start.1, c.0, c.1);
                            let marks = assist::drd_marks(l, cx, &[(self.active_layer, w)], &BTreeSet::new(), true);
                            let sp = def.min_spacing;
                            if sp > 0 && w.width() > 0 && w.height() > 0 {
                                let halo = IRect::new(w.x0 - sp, w.y0 - sp, w.x1 + sp, w.y1 + sp);
                                assist::dashed_rect(
                                    &painter,
                                    screen_rect(&self.view, rect, halo),
                                    Stroke::new(1.0, layer_color(def, pal.dark).gamma_multiply(0.6)),
                                );
                            }
                            assist::draw_drd(&painter, &self.view, rect, &pal, &marks);
                        }
                        painter.text(dims_at, Align2::LEFT_TOP, dims, FontId::monospace(11.0), pal.text);
                    }
                    LayTool::Via => {
                        assist::dashed_rect(&painter, r, Stroke::new(1.5, layer_color(def, pal.dark)));
                        painter.text(dims_at, Align2::LEFT_TOP, dims, FontId::monospace(11.0), pal.text);
                    }
                    LayTool::Chop => {
                        painter.rect_filled(r, 0.0, pal.danger.gamma_multiply(0.12));
                        assist::dashed_rect(&painter, r, Stroke::new(1.5, pal.danger));
                        painter.text(dims_at, Align2::LEFT_TOP, dims, FontId::monospace(11.0), pal.text);
                    }
                    LayTool::Ruler => draw_ruler(&painter, &self.view, rect, &pal, tech, start, c),
                    _ => {}
                }
            }
        }
        for (a, b) in &self.rulers {
            draw_ruler(&painter, &self.view, rect, &pal, tech, *a, *b);
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
            if h.x < start.x {
                assist::dashed_rect(&painter, r, Stroke::new(1.0, pal.selection));
            } else {
                painter.rect_stroke(r, 0.0, Stroke::new(1.0, pal.selection), StrokeKind::Inside);
            }
        }
        if !self.path_pts.is_empty() {
            let mut pts = self.path_pts.clone();
            if let Some(c) = self.cursor {
                pts.push(manhattan(*pts.last().unwrap(), c));
            }
            let w = self.wire_width(tech);
            for q in path_quads(&pts, w) {
                let sp: Vec<Pos2> = q.iter().map(|p| self.view.to_screen(rect, p.0, p.1)).collect();
                painter.add(egui::Shape::convex_polygon(
                    sp,
                    layer_color(def, pal.dark).gamma_multiply(0.3),
                    Stroke::new(1.0, layer_color(def, pal.dark)),
                ));
            }
            if self.drd {
                let cands: Vec<(usize, IRect)> =
                    Geometry::Path { points: pts, width: w }.rects().into_iter().map(|r| (self.active_layer, r)).collect();
                let marks = assist::drd_marks(l, cx, &cands, &BTreeSet::new(), false);
                assist::draw_drd(&painter, &self.view, rect, &pal, &marks);
            }
        }
        // Gravity marker: a diamond on a vertex, a circle on an edge.
        if let (Some(vertex), Some(c), Some(_)) = (self.snapped, self.cursor, hover) {
            let p = self.view.to_screen(rect, c.0 as f64, c.1 as f64);
            let st = Stroke::new(1.5, pal.accent);
            if vertex {
                let d = 6.0;
                let pts = vec![p + Vec2::new(0.0, -d), p + Vec2::new(d, 0.0), p + Vec2::new(0.0, d), p + Vec2::new(-d, 0.0)];
                painter.add(egui::Shape::closed_line(pts, st));
            } else {
                painter.circle_stroke(p, 4.5, st);
            }
        }
        if let Some(text) = self.prompt() {
            let g = painter.layout_no_wrap(text, FontId::proportional(12.0), pal.text);
            let r = Rect::from_min_size(rect.min + Vec2::new(10.0, 10.0), g.size() + Vec2::new(14.0, 8.0));
            painter.rect_filled(r, 4.0, pal.bg_panel.gamma_multiply(0.92));
            painter.rect_stroke(r, 4.0, Stroke::new(1.0, pal.layout.gamma_multiply(0.6)), StrokeKind::Inside);
            painter.galley(r.min + Vec2::new(7.0, 4.0), g, pal.text);
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
            ui.ctx().set_cursor_icon(if self.tool == LayTool::Select && self.paste.is_none() && !self.zoom_box {
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
        theme::section(ui, &format!("Layers · {}", tech.name));
        ui.horizontal(|ui| {
            let all = |ui: &mut egui::Ui, text: &str, tip: &str, v: &mut [bool], on: bool| {
                if ui.small_button(text).on_hover_text(tip).clicked() {
                    v.iter_mut().for_each(|x| *x = on);
                }
            };
            all(ui, "AV", "All layers visible", &mut self.visible, true);
            all(ui, "NV", "No layer visible", &mut self.visible, false);
            all(ui, "AS", "All layers selectable", &mut self.selectable, true);
            all(ui, "NS", "No layer selectable", &mut self.selectable, false);
        });
        ui.horizontal(|ui| {
            let fl = ui.label("Filter");
            ui.add(egui::TextEdit::singleline(&mut self.layer_filter).hint_text("layer name").desired_width(f32::INFINITY))
                .labelled_by(fl.id);
        });
        ui.checkbox(&mut self.inst_selectable, "Instances selectable");
        let filter = self.layer_filter.trim().to_lowercase();
        egui::ScrollArea::vertical().id_salt("layers").show(ui, |ui| {
            for (i, def) in tech.layers.iter().enumerate() {
                if !filter.is_empty() && !def.name.to_lowercase().contains(&filter) {
                    continue;
                }
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
                    r.rect.right_center() - Vec2::new(52.0, 0.0),
                    Align2::RIGHT_CENTER,
                    format!("{}/{}", def.gds_layer, def.gds_datatype),
                    FontId::monospace(10.5),
                    pal.text_weak,
                );
                // Selectable toggle: a filled box when shapes on the layer can be picked.
                let sel = Rect::from_center_size(r.rect.right_center() - Vec2::new(36.0, 0.0), Vec2::splat(18.0));
                let sel_resp = ui
                    .interact(sel, ui.scope_id().with(("selectable", i)), Sense::click())
                    .on_hover_text("Selectable: shapes on this layer can be picked");
                let b = Rect::from_center_size(sel.center(), Vec2::splat(9.0));
                let col = if sel_resp.hovered() { pal.text } else { pal.text_weak };
                if self.selectable[i] {
                    painter.rect_filled(b, 2.0, col);
                } else {
                    painter.rect_stroke(b, 2.0, Stroke::new(1.2, col), StrokeKind::Inside);
                }
                if sel_resp.clicked() {
                    self.selectable[i] = !self.selectable[i];
                }
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
                } else if r.clicked() && !sel_resp.clicked() {
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
    let layout = layout.or_else(|| src.layout(cell));
    // Children are grouped only when the node is expanded: cells can hold many thousands of instances.
    let kids = |l: &Layout| -> Vec<(CellRef, u64)> {
        let mut order: Vec<&CellRef> = vec![];
        let mut counts: std::collections::HashMap<&CellRef, u64> = std::collections::HashMap::new();
        for inst in &l.instances {
            let n = counts.entry(&inst.cell).or_insert_with(|| {
                order.push(&inst.cell);
                0
            });
            *n += inst.count();
        }
        order.into_iter().map(|c| (c.clone(), counts[c])).collect()
    };
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
    let Some(l) = layout.filter(|l| !l.instances.is_empty() && depth < 12) else {
        ui.horizontal(|ui| {
            ui.add_space(18.0);
            header(ui, open);
        });
        return;
    };
    let id = ui.make_persistent_id(("hier", cell, depth));
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, depth == 0)
        .show_header(ui, |ui| header(ui, open))
        .body(|ui| {
            for (c, n) in kids(l).into_iter().take(500) {
                hier_node(ui, src, &c, None, Some(n), depth + 1, open);
            }
        });
}

/// Outline of a geometry (reshape previews).
fn draw_outline(painter: &egui::Painter, view: &View2D, rect: Rect, g: &Geometry, st: Stroke) {
    match g {
        Geometry::Polygon { points } => {
            let pts: Vec<Pos2> = points.iter().map(|p| view.to_screen(rect, p.0 as f64, p.1 as f64)).collect();
            painter.add(egui::Shape::closed_line(pts, st));
        }
        Geometry::Path { points, width } => {
            for q in path_quads(points, *width) {
                let sp: Vec<Pos2> = q.iter().map(|p| view.to_screen(rect, p.0, p.1)).collect();
                painter.add(egui::Shape::closed_line(sp, st));
            }
        }
        _ => {
            painter.rect_stroke(screen_rect(view, rect, g.bbox()), 0.0, st, StrokeKind::Middle);
        }
    }
}

/// A measurement line with its length and dx / dy.
fn draw_ruler(painter: &egui::Painter, view: &View2D, rect: Rect, pal: &Palette, tech: &Tech, a: (i64, i64), b: (i64, i64)) {
    let pa = view.to_screen(rect, a.0 as f64, a.1 as f64);
    let pb = view.to_screen(rect, b.0 as f64, b.1 as f64);
    let st = Stroke::new(1.5, pal.warning);
    painter.line_segment([pa, pb], st);
    let n = (pb - pa).normalized().rot90() * 5.0;
    for p in [pa, pb] {
        painter.line_segment([p - n, p + n], st);
    }
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let d = ((dx as f64).powi(2) + (dy as f64).powi(2)).sqrt().round() as i64;
    let txt = if dx == 0 || dy == 0 {
        format!("{} µm", um(d, tech))
    } else {
        format!("{} µm  (dx {}, dy {})", um(d, tech), um(dx, tech), um(dy, tech))
    };
    let mid = pa + (pb - pa) * 0.5;
    let g = painter.layout_no_wrap(txt, FontId::monospace(12.0), pal.warning);
    let r = Rect::from_min_size(mid + Vec2::new(8.0, -24.0), g.size() + Vec2::new(10.0, 4.0));
    painter.rect_filled(r, 4.0, pal.bg_panel.gamma_multiply(0.92));
    painter.galley(r.min + Vec2::new(5.0, 2.0), g, pal.warning);
}

pub(super) fn manhattan(from: (i64, i64), to: (i64, i64)) -> (i64, i64) {
    if (to.0 - from.0).abs() >= (to.1 - from.1).abs() { (to.0, from.1) } else { (from.0, to.1) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Editing a rotated or mirrored placement in place, nudges and turns follow the screen.
    #[test]
    fn edits_follow_the_screen_in_every_orientation() {
        for (rot, mirror) in (0..4).flat_map(|r| [(r, false), (r, true)]) {
            let o = Trans::with(700, -300, rot, mirror);
            let mut ed = LayoutEditor::default();
            ed.view.orient = o;
            let mut l = Layout::default();
            let id = l.add_rect(0, 0, 0, 100, 40);
            let before = o.apply_rect(l.shape(id).unwrap().bbox());
            ed.selection.insert(id);
            assert!(ed.nudge(&mut l, 50, 0));
            assert_eq!(o.apply_rect(l.shape(id).unwrap().bbox()), before.translate(50, 0), "rot {rot} mirror {mirror}");

            let (p, q) = ((30, 20), (80, 20));
            let k = Trans::rotate_about(o.apply(p));
            assert_eq!(o.apply(ed.on_screen(k).apply(q)), k.apply(o.apply(q)), "rot {rot} mirror {mirror}");
            let turn = ed.on_screen_linear(Trans::rotate_about((0, 0)));
            let d = o.apply_vec((10, 0));
            assert_eq!(o.apply_vec(turn.apply_vec((10, 0))), (-d.1, d.0), "turn is counter-clockwise on screen");
        }
    }

    /// DRD sees shapes inside placed cells, and while editing in place those of the
    /// surroundings, but not the edited placement's own stale copy.
    #[test]
    fn drd_checks_the_hierarchy_and_the_edit_in_place_context() {
        let mut p = Project::new("t");
        let tech = p.tech.clone();
        let m = tech.layers.iter().position(|d| d.min_spacing > 0).unwrap();
        let sp = tech.layers[m].min_spacing;
        let mut leaf = ics_core::Cell::new("leaf");
        let mut ll = Layout::default();
        ll.add_rect(m, 0, 0, 1000, 1000);
        leaf.layout = Some(ll);
        let leaf = p.add_cell("work", leaf).unwrap();
        let mut top = ics_core::Cell::new("top");
        let mut tl = Layout::default();
        let inst = tl.add_instance(leaf.clone(), Trans::new(5000, 0), None);
        tl.add_rect(m, 6000 + sp / 2, 0, 7000, 1000);
        top.layout = Some(tl.clone());
        let top = p.add_cell("work", top).unwrap();
        let mut hier = Hier::default();

        // In `top`: a box just left of the placed leaf is too close to its metal.
        let cand = [(m, IRect::new(3000, 0, 5000 - sp / 2, 1000))];
        let none = BTreeSet::new();
        let mut cx = LayCtx { cell: &top, src: &p, hier: &mut hier, tech: &tech, cells: &[], context: None };
        assert!(!assist::drd_marks(&tl, &mut cx, &cand, &none, false).is_empty(), "shape inside an instance");
        assert!(assist::drd_marks(&tl, &mut cx, &cand, &[inst].into(), false).is_empty(), "moved instance skipped");

        // Editing `leaf` in place: a box on its right edge comes close to the top-level shape.
        let ll = p.cell_mut(&leaf).unwrap().layout.take().unwrap();
        let ctx = EipContext { root: top.clone(), to_root: Trans::new(5000, 0), path: vec![(inst, (0, 0))] };
        let mut cx = LayCtx { cell: &leaf, src: &p, hier: &mut hier, tech: &tech, cells: &[], context: Some(&ctx) };
        let near_top = [(m, IRect::new(500, 0, 1000, 1000))];
        assert!(!assist::drd_marks(&ll, &mut cx, &near_top, &none, false).is_empty(), "context shape");
        let inside = [(m, IRect::new(-2000, 0, -sp / 2, 1000))];
        let marks = assist::drd_marks(&ll, &mut cx, &inside, &[ll.shapes[0].id].into(), false);
        assert!(marks.is_empty(), "the edited placement itself is not context: {marks:?}");
    }

    #[test]
    fn move_with_rotation_maps_reference_point_to_destination() {
        let mut ed = LayoutEditor { xfer: Some((100, 0)), ..Default::default() };
        ed.xfer_t = Trans::rotate_about((0, 0));
        let t = ed.xfer_trans((200, 0)).unwrap();
        assert_eq!(t.apply((100, 0)), (200, 0));
        assert_eq!(t.apply((110, 0)), (200, 10), "rotated 90° about the reference point");
    }

    #[test]
    fn raster_runs_cover_exactly_the_marked_cells() {
        let mut r = Raster::new(Rect::from_min_size(Pos2::ZERO, Vec2::new(40.0, 40.0)), 1, Raster::CELL);
        r.mark(0, Rect::from_min_max(Pos2::new(2.0, 2.0), Pos2::new(3.0, 30.0)));
        r.mark(0, Rect::from_min_max(Pos2::new(10.0, 4.0), Pos2::new(20.0, 6.0)));
        r.mark(0, Rect::from_min_max(Pos2::new(10.0, 8.0), Pos2::new(14.0, 9.0)));
        let marked: usize = r.planes[0].iter().map(|w| w.count_ones() as usize).sum();
        let mut mesh = Mesh::default();
        r.emit(&mut mesh, &[(0, Color32::WHITE)]);
        let area: f32 = mesh.vertices.chunks(4).map(|q| (q[3].pos.x - q[0].pos.x) * (q[3].pos.y - q[0].pos.y)).sum();
        assert_eq!(area, marked as f32 * Raster::CELL * Raster::CELL);
        assert!(mesh.vertices.len() / 4 <= 4, "the vertical bar must merge into one quad: {}", mesh.vertices.len() / 4);
    }

    /// Every set sprite pixel lands on the raster cell holding its transformed centre, for all
    /// orientations, with and without the axis-aligned fast path.
    #[test]
    fn stamped_sprites_land_on_their_pixel_centres() {
        let mut sp = Sprite::new(IRect::new(0, 0, 160, 96), 4);
        sp.mark(0, PLANE_SMALL, IRect::new(0, 0, 160, 16));
        sp.mark(0, PLANE_SMALL, IRect::new(0, 0, 16, 96));
        sp.mark(0, PLANE_SMALL, IRect::new(64, 48, 80, 64));
        let rect = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(200.0, 160.0));
        let mut view = View2D::new(Raster::CELL as f64 / 16.0, true, 1e-6, 1e6);
        view.center = (3.0, -5.0);
        let bits = sp.plane(0, PLANE_SMALL).unwrap();
        for (rot, mirror) in (0..4).flat_map(|r| [(r, false), (r, true)]) {
            let t = Trans::with(37, 21, rot, mirror);
            let mut got = Raster::new(rect, 1, Raster::CELL);
            got.stamp(0, &sp, bits, &t, &view, rect);
            let mut want = Raster::new(rect, 1, Raster::CELL);
            for j in 0..sp.h {
                for i in (0..sp.w).filter(|&i| sp.get(0, PLANE_SMALL, i, j)) {
                    let (x, y) = t.apply_f(((i as f64 + 0.5) * 16.0, (j as f64 + 0.5) * 16.0));
                    let s = view.to_screen(rect, x, y);
                    let c = (((s.x - rect.min.x) / Raster::CELL) as f64, ((s.y - rect.min.y) / Raster::CELL) as f64);
                    want.fill(0, c, c);
                }
            }
            assert_eq!(got.planes, want.planes, "rot {rot} mirror {mirror}");
        }
    }
}
