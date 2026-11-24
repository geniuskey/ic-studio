//! Schematic workspace: canvas, editing tools, device palette and property inspector.

use std::collections::{BTreeSet, HashMap};

use eframe::egui::{self, Align2, Color32, FontId, Key, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use ics_core::connectivity::{Connectivity, extract};
use ics_core::geom::{Orient, Pt};
use ics_core::schematic::{DeviceKind, Instance, PortDir, Prim, SchObj, Schematic};
use ics_core::units::format_eng;

use crate::canvas::View2D;
use crate::history::History;
use crate::i18n::{t, tf};
use crate::theme::{self, Palette};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SchTool {
    Select,
    Wire,
    Place(DeviceKind),
    Label,
}

impl SchTool {
    pub fn name(self) -> String {
        match self {
            SchTool::Select => "Select".into(),
            SchTool::Wire => "Wire".into(),
            SchTool::Place(k) => format!("Place {}", k.label()),
            SchTool::Label => "Net Label".into(),
        }
    }
}

/// A named schematic editor action, shared by the canvas pop-up menu, the command palette
/// and `Command::Sch` (automation).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SchAction {
    Tool(SchTool),
    Undo,
    Redo,
    Delete,
    Rotate,
    Mirror,
    Duplicate,
    SelectAll,
    DeselectAll,
    /// Select every wire and label of the selected wires' / labels' nets.
    SelectNet,
    Fit,
    ToggleGrid,
    ToggleWireBend,
}

impl SchAction {
    pub const PALETTE: &[SchAction] = &[
        SchAction::Tool(SchTool::Select),
        SchAction::Tool(SchTool::Wire),
        SchAction::Tool(SchTool::Label),
        SchAction::Rotate,
        SchAction::Mirror,
        SchAction::Duplicate,
        SchAction::Delete,
        SchAction::SelectAll,
        SchAction::DeselectAll,
        SchAction::SelectNet,
        SchAction::Fit,
        SchAction::ToggleGrid,
        SchAction::ToggleWireBend,
    ];

    pub fn name(self) -> String {
        match self {
            SchAction::Tool(SchTool::Select) => "Select Tool".into(),
            SchAction::Tool(SchTool::Wire) => "Draw Wire".into(),
            SchAction::Tool(SchTool::Label) => "Place Net Label".into(),
            SchAction::Tool(SchTool::Place(k)) => k.label().into(),
            SchAction::Undo => "Undo".into(),
            SchAction::Redo => "Redo".into(),
            SchAction::Delete => "Delete".into(),
            SchAction::Rotate => "Rotate 90°".into(),
            SchAction::Mirror => "Mirror".into(),
            SchAction::Duplicate => "Duplicate".into(),
            SchAction::SelectAll => "Select All".into(),
            SchAction::DeselectAll => "Deselect All".into(),
            SchAction::SelectNet => "Select Whole Net".into(),
            SchAction::Fit => "Fit to Window".into(),
            SchAction::ToggleGrid => "Toggle Grid".into(),
            SchAction::ToggleWireBend => "Toggle Wire Bend (H/V first)".into(),
        }
    }

    pub fn key(self) -> String {
        match self {
            SchAction::Tool(SchTool::Select) => "Esc".into(),
            SchAction::Tool(SchTool::Wire) => "W".into(),
            SchAction::Tool(SchTool::Label) => "T".into(),
            SchAction::Tool(SchTool::Place(k)) => k.shortcut().map(|c| c.to_ascii_uppercase().to_string()).unwrap_or_default(),
            SchAction::Undo => theme::kbd("⌘Z"),
            SchAction::Redo => theme::kbd("⇧⌘Z"),
            SchAction::Delete => "Del".into(),
            SchAction::Rotate => "Space".into(),
            SchAction::Mirror => "X".into(),
            SchAction::Duplicate => theme::kbd("⇧⌘D"),
            SchAction::SelectAll => theme::kbd("⌘A"),
            SchAction::DeselectAll => theme::kbd("⌘D / Esc"),
            SchAction::SelectNet => String::new(),
            SchAction::Fit => "F".into(),
            SchAction::ToggleGrid => String::new(),
            SchAction::ToggleWireBend => "/".into(),
        }
    }
}

/// Something the schematic canvas asks the app to do (it cannot reach the app's command queue).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SchRequest {
    Simulate,
    Check,
    ExportNetlist,
    /// Choose a cell to place (Shift+I).
    PlaceCell,
    /// Open the schematic of the selected cell instance (Shift+X).
    Descend(u64),
    /// Back to the schematic descended from (Shift+B).
    Return,
}

enum Drag {
    Move { start: Pt, snapshot: Schematic, stretch: Vec<(u64, bool, bool)> },
    Rubber { start: Pos2 },
}

/// A cell to place: the cell, its pins and its symbol arrangement.
pub type CellPlacement = (ics_core::CellRef, Vec<(String, PortDir)>, Option<ics_core::schematic::SymbolSpec>);

pub struct SchematicEditor {
    pub view: View2D,
    needs_fit: bool,
    pub tool: SchTool,
    pub selection: BTreeSet<SchObj>,
    pub history: History<Schematic>,
    drag: Option<Drag>,
    wire_start: Option<Pt>,
    wire_hv: bool,
    place_orient: Orient,
    pub label_text: String,
    pub cursor: Option<Pt>,
    pub annotate_op: bool,
    pub show_grid: bool,
    conn_cache: Option<(Schematic, Connectivity)>,
    /// Actions queued by menus and `Command::Sch`, run by the next canvas frame.
    pub actions: Vec<SchAction>,
    /// New MOS devices are drawn with a body pin (`true`) or without one, the body on a rail and
    /// the arrow on the source. Kept in the app settings.
    pub mos_body_pin: bool,
    /// Body nets of new 3-terminal NMOS and PMOS devices, from the project settings (empty is
    /// the built-in `0` / `vdd`).
    pub bulk_nets: [String; 2],
    /// The last symbol switch of an instance: a note (the body pin was labelled) or why it
    /// could not be switched (`true`).
    body_pin_msg: Option<(u64, String, bool)>,
    /// The cell `SchTool::Place(DeviceKind::Cell)` places, with its pins.
    pub place_cell: Option<CellPlacement>,
    /// Requests from keys, handed out with the next canvas output.
    key_requests: Vec<SchRequest>,
}

impl Default for SchematicEditor {
    fn default() -> Self {
        Self {
            view: View2D::new(22.0, false, 2.0, 200.0),
            needs_fit: true,
            tool: SchTool::Select,
            selection: BTreeSet::new(),
            history: History::default(),
            drag: None,
            wire_start: None,
            wire_hv: true,
            place_orient: Orient::default(),
            label_text: "net1".into(),
            cursor: None,
            annotate_op: true,
            show_grid: true,
            conn_cache: None,
            actions: Vec::new(),
            mos_body_pin: true,
            bulk_nets: Default::default(),
            body_pin_msg: None,
            place_cell: None,
            key_requests: vec![],
        }
    }
}

/// Simulation data that can be back-annotated onto the schematic.
#[derive(Default, Clone)]
pub struct OpAnnotation {
    pub node_v: HashMap<String, f64>,
    pub device: HashMap<String, Vec<(String, f64)>>,
}

pub struct DrawOpts<'a> {
    pub pal: Palette,
    pub selection: Option<&'a BTreeSet<SchObj>>,
    pub highlight_net: Option<&'a str>,
    pub op: Option<&'a OpAnnotation>,
    pub show_grid: bool,
    pub thumbnail: bool,
}

fn screen(view: &View2D, rect: Rect, x: f32, y: f32) -> Pos2 {
    view.to_screen(rect, x as f64, y as f64)
}

pub fn draw_symbol(painter: &egui::Painter, view: &View2D, rect: Rect, inst: &Instance, color: Color32, width: f32) {
    let sym = inst.symbol();
    let stroke = Stroke::new(width, color);
    let tp = |x: f32, y: f32| {
        let (wx, wy) = inst.to_world_f(x, y);
        screen(view, rect, wx, wy)
    };
    for prim in &sym.prims {
        match prim {
            Prim::Line(x0, y0, x1, y1) => {
                painter.line_segment([tp(*x0, *y0), tp(*x1, *y1)], stroke);
            }
            Prim::Circle(cx, cy, r) => {
                painter.circle_stroke(tp(*cx, *cy), *r * view.scale as f32, stroke);
            }
            Prim::Poly(pts) => {
                painter.add(egui::Shape::line(pts.iter().map(|(x, y)| tp(*x, *y)).collect(), stroke));
            }
            Prim::Fill(pts) => {
                painter.add(egui::Shape::convex_polygon(pts.iter().map(|(x, y)| tp(*x, *y)).collect(), color, Stroke::NONE));
            }
        }
    }
}

