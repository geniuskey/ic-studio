//! Live DRC: the rule deck re-checked around every edit on a worker thread, violations shown
//! as they appear, and quick fixes (Ctrl+.) that are themselves verified before they are offered.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use eframe::egui::{self, Pos2, Rect, Stroke, StrokeKind, Vec2};
use ics_core::drc::{self, Deck, DrcSession, Violation};
use ics_core::drc_fix::{self, Fix};
use ics_core::geom::IRect;
use ics_core::layout::{CellInst, Layout, Shape};
use ics_mcp::tools;

use super::{LayCtx, LayoutEditor, screen_rect};
use crate::canvas::View2D;
use crate::theme::Palette;

/// Quiet time after an edit before the check starts.
const DEBOUNCE: Duration = Duration::from_millis(120);
/// Fix-all gives up after this many rounds.
const FIX_ROUNDS: usize = 64;

struct LiveJob {
    rx: Receiver<DrcSession>,
}

pub struct LiveDrc {
    pub enabled: bool,
    /// Violations of the layout as last checked (top-cell coordinates).
    pub violations: Vec<Violation>,
    /// Incremental checker of the layout as last checked (out on the worker while it runs).
    session: Option<DrcSession>,
    /// Layout the violations were computed for (diffed against the next edit).
    base: Option<Layout>,
    seen: u64,
    /// Area changed since the last check started.
    dirty: Option<IRect>,
    due: Option<Instant>,
    job: Option<LiveJob>,
    deck: Option<(String, Arc<Deck>)>,
    pub millis: f64,
    /// The quick-fix form: the violation and its fixes.
    pub quick: Option<(Violation, Vec<Fix>)>,
    quick_at: Option<Pos2>,
}

impl Default for LiveDrc {
    fn default() -> Self {
        Self {
            enabled: true,
            violations: vec![],
            session: None,
            base: None,
            seen: 0,
            dirty: None,
            due: None,
            job: None,
            deck: None,
            millis: 0.0,
            quick: None,
            quick_at: None,
        }
    }
}

fn union(a: Option<IRect>, b: IRect) -> Option<IRect> {
    Some(a.map_or(b, |a| a.union(&b)))
}

/// Area where `a` and `b` differ: every added, removed or changed shape and instance.
fn diff(a: &Layout, b: &Layout, inst_box: &mut impl FnMut(&CellInst) -> IRect) -> Option<IRect> {
    let mut out = None;
    fn shapes(l: &Layout) -> HashMap<u64, &Shape> {
        l.shapes.iter().map(|s| (s.id, s)).collect()
    }
    fn insts(l: &Layout) -> HashMap<u64, &CellInst> {
        l.instances.iter().map(|i| (i.id, i)).collect()
    }
    let (sa, sb) = (shapes(a), shapes(b));
    for (id, s) in &sa {
        match sb.get(id) {
            Some(t) if t.layer == s.layer && t.geom == s.geom => {}
            Some(t) => out = union(union(out, s.bbox()), t.bbox()),
            None => out = union(out, s.bbox()),
        }
    }
    for (id, t) in &sb {
        if !sa.contains_key(id) {
            out = union(out, t.bbox());
        }
    }
    let (ia, ib) = (insts(a), insts(b));
    for (id, i) in &ia {
        match ib.get(id) {
            Some(j) if j == i => {}
            Some(j) => out = union(union(out, inst_box(i)), inst_box(j)),
            None => out = union(out, inst_box(i)),
        }
    }
    for (id, j) in &ib {
        if !ia.contains_key(id) {
            out = union(out, inst_box(j));
        }
    }
    out
}

impl LiveDrc {
    /// Forget everything: the next frame checks the whole cell again.
    pub fn reset(&mut self) {
        self.violations.clear();
        self.session = None;
        self.base = None;
        self.seen = 0;
        self.dirty = None;
        self.job = None;
        self.quick = None;
    }

    pub fn running(&self) -> bool {
        self.job.is_some() || self.dirty.is_some()
    }

    /// Install a finished check.
    fn install(&mut self, session: DrcSession) {
        self.violations = session.report().violations.clone();
        self.millis = session.report().millis;
        self.session = Some(session);
    }

    /// Violation under the world point `p` (smallest marker first).
    pub fn at(&self, p: (i64, i64), tol: i64) -> Option<&Violation> {
        self.violations
            .iter()
            .filter(|v| {
                let m = v.marker;
                p.0 >= m.x0 - tol && p.0 <= m.x1 + tol && p.1 >= m.y0 - tol && p.1 <= m.y1 + tol
            })
            .min_by_key(|v| v.marker.width() as i128 * v.marker.height() as i128)
    }
}

impl LayoutEditor {
    /// Parsed deck of the editor (the one being edited in the DRC panel, else the technology's).
    pub(super) fn live_deck(&mut self, cx: &LayCtx) -> Option<Arc<Deck>> {
        let text = self.verify.deck.clone().unwrap_or_else(|| super::verify::deck_source(cx));
        if let Some((t, d)) = &self.live.deck
            && *t == text
        {
            return Some(d.clone());
        }
        let deck = Arc::new(drc::parse_any(&text, cx.tech).ok()?.deck);
        if self.live.deck.is_some() {
            self.live.base = None;
        }
        self.live.deck = Some((text, deck.clone()));
        Some(deck)
    }

