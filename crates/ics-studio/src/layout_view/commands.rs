//! Virtuoso-style command layer: named actions shared by bindkeys, menus and the canvas
//! pop-up menu, the option forms (q, F3, e, Shift+E), partial selection (F4) and the
//! mouse-button hints of the status line.

use std::collections::{BTreeMap, BTreeSet};

use eframe::egui::{self, Pos2, Rect, Stroke, StrokeKind, Vec2};
use ics_core::compact::Dir;
use ics_core::constraints::{Axis, Edge};
use ics_core::geom::IRect;
use ics_core::layout::{Geometry, Layout, Trans};
use ics_core::tech::Tech;

use super::{LDrag, LayCtx, LayRequest, LayTool, LayoutEditor, draw_outline, edit, hits, screen_rect};
use crate::theme::{self, Palette};

/// A named editor action, reachable from a bindkey, a menu and the command palette.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LayAction {
    Tool(LayTool),
    Undo,
    Redo,
    Delete,
    Yank,
    Paste,
    SelectAll,
    DeselectAll,
    Properties,
    Options,
    DisplayOptions,
    EditorOptions,
    PartialSelect,
    Merge,
    Rotate,
    FlipX,
    FlipY,
    Descend,
    EditInPlace,
    Return,
    HierarchyTree,
    MarkNet,
    LayerTap,
    Gravity,
    ZoomIn,
    ZoomOut,
    ZoomBox,
    PrevView,
    NextView,
    Fit,
    AllLevels,
    TopLevel,
    ClearRulers,
    LiveDrc,
    QuickFix,
    FixAll,
    SymmetryV,
    SymmetryH,
    AlignLeft,
    AlignBottom,
    AlignCenterX,
    AlignCenterY,
    FixInPlace,
    InferConstraints,
    ClearConstraints,
    CompactLeft,
    CompactRight,
    CompactDown,
    CompactUp,
    Compact,
    CompareSaved,
    CompareHead,
    NextDifference,
}

