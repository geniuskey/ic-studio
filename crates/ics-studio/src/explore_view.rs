//! Explorable figures of the Learn course (`<!-- explore: mosfet -->`): sliders for W, L and
//! the voltages, and the curve redrawn as they move, from the square-law model the chapter
//! teaches. They make the equations something to play with before the simulator checks them.

use eframe::egui::{self, RichText};
use egui_plot::{Line, LineStyle, Plot, PlotPoints, Points, VLine};
use ics_core::units::format_eng;

use crate::i18n::{t, tf};
use crate::theme;

/// The generic technology's square-law numbers the chapters use.
const KN: f64 = 280e-6;
const KP: f64 = 70e-6;
const VTH: f64 = 0.45;
const VDD: f64 = 1.8;
/// Channel-length modulation at L = 1 µm, as the generic model cards give it (`lambda` 0.08 for
/// NMOS and 0.1 for PMOS at 0.5 µm, scaled as 1/L): 0.04 and 0.05.
const LAMBDA_N_1U: f64 = 0.04;
const LAMBDA_P_1U: f64 = 0.05;

/// The explorable figures, by the name a chapter gives.
pub const NAMES: &[&str] = &["mosfet", "inverter"];

/// What the sliders are set to, kept while the chapter is open.
#[derive(Clone)]
pub struct State {
    w: f64,
    l: f64,
    vgs: f64,
    vds: f64,
    wn: f64,
    wp: f64,
}

impl Default for State {
    fn default() -> Self {
        // The chapters' worked examples: W/L = 2 µm / 1 µm at 0.7 V; the inverter of the tutorial.
        State { w: 2.0, l: 1.0, vgs: 0.7, vds: 1.0, wn: 1.0, wp: 2.0 }
    }
}

/// A MOSFET's region and drain current (square law with λ).
pub fn square_law(k: f64, w_l: f64, lambda: f64, vgs: f64, vds: f64) -> (&'static str, f64) {
    let vov = vgs - VTH;
    if vov <= 0.0 {
        ("cut-off", 0.0)
    } else if vds < vov {
        ("triode", k * w_l * (vov - vds / 2.0) * vds * (1.0 + lambda * vds))
    } else {
        ("saturation", 0.5 * k * w_l * vov * vov * (1.0 + lambda * vds))
    }
}

/// The inverter's output for an input: where the NMOS and PMOS currents are equal
/// (bisection; the NMOS current rises and the PMOS current falls with the output).
pub fn inverter_out(wn: f64, wp: f64, l: f64, vin: f64) -> f64 {
    let diff = |vout: f64| {
        let n = square_law(KN, wn / l, LAMBDA_N_1U / l, vin, vout).1;
        let p = square_law(KP, wp / l, LAMBDA_P_1U / l, VDD - vin, VDD - vout).1;
        n - p
    };
    let (mut lo, mut hi) = (0.0, VDD);
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        if diff(mid) > 0.0 {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    (lo + hi) / 2.0
}

/// The switching threshold from the chapter's formula.
pub fn switching_threshold(wn: f64, wp: f64) -> f64 {
    let r = (KP * wp / (KN * wn)).sqrt();
    (VTH + r * (VDD - VTH)) / (1.0 + r)
}

fn slider(ui: &mut egui::Ui, label: &str, v: &mut f64, range: std::ops::RangeInclusive<f64>, unit: &str, log: bool) {
    ui.horizontal(|ui| {
        let l = ui.add_sized([60.0, 18.0], egui::Label::new(label));
        ui.spacing_mut().slider_width = (ui.available_width() - 110.0).clamp(100.0, 320.0);
        ui.add(egui::Slider::new(v, range).suffix(format!(" {unit}")).logarithmic(log).max_decimals(2)).labelled_by(l.id);
    });
}

fn region_text(r: &str) -> &'static str {
    match r {
        "cut-off" => t("cut-off"),
        "triode" => t("triode"),
        _ => t("saturation"),
    }
}

