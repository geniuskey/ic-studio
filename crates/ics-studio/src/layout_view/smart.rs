//! Constraint-driven editing: symmetry / alignment / fixed constraints kept through every edit,
//! inferred from the schematic, drawn on the canvas, and compaction that respects them.

use std::collections::BTreeSet;

use eframe::egui::{self, Rect, Stroke};
use ics_core::compact::{self, CompactOptions, Dir};
use ics_core::constraints::{self, Axis, Constraint, Edge};
use ics_core::layout::Layout;

use super::{LayCtx, LayoutEditor};
use crate::canvas::View2D;
use crate::theme::{self, Palette};

/// Items whose geometry or placement differs between `a` and `b` (present in both).
fn moved_items(a: &Layout, b: &Layout) -> BTreeSet<u64> {
    let mut out = BTreeSet::new();
    for s in &b.shapes {
        if a.shape(s.id).is_some_and(|o| o.geom != s.geom) {
            out.insert(s.id);
        }
    }
    for i in &b.instances {
        if a.instance(i.id).is_some_and(|o| o.trans != i.trans || o.array != i.array) {
            out.insert(i.id);
        }
    }
    out
}

impl LayoutEditor {
    /// After an edit: move the partners of the items it moved so every constraint still holds.
    /// The adjustment belongs to the same undo step.
    pub(super) fn keep_constraints(&mut self, l: &mut Layout, cx: &mut LayCtx) {
        if l.constraints.is_empty() {
            return;
        }
        let Some(prev) = self.history.last() else { return };
        let moved = moved_items(prev, l);
        if moved.is_empty() {
            return;
        }
        let followed = constraints::enforce(l, cx.src, &moved);
        if !followed.is_empty() {
            cx.touched();
            self.note(true, format!("Constraints: {} item(s) followed", followed.len()));
        }
    }

    fn add_constraint(&mut self, l: &mut Layout, c: Constraint) {
        self.history.push(l);
        l.constraints.push(c);
    }

    pub(super) fn constrain_symmetry(&mut self, l: &mut Layout, cx: &mut LayCtx, axis: Axis) -> bool {
        if self.selection.len() < 2 {
            self.note(false, "Select the items to make symmetric");
            return false;
        }
        let Some(c) = constraints::symmetry_of(l, cx.src, &self.selection, axis) else {
            self.note(false, "No symmetric pairing: every item needs a partner near its mirror image");
            return false;
        };
        let desc = c.describe(cx.tech.dbu_per_um);
        self.add_constraint(l, c);
        let k = l.constraints.len() - 1;
        constraints::snap(l, cx.src, k);
        self.note(true, desc);
        true
    }

    pub(super) fn constrain_align(&mut self, l: &mut Layout, cx: &mut LayCtx, edge: Edge) -> bool {
        if self.selection.len() < 2 {
            self.note(false, "Select the items to align");
            return false;
        }
        let c = Constraint::Align { edge, items: self.selection.iter().copied().collect() };
        let desc = c.describe(cx.tech.dbu_per_um);
        self.add_constraint(l, c);
        let k = l.constraints.len() - 1;
        constraints::snap(l, cx.src, k);
        self.note(true, desc);
        true
    }

    pub(super) fn constrain_fixed(&mut self, l: &mut Layout) -> bool {
        if self.selection.is_empty() {
            self.note(false, "Select the items to fix in place");
            return false;
        }
        let c = Constraint::Fixed { items: self.selection.iter().copied().collect() };
        self.add_constraint(l, c);
        self.note(true, format!("Fixed {} item(s) in place", self.selection.len()));
        true
    }

    pub(super) fn infer_constraints(&mut self, l: &mut Layout, cx: &mut LayCtx) -> bool {
        let Some(sch) = cx.src.cell(cx.cell).and_then(|c| c.schematic.as_ref()) else {
            self.note(false, "Inferring constraints needs a schematic view of this cell");
            return false;
        };
        let before = l.clone();
        let n = constraints::infer(l, cx.src, sch);
        if n > 0 {
            let after = std::mem::replace(l, before);
            self.history.push(l);
            *l = after;
        }
        self.note(n > 0, format!("Inferred {n} symmetry constraint(s) from matched devices"));
        n > 0
    }

    /// Remove the constraints of the selected items (all constraints with nothing selected).
    pub(super) fn clear_constraints(&mut self, l: &mut Layout) -> bool {
        let keep: Vec<Constraint> = if self.selection.is_empty() {
            vec![]
        } else {
            l.constraints.iter().filter(|c| !c.items().iter().any(|i| self.selection.contains(i))).cloned().collect()
        };
        if keep.len() == l.constraints.len() {
            return false;
        }
        self.history.push(l);
        let n = l.constraints.len() - keep.len();
        l.constraints = keep;
        self.note(true, format!("Removed {n} constraint(s)"));
        true
    }