impl LayAction {
    pub fn name(self) -> &'static str {
        match self {
            LayAction::Tool(LayTool::Select) => "Select",
            LayAction::Tool(LayTool::Stretch) => "Stretch",
            LayAction::Tool(LayTool::Move) => "Move",
            LayAction::Tool(LayTool::Copy) => "Copy",
            LayAction::Tool(LayTool::Rect) => "Rectangle",
            LayAction::Tool(LayTool::Polygon) => "Polygon",
            LayAction::Tool(LayTool::Path) => "Path",
            LayAction::Tool(LayTool::Label) => "Label",
            LayAction::Tool(LayTool::Via) => "Via",
            LayAction::Tool(LayTool::Instance) => "Instance",
            LayAction::Tool(LayTool::Chop) => "Chop",
            LayAction::Tool(LayTool::Ruler) => "Ruler",
            LayAction::Undo => "Undo",
            LayAction::Redo => "Redo",
            LayAction::Delete => "Delete",
            LayAction::Yank => "Yank",
            LayAction::Paste => "Paste",
            LayAction::SelectAll => "Select All",
            LayAction::DeselectAll => "Deselect All",
            LayAction::Properties => "Properties",
            LayAction::Options => "Command Options",
            LayAction::DisplayOptions => "Display Options",
            LayAction::EditorOptions => "Editor Options",
            LayAction::PartialSelect => "Toggle Partial Select",
            LayAction::Merge => "Merge",
            LayAction::Rotate => "Rotate 90°",
            LayAction::FlipX => "Flip Sideways",
            LayAction::FlipY => "Flip Upside Down",
            LayAction::Descend => "Descend Edit",
            LayAction::EditInPlace => "Edit In Place",
            LayAction::HierarchyTree => "Hierarchy Tree",
            LayAction::Return => "Return",
            LayAction::MarkNet => "Mark Net",
            LayAction::LayerTap => "Layer Tap",
            LayAction::Gravity => "Toggle Gravity",
            LayAction::ZoomIn => "Zoom In by 2",
            LayAction::ZoomOut => "Zoom Out by 2",
            LayAction::ZoomBox => "Zoom to Area",
            LayAction::PrevView => "Previous View",
            LayAction::NextView => "Next View",
            LayAction::Fit => "Fit All",
            LayAction::AllLevels => "Show All Levels",
            LayAction::TopLevel => "Show Top Level Only",
            LayAction::ClearRulers => "Clear All Rulers",
            LayAction::LiveDrc => "Toggle Live DRC",
            LayAction::QuickFix => "Quick Fix",
            LayAction::FixAll => "Fix All Violations",
            LayAction::SymmetryV => "Symmetric About Vertical Axis",
            LayAction::SymmetryH => "Symmetric About Horizontal Axis",
            LayAction::AlignLeft => "Keep Left Edges Aligned",
            LayAction::AlignBottom => "Keep Bottom Edges Aligned",
            LayAction::AlignCenterX => "Keep Centres Aligned Vertically",
            LayAction::AlignCenterY => "Keep Centres Aligned Horizontally",
            LayAction::FixInPlace => "Fix In Place",
            LayAction::InferConstraints => "Infer Constraints From Schematic",
            LayAction::ClearConstraints => "Remove Constraints",
            LayAction::CompactLeft => "Compact Left",
            LayAction::CompactRight => "Compact Right",
            LayAction::CompactDown => "Compact Down",
            LayAction::CompactUp => "Compact Up",
            LayAction::Compact => "Compact (Left, Then Down)",
            LayAction::CompareSaved => "Compare With Saved",
            LayAction::CompareHead => "Compare With Git HEAD",
            LayAction::NextDifference => "Next Difference",
        }
    }

    /// The Virtuoso default bindkey.
    pub fn key(self) -> &'static str {
        match self {
            LayAction::Tool(t) => t.key(),
            LayAction::Undo => "u",
            LayAction::Redo => "Shift+U",
            LayAction::Delete => "Del",
            LayAction::Yank => "y",
            LayAction::Paste => "Shift+Y",
            LayAction::SelectAll => "Ctrl+A",
            LayAction::DeselectAll => "Ctrl+D",
            LayAction::Properties => "q",
            LayAction::Options => "F3",
            LayAction::DisplayOptions => "e",
            LayAction::EditorOptions => "Shift+E",
            LayAction::PartialSelect => "F4",
            LayAction::Merge => "Shift+M",
            LayAction::Rotate => "Shift+O",
            LayAction::FlipX => "Ctrl+J",
            LayAction::FlipY => "",
            LayAction::EditInPlace => "x",
            LayAction::HierarchyTree => "Shift+T",
            LayAction::Descend => "Shift+X",
            LayAction::Return => "Shift+B",
            LayAction::MarkNet => "Shift+N",
            LayAction::LayerTap => "t",
            LayAction::Gravity => "g",
            LayAction::ZoomIn => "[",
            LayAction::ZoomOut => "Shift+Z",
            LayAction::ZoomBox => "z",
            LayAction::PrevView => "w",
            LayAction::NextView => "Shift+W",
            LayAction::Fit => "f",
            LayAction::AllLevels => "Shift+F",
            LayAction::TopLevel => "Ctrl+F",
            LayAction::ClearRulers => "Shift+K",
            LayAction::LiveDrc => "",
            LayAction::QuickFix => "Ctrl+.",
            LayAction::FixAll => "",
            LayAction::SymmetryV
            | LayAction::SymmetryH
            | LayAction::AlignLeft
            | LayAction::AlignBottom
            | LayAction::AlignCenterX
            | LayAction::AlignCenterY
            | LayAction::FixInPlace
            | LayAction::InferConstraints
            | LayAction::ClearConstraints
            | LayAction::CompactLeft
            | LayAction::CompactRight
            | LayAction::CompactDown
            | LayAction::CompactUp
            | LayAction::Compact
            | LayAction::CompareSaved
            | LayAction::CompareHead
            | LayAction::NextDifference => "",
        }
    }

    /// Menus of the layout editor, Virtuoso layout: (menu, items), `None` separates.
    pub const MENUS: &[(&str, &[Option<LayAction>])] = &[
        (
            "Create",
            &[
                Some(LayAction::Tool(LayTool::Rect)),
                Some(LayAction::Tool(LayTool::Polygon)),
                Some(LayAction::Tool(LayTool::Path)),
                Some(LayAction::Tool(LayTool::Label)),
                Some(LayAction::Tool(LayTool::Via)),
                Some(LayAction::Tool(LayTool::Instance)),
                None,
                Some(LayAction::Tool(LayTool::Ruler)),
                Some(LayAction::ClearRulers),
            ],
        ),
        (
            "Edit",
            &[
                Some(LayAction::Tool(LayTool::Move)),
                Some(LayAction::Tool(LayTool::Copy)),
                Some(LayAction::Tool(LayTool::Stretch)),
                Some(LayAction::Properties),
                None,
                Some(LayAction::Yank),
                Some(LayAction::Paste),
                None,
                Some(LayAction::Tool(LayTool::Chop)),
                Some(LayAction::Merge),
                Some(LayAction::Rotate),
                Some(LayAction::FlipX),
                Some(LayAction::FlipY),
                None,
                Some(LayAction::Descend),
                Some(LayAction::EditInPlace),
                Some(LayAction::Return),
                Some(LayAction::HierarchyTree),
                None,
                Some(LayAction::SelectAll),
                Some(LayAction::DeselectAll),
                Some(LayAction::PartialSelect),
            ],
        ),
        (
            "View",
            &[
                Some(LayAction::ZoomBox),
                Some(LayAction::ZoomIn),
                Some(LayAction::ZoomOut),
                Some(LayAction::Fit),
                Some(LayAction::PrevView),
                Some(LayAction::NextView),
                None,
                Some(LayAction::AllLevels),
                Some(LayAction::TopLevel),
            ],
        ),
        (
            "Options",
            &[
                Some(LayAction::DisplayOptions),
                Some(LayAction::EditorOptions),
                Some(LayAction::Options),
                None,
                Some(LayAction::Gravity),
                Some(LayAction::PartialSelect),
                Some(LayAction::LayerTap),
                Some(LayAction::MarkNet),
            ],
        ),
        (
            "Assist",
            &[
                Some(LayAction::LiveDrc),
                Some(LayAction::QuickFix),
                Some(LayAction::FixAll),
                None,
                Some(LayAction::SymmetryV),
                Some(LayAction::SymmetryH),
                Some(LayAction::AlignLeft),
                Some(LayAction::AlignBottom),
                Some(LayAction::AlignCenterX),
                Some(LayAction::AlignCenterY),
                Some(LayAction::FixInPlace),
                Some(LayAction::InferConstraints),
                Some(LayAction::ClearConstraints),
                None,
                Some(LayAction::Compact),
                Some(LayAction::CompactLeft),
                Some(LayAction::CompactRight),
                Some(LayAction::CompactDown),
                Some(LayAction::CompactUp),
                None,
                Some(LayAction::CompareSaved),
                Some(LayAction::CompareHead),
                Some(LayAction::NextDifference),
            ],
        ),
    ];

    /// The canvas pop-up menu (middle click, or right click with no command running).
    const POPUP: &[Option<LayAction>] = &[
        Some(LayAction::Tool(LayTool::Move)),
        Some(LayAction::Tool(LayTool::Copy)),
        Some(LayAction::Tool(LayTool::Stretch)),
        Some(LayAction::Delete),
        Some(LayAction::Properties),
        None,
        Some(LayAction::Rotate),
        Some(LayAction::Merge),
        Some(LayAction::Yank),
        Some(LayAction::Paste),
        None,
        Some(LayAction::Descend),
        Some(LayAction::EditInPlace),
        Some(LayAction::Return),
        None,
        Some(LayAction::Fit),
        Some(LayAction::PrevView),
    ];
}