fn draw_grid(painter: &egui::Painter, view: &View2D, rect: Rect, pal: &Palette) {
    let step = if view.scale < 6.0 { 5 } else { 1 };
    let (x0, y0) = view.to_world(rect, rect.min);
    let (x1, y1) = view.to_world(rect, rect.max);
    let (xa, xb) = ((x0.min(x1) / step as f64).floor() as i64 * step, (x0.max(x1)).ceil() as i64);
    let (ya, yb) = ((y0.min(y1) / step as f64).floor() as i64 * step, (y0.max(y1)).ceil() as i64);
    if (xb - xa) / step * (yb - ya) / step > 40_000 {
        return;
    }
    let r = (view.scale as f32 * 0.06).clamp(0.8, 1.6);
    let mut x = xa;
    while x <= xb {
        let mut y = ya;
        while y <= yb {
            let major = x % 10 == 0 && y % 10 == 0;
            let p = screen(view, rect, x as f32, y as f32);
            painter.circle_filled(p, if major { r * 1.5 } else { r }, if major { pal.grid_major } else { pal.grid });
            y += step;
        }
        x += step;
    }
}

pub fn draw_schematic(painter: &egui::Painter, view: &View2D, rect: Rect, sch: &Schematic, conn: &Connectivity, o: &DrawOpts) {
    let pal = o.pal;
    let s = view.scale as f32;
    if o.show_grid && !o.thumbnail {
        draw_grid(painter, view, rect, &pal);
    }
    let sel = |obj: SchObj| o.selection.is_some_and(|set| set.contains(&obj));
    let line_w = if o.thumbnail { 1.0 } else { (s * 0.09).clamp(1.2, 2.6) };
    // wires
    for w in &sch.wires {
        let on_net = o.highlight_net.is_some_and(|n| conn.wire_net.get(&w.id).map(String::as_str) == Some(n));
        let color = if sel(SchObj::Wire(w.id)) {
            pal.selection
        } else if on_net {
            pal.highlight
        } else {
            pal.wire
        };
        let width = if sel(SchObj::Wire(w.id)) || on_net { line_w * 1.6 } else { line_w };
        painter.line_segment(
            [screen(view, rect, w.a.x as f32, w.a.y as f32), screen(view, rect, w.b.x as f32, w.b.y as f32)],
            Stroke::new(width, color),
        );
    }
    for j in &conn.junctions {
        painter.circle_filled(screen(view, rect, j.x as f32, j.y as f32), (s * 0.18).clamp(2.0, 6.0), pal.wire);
    }
    // instances
    let font = FontId::proportional((s * 0.62).clamp(7.0, 18.0));
    for inst in &sch.instances {
        let selected = sel(SchObj::Instance(inst.id));
        let color = if selected { pal.selection } else { pal.symbol };
        draw_symbol(painter, view, rect, inst, color, line_w);
        if selected {
            let b = inst.bbox();
            let r = Rect::from_two_pos(screen(view, rect, b.0 - 0.3, b.1 - 0.3), screen(view, rect, b.2 + 0.3, b.3 + 0.3));
            painter.rect_stroke(r, 3.0, Stroke::new(1.0, pal.selection.gamma_multiply(0.7)), StrokeKind::Outside);
        }
        if o.thumbnail || s < 7.0 {
            continue;
        }
        let sym = inst.symbol();
        if inst.kind == DeviceKind::Cell {
            // The placed cell's name in the box and each pin's name inside its edge.
            let (cx, cy) = inst.to_world_f(0.0, 0.0);
            let name = inst.cell.as_ref().map_or("?".to_string(), |c| c.cell.clone());
            painter.text(screen(view, rect, cx, cy), Align2::CENTER_CENTER, name, font.clone(), pal.symbol);
            let small = FontId::proportional((s * 0.45).clamp(6.0, 13.0));
            for (pin, at) in inst.pin_positions() {
                let (dx, dy) = ((cx - at.x as f32).signum(), (cy - at.y as f32).signum());
                let horizontal = (cx - at.x as f32).abs() > (cy - at.y as f32).abs();
                let (px, py) =
                    if horizontal { (at.x as f32 + dx * 1.3, at.y as f32) } else { (at.x as f32, at.y as f32 + dy * 1.3) };
                let align = if !horizontal {
                    Align2::CENTER_CENTER
                } else if dx > 0.0 {
                    Align2::LEFT_CENTER
                } else {
                    Align2::RIGHT_CENTER
                };
                painter.text(screen(view, rect, px, py), align, pin, small.clone(), pal.text_weak);
            }
        }
        // Label anchor: keep text upright, place on the side the anchor ends up on.
        let (lx, ly) = inst.to_world_f(sym.label_at.0, sym.label_at.1);
        let (cx, cy) = inst.to_world_f(0.0, 0.0);
        let align = if lx >= cx - 0.01 { Align2::LEFT_TOP } else { Align2::RIGHT_TOP };
        let p = screen(view, rect, lx, ly.min(cy + 10.0));
        let mut lines: Vec<String> = vec![];
        match inst.kind {
            DeviceKind::Ground => {}
            DeviceKind::Cell => lines.push(inst.name.clone()),
            DeviceKind::Supply => lines.push(inst.param("net").to_string()),
            DeviceKind::Nmos | DeviceKind::Pmos | DeviceKind::Nmos3 | DeviceKind::Pmos3 => {
                lines.push(inst.name.clone());
                lines.push(format!("{}/{}", inst.param("w"), inst.param("l")));
            }
            _ => {
                lines.push(inst.name.clone());
                if let Some(pp) = inst.kind.primary_param() {
                    let v = inst.param(pp);
                    if !v.is_empty() {
                        lines.push(v.to_string());
                    }
                }
                let tran = inst.param("tran");
                if !tran.is_empty() {
                    let short: String = tran.chars().take(18).collect();
                    lines.push(if tran.len() > 18 { format!("{short}…") } else { short });
                }
            }
        }
        if inst.kind == DeviceKind::Supply {
            let p = screen(view, rect, cx, cy - 1.0);
            painter.text(p, Align2::CENTER_BOTTOM, &lines[0], font.clone(), pal.label);
        } else {
            for (k, l) in lines.iter().enumerate() {
                let c = if k == 0 { pal.text_weak } else { pal.symbol.gamma_multiply(0.85) };
                painter.text(p + Vec2::new(0.0, k as f32 * font.size * 1.15), align, l, font.clone(), c);
            }
        }
        if let Some(op) = o.op
            && let Some(vals) = op.device.get(&inst.name.to_ascii_lowercase())
            && let Some((_, id)) = vals.iter().find(|(k, _)| k == "id")
        {
            // Below the name / size lines.
            let pp = p + Vec2::new(0.0, lines.len() as f32 * font.size * 1.15);
            let id = format!("id={}", format_eng(*id, "A"));
            painter.text(pp, align, id, FontId::monospace(font.size * 0.8), pal.simulation);
        }
    }
    // pins without connection
    if !o.thumbnail {
        for p in &conn.unconnected_pins {
            let c = screen(view, rect, p.x as f32, p.y as f32);
            let h = (s * 0.18).clamp(2.0, 5.0);
            painter.rect_stroke(
                Rect::from_center_size(c, Vec2::splat(2.0 * h)),
                0.0,
                Stroke::new(1.2, pal.pin),
                StrokeKind::Middle,
            );
        }
    }
    // labels
    for l in &sch.labels {
        let p = screen(view, rect, l.pos.x as f32, l.pos.y as f32);
        let color = if sel(SchObj::Label(l.id)) { pal.selection } else { pal.label };
        painter.circle_filled(p, (s * 0.12).clamp(1.5, 4.0), color);
        if let Some(dir) = l.port {
            // Pin: a pentagon beyond the label point on the side away from its wire, pointing
            // into the circuit for an input, out of it for an output, both ways for inout.
            let (h, w) = ((s * 0.3).clamp(3.0, 9.0), (s * 0.6).clamp(5.0, 16.0));
            let (ox, oy) = sch.pin_outward(l.id);
            // Screen y points down like schematic y.
            let (u, n) = (Vec2::new(ox as f32, oy as f32), Vec2::new(-oy as f32, ox as f32));
            let at = |a: f32, b: f32| p + u * (3.0 + a) + n * b;
            let pts = match dir {
                PortDir::Input => vec![at(0.0, 0.0), at(h, -h), at(w, -h), at(w, h), at(h, h)],
                PortDir::Output => vec![at(0.0, -h), at(w - h, -h), at(w, 0.0), at(w - h, h), at(0.0, h)],
                PortDir::InOut => vec![at(0.0, 0.0), at(h, -h), at(w - h, -h), at(w, 0.0), at(w - h, h), at(h, h)],
            };
            painter.add(egui::Shape::closed_line(pts, Stroke::new(1.2, color)));
        }
        if !o.thumbnail {
            painter.text(p + Vec2::new(3.0, -3.0), Align2::LEFT_BOTTOM, &l.name, font.clone(), color);
        }
    }
    // operating point voltages
    if let Some(op) = o.op
        && !o.thumbnail
    {
        let mut placed: BTreeSet<String> = BTreeSet::new();
        let tag_font = FontId::monospace((s * 0.55).clamp(8.0, 14.0));
        let mut tag = |net: &str, at: Pos2| {
            if placed.contains(net) {
                return;
            }
            if let Some(v) = op.node_v.get(&net.to_ascii_lowercase()) {
                let text = format_eng(*v, "V");
                let g = painter.layout_no_wrap(text, tag_font.clone(), pal.simulation);
                let r = Rect::from_min_size(at + Vec2::new(4.0, 4.0), g.size() + Vec2::new(8.0, 2.0));
                painter.rect_filled(r, 3.0, pal.bg_canvas.gamma_multiply(0.85));
                painter.rect_stroke(r, 3.0, Stroke::new(1.0, pal.simulation.gamma_multiply(0.6)), StrokeKind::Inside);
                painter.galley(r.min + Vec2::new(4.0, 1.0), g, pal.simulation);
                placed.insert(net.to_string());
            }
        };
        for l in &sch.labels {
            tag(&l.name, screen(view, rect, l.pos.x as f32, l.pos.y as f32));
        }
        for i in sch.instances.iter().filter(|i| i.kind == DeviceKind::Supply) {
            let net = i.param("net");
            let net = if net.trim().is_empty() { "vdd" } else { net.trim() };
            tag(net, screen(view, rect, i.pos.x as f32 + 0.6, i.pos.y as f32 - 2.4));
        }
        for w in &sch.wires {
            if let Some(n) = conn.wire_net.get(&w.id) {
                if n == "0" {
                    continue;
                }
                let m = screen(view, rect, (w.a.x + w.b.x) as f32 / 2.0, (w.a.y + w.b.y) as f32 / 2.0);
                tag(n, m);
            }
        }
    }
}

