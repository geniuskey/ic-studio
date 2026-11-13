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
    let mut h = app(args);
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
    shot(&mut h, name);
}

/// The app at screenshot size.
fn app(args: LaunchArgs) -> Harness<'static, IcStudio> {
    let mut h = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_eframe(|cc| IcStudio::new(cc, args));
    h.run_steps(4);
    h
}

/// Close the cell tab that opening a project adds, and forget the explorer state it left.
fn close_first_tab(h: &mut Harness<'_, IcStudio>) {
    if let Some(first) = h.state().project().and_then(|p| p.all_cells().next().map(|(r, _)| r)) {
        h.state_mut().command(Command::CloseTab(first));
        h.step();
        h.ctx.data_mut(|d| d.clear());
    }
}

fn shot(h: &mut Harness<'_, IcStudio>, name: &str) {
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
fn light_theme_layout() {
    scenario_then("light_layout", true, Some("cmos_inverter"), "layout", false, Some(Command::ToggleTheme));
}

#[test]
fn light_theme_simulation() {
    scenario_then("light_simulation", true, Some("cmos_inverter"), "simulation", true, Some(Command::ToggleTheme));
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
    let mut h = app(args);
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

/// The demo project with the layout of `cell` removed, saved to a temporary directory.
fn demo_without_layout(test: &str, cell: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ics-ui-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut p = ics_core::examples::demo_project();
    p.cell_mut(&ics_core::CellRef::new("work", cell)).unwrap().layout = None;
    p.save_as(&dir).unwrap();
    dir
}

fn flylines(h: &Harness<'_, IcStudio>, cell: &str) -> usize {
    let p = h.state().project().unwrap();
    let c = p.cell(&ics_core::CellRef::new("work", cell)).unwrap();
    ics_core::sdl::ratsnest(c.schematic.as_ref().unwrap(), c.layout.as_ref().unwrap(), p, &p.tech).len()
}

/// Placement of the 28-transistor mirror adder from its schematic, before routing.
fn full_adder_placed(test: &str) -> Harness<'static, IcStudio> {
    let dir = demo_without_layout(test, "full_adder");
    let args = LaunchArgs {
        project: Some(dir),
        demo: false,
        workspace: Some("schematic".into()),
        cell: Some("full_adder".into()),
        run: false,
    };
    let mut h = app(args);
    h.state_mut().command(Command::GenerateLayout);
    h.run_steps(4);
    assert!(flylines(&h, "full_adder") > 0);
    h
}

#[test]
fn demo_pnr_placed() {
    let mut h = full_adder_placed("pnr-placed");
    h.run_steps(4);
    settle(&mut h, "demo_pnr_placed");
    shot(&mut h, "demo_pnr_placed");
}

#[test]
fn demo_pnr_routed() {
    let mut h = full_adder_placed("pnr-routed");
    h.state_mut().command(Command::RouteFlylines);
    h.run_steps(4);
    assert_eq!(flylines(&h, "full_adder"), 0);
    h.state_mut().command(Command::RunLvs);
    h.run_steps(2);
    settle(&mut h, "demo_pnr_routed");
    assert!(
        h.state().console_lines().any(|l| l.ends_with("LVS: layout matches schematic")),
        "{:?}",
        h.state().console_lines().collect::<Vec<_>>()
    );
    h.state_mut().command(Command::FitView);
    h.run_steps(4);
    shot(&mut h, "demo_pnr_routed");
}

#[test]
fn demo_ota2s() {
    scenario("demo_ota2s", true, Some("ota2s"), "layout", false);
}

/// Transient of the mirror adder, plotting its sum and carry outputs.
#[test]
fn demo_full_adder_sim() {
    use egui_kittest::kittest::Queryable;
    let args = LaunchArgs {
        project: None,
        demo: true,
        workspace: Some("simulation".into()),
        cell: Some("full_adder".into()),
        run: true,
    };
    let mut h = app(args);
    settle(&mut h, "demo_full_adder_sim");
    for sig in ["v(a)", "v(n3)", "v(n9)", "v(s)", "v(co)"] {
        // The plot legend has a checkbox per plotted signal too; use the signal list's.
        let boxes = h.get_all_by_role_and_label(egui::accesskit::Role::CheckBox, sig);
        boxes.max_by(|a, b| a.rect().left().total_cmp(&b.rect().left())).unwrap().click();
        h.run_steps(2);
    }
    h.event(egui::Event::PointerGone);
    h.run_steps(2);
    shot(&mut h, "demo_full_adder_sim");
}

#[test]
fn demo_sram_array() {
    scenario("demo_sram_array", true, Some("sram_array"), "layout", false);
}

#[test]
fn demo_pixel_array() {
    scenario("demo_pixel_array", true, Some("pixel_array"), "layout", false);
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
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::P);
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
fn rule_designer() {
    let args = LaunchArgs {
        project: None,
        demo: true,
        workspace: Some("layout".into()),
        cell: Some("cmos_inverter".into()),
        run: false,
    };
    let mut h = app(args);
    h.state_mut().command(Command::RuleDesigner);
    h.run_steps(6);
    let tech = h.state().project().unwrap().tech.clone();
    assert!(h.state().rule_designer().open);
    let text = h.state().rule_designer().deck_text(&tech);
    let deck = ics_core::drc::parse_deck(&text, &tech).expect("designer deck parses");
    assert!(!deck.rules.is_empty());
    shot(&mut h, "30_rule_designer");
}

#[test]
fn ciw_console() {
    let args = LaunchArgs {
        project: None,
        demo: true,
        workspace: Some("layout".into()),
        cell: Some("cmos_inverter".into()),
        run: false,
    };
    let mut h = app(args);
    let count = |h: &Harness<'_, IcStudio>| {
        let p = h.state().project().unwrap();
        p.all_cells().find(|(r, _)| r.cell == "cmos_inverter").and_then(|(_, c)| c.layout.as_ref()).map_or(0, |l| l.shapes.len())
    };
    let before = count(&h);
    h.state_mut().command(Command::Ciw);
    h.state_mut().command(Command::CiwEval("procedure(pad(cv x) dbCreateRect(cv \"metal1\" list(x:10 x+1:11)))".into()));
    h.state_mut().command(Command::CiwEval("cv = geGetEditCellView() pad(cv 0) pad(cv 2) length(cv~>shapes)".into()));
    h.state_mut().command(Command::CiwEval("printf(\"hello %d\n\" 42) undefinedFn()".into()));
    h.run_steps(6);
    assert_eq!(count(&h), before + 2);
    let lines = &h.state().ciw().lines;
    assert!(lines.iter().any(|(_, t)| t == &(before + 2).to_string()), "{lines:?}");
    assert!(lines.iter().any(|(_, t)| t == "hello 42"));
    assert!(lines.last().is_some_and(|(_, t)| t.contains("undefined function undefinedFn")));
    shot(&mut h, "31_ciw");
}