/// Partial selection (F4, or a box in the Stretch command): outline vertices per shape, and
/// items entirely inside the box, which move as a whole.
#[derive(Default, Clone, Debug)]
pub struct Partial {
    pub verts: BTreeMap<u64, BTreeSet<usize>>,
    pub whole: BTreeSet<u64>,
}

impl Partial {
    pub fn is_empty(&self) -> bool {
        self.verts.is_empty() && self.whole.is_empty()
    }

    pub fn len(&self) -> usize {
        self.verts.len() + self.whole.len()
    }
}

impl LayoutEditor {
    /// Carry out an action, at the pointer for a bindkey (else at the canvas centre). Returns true
    /// if the layout changed.
    pub(super) fn act(&mut self, a: LayAction, ui: &egui::Ui, l: &mut Layout, cx: &mut LayCtx, rect: Rect, key: bool) -> bool {
        let hover = ui.input(|i| i.pointer.hover_pos()).filter(|p| key && rect.contains(*p));
        let anchor = hover.unwrap_or(rect.center());
        let at = hover.map(|p| {
            let w = self.view.to_world(rect, p);
            (w.0 as i64, w.1 as i64)
        });
        let tol = (5.0 / self.view.scale) as i64;
        match a {
            LayAction::Tool(t) => self.set_tool(t),
            LayAction::Undo => return self.undo(l),
            LayAction::Redo => return self.redo(l),
            LayAction::Delete => return self.delete_selection(l),
            LayAction::Yank => {
                if let Some(text) = self.yank(l, cx) {
                    ui.ctx().copy_text(text);
                }
            }
            LayAction::Paste => {
                self.begin_paste(None);
            }
            LayAction::SelectAll => self.select_all(l),
            LayAction::DeselectAll => {
                self.selection.clear();
                self.part = Partial::default();
            }
            LayAction::Properties => self.show_props = !self.show_props,
            LayAction::Options => self.show_opts = !self.show_opts,
            LayAction::DisplayOptions => self.show_display = !self.show_display,
            LayAction::EditorOptions => self.show_editor = !self.show_editor,
            LayAction::PartialSelect => {
                self.partial_mode = !self.partial_mode;
                self.part = Partial::default();
                self.selection.clear();
                let m = if self.partial_mode {
                    "Partial select: click an edge or vertex, or box vertices; S stretches them (F4 toggles)"
                } else {
                    "Full select"
                };
                self.note(true, m);
            }
            LayAction::Merge => return self.merge_selection(l),
            LayAction::Rotate => return self.transform_selection(l, cx, 0),
            LayAction::FlipX => return self.transform_selection(l, cx, 1),
            LayAction::FlipY => return self.transform_selection(l, cx, 2),
            LayAction::Descend => self.descend(l, cx, at, tol, false),
            LayAction::EditInPlace => self.descend(l, cx, at, tol, true),
            LayAction::HierarchyTree => self.show_tree = !self.show_tree,
            LayAction::Return => self.requests.push(LayRequest::Return),
            LayAction::MarkNet => {
                self.verify.pick_nets = !self.verify.pick_nets;
                let m = if self.verify.pick_nets { "Mark net: click a shape to highlight its net" } else { "Mark net off" };
                self.note(true, m);
            }
            LayAction::LayerTap => self.layer_tap(l, cx, at, tol),
            LayAction::Gravity => {
                self.gravity = !self.gravity;
                self.note(true, if self.gravity { "Gravity on" } else { "Gravity off" });
            }
            LayAction::ZoomIn | LayAction::ZoomOut => {
                self.push_view();
                self.view.zoom_at(rect, anchor, if a == LayAction::ZoomIn { 2.0 } else { 0.5 });
            }
            LayAction::ZoomBox => self.arm_zoom_box(),
            LayAction::PrevView => {
                self.step_view(true);
            }
            LayAction::NextView => {
                self.step_view(false);
            }
            LayAction::Fit => {
                self.push_view();
                self.needs_fit = true;
            }
            LayAction::AllLevels => self.max_depth = 32,
            LayAction::TopLevel => self.max_depth = 0,
            LayAction::ClearRulers => self.rulers.clear(),
            LayAction::LiveDrc => {
                self.live.enabled = !self.live.enabled;
                self.live.reset();
                self.note(true, if self.live.enabled { "Live DRC on" } else { "Live DRC off" });
            }
            LayAction::QuickFix => {
                if !self.live.enabled {
                    self.live.enabled = true;
                    self.note(true, "Live DRC on");
                    return false;
                }
                self.open_quick_fix(l, cx, at, hover);
            }
            LayAction::FixAll => return self.fix_all(l, cx) > 0,
            LayAction::SymmetryV => return self.constrain_symmetry(l, cx, Axis::Vertical),
            LayAction::SymmetryH => return self.constrain_symmetry(l, cx, Axis::Horizontal),
            LayAction::AlignLeft => return self.constrain_align(l, cx, Edge::Left),
            LayAction::AlignBottom => return self.constrain_align(l, cx, Edge::Bottom),
            LayAction::AlignCenterX => return self.constrain_align(l, cx, Edge::CenterX),
            LayAction::AlignCenterY => return self.constrain_align(l, cx, Edge::CenterY),
            LayAction::FixInPlace => return self.constrain_fixed(l),
            LayAction::InferConstraints => return self.infer_constraints(l, cx),
            LayAction::ClearConstraints => return self.clear_constraints(l),
            LayAction::CompactLeft => return self.compact(l, cx, &[Dir::Left]),
            LayAction::CompactRight => return self.compact(l, cx, &[Dir::Right]),
            LayAction::CompactDown => return self.compact(l, cx, &[Dir::Down]),
            LayAction::CompactUp => return self.compact(l, cx, &[Dir::Up]),
            LayAction::Compact => return self.compact(l, cx, &[Dir::Left, Dir::Down]),
            LayAction::CompareSaved => self.compare_with(l, cx, None),
            LayAction::CompareHead => self.compare_with(l, cx, Some("HEAD")),
            LayAction::NextDifference => self.next_difference(),
        }
        false
    }

