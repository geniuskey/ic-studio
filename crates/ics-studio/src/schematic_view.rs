//! Schematic workspace: canvas, editing tools, device palette and property inspector.

use std::collections::{BTreeSet, HashMap};

use eframe::egui::{self, Align2, Color32, FontId, Key, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use ics_core::connectivity::{Connectivity, extract};
use ics_core::geom::{Orient, Pt};
use ics_core::schematic::{DeviceKind, Instance, Prim, SchObj, Schematic};
use ics_core::units::format_eng;

use crate::canvas::View2D;
use crate::history::History;
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

enum Drag {
    Move { start: Pt, snapshot: Schematic, stretch: Vec<(u64, bool, bool)> },
    Rubber { start: Pos2 },
}

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
    let sym = inst.kind.symbol();
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
        let sym = inst.kind.symbol();
        // Label anchor: keep text upright, place on the side the anchor ends up on.
        let (lx, ly) = inst.to_world_f(sym.label_at.0, sym.label_at.1);
        let (cx, cy) = inst.to_world_f(0.0, 0.0);
        let align = if lx >= cx - 0.01 { Align2::LEFT_TOP } else { Align2::RIGHT_TOP };
        let p = screen(view, rect, lx, ly.min(cy + 10.0));
        let mut lines: Vec<String> = vec![];
        match inst.kind {
            DeviceKind::Ground => {}
            DeviceKind::Supply => lines.push(inst.param("net").to_string()),
            DeviceKind::Nmos | DeviceKind::Pmos => {
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
            painter.text(pp, align, format!("id={}", format_eng(*id, "A")), FontId::monospace(font.size * 0.8), pal.simulation);
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
                        new_sel.insert(SchObj::Label(sch.add_label(l.name, l.pos + d)));
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

    fn snap(&self, w: (f64, f64)) -> Pt {
        Pt::new(w.0.round() as i32, w.1.round() as i32)
    }

    fn handle_keys(&mut self, ui: &egui::Ui, sch: &mut Schematic) -> bool {
        if ui.ctx().egui_wants_keyboard_input() {
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
                    Key::D => changed |= self.duplicate_selection(sch),
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
                Key::Space | Key::X => {
                    let mirror = k == Key::X;
                    if let SchTool::Place(_) = self.tool {
                        self.place_orient = if mirror { self.place_orient.mirrored() } else { self.place_orient.rotated() };
                    } else {
                        changed |= self.rotate_selection(sch, mirror);
                    }
                }
                Key::T => self.set_tool(SchTool::Label),
                _ => {
                    for kind in DeviceKind::ALL {
                        let Some(c) = kind.shortcut() else { continue };
                        if Key::from_name(&c.to_string()) == Some(k) && !shift {
                            self.set_tool(SchTool::Place(kind));
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
        self.view.handle_navigation(ui, &resp, rect);
        if resp.hovered() {
            changed |= self.handle_keys(ui, sch);
        }

        let hover = resp.hover_pos();
        let world = hover.map(|p| self.view.to_world(rect, p));
        self.cursor = world.map(|w| self.snap(w));
        let tol = (6.0 / self.view.scale).max(0.25);

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
                    let id = sch.add_instance(kind, c, self.place_orient);
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
                    let ghost = Instance {
                        id: 0,
                        kind,
                        name: String::new(),
                        pos: c,
                        orient: self.place_orient,
                        params: Default::default(),
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
                "Empty schematic — pick a device from the library (or press R, C, V, N, P…) and click to place.\nW draws wires, T places net labels, scroll to zoom, right-drag to pan.",
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
        SchOutput { changed }
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
            theme::section(ui, "Schematic");
            egui::Grid::new("sch_stats").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                ui.label("Instances");
                ui.label(sch.instances.iter().filter(|i| i.kind.spice_prefix().is_some()).count().to_string());
                ui.end_row();
                ui.label("Wires");
                ui.label(sch.wires.len().to_string());
                ui.end_row();
                ui.label("Nets");
                ui.label(conn.nets().len().to_string());
                ui.end_row();
            });
            ui.add_space(8.0);
            theme::section(ui, "Nets");
            egui::ScrollArea::vertical().id_salt("nets").max_height(260.0).show(ui, |ui| {
                for n in conn.nets() {
                    let pins = conn.pin_nets.values().filter(|x| **x == n).count();
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&n).monospace().color(if n == "0" { pal.text_weak } else { pal.wire }));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new(format!("{pins} pins")).small().color(pal.text_weak));
                        });
                    });
                }
            });
            if !conn.warnings.is_empty() {
                theme::section(ui, "Warnings");
                for w in &conn.warnings {
                    ui.colored_label(pal.warning, w);
                }
            }
            return false;
        }
        if self.selection.len() > 1 {
            theme::section(ui, "Selection");
            ui.label(format!("{} objects selected", self.selection.len()));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("⟳ Rotate").clicked() {
                    changed |= self.rotate_selection(sch, false);
                }
                if ui.button("🗑 Delete").clicked() {
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
                theme::section(ui, before.kind.label());
                let mut inst = before.clone();
                let mut focus_gained = false;
                egui::Grid::new("inst_props").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    let l = ui.label("Name");
                    let r = ui.add(egui::TextEdit::singleline(&mut inst.name).desired_width(140.0)).labelled_by(l.id);
                    focus_gained |= r.gained_focus();
                    ui.end_row();
                    for (k, _) in inst.kind.default_params() {
                        let l = ui.label(param_label(k));
                        let mut v = inst.params.get(k).cloned().unwrap_or_default();
                        let r = ui
                            .add(egui::TextEdit::singleline(&mut v).desired_width(140.0).hint_text(param_hint(inst.kind, k)))
                            .labelled_by(l.id);
                        focus_gained |= r.gained_focus();
                        if r.changed() {
                            inst.params.insert(k.to_string(), v);
                        }
                        ui.end_row();
                    }
                    ui.label("Orientation");
                    ui.horizontal(|ui| {
                        ui.label(format!("R{}{}", inst.orient.rot as u32 * 90, if inst.orient.mirror { " MY" } else { "" }));
                        if ui.small_button("⟳").on_hover_text("Rotate (Space)").clicked() {
                            focus_gained = true;
                            inst.orient = inst.orient.rotated();
                        }
                        if ui.small_button("⇋").on_hover_text("Mirror (X)").clicked() {
                            focus_gained = true;
                            inst.orient = inst.orient.mirrored();
                        }
                    });
                    ui.end_row();
                    ui.label("Position");
                    ui.label(format!("({}, {})", inst.pos.x, inst.pos.y));
                    ui.end_row();
                });
                if inst.kind.spice_prefix().is_some() {
                    ui.add_space(8.0);
                    theme::section(ui, "Connections");
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
                theme::section(ui, "Net Label");
                if let Some(idx) = sch.labels.iter().position(|l| l.id == id) {
                    let mut name = sch.labels[idx].name.clone();
                    let lb = ui.label("Net name");
                    let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(160.0)).labelled_by(lb.id);
                    if r.gained_focus() {
                        self.history.push(sch);
                    }
                    if r.changed() {
                        sch.labels[idx].name = name;
                        changed = true;
                    }
                }
            }
            SchObj::Wire(id) => {
                theme::section(ui, "Wire");
                if let Some(w) = sch.wires.iter().find(|w| w.id == id) {
                    ui.label(format!("({}, {}) → ({}, {})", w.a.x, w.a.y, w.b.x, w.b.y));
                }
                if let Some(n) = conn.wire_net.get(&id) {
                    ui.horizontal(|ui| {
                        ui.label("Net");
                        ui.label(egui::RichText::new(n).monospace().color(pal.highlight));
                    });
                }
            }
        }
        ui.add_space(10.0);
        if ui.button("🗑 Delete").clicked() {
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

fn param_label(k: &str) -> &str {
    match k {
        "r" => "Resistance",
        "c" => "Capacitance",
        "l" => "Inductance / L",
        "dc" => "DC value",
        "ac" => "AC magnitude",
        "tran" => "Transient",
        "model" => "Model",
        "w" => "Width",
        "m" => "Multiplier",
        "net" => "Net name",
        _ => k,
    }
}

fn param_hint(kind: DeviceKind, k: &str) -> &'static str {
    match (kind, k) {
        (_, "tran") => "PULSE(0 1 0 1n 1n 5n 10n)",
        (_, "ac") => "e.g. 1",
        (DeviceKind::Nmos | DeviceKind::Pmos, "l") => "180n",
        _ => "",
    }
}

