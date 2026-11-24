//! Search and replace (Virtuoso Edit > Search): find the cell's objects by kind, layer, name
//! and size, select or zoom to them, and change the layer of the shapes found or delete them.

use crate::i18n::{t, tf};
use eframe::egui;
use ics_core::geom::IRect;
use ics_core::layout::{Geometry, Layout};
use ics_core::tech::Tech;

use super::{LayCtx, LayoutEditor};
use crate::theme;

/// Kind of object searched for.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Kind {
    #[default]
    Any,
    Rect,
    Polygon,
    Path,
    Label,
    Instance,
}

impl Kind {
    const ALL: [Kind; 6] = [Kind::Any, Kind::Rect, Kind::Polygon, Kind::Path, Kind::Label, Kind::Instance];

    fn name(self) -> &'static str {
        match self {
            Kind::Any => "Any object",
            Kind::Rect => "Rectangle",
            Kind::Polygon => "Polygon",
            Kind::Path => "Path",
            Kind::Label => "Label",
            Kind::Instance => "Instance",
        }
    }
}

/// Search criteria. Sizes are in dbu; 0 means no limit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub kind: Kind,
    /// Layer index; `None` matches every layer.
    pub layer: Option<usize>,
    /// Case-insensitive part of a label's text, an instance's name or its cell's name.
    pub text: String,
    pub min_w: i64,
    pub max_w: i64,
    pub min_h: i64,
    pub max_h: i64,
    /// Only within the current view.
    pub in_view: bool,
}

/// Search form state.
#[derive(Clone, Debug, Default)]
pub struct SearchState {
    pub query: Query,
    /// Ids found by the last search, in layout order.
    pub found: Vec<u64>,
    /// Index of the object last zoomed to.
    next: usize,
    /// Layer the "replace" changes found shapes to.
    pub to_layer: usize,
    pub searched: bool,
}

fn size_ok(v: i64, lo: i64, hi: i64) -> bool {
    (lo <= 0 || v >= lo) && (hi <= 0 || v <= hi)
}

/// Ids of the top-level objects of `l` matching `q`. Instance sizes come from `inst_bbox`.
pub fn find(l: &Layout, q: &Query, view: Option<IRect>, inst_bbox: &dyn Fn(usize) -> Option<IRect>) -> Vec<u64> {
    let text = q.text.trim().to_lowercase();
    let sized = |b: IRect| {
        size_ok(b.width(), q.min_w, q.max_w) && size_ok(b.height(), q.min_h, q.max_h) && view.is_none_or(|v| v.intersects(&b))
    };
    let mut out = Vec::new();
    if q.kind != Kind::Instance {
        for s in &l.shapes {
            let kind_ok = matches!(
                (&s.geom, q.kind),
                (_, Kind::Any)
                    | (Geometry::Rect(_), Kind::Rect)
                    | (Geometry::Polygon { .. }, Kind::Polygon)
                    | (Geometry::Path { .. }, Kind::Path)
                    | (Geometry::Label { .. }, Kind::Label)
            );
            let text_ok =
                text.is_empty() || matches!(&s.geom, Geometry::Label { text: t, .. } if t.to_lowercase().contains(&text));
            if kind_ok && text_ok && q.layer.is_none_or(|k| k == s.layer) && sized(s.bbox()) {
                out.push(s.id);
            }
        }
    }
    if matches!(q.kind, Kind::Any | Kind::Instance) && q.layer.is_none() {
        for (k, i) in l.instances.iter().enumerate() {
            let name_ok = text.is_empty()
                || i.name.as_deref().is_some_and(|n| n.to_lowercase().contains(&text))
                || i.cell.to_string().to_lowercase().contains(&text);
            if name_ok && inst_bbox(k).is_some_and(sized) {
                out.push(i.id);
            }
        }
    }
    out
}

fn layer_combo(ui: &mut egui::Ui, id: &str, label: egui::Id, tech: &Tech, v: &mut Option<usize>, any: bool) {
    let name = |v: &Option<usize>| match v {
        None => "Any layer".to_string(),
        Some(k) => tech.layers.get(*k).map_or("?".into(), |d| d.name.clone()),
    };
    egui::ComboBox::from_id_salt(id)
        .selected_text(name(v))
        .height(320.0)
        .show_ui(ui, |ui| {
            if any {
                ui.selectable_value(v, None, t("Any layer"));
            }
            for (k, d) in tech.layers.iter().enumerate() {
                ui.selectable_value(v, Some(k), &d.name);
            }
        })
        .response
        .labelled_by(label);
}