    /// Partial selection by box: the vertices inside it, and the items it encloses entirely.
    pub(super) fn partial_from_box(&mut self, l: &Layout, cx: &mut LayCtx, r: IRect, add: bool) {
        if !add {
            self.part = Partial::default();
        }
        let idx = cx.index(l);
        let inside = |b: IRect| b.x0 >= r.x0 && b.x1 <= r.x1 && b.y0 >= r.y0 && b.y1 <= r.y1;
        for (li, ok) in self.pickable().iter().enumerate() {
            if !*ok {
                continue;
            }
            for i in idx.shapes_in(li, r) {
                let Some(s) = l.shapes.get(i) else { continue };
                if inside(s.bbox()) {
                    self.part.verts.remove(&s.id);
                    self.part.whole.insert(s.id);
                    continue;
                }
                let v = edit::vertices_in(&s.geom, r);
                if !v.is_empty() {
                    self.part.verts.entry(s.id).or_default().extend(v);
                }
            }
        }
        if self.inst_selectable {
            for i in idx.instances_in(r) {
                if let (Some(b), Some(inst)) = (idx.instance_bbox(l, i), l.instances.get(i))
                    && inside(b)
                {
                    self.part.whole.insert(inst.id);
                }
            }
        }
    }

    /// Partial selection by click: the edge (or vertex) of the shape under the cursor.
    pub(super) fn partial_pick(&mut self, l: &Layout, cx: &mut LayCtx, p: (i64, i64), tol: i64, add: bool) {
        if !add {
            self.part = Partial::default();
        }
        let idx = cx.index(l);
        let hit = hits(l, &idx, &self.pickable(), false, p.0, p.1, 3 * tol).into_iter().find_map(|id| l.shape(id));
        if let Some(s) = hit
            && let Some(h) = edit::nearest_handle(&s.geom, p, 3 * tol)
        {
            let v = self.part.verts.entry(s.id).or_default();
            let hv = edit::handle_vertices(&s.geom, h);
            if add && hv.is_subset(v) {
                v.retain(|i| !hv.contains(i));
                if v.is_empty() {
                    self.part.verts.remove(&s.id);
                }
            } else {
                v.extend(hv);
            }
        }
    }