fn dist_seg(p: (f64, f64), a: Pt, b: Pt) -> f64 {
    let (ax, ay, bx, by) = (a.x as f64, a.y as f64, b.x as f64, b.y as f64);
    let (dx, dy) = (bx - ax, by - ay);
    let l2 = dx * dx + dy * dy;
    let t = if l2 == 0.0 { 0.0 } else { (((p.0 - ax) * dx + (p.1 - ay) * dy) / l2).clamp(0.0, 1.0) };
    ((p.0 - ax - t * dx).powi(2) + (p.1 - ay - t * dy).powi(2)).sqrt()
}

pub fn hit_test(sch: &Schematic, p: (f64, f64), tol: f64) -> Option<SchObj> {
    for l in sch.labels.iter().rev() {
        if ((l.pos.x as f64 - p.0).powi(2) + (l.pos.y as f64 - p.1).powi(2)).sqrt() < tol * 1.5 {
            return Some(SchObj::Label(l.id));
        }
    }
    let mut best: Option<(f64, u64)> = None;
    for w in &sch.wires {
        let d = dist_seg(p, w.a, w.b);
        if d < tol && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, w.id));
        }
    }
    if let Some((_, id)) = best {
        return Some(SchObj::Wire(id));
    }
    for i in sch.instances.iter().rev() {
        let b = i.bbox();
        let m = 0.2;
        if p.0 >= (b.0 - m) as f64 && p.0 <= (b.2 + m) as f64 && p.1 >= (b.1 - m) as f64 && p.1 <= (b.3 + m) as f64 {
            return Some(SchObj::Instance(i.id));
        }
    }
    None
}

fn obj_bbox(sch: &Schematic, o: SchObj) -> Option<(f32, f32, f32, f32)> {
    match o {
        SchObj::Instance(id) => sch.instance(id).map(|i| i.bbox()),
        SchObj::Wire(id) => sch
            .wires
            .iter()
            .find(|w| w.id == id)
            .map(|w| (w.a.x.min(w.b.x) as f32, w.a.y.min(w.b.y) as f32, w.a.x.max(w.b.x) as f32, w.a.y.max(w.b.y) as f32)),
        SchObj::Label(id) => {
            sch.labels.iter().find(|l| l.id == id).map(|l| (l.pos.x as f32, l.pos.y as f32, l.pos.x as f32, l.pos.y as f32))
        }
    }
}

pub fn schematic_bounds(sch: &Schematic) -> Option<((f64, f64), (f64, f64))> {
    let mut b: Option<(f32, f32, f32, f32)> = None;
    let objs = sch
        .instances
        .iter()
        .map(|i| SchObj::Instance(i.id))
        .chain(sch.wires.iter().map(|w| SchObj::Wire(w.id)))
        .chain(sch.labels.iter().map(|l| SchObj::Label(l.id)));
    for o in objs {
        if let Some(r) = obj_bbox(sch, o) {
            b = Some(match b {
                None => r,
                Some(a) => (a.0.min(r.0), a.1.min(r.1), a.2.max(r.2), a.3.max(r.3)),
            });
        }
    }
    b.map(|b| ((b.0 as f64 - 1.0, b.1 as f64 - 1.0), (b.2 as f64 + 1.0, b.3 as f64 + 1.0)))
}

fn connection_points(sch: &Schematic) -> Vec<Pt> {
    let mut v: Vec<Pt> = sch.instances.iter().flat_map(|i| i.pin_positions().into_iter().map(|(_, p)| p)).collect();
    for w in &sch.wires {
        v.push(w.a);
        v.push(w.b);
    }
    v
}

pub struct SchOutput {
    pub requests: Vec<SchRequest>,
    pub changed: bool,
}

impl SchematicEditor {
    pub fn request_fit(&mut self) {
        self.needs_fit = true;
    }

