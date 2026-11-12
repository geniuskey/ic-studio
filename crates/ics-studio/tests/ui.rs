//! Headless UI smoke tests: drive the full app through each workspace with a real wgpu
//! renderer. Set `ICS_SHOT_DIR=<dir>` to also write PNG screenshots of every scenario.

use std::time::Duration;

use eframe::egui;
use egui_kittest::Harness;
use ics_studio::LaunchArgs;
use ics_studio::app::{Command, IcStudio, Workspace};

fn scenario(name: &str, demo: bool, cell: Option<&str>, workspace: &str, run: bool) {
    scenario_then(name, demo, cell, workspace, run, None);
}

fn scenario_then(name: &str, demo: bool, cell: Option<&str>, workspace: &str, run: bool, then: Option<Command>) {
    let args = LaunchArgs { project: None, demo, workspace: Some(workspace.into()), cell: cell.map(str::to_string), run };
    let mut h = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_eframe(|cc| IcStudio::new(cc, args));
    h.run_steps(4);
    let mut waited = 0;
    while h.state().is_simulating() && waited < 300 {
        std::thread::sleep(Duration::from_millis(20));
        h.step();
        waited += 1;
    }
    assert!(!h.state().is_simulating(), "{name}: simulation did not finish");
    if let Some(c) = then {
        h.state_mut().command(c);
    }
    h.run_steps(6);
    let img = h.render().expect("render");
    if let Ok(dir) = std::env::var("ICS_SHOT_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        img.save(std::path::Path::new(&dir).join(format!("{name}.png"))).unwrap();
    }
}

#[test]
fn welcome_screen() {
    scenario("01_welcome", false, None, "home", false);
}

#[test]
fn project_home() {
    scenario("02_project_home", true, None, "home", false);
}

#[test]
fn schematic_workspace() {
    scenario("03_schematic", true, Some("cmos_inverter"), "schematic", false);
}

#[test]
fn simulation_workspace() {
    scenario("04_simulation", true, Some("cmos_inverter"), "simulation", true);
}

#[test]
fn simulation_ac() {
    scenario("05_simulation_rc", true, Some("rc_lowpass"), "simulation", true);
}

#[test]
fn layout_workspace() {
    scenario("06_layout", true, Some("cmos_inverter"), "layout", false);
}

#[test]
fn layout_hierarchy() {
    scenario("08_layout_hierarchy", true, Some("inv_chain"), "layout", false);
}

#[test]
fn layout_array_of_arrays() {
    scenario("09_layout_tiles", true, Some("inv_tiles"), "layout", false);
}

#[test]
fn layout_drc() {
    scenario_then("11_layout_drc", true, Some("inv_chain"), "layout", false, Some(Command::RunDrc));
}

#[test]
fn layout_lvs() {
    scenario_then("12_layout_lvs", true, Some("cmos_inverter"), "layout", false, Some(Command::RunLvs));
}

#[test]
fn layout_from_schematic() {
    scenario_then("13_layout_sdl", true, Some("cmos_inverter"), "schematic", false, Some(Command::GenerateLayout));
}

#[test]
fn layout_pcells() {
    scenario("10_layout_pcells", true, Some("devices"), "layout", false);
}

fn shape_count(h: &Harness<'_, IcStudio>, cell: &str) -> usize {
    let p = h.state().project().unwrap();
    let (_, c) = p.all_cells().find(|(_, c)| c.name == cell).unwrap();
    c.layout.as_ref().unwrap().shapes.len()
}

fn click(h: &mut Harness<'_, IcStudio>, p: egui::Pos2) {
    h.hover_at(p);
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        });
        h.step();
    }
    // Let the double-click window pass so separate clicks stay separate.
    std::thread::sleep(Duration::from_millis(350));
    h.step();
}

/// Draw an L-shaped polygon with the polygon tool, then undo it.
#[test]
fn layout_draw_polygon() {
    let args =
        LaunchArgs { project: None, demo: true, workspace: Some("layout".into()), cell: Some("inv_chain".into()), run: false };
    let mut h = Harness::builder().with_size(egui::vec2(1440.0, 900.0)).wgpu().build_eframe(|cc| IcStudio::new(cc, args));
    h.run_steps(4);
    let before = shape_count(&h, "inv_chain");
    h.hover_at(egui::pos2(800.0, 650.0));
    h.run_steps(2);
    h.key_press(egui::Key::G);
    h.run_steps(2);
    for p in [(700.0, 620.0), (900.0, 620.0), (900.0, 660.0), (760.0, 660.0), (760.0, 700.0)] {
        click(&mut h, egui::pos2(p.0, p.1));
    }
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(shape_count(&h, "inv_chain"), before + 1, "polygon should be added");
    let p = h.state().project().unwrap();
    let (_, c) = p.all_cells().find(|(_, c)| c.name == "inv_chain").unwrap();
    let last = c.layout.as_ref().unwrap().shapes.last().unwrap();
    assert!(matches!(&last.geom, ics_core::Geometry::Polygon { points } if points.len() == 6), "{:?}", last.geom);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    h.run_steps(3);
    assert_eq!(shape_count(&h, "inv_chain"), before);
}

#[test]
fn schematic_annotated() {
    // Run OP first, then show the annotated schematic.
    scenario_then(
        "07_schematic_op",
        true,
        Some("cmos_inverter"),
        "schematic",
        true,
        Some(Command::SetWorkspace(Workspace::Schematic)),
    );
}