    /// Once per frame: pick up finished checks, note what the last edit changed, and start the
    /// next check when the layout has been quiet for a moment.
    pub(super) fn live_tick(&mut self, ctx: &egui::Context, l: &Layout, cx: &mut LayCtx) {
        if !self.live.enabled {
            return;
        }
        if let Some(job) = &self.live.job {
            match job.rx.try_recv() {
                Ok(session) => {
                    self.live.job = None;
                    self.live.install(session);
                }
                Err(TryRecvError::Disconnected) => self.live.job = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        let Some(deck) = self.live_deck(cx) else { return };
        let generation = self.verify.generation();
        if self.live.seen != generation || self.live.base.is_none() {
            let changed = match &self.live.base {
                None => {
                    self.live.violations.clear();
                    self.live.session = None;
                    self.live.job = None;
                    cx.index(l).bbox
                }
                Some(base) => {
                    let (hier, src) = (&mut *cx.hier, cx.src);
                    let mut inst_box = |i: &CellInst| {
                        let cb = hier.get(&i.cell, src).and_then(|x| x.bbox).unwrap_or_default();
                        i.bbox(cb)
                    };
                    diff(base, l, &mut inst_box)
                }
            };
            self.live.seen = generation;
            self.live.base = Some(l.clone());
            if let Some(c) = changed {
                self.live.dirty = union(self.live.dirty, c);
                self.live.due = Some(Instant::now() + DEBOUNCE);
            }
        }
        if self.live.job.is_none()
            && let Some(window) = self.live.dirty
        {
            let due = self.live.due.unwrap_or_else(Instant::now);
            let now = Instant::now();
            if now < due {
                ctx.request_repaint_after(due - now);
                return;
            }
            self.live.dirty = None;
            let mut snap = tools::snapshot(cx.src, Some(cx.cell), Some(l));
            let top = snap.cell_mut(cx.cell).and_then(|c| c.layout.take()).unwrap_or_default();
            let (tx, rx) = mpsc::channel();
            let ctx = ctx.clone();
            let session = self.live.session.take();
            std::thread::spawn(move || {
                let session = match session {
                    Some(mut s) => {
                        s.update(&top, &snap, &snap.tech, &[window]);
                        s
                    }
                    None => DrcSession::new(&deck.local(), &top, &snap, &snap.tech),
                };
                let _ = tx.send(session);
                ctx.request_repaint();
            });
            self.live.job = Some(LiveJob { rx });
        }
    }

    /// Open the quick-fix form for the violation under `p` (else the one nearest the view centre).
    pub(super) fn open_quick_fix(&mut self, l: &Layout, cx: &mut LayCtx, p: Option<(i64, i64)>, screen: Option<Pos2>) {
        let tol = (6.0 / self.view.scale) as i64;
        let v = match p.and_then(|p| self.live.at(p, tol)) {
            Some(v) => v.clone(),
            None => {
                let c = (self.view.center.0 as i64, self.view.center.1 as i64);
                let d = |v: &Violation| {
                    let m = v.marker;
                    ((m.x0 + m.x1) / 2 - c.0).abs() + ((m.y0 + m.y1) / 2 - c.1).abs()
                };
                match self.live.violations.iter().min_by_key(|v| d(v)) {
                    Some(v) => v.clone(),
                    None => {
                        self.note(true, "No live DRC violations");
                        return;
                    }
                }
            }
        };
        let Some(deck) = self.live_deck(cx) else { return };
        let fixes = drc_fix::quick_fixes(&deck, &v, l, cx.src, cx.tech);
        self.live.quick_at = screen;
        self.live.quick = Some((v, fixes));
    }

    /// Apply a fix as one undoable edit.
    pub(super) fn apply_fix(&mut self, l: &mut Layout, fix: &Fix) {
        self.history.push(l);
        drc_fix::apply(l, fix);
        self.note(true, format!("Applied: {}", fix.title));
    }

    /// Apply the best fix of every live violation as one undoable edit (see [`drc_fix::fix_all`]).
    /// Returns the number of fixes applied.
    pub(super) fn fix_all(&mut self, l: &mut Layout, cx: &mut LayCtx) -> usize {
        let Some(deck) = self.live_deck(cx) else { return 0 };
        let mut after = l.clone();
        let done = drc_fix::fix_all(&deck, &mut after, cx.src, cx.tech, FIX_ROUNDS);
        if !done.is_empty() {
            self.history.push(l);
            *l = after;
        }
        self.note(!done.is_empty(), format!("Fix all: {} fix(es) applied", done.len()));
        done.len()
    }

    /// The quick-fix form. Returns true if a fix was applied.
    pub(super) fn quick_fix_form(&mut self, ctx: &egui::Context, rect: Rect, l: &mut Layout, cx: &LayCtx) -> bool {
        let Some((v, fixes)) = self.live.quick.clone() else { return false };
        let at = self.live.quick_at.unwrap_or(rect.center()) + Vec2::new(12.0, 12.0);
        let mut open = true;
        let mut picked = None;
        let shown = egui::Window::new(egui::RichText::new("Quick Fix").size(14.0).strong())
            .id(egui::Id::unique("lay_form_quickfix"))
            .collapsible(false)
            .resizable(false)
            .default_width(300.0)
            .default_pos(at)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(egui::RichText::new(&v.rule).strong());
                ui.label(&v.description);
                ui.label(
                    egui::RichText::new(format!(
                        "at ({}, {})",
                        super::um((v.marker.x0 + v.marker.x1) / 2, cx.tech),
                        super::um((v.marker.y0 + v.marker.y1) / 2, cx.tech)
                    ))
                    .small()
                    .weak(),
                );
                ui.separator();
                if fixes.is_empty() {
                    ui.label("No automatic fix: the geometry is inside a placed cell or the rule is global.");
                }
                for (k, f) in fixes.iter().enumerate() {
                    let side = if f.new_violations == 0 { "clean".to_string() } else { format!("+{} new", f.new_violations) };
                    let b = egui::Button::new(&f.title).shortcut_text(side);
                    if ui.add_sized([ui.available_width(), 22.0], b).clicked() {
                        picked = Some(k);
                    }
                }
            });
        if let Some(r) = shown {
            self.form_rects.push(r.response.rect);
        }
        if let Some(k) = picked {
            self.live.quick = None;
            self.apply_fix(l, &fixes[k]);
            return true;
        }
        if !open {
            self.live.quick = None;
        }
        false
    }