    pub fn conn<'a>(&'a mut self, sch: &Schematic) -> &'a Connectivity {
        let stale = self.conn_cache.as_ref().is_none_or(|(s, _)| s != sch);
        if stale {
            self.conn_cache = Some((sch.clone(), extract(sch)));
        }
        &self.conn_cache.as_ref().unwrap().1
    }

    /// The body net a new 3-terminal device of `kind` gets.
    fn bulk_net(&self, kind: DeviceKind) -> &str {
        let n = self.bulk_nets[kind.is_pmos() as usize].trim();
        if n.is_empty() { kind.default_bulk() } else { n }
    }

    pub fn set_tool(&mut self, t: SchTool) {
        self.tool = t;
        self.wire_start = None;
        if matches!(t, SchTool::Place(_)) {
            self.place_orient = Orient::default();
        }
    }

    pub fn undo(&mut self, sch: &mut Schematic) -> bool {
        self.selection.clear();
        self.history.undo(sch)
    }

    pub fn redo(&mut self, sch: &mut Schematic) -> bool {
        self.selection.clear();
        self.history.redo(sch)
    }

    pub fn delete_selection(&mut self, sch: &mut Schematic) -> bool {
        if self.selection.is_empty() {
            return false;
        }
        self.history.push(sch);
        for o in std::mem::take(&mut self.selection) {
            sch.remove(o);
        }
        true
    }

    pub fn rotate_selection(&mut self, sch: &mut Schematic, mirror: bool) -> bool {
        let ids: Vec<u64> =
            self.selection.iter().filter_map(|o| if let SchObj::Instance(i) = o { Some(*i) } else { None }).collect();
        if ids.is_empty() {
            return false;
        }
        self.history.push(sch);
        for id in ids {
            if let Some(i) = sch.instance_mut(id) {
                i.orient = if mirror { i.orient.mirrored() } else { i.orient.rotated() };
            }
        }
        true
    }

    pub fn duplicate_selection(&mut self, sch: &mut Schematic) -> bool {
        if self.selection.is_empty() {
            return false;
        }
        self.history.push(sch);
        let d = Pt::new(2, 2);
        let mut new_sel = BTreeSet::new();
        for o in self.selection.clone() {
            match o {
                SchObj::Instance(id) => {
                    if let Some(src) = sch.instance(id).cloned() {
                        let nid = sch.alloc_id();
                        let name = sch.next_name(src.kind);
                        sch.instances.push(Instance { id: nid, name, pos: src.pos + d, ..src });
                        new_sel.insert(SchObj::Instance(nid));
                    }
                }
                SchObj::Wire(id) => {
                    if let Some(w) = sch.wires.iter().find(|w| w.id == id).cloned()
                        && let Some(nid) = sch.add_wire(w.a + d, w.b + d)
                    {
                        new_sel.insert(SchObj::Wire(nid));
                    }
                }
                SchObj::Label(id) => {
                    if let Some(l) = sch.labels.iter().find(|l| l.id == id).cloned() {
                        let nid = match l.port {
                            Some(dir) => sch.add_pin(l.name, l.pos + d, dir),
                            None => sch.add_label(l.name, l.pos + d),
                        };
                        new_sel.insert(SchObj::Label(nid));
                    }
                }
            }
        }
        self.selection = new_sel;
        true
    }

    pub fn select_all(&mut self, sch: &Schematic) {
        self.selection = sch
            .instances
            .iter()
            .map(|i| SchObj::Instance(i.id))
            .chain(sch.wires.iter().map(|w| SchObj::Wire(w.id)))
            .chain(sch.labels.iter().map(|l| SchObj::Label(l.id)))
            .collect();
    }

    /// Select every wire and label on the nets of the selected wires and labels.
    pub fn select_net(&mut self, sch: &Schematic) {
        let conn = self.conn(sch).clone();
        let mut nets: BTreeSet<String> = BTreeSet::new();
        for o in &self.selection {
            match o {
                SchObj::Wire(id) => nets.extend(conn.wire_net.get(id).cloned()),
                SchObj::Label(id) => nets.extend(sch.labels.iter().find(|l| l.id == *id).map(|l| l.name.clone())),
                SchObj::Instance(_) => {}
            }
        }
        for n in &nets {
            for w in conn.net_wires.get(n).into_iter().flatten() {
                self.selection.insert(SchObj::Wire(*w));
            }
            for l in sch.labels.iter().filter(|l| &l.name == n) {
                self.selection.insert(SchObj::Label(l.id));
            }
        }
    }

    /// Run one named action. Returns true when the schematic changed.
    pub fn apply(&mut self, a: SchAction, sch: &mut Schematic) -> bool {
        match a {
            SchAction::Tool(t) => self.set_tool(t),
            SchAction::Undo => return self.undo(sch),
            SchAction::Redo => return self.redo(sch),
            SchAction::Delete => return self.delete_selection(sch),
            SchAction::Rotate => return self.rotate_selection(sch, false),
            SchAction::Mirror => return self.rotate_selection(sch, true),
            SchAction::Duplicate => return self.duplicate_selection(sch),
            SchAction::SelectAll => self.select_all(sch),
            SchAction::DeselectAll => self.selection.clear(),
            SchAction::SelectNet => self.select_net(sch),
            SchAction::Fit => self.needs_fit = true,
            SchAction::ToggleGrid => self.show_grid = !self.show_grid,
            SchAction::ToggleWireBend => self.wire_hv = !self.wire_hv,
        }
        false
    }

    /// One-line description of the selection, for the pop-up menu header.
    fn selection_summary(&mut self, sch: &Schematic) -> String {
        if let [o] = self.selection.iter().copied().collect::<Vec<_>>()[..] {
            return match o {
                SchObj::Instance(id) => {
                    sch.instance(id).map_or_else(String::new, |i| format!("{} · {}", i.name, t(i.kind.label())))
                }
                SchObj::Wire(id) => match self.conn(sch).wire_net.get(&id) {
                    Some(n) => tf("Wire · net {}", &[n]),
                    None => t("Wire").into(),
                },
                SchObj::Label(id) => {
                    sch.labels.iter().find(|l| l.id == id).map_or_else(String::new, |l| tf("Net label {}", &[&l.name]))
                }
            };
        }
        let n = |f: fn(&SchObj) -> bool| self.selection.iter().filter(|o| f(o)).count();
        let (i, w, l) =
            (n(|o| matches!(o, SchObj::Instance(_))), n(|o| matches!(o, SchObj::Wire(_))), n(|o| matches!(o, SchObj::Label(_))));
        let mut parts = vec![];
        for (c, one, many) in [(i, "{} instance", "{} instances"), (w, "{} wire", "{} wires"), (l, "{} label", "{} labels")] {
            if c > 0 {
                parts.push(tf(if c == 1 { one } else { many }, &[&c.to_string()]));
            }
        }
        parts.join(", ")
    }

    /// The canvas pop-up menu: right click with the Select tool (or an idle Wire tool).
    /// Right-clicking an unselected object selects it first.
    fn popup_menu(&mut self, resp: &egui::Response, sch: &Schematic, world: Option<(f64, f64)>, tol: f64) -> Vec<SchRequest> {
        let idle = self.drag.is_none() && self.wire_start.is_none() && matches!(self.tool, SchTool::Select | SchTool::Wire);
        let open = resp.secondary_clicked() && idle;
        // Empty space keeps the selection, as in the layout editor; Esc clears it.
        if open
            && let Some(o) = world.and_then(|w| hit_test(sch, w, tol))
            && !self.selection.contains(&o)
        {
            self.selection = [o].into();
        }
        let summary = if open || !self.selection.is_empty() { self.selection_summary(sch) } else { String::new() };
        let has_inst = self.selection.iter().any(|o| matches!(o, SchObj::Instance(_)));
        let has_net = self.selection.iter().any(|o| matches!(o, SchObj::Wire(_) | SchObj::Label(_)));
        let (can_undo, can_redo) = (self.history.can_undo(), self.history.can_redo());
        let (grid, hv) = (self.show_grid, self.wire_hv);
        let selected = !self.selection.is_empty();
        let mut picked: Option<SchAction> = None;
        let mut requests = vec![];
        let item = |ui: &mut egui::Ui, a: SchAction, enabled: bool, picked: &mut Option<SchAction>| {
            if ui.add_enabled(enabled, egui::Button::new(t(&a.name())).shortcut_text(a.key())).clicked() {
                *picked = Some(a);
            }
        };
        egui::Popup::menu(resp)
            .id(egui::Id::unique("sch_popup"))
            .open_memory(open.then_some(egui::SetOpenCommand::Bool(true)))
            .at_pointer_fixed()
            .show(|ui| {
                ui.set_min_width(200.0);
                if selected {
                    ui.label(egui::RichText::new(&summary).small().strong());
                    ui.separator();
                    if let Some(id) = self.selected_cell_instance(sch)
                        && ui.add(egui::Button::new(t("Descend into Cell")).shortcut_text("Shift+X")).clicked()
                    {
                        requests.push(SchRequest::Descend(id));
                    }
                    item(ui, SchAction::Rotate, has_inst, &mut picked);
                    item(ui, SchAction::Mirror, has_inst, &mut picked);
                    item(ui, SchAction::Duplicate, true, &mut picked);
                    if has_net {
                        item(ui, SchAction::SelectNet, true, &mut picked);
                    }
                    ui.separator();
                    item(ui, SchAction::Delete, true, &mut picked);
                    item(ui, SchAction::DeselectAll, true, &mut picked);
                } else {
                    item(ui, SchAction::Tool(SchTool::Wire), true, &mut picked);
                    item(ui, SchAction::Tool(SchTool::Label), true, &mut picked);
                    ui.menu_button(t("Place Device"), |ui| {
                        for k in DeviceKind::PALETTE {
                            item(ui, SchAction::Tool(SchTool::Place(k.with_body_pin(self.mos_body_pin))), true, &mut picked);
                        }
                    });
                    if ui.add(egui::Button::new(t("Place Cell Instance…")).shortcut_text("Shift+I")).clicked() {
                        requests.push(SchRequest::PlaceCell);
                    }
                    if ui.add(egui::Button::new(t("Return to Parent")).shortcut_text("Shift+B")).clicked() {
                        requests.push(SchRequest::Return);
                    }
                    ui.separator();
                    item(ui, SchAction::Undo, can_undo, &mut picked);
                    item(ui, SchAction::Redo, can_redo, &mut picked);
                    item(ui, SchAction::SelectAll, !sch.is_empty(), &mut picked);
                    ui.separator();
                    item(ui, SchAction::Fit, true, &mut picked);
                    let mut g = grid;
                    if ui.checkbox(&mut g, t("Show Grid")).clicked() {
                        picked = Some(SchAction::ToggleGrid);
                    }
                    let mut h = hv;
                    if ui.checkbox(&mut h, t("Wires Bend Horizontal First")).on_hover_text("/").clicked() {
                        picked = Some(SchAction::ToggleWireBend);
                    }
                    ui.separator();
                    if ui.add(egui::Button::new(t("Run Simulation")).shortcut_text("F5")).clicked() {
                        requests.push(SchRequest::Simulate);
                    }
                    if ui.button(t("Check Schematic")).clicked() {
                        requests.push(SchRequest::Check);
                    }
                    if ui.button(t("Export Netlist…")).clicked() {
                        requests.push(SchRequest::ExportNetlist);
                    }
                }
            });
        if let Some(a) = picked {
            self.actions.push(a);
            resp.ctx.request_repaint();
        }
        requests
    }

    /// The only selected instance, when it places a cell.
    fn selected_cell_instance(&self, sch: &Schematic) -> Option<u64> {
        let mut it = self.selection.iter().filter_map(|o| match o {
            SchObj::Instance(id) => Some(*id),
            _ => None,
        });
        let id = it.next()?;
        (it.next().is_none() && sch.instance(id).is_some_and(|i| i.kind == DeviceKind::Cell)).then_some(id)
    }

    fn snap(&self, w: (f64, f64)) -> Pt {
        Pt::new(w.0.round() as i32, w.1.round() as i32)
    }

    fn handle_keys(&mut self, ui: &egui::Ui, sch: &mut Schematic) -> bool {
        if ui.ctx().text_edit_focused() {
            return false;
        }
        let mut changed = false;
        let (pressed, cmd, shift): (Vec<Key>, bool, bool) = ui.input(|i| {
            let keys = i
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key { key, pressed: true, repeat: false, .. } => Some(*key),
                    _ => None,
                })
                .collect();
            (keys, i.modifiers.command, i.modifiers.shift)
        });
        for k in pressed {
            if cmd {
                match k {
                    Key::A => self.select_all(sch),
                    // Shift+Cmd+D duplicates; Cmd+D deselects all, as in the layout (Virtuoso).
                    Key::D if shift => changed |= self.duplicate_selection(sch),
                    Key::D => self.selection.clear(),
                    _ => {}
                }
                continue;
            }
            match k {
                Key::Escape => {
                    if self.wire_start.is_some() {
                        self.wire_start = None;
                    } else if self.tool != SchTool::Select {
                        self.set_tool(SchTool::Select);
                    } else {
                        self.selection.clear();
                    }
                }
                Key::Delete | Key::Backspace => changed |= self.delete_selection(sch),
                Key::W => self.set_tool(SchTool::Wire),
                Key::F => self.needs_fit = true,
                Key::Slash => self.wire_hv = !self.wire_hv,
                Key::Space | Key::X if !(shift && k == Key::X) => {
                    let mirror = k == Key::X;
                    if let SchTool::Place(_) = self.tool {
                        self.place_orient = if mirror { self.place_orient.mirrored() } else { self.place_orient.rotated() };
                    } else {
                        changed |= self.rotate_selection(sch, mirror);
                    }
                }
                Key::T => self.set_tool(SchTool::Label),
                Key::I if shift => self.key_requests.push(SchRequest::PlaceCell),
                Key::X if shift => {
                    if let Some(id) = self.selected_cell_instance(sch) {
                        self.key_requests.push(SchRequest::Descend(id));
                    }
                }
                Key::B if shift => self.key_requests.push(SchRequest::Return),
                _ => {
                    for kind in DeviceKind::PALETTE {
                        let Some(c) = kind.shortcut() else { continue };
                        if Key::from_name(&c.to_string()) == Some(k) && !shift {
                            self.set_tool(SchTool::Place(kind.with_body_pin(self.mos_body_pin)));
                        }
                    }
                }
            }
        }
        changed
    }

    /// Draw and interact with the schematic canvas.
    pub fn canvas(&mut self, ui: &mut egui::Ui, sch: &mut Schematic, op: Option<&OpAnnotation>) -> SchOutput {
        let pal = theme::palette(ui.ctx());
        let mut changed = false;
        for a in std::mem::take(&mut self.actions) {
            changed |= self.apply(a, sch);
        }
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = resp.rect;
        painter.rect_filled(rect, 0.0, pal.bg_canvas);
        if self.needs_fit {
            match schematic_bounds(sch) {
                Some((a, b)) => self.view.fit(rect, a, b, 60.0),
                None => {
                    self.view.center = (10.0, 5.0);
                    self.view.scale = 22.0;
                }
            }
            self.view.scale = self.view.scale.min(40.0);
            self.needs_fit = false;
        }
        self.view.handle_navigation(ui, &resp, rect, true);
        if resp.hovered() {
            changed |= self.handle_keys(ui, sch);
        }

        let hover = resp.hover_pos();
        let world = hover.map(|p| self.view.to_world(rect, p));
        self.cursor = world.map(|w| self.snap(w));
        let tol = (6.0 / self.view.scale).max(0.25);
        let requests = self.popup_menu(&resp, sch, world, tol);

        match self.tool {
            SchTool::Select => changed |= self.select_tool(ui, &resp, rect, sch, world, tol),
            SchTool::Wire => {
                if let Some(c) = self.cursor {
                    if resp.clicked() {
                        match self.wire_start {
                            None => self.wire_start = Some(c),
                            Some(s) => {
                                if s != c {
                                    self.history.push(sch);
                                    let corner = if self.wire_hv { Pt::new(c.x, s.y) } else { Pt::new(s.x, c.y) };
                                    sch.add_wire(s, corner);
                                    sch.add_wire(corner, c);
                                    sch.cleanup_wires();
                                    changed = true;
                                    let pts = connection_points(sch);
                                    let ends_on_existing = pts.iter().filter(|p| **p == c).count() > 1
                                        || sch
                                            .wires
                                            .iter()
                                            .any(|w| w.a != c && w.b != c && ics_core::geom::on_segment(c, w.a, w.b));
                                    self.wire_start = if ends_on_existing { None } else { Some(c) };
                                }
                            }
                        }
                    }
                    if resp.double_clicked() || resp.secondary_clicked() {
                        self.wire_start = None;
                    }
                }
            }
            SchTool::Place(kind) => {
                if resp.clicked()
                    && let Some(c) = self.cursor
                {
                    self.history.push(sch);
                    let id = match (&self.place_cell, kind) {
                        (Some((cell, ports, spec)), DeviceKind::Cell) => {
                            let id = sch.add_cell_instance(cell.clone(), ports.clone(), c, self.place_orient);
                            if let Some(i) = sch.instance_mut(id) {
                                i.symbol = spec.clone();
                            }
                            id
                        }
                        _ => sch.add_instance(kind, c, self.place_orient),
                    };
                    if kind.implicit_bulk()
                        && let Some(i) = sch.instance_mut(id)
                    {
                        i.params.insert("bulk".into(), self.bulk_net(kind).to_string());
                    }
                    self.selection = [SchObj::Instance(id)].into();
                    changed = true;
                }
                if resp.secondary_clicked() {
                    self.set_tool(SchTool::Select);
                }
            }
            SchTool::Label => {
                if resp.clicked()
                    && let Some(c) = self.cursor
                {
                    let name = self.label_text.trim().to_string();
                    if !name.is_empty() {
                        self.history.push(sch);
                        sch.add_label(&name, c);
                        self.label_text = bump_name(&name);
                        changed = true;
                    }
                }
                if resp.secondary_clicked() {
                    self.set_tool(SchTool::Select);
                }
            }
        }

        // Hover net highlight
        let conn = self.conn(sch).clone();
        let hover_obj = world.and_then(|w| hit_test(sch, w, tol));
        let mut highlight: Option<String> = None;
        if let Some(SchObj::Wire(id)) = hover_obj {
            highlight = conn.wire_net.get(&id).cloned();
        }
        if highlight.is_none()
            && let Some(SchObj::Wire(id)) = self.selection.iter().next().copied()
        {
            highlight = conn.wire_net.get(&id).cloned();
        }

        let painter = painter.with_clip_rect(rect);
        draw_schematic(
            &painter,
            &self.view,
            rect,
            sch,
            &conn,
            &DrawOpts {
                pal,
                selection: Some(&self.selection),
                highlight_net: highlight.as_deref(),
                op: if self.annotate_op { op } else { None },
                show_grid: self.show_grid,
                thumbnail: false,
            },
        );

        // Tool overlays
        let s = self.view.scale as f32;
        if let Some(c) = self.cursor {
            let cp = screen(&self.view, rect, c.x as f32, c.y as f32);
            match self.tool {
                SchTool::Wire => {
                    let crosshair = Stroke::new(1.0, pal.wire.gamma_multiply(0.5));
                    painter.line_segment([cp - Vec2::new(8.0, 0.0), cp + Vec2::new(8.0, 0.0)], crosshair);
                    painter.line_segment([cp - Vec2::new(0.0, 8.0), cp + Vec2::new(0.0, 8.0)], crosshair);
                    if let Some(st) = self.wire_start {
                        let corner = if self.wire_hv { Pt::new(c.x, st.y) } else { Pt::new(st.x, c.y) };
                        let pts = [st, corner, c].map(|p| screen(&self.view, rect, p.x as f32, p.y as f32));
                        let st = Stroke::new((s * 0.09).clamp(1.2, 2.6), pal.wire.gamma_multiply(0.8));
                        painter.line_segment([pts[0], pts[1]], st);
                        painter.line_segment([pts[1], pts[2]], st);
                    }
                }
                SchTool::Place(kind) => {
                    let (cell, ports, symbol) = match (&self.place_cell, kind) {
                        (Some((c, p, sp)), DeviceKind::Cell) => (Some(c.clone()), p.clone(), sp.clone()),
                        _ => (None, vec![], None),
                    };
                    let ghost = Instance {
                        id: 0,
                        kind,
                        name: String::new(),
                        pos: c,
                        orient: self.place_orient,
                        params: Default::default(),
                        cell,
                        ports,
                        symbol,
                    };
                    draw_symbol(
                        &painter,
                        &self.view,
                        rect,
                        &ghost,
                        pal.selection.gamma_multiply(0.7),
                        (s * 0.09).clamp(1.2, 2.6),
                    );
                }
                SchTool::Label => {
                    painter.circle_filled(cp, 3.0, pal.label);
                    painter.text(
                        cp + Vec2::new(4.0, -4.0),
                        Align2::LEFT_BOTTOM,
                        &self.label_text,
                        FontId::proportional((s * 0.62).clamp(8.0, 18.0)),
                        pal.label.gamma_multiply(0.6),
                    );
                }
                SchTool::Select => {}
            }
        }
        if let Some(Drag::Rubber { start }) = &self.drag
            && let Some(h) = hover
        {
            let r = Rect::from_two_pos(*start, h);
            painter.rect_filled(r, 0.0, pal.selection.gamma_multiply(0.08));
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, pal.selection), StrokeKind::Inside);
        }
        // Empty-state hint
        if sch.is_empty() {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                t("Empty schematic — pick a device from the library (or press R, C, V, N, P…) and click to place.\nW draws wires, T places net labels, right-click for a menu, scroll to zoom, right-drag to pan."),
                FontId::proportional(14.0),
                pal.text_weak,
            );
        }
        // Cursor
        let cursor = match self.tool {
            SchTool::Select if hover_obj.is_some() => egui::CursorIcon::PointingHand,
            SchTool::Select => egui::CursorIcon::Default,
            _ => egui::CursorIcon::Crosshair,
        };
        if resp.hovered() {
            ui.ctx().set_cursor_icon(cursor);
        }
        let mut requests = requests;
        requests.append(&mut self.key_requests);
        SchOutput { changed, requests }
    }

    fn select_tool(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        rect: Rect,
        sch: &mut Schematic,
        world: Option<(f64, f64)>,
        tol: f64,
    ) -> bool {
        let mut changed = false;
        let shift = ui.input(|i| i.modifiers.shift || i.modifiers.command);
        if resp.drag_started_by(egui::PointerButton::Primary) {
            let press = ui.input(|i| i.pointer.press_origin()).unwrap_or(rect.center());
            let pw = self.view.to_world(rect, press);
            match hit_test(sch, pw, tol) {
                Some(obj) => {
                    if !self.selection.contains(&obj) {
                        if !shift {
                            self.selection.clear();
                        }
                        self.selection.insert(obj);
                    }
                    // Collect wire endpoints to stretch along with moved pins.
                    let mut anchor_pts: Vec<Pt> = vec![];
                    for o in &self.selection {
                        if let SchObj::Instance(id) = o
                            && let Some(i) = sch.instance(*id)
                        {
                            anchor_pts.extend(i.pin_positions().into_iter().map(|(_, p)| p));
                        }
                    }
                    let mut stretch = vec![];
                    for w in &sch.wires {
                        if self.selection.contains(&SchObj::Wire(w.id)) {
                            continue;
                        }
                        let a = anchor_pts.contains(&w.a);
                        let b = anchor_pts.contains(&w.b);
                        if a || b {
                            stretch.push((w.id, a, b));
                        }
                    }
                    self.drag = Some(Drag::Move { start: self.snap(pw), snapshot: sch.clone(), stretch });
                }
                None => self.drag = Some(Drag::Rubber { start: press }),
            }
        }
        if let (Some(Drag::Move { start, snapshot, stretch }), Some(w)) = (&self.drag, world) {
            let d = self.snap(w) - *start;
            let mut next = snapshot.clone();
            if d != Pt::new(0, 0) {
                for o in &self.selection {
                    next.translate(*o, d);
                }
                for (id, a, b) in stretch {
                    if let Some(wire) = next.wires.iter_mut().find(|x| x.id == *id) {
                        if *a {
                            wire.a = wire.a + d;
                        }
                        if *b {
                            wire.b = wire.b + d;
                        }
                    }
                }
            }
            if next != *sch {
                *sch = next;
            }
        }
        if resp.drag_stopped() {
            match self.drag.take() {
                Some(Drag::Move { snapshot, .. }) => {
                    if snapshot != *sch {
                        sch.cleanup_wires();
                        self.history.push(&snapshot);
                        changed = true;
                    }
                }
                Some(Drag::Rubber { start }) => {
                    if let Some(end) = resp.interact_pointer_pos().or(resp.hover_pos()) {
                        let a = self.view.to_world(rect, start);
                        let b = self.view.to_world(rect, end);
                        let (x0, x1) = (a.0.min(b.0) as f32, a.0.max(b.0) as f32);
                        let (y0, y1) = (a.1.min(b.1) as f32, a.1.max(b.1) as f32);
                        if !shift {
                            self.selection.clear();
                        }
                        let all: Vec<SchObj> = sch
                            .instances
                            .iter()
                            .map(|i| SchObj::Instance(i.id))
                            .chain(sch.wires.iter().map(|w| SchObj::Wire(w.id)))
                            .chain(sch.labels.iter().map(|l| SchObj::Label(l.id)))
                            .collect();
                        for o in all {
                            if let Some(bb) = obj_bbox(sch, o)
                                && bb.0 >= x0
                                && bb.2 <= x1
                                && bb.1 >= y0
                                && bb.3 <= y1
                            {
                                self.selection.insert(o);
                            }
                        }
                    }
                }
                None => {}
            }
        }
        if resp.clicked()
            && let Some(w) = world
        {
            match hit_test(sch, w, tol) {
                Some(o) => {
                    if shift {
                        if !self.selection.remove(&o) {
                            self.selection.insert(o);
                        }
                    } else {
                        self.selection = [o].into();
                    }
                }
                None => {
                    if !shift {
                        self.selection.clear();
                    }
                }
            }
        }
        changed
    }

    /// Property inspector for the current selection.
    pub fn inspector(&mut self, ui: &mut egui::Ui, sch: &mut Schematic) -> bool {
        let pal = theme::palette(ui.ctx());
        let mut changed = false;
        let conn = self.conn(sch).clone();
        if self.selection.is_empty() {
            theme::section(ui, t("Schematic"));
            egui::Grid::new("sch_stats").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                ui.label(t("Instances"));
                ui.label(sch.instances.iter().filter(|i| i.kind.spice_prefix().is_some()).count().to_string());
                ui.end_row();
                ui.label(t("Wires"));
                ui.label(sch.wires.len().to_string());
                ui.end_row();
                ui.label(t("Nets"));
                ui.label(conn.nets().len().to_string());
                ui.end_row();
            });
            ui.add_space(8.0);
            theme::section(ui, t("Nets"));
            egui::ScrollArea::vertical().id_salt("nets").max_height(260.0).show(ui, |ui| {
                for n in conn.nets() {
                    let pins = conn.pin_nets.values().filter(|x| **x == n).count();
                    ui.horizontal(|ui| {
                        let r = ui
                            .add(
                                egui::Label::new(egui::RichText::new(&n).monospace().color(if n == "0" {
                                    pal.text_weak
                                } else {
                                    pal.wire
                                }))
                                .sense(egui::Sense::click()),
                            )
                            .on_hover_text(t("Click to select the net, right-click for more"));
                        let select = |ed: &mut Self| {
                            ed.selection = conn.net_wires.get(&n).into_iter().flatten().map(|w| SchObj::Wire(*w)).collect();
                            ed.selection.extend(sch.labels.iter().filter(|l| l.name == n).map(|l| SchObj::Label(l.id)));
                        };
                        if r.clicked() {
                            select(self);
                        }
                        r.context_menu(|ui| {
                            if ui.button(t("Select Net")).clicked() {
                                select(self);
                            }
                            if ui.button(t("Copy Net Name")).clicked() {
                                ui.ctx().copy_text(n.clone());
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new(tf("{} pins", &[&pins.to_string()])).small().color(pal.text_weak));
                        });
                    });
                }
            });
            if !conn.warnings.is_empty() {
                theme::section(ui, t("Warnings"));
                for w in &conn.warnings {
                    ui.colored_label(pal.warning, w);
                }
            }
            return false;
        }
        if self.selection.len() > 1 {
            theme::section(ui, t("Selection"));
            ui.label(tf("{} objects selected", &[&self.selection.len().to_string()]));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button(t("⟳ Rotate")).clicked() {
                    changed |= self.rotate_selection(sch, false);
                }
                if ui.button(t("🗑 Delete")).clicked() {
                    changed |= self.delete_selection(sch);
                }
            });
            return changed;
        }
        let obj = *self.selection.iter().next().unwrap();
        match obj {
            SchObj::Instance(id) => {
                let Some(before) = sch.instance(id).cloned() else {
                    self.selection.clear();
                    return false;
                };
                theme::section(ui, t(before.kind.label()));
                if before.kind.is_mos() {
                    let has_body = !before.kind.implicit_bulk();
                    let mut body = has_body;
                    ui.horizontal(|ui| {
                        ui.label(t("Symbol"));
                        ui.selectable_value(&mut body, true, t("4-terminal"))
                            .on_hover_text(t("The body is a pin to wire (unwired, it is tied to the source)"));
                        ui.selectable_value(&mut body, false, t("3-terminal")).on_hover_text(t(
                            "No body pin, the arrow on the source: the body connects to the net of the bulk parameter",
                        ));
                    });
                    if body != has_body {
                        let rail = self.bulk_net(before.kind).to_string();
                        let mut next = sch.clone();
                        match next.set_body_pin(id, body, Some(&rail)) {
                            Ok(true) => {
                                // Showing the pin labels it with the body's net: say so.
                                self.body_pin_msg = next
                                    .labels
                                    .iter()
                                    .find(|l| !sch.labels.iter().any(|o| o.id == l.id))
                                    .map(|l| (id, tf("Labelled the body pin '{}' to keep it on its net", &[&l.name]), false));
                                self.history.push(sch);
                                *sch = next;
                                return true;
                            }
                            Ok(false) => {}
                            Err(e) => self.body_pin_msg = Some((id, e, true)),
                        }
                    }
                    if let Some((_, msg, err)) = self.body_pin_msg.as_ref().filter(|(i, ..)| *i == id) {
                        let color = if *err { ui.visuals().error_fg_color } else { ui.visuals().weak_text_color() };
                        ui.label(egui::RichText::new(msg).small().color(color));
                    }
                }
                if before.kind.implicit_bulk() {
                    // An empty bulk means the project's default body net.
                    if let Some(b) = sch.instance_mut(id)
                        && b.param("bulk").trim().is_empty()
                        && !ui.ctx().memory(|m| m.focused().is_some())
                    {
                        b.params.insert("bulk".into(), self.bulk_net(before.kind).to_string());
                        return true;
                    }
                }
                if before.kind.is_mos() {
                    let url = "https://pages.euiyun.com/ic-studio/learn/mosfet.html#w-l-and-the-parameters-in-ic-studio";
                    ui.hyperlink_to(egui::RichText::new(t("What do W, L and M do? (beginner course)")).small(), url);
                    ui.add_space(4.0);
                }
                let mut inst = before.clone();
                let mut focus_gained = false;
                egui::Grid::new("inst_props").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    let l = ui.label(t("Name"));
                    let r = ui.add(egui::TextEdit::singleline(&mut inst.name).desired_width(140.0)).labelled_by(l.id);
                    focus_gained |= r.gained_focus();
                    ui.end_row();
                    for (k, _) in inst.kind.default_params() {
                        let help = crate::i18n::t(param_help(inst.kind, k));
                        let mut l = ui.label(crate::i18n::t(param_label(inst.kind, k)));
                        if !help.is_empty() {
                            l = l.on_hover_text(help);
                        }
                        let mut v = inst.params.get(k).cloned().unwrap_or_default();
                        let mut r = ui
                            .add(egui::TextEdit::singleline(&mut v).desired_width(140.0).hint_text(param_hint(inst.kind, k)))
                            .labelled_by(l.id);
                        if !help.is_empty() {
                            r = r.on_hover_text(help);
                        }
                        focus_gained |= r.gained_focus();
                        if r.changed() {
                            inst.params.insert(k.to_string(), v);
                        }
                        ui.end_row();
                    }
                    ui.label(t("Orientation"));
                    ui.horizontal(|ui| {
                        ui.label(format!("R{}{}", inst.orient.rot as u32 * 90, if inst.orient.mirror { " MY" } else { "" }));
                        if ui.small_button("⟳").on_hover_text(t("Rotate (Space)")).clicked() {
                            focus_gained = true;
                            inst.orient = inst.orient.rotated();
                        }
                        if ui.small_button("⇋").on_hover_text(t("Mirror (X)")).clicked() {
                            focus_gained = true;
                            inst.orient = inst.orient.mirrored();
                        }
                    });
                    ui.end_row();
                    ui.label(t("Position"));
                    ui.label(format!("({}, {})", inst.pos.x, inst.pos.y));
                    ui.end_row();
                });
                if inst.kind.spice_prefix().is_some() {
                    ui.add_space(8.0);
                    theme::section(ui, t("Connections"));
                    egui::Grid::new("inst_pins").num_columns(2).spacing([10.0, 3.0]).show(ui, |ui| {
                        for (pin, _) in inst.pin_positions() {
                            ui.label(egui::RichText::new(pin).monospace().color(pal.text_weak));
                            ui.label(egui::RichText::new(conn.net_of(id, pin).unwrap_or("?")).monospace().color(pal.wire));
                            ui.end_row();
                        }
                    });
                }
                if focus_gained {
                    self.history.push(sch);
                }
                if inst != before {
                    if let Some(target) = sch.instance_mut(id) {
                        *target = inst;
                    }
                    changed = true;
                }
            }
            SchObj::Label(id) => {
                theme::section(ui, t("Net Label"));
                if let Some(idx) = sch.labels.iter().position(|l| l.id == id) {
                    let mut name = sch.labels[idx].name.clone();
                    let lb = ui.label(t("Net name"));
                    let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(160.0)).labelled_by(lb.id);
                    if r.gained_focus() {
                        self.history.push(sch);
                    }
                    if r.changed() {
                        sch.labels[idx].name = name;
                        changed = true;
                    }
                    let mut port = sch.labels[idx].port;
                    let lb = ui.label(t("Pin"));
                    egui::ComboBox::from_id_salt("label_port")
                        .selected_text(port.map_or(t("not a pin"), PortDir::name))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut port, None, t("not a pin"));
                            for d in PortDir::ALL {
                                ui.selectable_value(&mut port, Some(d), d.name());
                            }
                        })
                        .response
                        .labelled_by(lb.id);
                    if port != sch.labels[idx].port {
                        self.history.push(sch);
                        sch.labels[idx].port = port;
                        changed = true;
                    }
                }
            }
            SchObj::Wire(id) => {
                theme::section(ui, t("Wire"));
                if let Some(w) = sch.wires.iter().find(|w| w.id == id) {
                    ui.label(format!("({}, {}) → ({}, {})", w.a.x, w.a.y, w.b.x, w.b.y));
                }
                if let Some(n) = conn.wire_net.get(&id) {
                    ui.horizontal(|ui| {
                        ui.label(t("Net"));
                        ui.label(egui::RichText::new(n).monospace().color(pal.highlight));
                    });
                }
            }
        }
        ui.add_space(10.0);
        if ui.button(t("🗑 Delete")).clicked() {
            changed |= self.delete_selection(sch);
        }
        changed
    }
}