/// Draw the explorable figure `name`.
pub fn show(ui: &mut egui::Ui, name: &str, st: &mut State) {
    let pal = theme::palette(ui.ctx());
    egui::Frame::new().fill(pal.bg_canvas).stroke(egui::Stroke::new(1.0, pal.border)).corner_radius(6).inner_margin(10).show(
        ui,
        |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(t("Explore: move the sliders")).strong().color(pal.accent));
            match name {
                "mosfet" => mosfet(ui, st),
                "inverter" => inverter(ui, st),
                _ => {
                    ui.label(tf("Unknown explorable figure '{}'", &[name]));
                }
            }
            ui.label(
                RichText::new(t(
                    "A simple square-law model, as in the text. The simulator's model is more exact: check with it.",
                ))
                .small()
                .color(pal.text_weak),
            );
        },
    );
    ui.add_space(6.0);
}

fn mosfet(ui: &mut egui::Ui, st: &mut State) {
    let pal = theme::palette(ui.ctx());
    slider(ui, "W", &mut st.w, 0.22..=20.0, "µm", true);
    slider(ui, "L", &mut st.l, 0.18..=5.0, "µm", true);
    slider(ui, "V_GS", &mut st.vgs, 0.0..=1.8, "V", false);
    slider(ui, "V_DS", &mut st.vds, 0.0..=1.8, "V", false);
    let lambda = LAMBDA_N_1U / st.l;
    let w_l = st.w / st.l;
    let (region, id) = square_law(KN, w_l, lambda, st.vgs, st.vds);
    let vov = st.vgs - VTH;
    let gm = if region == "saturation" { 2.0 * id / vov } else { 0.0 };
    ui.horizontal_wrapped(|ui| {
        ui.label(tf("V_ov = {} V", &[&format!("{vov:.2}")]));
        ui.label(RichText::new(format!("I_D = {}A", format_eng(id, ""))).strong());
        let color = match region {
            "saturation" => pal.ok,
            "triode" => pal.warning,
            _ => pal.text_weak,
        };
        ui.label(RichText::new(region_text(region)).strong().color(color));
        if gm > 0.0 {
            ui.label(format!("g_m = {}S", format_eng(gm, "")));
        }
    });
    let hint = match region {
        "cut-off" => t("V_GS is below the threshold (0.45 V): no channel, no current. Raise V_GS."),
        "triode" => t("V_DS is below V_ov: the channel reaches the drain and the transistor acts as a resistor."),
        _ => t("V_DS is above V_ov: the channel is pinched off and the current hardly depends on V_DS. Try doubling W."),
    };
    ui.label(RichText::new(hint).color(pal.text_weak));
    let n = 90;
    let curve = |vgs: f64| -> PlotPoints {
        (0..=n)
            .map(|i| {
                let vds = VDD * i as f64 / n as f64;
                [vds, square_law(KN, w_l, lambda, vgs, vds).1 * 1e6]
            })
            .collect()
    };
    let edge: PlotPoints = (0..=n)
        .map(|i| {
            let vgs = VTH + ((st.vgs + 0.25).min(VDD) - VTH) * i as f64 / n as f64;
            let vds = vgs - VTH;
            [vds, square_law(KN, w_l, lambda, vgs, vds).1 * 1e6]
        })
        .filter(|p| p[0] <= VDD)
        .collect();
    Plot::new("explore_mos")
        .height(170.0)
        .allow_scroll(false)
        .allow_drag(false)
        .allow_zoom(false)
        .x_axis_label("V_DS (V)")
        .y_axis_label("I_D (µA)")
        .show(ui, |p| {
            // Neighbouring V_GS, so the curve shown keeps a readable scale.
            for d in [-0.2, -0.1, 0.1, 0.2] {
                let vgs = ((st.vgs + d) * 100.0).round() / 100.0;
                if vgs > VTH && vgs <= VDD {
                    p.line(Line::new(format!("V_GS = {vgs:.2} V"), curve(vgs)).color(pal.text_weak).width(1.0));
                }
            }
            p.line(Line::new(t("triode | saturation"), edge).color(pal.warning).style(LineStyle::dashed_dense()));
            p.line(Line::new(t("this V_GS"), curve(st.vgs)).color(pal.accent).width(2.0));
            p.points(Points::new(t("operating point"), vec![[st.vds, id * 1e6]]).radius(4.0).color(pal.accent));
        });
}

