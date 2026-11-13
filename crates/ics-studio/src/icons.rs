//! Vector icons painted with egui shapes (crisp at any DPI, no asset files).

use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};

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