fn bump_name(name: &str) -> String {
    let digits: String = name.chars().rev().take_while(|c| c.is_ascii_digit()).collect::<Vec<_>>().into_iter().rev().collect();
    if digits.is_empty() {
        return name.to_string();
    }
    let base = &name[..name.len() - digits.len()];
    format!("{base}{}", digits.parse::<u64>().unwrap_or(0) + 1)
}

fn param_label(kind: DeviceKind, k: &str) -> &str {
    match (kind, k) {
        (DeviceKind::Inductor, "l") => "Inductance",
        (_, "l") if kind.is_mos() => "Length (L)",
        (_, "w") if kind.is_mos() => "Width (W)",
        (_, "w") => "Width",
        (DeviceKind::Ccvs, "r") => "Transresistance",
        (_, "r") => "Resistance",
        (_, "c") => "Capacitance",
        (_, "dc") => "DC value",
        (_, "ac") => "AC magnitude",
        (_, "tran") => "Transient",
        (_, "model") => "Model",
        (_, "m") => "Multiplier (M)",
        (_, "bulk") => "Body net",
        (_, "net") => "Net name",
        (_, "area") => "Area factor",
        (_, "gain") => "Gain",
        (_, "gm") => "Transconductance",
        (_, "vctl") => "Controlling source",
        _ => k,
    }
}