fn demo_args(cell: &str, workspace: &str, run: bool) -> LaunchArgs {
    LaunchArgs { project: None, demo: true, workspace: Some(workspace.into()), cell: Some(cell.into()), run }
}

/// Waveform Calculator: a scalar measurement and a plotted derived waveform, kept across reruns.
#[test]
fn waveform_calculator() {
    let mut h = app(demo_args("rc_lowpass", "simulation", true));
    settle(&mut h, "calculator");
    let r = ics_core::CellRef::new("work", "rc_lowpass");
    assert_eq!(h.state().sim_state(&r).unwrap().tab, "ac");
    h.state_mut().command(Command::Calculator);
    h.state_mut().command(Command::CalcPlot("bandwidth(v(out)/v(in))".into()));
    h.state_mut().command(Command::CalcPlot("db(v(out)) + 3".into()));
    h.state_mut().command(Command::CalcPlot("nosuch(v(out))".into()));
    h.run_steps(4);
    let hist = &h.state().calculator().history;
    assert_eq!(hist.len(), 3);
    let bw = ics_core::units::parse_spice(hist[0].2.trim_end_matches('k')).unwrap();
    assert!((bw - 159.15).abs() < 2.0, "{hist:?}");
    assert!(hist[1].3 && !hist[2].3, "{hist:?}");
    h.state_mut().command(Command::RunSimulation);
    h.run_steps(2);
    settle(&mut h, "calculator rerun");
    let res = h.state().sim_state(&r).unwrap().results.as_ref().unwrap();
    assert!(res.dataset("ac").unwrap().get("db(v(out)) + 3").is_some(), "derived trace re-evaluated after a rerun");
    shot(&mut h, "32_calculator");
}

/// Corner / Sweep Manager: sweep R1 and check the bandwidth spec on every point.
#[test]
fn sweep_manager() {
    let mut h = app(demo_args("rc_lowpass", "simulation", false));
    h.state_mut().command(Command::SweepManager);
    h.run_steps(2);
    {
        let sw = h.state_mut().sweep_manager();
        assert!(sw.open);
        sw.setup.temps = "-40 125".into();
        sw.setup.vars.push(ics_core::sweep::SweepVar { name: "R1.r".into(), values: "1k 2k 4k".into() });
        sw.setup.specs.push(ics_core::sweep::Spec {
            name: "bw".into(),
            analysis: "ac".into(),
            expr: "bandwidth(v(out))".into(),
            min: "50k".into(),
            max: String::new(),
        });
    }
    h.state_mut().command(Command::RunSweep);
    h.run_steps(2);
    settle(&mut h, "sweep");
    let sw = h.state_mut().sweep_manager();
    assert_eq!(sw.results.len(), 6);
    let bws: Vec<f64> = sw.results.iter().map(|r| *r.as_ref().unwrap().specs[0].as_ref().unwrap()).collect();
    for (k, want) in [159.15e3, 79.58e3, 39.79e3].iter().enumerate() {
        assert!((bws[k] - want).abs() / want < 0.02, "{bws:?}");
    }
    assert_eq!(sw.pass_count(), (4, 6));
    assert!(h.state().console_lines().any(|l| l.contains("4/6 pass every spec")));
    h.state_mut().sweep_manager().setup.specs[0].name = "bandwidth".into();
    h.run_steps(4);
    shot(&mut h, "33_sweep_manager");
}

