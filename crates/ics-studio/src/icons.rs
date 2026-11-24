//! Vector icons painted with egui shapes (crisp at any DPI, no asset files).

use eframe::egui::{self, Color32, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use crate::i18n::{t, tf};
use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Home,
    Schematic,
    Simulation,
    Layout,
    Settings,
    Library,
    Cell,
    PopOut,
    DockIn,
    // Toolbar icons
    Undo,
    Redo,
    Rotate,
    Mirror,
    Duplicate,
    Delete,
    Fit,
    ZoomBox,
    PrevView,
    Pointer,
    Stretch,
    Move,
    Rectangle,
    Polygon,
    Path,
    Text,
    Via,
    Instance,
    Chop,
    Split,
    Ruler,
    Wire,
    NetLabel,
    Grid,
    Gravity,
    Drd,
    Constraints,
    Magnet,
    LiveDrc,
    Flylines,
    Export,
    Play,
    Return,
    Annotate,
}

pub fn paint(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width().min(rect.height());
    let c = rect.center();
    let u = s / 24.0; // design grid: 24x24
    let at = |x: f32, y: f32| -> Pos2 { pos2(c.x + (x - 12.0) * u, c.y + (y - 12.0) * u) };
    let st = Stroke::new(1.7 * u.max(0.8), color);
    match icon {
        Icon::Home => {
            p.add(Shape::line(vec![at(3.5, 11.0), at(12.0, 4.0), at(20.5, 11.0)], st));
            p.add(Shape::line(vec![at(6.0, 9.5), at(6.0, 19.5), at(18.0, 19.5), at(18.0, 9.5)], st));
            p.add(Shape::line(vec![at(10.0, 19.5), at(10.0, 14.0), at(14.0, 14.0), at(14.0, 19.5)], st));
        }
        Icon::Schematic => {
            // resistor zigzag between two leads + a node dot
            let pts = vec![
                at(2.5, 12.0),
                at(6.0, 12.0),
                at(7.5, 8.0),
                at(10.0, 16.0),
                at(12.5, 8.0),
                at(15.0, 16.0),
                at(16.5, 12.0),
                at(21.5, 12.0),
            ];
            p.add(Shape::line(pts, st));
            p.circle_filled(at(21.0, 12.0), 1.8 * u, color);
            p.circle_filled(at(3.0, 12.0), 1.8 * u, color);
        }
        Icon::Simulation => {
            p.add(Shape::line(vec![at(3.0, 4.0), at(3.0, 20.5), at(21.0, 20.5)], st));
            let mut wave = vec![];
            for i in 0..=24 {
                let x = 5.0 + i as f32 * (15.0 / 24.0);
                let y = 12.5 - 5.5 * ((i as f32 / 24.0) * std::f32::consts::TAU * 1.25).sin() * (1.0 - i as f32 / 40.0);
                wave.push(at(x, y));
            }
            p.add(Shape::line(wave, st));
        }
        Icon::Layout => {
            let r1 = Rect::from_min_max(at(3.5, 4.5), at(14.5, 13.5));
            let r2 = Rect::from_min_max(at(9.5, 10.5), at(20.5, 19.5));
            p.rect_filled(r1, 1.5 * u, color.gamma_multiply(0.25));
            p.rect_stroke(r1, 1.5 * u, st, eframe::egui::StrokeKind::Middle);
            p.rect_filled(r2, 1.5 * u, color.gamma_multiply(0.45));
            p.rect_stroke(r2, 1.5 * u, st, eframe::egui::StrokeKind::Middle);
        }
        Icon::Settings => {
            p.circle_stroke(c, 3.2 * u, st);
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::TAU / 8.0;
                let d = vec2(a.cos(), a.sin());
                p.line_segment([c + d * 6.0 * u, c + d * 9.0 * u], Stroke::new(2.4 * u, color));
            }
            p.circle_stroke(c, 6.2 * u, st);
        }
        Icon::Library => {
            for (i, x) in [5.0f32, 10.0, 15.0].iter().enumerate() {
                let h = if i == 2 { 4.0 } else { 5.0 };
                p.rect_stroke(Rect::from_min_max(at(*x, h), at(*x + 3.6, 20.0)), 0.8 * u, st, eframe::egui::StrokeKind::Middle);
            }
        }
        Icon::Cell => {
            p.rect_stroke(Rect::from_min_max(at(5.0, 5.0), at(19.0, 19.0)), 2.0 * u, st, eframe::egui::StrokeKind::Middle);
            p.rect_filled(Rect::from_min_max(at(9.0, 9.0), at(15.0, 15.0)), 1.0 * u, color.gamma_multiply(0.6));
        }
        Icon::Undo | Icon::Redo => {
            // Hook arrow: left at the top, round the right side, back along the bottom.
            let m = |x: f32| if icon == Icon::Undo { x } else { 24.0 - x };
            let mut pts = vec![at(m(5.0), 8.0)];
            pts.extend(arc(13.5, 13.5, 5.5, -90.0, 90.0).map(|(x, y)| at(m(x), y)));
            pts.push(at(m(8.0), 19.0));
            p.add(Shape::line(pts, st));
            p.add(Shape::line(vec![at(m(9.0), 4.0), at(m(5.0), 8.0), at(m(9.0), 12.0)], st));
        }
        Icon::Rotate => {
            let (cx, cy, r, a1) = (12.0, 12.5, 7.5, 290.0f32);
            p.add(Shape::line(arc(cx, cy, r, 20.0, a1).map(|(x, y)| at(x, y)).collect(), st));
            // Arrowhead along the arc's direction of travel at its end.
            let a = a1.to_radians();
            let (tx, ty) = (cx + r * a.cos(), cy + r * a.sin());
            let (dx, dy) = (-a.sin(), a.cos());
            let back = |side: f32| at(tx - dx * 4.5 + dy * side * 4.0, ty - dy * 4.5 - dx * side * 4.0);
            p.add(Shape::line(vec![back(1.0), at(tx, ty), back(-1.0)], st));
        }
        Icon::Mirror => {
            p.add(Shape::dashed_line(&[at(12.0, 2.5), at(12.0, 21.5)], Stroke::new(st.width * 0.8, color), 2.5 * u, 2.0 * u));
            let left = vec![at(9.5, 6.0), at(9.5, 18.0), at(3.0, 18.0)];
            p.add(Shape::convex_polygon(left, color.gamma_multiply(0.5), st));
            p.add(Shape::closed_line(vec![at(14.5, 6.0), at(14.5, 18.0), at(21.0, 18.0)], st));
        }
        Icon::Duplicate => {
            p.rect_stroke(Rect::from_min_max(at(8.5, 8.5), at(20.0, 20.0)), 2.0 * u, st, StrokeKind::Middle);
            p.add(Shape::line(vec![at(5.0, 15.5), at(4.0, 15.5), at(4.0, 4.0), at(15.5, 4.0), at(15.5, 5.0)], st));
        }
        Icon::Delete => {
            p.line_segment([at(4.0, 6.5), at(20.0, 6.5)], st);
            p.add(Shape::line(vec![at(9.5, 6.5), at(9.5, 3.5), at(14.5, 3.5), at(14.5, 6.5)], st));
            p.add(Shape::line(vec![at(6.0, 6.5), at(7.2, 20.5), at(16.8, 20.5), at(18.0, 6.5)], st));
            p.line_segment([at(10.3, 10.0), at(10.3, 17.0)], st);
            p.line_segment([at(13.7, 10.0), at(13.7, 17.0)], st);
        }
        Icon::Fit => {
            for (x, y, dx, dy) in [(3.5, 3.5, 1.0, 1.0), (20.5, 3.5, -1.0, 1.0), (3.5, 20.5, 1.0, -1.0), (20.5, 20.5, -1.0, -1.0)]
            {
                p.add(Shape::line(vec![at(x, y + dy * 5.0), at(x, y), at(x + dx * 5.0, y)], st));
            }
            p.rect_filled(Rect::from_min_max(at(8.5, 8.5), at(15.5, 15.5)), 1.0 * u, color.gamma_multiply(0.55));
        }
        Icon::ZoomBox => {
            p.circle_stroke(at(10.0, 10.0), 6.5 * u, st);
            p.line_segment([at(14.8, 14.8), at(20.5, 20.5)], Stroke::new(st.width * 1.5, color));
            p.rect_stroke(
                Rect::from_min_max(at(7.5, 7.5), at(12.5, 12.5)),
                0.0,
                Stroke::new(st.width * 0.8, color),
                StrokeKind::Middle,
            );
        }
        Icon::PrevView => {
            let pts: Vec<Pos2> = arc(12.5, 12.0, 8.0, 200.0, 520.0).map(|(x, y)| at(x, y)).collect();
            p.add(Shape::line(pts, st));
            let (tx, ty) = (12.5 + 8.0 * 200f32.to_radians().cos(), 12.0 + 8.0 * 200f32.to_radians().sin());
            p.add(Shape::line(vec![at(tx - 3.0, ty - 4.5), at(tx, ty), at(tx + 4.5, ty - 2.0)], st));
            p.add(Shape::line(vec![at(12.5, 7.5), at(12.5, 12.0), at(15.5, 14.0)], st));
        }
        Icon::Pointer => {
            let pts =
                vec![at(6.0, 3.0), at(6.0, 19.0), at(10.0, 15.2), at(13.0, 21.0), at(15.6, 19.8), at(12.7, 14.0), at(18.0, 14.0)];
            p.add(Shape::closed_line(pts.clone(), st));
            p.add(Shape::convex_polygon(vec![pts[0], pts[1], pts[2], pts[6]], color.gamma_multiply(0.35), Stroke::NONE));
        }
        Icon::Stretch => {
            p.rect_stroke(Rect::from_min_max(at(3.0, 6.0), at(11.0, 18.0)), 0.0, st, StrokeKind::Middle);
            p.add(Shape::dashed_line(&[at(11.0, 6.0), at(18.0, 6.0), at(18.0, 18.0), at(11.0, 18.0)], st, 2.0 * u, 1.8 * u));
            p.line_segment([at(13.0, 12.0), at(21.5, 12.0)], st);
            p.add(Shape::line(vec![at(18.5, 9.0), at(21.5, 12.0), at(18.5, 15.0)], st));
        }
        Icon::Move => {
            p.line_segment([at(12.0, 2.5), at(12.0, 21.5)], st);
            p.line_segment([at(2.5, 12.0), at(21.5, 12.0)], st);
            for (tx, ty, dx, dy) in
                [(12.0, 2.5, 0.0, 1.0), (12.0, 21.5, 0.0, -1.0), (2.5, 12.0, 1.0, 0.0), (21.5, 12.0, -1.0, 0.0)]
            {
                let (bx, by) = (tx + dx * 3.5, ty + dy * 3.5);
                p.add(Shape::line(vec![at(bx - dy * 3.0, by - dx * 3.0), at(tx, ty), at(bx + dy * 3.0, by + dx * 3.0)], st));
            }
        }
        Icon::Rectangle => {
            let r = Rect::from_min_max(at(3.5, 6.0), at(20.5, 18.0));
            p.rect_filled(r, 0.0, color.gamma_multiply(0.3));
            p.rect_stroke(r, 0.0, st, StrokeKind::Middle);
        }
        Icon::Polygon => {
            let pts = vec![at(3.5, 4.0), at(13.0, 4.0), at(13.0, 11.0), at(20.5, 11.0), at(20.5, 20.0), at(3.5, 20.0)];
            p.add(Shape::Path(eframe::epaint::PathShape {
                points: pts,
                closed: true,
                fill: Color32::TRANSPARENT,
                stroke: st.into(),
            }));
            p.rect_filled(Rect::from_min_max(at(4.5, 5.0), at(12.0, 19.0)), 0.0, color.gamma_multiply(0.3));
            p.rect_filled(Rect::from_min_max(at(12.0, 12.0), at(19.5, 19.0)), 0.0, color.gamma_multiply(0.3));
        }
        Icon::Path => {
            p.add(Shape::line(
                vec![at(3.0, 18.5), at(11.5, 18.5), at(11.5, 6.0), at(21.0, 6.0)],
                Stroke::new(4.2 * u, color.gamma_multiply(0.45)),
            ));
            p.add(Shape::line(
                vec![at(3.0, 18.5), at(11.5, 18.5), at(11.5, 6.0), at(21.0, 6.0)],
                Stroke::new(st.width * 0.6, color),
            ));
        }
        Icon::Text => {
            p.line_segment([at(5.0, 5.0), at(19.0, 5.0)], Stroke::new(st.width * 1.3, color));
            p.line_segment([at(12.0, 5.0), at(12.0, 20.0)], Stroke::new(st.width * 1.3, color));
            p.line_segment([at(9.0, 20.0), at(15.0, 20.0)], st);
        }
        Icon::Via => {
            let r = Rect::from_min_max(at(4.5, 4.5), at(19.5, 19.5));
            p.rect_stroke(r, 0.0, st, StrokeKind::Middle);
            p.line_segment([at(4.5, 4.5), at(19.5, 19.5)], st);
            p.line_segment([at(19.5, 4.5), at(4.5, 19.5)], st);
        }
        Icon::Instance => {
            p.rect_stroke(Rect::from_min_max(at(3.0, 4.0), at(21.0, 20.0)), 1.5 * u, st, StrokeKind::Middle);
            p.rect_filled(Rect::from_min_max(at(6.5, 7.5), at(11.0, 16.5)), 0.0, color.gamma_multiply(0.55));
            p.rect_filled(Rect::from_min_max(at(13.0, 7.5), at(17.5, 16.5)), 0.0, color.gamma_multiply(0.55));
        }
        Icon::Chop => {
            p.rect_stroke(Rect::from_min_max(at(3.0, 7.0), at(21.0, 17.0)), 0.0, st, StrokeKind::Middle);
            p.add(Shape::dashed_line(
                &[at(9.5, 4.0), at(9.5, 20.0), at(14.5, 20.0), at(14.5, 4.0), at(9.5, 4.0)],
                Stroke::new(st.width * 0.8, color),
                1.8 * u,
                1.5 * u,
            ));
        }
        Icon::Split => {
            p.rect_stroke(Rect::from_min_max(at(4.0, 6.0), at(20.0, 18.0)), 0.0, st, StrokeKind::Middle);
            p.line_segment([at(2.0, 12.0), at(22.0, 12.0)], Stroke::new(st.width, color));
            p.circle_filled(at(2.5, 12.0), 1.6 * u, color);
            p.circle_filled(at(21.5, 12.0), 1.6 * u, color);
        }
        Icon::Ruler => {
            p.rect_stroke(Rect::from_min_max(at(2.5, 8.0), at(21.5, 16.0)), 1.0 * u, st, StrokeKind::Middle);
            for (i, x) in [6.0f32, 9.5, 13.0, 16.5].iter().enumerate() {
                let h = if i % 2 == 0 { 12.5 } else { 11.0 };
                p.line_segment([at(*x, 8.0), at(*x, h)], Stroke::new(st.width * 0.8, color));
            }
        }
        Icon::Wire => {
            p.add(Shape::line(vec![at(4.0, 18.0), at(12.0, 18.0), at(12.0, 6.0), at(20.0, 6.0)], st));
            p.circle_filled(at(4.0, 18.0), 2.0 * u, color);
            p.circle_filled(at(20.0, 6.0), 2.0 * u, color);
        }
        Icon::NetLabel => {
            p.add(Shape::closed_line(vec![at(7.0, 7.5), at(19.0, 7.5), at(22.0, 12.0), at(19.0, 16.5), at(7.0, 16.5)], st));
            p.line_segment([at(2.0, 12.0), at(7.0, 12.0)], st);
            p.circle_filled(at(2.5, 12.0), 1.8 * u, color);
        }
        Icon::Grid => {
            for x in [5.0, 12.0, 19.0] {
                for y in [5.0, 12.0, 19.0] {
                    p.circle_filled(at(x, y), 1.6 * u, color);
                }
            }
        }
        Icon::Gravity => {
            p.circle_stroke(c, 6.5 * u, st);
            p.circle_filled(c, 2.0 * u, color);
            for (a, b) in [
                ((12.0, 1.5), (12.0, 5.0)),
                ((12.0, 19.0), (12.0, 22.5)),
                ((1.5, 12.0), (5.0, 12.0)),
                ((19.0, 12.0), (22.5, 12.0)),
            ] {
                p.line_segment([at(a.0, a.1), at(b.0, b.1)], st);
            }
        }
        Icon::Drd => {
            p.rect_filled(Rect::from_min_max(at(2.5, 5.0), at(8.0, 19.0)), 0.0, color.gamma_multiply(0.45));
            p.rect_filled(Rect::from_min_max(at(16.0, 5.0), at(21.5, 19.0)), 0.0, color.gamma_multiply(0.45));
            p.line_segment([at(8.5, 12.0), at(15.5, 12.0)], st);
            p.add(Shape::line(vec![at(11.0, 9.5), at(8.5, 12.0), at(11.0, 14.5)], st));
            p.add(Shape::line(vec![at(13.0, 9.5), at(15.5, 12.0), at(13.0, 14.5)], st));
        }
        Icon::Constraints => {
            p.rect_stroke(Rect::from_min_max(at(3.0, 8.0), at(9.5, 17.0)), 0.0, st, StrokeKind::Middle);
            p.rect_stroke(Rect::from_min_max(at(14.5, 8.0), at(21.0, 17.0)), 0.0, st, StrokeKind::Middle);
            p.add(Shape::dashed_line(&[at(12.0, 3.0), at(12.0, 21.0)], Stroke::new(st.width * 0.8, color), 2.0 * u, 1.8 * u));
            p.line_segment([at(1.5, 20.0), at(22.5, 20.0)], Stroke::new(st.width * 0.8, color.gamma_multiply(0.6)));
        }
        Icon::Magnet => {
            let mut pts = vec![at(5.0, 4.0)];
            pts.extend(arc(12.0, 12.5, 7.0, 180.0, 0.0).map(|(x, y)| at(x, y)));
            pts.push(at(19.0, 4.0));
            // arc() walks from 180° to 0° through the bottom (positive y is down).
            p.add(Shape::line(pts, Stroke::new(st.width * 1.6, color)));
            p.rect_filled(Rect::from_min_max(at(3.0, 3.0), at(7.0, 6.5)), 0.0, color);
            p.rect_filled(Rect::from_min_max(at(17.0, 3.0), at(21.0, 6.5)), 0.0, color);
        }
        Icon::LiveDrc => {
            p.circle_stroke(c, 8.5 * u, st);
            p.add(Shape::line(vec![at(7.5, 12.5), at(10.8, 15.8), at(16.8, 8.8)], Stroke::new(st.width * 1.2, color)));
        }
        Icon::Flylines => {
            for (a, b) in [((4.0, 18.0), (12.0, 6.0)), ((12.0, 6.0), (20.0, 16.0)), ((4.0, 18.0), (20.0, 16.0))] {
                p.add(Shape::dashed_line(&[at(a.0, a.1), at(b.0, b.1)], Stroke::new(st.width * 0.8, color), 2.0 * u, 1.6 * u));
            }
            for q in [(4.0, 18.0), (12.0, 6.0), (20.0, 16.0)] {
                p.circle_filled(at(q.0, q.1), 2.2 * u, color);
            }
        }
        Icon::Export => {
            p.line_segment([at(12.0, 3.0), at(12.0, 15.0)], st);
            p.add(Shape::line(vec![at(7.0, 10.5), at(12.0, 15.5), at(17.0, 10.5)], st));
            p.add(Shape::line(vec![at(4.0, 15.0), at(4.0, 20.5), at(20.0, 20.5), at(20.0, 15.0)], st));
        }
        Icon::Play => {
            p.add(Shape::convex_polygon(vec![at(7.0, 4.5), at(19.5, 12.0), at(7.0, 19.5)], color, Stroke::NONE));
        }
        Icon::Return => {
            p.add(Shape::line(vec![at(19.0, 20.0), at(19.0, 9.0), at(5.0, 9.0)], st));
            p.add(Shape::line(vec![at(9.5, 4.5), at(5.0, 9.0), at(9.5, 13.5)], st));
        }
        Icon::Annotate => {
            let pts: Vec<Pos2> = arc(12.0, 15.0, 8.5, 180.0, 360.0).map(|(x, y)| at(x, y)).collect();
            p.add(Shape::line(pts, st));
            p.line_segment([at(12.0, 15.0), at(16.5, 9.0)], st);
            p.circle_filled(at(12.0, 15.0), 1.8 * u, color);
            p.line_segment([at(3.5, 19.5), at(20.5, 19.5)], st);
        }
        Icon::PopOut | Icon::DockIn => {
            p.add(Shape::line(vec![at(11.0, 5.0), at(5.0, 5.0), at(5.0, 19.0), at(19.0, 19.0), at(19.0, 13.0)], st));
            let (tip, tail) = if icon == Icon::PopOut { ((19.5, 4.5), (11.5, 12.5)) } else { ((11.5, 12.5), (19.5, 4.5)) };
            let (tx, ty) = tip;
            let sx = if tail.0 > tx { 1.0 } else { -1.0 };
            p.line_segment([at(tail.0, tail.1), at(tx, ty)], st);
            p.add(Shape::line(vec![at(tx + sx * 6.0, ty), at(tx, ty), at(tx, ty - sx * 6.0)], st));
        }
    }
}