    /// Stretch the partial selection by `d`.
    pub(super) fn apply_stretch(&mut self, l: &mut Layout, d: (i64, i64)) -> bool {
        if d == (0, 0) || self.part.is_empty() {
            return false;
        }
        self.history.push(l);
        l.transform_items(&self.part.whole, &Trans::new(d.0, d.1));
        for (id, v) in &self.part.verts {
            if let Some(s) = l.shape(*id).cloned() {
                let g = edit::stretch_points(&s.geom, v, d);
                if let Some(t) = l.shape_mut(*id) {
                    *t = edit::with_geom(&s, g);
                }
            }
        }
        let n = self.part.len();
        self.part = Partial::default();
        self.part_ref = None;
        self.note(true, format!("Stretched {n} item(s)"));
        true
    }

    /// Displacement of the partial stretch in progress.
    pub(super) fn part_delta(&self) -> (i64, i64) {
        let from = match &self.drag {
            Some(LDrag::Part { start }) => Some(*start),
            _ => self.part_ref,
        };
        match (from, self.cursor) {
            (Some(a), Some(c)) => (c.0 - a.0, c.1 - a.1),
            _ => (0, 0),
        }
    }

    /// Selected vertices and edges, and the stretched outlines while a stretch is in progress.
    pub(super) fn draw_partial(&self, painter: &egui::Painter, rect: Rect, l: &Layout, cx: &mut LayCtx, pal: &Palette) {
        if self.part.is_empty() {
            return;
        }
        let d = self.part_delta();
        let s = |p: (i64, i64)| self.view.to_screen(rect, p.0 as f64, p.1 as f64);
        let hl = Stroke::new(3.0, pal.warning);
        for (id, v) in &self.part.verts {
            let Some(shape) = l.shape(*id) else { continue };
            let pts = edit::vertices(&shape.geom);
            let n = pts.len();
            let closed = !matches!(shape.geom, Geometry::Path { .. });
            for i in 0..if closed { n } else { n.saturating_sub(1) } {
                let j = (i + 1) % n;
                if v.contains(&i) && v.contains(&j) {
                    painter.line_segment([s(pts[i]), s(pts[j])], hl);
                }
            }
            for i in v.iter().filter(|i| **i < n) {
                painter.rect_filled(Rect::from_center_size(s(pts[*i]), Vec2::splat(6.0)), 0.0, pal.warning);
            }
            if d != (0, 0) {
                let g = edit::stretch_points(&shape.geom, v, d);
                draw_outline(painter, &self.view, rect, &g, Stroke::new(1.5, pal.selection));
            }
        }
        let idx = cx.index(l);
        let mut boxes: Vec<IRect> = self.part.whole.iter().filter_map(|id| l.shape(*id)).map(|s| s.bbox()).collect();
        for (i, inst) in l.instances.iter().enumerate() {
            if self.part.whole.contains(&inst.id)
                && let Some(b) = idx.instance_bbox(l, i)
            {
                boxes.push(b);
            }
        }
        for b in boxes {
            let st = Stroke::new(1.5, pal.warning);
            painter.rect_stroke(screen_rect(&self.view, rect, b), 0.0, st, StrokeKind::Middle);
            if d != (0, 0) {
                let ghost = Stroke::new(1.5, pal.selection);
                painter.rect_stroke(screen_rect(&self.view, rect, b.translate(d.0, d.1)), 0.0, ghost, StrokeKind::Middle);
            }
        }
        if let (Some(a), Some(c)) = (self.part_ref, self.cursor) {
            painter.add(egui::Shape::dashed_line(&[s(a), s(c)], Stroke::new(1.0, pal.selection), 4.0, 3.0));
            painter.circle_stroke(s(a), 3.5, Stroke::new(1.5, pal.selection));
        }
    }

