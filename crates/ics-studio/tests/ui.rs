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