/// Stream Layer Map: a map applied to the technology renumbers metal1 on export.
#[test]
fn stream_layer_map() {
    use egui_kittest::kittest::Queryable;
    let mut h = app(demo_args("cmos_inverter", "layout", false));
    h.state_mut().command(Command::StreamMap);
    h.run_steps(2);
    let tech = h.state().project().unwrap().tech.clone();
    assert_eq!(
        h.state_mut().stream_map_editor().load_text(
            "metal1 drawing 31 0
via1 drawing 51 0
",
            &tech
        ),
        2
    );
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(3);
    let tech = &h.state().project().unwrap().tech;
    assert_eq!(tech.stream_map.len(), 2);
    let m1 = tech.layer_index("metal1").unwrap();
    assert_eq!(ics_core::stream_map::export_pairs(tech)[m1], (31, 0));
    shot(&mut h, "34_stream_map");
}

/// Dummy Fill: adds squares to the active layout, undoable.
#[test]
fn dummy_fill() {
    let mut h = app(demo_args("inv_chain", "layout", false));
    let before = shape_count(&h, "inv_chain");
    h.state_mut().command(Command::DummyFill);
    h.run_steps(2);
    let bb = {
        let p = h.state().project().unwrap();
        let (_, c) = p.all_cells().find(|(_, c)| c.name == "inv_chain").unwrap();
        let shapes = &c.layout.as_ref().unwrap().shapes;
        let um = |v: i64| v as f64 / 1000.0;
        let x0 = shapes.iter().map(|s| s.bbox().x0).min().unwrap();
        let y0 = shapes.iter().map(|s| s.bbox().y0).min().unwrap();
        let x1 = shapes.iter().map(|s| s.bbox().x1).max().unwrap();
        [um(x0) - 20.0, um(y0) - 20.0, um(x1) + 20.0, um(y0)]
    };
    h.state_mut().fill_tool().region = Some(bb);
    h.state_mut().command(Command::AddFill);
    h.run_steps(3);
    let after = shape_count(&h, "inv_chain");
    assert!(after > before, "{:?}", h.state().console_lines().collect::<Vec<_>>());
    assert!(h.state().console_lines().any(|l| l.starts_with("Dummy fill on")));
    shot(&mut h, "35_dummy_fill");
    h.state_mut().command(Command::Undo);
    h.run_steps(2);
    assert_eq!(shape_count(&h, "inv_chain"), before);
}

/// Monte Carlo on the RC filter: spread of the bandwidth, yield and the HTML datasheet.
#[test]
fn monte_carlo_sweep() {
    let mut h = app(demo_args("rc_lowpass", "simulation", false));
    h.state_mut().command(Command::SweepManager);
    h.run_steps(2);
    {
        let sw = h.state_mut().sweep_manager();
        sw.setup.monte_carlo.runs = 40;
        sw.setup.monte_carlo.seed = 3;
        sw.setup.monte_carlo.vars.push(ics_core::sweep::McVar {
            target: "R1.r".into(),
            sigma: "10%".into(),
            ..Default::default()
        });
        sw.setup.specs.push(ics_core::sweep::Spec {
            name: "bw".into(),
            analysis: "ac".into(),
            expr: "bandwidth(v(out))".into(),
            min: "150k".into(),
            max: String::new(),
        });
    }
    h.state_mut().command(Command::RunSweep);
    h.run_steps(2);
    settle(&mut h, "monte carlo");
    let sw = h.state_mut().sweep_manager();
    assert_eq!(sw.results.len(), 40);
    assert!(sw.is_monte_carlo());
    let st = ics_core::sweep::stats(&sw.spec_values(0));
    assert!((st.mean - 159e3).abs() < 15e3 && st.std > 5e3, "{st:?}");
    let (pass, n) = sw.spec_yield(0);
    assert!(n == 40 && pass > 5 && pass < 40, "{pass}/{n}");
    let html = sw.datasheet();
    assert!(html.contains("<h2>Specs</h2>") && html.contains("mc#40"), "{html}");
    h.run_steps(4);
    shot(&mut h, "36_monte_carlo");
}

/// Results Browser: the previous run is kept and can be overlaid.
#[test]
fn results_compare() {
    let mut h = app(demo_args("rc_lowpass", "simulation", true));
    settle(&mut h, "first run");
    h.state_mut().command(Command::RunSimulation);
    h.run_steps(2);
    settle(&mut h, "second run");
    let r = ics_core::CellRef::new("work", "rc_lowpass");
    let s = h.state().sim_state(&r).unwrap();
    assert_eq!(s.label, "Run 2");
    assert_eq!(s.history.len(), 1);
    assert_eq!(s.history[0].label, "Run 1");
    h.state_mut().command(Command::ResultsBrowser);
    h.run_steps(4);
    shot(&mut h, "37_results_browser");
}

#[test]
fn check_schematic() {
    let mut h = app(demo_args("cmos_inverter", "schematic", false));
    h.state_mut().command(Command::CheckSchematic(false));
    h.run_steps(3);
    assert!(h.state().last_check().is_empty(), "{:?}", h.state().last_check());
    assert!(h.state().console_lines().any(|l| l == "Check work/cmos_inverter: 0 error(s), 0 warning(s)"));
}

#[test]
fn net_browser() {
    let mut h = app(demo_args("inv_chain", "layout", false));
    h.state_mut().command(Command::NetBrowser);
    h.run_steps(2);
    settle(&mut h, "net browser");
    h.run_steps(2);
    let rows = h.state().net_browser().rows();
    assert!(rows.len() > 3 && rows.iter().any(|r| r.labelled && r.terminals > 0), "{}", rows.len());
    shot(&mut h, "38_net_browser");
}