    /// Canvas overlay: live violations as dashed boxes, the hovered one explained.
    pub(super) fn draw_live(&self, painter: &egui::Painter, view: &View2D, rect: Rect, pal: &Palette, hover: Option<Pos2>) {
        if !self.live.enabled {
            return;
        }
        let st = Stroke::new(1.4, pal.warning);
        let mut hovered = None;
        for v in self.live.violations.iter().take(5000) {
            let sr = screen_rect(view, rect, v.marker).expand(3.0);
            if !sr.intersects(rect) {
                continue;
            }
            painter.rect_filled(sr, 1.0, pal.warning.gamma_multiply(0.12));
            let c = [sr.left_top(), sr.right_top(), sr.right_bottom(), sr.left_bottom(), sr.left_top()];
            painter.add(egui::Shape::dashed_line(&c, st, 4.0, 3.0));
            if hover.is_some_and(|h| sr.expand(3.0).contains(h)) {
                hovered = Some((v, sr));
            }
        }
        if let Some((v, sr)) = hovered {
            painter.rect_stroke(sr, 1.0, Stroke::new(2.0, pal.warning), StrokeKind::Outside);
            let text = format!("{}: {}   Ctrl+. quick fix", v.rule, v.description);
            let font = egui::FontId::proportional(12.0);
            let galley = painter.layout_no_wrap(text, font, pal.text);
            let pos = Pos2::new(sr.left(), sr.top() - galley.size().y - 6.0).max(rect.left_top() + Vec2::splat(4.0));
            let bg = Rect::from_min_size(pos, galley.size()).expand(4.0);
            painter.rect_filled(bg, 3.0, pal.bg_panel);
            painter.rect_stroke(bg, 3.0, Stroke::new(1.0, pal.warning), StrokeKind::Inside);
            painter.galley(pos, galley, pal.text);
        }
        let n = self.live.violations.len();
        let (label, color) = if self.live.running() {
            ("Live DRC: checking…".to_string(), pal.text_weak)
        } else if n == 0 {
            ("Live DRC: clean".to_string(), pal.ok)
        } else {
            (format!("Live DRC: {n} violation(s)"), pal.warning)
        };
        painter.text(
            rect.right_bottom() + Vec2::new(-10.0, -8.0),
            egui::Align2::RIGHT_BOTTOM,
            label,
            egui::FontId::proportional(12.0),
            color,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ics_core::layout::Geometry;

    #[test]
    fn diff_covers_moved_added_and_removed_items() {
        let mut a = Layout::default();
        let keep = a.add_rect(1, 0, 0, 100, 100);
        let moved = a.add_rect(1, 1000, 0, 1100, 100);
        let gone = a.add_rect(2, 5000, 5000, 5100, 5100);
        let mut b = a.clone();
        b.translate_item(moved, 50, 0);
        b.remove(gone);
        b.add(3, Geometry::Rect(IRect::new(-200, -200, -100, -100)));
        let mut ib = |_: &CellInst| IRect::default();
        assert_eq!(diff(&a, &b, &mut ib), Some(IRect::new(-200, -200, 5100, 5100)));
        let mut c = a.clone();
        c.translate_item(moved, 50, 0);
        assert_eq!(diff(&a, &c, &mut ib), Some(IRect::new(1000, 0, 1150, 100)));
        assert_eq!(diff(&a, &a, &mut ib), None);
        let _ = keep;
    }
}
