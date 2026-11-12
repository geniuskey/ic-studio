//! Headless UI smoke tests: drive the full app through each workspace with a real wgpu
//! renderer. Set `ICS_SHOT_DIR=<dir>` to also write PNG screenshots of every scenario.

use std::time::Duration;

use eframe::egui;
use egui_kittest::Harness;
use ics_studio::LaunchArgs;
use ics_studio::app::{Command, IcStudio, Workspace};

/// Step until background work (simulation, verification, MCP calls) is done, then let the
/// results render.
fn settle(h: &mut Harness<'_, IcStudio>, name: &str) {
    let t = std::time::Instant::now();
    while h.state().is_busy() && t.elapsed() < Duration::from_secs(60) {
        std::thread::sleep(Duration::from_millis(10));
        h.step();
    }
    assert!(!h.state().is_busy(), "{name}: background work did not finish");
    h.run_steps(2);
}

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
    settle(&mut h, name);
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
fn shortcuts_dialog() {
    scenario_then("14_shortcuts", true, Some("cmos_inverter"), "schematic", false, Some(Command::Shortcuts));
}

#[test]
fn layout_from_schematic() {
    scenario_then("13_layout_sdl", true, Some("cmos_inverter"), "schematic", false, Some(Command::GenerateLayout));
}

#[test]
fn layout_routed() {
    let args = LaunchArgs {
        project: None,
        demo: true,
        workspace: Some("schematic".into()),
        cell: Some("cmos_inverter".into()),
        run: false,
    };
    let mut h = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_eframe(|cc| IcStudio::new(cc, args));
    h.run_steps(4);
    h.state_mut().command(Command::GenerateLayout);
    h.run_steps(2);
    let before = shape_count(&h, "cmos_inverter");
    h.state_mut().command(Command::RouteFlylines);
    h.run_steps(6);
    let p = h.state().project().unwrap();
    let r = ics_core::project::CellRef::new("work", "cmos_inverter");
    let c = p.cell(&r).unwrap();
    let left = ics_core::sdl::ratsnest(c.schematic.as_ref().unwrap(), c.layout.as_ref().unwrap(), p, &p.tech);
    assert!(left.is_empty(), "{left:?}");
    assert!(shape_count(&h, "cmos_inverter") > before);
    settle(&mut h, "14_layout_routed");
    let img = h.render().expect("render");
    if let Ok(dir) = std::env::var("ICS_SHOT_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        img.save(std::path::Path::new(&dir).join("14_layout_routed.png")).unwrap();
    }
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
        settle(h, tool);
        answer.try_recv().expect("answered")
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

/// Live mode: write_schematic draws a schematic from a netlist, overwriting is undoable, and
/// set_simulation switches to the Simulation workspace.
#[test]
fn mcp_live_write_schematic() {
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
        settle(h, tool);
        answer.try_recv().expect("answered")
    };
    let sch = |h: &Harness<'_, IcStudio>, cell: &str| {
        let p = h.state().project().unwrap();
        p.all_cells().find(|(_, c)| c.name == cell).and_then(|(_, c)| c.schematic.clone())
    };
    let ota = "M1 x inp tail 0 nmos w=4u l=500n\nM2 out inn tail 0 nmos w=4u l=500n\nM3 x x vdd vdd pmos w=4u l=500n\n\
               M4 out x vdd vdd pmos w=4u l=500n\nM5 tail vb 0 0 nmos w=2u l=1u\nC1 out 0 100f\nV1 vdd 0 1.8\n\
               V2 inp 0 DC 0.9 AC 1\nV3 inn 0 0.9\nV4 vb 0 0.65\n.op\n.ac dec 20 1 10g\n";
    let r = call(&mut h, "write_schematic", serde_json::json!({"cell": "ota_net", "netlist": ota}));
    assert!(!r.is_error, "{r:?}");
    assert_eq!(h.state().workspace(), Workspace::Schematic);
    assert_eq!(sch(&h, "ota_net").unwrap().instances.iter().filter(|i| i.kind.spice_prefix().is_some()).count(), 10);
    h.run_steps(4);
    let img = h.render().expect("render");
    if let Ok(dir) = std::env::var("ICS_SHOT_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        img.save(std::path::Path::new(&dir).join("15_mcp_write_schematic.png")).unwrap();
    }

    let original = sch(&h, "cmos_inverter").unwrap();
    let inv = "M1 out in vdd vdd pmos w=4u l=180n\nM2 out in 0 0 nmos w=2u l=180n\nV1 vdd 0 1.8\nV2 in 0 0\n";
    assert!(call(&mut h, "write_schematic", serde_json::json!({"cell": "cmos_inverter", "netlist": inv})).is_error);
    let r = call(&mut h, "write_schematic", serde_json::json!({"cell": "cmos_inverter", "netlist": inv, "overwrite": true}));
    assert!(!r.is_error, "{r:?}");
    assert_ne!(sch(&h, "cmos_inverter").unwrap(), original);
    h.state_mut().command(Command::Undo);
    h.run_steps(2);
    assert_eq!(sch(&h, "cmos_inverter").unwrap(), original, "overwriting a schematic is undoable");

    let r = call(&mut h, "set_simulation", serde_json::json!({"cell": "ota_net", "analyses": [{"type": "op"}]}));
    assert!(!r.is_error, "{r:?}");
    assert_eq!(h.state().workspace(), Workspace::Simulation);
}

