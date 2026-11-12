//! Layout workspace: full-custom polygon editor with layer palette and basic DRC.

use std::collections::BTreeSet;

use eframe::egui::{self, Align2, Color32, FontId, Key, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use ics_core::geom::IRect;
use ics_core::layout::{DrcViolation, Geometry, Layout, Shape, run_basic_drc};
use ics_core::tech::{FillStyle, LayerDef, Tech};

use crate::canvas::{View2D, nice_step};
use crate::history::History;
use crate::theme::{self, Palette};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LayTool {
    Select,
    Rect,
    Path,
    Label,
    Ruler,
}

impl LayTool {
    pub fn name(self) -> &'static str {
        match self {
            LayTool::Select => "Select",
            LayTool::Rect => "Rectangle",
            LayTool::Path => "Path",
            LayTool::Label => "Label",
            LayTool::Ruler => "Ruler",
        }
    }
}

enum LDrag {
    Move { start: (i64, i64), snapshot: Layout },
    Rubber { start: Pos2 },
    Create { start: (i64, i64) },
    Ruler { start: (i64, i64) },
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
    pub drc: Option<Vec<DrcViolation>>,
    pub cursor: Option<(i64, i64)>,
    pub show_grid: bool,
}

impl Default for LayoutEditor {
    fn default() -> Self {
        Self {
            view: View2D::new(0.08, true, 1e-5, 50.0),
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
            drc: None,
            cursor: None,
            show_grid: true,
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

/// Fill a screen-space rect with the layer's stipple pattern (screen anchored so adjacent shapes line up).
fn pattern_rect(painter: &egui::Painter, r: Rect, def: &LayerDef, emphasis: f32) {
    let c = layer_color(def);
    let a = def.color[3] as f32 / 255.0;
    let base_alpha = match def.fill {
        FillStyle::Solid => 0.55,
        FillStyle::Outline => 0.0,
        _ => 0.18,
    } * a;
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
}

pub fn draw_layout(painter: &egui::Painter, view: &View2D, rect: Rect, layout: &Layout, tech: &Tech, o: &LayoutDrawOpts) {
    let pal = o.pal;
    if o.show_grid && !o.thumbnail {
        let step = nice_step(view.scale, 18.0, o.grid as f64);
        let (x0, y1) = view.to_world(rect, rect.min);
        let (x1, y0) = view.to_world(rect, rect.max);
        let mut x = (x0 / step).floor() * step;
        let mut n = 0;
        while x <= x1 && n < 400 {
            let major = ((x / step).round() as i64) % 5 == 0;
            let p = view.to_screen(rect, x, 0.0);
            painter.line_segment(
                [Pos2::new(p.x, rect.min.y), Pos2::new(p.x, rect.max.y)],
                Stroke::new(1.0, if major { pal.grid_major.gamma_multiply(0.7) } else { pal.grid.gamma_multiply(0.6) }),
            );
            x += step;
            n += 1;
        }
        let mut y = (y0 / step).floor() * step;
        n = 0;
        while y <= y1 && n < 400 {
            let major = ((y / step).round() as i64) % 5 == 0;
            let p = view.to_screen(rect, 0.0, y);
            painter.line_segment(
                [Pos2::new(rect.min.x, p.y), Pos2::new(rect.max.x, p.y)],
                Stroke::new(1.0, if major { pal.grid_major.gamma_multiply(0.7) } else { pal.grid.gamma_multiply(0.6) }),
            );
            y += step;
            n += 1;
        }
        // origin axes
        let o0 = view.to_screen(rect, 0.0, 0.0);
        let axis = Stroke::new(1.0, pal.text_weak.gamma_multiply(0.35));
        painter.line_segment([Pos2::new(o0.x, rect.min.y), Pos2::new(o0.x, rect.max.y)], axis);
        painter.line_segment([Pos2::new(rect.min.x, o0.y), Pos2::new(rect.max.x, o0.y)], axis);
    }
    let selected = |id: u64| o.selection.is_some_and(|s| s.contains(&id));
    let has_sel = o.selection.is_some_and(|s| !s.is_empty());
    let to_s = |x: f64, y: f64| view.to_screen(rect, x, y);
    for li in 0..tech.layers.len() {
        if !o.visible.get(li).copied().unwrap_or(true) {
            continue;
        }
        let def = &tech.layers[li];
        let c = layer_color(def);
        for s in layout.shapes.iter().filter(|s| s.layer == li) {
            let emphasis = if has_sel && !selected(s.id) { 0.75 } else { 1.0 };
            let sr = {
                let b = s.bbox();
                Rect::from_two_pos(to_s(b.x0 as f64, b.y0 as f64), to_s(b.x1 as f64, b.y1 as f64))
            };
            if !sr.intersects(rect) {
                continue;
            }
            match &s.geom {
                Geometry::Rect(_) => {
                    if o.thumbnail {
                        painter.rect_filled(sr, 0.0, c.gamma_multiply(0.45));
                        painter.rect_stroke(sr, 0.0, Stroke::new(0.6, c), StrokeKind::Inside);
                    } else {
                        pattern_rect(painter, sr, def, emphasis);
                    }
                }
                Geometry::Polygon { points } => {
                    let pts: Vec<Pos2> = points.iter().map(|p| to_s(p.0 as f64, p.1 as f64)).collect();
                    painter.add(egui::Shape::closed_line(pts, Stroke::new(1.2, c)));
                }
                Geometry::Path { points, width } => {
                    for q in path_quads(points, *width) {
                        let pts: Vec<Pos2> = q.iter().map(|p| to_s(p.0, p.1)).collect();
                        painter.add(egui::Shape::convex_polygon(
                            pts,
                            c.gamma_multiply(0.35 * emphasis),
                            Stroke::new(1.0, c.gamma_multiply(emphasis)),
                        ));
                    }
                    if !o.thumbnail {
                        let center: Vec<Pos2> = points.iter().map(|p| to_s(p.0 as f64, p.1 as f64)).collect();
                        painter.add(egui::Shape::dashed_line(&center, Stroke::new(1.0, c.gamma_multiply(0.6)), 4.0, 4.0));
                    }
                }
                Geometry::Label { text, at, size } => {
                    if !o.thumbnail {
                        let p = to_s(at.0 as f64, at.1 as f64);
                        let fs = ((*size as f64 * view.scale) as f32).clamp(8.0, 64.0);
                        painter.text(
                            p,
                            Align2::LEFT_BOTTOM,
                            text,
                            FontId::proportional(fs),
                            c.gamma_multiply(emphasis).to_opaque(),
                        );
                        painter.line_segment([p - Vec2::new(3.0, 0.0), p + Vec2::new(3.0, 0.0)], Stroke::new(1.0, c));
                        painter.line_segment([p - Vec2::new(0.0, 3.0), p + Vec2::new(0.0, 3.0)], Stroke::new(1.0, c));
                    }
                }
            }
            if selected(s.id) {
                painter.rect_stroke(sr.expand(1.5), 0.0, Stroke::new(2.0, pal.selection), StrokeKind::Outside);
            }
        }
    }
}

fn hit(layout: &Layout, visible: &[bool], x: i64, y: i64, tol: i64) -> Option<u64> {
    // Topmost layer first, then latest shape.
    let mut best: Option<(usize, usize, u64)> = None;
    for s in &layout.shapes {
        if !visible.get(s.layer).copied().unwrap_or(true) {
            continue;
        }
        if s.hit(x, y, tol) {
            // Prefer smaller shapes when overlapping (more specific), then higher layers.
            let area = (s.bbox().width().max(1) as f64 * s.bbox().height().max(1) as f64) as usize;
            let key = (usize::MAX - area, s.layer, s.id);
            if best.is_none_or(|b| (key.0, key.1) > (b.0, b.1)) {
                best = Some(key);
            }
        }
    }
    best.map(|b| b.2)
}

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

    pub fn request_fit(&mut self) {
        self.needs_fit = true;
    }

    pub fn set_tool(&mut self, t: LayTool) {
        self.tool = t;
        self.path_pts.clear();
        if t != LayTool::Ruler {
            self.ruler = None;
        }
    }

    pub fn undo(&mut self, l: &mut Layout) -> bool {
        self.selection.clear();
        self.history.undo(l)
    }

    pub fn redo(&mut self, l: &mut Layout) -> bool {
        self.selection.clear();
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
            if let Some(s) = l.shape_mut(*id) {
                s.translate(dx, dy);
            }
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

    fn handle_keys(&mut self, ui: &egui::Ui, l: &mut Layout, tech: &Tech) -> bool {
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
                    Key::A => self.selection = l.shapes.iter().map(|s| s.id).collect(),
                    Key::D => changed |= self.duplicate(l, tech.grid),
                    _ => {}
                }
                continue;
            }
            match k {
                Key::Escape => {
                    if !self.path_pts.is_empty() {
                        self.path_pts.clear();
                    } else if self.tool != LayTool::Select {
                        self.set_tool(LayTool::Select);
                    } else {
                        self.selection.clear();
                    }
                }
                Key::Enter => changed |= self.finish_path(l, tech),
                Key::Delete | Key::Backspace => {
                    if !self.path_pts.is_empty() {
                        self.path_pts.pop();
                    } else {
                        changed |= self.delete_selection(l);
                    }
                }
                Key::R => self.set_tool(LayTool::Rect),
                Key::P => self.set_tool(LayTool::Path),
                Key::L => self.set_tool(LayTool::Label),
                Key::K => self.set_tool(LayTool::Ruler),
                Key::S => self.set_tool(LayTool::Select),
                Key::F => self.needs_fit = true,
                Key::ArrowLeft => changed |= self.nudge(l, -g, 0),
                Key::ArrowRight => changed |= self.nudge(l, g, 0),
                Key::ArrowUp => changed |= self.nudge(l, 0, g),
                Key::ArrowDown => changed |= self.nudge(l, 0, -g),
                _ => {}
            }
        }
        changed
    }