/// What a parameter means and what changing it does, in words a beginner can act on.
pub fn param_help(kind: DeviceKind, k: &str) -> &'static str {
    match (kind, k) {
        (_, "w") if kind.is_mos() => {
            "Channel width. A wider transistor passes more current at the same voltages: faster \
             and higher transconductance (gm), but larger, with more capacitance and power. \
             Current scales about with W / L."
        }
        (_, "l") if kind.is_mos() => {
            "Channel length (the process minimum is 180n). A longer transistor passes less current \
             but behaves more like an ideal current source (higher output resistance, more gain) \
             and matches better. Short L is fast; long L is for gain, mirrors and matching."
        }
        (_, "m") if kind.is_mos() => {
            "Number of identical transistors in parallel. M = 2 acts like twice the width, and \
             copies of the same size match better than one wide device."
        }
        (_, "model") if kind.is_mos() => "The device model of the PDK (nmos / pmos, or a PDK model name).",
        (_, "bulk") => "The net the transistor body connects to: ground (0) for NMOS, the supply (vdd) for PMOS.",
        (DeviceKind::Resistor, "r") => "Resistance in ohms, e.g. 10k. Larger R gives less current and a slower RC.",
        (DeviceKind::Capacitor, "c") => "Capacitance in farads, e.g. 1p. Larger C stores more charge and slows the node down.",
        (DeviceKind::Inductor, "l") => "Inductance in henries, e.g. 1n.",
        (_, "dc") => "The constant value used by the operating point and DC sweep.",
        (_, "ac") => "Amplitude of the small signal for AC analysis. Set 1 on the input source to read gain directly.",
        (_, "tran") => {
            "The waveform for transient analysis, e.g. PULSE(low high delay rise fall width period) or SIN(offset amp freq)."
        }
        (_, "m") => "Number of identical devices in parallel.",
        _ => "",
    }
}