/// A saved demo project plus cell `work/big`: an array of `inv_tiles` large enough that
/// DRC takes a noticeable time.
fn big_project(test: &str) -> (std::path::PathBuf, ics_core::CellRef) {
    let dir = std::env::temp_dir().join(format!("ics-ui-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut p = ics_core::examples::demo_project();
    let (tiles, _) = p.all_cells().find(|(_, c)| c.name == "inv_tiles").unwrap();
    let mut lay = ics_core::Layout::default();
    let step = 2_000_000;
    let array = ics_core::ArraySpec { cols: 2, rows: 1, col: (step, 0), row: (0, step) };
    lay.add_instance(tiles, ics_core::Trans::new(0, 0), Some(array));
    let mut cell = ics_core::Cell::new("big");
    cell.layout = Some(lay);
    let r = p.add_cell("work", cell).unwrap();
    p.save_as(&dir).unwrap();
    (dir, r)
}

fn open_layout(dir: &std::path::Path, cell: &str) -> Harness<'static, IcStudio> {
    let args = LaunchArgs {
        project: Some(dir.to_path_buf()),
        demo: false,
        workspace: Some("layout".into()),
        cell: Some(cell.into()),
        run: false,
    };
    let mut h = Harness::builder().with_size(egui::vec2(1440.0, 900.0)).wgpu().build_eframe(|cc| IcStudio::new(cc, args));
    h.run_steps(4);
    h
}

/// Step while the app is busy; returns (frames, slowest frame).
fn frames_while_busy(h: &mut Harness<'_, IcStudio>) -> (usize, Duration) {
    let (mut frames, mut slowest, t) = (0, Duration::ZERO, std::time::Instant::now());
    while h.state().is_busy() && t.elapsed() < Duration::from_secs(120) {
        let f = std::time::Instant::now();
        h.step();
        slowest = slowest.max(f.elapsed());
        frames += 1;
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!h.state().is_busy(), "background work did not finish");
    (frames, slowest)
}

/// DRC on a large cell runs on a worker thread: frames keep coming and the report lands in
/// the editor when it is done.
#[test]
fn verification_in_background() {
    let (dir, big) = big_project("verify");
    let mut h = open_layout(&dir, "big");
    let t = std::time::Instant::now();
    h.state_mut().command(Command::RunDrc);
    h.step();
    assert!(h.state().is_busy(), "DRC should still be running");
    let (frames, slowest) = frames_while_busy(&mut h);
    let total = t.elapsed();
    assert!(frames >= 5, "only {frames} frame(s) while DRC ran for {total:?}");
    assert!(slowest < total / 2, "a frame took {slowest:?} of {total:?}");
    h.run_steps(2);
    let ed = h.state().layout_editor(&big).expect("editor");
    let drc = ed.verify.drc.as_ref().expect("DRC report installed");
    assert!(drc.shapes > 100_000, "{} shapes", drc.shapes);
    assert!(!ed.verify.stale);
    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Live mode: a slow read-only tool runs off the UI thread. Calls behind it are still served
/// while it runs, and its reply arrives when it is done.
#[test]
fn mcp_heavy_tool_in_background() {
    let (dir, big) = big_project("mcp");
    let mut h = open_layout(&dir, "big");
    let (tx, rx) = std::sync::mpsc::channel();
    h.state_mut().attach_bridge(rx);
    let send = |tool: &str, args: serde_json::Value| {
        let (reply, answer) = std::sync::mpsc::channel();
        let arguments = args.as_object().unwrap().clone();
        tx.send(ics_mcp::bridge::Pending { tool: tool.into(), arguments, reply }).unwrap();
        answer
    };
    let drc = send("run_drc", serde_json::json!({"cell": "big"}));
    h.run_steps(2);
    assert!(drc.try_recv().is_err(), "run_drc should still be running");
    let status = send("gui_status", serde_json::json!({}));
    h.run_steps(2);
    let s = status.try_recv().expect("gui_status answered while run_drc runs").value().unwrap();
    assert_eq!(s["live"], true);
    assert!(drc.try_recv().is_err(), "run_drc should still be running");
    let (frames, _) = frames_while_busy(&mut h);
    assert!(frames >= 3, "only {frames} frame(s) while run_drc ran");
    let v = drc.try_recv().expect("run_drc answered").value().unwrap();
    assert_eq!(v["cell"], big.to_string());
    assert!(v["shapes_checked"].as_u64().unwrap() > 100_000, "{v}");
    h.run_steps(2);
    assert_eq!(h.state().workspace(), Workspace::Layout);
    let ed = h.state().layout_editor(&big).expect("editor");
    assert_eq!(ed.verify.drc.as_ref().map(|r| r.shapes as u64), v["shapes_checked"].as_u64(), "the GUI shows the same report");
    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Frame times on a large real design. Run with
/// `ICS_PERF_PROJECT=<project dir> ICS_PERF_CELL=<cell> cargo test --release -p ics-studio --test ui perf_ -- --ignored --nocapture`
#[test]
#[ignore]
fn perf_large_layout() {
    let (Ok(project), Ok(cell)) = (std::env::var("ICS_PERF_PROJECT"), std::env::var("ICS_PERF_CELL")) else {
        eprintln!("set ICS_PERF_PROJECT and ICS_PERF_CELL");
        return;
    };
    let t = std::time::Instant::now();
    let args =
        LaunchArgs { project: Some(project.into()), demo: false, workspace: Some("layout".into()), cell: Some(cell), run: false };
    let mut h = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_eframe(|cc| IcStudio::new(cc, args));
    println!("open project: {:.2} s", t.elapsed().as_secs_f64());
    let frame = |h: &mut Harness<'_, IcStudio>| {
        let t = std::time::Instant::now();
        h.step();
        let ui = t.elapsed().as_secs_f64() * 1000.0;
        if std::env::var("ICS_PROFILE").is_ok() {
            let shapes = h.output().shapes.clone();
            let n = shapes.len();
            let t2 = std::time::Instant::now();
            let prims = h.ctx.tessellate(shapes, 1.0);
            let verts: usize = prims
                .iter()
                .map(|p| if let egui::epaint::Primitive::Mesh(m) = &p.primitive { m.vertices.len() } else { 0 })
                .sum();
            let tess = t2.elapsed().as_secs_f64() * 1000.0;
            let t3 = std::time::Instant::now();
            let _ = h.render();
            eprintln!(
                "PROFILE frame: app {ui:.0} ms, tessellate {tess:.0} ms ({n} shapes, {verts} vertices), render+readback {:.0} ms",
                t3.elapsed().as_secs_f64() * 1000.0
            );
        } else {
            let _ = h.render();
        }
        t.elapsed().as_secs_f64() * 1000.0
    };
    let first = frame(&mut h);
    println!("first frame (index build): {first:.0} ms");
    for _ in 0..3 {
        frame(&mut h);
    }
    let center = egui::pos2(880.0, 420.0);
    for (label, zooms) in [("full view", 0), ("zoom x8", 3), ("zoom x64", 3), ("zoom x512", 3)] {
        for _ in 0..zooms {
            h.hover_at(center);
            h.event(egui::Event::Zoom(2.0));
            frame(&mut h);
        }
        let ms: Vec<f64> = (0..5).map(|_| frame(&mut h)).collect();
        let avg = ms.iter().sum::<f64>() / ms.len() as f64;
        println!("{label}: {avg:.1} ms/frame");
        if let Ok(dir) = std::env::var("ICS_SHOT_DIR") {
            std::fs::create_dir_all(&dir).unwrap();
            let name = format!("perf_{}.png", label.replace(' ', "_"));
            h.render().unwrap().save(std::path::Path::new(&dir).join(name)).unwrap();
        }
    }
}

/// Sign-off section: a KLayout DRC runset on the demo inverter, run through the command. The
/// result lands in the editor (as DRC markers when KLayout is installed, else as an error).
#[test]
fn layout_signoff() {
    let dir = std::env::temp_dir().join(format!("ics-ui-signoff-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let deck = dir.join("m1.drc");
    std::fs::write(
        &deck,
        "source($input, $topcell)\nreport(\"m1\", $report)\ninput(7, 0).width(0.5).output(\"M1.W\", \"metal1 width < 0.5 um\")\n",
    )
    .unwrap();
    let mut p = ics_core::examples::demo_project();
    p.tech.signoff.push(ics_core::signoff::Runset {
        name: "KLayout metal1 width".into(),
        tool: ics_core::signoff::Tool::Klayout,
        check: ics_core::signoff::Check::Drc,
        deck: deck.to_string_lossy().into_owned(),
        vars: Default::default(),
        klayout: Default::default(),
        skip: vec!["V".into(), "I".into()],
        ground: "gnd".into(),
    });
    let proj = dir.join("proj");
    p.save_as(&proj).unwrap();
    let mut h = open_layout(&proj, "cmos_inverter");
    h.state_mut().command(Command::RunSignoff(Some("metal1".into())));
    h.step();
    settle(&mut h, "layout_signoff");
    let r = ics_core::CellRef::new("work", "cmos_inverter");
    let ed = h.state().layout_editor(&r).expect("editor");
    match ed.verify.signoff.as_ref().expect("sign-off result installed") {
        Ok(rep) => assert!(ed.verify.drc.as_ref().is_some_and(|d| !d.violations.is_empty()), "{}", rep.summary()),
        Err(e) => assert!(e.contains("KLayout"), "{e}"),
    }
    let img = h.render().expect("render");
    if let Ok(shots) = std::env::var("ICS_SHOT_DIR") {
        std::fs::create_dir_all(&shots).unwrap();
        img.save(std::path::Path::new(&shots).join("15_layout_signoff.png")).unwrap();
    }
    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn import_oasis() {
    let src = ics_core::examples::demo_project();
    let (top, _) = src.all_cells().find(|(_, c)| c.name == "pixel_array").unwrap();
    let dir = std::env::temp_dir().join(format!("ics-ui-oasis-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("pixels.oas");
    std::fs::write(&file, ics_core::oasis::write_oasis_hier(&src, std::slice::from_ref(&top), &src.tech)).unwrap();

    let args = LaunchArgs { project: None, demo: true, workspace: Some("layout".into()), cell: None, run: false };
    let mut h = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_eframe(|cc| IcStudio::new(cc, args));
    h.run_steps(4);
    h.state_mut().command(Command::ImportGds(Some(file)));
    h.run_steps(6);
    settle(&mut h, "17_import_oasis");

    let console: Vec<String> = h.state().console_lines().map(str::to_string).collect();
    assert!(console.iter().any(|l| l.starts_with("Imported 4 cell(s)") && l.contains("library 'pixels'")), "{console:?}");
    assert!(!console.iter().any(|l| l.contains("import failed")), "{console:?}");
    let p = h.state().project().unwrap();
    let lib = p.libraries.iter().find(|l| l.name == "pixels").expect("imported library");
    for c in &lib.cells {
        let (_, orig) = src.all_cells().find(|(_, o)| o.name == c.name).unwrap();
        let (a, b) = (c.layout.as_ref().unwrap(), orig.layout.as_ref().unwrap());
        assert_eq!((a.shapes.len(), a.instances.len()), (b.shapes.len(), b.instances.len()), "{}", c.name);
    }
    assert_eq!(h.state().workspace(), Workspace::Layout);
    let img = h.render().expect("render");
    if let Ok(dir) = std::env::var("ICS_SHOT_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        img.save(std::path::Path::new(&dir).join("17_import_oasis.png")).unwrap();
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