impl LayoutEditor {
    /// The Search form body. Returns true when the layout changed.
    pub(super) fn search_form(&mut self, ui: &mut egui::Ui, l: &mut Layout, cx: &mut LayCtx, view: IRect) -> bool {
        let tech = cx.tech;
        let pal = theme::palette(ui.ctx());
        let k = tech.dbu_per_um.max(1) as f64;
        let g = tech.grid.max(1);
        let mut changed = false;
        let q = &mut self.search.query;
        egui::Grid::new("search_q").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
            let lb = ui.label(t("Object"));
            egui::ComboBox::from_id_salt("search_kind")
                .selected_text(t(q.kind.name()))
                .show_ui(ui, |ui| {
                    for kd in Kind::ALL {
                        ui.selectable_value(&mut q.kind, kd, t(kd.name()));
                    }
                })
                .response
                .labelled_by(lb.id);
            ui.end_row();
            let lb = ui.label(t("Layer"));
            layer_combo(ui, "search_layer", lb.id, tech, &mut q.layer, true);
            ui.end_row();
            let lb = ui.label(t("Name contains"));
            ui.text_edit_singleline(&mut q.text).labelled_by(lb.id);
            ui.end_row();
            let um = |ui: &mut egui::Ui, label: &str, v: &mut i64| {
                let lb = ui.label(t(label));
                let mut f = *v as f64 / k;
                ui.add(egui::DragValue::new(&mut f).speed(0.01).range(0.0..=1e6).max_decimals(4).suffix(" µm"))
                    .labelled_by(lb.id);
                *v = ((f * k / g as f64).round() as i64) * g;
                ui.end_row();
            };
            um(ui, "Width at least", &mut q.min_w);
            um(ui, "Width at most", &mut q.max_w);
            um(ui, "Height at least", &mut q.min_h);
            um(ui, "Height at most", &mut q.max_h);
        });
        ui.checkbox(&mut q.in_view, t("Only in the current view"));
        ui.label(egui::RichText::new(t("Sizes of 0 are not checked; layers match shapes only")).small().color(pal.text_weak));
        ui.horizontal(|ui| {
            if ui.button(t("Search")).clicked() {
                let idx = cx.index(l);
                let q = self.search.query.clone();
                let v = q.in_view.then_some(view);
                self.search.found = find(l, &q, v, &|i| idx.instance_bbox(l, i));
                self.search.next = 0;
                self.search.searched = true;
            }
            if ui.add_enabled(!self.search.found.is_empty(), egui::Button::new(t("Select all"))).clicked() {
                self.selection = self.search.found.iter().copied().collect();
            }
            if ui.add_enabled(!self.search.found.is_empty(), egui::Button::new(t("Zoom to next"))).clicked() {
                let n = self.search.found.len();
                let id = self.search.found[self.search.next % n];
                self.search.next = (self.search.next + 1) % n;
                let idx = cx.index(l);
                let b = l
                    .shape(id)
                    .map(|s| s.bbox())
                    .or_else(|| l.instances.iter().position(|i| i.id == id).and_then(|i| idx.instance_bbox(l, i)));
                if let Some(b) = b {
                    self.selection = [id].into();
                    self.zoom_to(b);
                }
            }
        });
        if self.search.searched {
            ui.label(tf("{} object(s) found", &[&self.search.found.len().to_string()]));
        }
        let shapes: Vec<u64> = self.search.found.iter().copied().filter(|id| l.shape(*id).is_some()).collect();
        if !shapes.is_empty() {
            theme::section(ui, t("Replace"));
            ui.horizontal(|ui| {
                let lb = ui.label(t("Layer of found shapes to"));
                let mut to = Some(self.search.to_layer.min(tech.layers.len().saturating_sub(1)));
                layer_combo(ui, "search_to_layer", lb.id, tech, &mut to, false);
                self.search.to_layer = to.unwrap_or(0);
            });
            ui.horizontal(|ui| {
                if ui.button(tf("Replace in {} shape(s)", &[&shapes.len().to_string()])).clicked() {
                    self.history.push(l);
                    for id in &shapes {
                        if let Some(s) = l.shape_mut(*id) {
                            s.layer = self.search.to_layer;
                        }
                    }
                    changed = true;
                    let name = tech.layers.get(self.search.to_layer).map_or("?", |d| d.name.as_str());
                    self.note(true, tf("Moved {} shape(s) to {}", &[&shapes.len().to_string(), name]));
                }
                if ui.button(t("Delete found")).clicked() {
                    self.history.push(l);
                    for id in &self.search.found {
                        l.remove(*id);
                    }
                    self.note(true, tf("Deleted {} object(s)", &[&self.search.found.len().to_string()]));
                    self.selection.retain(|id| !self.search.found.contains(id));
                    self.search.found.clear();
                    changed = true;
                }
            });
        }
        if changed {
            cx.touched();
            self.layout_changed();
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_by_kind_layer_text_and_size() {
        let mut l = Layout::default();
        let a = l.add_rect(1, 0, 0, 100, 100);
        let b = l.add_rect(2, 0, 0, 500, 100);
        let c = l.add(1, Geometry::Label { text: "VDD".into(), at: (0, 0), size: 100 });
        let none = |_: usize| None;
        let q = |f: fn(&mut Query)| {
            let mut q = Query::default();
            f(&mut q);
            q
        };
        assert_eq!(find(&l, &Query::default(), None, &none), vec![a, b, c]);
        assert_eq!(find(&l, &q(|q| q.layer = Some(1)), None, &none), vec![a, c]);
        assert_eq!(find(&l, &q(|q| q.kind = Kind::Rect), None, &none), vec![a, b]);
        assert_eq!(find(&l, &q(|q| q.text = "vd".into()), None, &none), vec![c]);
        assert_eq!(find(&l, &q(|q| q.min_w = 200), None, &none), vec![b]);
        assert_eq!(find(&l, &q(|q| q.kind = Kind::Rect), Some(IRect::new(200, 0, 300, 50)), &none), vec![b]);
    }
}