    /// True when a click would not complete a command point (the pop-up menu may open).
    pub(super) fn idle(&self) -> bool {
        self.drag.is_none()
            && self.paste.is_none()
            && !self.zoom_box
            && self.xfer.is_none()
            && self.stretch.is_none()
            && self.part_ref.is_none()
            && self.box_start.is_none()
            && self.path_pts.is_empty()
            && self.poly_pts.is_empty()
    }

    /// The canvas pop-up menu, Virtuoso style: middle click anywhere, right click with no command point pending.
    pub(super) fn popup_menu(&mut self, resp: &egui::Response, l: &Layout, cx: &mut LayCtx, at: Option<(i64, i64)>) {
        let open = resp.clicked_by(egui::PointerButton::Middle) || (resp.secondary_clicked() && self.idle());
        if open
            && self.selection.is_empty()
            && let Some(p) = at
        {
            let idx = cx.index(l);
            let tol = (5.0 / self.view.scale) as i64;
            if let Some(id) = hits(l, &idx, &self.pickable(), self.inst_selectable, p.0, p.1, tol).first() {
                self.selection = [*id].into();
            }
        }
        let mut picked = None;
        let shown = egui::Popup::menu(resp)
            .id(egui::Id::unique("lay_popup"))
            .open_memory(open.then_some(egui::SetOpenCommand::Bool(true)))
            .at_pointer_fixed()
            .show(|ui| {
                for item in LayAction::POPUP {
                    match item {
                        Some(a) => {
                            if ui.add(egui::Button::new(a.name()).shortcut_text(a.key())).clicked() {
                                picked = Some(*a);
                            }
                        }
                        None => {
                            ui.separator();
                        }
                    }
                }
            });
        if let Some(r) = shown {
            self.form_rects.push(r.response.rect);
        }
        if let Some(a) = picked {
            self.actions.push(a);
        }
    }