    pub fn canvas(&mut self, ui: &mut egui::Ui, l: &mut Layout, tech: &Tech) -> LayOutput {
        self.sync_layers(tech);
        let pal = theme::palette(ui.ctx());
        let mut changed = false;
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = resp.rect;
        let painter = painter.with_clip_rect(rect);
        painter.rect_filled(rect, 0.0, if pal.dark { Color32::from_rgb(0x0e, 0x10, 0x13) } else { pal.bg_canvas });
        if self.needs_fit {
            match l.bbox() {
                Some(b) => self.view.fit(rect, (b.x0 as f64, b.y0 as f64), (b.x1 as f64, b.y1 as f64), 50.0),
                None => {
                    self.view.scale = 0.08;
                    self.view.center = (2500.0, 2500.0);
                }
            }
            self.needs_fit = false;
        }
        self.view.handle_navigation(ui, &resp, rect);
        if resp.hovered() {
            changed |= self.handle_keys(ui, l, tech);
        }
        let hover = resp.hover_pos();
        let world = hover.map(|p| self.view.to_world(rect, p));
        self.cursor = world.map(|w| self.snap(w, tech.grid));
        let tol = (5.0 / self.view.scale) as i64;
        let shift = ui.input(|i| i.modifiers.shift || i.modifiers.command);

        match self.tool {
            LayTool::Select => {
                if resp.drag_started_by(egui::PointerButton::Primary) {
                    let press = ui.input(|i| i.pointer.press_origin()).unwrap_or(rect.center());
                    let pw = self.view.to_world(rect, press);
                    let (px, py) = (pw.0 as i64, pw.1 as i64);
                    match hit(l, &self.visible, px, py, tol) {
                        Some(id) => {
                            if !self.selection.contains(&id) {
                                if !shift {
                                    self.selection.clear();
                                }
                                self.selection.insert(id);
                            }
                            self.drag = Some(LDrag::Move { start: self.snap(pw, tech.grid), snapshot: l.clone() });
                        }
                        None => self.drag = Some(LDrag::Rubber { start: press }),
                    }
                }
                if resp.clicked()
                    && let Some(w) = world
                {
                    match hit(l, &self.visible, w.0 as i64, w.1 as i64, tol) {
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
            LayTool::Rect => {
                if resp.drag_started_by(egui::PointerButton::Primary) {
                    let press = ui.input(|i| i.pointer.press_origin()).unwrap_or(rect.center());
                    self.drag = Some(LDrag::Create { start: self.snap(self.view.to_world(rect, press), tech.grid) });
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
                    if resp.double_clicked() {
                        changed |= self.finish_path(l, tech);
                    }
                }
                if resp.secondary_clicked() {
                    changed |= self.finish_path(l, tech);
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
                        changed = true;
                    }
                }
            }
        }

        // Drag updates
        if let Some(w) = world {
            let c = self.snap(w, tech.grid);
            match &self.drag {
                Some(LDrag::Move { start, snapshot }) => {
                    let (dx, dy) = (c.0 - start.0, c.1 - start.1);
                    let mut next = snapshot.clone();
                    for id in &self.selection {
                        if let Some(s) = next.shape_mut(*id) {
                            s.translate(dx, dy);
                        }
                    }
                    if next != *l {
                        *l = next;
                    }
                }
                Some(LDrag::Ruler { start }) => self.ruler = Some((*start, c)),
                _ => {}
            }
        }
        if resp.drag_stopped() {
            match self.drag.take() {
                Some(LDrag::Move { snapshot, .. }) => {
                    if snapshot != *l {
                        self.history.push(&snapshot);
                        changed = true;
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
                        changed = true;
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
                        for s in &l.shapes {
                            let bb = s.bbox();
                            if self.visible.get(s.layer).copied().unwrap_or(true)
                                && bb.x0 >= r.x0
                                && bb.x1 <= r.x1
                                && bb.y0 >= r.y0
                                && bb.y1 <= r.y1
                            {
                                self.selection.insert(s.id);
                            }
                        }
                    }
                }
                Some(LDrag::Ruler { .. }) | None => {}
            }
        }

        draw_layout(
            &painter,
            &self.view,
            rect,
            l,
            tech,
            &LayoutDrawOpts {
                pal,
                visible: &self.visible,
                selection: Some(&self.selection),
                thumbnail: false,
                show_grid: self.show_grid,
                grid: tech.grid,
            },
        );

        // Overlays: DRC markers
        if let Some(v) = &self.drc {
            for x in v {
                let r = Rect::from_two_pos(
                    self.view.to_screen(rect, x.area.x0 as f64, x.area.y0 as f64),
                    self.view.to_screen(rect, x.area.x1 as f64, x.area.y1 as f64),
                )
                .expand(3.0);
                painter.rect_stroke(r, 2.0, Stroke::new(2.0, pal.danger), StrokeKind::Outside);
            }
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
        if l.shapes.is_empty() {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "Empty layout — choose a layer on the left, then press R and drag to draw a rectangle.\nP draws paths, L places labels, K measures, scroll to zoom, right-drag to pan.",
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
        LayOutput { changed }
    }

    pub fn layer_palette(&mut self, ui: &mut egui::Ui, tech: &Tech) {
        self.sync_layers(tech);
        let pal = theme::palette(ui.ctx());
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
    }

    pub fn inspector(&mut self, ui: &mut egui::Ui, l: &mut Layout, tech: &Tech) -> bool {
        let pal = theme::palette(ui.ctx());
        let mut changed = false;
        if self.selection.len() == 1 {
            let id = *self.selection.iter().next().unwrap();
            let Some(before) = l.shape(id).cloned() else {
                self.selection.clear();
                return false;
            };
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
        } else if self.selection.len() > 1 {
            theme::section(ui, "Selection");
            ui.label(format!("{} shapes selected", self.selection.len()));
            if ui.button("🗑 Delete").clicked() {
                changed |= self.delete_selection(l);
            }
        } else {
            theme::section(ui, "Cell");
            egui::Grid::new("lay_stats").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                ui.label("Shapes");
                ui.label(l.shapes.len().to_string());
                ui.end_row();
                if let Some(b) = l.bbox() {
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
        ui.add_space(12.0);
        theme::section(ui, "Design Rule Check");
        ui.horizontal(|ui| {
            if ui.button("▶ Run DRC").on_hover_text("Built-in min width / spacing checks").clicked() {
                self.drc = Some(run_basic_drc(l, tech));
            }
            if self.drc.is_some() && ui.button("Clear").clicked() {
                self.drc = None;
            }
        });
        let mut zoom_to: Option<IRect> = None;
        if let Some(v) = &self.drc {
            if v.is_empty() {
                ui.colored_label(pal.ok, "✔ No violations");
            } else {
                ui.colored_label(pal.danger, format!("{} violation(s)", v.len()));
                egui::ScrollArea::vertical().id_salt("drc").max_height(220.0).show(ui, |ui| {
                    for x in v {
                        if ui
                            .add(egui::Button::new(egui::RichText::new(&x.rule).small()).frame(false))
                            .on_hover_text("Click to zoom")
                            .clicked()
                        {
                            zoom_to = Some(x.area);
                        }
                    }
                });
            }
        }
        if let Some(a) = zoom_to {
            let c = ((a.x0 + a.x1) as f64 / 2.0, (a.y0 + a.y1) as f64 / 2.0);
            self.view.center = c;
            self.view.scale = self.view.scale.max(0.3);
        }
        changed
    }
}

fn manhattan(from: (i64, i64), to: (i64, i64)) -> (i64, i64) {
    if (to.0 - from.0).abs() >= (to.1 - from.1).abs() { (to.0, from.1) } else { (from.0, to.1) }
}
