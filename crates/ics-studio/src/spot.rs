//! Spotlights: named places in the UI ("anchors") that the tutorial, the Learn course and the
//! copilot tutor point at, so a beginner sees where to click instead of reading a menu path.
//!
//! Widgets register their rectangle every frame with [`mark`]; a guide sets the target with
//! [`point`]; [`paint`] draws a pulsing outline and a note over everything at the end of the
//! frame. A target whose widget is not on screen this frame is not drawn; its [`Anchor`] says
//! which workspace shows it, so "Show me" can switch there first.

use std::collections::HashMap;

use eframe::egui::{self, Color32, FontId, Id, LayerId, Order, Rect, Stroke, StrokeKind, Vec2};

use crate::app::Workspace;

/// A named place in the UI.
pub struct Anchor {
    pub key: &'static str,
    /// What it is, for the copilot's tool description (English).
    pub what: &'static str,
    /// The workspace that shows it; `None` when it is always on screen.
    pub workspace: Option<Workspace>,
}

const fn a(key: &'static str, what: &'static str, workspace: Option<Workspace>) -> Anchor {
    Anchor { key, what, workspace }
}

/// Every anchor the UI registers.
pub const ANCHORS: &[Anchor] = &[
    a("ws.home", "the Home button of the activity bar on the far left", None),
    a("ws.schematic", "the Schematic workspace button of the activity bar", None),
    a("ws.simulation", "the Simulation workspace button of the activity bar", None),
    a("ws.layout", "the Layout workspace button of the activity bar", None),
    a("explorer", "the Explorer: the project's libraries and cells, on the left", None),
    a("palette", "the command search box at the top right (Ctrl+K)", None),
    a("menu.file", "the File menu", None),
    a("menu.simulation", "the Simulation menu", None),
    a("menu.verify", "the Verify menu (DRC, LVS, extraction)", None),
    a("menu.help", "the Help menu (Learn, tutorial, user guide)", None),
    a("home.learn", "the Learn IC design card on the welcome page", Some(Workspace::Home)),
    a("home.demo", "the Open demo project card on the welcome page", Some(Workspace::Home)),
    a("home.new_project", "the New project card on the welcome page", Some(Workspace::Home)),
    a(
        "sch.library",
        "the Device Library on the left of the schematic: click a device, then the canvas to place it",
        Some(Workspace::Schematic),
    ),
    a("sch.canvas", "the schematic canvas", Some(Workspace::Schematic)),
    a(
        "sch.inspector",
        "the inspector on the right of the schematic: parameters of the selected device",
        Some(Workspace::Schematic),
    ),
    a(
        "sch.design",
        "the Design section of the schematic inspector (nothing selected): specs and Derive a new design",
        Some(Workspace::Schematic),
    ),
    a("sch.run", "the Simulate button at the right end of the schematic toolbar (F5)", Some(Workspace::Schematic)),
    a("sim.run", "the Run Simulation button of the simulation workspace (F5)", Some(Workspace::Simulation)),
    a(
        "sim.analyses",
        "the Analyses list of the simulation setup (operating point, DC, AC, transient)",
        Some(Workspace::Simulation),
    ),
    a(
        "sim.results",
        "the results area of the simulation workspace: waveforms or the operating point tables",
        Some(Workspace::Simulation),
    ),
    a("sim.signals", "the Signals list next to the waveforms: which signals are plotted", Some(Workspace::Simulation)),
    a(
        "sim.measurements",
        "the Measurements table under the waveforms (min, max, delay, gain, bandwidth...)",
        Some(Workspace::Simulation),
    ),
    a("lay.canvas", "the layout canvas", Some(Workspace::Layout)),
    a("lay.verify", "the Verify panel of the layout workspace (DRC, LVS)", Some(Workspace::Layout)),
    a("lay.run_drc", "the Run DRC button of the layout Verify panel", Some(Workspace::Layout)),
    a("learn", "the Learn IC Design window", None),
    a("tutor.step", "the tutor's current step card", None),
];

/// The activity bar button of a workspace.
pub fn workspace_key(w: Workspace) -> &'static str {
    match w {
        Workspace::Home => "ws.home",
        Workspace::Schematic => "ws.schematic",
        Workspace::Simulation => "ws.simulation",
        Workspace::Layout => "ws.layout",
    }
}

pub fn anchor(key: &str) -> Option<&'static Anchor> {
    ANCHORS.iter().find(|a| a.key == key)
}

#[derive(Clone)]
struct Target {
    key: String,
    note: String,
    /// Time (ctx input time) until which it shows; `None` until cleared.
    until: Option<f64>,
}

#[derive(Clone, Default)]
struct State {
    rects: HashMap<&'static str, (Rect, u64)>,
    target: Option<Target>,
    /// Shown for one frame only (the tutorial sets it every frame).
    frame: Option<(String, u64)>,
}

fn id() -> Id {
    Id::unique("ics_spotlight")
}

fn with<R>(ctx: &egui::Context, f: impl FnOnce(&mut State) -> R) -> R {
    ctx.data_mut(|d| f(d.get_temp_mut_or_default::<State>(id())))
}