fn param_hint(kind: DeviceKind, k: &str) -> &'static str {
    match (kind, k) {
        (_, "tran") => "PULSE(0 1 0 1n 1n 5n 10n)",
        (_, "ac") => "e.g. 1",
        (_, "l") if kind.is_mos() => "180n",
        _ => "",
    }
}

/// Device library palette with mini symbol previews.
pub fn device_palette(ui: &mut egui::Ui, editor: &mut SchematicEditor) {
    let pal = theme::palette(ui.ctx());
    // Select / Wire / Label live on the canvas toolbar; only the label tool's net name is here.
    if editor.tool == SchTool::Label {
        theme::section(ui, t("Net Label"));
        ui.horizontal(|ui| {
            let l = ui.label(t("Net:"));
            ui.add(egui::TextEdit::singleline(&mut editor.label_text).desired_width(110.0)).labelled_by(l.id);
        });
        ui.add_space(6.0);
    }
    theme::section(ui, t("Device Library · analogLib"));
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("MOS").color(pal.text_weak));
        let mut body = editor.mos_body_pin;
        ui.selectable_value(&mut body, true, t("4-terminal")).on_hover_text(t("Place MOS devices with a body pin to wire"));
        ui.selectable_value(&mut body, false, t("3-terminal")).on_hover_text(t(
            "Place MOS devices without a body pin, the arrow on the source: the body is on ground (NMOS) or vdd (PMOS), set by the bulk parameter",
        ));
        if body != editor.mos_body_pin {
            editor.mos_body_pin = body;
            if let SchTool::Place(k) = editor.tool {
                editor.tool = SchTool::Place(k.with_body_pin(body));
            }
        }
    });
    if !editor.mos_body_pin {
        egui::Grid::new("bulk_nets").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            for (k, name) in ["NMOS body", "PMOS body"].into_iter().enumerate() {
                let l = ui.label(egui::RichText::new(t(name)).color(pal.text_weak));
                let hint = if k == 1 { "vdd" } else { "0" };
                ui.add(egui::TextEdit::singleline(&mut editor.bulk_nets[k]).desired_width(90.0).hint_text(hint))
                    .labelled_by(l.id)
                    .on_hover_text(t("Net the body of new 3-terminal devices connects to (a project setting)"));
                ui.end_row();
            }
        });
    }
    ui.add_space(4.0);
    let cell = Vec2::new(ui.available_width() / 2.0 - 4.0, 64.0);
    egui::Grid::new("devgrid").num_columns(2).spacing([6.0, 6.0]).show(ui, |ui| {
        let body = editor.mos_body_pin;
        for (i, kind) in DeviceKind::PALETTE.iter().map(|k| k.with_body_pin(body)).enumerate() {
            let kind = &kind;
            let active = editor.tool == SchTool::Place(*kind);
            let (r, painter) = ui.allocate_painter(cell, Sense::click());
            let fill = if active {
                pal.accent.gamma_multiply(0.25)
            } else if r.hovered() {
                pal.bg_hover
            } else {
                pal.bg_widget
            };
            painter.rect_filled(r.rect, 6.0, fill);
            if active {
                painter.rect_stroke(r.rect, 6.0, Stroke::new(1.0, pal.accent), StrokeKind::Inside);
            }
            let sym_rect = Rect::from_min_size(r.rect.min + Vec2::new(0.0, 2.0), Vec2::new(r.rect.width(), 44.0));
            let mut view = View2D::new(8.5, false, 1.0, 100.0);
            let b = kind.symbol().bbox;
            view.center = (((b.0 + b.2) / 2.0) as f64, ((b.1 + b.3) / 2.0) as f64);
            let inst = Instance {
                id: 0,
                kind: *kind,
                name: String::new(),
                pos: Pt::new(0, 0),
                orient: Orient::default(),
                params: Default::default(),
                cell: None,
                ports: vec![],
                symbol: None,
            };
            draw_symbol(&painter, &view, sym_rect, &inst, if active { pal.accent } else { pal.symbol }, 1.4);
            painter.text(
                r.rect.center_bottom() - Vec2::new(0.0, 4.0),
                Align2::CENTER_BOTTOM,
                // The MOS toggle above says which symbol; the tile keeps the short name.
                t(kind.with_body_pin(true).label()),
                FontId::proportional(11.0),
                pal.text_weak,
            );
            if let Some(c) = kind.with_body_pin(true).shortcut() {
                painter.text(
                    r.rect.right_top() + Vec2::new(-6.0, 4.0),
                    Align2::RIGHT_TOP,
                    c.to_string(),
                    FontId::monospace(10.0),
                    pal.text_weak.gamma_multiply(0.7),
                );
            }
            if r.clicked() {
                editor.set_tool(SchTool::Place(*kind));
            }
            if i % 2 == 1 {
                ui.end_row();
            }
        }
    });
    ui.add_space(8.0);
    theme::section(ui, t("Shortcuts"));
    for (k, d) in [
        ("Space / X", "rotate / mirror"),
        ("Del", "delete selection"),
        ("⇧⌘D", "duplicate"),
        ("⌘Z", "undo"),
        ("/", "wire corner direction"),
        ("F", "fit to window"),
        ("Right-click", "context menu"),
    ] {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(theme::kbd(k)).monospace().small().color(pal.accent));
            ui.label(egui::RichText::new(t(d)).small().color(pal.text_weak));
        });
    }
}
