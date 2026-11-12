//! Shared 2D canvas view transform with pan / zoom handling.

use eframe::egui::{self, Pos2, Rect, Vec2};

#[derive(Clone, Copy, Debug)]
pub struct View2D {
    /// World coordinate shown at the centre of the canvas.
    pub center: (f64, f64),
    /// Screen pixels per world unit.
    pub scale: f64,
    /// World y axis points up (layout) instead of down (schematic).
    pub y_up: bool,
    pub min_scale: f64,
    pub max_scale: f64,
}

impl View2D {
    pub fn new(scale: f64, y_up: bool, min_scale: f64, max_scale: f64) -> Self {
        Self { center: (0.0, 0.0), scale, y_up, min_scale, max_scale }
    }

    pub fn to_screen(&self, rect: Rect, x: f64, y: f64) -> Pos2 {
        let c = rect.center();
        let dy = (y - self.center.1) * self.scale;
        egui::pos2(c.x + ((x - self.center.0) * self.scale) as f32, if self.y_up { c.y - dy as f32 } else { c.y + dy as f32 })
    }

    pub fn to_world(&self, rect: Rect, p: Pos2) -> (f64, f64) {
        let c = rect.center();
        let x = (p.x - c.x) as f64 / self.scale + self.center.0;
        let dy = (p.y - c.y) as f64 / self.scale;
        let y = if self.y_up { self.center.1 - dy } else { self.center.1 + dy };
        (x, y)
    }

    /// Zoom by `factor` keeping the world point under `anchor` fixed.
    pub fn zoom_at(&mut self, rect: Rect, anchor: Pos2, factor: f64) {
        let before = self.to_world(rect, anchor);
        self.scale = (self.scale * factor).clamp(self.min_scale, self.max_scale);
        let after = self.to_world(rect, anchor);
        self.center.0 += before.0 - after.0;
        self.center.1 += before.1 - after.1;
    }

    pub fn pan_pixels(&mut self, d: Vec2) {
        self.center.0 -= d.x as f64 / self.scale;
        let dy = d.y as f64 / self.scale;
        if self.y_up {
            self.center.1 += dy;
        } else {
            self.center.1 -= dy;
        }
    }

    /// Fit a world-space bounding box into the canvas with a margin.
    pub fn fit(&mut self, rect: Rect, min: (f64, f64), max: (f64, f64), margin: f32) {
        let w = (max.0 - min.0).max(1e-9);
        let h = (max.1 - min.1).max(1e-9);
        let sx = (rect.width() - 2.0 * margin).max(10.0) as f64 / w;
        let sy = (rect.height() - 2.0 * margin).max(10.0) as f64 / h;
        self.scale = sx.min(sy).clamp(self.min_scale, self.max_scale);
        self.center = ((min.0 + max.0) / 2.0, (min.1 + max.1) / 2.0);
    }

    /// Standard EDA-style navigation: wheel / pinch zoom, middle or right drag pan,
    /// shift+wheel / trackpad horizontal scroll pans. Returns true if the view changed.
    pub fn handle_navigation(&mut self, ui: &egui::Ui, resp: &egui::Response, rect: Rect) -> bool {
        let mut changed = false;
        if resp.dragged_by(egui::PointerButton::Middle) || resp.dragged_by(egui::PointerButton::Secondary) {
            self.pan_pixels(resp.drag_delta());
            changed = true;
        }
        if resp.hovered() {
            let (scroll, zoom, shift, hover) =
                ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.modifiers.shift, i.pointer.hover_pos()));
            let anchor = hover.unwrap_or(rect.center());
            if zoom != 1.0 {
                self.zoom_at(rect, anchor, zoom as f64);
                changed = true;
            }
            if shift || scroll.x.abs() > scroll.y.abs() {
                if scroll != Vec2::ZERO {
                    self.pan_pixels(scroll);
                    changed = true;
                }
            } else if scroll.y != 0.0 {
                self.zoom_at(rect, anchor, (scroll.y as f64 * 0.0025).exp());
                changed = true;
            }
        }
        changed
    }
}

/// Pick a "nice" grid step (1, 2, 5 × 10^n) so that lines are at least `min_px` apart.
pub fn nice_step(scale: f64, min_px: f64, base: f64) -> f64 {
    let raw = min_px / scale;
    let mut step = base;
    while step < raw {
        for m in [2.0, 2.5, 2.0] {
            step *= m;
            if step >= raw {
                break;
            }
        }
    }
    step
}