#[test]
fn hierarchy_browser() {
    let mut h = app(demo_args("inv_chain", "layout", false));
    h.state_mut().command(Command::HierarchyBrowser);
    h.run_steps(3);
    let p = h.state().project().unwrap();
    let u = ics_studio::hier_view::usage(p);
    let chain = ics_core::CellRef::new("work", "inv_chain");
    assert!(u.tops.contains(&chain));
    assert!(!ics_studio::hier_view::flat_counts(&u, &chain).is_empty());
    shot(&mut h, "39_hierarchy_browser");
}

/// Guard ring, via array and matched placement, each undoable.
#[test]
fn layout_generators() {
    use ics_studio::gen_view::GenAction;
    let mut h = app(demo_args("inv_chain", "layout", false));
    h.state_mut().command(Command::GuardRing);
    h.run_steps(2);
    let before = shape_count(&h, "inv_chain");
    let area = {
        let p = h.state().project().unwrap();
        let (_, c) = p.all_cells().find(|(_, c)| c.name == "inv_chain").unwrap();
        let flat = ics_core::hier::flatten(c.layout.as_ref().unwrap(), p, 8);
        flat.iter()
            .filter(|s| !matches!(s.geom, ics_core::Geometry::Label { .. }))
            .map(|s| s.geom.bbox())
            .reduce(|a, b| ics_core::IRect::new(a.x0.min(b.x0), a.y0.min(b.y0), a.x1.max(b.x1), a.y1.max(b.y1)))
            .unwrap()
    };
    h.state_mut().layout_generators().set_area(area, &ics_core::Tech::generic());
    let ring = ics_core::layout_gen::GuardRing { area, well: false, width: 0, gap: 0, contacts: true, metal: true };
    h.state_mut().apply_gen(GenAction::GuardRing(ring));
    h.run_steps(2);
    let ringed = shape_count(&h, "inv_chain");
    assert!(ringed > before + 8, "{ringed}");
    h.state_mut().command(Command::FitView);
    h.run_steps(3);
    shot(&mut h, "40_layout_generators");
    let via =
        ics_core::layout_gen::ViaArray { area: ics_core::IRect::new(-60_000, -60_000, -58_000, -58_000), via: "via1".into() };
    h.state_mut().apply_gen(GenAction::ViaArray(via));
    h.run_steps(2);
    assert!(shape_count(&h, "inv_chain") > ringed);
    h.state_mut().command(Command::Undo);
    h.state_mut().command(Command::Undo);
    h.run_steps(2);
    assert_eq!(shape_count(&h, "inv_chain"), before);

    let mut h = app(demo_args("ota5t", "layout", false));
    let spec = ics_core::matching::MatchSpec {
        devices: vec!["M1".into(), "M2".into()],
        pattern: ics_core::matching::Pattern::Interdigitated,
        ..Default::default()
    };
    h.state_mut().apply_gen(GenAction::Match(spec));
    h.run_steps(2);
    assert!(
        h.state().console_lines().any(|l| l.starts_with("Layout generator on work/ota5t: placed")),
        "{:?}",
        h.state().console_lines().collect::<Vec<_>>()
    );
    h.state_mut().command(Command::MatchedPlacement);
    h.run_steps(4);
    shot(&mut h, "41_matched_placement");
}

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
        include_str!("../src/rule_designer.rs"),
        include_str!("../src/ciw.rs"),
        include_str!("../src/calc_view.rs"),
        include_str!("../src/sweep_view.rs"),
        include_str!("../src/stream_map_view.rs"),
        include_str!("../src/fill_view.rs"),
        include_str!("../src/net_view.rs"),
        include_str!("../src/hier_view.rs"),
        include_str!("../src/gen_view.rs"),
        include_str!("../../ics-core/src/sweep.rs"),
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
    close_first_tab(&mut h);
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

fn shapes_of(h: &Harness<'_, IcStudio>, cell: &str) -> Vec<ics_core::Shape> {
    let p = h.state().project().unwrap();
    p.cell(&ics_core::CellRef::new("work", cell)).unwrap().layout.as_ref().unwrap().shapes.clone()
}

