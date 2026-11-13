//! Layout compare: the cell XOR-ed against its last saved version or a git revision, with the
//! added and removed geometry drawn on the canvas and stepped through one piece at a time.

use eframe::egui::{self, Rect, Stroke, StrokeKind};
use ics_core::geom::IRect;
use ics_core::hier::LayoutSource;
use ics_core::layout::Layout;
use ics_core::layout_diff::{self, LayoutDiff};
use ics_core::project::Project;

use super::{LayCtx, LayoutEditor, screen_rect};
use crate::canvas::View2D;
use crate::theme::{self, Palette};

pub struct CompareView {
    /// What the cell is compared with, e.g. "HEAD" or "saved".
    pub against: String,
    pub diff: LayoutDiff,
    pieces: Vec<(usize, bool, IRect)>,
    at: usize,
}

impl LayoutEditor {
    /// Compare the cell with `rev` (a git revision) or, for `None`, its saved version on disk.
    pub(super) fn compare_with(&mut self, l: &Layout, cx: &LayCtx, rev: Option<&str>) {
        let Some(root) = cx.src.root.clone() else {
            self.note(false, "Save the project first: there is nothing to compare with");
            return;
        };
        let old: Result<Project, String> = match rev {
            Some(r) => layout_diff::open_revision(&root, r),
            None => Project::open(&root).map_err(|e| e.to_string()),
        };
        let old = match old {
            Ok(p) => p,
            Err(e) => {
                self.note(false, format!("Compare: {}", e.lines().next().unwrap_or_default()));
                return;
            }
        };
        let empty = Layout::default();
        let before = old.layout(cx.cell).unwrap_or(&empty);
        let diff = layout_diff::diff(before, &old, l, cx.src);
        let against = rev.unwrap_or("saved").to_string();
        let pieces = diff.pieces();
        let msg = if diff.is_empty() {
            format!("No layout differences against {against}")
        } else {
            format!("{} differing piece(s) on {} layer(s) against {against}", pieces.len(), diff.layers.len())
        };
        self.note(true, msg);
        self.compare = Some(CompareView { against, diff, pieces, at: 0 });
        self.next_difference();
    }

    /// Zoom to the next differing piece.
    pub(super) fn next_difference(&mut self) {
        let Some(c) = self.compare.as_mut() else { return };
        let Some(&(_, _, b)) = c.pieces.get(c.at) else { return };
        c.at = (c.at + 1) % c.pieces.len().max(1);
        let g = (b.width().max(b.height()) * 2).max(2000);
        self.zoom_to(IRect::new(b.x0 - g, b.y0 - g, b.x1 + g, b.y1 + g));
    }

    /// Canvas overlay: added geometry green, removed red.
    pub(super) fn draw_compare(&self, painter: &egui::Painter, view: &View2D, rect: Rect, pal: &Palette) {
        let Some(c) = &self.compare else { return };
        let mut n = 0;
        for d in &c.diff.layers {
            for (region, color) in [(&d.added, pal.ok), (&d.removed, pal.danger)] {
                for r in region.rects() {
                    let sr = screen_rect(view, rect, *r);
                    if !sr.intersects(rect) || n > 20_000 {
                        continue;
                    }
                    n += 1;
                    painter.rect_filled(sr, 0.0, color.gamma_multiply(0.35));
                }
                for p in region.parts().iter().take(2000) {
                    if let Some(b) = p.bbox() {
                        let sr = screen_rect(view, rect, b).expand(2.0);
                        if sr.intersects(rect) {
                            painter.rect_stroke(sr, 1.0, Stroke::new(1.2, color), StrokeKind::Outside);
                        }
                    }
                }
            }
        }
        let text = if c.diff.is_empty() {
            format!("Compare with {}: identical", c.against)
        } else {
            format!("Compare with {}: {} piece(s)   green added, red removed", c.against, c.pieces.len())
        };
        painter.text(
            rect.left_bottom() + egui::vec2(10.0, -8.0),
            egui::Align2::LEFT_BOTTOM,
            text,
            egui::FontId::proportional(12.0),
            pal.text,
        );
    }

    /// Inspector section with the per-layer differences.
    pub(super) fn compare_panel(&mut self, ui: &mut egui::Ui, cx: &LayCtx) {
        let Some(c) = &self.compare else { return };
        ui.add_space(6.0);
        theme::section(ui, &format!("Compare with {}", c.against));
        let names = |l: usize| cx.tech.layers.get(l).map_or(format!("#{l}"), |d| d.name.clone());
        let lines = c.diff.summary(&names, cx.tech.dbu_per_um);
        if lines.is_empty() {
            ui.label("No differences");
        }
        for line in lines.iter().take(40) {
            ui.label(egui::RichText::new(line).small());
        }
        let mut next = false;
        let mut close = false;
        ui.horizontal(|ui| {
            next = !c.pieces.is_empty() && ui.button("Next difference").clicked();
            close = ui.button("Close").clicked();
        });
        if next {
            self.next_difference();
        }
        if close {
            self.compare = None;
        }
    }
}