fn inverter(ui: &mut egui::Ui, st: &mut State) {
    let pal = theme::palette(ui.ctx());
    slider(ui, "W_n", &mut st.wn, 0.22..=8.0, "µm", true);
    slider(ui, "W_p", &mut st.wp, 0.22..=16.0, "µm", true);
    let l = 0.18;
    let n = 120;
    let vtc: Vec<[f64; 2]> = (0..=n)
        .map(|i| {
            let vin = VDD * i as f64 / n as f64;
            [vin, inverter_out(st.wn, st.wp, l, vin)]
        })
        .collect();
    // Where out = in, and the steepest slope (the gain in the middle).
    let vm = vtc.windows(2).find(|w| w[0][1] >= w[0][0] && w[1][1] < w[1][0]).map_or(VDD / 2.0, |w| w[0][0]);
    let gain = vtc.windows(2).map(|w| (w[1][1] - w[0][1]) / (w[1][0] - w[0][0])).fold(0.0f64, f64::min);
    let formula = switching_threshold(st.wn, st.wp);
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("V_M ≈ {vm:.2} V")).strong());
        ui.label(tf("formula: {} V", &[&format!("{formula:.2}")]));
        ui.label(tf("gain in the middle: {}", &[&format!("{gain:.0}")]));
        ui.label(format!("W_p / W_n = {:.1}", st.wp / st.wn));
    });
    let hint = if (st.wp / st.wn - 4.0).abs() < 0.3 {
        t("W_p ≈ 4·W_n makes the PMOS as strong as the NMOS: V_M sits at V_DD/2.")
    } else if st.wp / st.wn < 4.0 {
        t("The NMOS is stronger, so it pulls the output down early: V_M is below V_DD/2. Widen the PMOS.")
    } else {
        t("The PMOS is stronger, so it holds the output high longer: V_M is above V_DD/2.")
    };
    ui.label(RichText::new(hint).color(pal.text_weak));
    let diag: PlotPoints = vec![[0.0, 0.0], [VDD, VDD]].into();
    Plot::new("explore_inv")
        .height(170.0)
        .allow_scroll(false)
        .allow_drag(false)
        .allow_zoom(false)
        .include_x(0.0)
        .include_x(VDD)
        .include_y(0.0)
        .include_y(VDD)
        .x_axis_label(t("in (V)"))
        .y_axis_label(t("out (V)"))
        .show(ui, |p| {
            p.line(Line::new("out = in", diag).color(pal.text_weak).style(LineStyle::dashed_dense()));
            p.vline(VLine::new("V_DD/2", VDD / 2.0).color(pal.text_weak.gamma_multiply(0.5)));
            p.line(Line::new(t("transfer curve"), PlotPoints::from(vtc)).color(pal.accent).width(2.0));
            p.points(Points::new("V_M", vec![[vm, vm]]).radius(4.0).color(pal.warning));
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The model gives the numbers the chapters quote.
    #[test]
    fn matches_the_chapters() {
        // mosfet.md's worked example, without λ: 17.5 µA.
        let (r, id) = square_law(KN, 2.0, 0.0, 0.7, 1.0);
        assert_eq!(r, "saturation");
        assert!((id - 17.5e-6).abs() < 1e-8);
        assert_eq!(square_law(KN, 2.0, 0.0, 1.2, 0.3).0, "triode");
        assert_eq!(square_law(KN, 2.0, 0.0, 0.3, 1.0).0, "cut-off");
        // cmos-logic.md: V_M ≈ 0.83 V with W_p = 2·W_n and exactly V_DD/2 with 4·W_n.
        assert!((switching_threshold(1.0, 2.0) - 0.823).abs() < 0.01);
        assert!((switching_threshold(1.0, 4.0) - 0.9).abs() < 1e-9);
        let out = |vin| inverter_out(1.0, 4.0, 0.18, vin);
        assert!(out(0.0) > 1.79 && out(1.8) < 0.01);
        // The output crosses V_DD/2 at an input near 0.9 V (the NMOS and PMOS λ differ, and the
        // transfer curve is steep there, so the output at exactly 0.9 V is not 0.9 V).
        let (mut lo, mut hi) = (0.0, 1.8);
        for _ in 0..60 {
            let mid = (lo + hi) / 2.0;
            if out(mid) > 0.9 { lo = mid } else { hi = mid }
        }
        assert!((lo - 0.9).abs() < 0.02, "V_M {lo}");
    }
}