/// Device library palette with mini symbol previews.
pub fn device_palette(ui: &mut egui::Ui, editor: &mut SchematicEditor) {
    let pal = theme::palette(ui.ctx());
    theme::section(ui, "Tools");
    ui.horizontal_wrapped(|ui| {
        for (tool, label, key) in
            [(SchTool::Select, "Select", "Esc"), (SchTool::Wire, "Wire", "W"), (SchTool::Label, "Label", "T")]
        {
            if ui.selectable_label(editor.tool == tool, label).on_hover_text(format!("Shortcut: {key}")).clicked() {
                editor.set_tool(tool);
            }
        }
    });
    if editor.tool == SchTool::Label {
        ui.horizontal(|ui| {
            let l = ui.label("Net:");
            ui.add(egui::TextEdit::singleline(&mut editor.label_text).desired_width(110.0)).labelled_by(l.id);
        });
    }
    ui.add_space(6.0);
    theme::section(ui, "Device Library · analogLib");
    let cell = Vec2::new(ui.available_width() / 2.0 - 4.0, 64.0);
    egui::Grid::new("devgrid").num_columns(2).spacing([6.0, 6.0]).show(ui, |ui| {
        for (i, kind) in DeviceKind::ALL.iter().enumerate() {
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
            };
            draw_symbol(&painter, &view, sym_rect, &inst, if active { pal.accent } else { pal.symbol }, 1.4);
            painter.text(
                r.rect.center_bottom() - Vec2::new(0.0, 4.0),
                Align2::CENTER_BOTTOM,
                kind.label(),
                FontId::proportional(11.0),
                pal.text_weak,
            );
            if let Some(c) = kind.shortcut() {
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
    theme::section(ui, "Shortcuts");
    for (k, d) in [
        ("Space / X", "rotate / mirror"),
        ("Del", "delete selection"),
        ("⌘/Ctrl+D", "duplicate"),
        ("⌘/Ctrl+Z", "undo"),
        ("/", "wire corner direction"),
        ("F", "fit to window"),
    ] {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(k).monospace().small().color(pal.accent));
            ui.label(egui::RichText::new(d).small().color(pal.text_weak));
        });
    }
}