/// Virtuoso-style commands on an empty cell: two-click rectangle, move by reference point,
/// yank / paste, chop, wire with a layer change, persistent ruler, zoom box and previous view.
#[test]
fn layout_virtuoso_commands() {
    let dir = std::env::temp_dir().join(format!("ics-ui-virtuoso-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut p = ics_core::examples::demo_project();
    let mut cell = ics_core::Cell::new("scratch");
    cell.layout = Some(ics_core::Layout::default());
    let r = p.add_cell("work", cell).unwrap();
    p.save_as(&dir).unwrap();
    let mut h = open_layout(&dir, "scratch");
    let key = |h: &mut Harness<'_, IcStudio>, m: egui::Modifiers, k: egui::Key| {
        h.key_press_modifiers(m, k);
        h.run_steps(2);
    };
    let none = egui::Modifiers::NONE;
    h.hover_at(egui::pos2(700.0, 450.0));
    h.run_steps(2);

    key(&mut h, none, egui::Key::R);
    click(&mut h, egui::pos2(600.0, 400.0));
    click(&mut h, egui::pos2(700.0, 480.0));
    let s = shapes_of(&h, "scratch");
    assert_eq!(s.len(), 1, "two clicks draw a rectangle");
    let b0 = s[0].bbox();

    key(&mut h, none, egui::Key::M);
    click(&mut h, egui::pos2(650.0, 440.0));
    click(&mut h, egui::pos2(750.0, 440.0));
    let b1 = shapes_of(&h, "scratch")[0].bbox();
    assert_eq!((b1.width(), b1.height()), (b0.width(), b0.height()));
    assert!((b1.x0 - b0.x0 - 1250).abs() <= 10 && b1.y0 == b0.y0, "moved by the reference vector: {b0:?} -> {b1:?}");

    // Stretch the right edge 40 px (500 nm) to the right, undo with u, redo with Shift+U.
    key(&mut h, none, egui::Key::S);
    click(&mut h, egui::pos2(800.0, 440.0));
    click(&mut h, egui::pos2(840.0, 440.0));
    let b2 = shapes_of(&h, "scratch")[0].bbox();
    assert!((b2.width() - b1.width() - 500).abs() <= 10 && b2.x0 == b1.x0, "stretched: {b1:?} -> {b2:?}");
    key(&mut h, none, egui::Key::U);
    assert_eq!(shapes_of(&h, "scratch")[0].bbox(), b1, "u undoes");
    key(&mut h, egui::Modifiers::SHIFT, egui::Key::U);
    assert_eq!(shapes_of(&h, "scratch")[0].bbox(), b2, "Shift+U redoes");

    key(&mut h, none, egui::Key::Escape);
    click(&mut h, egui::pos2(750.0, 440.0));
    key(&mut h, egui::Modifiers::COMMAND, egui::Key::C);
    key(&mut h, egui::Modifiers::COMMAND, egui::Key::V);
    assert!(h.state().layout_editor(&r).unwrap().paste.is_some(), "paste follows the cursor");
    click(&mut h, egui::pos2(600.0, 600.0));
    assert_eq!(shapes_of(&h, "scratch").len(), 2, "paste drops a copy");

    // Chop a vertical stripe out of the pasted (selected) rectangle: it splits in two.
    key(&mut h, egui::Modifiers::SHIFT, egui::Key::C);
    click(&mut h, egui::pos2(640.0, 500.0));
    click(&mut h, egui::pos2(660.0, 620.0));
    assert_eq!(shapes_of(&h, "scratch").len(), 3, "chop splits the shape");

    // Path on metal1, '=' drops a via1 and continues on metal2.
    key(&mut h, none, egui::Key::Escape);
    key(&mut h, none, egui::Key::P);
    click(&mut h, egui::pos2(600.0, 300.0));
    key(&mut h, none, egui::Key::Equals);
    click(&mut h, egui::pos2(760.0, 300.0));
    key(&mut h, none, egui::Key::Enter);
    let tech = &h.state().project().unwrap().tech;
    let (m2, v1) = (tech.layer_index("metal2").unwrap(), tech.layer_index("via1").unwrap());
    let s = shapes_of(&h, "scratch");
    assert!(s.iter().any(|s| s.layer == v1), "via1 cut placed");
    assert!(matches!(s.last().map(|s| (&s.geom, s.layer)), Some((ics_core::Geometry::Path { .. }, l)) if l == m2));

    key(&mut h, none, egui::Key::K);
    click(&mut h, egui::pos2(560.0, 220.0));
    click(&mut h, egui::pos2(1000.0, 220.0));
    assert_eq!(h.state().layout_editor(&r).unwrap().rulers.len(), 1, "ruler stays on screen");

    // Layer tap picks the layer of the shape under the cursor.
    let m1 = h.state().project().unwrap().tech.layer_index("metal1").unwrap();
    h.hover_at(egui::pos2(750.0, 440.0));
    h.run_steps(2);
    key(&mut h, none, egui::Key::T);
    assert_eq!(h.state().layout_editor(&r).unwrap().active_layer, m1, "T taps metal1");
    h.hover_at(egui::pos2(700.0, 300.0));
    h.run_steps(2);
    key(&mut h, none, egui::Key::T);
    assert_eq!(h.state().layout_editor(&r).unwrap().active_layer, m2, "T taps metal2");

    let before = h.state().layout_editor(&r).unwrap().view.scale;
    key(&mut h, none, egui::Key::Z);
    click(&mut h, egui::pos2(580.0, 380.0));
    click(&mut h, egui::pos2(780.0, 500.0));
    let zoomed = h.state().layout_editor(&r).unwrap().view.scale;
    assert!(zoomed > before * 2.0, "zoom box zooms in: {before} -> {zoomed}");
    key(&mut h, none, egui::Key::W);
    assert_eq!(h.state().layout_editor(&r).unwrap().view.scale, before, "W returns to the previous view");

    key(&mut h, none, egui::Key::R);
    // Closer than the metal2 spacing to the routed path: DRD marks the gap.
    click(&mut h, egui::pos2(620.0, 330.0));
    h.hover_at(egui::pos2(720.0, 370.0));
    h.run_steps(3);
    shot(&mut h, "15_layout_virtuoso");
}

/// Shift+X edits the instance under the cursor in place, Shift+B returns.
#[test]
fn layout_descend_and_return() {
    let args =
        LaunchArgs { project: None, demo: true, workspace: Some("layout".into()), cell: Some("inv_chain".into()), run: false };
    let mut h = app(args);
    let top = h.state().active_ref().cloned().unwrap();
    h.hover_at(egui::pos2(865.0, 440.0));
    h.run_steps(2);
    let before = h.state().layout_editor(&top).unwrap().view;
    h.key_press(egui::Key::X);
    h.run_steps(4);
    let child = h.state().active_ref().cloned().unwrap();
    assert_ne!(child, top, "descended into a placed cell");
    let v = h.state().layout_editor(&child).unwrap().view;
    assert_eq!((v.center, v.scale), (before.center, before.scale), "edit in place keeps the picture");
    assert!(!v.orient.is_identity(), "the child is shown in the parent's frame");
    shot(&mut h, "16_layout_edit_in_place");
    h.hover_at(egui::pos2(865.0, 440.0));
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::B);
    h.run_steps(4);
    assert_eq!(h.state().active_ref(), Some(&top), "returned to the top cell");
    let after = h.state().layout_editor(&top).unwrap().view;
    assert_eq!((after.center, after.scale, after.orient), (before.center, before.scale, before.orient));

    // Shift+X descends: the cell opens on its own, Shift+B returns.
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::X);
    h.run_steps(4);
    let child = h.state().active_ref().cloned().unwrap();
    assert_ne!(child, top, "descended");
    assert!(h.state().layout_editor(&child).unwrap().view.orient.is_identity(), "not in place");
    h.hover_at(egui::pos2(865.0, 440.0));
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::B);
    h.run_steps(4);
    assert_eq!(h.state().active_ref(), Some(&top), "returned again");
}