/// Points of a circular arc (degrees, positive y down) in icon grid units.
fn arc(cx: f32, cy: f32, r: f32, a0: f32, a1: f32) -> impl Iterator<Item = (f32, f32)> {
    let n = 20;
    (0..=n).map(move |i| {
        let a = (a0 + (a1 - a0) * i as f32 / n as f32).to_radians();
        (cx + r * a.cos(), cy + r * a.sin())
    })
}

/// A square toolbar button with a painted icon. `tip` is the tooltip; the text before " (" is
/// also the accessible name, so tests and screen readers find the button by it.
pub fn button(ui: &mut Ui, icon: Icon, selected: bool, enabled: bool, tip: &str) -> Response {
    let pal = theme::palette(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(28.0), if enabled { Sense::click() } else { Sense::hover() });
    let name = tip.split(" (").next().unwrap_or(tip).to_string();
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, enabled, selected, &name));
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        if selected {
            p.rect_filled(rect, 5.0, pal.accent.gamma_multiply(0.22));
            p.rect_stroke(rect, 5.0, Stroke::new(1.0, pal.accent.gamma_multiply(0.7)), StrokeKind::Inside);
        } else if enabled && resp.hovered() {
            p.rect_filled(rect, 5.0, pal.bg_hover);
        }
        let color = if !enabled {
            pal.text_weak.gamma_multiply(0.4)
        } else if selected {
            pal.accent
        } else if resp.hovered() {
            pal.text_strong
        } else {
            pal.text
        };
        paint(p, rect.shrink(5.0), icon, color);
    }
    let resp = if enabled { resp.on_hover_text(t(tip)) } else { resp.on_hover_text(tf("{} (not available now)", &[t(tip)])) };
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// An on/off toolbar button bound to a flag. Returns its response, marked changed on a click.
pub fn toggle(ui: &mut Ui, icon: Icon, on: &mut bool, tip: &str) -> Response {
    let mut resp = button(ui, icon, *on, true, tip);
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp
}

/// A thin vertical divider between toolbar groups.
pub fn divider(ui: &mut Ui) {
    let pal = theme::palette(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(9.0, 22.0), Sense::hover());
    ui.painter().line_segment([r.center_top(), r.center_bottom()], Stroke::new(1.0, pal.border));
}