    /// Whether bindkeys go to the canvas. Like Virtuoso's window focus, the canvas keeps them
    /// while one of its forms is open on top of it, until another part of the window is clicked;
    /// a focused widget in a form, or any text field, gets keys first.
    pub(super) fn keyboard_focus(&mut self, ui: &egui::Ui, resp: &egui::Response, rect: Rect) -> bool {
        let (press, hover) =
            ui.input(|i| (i.pointer.any_pressed().then(|| i.pointer.press_origin()).flatten(), i.pointer.hover_pos()));
        let on_form = |p: Pos2| self.form_rects.iter().any(|r| r.contains(p));
        if let Some(p) = press
            && !on_form(p)
        {
            self.kbd_focus = rect.contains(p);
            if self.kbd_focus {
                // Enter / Space must not re-trigger a form button clicked earlier.
                ui.memory_mut(|m| {
                    if let Some(id) = m.focused() {
                        m.surrender_focus(id);
                    }
                });
            }
        }
        if resp.hovered() {
            self.kbd_focus = true;
        }
        let form_widget = hover.is_some_and(on_form) && ui.memory(|m| m.focused().is_some());
        self.kbd_focus && !form_widget
    }

    /// What the left, middle and right mouse buttons do now (Virtuoso's mouse line).
    pub fn mouse_hints(&self) -> [&'static str; 3] {
        let l = if self.paste.is_some() {
            "Place the yanked items"
        } else if self.zoom_box {
            "Zoom box corner"
        } else {
            match self.tool {
                LayTool::Select if self.partial_mode => "Select edge / vertex (drag: box vertices)",
                LayTool::Select => "Select (drag: box)",
                LayTool::Stretch if self.part_ref.is_some() || self.stretch.is_some() => "Stretch to",
                LayTool::Stretch if !self.part.is_empty() => "Reference point",
                LayTool::Stretch => "Pick an edge (drag: box)",
                LayTool::Move | LayTool::Copy if self.xfer.is_some() => "Destination",
                LayTool::Move | LayTool::Copy => "Reference point",
                LayTool::Rect | LayTool::Chop => "Box corner",
                LayTool::Polygon => "Add vertex",
                LayTool::Path => "Add point",
                LayTool::Label => "Place label",
                LayTool::Via => "Place via (drag: area)",
                LayTool::Instance => "Place instance",
                LayTool::Ruler => "Ruler point",
            }
        };
        let r = match self.tool {
            LayTool::Move | LayTool::Copy if self.xfer.is_some() => "Rotate (Shift: flip)",
            LayTool::Polygon if !self.poly_pts.is_empty() => "Finish polygon",
            LayTool::Path if !self.path_pts.is_empty() => "Finish path",
            _ if self.idle() => "Pop-up menu (drag: zoom box)",
            _ => "Zoom box (drag)",
        };
        [l, "Pop-up menu (drag: pan)", r]
    }

    /// Snap spacing field (toolbar and editor options).
    pub fn snap_control(&mut self, ui: &mut egui::Ui, tech: &Tech) {
        ui.horizontal(|ui| {
            let sl = ui.label("Snap");
            let mut snap_um = self.snap_step(tech) as f64 / tech.dbu_per_um as f64;
            if ui
                .add(egui::DragValue::new(&mut snap_um).speed(0.005).range(0.0..=100.0).max_decimals(3).suffix(" µm"))
                .labelled_by(sl.id)
                .on_hover_text("Snap spacing, rounded to the technology grid")
                .changed()
            {
                self.snap = (snap_um * tech.dbu_per_um as f64).round() as i64;
            }
        });
    }