fn drag(h: &mut Harness<'_, IcStudio>, button: egui::PointerButton, a: egui::Pos2, b: egui::Pos2) {
    h.hover_at(a);
    h.step();
    h.event(egui::Event::PointerButton { pos: a, button, pressed: true, modifiers: Default::default() });
    h.step();
    for i in 1..=6 {
        h.hover_at(a + (b - a) * (i as f32 / 6.0));
        h.step();
    }
    h.event(egui::Event::PointerButton { pos: b, button, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
}

/// Virtuoso habits: stretch by box, partial select (F4), right-drag zoom, the option forms
/// (q, e, Shift+E, F3) and the layout menus.
#[test]
fn layout_virtuoso_stretch_and_forms() {
    use ics_studio::layout_view::LayAction;
    let dir = std::env::temp_dir().join(format!("ics-ui-virtuoso-forms-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut p = ics_core::examples::demo_project();
    let mut cell = ics_core::Cell::new("scratch");
    cell.layout = Some(ics_core::Layout::default());
    let r = p.add_cell("work", cell).unwrap();
    p.save_as(&dir).unwrap();
    let mut h = open_layout(&dir, "scratch");
    let key = |h: &mut Harness<'_, IcStudio>, m: egui::Modifiers, k: egui::Key| {
        h.key_press_modifiers(m, k);
        h.run_steps(2);
    };
    let none = egui::Modifiers::NONE;
    let pos = egui::pos2;
    h.hover_at(pos(700.0, 450.0));
    h.run_steps(2);
    key(&mut h, none, egui::Key::G);
    key(&mut h, none, egui::Key::R);
    click(&mut h, pos(600.0, 400.0));
    click(&mut h, pos(700.0, 480.0));
    click(&mut h, pos(600.0, 520.0));
    click(&mut h, pos(700.0, 600.0));
    let b0: Vec<_> = shapes_of(&h, "scratch").iter().map(|s| s.bbox()).collect();
    assert_eq!(b0.len(), 2);

    // S, box around the right edges of both rectangles, reference point, destination.
    key(&mut h, none, egui::Key::S);
    drag(&mut h, egui::PointerButton::Primary, pos(680.0, 370.0), pos(730.0, 630.0));
    assert_eq!(h.state().layout_editor(&r).unwrap().part.len(), 2, "the box picks both right edges");
    click(&mut h, pos(700.0, 500.0));
    click(&mut h, pos(740.0, 500.0));
    let b1: Vec<_> = shapes_of(&h, "scratch").iter().map(|s| s.bbox()).collect();
    for (a, b) in b0.iter().zip(&b1) {
        assert!((b.width() - a.width() - 500).abs() <= 10 && b.x0 == a.x0 && b.y0 == a.y0, "stretched: {a:?} -> {b:?}");
    }

    // F4: click the top edge of the upper rectangle, stretch it up.
    key(&mut h, none, egui::Key::Escape);
    key(&mut h, none, egui::Key::F4);
    assert!(h.state().layout_editor(&r).unwrap().partial_mode);
    click(&mut h, pos(650.0, 400.0));
    assert_eq!(h.state().layout_editor(&r).unwrap().part.verts.values().map(|v| v.len()).sum::<usize>(), 2);
    key(&mut h, none, egui::Key::S);
    click(&mut h, pos(650.0, 400.0));
    click(&mut h, pos(650.0, 360.0));
    let top = b1.iter().max_by_key(|b| b.y1).unwrap();
    let b2 = shapes_of(&h, "scratch").iter().map(|s| s.bbox()).max_by_key(|b| b.y1).unwrap();
    assert!((b2.height() - top.height() - 500).abs() <= 10 && b2.y0 == top.y0, "top edge stretched: {top:?} -> {b2:?}");
    key(&mut h, none, egui::Key::F4);
    key(&mut h, none, egui::Key::Escape);

    // Right drag zooms into the box, w goes back.
    let before = h.state().layout_editor(&r).unwrap().view.scale;
    drag(&mut h, egui::PointerButton::Secondary, pos(580.0, 340.0), pos(780.0, 620.0));
    let zoomed = h.state().layout_editor(&r).unwrap().view.scale;
    assert!(zoomed > before * 1.5, "right drag zooms in: {before} -> {zoomed}");
    key(&mut h, none, egui::Key::W);
    assert_eq!(h.state().layout_editor(&r).unwrap().view.scale, before);
    h.state_mut().command(Command::Lay(LayAction::ZoomIn));
    h.run_steps(2);
    assert_eq!(h.state().layout_editor(&r).unwrap().view.scale, before * 2.0, "menu zoom in by 2");

    // Option forms.
    key(&mut h, none, egui::Key::F);
    key(&mut h, egui::Modifiers::COMMAND, egui::Key::A);
    key(&mut h, none, egui::Key::Q);
    // The canvas keeps the bindkeys while the pointer is over a form opened on top of it.
    h.hover_at(pos(1100.0, 300.0));
    h.run_steps(2);
    key(&mut h, none, egui::Key::E);
    key(&mut h, egui::Modifiers::SHIFT, egui::Key::E);
    key(&mut h, none, egui::Key::P);
    key(&mut h, none, egui::Key::F3);
    let ed = h.state().layout_editor(&r).unwrap();
    assert!(ed.show_props && ed.show_display && ed.show_editor && ed.show_opts, "q, e, Shift+E and F3 open their forms");
    h.run_steps(3);
    shot(&mut h, "17_layout_virtuoso_forms");

    // Middle click: the pop-up menu; F4 then a box: the vertices it holds.
    for k in [egui::Key::Q, egui::Key::E, egui::Key::F3] {
        key(&mut h, none, k);
    }
    key(&mut h, egui::Modifiers::SHIFT, egui::Key::E);
    key(&mut h, none, egui::Key::Escape);
    key(&mut h, none, egui::Key::F4);
    drag(&mut h, egui::PointerButton::Primary, pos(900.0, 180.0), pos(1100.0, 700.0));
    assert!(!h.state().layout_editor(&r).unwrap().part.is_empty(), "a box picks vertices in partial select");
    h.hover_at(pos(800.0, 300.0));
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos: pos(800.0, 300.0),
            button: egui::PointerButton::Middle,
            pressed,
            modifiers: Default::default(),
        });
        h.step();
    }
    h.run_steps(3);
    shot(&mut h, "18_layout_virtuoso_popup");
}