/// Register where an anchor is drawn this frame.
pub fn mark(ctx: &egui::Context, key: &'static str, rect: Rect) {
    debug_assert!(anchor(key).is_some(), "unknown anchor {key}");
    let pass = ctx.cumulative_pass_nr();
    with(ctx, |s| {
        s.rects.insert(key, (rect, pass));
    });
}

/// Point at an anchor with a note, for `secs` seconds (`None`: until [`clear`]).
pub fn point(ctx: &egui::Context, key: &str, note: &str, secs: Option<f64>) {
    let now = ctx.input(|i| i.time);
    with(ctx, |s| s.target = Some(Target { key: key.into(), note: note.into(), until: secs.map(|d| now + d) }));
    ctx.request_repaint();
}

/// Point at an anchor during this frame only.
pub fn point_now(ctx: &egui::Context, key: &str) {
    let pass = ctx.cumulative_pass_nr();
    with(ctx, |s| s.frame = Some((key.into(), pass)));
}

pub fn clear(ctx: &egui::Context) {
    with(ctx, |s| s.target = None);
}

/// The anchor pointed at, if any.
pub fn target(ctx: &egui::Context) -> Option<String> {
    let now = ctx.input(|i| i.time);
    with(ctx, |s| s.target.as_ref().filter(|t| t.until.is_none_or(|u| u > now)).map(|t| t.key.clone()))
}

/// Where an anchor was drawn this frame or the last one.
pub fn rect(ctx: &egui::Context, key: &str) -> Option<Rect> {
    let pass = ctx.cumulative_pass_nr();
    with(ctx, |s| s.rects.get(key).filter(|(_, p)| *p + 1 >= pass).map(|(r, _)| *r))
}

/// Draw the spotlights over everything. Call once at the end of the frame.
pub fn paint(ctx: &egui::Context, color: Color32, note_bg: Color32, note_fg: Color32) {
    let now = ctx.input(|i| i.time);
    let pass = ctx.cumulative_pass_nr();
    let (targets, rects) = with(ctx, |s| {
        if s.target.as_ref().is_some_and(|t| t.until.is_some_and(|u| u <= now)) {
            s.target = None;
        }
        let mut v: Vec<(String, String)> = s.target.iter().map(|t| (t.key.clone(), t.note.clone())).collect();
        if let Some((k, p)) = &s.frame
            && *p == pass
            && !v.iter().any(|(x, _)| x == k)
        {
            v.push((k.clone(), String::new()));
        }
        (v, s.rects.clone())
    });
    if targets.is_empty() {
        return;
    }
    let painter = ctx.layer_painter(LayerId::new(Order::Tooltip, id()));
    let screen = ctx.content_rect();
    let pulse = 0.55 + 0.45 * (now * 4.0).sin().abs() as f32;
    let mut any = false;
    for (key, note) in targets {
        let Some((r, p)) = rects.get(key.as_str()) else { continue };
        if *p + 1 < pass {
            continue;
        }
        any = true;
        let r = r.expand(3.0).intersect(screen.shrink(1.0));
        painter.rect_stroke(r, 8.0, Stroke::new(3.0, color.gamma_multiply(pulse)), StrokeKind::Outside);
        painter.rect_stroke(r.expand(5.0), 11.0, Stroke::new(1.5, color.gamma_multiply(pulse * 0.4)), StrokeKind::Outside);
        if !note.trim().is_empty() {
            let font = FontId::proportional(13.0);
            let galley = painter.layout(note, font, note_fg, 300.0);
            let size = galley.size() + Vec2::new(16.0, 10.0);
            // Below the target if it fits, else above; kept on screen.
            let below = r.bottom() + 10.0 + size.y < screen.bottom();
            let y = if below { r.bottom() + 10.0 } else { r.top() - 10.0 - size.y };
            let x = (r.center().x - size.x / 2.0).clamp(screen.left() + 4.0, (screen.right() - size.x - 4.0).max(screen.left()));
            let bubble = Rect::from_min_size(egui::pos2(x, y), size);
            painter.rect_filled(bubble, 6.0, note_bg);
            painter.rect_stroke(bubble, 6.0, Stroke::new(1.5, color), StrokeKind::Inside);
            let tip = if below { bubble.center_top() } else { bubble.center_bottom() };
            let tip_x = r.center().x.clamp(bubble.left() + 10.0, bubble.right() - 10.0);
            let dir = if below { -1.0 } else { 1.0 };
            painter.add(egui::Shape::convex_polygon(
                vec![egui::pos2(tip_x - 6.0, tip.y), egui::pos2(tip_x + 6.0, tip.y), egui::pos2(tip_x, tip.y + 7.0 * dir)],
                color,
                Stroke::NONE,
            ));
            painter.galley(bubble.min + Vec2::new(8.0, 5.0), galley, note_fg);
        }
    }
    if any {
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_are_unique() {
        let mut keys: Vec<_> = ANCHORS.iter().map(|a| a.key).collect();
        keys.sort();
        let n = keys.len();
        keys.dedup();
        assert_eq!(n, keys.len());
    }
}