    /// Option forms: Edit Object Properties (q), command options (F3), display (e) and editor
    /// (Shift+E) options. Returns true if the layout changed.
    pub(super) fn forms(&mut self, ctx: &egui::Context, rect: Rect, l: &mut Layout, cx: &mut LayCtx) -> bool {
        let mut changed = false;
        let mut rects = vec![];
        // Command options open at the top left of the canvas, properties at the top right.
        let window = |title: &str, id: &str, at: Pos2| {
            egui::Window::new(egui::RichText::new(title).size(14.0).strong())
                .id(egui::Id::unique(id))
                .resizable(true)
                .collapsible(false)
                .default_width(280.0)
                .default_pos(at)
        };
        if self.show_props {
            let mut open = true;
            let shown = window("Edit Object Properties", "lay_form_props", rect.right_top() + Vec2::new(-300.0, 48.0))
                .open(&mut open)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().max_height(480.0).show(ui, |ui| {
                        let (c, o) = self.inspector(ui, l, cx);
                        changed |= c;
                        if let Some(o) = o {
                            self.requests.push(LayRequest::Open(o));
                        }
                    });
                });
            rects.extend(shown.map(|r| r.response.rect));
            self.show_props = open;
        }
        if self.show_opts {
            let mut open = true;
            let title = format!("{} Options", self.tool.name());
            let shown =
                window(&title, "lay_form_opts", rect.left_top() + Vec2::new(12.0, 48.0)).open(&mut open).show(ctx, |ui| {
                    if !self.tool_options(ui, cx) {
                        ui.label(egui::RichText::new(format!("{} has no options of its own.", self.tool.name())).weak());
                        self.snap_control(ui, cx.tech);
                        ui.checkbox(&mut self.gravity, "Gravity");
                    }
                });
            rects.extend(shown.map(|r| r.response.rect));
            self.show_opts = open;
        }
        if self.show_display {
            let mut open = true;
            let shown = window("Display Options", "lay_form_display", rect.left_top() + Vec2::new(12.0, 240.0))
                .open(&mut open)
                .show(ctx, |ui| {
                    theme::section(ui, "Grid");
                    ui.checkbox(&mut self.show_grid, "Show grid");
                    theme::section(ui, "Display Levels");
                    ui.horizontal(|ui| {
                        ui.label("From 0");
                        let t = ui.label("To");
                        ui.add(egui::DragValue::new(&mut self.max_depth).range(0..=32)).labelled_by(t.id);
                    });
                    theme::section(ui, "Show");
                    ui.checkbox(&mut self.show_flylines, "Flylines (unrouted schematic connections)");
                    if ui.button("Clear all rulers").clicked() {
                        self.rulers.clear();
                    }
                });
            rects.extend(shown.map(|r| r.response.rect));
            self.show_display = open;
        }
        if self.show_editor {
            let mut open = true;
            let shown = window("Layout Editor Options", "lay_form_editor", rect.left_top() + Vec2::new(300.0, 240.0))
                .open(&mut open)
                .show(ctx, |ui| {
                    theme::section(ui, "Snapping");
                    self.snap_control(ui, cx.tech);
                    ui.checkbox(&mut self.gravity, "Gravity (snap to vertices and edges)");
                    theme::section(ui, "Editing");
                    ui.checkbox(&mut self.drd, "Design-rule-driven feedback (DRD)");
                    ui.checkbox(&mut self.inst_selectable, "Instances selectable");
                    if ui.checkbox(&mut self.partial_mode, "Partial selection (F4)").changed() {
                        self.part = Partial::default();
                    }
                    ui.horizontal(|ui| {
                        let w = ui.label("Default path width (nm)");
                        ui.add(egui::DragValue::new(&mut self.path_width).range(0..=100_000).speed(10)).labelled_by(w.id);
                    });
                });
            rects.extend(shown.map(|r| r.response.rect));
            self.show_editor = open;
        }
        if self.show_tree {
            let mut open = true;
            let mut pick = None;
            let shown = window("Hierarchy Tree", "lay_form_tree", rect.right_top() + Vec2::new(-300.0, 48.0))
                .open(&mut open)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().max_height(480.0).show(ui, |ui| {
                        super::hier_node(ui, cx.src, cx.cell, Some(&*l), None, 0, &mut pick);
                    });
                });
            rects.extend(shown.map(|r| r.response.rect));
            if let Some(c) = pick {
                self.requests.push(LayRequest::Open(c));
            }
            self.show_tree = open;
        }
        self.form_rects = rects;
        changed
    }
}

/// Screen rectangle of a box being dragged with the right button (zoom to area).
pub(super) fn zoom_drag_rect(start: Pos2, end: Pos2) -> Option<Rect> {
    (start.distance(end) > 6.0).then(|| Rect::from_two_pos(start, end))
}