/// Every non-ASCII character used in UI source must be drawable by the bundled fonts,
/// otherwise users would see "tofu" boxes.
#[test]
fn glyph_coverage() {
    let sources = [
        include_str!("../src/app.rs"),
        include_str!("../src/home_view.rs"),
        include_str!("../src/layout_view.rs"),
        include_str!("../src/layout_view/edit.rs"),
        include_str!("../src/layout_view/verify.rs"),
        include_str!("../src/schematic_view.rs"),
        include_str!("../src/sim_view.rs"),
        include_str!("../src/theme.rs"),
        include_str!("../../ics-core/src/units.rs"),
        include_str!("../../ics-core/src/schematic.rs"),
    ];
    let mut chars: Vec<char> = sources.iter().flat_map(|s| s.chars()).filter(|c| !c.is_ascii()).collect();
    chars.sort();
    chars.dedup();
    let mut h = Harness::builder().build_ui(|ui| {
        ui.label("x");
    });
    ics_studio::theme::setup_fonts(&h.ctx);
    h.run();
    let missing: Vec<char> =
        h.ctx.fonts_mut(|f| chars.iter().copied().filter(|c| !f.has_glyph(&egui::FontId::proportional(13.0), *c)).collect());
    assert!(
        missing.is_empty(),
        "missing glyphs: {}",
        missing.iter().map(|c| format!("{c} U+{:04X}", *c as u32)).collect::<Vec<_>>().join(", ")
    );
}

fn instance_count(h: &Harness<'_, IcStudio>) -> usize {
    let p = h.state().project().unwrap();
    let (_, c) = p.all_cells().find(|(_, c)| c.name == "rc_lowpass").unwrap();
    c.schematic.as_ref().unwrap().instances.len()
}

/// Place a resistor with the keyboard + mouse, then undo it.
#[test]
fn schematic_place_and_undo() {
    let args = LaunchArgs {
        project: None,
        demo: true,
        workspace: Some("schematic".into()),
        cell: Some("rc_lowpass".into()),
        run: false,
    };
    let mut h = Harness::builder().with_size(egui::vec2(1440.0, 900.0)).wgpu().build_eframe(|cc| IcStudio::new(cc, args));
    h.run_steps(4);
    let before = instance_count(&h);
    // Somewhere empty in the canvas (lower part of the central area).
    let p = egui::pos2(760.0, 640.0);
    h.hover_at(p);
    h.run_steps(2);
    h.key_press(egui::Key::R);
    h.run_steps(2);
    h.hover_at(p);
    h.event(egui::Event::PointerButton {
        pos: p,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Default::default(),
    });
    h.step();
    h.event(egui::Event::PointerButton {
        pos: p,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Default::default(),
    });
    h.run_steps(3);
    assert_eq!(instance_count(&h), before + 1, "resistor should be placed");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    h.run_steps(3);
    assert_eq!(instance_count(&h), before, "undo should remove it");
}

/// Live mode: MCP tool calls arriving over the bridge channel run on the open project and
/// switch the window to their result.
#[test]
fn mcp_live_requests() {
    let args = LaunchArgs { project: None, demo: true, workspace: Some("home".into()), cell: None, run: false };
    let mut h = Harness::builder().with_size(egui::vec2(1440.0, 900.0)).wgpu().build_eframe(|cc| IcStudio::new(cc, args));
    let (tx, rx) = std::sync::mpsc::channel();
    h.state_mut().attach_bridge(rx);
    h.run_steps(2);
    let call = |h: &mut Harness<'_, IcStudio>, tool: &str, args: serde_json::Value| {
        let (reply, answer) = std::sync::mpsc::channel();
        let arguments = args.as_object().unwrap().clone();
        tx.send(ics_mcp::bridge::Pending { tool: tool.into(), arguments, reply }).unwrap();
        h.run_steps(3);
        answer.try_recv().expect("answered on the UI thread")
    };
    let status = call(&mut h, "gui_status", serde_json::json!({})).value().unwrap();
    assert_eq!(status["live"], true);
    assert_eq!(status["project"]["unsaved_changes"], false);

    let before = shape_count(&h, "cmos_inverter");
    let r = call(
        &mut h,
        "add_rect",
        serde_json::json!({"cell": "cmos_inverter", "layer": "metal1", "x0": 0, "y0": -3, "x1": 2, "y1": -2}),
    );
    assert!(!r.is_error, "{r:?}");
    assert_eq!(shape_count(&h, "cmos_inverter"), before + 1);
    assert_eq!(h.state().workspace(), Workspace::Layout);
    h.state_mut().command(Command::Undo);
    h.run_steps(2);
    assert_eq!(shape_count(&h, "cmos_inverter"), before, "MCP edits are undoable");

    let lvs = call(&mut h, "run_lvs", serde_json::json!({"cell": "cmos_inverter"})).value().unwrap();
    assert_eq!(lvs["differences"], 2);
    assert_eq!(h.state().workspace(), Workspace::Layout);
    let bad = call(&mut h, "run_drc", serde_json::json!({"cell": "nope"}));
    assert!(bad.is_error);
    h.run_steps(4);
    let img = h.render().expect("render");
    if let Ok(dir) = std::env::var("ICS_SHOT_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        img.save(std::path::Path::new(&dir).join("14_mcp_live.png")).unwrap();
    }
}