fn wheel(h: &mut Harness<'_, IcStudio>, at: egui::Pos2, dy: f32, modifiers: egui::Modifiers) {
    h.hover_at(at);
    h.step();
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, dy),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
        modifiers,
    });
    h.run_steps(3);
}

/// Interaction profiles: Virtuoso (default) pans with the wheel and zooms with Ctrl+wheel and a
/// right drag; Conventional zooms with the wheel and pans with a right drag.
#[test]
fn layout_interaction_profiles() {
    use ics_studio::canvas::Profile;
    let args =
        LaunchArgs { project: None, demo: true, workspace: Some("layout".into()), cell: Some("inv_chain".into()), run: false };
    let mut h = app(args);
    let r = h.state().active_ref().cloned().unwrap();
    let view = |h: &Harness<'_, IcStudio>| h.state().layout_editor(&r).unwrap().view;
    let at = egui::pos2(865.0, 440.0);

    let v0 = view(&h);
    wheel(&mut h, at, -60.0, egui::Modifiers::NONE);
    let v1 = view(&h);
    assert_eq!(v1.scale, v0.scale, "Virtuoso: the wheel does not zoom");
    assert!(v1.center.1 != v0.center.1, "Virtuoso: the wheel pans");
    wheel(&mut h, at, 60.0, egui::Modifiers::COMMAND);
    assert!(view(&h).scale > v1.scale, "Virtuoso: Ctrl+wheel zooms");

    h.state_mut().set_interaction(Profile::Conventional);
    h.run_steps(2);
    let v2 = view(&h);
    wheel(&mut h, at, 60.0, egui::Modifiers::NONE);
    assert!(view(&h).scale != v2.scale, "Conventional: the wheel zooms");
    let v3 = view(&h);
    drag(&mut h, egui::PointerButton::Secondary, egui::pos2(700.0, 400.0), egui::pos2(800.0, 450.0));
    let v4 = view(&h);
    assert_eq!(v4.scale, v3.scale, "Conventional: right drag pans");
    assert!(v4.center != v3.center);
}