    /// Compact the selection (or the whole cell) toward `dirs` in turn, as one undoable edit.
    pub(super) fn compact(&mut self, l: &mut Layout, cx: &mut LayCtx, dirs: &[Dir]) -> bool {
        let deck = self.live_deck(cx);
        let opts = CompactOptions { only: (!self.selection.is_empty()).then(|| self.selection.clone()) };
        let mut after = l.clone();
        let mut moved = 0;
        let (mut first, mut last) = (None, None);
        for &d in dirs {
            let rep = compact::compact(&mut after, cx.src, cx.tech, deck.as_deref(), d, &opts);
            moved += rep.moved;
            first = first.or(rep.before);
            last = rep.after.or(last);
        }
        let ext = first.zip(last);
        if after == *l {
            self.note(false, "Compaction: nothing can move closer");
            return false;
        }
        self.history.push(l);
        *l = after;
        let area = |r: ics_core::geom::IRect| r.width() as f64 * r.height() as f64;
        let msg = match ext {
            Some((b, a)) if area(b) > 0.0 => {
                format!("Compacted: {moved} group move(s), area {:.0}% of before", 100.0 * area(a) / area(b))
            }
            _ => format!("Compacted: {moved} group move(s)"),
        };
        self.note(true, msg);
        true
    }

    /// Canvas overlay: symmetry axes with pair links, aligned edges, fixed items.
    pub(super) fn draw_constraints(
        &self,
        painter: &egui::Painter,
        view: &View2D,
        rect: Rect,
        pal: &Palette,
        l: &Layout,
        cx: &LayCtx,
    ) {
        if !self.show_constraints || l.constraints.is_empty() {
            return;
        }
        let boxes = constraints::item_boxes(l, cx.src);
        let errs = constraints::errors(l, cx.src);
        let p = |x: i64, y: i64| view.to_screen(rect, x as f64, y as f64);
        let mid = |b: ics_core::geom::IRect| p((b.x0 + b.x1) / 2, (b.y0 + b.y1) / 2);
        for (c, err) in l.constraints.iter().zip(errs) {
            let color = if err > 0 { pal.warning } else { pal.ok };
            let st = Stroke::new(1.6, color);
            let ext = c.items().iter().filter_map(|i| boxes.get(i)).copied().reduce(|a, b| a.union(&b));
            let Some(ext) = ext else { continue };
            match c {
                Constraint::Symmetry { axis, at, pairs, .. } => {
                    let g = ext.width().max(ext.height()) / 10;
                    let (a, b) = match axis {
                        Axis::Vertical => (p(*at, ext.y0 - g), p(*at, ext.y1 + g)),
                        Axis::Horizontal => (p(ext.x0 - g, *at), p(ext.x1 + g, *at)),
                    };
                    painter.add(egui::Shape::dashed_line(&[a, b], Stroke::new(2.2, color), 8.0, 4.0));
                    for (x, y) in pairs {
                        if let (Some(bx), Some(by)) = (boxes.get(x), boxes.get(y)) {
                            painter.add(egui::Shape::dashed_line(&[mid(*bx), mid(*by)], st, 3.0, 3.0));
                            painter.circle_stroke(mid(*bx), 3.0, st);
                            painter.circle_stroke(mid(*by), 3.0, st);
                        }
                    }
                }
                Constraint::Align { edge, items } => {
                    let v = items.iter().filter_map(|i| boxes.get(i)).map(|b| edge.of(*b)).next().unwrap_or(0);
                    let (a, b) = if edge.horizontal() { (p(v, ext.y0), p(v, ext.y1)) } else { (p(ext.x0, v), p(ext.x1, v)) };
                    painter.add(egui::Shape::dashed_line(&[a, b], st, 6.0, 3.0));
                }
                Constraint::Fixed { items } => {
                    for b in items.iter().filter_map(|i| boxes.get(i)) {
                        let c = p(b.x0, b.y1);
                        let r = Rect::from_min_size(c + egui::vec2(2.0, 2.0), egui::vec2(7.0, 7.0));
                        painter.rect_filled(r, 1.0, color);
                    }
                }
            }
        }
    }

    /// Inspector section listing the cell's constraints. Returns true if the layout changed.
    pub(super) fn constraints_panel(&mut self, ui: &mut egui::Ui, l: &mut Layout, cx: &mut LayCtx) -> bool {
        if l.constraints.is_empty() {
            return false;
        }
        ui.add_space(6.0);
        theme::section(ui, "Constraints");
        let errs = constraints::errors(l, cx.src);
        let mut remove = None;
        let mut select = None;
        let mut snap = None;
        for (k, (c, err)) in l.constraints.iter().zip(errs).enumerate() {
            ui.horizontal(|ui| {
                let mut text = egui::RichText::new(c.describe(cx.tech.dbu_per_um)).small();
                if err > 0 {
                    text = text.color(theme::palette(ui.ctx()).warning);
                }
                if ui.link(text).on_hover_text("Select its items").clicked() {
                    select = Some(k);
                }
                if err > 0 && ui.small_button("Snap").on_hover_text(format!("Off by {} nm; move the items back", err)).clicked() {
                    snap = Some(k);
                }
                if ui.small_button("x").on_hover_text("Remove the constraint").clicked() {
                    remove = Some(k);
                }
            });
        }
        if let Some(k) = select {
            self.selection = l.constraints[k].items().into_iter().collect();
        }
        if let Some(k) = snap {
            self.history.push(l);
            constraints::snap(l, cx.src, k);
            return true;
        }
        if let Some(k) = remove {
            self.history.push(l);
            l.constraints.remove(k);
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moved_items_ignores_added_and_removed() {
        let mut a = Layout::default();
        let x = a.add_rect(1, 0, 0, 10, 10);
        let y = a.add_rect(1, 20, 0, 30, 10);
        let mut b = a.clone();
        b.translate_item(x, 5, 0);
        b.remove(y);
        b.add_rect(1, 50, 0, 60, 10);
        assert_eq!(moved_items(&a, &b), [x].into());
    }
}