/// Live DRC flags a shape as soon as it is drawn; the quick-fix form offers verified fixes and
/// Fix All repairs it as one undoable edit.
#[test]
fn live_drc_quick_fix() {
    use ics_studio::layout_view::LayAction;
    let mut h = app(demo_args("cmos_inverter", "layout", false));
    let r = h.state().active_ref().cloned().unwrap();
    let wait = |h: &mut Harness<'_, IcStudio>| {
        let t = std::time::Instant::now();
        h.run_steps(2);
        loop {
            h.step();
            let ed = h.state().layout_editor(&r).unwrap();
            if !ed.live.running() || t.elapsed() > Duration::from_secs(30) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        h.run_steps(2);
        h.state().layout_editor(&r).unwrap().live.violations.clone()
    };
    let base = wait(&mut h);
    h.state_mut().command(Command::CiwEval("dbCreateRect(geGetEditCellView() \"metal1\" list(30:0 30.1:2))".into()));
    let after = wait(&mut h);
    assert_eq!(after.len(), base.len() + 1, "{after:?}");
    let new = after.iter().find(|v| !base.contains(v)).unwrap().clone();
    assert!(new.rule.to_lowercase().contains("metal1"), "{new:?}");
    h.state_mut().command(Command::Lay(LayAction::QuickFix));
    h.run_steps(3);
    let quick = h.state().layout_editor(&r).unwrap().live.quick.clone();
    let (_, fixes) = quick.expect("quick-fix form open");
    assert!(!fixes.is_empty(), "the narrow bar has a fix");
    shot(&mut h, "40_live_drc_quick_fix");
    h.state_mut().command(Command::Lay(LayAction::FixAll));
    h.run_steps(2);
    let fixed = wait(&mut h);
    assert!(!fixed.contains(&new), "{fixed:?}");
    assert!(fixed.len() <= base.len());
    h.state_mut().command(Command::Undo);
    let undone = wait(&mut h);
    assert!(undone.contains(&new), "undo brings the violation back");
}

/// Constraints inferred from the matched devices of the two-stage OTA hold, are drawn, and a
/// moved member drags its mirror partner along.
#[test]
fn layout_constraints() {
    use ics_studio::layout_view::LayAction;
    let mut h = app(demo_args("ota2s", "layout", false));
    let r = h.state().active_ref().cloned().unwrap();
    let layout = |h: &Harness<'_, IcStudio>| {
        let p = h.state().project().unwrap();
        p.cell(&r).unwrap().layout.clone().unwrap()
    };
    h.state_mut().command(Command::Lay(LayAction::InferConstraints));
    h.run_steps(3);
    let l = layout(&h);
    assert!(!l.constraints.is_empty(), "{:?}", h.state().console_lines().collect::<Vec<_>>());
    let p = h.state().project().unwrap().clone();
    assert!(ics_core::constraints::errors(&l, &p).iter().all(|e| *e == 0));
    shot(&mut h, "41_layout_constraints");
}

/// Compare With Saved: geometry drawn since the last save shows as added.
#[test]
fn layout_compare_saved() {
    use ics_studio::layout_view::LayAction;
    let dir = std::env::temp_dir().join(format!("ics-ui-compare-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    ics_core::examples::demo_project().save_as(&dir).unwrap();
    let args = LaunchArgs {
        project: Some(dir.clone()),
        demo: false,
        workspace: Some("layout".into()),
        cell: Some("cmos_inverter".into()),
        run: false,
    };
    let mut h = app(args);
    let r = h.state().active_ref().cloned().unwrap();
    h.state_mut().command(Command::CiwEval("dbCreateRect(geGetEditCellView() \"metal2\" list(1:1 3:2))".into()));
    h.run_steps(2);
    h.state_mut().command(Command::Lay(LayAction::CompareSaved));
    h.run_steps(3);
    let ed = h.state().layout_editor(&r).unwrap();
    let c = ed.compare.as_ref().expect("compare view");
    assert_eq!(c.diff.layers.len(), 1, "{:?}", h.state().console_lines().collect::<Vec<_>>());
    assert_eq!(c.diff.layers[0].added.area(), 2_000_000);
    shot(&mut h, "42_layout_compare");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Compacting the schematic-driven placement of the full adder shrinks it without adding DRC
/// violations, as one undoable edit.
#[test]
fn layout_compact() {
    use ics_studio::layout_view::LayAction;
    let mut h = full_adder_placed("compact");
    h.state_mut().command(Command::SetWorkspace(Workspace::Layout));
    h.run_steps(3);
    let r = h.state().active_ref().cloned().unwrap();
    let get = |h: &Harness<'_, IcStudio>| h.state().project().unwrap().cell(&r).unwrap().layout.clone().unwrap();
    let before = get(&h);
    let p = h.state().project().unwrap().clone();
    let deck = ics_core::drc::parse_any(&p.tech.deck_text(), &p.tech).unwrap().deck;
    let v0 = ics_core::drc::run(&deck, &before, &p, &p.tech).violations.len();
    h.state_mut().command(Command::Lay(LayAction::Compact));
    h.run_steps(3);
    let after = get(&h);
    let area = |l: &ics_core::Layout| {
        let b = ics_core::constraints::item_boxes(l, &p).into_values().reduce(|a, b| a.union(&b)).unwrap();
        b.width() as i128 * b.height() as i128
    };
    assert!(area(&after) < area(&before), "{:?}", h.state().console_lines().collect::<Vec<_>>());
    assert!(ics_core::drc::run(&deck, &after, &p, &p.tech).violations.len() <= v0);
    shot(&mut h, "43_layout_compact");
    h.state_mut().command(Command::Undo);
    h.run_steps(2);
    assert_eq!(get(&h), before);
}
