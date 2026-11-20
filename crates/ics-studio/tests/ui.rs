//! Headless UI smoke tests: drive the full app through each workspace with a real wgpu
//! renderer. Set `ICS_SHOT_DIR=<dir>` to also write PNG screenshots of every scenario.

use std::time::Duration;

use eframe::egui;
use egui_kittest::Harness;
use ics_core::CellRef;
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

/// The welcome screen's example cards open a topic's example project.
#[test]
fn open_example_from_welcome() {
    use egui_kittest::kittest::Queryable;
    let mut h = app(LaunchArgs { project: None, demo: false, workspace: Some("home".into()), cell: None, run: false });
    h.get_by_label("Analog").click();
    h.run_steps(3);
    let p = h.state().project().expect("example opened");
    assert_eq!(p.name, "analog");
    let cells: Vec<String> = p.all_cells().filter(|(r, _)| r.lib == "work").map(|(r, _)| r.cell).collect();
    assert_eq!(cells, ["bandgap", "current_mirror", "ota2s", "ota5t"]);
    assert!(p.root.is_none(), "opened unsaved");
}

/// Save Examples to Folder: writes the missing example projects and keeps existing ones.
#[test]
fn save_examples_to_folder() {
    let dir = std::env::temp_dir().join(format!("ics-ui-examples-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut h = app(LaunchArgs { project: None, demo: false, workspace: Some("home".into()), cell: None, run: false });
    assert!(h.state_mut().write_examples(&dir));
    assert!(dir.join("analog").join("ic-studio.json").exists());
    assert!(h.state().console_lines().any(|l| l.starts_with("Wrote example project(s) getting_started, analog")));
    std::fs::remove_dir_all(dir.join("digital")).unwrap();
    assert!(h.state_mut().write_examples(&dir));
    assert!(h.state().console_lines().any(|l| l.starts_with("Wrote example project(s) digital to")), "only the missing one");
    assert!(h.state_mut().write_examples(&dir));
    assert!(h.state().console_lines().any(|l| l.starts_with("The example projects are already in")));
    std::fs::remove_dir_all(&dir).unwrap();
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

/// The layer palette can hide layers with no shapes in the cell's hierarchy.
#[test]
fn layout_used_layers_only() {
    use egui_kittest::kittest::Queryable;
    use ics_studio::layout_view::LayAction;
    let args = LaunchArgs {
        project: None,
        demo: true,
        workspace: Some("layout".into()),
        cell: Some("cmos_inverter".into()),
        run: false,
    };
    let mut h = app(args);
    let r = h.state().active_ref().cloned().unwrap();
    h.state_mut().command(Command::Lay(LayAction::UsedLayersOnly));
    h.run_steps(4);
    assert!(h.state().layout_editor(&r).expect("editor").used_layers_only);
    assert!(h.query_all_by_label_contains("unused layers hidden").next().is_some(), "unused layers are listed");
    settle(&mut h, "layout_used_layers");
    shot(&mut h, "layout_used_layers");
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
fn demo_dram_array() {
    scenario("demo_dram_array", true, Some("dram_array"), "layout", false);
}

#[test]
fn demo_nand_array() {
    scenario("demo_nand_array", true, Some("nand_array"), "layout", false);
}

#[test]
fn demo_cells_layout() {
    for cell in ["dram1t1c", "nand_string", "aps2s"] {
        scenario(&format!("demo_{cell}_layout"), true, Some(cell), "layout", false);
    }
}

#[test]
fn demo_pixel_array() {
    scenario("demo_pixel_array", true, Some("pixel_array"), "layout", false);
}

#[test]
fn schematic_from_pixel_layout() {
    // aps4t has a hand-drawn schematic, so the drawing goes to a new cell next to it.
    let args = LaunchArgs { project: None, demo: true, workspace: Some("layout".into()), cell: Some("aps4t".into()), run: false };
    let mut h = app(args);
    h.state_mut().command(Command::SchematicFromLayout);
    h.run_steps(6);
    settle(&mut h, "schematic_from_layout");
    let target = CellRef::new("work", "aps4t_from_layout");
    assert_eq!(h.state().active_ref(), Some(&target));
    let sch = h.state().project().and_then(|p| p.cell(&target)).and_then(|c| c.schematic.as_ref()).expect("schematic");
    for name in ["M1_TG", "M4_RST", "M3_SF", "M2_SEL", "IPH_D1", "VTX", "ICOL_COL"] {
        assert!(sch.instance_by_name(name).is_some(), "{name} missing");
    }
    // The pixels were characterised in the background, in the Pixel Circuit window.
    assert!(h.state().pixel_circuit().results.first().is_some_and(|r| r.1.is_ok()));
    h.state_mut().pixel_circuit_mut().open = false;
    h.state_mut().command(Command::FitView);
    h.run_steps(4);
    shot(&mut h, "schematic_from_pixel_layout");
}

#[test]
fn pixel_circuit_window() {
    let args = LaunchArgs { project: None, demo: true, workspace: Some("layout".into()), cell: Some("aps2s".into()), run: false };
    let mut h = app(args);
    h.state_mut().command(Command::PixelCircuit);
    h.run_steps(4);
    settle(&mut h, "pixel_circuit");
    let v = h.state().pixel_circuit();
    assert!(v.open && v.error.is_none(), "{:?}", v.error);
    assert_eq!(v.results.len(), 1);
    let m = v.results[0].1.as_ref().expect("characterised");
    assert_eq!(m.pds.len(), 2);
    h.run_steps(2);
    shot(&mut h, "pixel_circuit");
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
    shot(&mut h, "30_rule_designer_guide");
    use egui_kittest::kittest::Queryable;
    h.get_by_label("+ Rule").click();
    h.run_steps(3);
    shot(&mut h, "30_rule_designer_add");
    h.get_by_label("Minimum area").click();
    h.run_steps(3);
    let added = ics_core::drc::parse_deck(&h.state().rule_designer().deck_text(&tech), &tech).unwrap();
    assert_eq!(added.rules.len(), deck.rules.len() + 1);
    shot(&mut h, "30_rule_designer");
}

/// PCell Designer: a drawn 3-finger NMOS becomes a PCell with nf / w / l through the
/// suggestions, passes the corner check and is saved as a placeable master.
#[test]
fn pcell_designer() {
    use egui_kittest::kittest::Queryable;
    let args = LaunchArgs {
        project: None,
        demo: true,
        workspace: Some("layout".into()),
        cell: Some("cmos_inverter".into()),
        run: false,
    };
    let mut h = app(args);
    let r = {
        let p = h.state_mut().project_mut().unwrap();
        let drawn = ics_core::pcell::PCell::mos(false, 1000, 180, 3).generate(&p.tech).unwrap();
        let mut c = ics_core::Cell::new("nmos_drawn");
        c.layout = Some(drawn);
        p.add_cell("work", c).unwrap()
    };
    h.state_mut().command(Command::OpenCell(r.clone(), Some(Workspace::Layout)));
    h.run_steps(3);
    h.state_mut().command(Command::PCellFromActive);
    h.run_steps(4);
    assert!(h.state().pcell_designer().open);
    assert_eq!(h.state().pcell_designer().template().unwrap().name, "nmos_drawn_pc");
    shot(&mut h, "32_pcell_designer_guide");
    for label in ["Contacts to fills", "Detect fingers (nf)", "Width stretch (w)", "Length stretch (l)"] {
        h.get_all_by_label(label).next().unwrap().click();
        h.run_steps(2);
    }
    let t = h.state().pcell_designer().template().unwrap().clone();
    let names: Vec<&str> = t.params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["nf", "w", "l"]);
    h.get_by_label("Design").click();
    h.run_steps(3);
    shot(&mut h, "32_pcell_designer");
    // Layer by layer: the panel lists the drawing's layers; Next / Prev / [ ] show one alone.
    let counts = h.state().pcell_designer().layer_counts().to_vec();
    assert!(counts.len() >= 4, "{counts:?}");
    assert!(counts.iter().all(|c| c.source > 0 && c.generated > 0), "{counts:?}");
    h.get_by_label("Next").click();
    h.run_steps(2);
    assert_eq!(h.state().pcell_designer().solo_layer(), Some(counts[0].layer));
    shot(&mut h, "32_pcell_designer_layer");
    h.get_by_label("Prev").click();
    h.run_steps(2);
    assert_eq!(h.state().pcell_designer().solo_layer(), Some(counts.last().unwrap().layer), "wraps around");
    h.get_by_label("All").click();
    h.run_steps(2);
    assert_eq!(h.state().pcell_designer().solo_layer(), None);
    // The suggestions keep the drawing: at the defaults every layer reproduces it.
    assert_eq!(h.state().pcell_designer().differing_layers(), Some(vec![]));
    // A condition that drops the gate at the defaults breaks poly; it is flagged and marked.
    let poly = h.state().project().unwrap().tech.poly().unwrap();
    let gate = h.state().pcell_designer().template().unwrap().source.shapes.iter().find(|s| s.layer == poly).unwrap().bbox();
    h.state_mut().pcell_designer_mut().template_mut().unwrap().ops.push(ics_core::pcell_design::Op::Condition {
        region: gate,
        when: "0".into(),
        layers: vec![poly],
    });
    h.run_steps(3);
    assert_eq!(h.state().pcell_designer().differing_layers(), Some(vec![poly]));
    shot(&mut h, "32_pcell_designer_diff");
    h.state_mut().pcell_designer_mut().template_mut().unwrap().ops.pop();
    h.run_steps(2);
    assert_eq!(h.state().pcell_designer().differing_layers(), Some(vec![]));
    // A fixed layer is stored with the design.
    h.state_mut().pcell_designer_mut().set_fixed(poly, true);
    h.run_steps(2);
    assert_eq!(h.state().pcell_designer().template().unwrap().fixed, [poly]);
    h.state_mut().pcell_designer_mut().set_fixed(poly, false);
    h.run_steps(2);
    // A deck that reads metal1 only: the other drawn layers are reported as unchecked.
    let deck = h.state().project().unwrap().tech.drc_deck.clone();
    h.state_mut().project_mut().unwrap().tech.drc_deck = Some("width metal1 0.23\n".into());
    h.run_steps(2);
    let unchecked = |h: &Harness<'_, IcStudio>| {
        h.state().pcell_designer().findings().iter().find(|f| f.message.starts_with("No DRC rule")).map(|f| f.message.clone())
    };
    let msg = unchecked(&h).expect("unchecked layers reported");
    assert!(msg.contains("poly") && msg.contains("contact") && !msg.contains("metal1") && !msg.contains("text"), "{msg}");
    shot(&mut h, "32_pcell_designer_drc");
    h.state_mut().project_mut().unwrap().tech.drc_deck = deck;
    h.run_steps(2);
    // Device layer roles: the MOS suggestions' gate and diffusion layers are set from here.
    h.state_mut().command(Command::SetDeviceRoles("metal1".into(), vec!["active".into()]));
    h.run_steps(1);
    assert_eq!(h.state().project().unwrap().tech.roles.poly, "metal1");
    h.state_mut().command(Command::SetDeviceRoles("nosuch".into(), vec!["active".into()]));
    h.run_steps(1);
    assert_eq!(h.state().project().unwrap().tech.roles.poly, "metal1", "unknown layers are refused");
    h.state_mut().command(Command::SetDeviceRoles("poly".into(), vec!["active".into()]));
    h.run_steps(2);
    h.get_all_by_label("Corners").next().unwrap().click();
    h.run_steps(2);
    h.get_by_label("Run corner check").click();
    h.run_steps(2);
    settle(&mut h, "pcell corners");
    h.run_steps(3);
    shot(&mut h, "32_pcell_designer_corners");
    h.get_by_label("Save PCell").click();
    h.run_steps(3);
    let p = h.state().project().unwrap();
    let masters = p.template_masters();
    assert_eq!(masters.len(), 1, "saved");
    assert_eq!(masters[0].0, CellRef::new("work", "nmos_drawn_pc"));
    let variant = ics_core::pcell::PCell::Template { def: Box::new(t), values: vec![("nf".into(), 4.0)] };
    assert!(variant.generate(&p.tech).unwrap().shapes.len() > 20);
}

#[test]
fn rule_designer_without_project() {
    let args = LaunchArgs { project: None, demo: false, workspace: None, cell: None, run: false };
    let mut h = app(args);
    assert!(h.state().project().is_none());
    h.state_mut().command(Command::RuleDesigner);
    h.run_steps(6);
    assert!(h.state().rule_designer().open);
    let tech = ics_core::Tech::generic();
    let text = h.state().rule_designer().deck_text(&tech);
    assert!(!ics_core::drc::parse_deck(&text, &tech).unwrap().rules.is_empty());
    shot(&mut h, "31_rule_designer_standalone");
    let dir = std::env::temp_dir().join(format!("ics_rd_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rules.deck");
    h.state_mut().rule_designer_mut().save_file(&tech, &path, true).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    assert_eq!(h.state().rule_designer().file(), Some(path.as_path()));
    let _ = std::fs::remove_dir_all(&dir);
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

/// A long SKILL script runs in the background behind a dialog and can be cancelled; the
/// session (and the project) survive it.
#[test]
fn long_skill_script_can_be_cancelled() {
    let mut h = app(demo_args("cmos_inverter", "layout", false));
    h.state_mut().command(Command::CiwEval("procedure(answer() 42) while(t 1)".into()));
    h.run_steps(3);
    assert!(h.state().skill_running(), "still looping");
    shot(&mut h, "31b_skill_running");
    h.state_mut().command(Command::CancelSkill);
    // Up to 5 s: the worker notices the cancel at its next step, later on a loaded machine.
    for _ in 0..250 {
        h.run_steps(1);
        if !h.state().skill_running() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(!h.state().skill_running());
    assert!(h.state().ciw().lines.last().is_some_and(|(_, t)| t.contains("cancelled")), "{:?}", h.state().ciw().lines);
    h.state_mut().command(Command::CiwEval("answer()".into()));
    h.run_steps(2);
    assert!(h.state().ciw().lines.last().is_some_and(|(_, t)| t == "42"), "{:?}", h.state().ciw().lines);
    assert!(h.state().project().is_some());
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

/// Pixel Optics: the example BSI stack's colour responses, and its photocurrent written into
/// the 4T pixel's current source.
#[test]
fn pixel_optics() {
    let mut h = app(demo_args("aps4t", "schematic", false));
    settle(&mut h, "pixel optics");
    h.state_mut().command(Command::RunPixelOptics);
    h.run_steps(3);
    let px = h.state().pixel_optics();
    let r = px.result.as_ref().unwrap().as_ref().unwrap();
    let at = |c: usize, l: f64| r.qe[c][r.wavelengths.iter().position(|&x| (x - l).abs() < 1e-9).unwrap()];
    assert!(at(3, 0.45) > 0.5 && at(1, 0.54) > 0.5 && at(0, 0.63) > 0.4, "{:?}", r.qe);
    let green = px.currents.unwrap()[1];
    h.state_mut().command(Command::ApplyPhotocurrent("I1".into(), 1));
    h.run_steps(2);
    let cell = h.state().project().unwrap().cell(&ics_core::CellRef::new("work", "aps4t")).unwrap();
    let i1 = cell.schematic.as_ref().unwrap().instances.iter().find(|i| i.name == "I1").unwrap();
    let dc = ics_core::units::parse_spice(i1.param("dc")).unwrap();
    assert!((dc / green - 1.0).abs() < 1e-3, "{dc} vs {green}");
    shot(&mut h, "34_pixel_optics");
    // The RCWA solver through the same command.
    h.state_mut().pixel_optics_mut().rcwa = true;
    h.state_mut().pixel_optics_mut().harmonics = 3;
    h.state_mut().command(Command::RunPixelOptics);
    h.run_steps(3);
    let r = h.state().pixel_optics().result.as_ref().unwrap().as_ref().unwrap();
    assert_eq!(r.method, "rcwa");
    assert!(r.qe.iter().all(|q| q.iter().all(|v| v.is_finite() && *v >= 0.0)));
    if std::env::var("ICS_SHOT_DIR").is_ok() {
        h.state_mut().pixel_optics_mut().harmonics = 11;
        h.state_mut().command(Command::RunPixelOptics);
        h.run_steps(3);
        shot(&mut h, "35_pixel_optics_rcwa");
    }
    // The demo pixel itself: metals and photodiode from the open project's layout, with the
    // lens centred over the photodiode.
    h.state_mut().pixel_optics_mut().rcwa = false;
    h.state_mut().pixel_optics_mut().text = include_str!("../../../pixels/aps4t_fsi.toml").to_string();
    h.state_mut().command(Command::RunPixelOptics);
    h.run_steps(3);
    let px = h.state().pixel_optics();
    let r = px.result.as_ref().unwrap().as_ref().unwrap();
    let st = px.stack.as_ref().unwrap();
    assert!((st.pitch - 5.6).abs() < 1e-9 && st.photodiode.rects.len() == 1);
    assert!(st.layers.iter().filter(|l| l.pattern.as_ref().is_some_and(|p| !p.rects.is_empty())).count() == 3);
    assert!(st.microlens.as_ref().unwrap().offset[1] < -1.0, "lens over the photodiode, below the transistors");
    let g = r.qe[1][r.wavelengths.iter().position(|&l| (l - 0.53).abs() < 1e-9).unwrap()];
    assert!((0.3..0.7).contains(&g), "{g}");
    shot(&mut h, "36_pixel_optics_layout");
    // The open cell's stack from the technology's cross section, with carrier diffusion.
    h.state_mut().command(Command::PixelStackFromPdk);
    h.state_mut().command(Command::RunPixelOptics);
    h.run_steps(3);
    let px = h.state().pixel_optics();
    assert!(px.text.contains("cell = \"work/aps4t\""), "{}", px.text);
    let r = px.result.as_ref().unwrap().as_ref().unwrap();
    assert!(px.stack.as_ref().unwrap().silicon.is_some());
    assert!(r.crosstalk_electrical[1].iter().any(|&x| x > 0.0));
    shot(&mut h, "37_pixel_optics_pdk");
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
        sw.setup = Default::default();
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

/// Verification: the demo's tests (every cell with specs) run together; a failing limit shows
/// as a regression against the saved run.
#[test]
fn verification_window() {
    let dir = std::env::temp_dir().join(format!("ics-ui-verify-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut h = app(demo_args("ota5t", "simulation", false));
    h.state_mut().project_mut().unwrap().save_as(&dir).unwrap();
    h.state_mut().command(Command::Verification);
    h.run_steps(2);
    assert!(h.state_mut().verification().open);
    h.state_mut().command(Command::RunVerification);
    h.run_steps(2);
    settle(&mut h, "verification");
    let run = h.state_mut().verification().run.clone().unwrap();
    let names: Vec<&str> = run.tests.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"ota5t") && names.contains(&"rc_lowpass"), "{names:?}");
    assert_eq!(run.status(), ics_core::verify::Status::Pass, "{run:?}");
    assert_eq!(ics_core::verify::list_history(&dir).len(), 1);
    // Tighten the OTA gain: the rerun fails and is compared with the first run.
    {
        let p = h.state_mut().project_mut().unwrap();
        let c = p.cell_mut(&ics_core::CellRef::new("work", "ota5t")).unwrap();
        c.sim.sweep.specs[0].min = "40".into();
    }
    h.state_mut().command(Command::RunVerification);
    h.run_steps(2);
    settle(&mut h, "verification rerun");
    assert!(h.state().console_lines().any(|l| l.contains("1 regression(s)")), "no regression reported");
    h.run_steps(4);
    shot(&mut h, "36_verification");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Layer Display editor: a stipple bitmap and line style edited for metal1 are applied to the
/// technology and drawn in the layout.
#[test]
fn layer_display_editor() {
    use egui_kittest::kittest::Queryable;
    use ics_core::tech::{FillStyle, LineStyle, Stipple};
    let mut h = app(demo_args("cmos_inverter", "layout", false));
    h.state_mut().command(Command::LayerDisplay(Some("metal1".into())));
    h.run_steps(2);
    let custom = Stipple { rows: vec!["1100".into(), "0110".into(), "0011".into(), "1001".into()] };
    {
        let l = h.state_mut().layer_display_editor().current().unwrap();
        assert_eq!(l.name, "metal1");
        l.fill = FillStyle::Grid;
        l.stipple = Some(custom.clone());
        l.line = LineStyle::Dashed;
        l.outline = Some([255, 255, 255]);
    }
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(3);
    let tech = &h.state().project().unwrap().tech;
    let m1 = &tech.layers[tech.layer_index("metal1").unwrap()];
    assert_eq!((m1.fill, m1.line, m1.outline), (FillStyle::Grid, LineStyle::Dashed, Some([255, 255, 255])));
    assert_eq!(m1.pattern(), Some(custom));
    shot(&mut h, "49_layer_display");
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

/// OPC: previews the corrected poly mask and writes it into the cell, undoable.
#[test]
fn opc() {
    use egui_kittest::kittest::Queryable;
    let mut h = app(demo_args("cmos_inverter", "layout", false));
    let before = shape_count(&h, "cmos_inverter");
    h.state_mut().command(Command::Opc);
    h.run_steps(4);
    h.get_by_label("Preview").click();
    h.run_steps(3);
    h.get_by_label("Printed (OPC)").scroll_to_me();
    h.run_steps(3);
    shot(&mut h, "36_opc");
    h.state_mut().command(Command::ApplyOpc);
    h.run_steps(3);
    let lines: Vec<String> = h.state().console_lines().map(String::from).collect();
    assert!(lines.iter().any(|l| l.starts_with("OPC on")), "{lines:?}");
    let once = shape_count(&h, "cmos_inverter");
    assert!(once > before);
    assert!(h.state().project().unwrap().tech.layer_index("poly_opc").is_some());
    h.state_mut().command(Command::ApplyOpc);
    h.run_steps(3);
    assert_eq!(shape_count(&h, "cmos_inverter"), once, "applying again replaces the mask");
    let has_mask_layer = |h: &Harness<'_, IcStudio>| h.state().project().unwrap().tech.layer_index("poly_opc").is_some();
    let layers = h.state().project().unwrap().tech.layers.len();
    h.state_mut().command(Command::Undo);
    h.run_steps(2);
    assert!(has_mask_layer(&h), "the first mask is still there");
    h.state_mut().command(Command::Undo);
    h.run_steps(2);
    assert_eq!(shape_count(&h, "cmos_inverter"), before);
    assert!(!has_mask_layer(&h), "undo also removes the layer OPC added");
    assert_eq!(h.state().project().unwrap().tech.layers.len(), layers - 1);
    h.state_mut().command(Command::Redo);
    h.run_steps(2);
    assert!(has_mask_layer(&h), "redo restores it");
    assert_eq!(shape_count(&h, "cmos_inverter"), once);
}

/// Monte Carlo on the RC filter: spread of the bandwidth, yield and the HTML datasheet.
#[test]
fn monte_carlo_sweep() {
    let mut h = app(demo_args("rc_lowpass", "simulation", false));
    h.state_mut().command(Command::SweepManager);
    h.run_steps(2);
    {
        let sw = h.state_mut().sweep_manager();
        sw.setup = Default::default();
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
    // The surrogate's predicted yield agrees with the simulated one; R1 sets the bandwidth.
    let c = &sw.prediction[0];
    let (lo, hi) = ics_core::predict::wilson(pass, n);
    let y = c.prediction.joint.unwrap();
    assert!(y > lo - 0.05 && y < hi + 0.05, "{y} vs {pass}/{n}");
    let sp = c.prediction.specs[0].as_ref().unwrap();
    assert!(sp.model.q2 > 0.99 && sp.sensitivity[0] > 0.95, "{sp:?}");
    let html = sw.datasheet();
    assert!(html.contains("<h2>Specs</h2>") && html.contains("mc#40") && html.contains("Predicted yield"), "{html}");
    h.run_steps(4);
    shot(&mut h, "36_monte_carlo");
}

/// Device-model Monte Carlo on the CMOS inverter: spread of the switching threshold.
#[test]
fn device_monte_carlo() {
    let mut h = app(demo_args("cmos_inverter", "simulation", false));
    h.state_mut().command(Command::MonteCarlo);
    h.run_steps(2);
    {
        let mc = h.state_mut().monte_carlo();
        assert!(mc.open);
        mc.setup.runs = 12;
        mc.setup.measures.push(ics_core::sim_setup::McMeasure { analysis: "dc".into(), expr: "cross(v(out),0.9)".into() });
    }
    h.state_mut().command(Command::RunMonteCarlo);
    h.run_steps(2);
    settle(&mut h, "device monte carlo");
    let mc = h.state_mut().monte_carlo();
    assert!(mc.error.is_none(), "{:?}", mc.error);
    let res = mc.result.as_ref().expect("monte carlo result");
    let st = &res.stats[0];
    assert_eq!(st.n + res.failed.len(), 12, "{res:?}");
    assert!(st.n >= 10 && st.mean > 0.5 && st.mean < 1.3 && st.sigma > 0.0 && st.min < st.max, "{st:?}");
    assert!(h.state().console_lines().any(|l| l.contains("Monte Carlo: 12 run(s)")));
    h.run_steps(4);
    shot(&mut h, "37_device_monte_carlo");
}

/// The simulation panel opens Monte Carlo; Stop ends a long run early and keeps its samples.
#[test]
fn monte_carlo_from_panel_and_stop() {
    let mut h = app(demo_args("cmos_inverter", "simulation", false));
    h.run_steps(2);
    use egui_kittest::kittest::Queryable;
    h.get_by_label("Monte Carlo...").click();
    h.run_steps(2);
    {
        let mc = h.state_mut().monte_carlo();
        assert!(mc.open, "the panel button opens the window");
        mc.setup.runs = 1_000_000;
        mc.setup.measures.push(ics_core::sim_setup::McMeasure { analysis: "op".into(), expr: "v(out)".into() });
    }
    h.state_mut().command(Command::RunMonteCarlo);
    let start = std::time::Instant::now();
    while h.state_mut().monte_carlo().progress().is_none_or(|d| d < 3) {
        assert!(start.elapsed() < Duration::from_secs(60), "monte carlo did not start");
        h.run_steps(1);
    }
    h.state_mut().command(Command::StopMonteCarlo);
    h.run_steps(2);
    settle(&mut h, "stopped monte carlo");
    let mc = h.state_mut().monte_carlo();
    let res = mc.result.as_ref().expect("partial result");
    assert!(res.cancelled && res.runs >= 3 && res.runs < 1_000_000, "{} runs", res.runs);
    assert_eq!(res.stats[0].n + res.failed.len(), res.runs);
    assert!(h.state().console_lines().any(|l| l.contains("(stopped)")));
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
    assert_eq!(ringed, before, "the ring is an instance, not loose shapes");
    {
        let p = h.state().project().unwrap();
        let (_, c) = p.all_cells().find(|(_, c)| c.name == "inv_chain").unwrap();
        let ring = c.layout.as_ref().unwrap().instances.last().unwrap();
        let pc = p.cell(&ring.cell).and_then(|c| c.pcell.clone());
        assert!(matches!(pc, Some(ics_core::pcell::PCell::GuardRing { well: false, .. })), "{pc:?}");
    }
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
        include_str!("../src/pcell_designer.rs"),
        include_str!("../../ics-core/src/pcell_design.rs"),
        include_str!("../src/ciw.rs"),
        include_str!("../src/copilot.rs"),
        include_str!("../src/chat.rs"),
        include_str!("../src/calc_view.rs"),
        include_str!("../src/sweep_view.rs"),
        include_str!("../src/mc_view.rs"),
        include_str!("../src/stream_map_view.rs"),
        include_str!("../src/fill_view.rs"),
        include_str!("../src/opc_view.rs"),
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

/// Hierarchical schematics: buffer_tb places two inv cells drawn as pin boxes; Shift+X on one
/// descends into inv and Shift+B returns; Place Cell Instance adds another, undoably.
#[test]
fn hierarchical_schematic() {
    let mut h = app(LaunchArgs {
        project: None,
        demo: true,
        workspace: Some("schematic".into()),
        cell: Some("buffer_tb".into()),
        run: false,
    });
    h.run_steps(4);
    shot(&mut h, "schematic_hierarchy");
    let tb = CellRef::new("work", "buffer_tb");
    let x1 = {
        let sch = h.state().project().unwrap().cell(&tb).unwrap().schematic.as_ref().unwrap();
        sch.instances.iter().find(|i| i.name == "X1").expect("X1").id
    };
    h.state_mut().command(Command::DescendSchematic(x1));
    h.run_steps(3);
    assert_eq!(h.state().active_ref().map(|r| r.cell.as_str()), Some("inv"));
    h.state_mut().command(Command::ReturnSchematic);
    h.run_steps(3);
    assert_eq!(h.state().active_ref(), Some(&tb));
    // Place a third inverter by clicking the canvas.
    let count = |h: &Harness<'_, IcStudio>| {
        h.state().project().unwrap().cell(&CellRef::new("work", "buffer_tb")).unwrap().schematic.as_ref().unwrap().instances.len()
    };
    let before = count(&h);
    h.state_mut().command(Command::PlaceCell(CellRef::new("work", "inv")));
    h.run_steps(2);
    let p = egui::pos2(900.0, 650.0);
    h.hover_at(p);
    h.run_steps(2);
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
    assert_eq!(count(&h), before + 1);
    let sch = h.state().project().unwrap().cell(&tb).unwrap().schematic.as_ref().unwrap();
    let x3 = sch.instances.iter().find(|i| i.name == "X3").expect("X3 placed");
    assert_eq!(x3.cell, Some(CellRef::new("work", "inv")));
    assert_eq!(x3.ports.len(), 2);
    h.key_press(egui::Key::Escape);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    h.run_steps(3);
    assert_eq!(count(&h), before, "placing is undoable");

    // Edit inv's symbol: moving a pin down reorders it, and buffer_tb's instances follow.
    use egui_kittest::kittest::Queryable;
    h.state_mut().command(Command::OpenCell(CellRef::new("work", "inv"), Some(Workspace::Schematic)));
    h.run_steps(3);
    h.state_mut().command(Command::EditSymbolDialog);
    h.run_steps(3);
    shot(&mut h, "symbol_pins");
    h.query_all_by_label("Down").next().expect("a Down button").click();
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(3);
    let p = h.state().project().unwrap();
    let spec = p.cell(&CellRef::new("work", "inv")).unwrap().schematic.as_ref().unwrap().symbol.clone().expect("a symbol spec");
    assert_eq!(spec.pins[0].0, "y");
    let tb = p.cell(&CellRef::new("work", "buffer_tb")).unwrap().schematic.as_ref().unwrap();
    assert!(tb.instances.iter().filter(|i| i.kind == ics_core::DeviceKind::Cell).all(|i| i.symbol.as_ref() == Some(&spec)));
}

/// MOS symbol style: with the 3-terminal style, N places an NMOS without a body pin; an existing
/// device switches style keeping its body net.
#[test]
fn schematic_mos_symbol_style() {
    let mut h = app(LaunchArgs {
        project: None,
        demo: true,
        workspace: Some("schematic".into()),
        cell: Some("rc_lowpass".into()),
        run: false,
    });
    h.run_steps(4);
    h.state_mut().command(Command::MosSymbol(false));
    h.run_steps(2);
    let p = egui::pos2(760.0, 640.0);
    h.hover_at(p);
    h.run_steps(2);
    h.key_press(egui::Key::N);
    h.run_steps(2);
    h.hover_at(p);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        });
        h.step();
    }
    h.run_steps(3);
    let kinds = |h: &Harness<'_, IcStudio>, cell: &str| -> Vec<ics_core::DeviceKind> {
        let p = h.state().project().unwrap();
        let (_, c) = p.all_cells().find(|(_, c)| c.name == cell).unwrap();
        c.schematic.as_ref().unwrap().instances.iter().map(|i| i.kind).filter(|k| k.is_mos()).collect()
    };
    assert_eq!(kinds(&h, "rc_lowpass"), [ics_core::DeviceKind::Nmos3]);
    h.key_press(egui::Key::Escape);
    h.run_steps(2);

    // The project's PMOS body net goes to new 3-terminal PMOS devices.
    h.state_mut().command(Command::DefaultBulk(true, "avdd".into()));
    h.run_steps(2);
    assert_eq!(h.state().project().unwrap().settings.default_bulk(ics_core::DeviceKind::Pmos3), "avdd");
    let p2 = egui::pos2(860.0, 640.0);
    h.hover_at(p2);
    h.run_steps(2);
    h.key_press(egui::Key::P);
    h.run_steps(2);
    h.hover_at(p2);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos: p2,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        });
        h.step();
    }
    h.run_steps(3);
    let p = h.state().project().unwrap();
    let (_, c) = p.all_cells().find(|(_, c)| c.name == "rc_lowpass").unwrap();
    let pm = c.schematic.as_ref().unwrap().instances.iter().find(|i| i.kind == ics_core::DeviceKind::Pmos3).expect("PMOS placed");
    assert_eq!(pm.param("bulk"), "avdd");
    h.key_press(egui::Key::Escape);

    // Show the inverter with both devices in the 3-terminal style.
    let p = h.state_mut().project_mut().unwrap();
    let (r, _) = p.all_cells().find(|(_, c)| c.name == "cmos_inverter").unwrap();
    h.state_mut().command(Command::OpenCell(r.clone(), None));
    let p = h.state_mut().project_mut().unwrap();
    let sch = p.cell_mut(&r).unwrap().schematic.as_mut().unwrap();
    let ids: Vec<u64> = sch.instances.iter().filter(|i| i.kind.is_mos()).map(|i| i.id).collect();
    for id in ids {
        assert!(sch.set_body_pin(id, false, None).unwrap());
    }
    assert!(sch.instances.iter().filter(|i| i.kind.is_mos()).all(|i| ["0", "vdd"].contains(&i.param("bulk"))));
    settle(&mut h, "03b_schematic_mos3");
    shot(&mut h, "03b_schematic_mos3");
}

/// The inspector's symbol switch: refused (with the reason shown) for a body on an unnamed net;
/// switching back to 4-terminal labels the body pin and says so; an emptied bulk takes the
/// project's default body net.
#[test]
fn inspector_body_pin_switch() {
    use egui_kittest::kittest::Queryable;
    let mut h = app(demo_args("cmos_inverter", "schematic", false));
    h.run_steps(4);
    let r = h.state().project().unwrap().all_cells().find(|(_, c)| c.name == "cmos_inverter").unwrap().0;
    let kind = |h: &Harness<'_, IcStudio>, name: &str| {
        let p = h.state().project().unwrap();
        p.cell(&r).unwrap().schematic.as_ref().unwrap().instances.iter().find(|i| i.name == name).unwrap().clone()
    };
    let pmos = h
        .state()
        .project()
        .unwrap()
        .cell(&r)
        .unwrap()
        .schematic
        .as_ref()
        .unwrap()
        .instances
        .iter()
        .find(|i| i.kind.is_pmos())
        .unwrap()
        .name
        .clone();

    // 3-terminal and back: the body pin gets a label with its net, and the inspector says so.
    h.state_mut().command(Command::SelectInstance(pmos.clone()));
    h.run_steps(3);
    h.get_all_by_label("3-terminal").last().unwrap().click(); // the inspector's, not the palette's
    h.run_steps(3);
    assert_eq!(kind(&h, &pmos).kind, ics_core::DeviceKind::Pmos3);
    let labels =
        |h: &Harness<'_, IcStudio>| h.state().project().unwrap().cell(&r).unwrap().schematic.as_ref().unwrap().labels.len();
    let before = labels(&h);
    h.get_all_by_label("4-terminal").last().unwrap().click();
    h.run_steps(3);
    assert_eq!(kind(&h, &pmos).kind, ics_core::DeviceKind::Pmos);
    assert_eq!(labels(&h), before + 1, "the body pin is labelled with its net");
    assert!(h.query_by_label_contains("Labelled the body pin").is_some(), "and the inspector says so");
    shot(&mut h, "03c_body_pin_back");

    // An emptied bulk takes the project default.
    h.state_mut().command(Command::DefaultBulk(true, "avdd".into()));
    h.run_steps(2);
    h.get_all_by_label("3-terminal").last().unwrap().click(); // the inspector's, not the palette's
    h.run_steps(3);
    let id = kind(&h, &pmos).id;
    h.state_mut()
        .project_mut()
        .unwrap()
        .cell_mut(&r)
        .unwrap()
        .schematic
        .as_mut()
        .unwrap()
        .instance_mut(id)
        .unwrap()
        .params
        .insert("bulk".into(), String::new());
    h.run_steps(3);
    assert_eq!(kind(&h, &pmos).param("bulk"), "avdd");

    // A body on an unnamed net is refused, with the reason in the inspector.
    let p = h.state_mut().project_mut().unwrap();
    let sch = p.cell_mut(&r).unwrap().schematic.as_mut().unwrap();
    let a = sch.add_instance(ics_core::DeviceKind::Nmos, ics_core::geom::Pt::new(60, 0), Default::default());
    let b = sch.add_instance(ics_core::DeviceKind::Nmos, ics_core::geom::Pt::new(80, 0), Default::default());
    let body = sch.instance(a).unwrap().pin_positions()[3].1;
    let src = sch.instance(b).unwrap().pin_positions()[2].1;
    sch.add_wire(body, src);
    let name = sch.instance(a).unwrap().name.clone();
    h.state_mut().command(Command::SelectInstance(name.clone()));
    h.run_steps(3);
    h.get_all_by_label("3-terminal").last().unwrap().click(); // the inspector's, not the palette's
    h.run_steps(3);
    assert_eq!(kind(&h, &name).kind, ics_core::DeviceKind::Nmos);
    assert!(h.query_by_label_contains("no name").is_some(), "the reason is shown");
    shot(&mut h, "03d_body_pin_refused");
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
    // So are edits of tools that change layouts as a side effect (here OPC writing its mask).
    let r = call(&mut h, "opc", serde_json::json!({"cell": "cmos_inverter", "layer": "poly", "apply": true}));
    assert!(!r.is_error, "{r:?}");
    assert!(shape_count(&h, "cmos_inverter") > before);
    h.state_mut().command(Command::Undo);
    h.run_steps(2);
    assert_eq!(shape_count(&h, "cmos_inverter"), before, "an applied OPC mask is undoable");
    // With unsaved changes in the window, an agent cannot replace the project.
    let r =
        call(&mut h, "new_project", serde_json::json!({"path": std::env::temp_dir().join("ics-ui-never").display().to_string()}));
    assert!(r.is_error && r.text.contains("unsaved"), "{r:?}");

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

/// IR drop / EM of a supply from the schematic's operating point, drawn as a heatmap.
#[test]
fn ir_drop_heatmap() {
    let args =
        LaunchArgs { project: None, demo: true, workspace: Some("layout".into()), cell: Some("ring_osc5".into()), run: false };
    let mut h = app(args);
    let r = h.state().active_ref().cloned().unwrap();
    h.state_mut().command(Command::IrDrop(Some("vdd".into())));
    h.run_steps(2);
    settle(&mut h, "ir_drop");
    let ed = h.state().layout_editor(&r).expect("editor");
    let rep = ed.verify.emir.as_ref().expect("IR drop report").as_ref().expect("IR drop ran");
    assert!(!rep.loads.is_empty() && rep.worst_drop > 0.0, "{rep:?}");
    shot(&mut h, "ir_drop");
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
    // Not "verify": verification_window uses that temp directory.
    let (dir, big) = big_project("verify-bg");
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
fn import_gds_with_virtuoso_layer_map() {
    let dir = std::env::temp_dir().join(format!("ics-ui-layermap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut wtech = ics_core::tech::Tech::generic();
    wtech.layers[0].gds_layer = 222;
    wtech.layers[0].gds_datatype = 0;
    let mut l = ics_core::layout::Layout::default();
    l.add(0, ics_core::layout::Geometry::Rect(ics_core::IRect::new(0, 0, 400, 200)));
    let top = ics_core::project::CellRef::new("lib", "rx_top");
    let gds = dir.join("rx.gds");
    std::fs::write(&gds, ics_core::gds::write_gds("lib", &[(&top, &l)], &wtech)).unwrap();
    let map = dir.join("tech.layermap");
    std::fs::write(&map, "# layer purpose stream datatype\nRX drawing 222 0\nRX label 222 10\nRX exclude 222 5\n").unwrap();

    let args = LaunchArgs { project: None, demo: true, workspace: Some("layout".into()), cell: None, run: false };
    let mut h = Harness::builder()
        .with_size(egui::vec2(1440.0, 900.0))
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_eframe(|cc| IcStudio::new(cc, args));
    h.run_steps(4);
    h.state_mut().command(Command::ImportGdsWithLayerMap(Some(gds), Some(map)));
    h.run_steps(6);

    let console: Vec<String> = h.state().console_lines().map(str::to_string).collect();
    assert!(console.iter().any(|l| l.contains("added layer(s) RX, RX.exclude")), "{console:?}");
    let p = h.state().project().unwrap();
    let rx = p.tech.layer_index("RX").expect("RX layer added");
    let lib = p.libraries.iter().find(|l| l.name == "rx").expect("imported library");
    let shapes = &lib.cells[0].layout.as_ref().unwrap().shapes;
    assert_eq!(shapes.iter().map(|s| s.layer).collect::<Vec<_>>(), [rx]);
    assert_eq!(ics_core::stream_map::export_pairs(&p.tech)[rx], (222, 0));
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
    assert!(console.iter().any(|l| l.starts_with("Imported 2 cell(s)") && l.contains("library 'pixels'")), "{console:?}");
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

/// Split (Shift+S) cuts the shapes a line crosses; Convert to Polygon and Chamfer Corners
/// reshape the selection.
#[test]
fn layout_split_convert_chamfer() {
    use ics_studio::layout_view::LayAction;
    let dir = std::env::temp_dir().join(format!("ics-ui-split-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut p = ics_core::examples::demo_project();
    let (m1, m2) = (p.tech.layer_index("metal1").unwrap(), p.tech.layer_index("metal2").unwrap());
    let mut l = ics_core::Layout::default();
    l.add_rect(m1, 0, 0, 10_000, 4_000);
    l.add(m2, ics_core::Geometry::Path { points: vec![(0, 8_000), (10_000, 8_000)], width: 500 });
    let mut cell = ics_core::Cell::new("scratch");
    cell.layout = Some(l);
    let r = p.add_cell("work", cell).unwrap();
    p.save_as(&dir).unwrap();
    let mut h = open_layout(&dir, "scratch");
    let key = |h: &mut Harness<'_, IcStudio>, m: egui::Modifiers, k: egui::Key| {
        h.key_press_modifiers(m, k);
        h.run_steps(2);
    };
    h.hover_at(egui::pos2(870.0, 450.0));
    h.run_steps(2);
    key(&mut h, egui::Modifiers::NONE, egui::Key::F);
    // A vertical cut line through the middle of the fitted view crosses both shapes.
    key(&mut h, egui::Modifiers::SHIFT, egui::Key::S);
    click(&mut h, egui::pos2(870.0, 205.0));
    click(&mut h, egui::pos2(870.0, 705.0));
    let s = shapes_of(&h, "scratch");
    assert_eq!(s.len(), 4, "both shapes split in two: {s:?}");
    assert_eq!(s.iter().filter(|s| s.layer == m1).count(), 2);
    let area: i64 = s.iter().filter(|s| s.layer == m1).map(|s| s.bbox().width() * s.bbox().height()).sum();
    assert_eq!(area, 40_000_000, "the pieces keep the area");

    key(&mut h, egui::Modifiers::COMMAND, egui::Key::A);
    h.state_mut().command(Command::Lay(LayAction::ConvertToPolygon));
    h.run_steps(2);
    let s = shapes_of(&h, "scratch");
    assert!(s.iter().all(|s| matches!(&s.geom, ics_core::Geometry::Polygon { points } if points.len() == 4)), "{s:?}");
    h.state_mut().command(Command::Lay(LayAction::Chamfer));
    h.run_steps(2);
    let s = shapes_of(&h, "scratch");
    assert!(s.iter().all(|s| matches!(&s.geom, ics_core::Geometry::Polygon { points } if points.len() == 8)), "{s:?}");
    key(&mut h, egui::Modifiers::NONE, egui::Key::U);
    assert!(
        shapes_of(&h, "scratch").iter().all(|s| matches!(&s.geom, ics_core::Geometry::Polygon { points } if points.len() == 4))
    );
    key(&mut h, egui::Modifiers::SHIFT, egui::Key::U);
    let _ = r;
    shot(&mut h, "37_layout_split_chamfer");
    let _ = std::fs::remove_dir_all(&dir);
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
    let rulers = |h: &Harness<'_, IcStudio>| h.state().project().unwrap().cell(&r).unwrap().layout.as_ref().unwrap().rulers.len();
    assert_eq!(rulers(&h), 1, "the ruler is kept with the cell");
    key(&mut h, none, egui::Key::U);
    assert_eq!(rulers(&h), 0, "undo removes the ruler");
    key(&mut h, egui::Modifiers::SHIFT, egui::Key::U);
    assert_eq!(rulers(&h), 1, "redo brings it back");

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
    drag(&mut h, egui::PointerButton::Primary, pos(900.0, 205.0), pos(1100.0, 700.0));
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

/// A scripted model standing in for Gemini: answers each request with the next canned reply.
struct Scripted(std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>);

impl ics_studio::copilot::Llm for Scripted {
    fn generate(&mut self, _: &serde_json::Value) -> Result<serde_json::Value, String> {
        let mut q = self.0.lock().unwrap();
        if q.is_empty() { Err("script exhausted".into()) } else { Ok(q.remove(0)) }
    }
}

/// The AI Models window: provider, key, model, base URL and headers for the AI features.
#[test]
fn ai_models_window() {
    use egui_kittest::kittest::Queryable;
    let mut h = app(demo_args("rc_lowpass", "schematic", false));
    h.state_mut().command(Command::AiModels);
    h.run_steps(3);
    h.get_by_label("Base URL");
    h.get_by_label("+ Header").click();
    h.run_steps(2);
    shot(&mut h, "46_ai_models");
}

/// The copilot runs the model's tool calls on the open project (here: sizing R1 of the RC
/// filter for a 10 kHz bandwidth with the optimizer); the Optimizer window then re-runs it.
#[test]
fn copilot_optimizes_rc_filter() {
    use serde_json::json;
    let mut h = app(demo_args("rc_lowpass", "schematic", false));
    let call = json!({"candidates": [{"content": {"role": "model", "parts": [{"functionCall": {"name": "optimize", "args": {
        "cell": "rc_lowpass",
        "vars": [{"target": "R1.r", "min": "100", "max": "1meg", "log": true}],
        "specs": [{"name": "bw", "analysis": "ac", "expr": "bandwidth(v(out))", "min": "9k", "max": "11k"}],
        "apply": true
    }}}]}}]});
    let answer = json!({"candidates": [{"content": {"role": "model", "parts": [{"text": "R1 is sized for 10 kHz."}]}}]});
    let script = std::sync::Arc::new(std::sync::Mutex::new(vec![call, answer]));
    let s2 = script.clone();
    let cp = h.state_mut().copilot_mut();
    cp.set_llm(Box::new(move |_, _| Ok(Box::new(Scripted(s2.clone())))));
    h.state_mut().command(Command::CopilotAsk("Size R1 for a 10 kHz bandwidth".into()));
    h.run_steps(2);
    settle(&mut h, "copilot");
    let log = h.state_mut().copilot_mut().log.clone();
    use ics_studio::copilot::Entry;
    assert!(
        log.iter().any(|e| matches!(e, Entry::Tool { name, error: false, result, .. } if name == "optimize" && result.contains("\"passed\":true"))),
        "{log:?}"
    );
    assert_eq!(log.last(), Some(&Entry::Assistant("R1 is sized for 10 kHz.".into())));
    let r = h.state().active_ref().cloned().unwrap();
    let rv = {
        let p = h.state().project().unwrap();
        let sch = p.cell(&r).unwrap().schematic.as_ref().unwrap();
        sch.instances.iter().find(|i| i.name == "R1").unwrap().param("r").to_string()
    };
    let ohms = ics_core::units::parse_spice(&rv).unwrap();
    assert!((14e3..18e3).contains(&ohms), "R1 = {rv}");
    shot(&mut h, "44_copilot");

    h.state_mut().command(Command::Optimizer);
    h.state_mut().command(Command::RunOptimizer);
    h.run_steps(2);
    settle(&mut h, "optimizer");
    let opt = h.state_mut().optimizer();
    let best = opt.best().expect("evaluated");
    assert!(opt.evals[best].passed);
    shot(&mut h, "45_optimizer");
}

/// A usage question: the copilot searches and reads the user manual (also in plan mode, as
/// the docs tools only read) and the system prompt tells it to answer from the manual.
#[test]
fn copilot_answers_from_the_manual() {
    use ics_studio::copilot::Entry;
    use serde_json::json;
    let mut h = app(demo_args("cmos_inverter", "layout", false));
    let step = |name: &str, args: serde_json::Value| json!({"candidates": [{"content": {"role": "model", "parts": [{"functionCall": {"name": name, "args": args}}]}}]});
    let answer =
        json!({"candidates": [{"content": {"role": "model", "parts": [{"text": "**File > Import > GDSII / OASIS...**"}]}}]});
    let script = std::sync::Arc::new(std::sync::Mutex::new(vec![
        step("docs_search", json!({"query": "GDS 파일 가져오기"})),
        step("docs_read", json!({"page": "ko/layout", "section": "GDSII"})),
        answer,
    ]));
    let seen = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let (s2, seen2) = (script.clone(), seen.clone());
    let cp = h.state_mut().copilot_mut();
    cp.plan_mode = true;
    cp.set_llm(Box::new(move |_, _| Ok(Box::new(Recorded { script: s2.clone(), seen: seen2.clone() }))));
    h.state_mut().command(Command::CopilotAsk("GDS 파일은 어떻게 가져오나요?".into()));
    h.run_steps(2);
    settle(&mut h, "copilot");
    let log = h.state_mut().copilot_mut().log.clone();
    let tool = |n: &str| {
        log.iter().find_map(|e| match e {
            Entry::Tool { name, result, error: false, .. } if name == n => Some(result.clone()),
            _ => None,
        })
    };
    let found = tool("docs_search").unwrap_or_else(|| panic!("{log:?}"));
    assert!(found.contains("ko/"), "{found}");
    let read = tool("docs_read").unwrap_or_else(|| panic!("{log:?}"));
    assert!(read.contains("GDSII"), "{read}");
    let req = seen.lock().unwrap()[0].to_string();
    assert!(req.contains("answer from the user") && req.contains("docs_search"), "system prompt");
}

/// A scripted model that also records the requests it gets.
struct Recorded {
    script: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    seen: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
}

impl ics_studio::copilot::Llm for Recorded {
    fn generate(&mut self, req: &serde_json::Value) -> Result<serde_json::Value, String> {
        self.seen.lock().unwrap().push(req.clone());
        let mut q = self.script.lock().unwrap();
        if q.is_empty() { Err("script exhausted".into()) } else { Ok(q.remove(0)) }
    }
}

/// Plan mode: read-only tools run, edits are refused, the plan waits for approval; once approved
/// the agent keeps a task list and pauses on a question until the user answers.
#[test]
fn copilot_plan_mode_agent() {
    use ics_studio::copilot::{Entry, TodoStatus, WaitKind};
    use serde_json::json;
    let calls = |c: serde_json::Value| json!({"candidates": [{"content": {"role": "model", "parts": c}}]});
    let script = vec![
        calls(json!([{"text": "Looking at the project first."}, {"functionCall": {"name": "project_info", "args": {}}},
            {"functionCall": {"name": "route", "args": {"cell": "nand2"}}}])),
        calls(json!([{"functionCall": {"name": "exit_plan_mode", "args": {"plan": "1. Place\n2. Route\n3. DRC"}}}])),
        calls(json!([{"functionCall": {"name": "update_todos", "args": {"todos": [
            {"content": "Place devices", "status": "completed"}, {"content": "Route", "status": "in_progress"}]}}},
            {"functionCall": {"name": "ask_user", "args": {"question": "Which metal?", "options": ["M1", "M2"]}}}])),
        calls(json!([{"text": "Done with M2."}])),
    ];
    let script = std::sync::Arc::new(std::sync::Mutex::new(script));
    let seen = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let mut h = app(demo_args("nand2", "layout", false));
    let (s2, r2) = (script.clone(), seen.clone());
    let cp = h.state_mut().copilot_mut();
    cp.plan_mode = true;
    cp.set_llm(Box::new(move |_, _| Ok(Box::new(Recorded { script: s2.clone(), seen: r2.clone() }))));
    h.state_mut().command(Command::CopilotAsk("Lay out nand2".into()));
    h.run_steps(2);
    settle(&mut h, "plan");
    let cp = h.state_mut().copilot_mut();
    assert_eq!(cp.waiting(), Some(WaitKind::Approval), "{:?}", cp.log);
    assert!(cp.log.iter().any(|e| matches!(e, Entry::Tool { name, error: false, .. } if name == "project_info")));
    assert!(!cp.log.iter().any(|e| matches!(e, Entry::Tool { name, .. } if name == "route")), "route ran in plan mode");
    assert!(matches!(cp.log.last(), Some(Entry::Plan(p)) if p.contains("Route")));
    shot(&mut h, "48_copilot_plan");

    h.state_mut().command(Command::CopilotApprove);
    h.run_steps(2);
    settle(&mut h, "approve");
    let cp = h.state_mut().copilot_mut();
    assert!(!cp.plan_mode);
    assert_eq!(cp.waiting(), Some(WaitKind::Answer));
    assert_eq!(cp.todos.len(), 2);
    assert_eq!(cp.todos[1].status, TodoStatus::InProgress);
    shot(&mut h, "49_copilot_question");

    h.state_mut().command(Command::CopilotAsk("M2".into()));
    h.run_steps(2);
    settle(&mut h, "answer");
    let cp = h.state_mut().copilot_mut();
    assert_eq!(cp.waiting(), None);
    assert_eq!(cp.log.last(), Some(&Entry::Assistant("Done with M2.".into())));
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 4);
    let last_user = |i: usize| seen[i]["contents"].as_array().unwrap().last().unwrap()["parts"].clone();
    assert!(last_user(1).to_string().contains("plan mode is on"), "{}", last_user(1));
    assert_eq!(last_user(2)[0]["functionResponse"]["response"]["approved"], true);
    let answer = last_user(3);
    assert_eq!(answer[0]["functionResponse"]["name"], "update_todos");
    assert_eq!(answer[1]["functionResponse"]["response"]["answer"], "M2");
    assert!(seen[0]["systemInstruction"].to_string().contains("PLAN MODE is on"));
    assert!(!seen[2]["systemInstruction"].to_string().contains("PLAN MODE is on"));
}

/// A model that streams its answer in two halves, holding the second until released.
struct Streamy {
    release: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

const STREAMED: &str =
    "The **netlist** of `rc_lowpass`:\n\n```spice\nR1 in out 1k\nC1 out 0 10n\n```\nBandwidth is about 16 kHz.";

impl ics_studio::copilot::Llm for Streamy {
    fn generate(&mut self, _: &serde_json::Value) -> Result<serde_json::Value, String> {
        unreachable!("the copilot streams")
    }

    fn stream(&mut self, _: &serde_json::Value, on_text: &mut dyn FnMut(&str) -> bool) -> Result<serde_json::Value, String> {
        let (a, b) = STREAMED.split_at(60);
        on_text(a);
        while !self.release.load(std::sync::atomic::Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(5));
        }
        on_text(b);
        Ok(serde_json::json!({"candidates": [{"content": {"role": "model", "parts": [{"text": STREAMED}]}}]}))
    }
}

/// The copilot chat: the answer shows while it streams, code blocks get a copy button, and a
/// long message keeps the input box and Send button inside the window.
#[test]
fn copilot_chat_streams_and_keeps_input_visible() {
    use egui_kittest::kittest::Queryable;
    use ics_studio::copilot::Entry;
    let mut h = app(demo_args("rc_lowpass", "schematic", false));
    let release = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let r2 = release.clone();
    h.state_mut().copilot_mut().set_llm(Box::new(move |_, _| Ok(Box::new(Streamy { release: r2.clone() }))));
    h.state_mut().command(Command::CopilotAsk("Show the netlist".into()));
    let t = std::time::Instant::now();
    while h.state_mut().copilot_mut().streaming.is_empty() && t.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(5));
        h.step();
    }
    h.run_steps(2);
    let cp = h.state_mut().copilot_mut();
    assert!(cp.busy());
    assert_eq!(cp.streaming, &STREAMED[..60]);
    assert!(!cp.log.iter().any(|e| matches!(e, Entry::Assistant(_))));
    shot(&mut h, "50_copilot_streaming");
    release.store(true, std::sync::atomic::Ordering::Relaxed);
    settle(&mut h, "stream");
    let cp = h.state_mut().copilot_mut();
    assert!(cp.streaming.is_empty());
    assert_eq!(cp.log.last(), Some(&Entry::Assistant(STREAMED.into())));
    // One copy button per message (question, answer) and one on the code block, plus "Copy all".
    assert!(h.get_all_by_label("Copy").count() >= 3);
    h.get_by_label("Copy all");

    h.state_mut().copilot_mut().input = (1..=40).map(|k| format!("line {k} of a long request")).collect::<Vec<_>>().join("\n");
    h.run_steps(3);
    let send = h.get_by_label("Send").rect();
    let input = h.get_by_label("Ask").rect();
    // The 640 px window sits centred in the 900 px screen: everything stays above its bottom edge.
    assert!(send.max.y < 780.0, "Send {send:?} pushed out");
    // The box scrolls inside its own area, so its top stays on screen above Send.
    assert!(input.min.y < send.min.y && send.min.y - input.min.y < 200.0, "input {input:?} grew past the window");
    shot(&mut h, "51_copilot_chat");

    // Dragging a text selection above the log scrolls it up.
    for k in 0..40 {
        h.state_mut().copilot_mut().log.push(Entry::Notice(format!("note {k}")));
    }
    h.run_steps(3);
    let before = h.get_by_label("note 30").rect();
    let at = before.center();
    let pos = |p: egui::Pos2| egui::Event::PointerMoved(p);
    let button = |p: egui::Pos2, pressed: bool| egui::Event::PointerButton {
        pos: p,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    h.event(pos(at));
    h.event(button(at, true));
    h.step();
    let above = egui::pos2(at.x, 60.0);
    for _ in 0..20 {
        h.event(pos(above));
        h.step();
    }
    h.event(button(above, false));
    h.run_steps(2);
    let after = h.get_by_label("note 30").rect();
    assert!(after.min.y > before.min.y + 50.0, "no autoscroll: {before:?} -> {after:?}");
}

/// Layout by example: a renamed copy of the NAND2 circuit is placed like the demo NAND2 layout
/// and routed, leaving no flylines.
#[test]
fn layout_by_example() {
    use ics_studio::gen_view::GenAction;
    let dir = std::env::temp_dir().join(format!("ics-ui-example-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut p = ics_core::examples::demo_project();
    let reference = ics_core::CellRef::new("work", "nand2");
    let mut copy = p.cell(&reference).unwrap().clone();
    copy.name = "nand2_b".into();
    copy.layout = Some(Default::default());
    for i in &mut copy.schematic.as_mut().unwrap().instances {
        if matches!(i.kind, ics_core::schematic::DeviceKind::Nmos | ics_core::schematic::DeviceKind::Pmos) {
            i.name = format!("Q{}", i.name);
        }
    }
    p.libraries.iter_mut().find(|l| l.name == "work").unwrap().cells.push(copy);
    p.save_as(&dir).unwrap();
    let args = LaunchArgs {
        project: Some(dir.clone()),
        demo: false,
        workspace: Some("layout".into()),
        cell: Some("nand2_b".into()),
        run: false,
    };
    let mut h = app(args);
    h.state_mut().apply_gen(GenAction::PlaceLike { reference: reference.clone(), route: true });
    h.run_steps(3);
    let (n_ref, n_new) = {
        let p = h.state().project().unwrap();
        let count = |c: &str| p.cell(&ics_core::CellRef::new("work", c)).unwrap().layout.as_ref().unwrap().instances.len();
        (count("nand2"), count("nand2_b"))
    };
    assert_eq!(n_new, n_ref, "{:?}", h.state().console_lines().collect::<Vec<_>>());
    assert_eq!(flylines(&h, "nand2_b"), 0, "{:?}", h.state().console_lines().collect::<Vec<_>>());
    shot(&mut h, "46_layout_by_example");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The copilot against the real Gemini API (needs GEMINI_API_KEY in the environment or a .env):
/// `cargo test -p ics-studio --test ui copilot_live -- --ignored --nocapture`.
#[test]
#[ignore]
fn copilot_live() {
    let dir = demo_without_layout("copilot-live", "nand2");
    let args =
        LaunchArgs { project: Some(dir), demo: false, workspace: Some("layout".into()), cell: Some("nand2".into()), run: false };
    let mut h = app(args);
    let custom = std::env::var("ICS_COPILOT_PROMPT").ok();
    let q = custom.clone().unwrap_or_else(|| {
        "Build the layout of nand2 from its schematic: place the devices, route all flylines, run DRC and fix any \
         violations, then run LVS. Report the result in two sentences."
            .into()
    });
    h.state_mut().command(Command::CopilotAsk(q));
    let t = std::time::Instant::now();
    h.run_steps(2);
    while h.state().is_busy() && t.elapsed() < Duration::from_secs(300) {
        std::thread::sleep(Duration::from_millis(20));
        h.step();
    }
    h.run_steps(3);
    use ics_studio::copilot::Entry;
    let log = h.state_mut().copilot_mut().log.clone();
    for e in &log {
        match e {
            Entry::Tool { name, args, result, error } => {
                let r: String = result.chars().take(300).collect();
                eprintln!("TOOL {name} {args}\n  -> {}{r}", if *error { "ERROR " } else { "" });
            }
            other => eprintln!("{other:?}"),
        }
    }
    eprintln!("took {:.1} s", t.elapsed().as_secs_f64());
    shot(&mut h, "47_copilot_live");
    assert!(!log.iter().any(|e| matches!(e, Entry::Error(_))), "copilot error");
    assert!(matches!(log.last(), Some(Entry::Assistant(_) | Entry::Question { .. })), "no final answer");
    if custom.is_none() {
        assert_eq!(flylines(&h, "nand2"), 0, "unrouted flylines left");
    }
}

#[test]
fn update_toast() {
    use egui_kittest::kittest::Queryable;
    let mut h = app(LaunchArgs { project: None, demo: true, workspace: Some("home".into()), cell: None, run: false });
    h.state_mut().announce_update(ics_studio::update::Release {
        version: "9.9.9".into(),
        page: String::new(),
        notes: "Test release".into(),
        url: None,
        size: 0,
        tools: vec![],
        checksums: Default::default(),
    });
    h.run_steps(4);
    h.get_by_label("Update now");
    shot(&mut h, "update_toast");
    h.get_by_label("Later").click();
    h.run_steps(2);
    assert!(h.query_by_label("Update now").is_none());
}

#[test]
fn tutorial_walkthrough() {
    use egui_kittest::kittest::Queryable;
    let mut h = app(LaunchArgs { project: None, demo: false, workspace: None, cell: None, run: false });
    h.state_mut().command(Command::Tutorial);
    h.run_steps(3);
    shot(&mut h, "tutorial_01_start");
    assert_eq!(h.state().tutorial().step, 0);
    let expect = |h: &mut Harness<'_, IcStudio>, c: Option<Command>, step: usize| {
        if let Some(c) = c {
            h.state_mut().command(c);
        }
        h.run_steps(3);
        settle(h, "tutorial");
        assert_eq!(h.state().tutorial().step, step, "tutorial did not advance to step {step}");
    };
    expect(&mut h, Some(Command::NewDemoProject), 1);
    let inv = h.state().project().unwrap().all_cells().find(|(_, c)| c.name == "cmos_inverter").unwrap().0;
    expect(&mut h, Some(Command::OpenCell(inv.clone(), Some(Workspace::Schematic))), 2);
    shot(&mut h, "tutorial_02_schematic");
    h.get_by_label("Skip").click();
    expect(&mut h, None, 3);
    h.get_by_label("Show me").click();
    expect(&mut h, None, 4);
    expect(&mut h, Some(Command::Calculator), 5);
    expect(&mut h, Some(Command::OpenCell(inv.clone(), Some(Workspace::Layout))), 6);
    expect(&mut h, Some(Command::RunDrc), 7);
    expect(&mut h, Some(Command::RunLvs), 8);
    expect(&mut h, Some(Command::CommandPalette), 9);
    assert!(h.state().tutorial().finished());
    assert_eq!(h.state().tutorial().completed(), 8);
    h.state_mut().command(Command::SetWorkspace(Workspace::Home));
    h.run_steps(3);
    shot(&mut h, "tutorial_03_done");
}

/// Requesting focus every frame interrupts IME composition every frame, which on Windows
/// toggles the IME and floods the event loop until the app hangs.
#[test]
fn text_prompts_do_not_interrupt_ime_every_frame() {
    let mut h = app(LaunchArgs { project: None, demo: true, workspace: None, cell: None, run: false });
    for c in [Command::CommandPalette, Command::NewCellDialog] {
        h.state_mut().command(c);
        h.run_steps(4);
        for _ in 0..5 {
            h.step();
            let ime = h.output().platform_output.ime.as_ref().expect("a text field has focus");
            assert!(!ime.should_interrupt_composition, "IME interrupted on a steady frame");
        }
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
    }
}

/// Silicon correlation: the example NMOS Id-Vd data against an imported characterisation cell,
/// compared and then fitted.
#[test]
fn silicon_correlation() {
    let mut h = app(demo_args("rc_lowpass", "simulation", false));
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../silicon");
    let net = std::fs::read_to_string(format!("{root}/nmos_iv.sp")).unwrap();
    {
        let p = h.state_mut().project_mut().unwrap();
        let imp = ics_core::spice_import::import(&net, &p.tech).unwrap();
        let r = p.add_cell("work", ics_core::Cell::new("nmos_iv")).unwrap();
        let c = p.cell_mut(&r).unwrap();
        imp.apply_to(&mut c.sim);
        c.schematic = Some(imp.schematic);
        use ics_core::silicon::{FitParam, SiliconSetup, Trace};
        c.sim.silicon = SiliconSetup {
            data: format!("{root}/nmos_idvd.csv"),
            analysis: "dc".into(),
            x: "vd".into(),
            traces: ["0.7", "0.9", "1.2"]
                .iter()
                .map(|v| Trace { column: format!("id_vg{v}"), expr: "-i(vd)".into(), bias: format!("VG.dc={v}"), log: false })
                .collect(),
            params: vec![
                FitParam { target: "model:nmos.vto".into(), min: "0.3".into(), max: "0.7".into(), log: false },
                FitParam { target: "model:nmos.kp".into(), min: "100u".into(), max: "500u".into(), log: true },
                FitParam { target: "model:nmos.lambda".into(), min: "0.01".into(), max: "0.3".into(), log: true },
            ],
        };
    }
    h.state_mut().command(Command::OpenCell(ics_core::CellRef::new("work", "nmos_iv"), None));
    h.run_steps(2);
    h.state_mut().command(Command::SiliconCorrelation);
    h.run_steps(2);
    assert_eq!(h.state_mut().silicon().data.as_ref().map(|d| d.columns.len()), Some(4));
    h.state_mut().command(Command::CompareSilicon);
    h.run_steps(2);
    settle(&mut h, "compare");
    let err = h.state_mut().silicon().error.clone();
    let start = h.state_mut().silicon().outcome.as_ref().unwrap_or_else(|| panic!("{err:?}")).start.error();
    assert!(start > 0.2, "{start}");
    h.state_mut().command(Command::FitSilicon);
    h.run_steps(2);
    settle(&mut h, "fit");
    let o = h.state_mut().silicon().outcome.clone().unwrap();
    let best = o.best.as_ref().unwrap();
    assert!(best.error() < 0.01, "{}", best.error());
    assert!((best.values[0] - 0.52).abs() < 0.01, "{:?}", best.values);
    assert!(o.cards.starts_with(".model nmos nmos"), "{}", o.cards);
    h.run_steps(3);
    shot(&mut h, "38_silicon_correlation");
}

/// Process emulation: the inverter's cross-section with the generic flow, and after the gate etch.
#[test]
fn process_emulation() {
    let mut h = app(demo_args("cmos_inverter", "layout", false));
    h.state_mut().command(Command::ProcessEmulation);
    h.run_steps(3);
    let pe = h.state_mut().process_emulation();
    let s = pe.section.as_ref().unwrap_or_else(|| panic!("{:?}", pe.error));
    for m in ["si", "gox", "poly", "w", "al"] {
        assert!(s.materials.iter().any(|x| x == m), "{m}: {:?}", s.materials);
    }
    // Doped: the PMOS source / drain junction in the n-well and the well junction.
    assert!(s.dopants.len() == 3, "{:?}", s.dopants.iter().map(|d| d.0).collect::<Vec<_>>());
    let w = s.nx as f64 * s.h;
    assert!((1..40).any(|k| s.junctions(w * k as f64 / 40.0).len() == 2));
    h.run_steps(2);
    shot(&mut h, "39_process_emulation");
    let pe = h.state_mut().process_emulation();
    let gate = pe.flow.as_ref().unwrap().steps.iter().position(|s| s.label().starts_with("gate etch")).unwrap();
    pe.steps = gate + 1;
    pe.set_text(pe.text.clone());
    pe.steps = gate + 1;
    h.run_steps(3);
    let s = h.state_mut().process_emulation().section.clone().unwrap();
    assert!(!s.materials.iter().any(|m| m == "al"), "{:?}", s.materials);
}

/// Process data: the example WAT data against the built-in spec, with the shifted lot caught.
#[test]
fn process_data() {
    let mut h = app(demo_args("rc_lowpass", "simulation", false));
    h.state_mut().command(Command::ProcessData);
    h.run_steps(2);
    let csv = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../process/generic180_wat.csv")).unwrap();
    h.state_mut().process_data().set_data_text(&csv, "generic180_wat.csv");
    h.run_steps(3);
    let pd = h.state_mut().process_data();
    assert_eq!(pd.summary.len(), 8);
    let vtn = pd.summary.iter().find(|s| s.name == "vtn").unwrap();
    assert!(!vtn.violations.is_empty() && vtn.cpk.unwrap() > 1.33, "{vtn:?}");
    let idn = pd.summary.iter().position(|s| s.name == "idsat_n").unwrap();
    assert_eq!(pd.summary[idn].corners_cover, Some(false));
    pd.selected = 0;
    h.run_steps(3);
    shot(&mut h, "40_process_data");
    // Corner models from the data.
    let p = h.state().project().unwrap().clone();
    let ctx = h.ctx.clone();
    h.state_mut().process_data().start_corners(&ctx, p).unwrap();
    h.run_steps(2);
    settle(&mut h, "corners");
    let c = h.state_mut().process_data().corners.clone().unwrap();
    assert_eq!(c.fits.len(), 5);
    assert!(c.fits.iter().all(|f| f.error_sigma < 0.2), "{:?}", c.fits);
    assert!(c.library.contains(".lib ff"));
    h.run_steps(2);
    shot(&mut h, "41_corner_models");
    // The process variation goes to the technology, and Monte Carlo draws it.
    use egui_kittest::kittest::Queryable;
    let v = c.variation.clone().unwrap();
    assert_eq!(v.variation.params.len(), 4);
    h.get_by_label("Use for Monte Carlo").click();
    h.run_steps(3);
    assert_eq!(h.state().project().unwrap().tech.variation.as_ref(), Some(&v.variation));
    h.state_mut().command(Command::MonteCarlo);
    h.run_steps(3);
    assert!(h.state_mut().monte_carlo().spec().unwrap().process.is_some());
    assert!(h.query_by_label_contains("Global variation: measured").is_some());
    // Mismatch from the example matched pairs, into the technology and Monte Carlo.
    h.state_mut().monte_carlo().open = false;
    h.state_mut().process_data().set_pairs_text(ics_core::mismatch::GENERIC180_PAIRS, "pairs");
    h.run_steps(3);
    assert_eq!(h.state_mut().process_data().mismatch.len(), 2);
    h.get_by_label("Use mismatch for Monte Carlo").scroll_to_me();
    h.run_steps(3);
    shot(&mut h, "42_mismatch");
    h.get_by_label("Use mismatch for Monte Carlo").click();
    h.run_steps(3);
    assert!(h.state().project().unwrap().tech.mismatch.is_some());
    h.state_mut().command(Command::MonteCarlo);
    h.run_steps(3);
    assert!(h.state_mut().monte_carlo().spec().unwrap().mismatch.is_some());
    assert!(h.query_by_label_contains("Mismatch: measured").is_some());
}

/// Virtuoso interchange: a hierarchical CDL netlist becomes a cell's schematic.
#[test]
fn import_cdl_netlist() {
    let mut h = app(demo_args("cmos_inverter", "schematic", false));
    let path = std::env::temp_dir().join(format!("ics_ui_test_{}.cdl", std::process::id()));
    std::fs::write(
        &path,
        ".SUBCKT inv in out\n*.PININFO in:I out:O\nMM1 out in vdd! vdd! pch W=2u L=180n\nMM0 out in gnd! gnd! nch W=1u L=180n\n.ENDS\n\
         .SUBCKT buf a y\nXI0 a mid / inv\nXI1 mid y / inv\n.ENDS\n",
    )
    .unwrap();
    h.state_mut().command(Command::ImportCdl(Some(path.clone())));
    h.run_steps(4);
    let _ = std::fs::remove_file(&path);
    let p = h.state().project().unwrap();
    // The hierarchy is kept, in a library named after the file: buf places two inv cells.
    let lib = path.file_stem().unwrap().to_string_lossy().to_string();
    let buf = CellRef::new(&lib, "buf");
    let c = p.cell(&buf).expect("imported cell");
    let xs = c.schematic.as_ref().unwrap().instances.iter().filter(|i| i.kind == ics_core::DeviceKind::Cell).count();
    assert_eq!(xs, 2);
    assert!(p.cell(&CellRef::new(&lib, "inv")).is_some_and(|c| c.schematic.is_some()));
    let flat = p.circuit(&buf).unwrap();
    assert_eq!(flat.instances.iter().filter(|i| i.kind.is_mos()).count(), 4, "two inverters of two transistors");
}

/// Right-clicking a library in the Explorer selects it and offers library-level actions:
/// export, storage, rename and delete.
#[test]
fn explorer_library_context_menu() {
    use egui_kittest::kittest::Queryable;
    let mut h = app(demo_args("cmos_inverter", "layout", false));
    let libs =
        |h: &Harness<'_, IcStudio>| h.state().project().unwrap().libraries.iter().map(|l| l.name.clone()).collect::<Vec<_>>();
    assert!(libs(&h).contains(&"work".to_string()));
    h.get_by_role_and_label(egui::accesskit::Role::Button, "work").click_secondary();
    h.run_steps(3);
    for item in ["New cell…", "Open top cell layout", "Export GDSII…", "Export OASIS…", "Store as one GDSII file", "Rename…"]
    {
        assert!(h.query_by_label(item).is_some(), "context menu has '{item}'");
    }
    shot(&mut h, "explorer_library_menu");

    h.get_by_label("Store as one GDSII file").click();
    h.run_steps(3);
    let gds =
        |h: &Harness<'_, IcStudio>| h.state().project().unwrap().libraries.iter().find(|l| l.name == "work").map(|l| l.storage);
    assert_eq!(gds(&h), Some(ics_core::project::LibStorage::Gds));

    h.get_by_role_and_label(egui::accesskit::Role::Button, "work").click_secondary();
    h.run_steps(3);
    h.get_by_label("Delete…").click();
    h.run_steps(3);
    assert!(h.query_by_label("Delete library?").is_some());
    h.get_by_label("Delete").click();
    h.run_steps(3);
    assert!(!libs(&h).contains(&"work".to_string()));
    assert!(h.state().active_ref().is_none_or(|r| r.lib != "work"), "tabs of the deleted library are closed");

    // The command palette offers the same actions for the selected library.
    h.get_by_role_and_label(egui::accesskit::Role::Button, "pcells").click();
    h.state_mut().command(Command::CommandPalette);
    h.run_steps(2);
    h.event(egui::Event::Text("library export".into()));
    h.run_steps(3);
    assert!(h.query_by_label_contains("Library pcells: export GDSII").is_some(), "palette targets the selected library");
    assert!(h.query_by_label_contains("Library work:").is_none());
    shot(&mut h, "explorer_library_palette");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
}

fn right_click(h: &mut Harness<'_, IcStudio>, p: egui::Pos2) {
    h.hover_at(p);
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: Default::default(),
        });
        h.step();
    }
    h.run_steps(2);
}

/// The schematic canvas pop-up menu: tools on empty space, edits on a selection.
#[test]
fn schematic_context_menu() {
    use egui_kittest::kittest::Queryable;
    let mut h = app(demo_args("cmos_inverter", "schematic", false));
    h.run_steps(4);
    let r = h.state().active_ref().unwrap().clone();
    let count = |h: &Harness<'_, IcStudio>| {
        let p = h.state().project().unwrap();
        p.cell(&r).unwrap().schematic.as_ref().unwrap().instances.len()
    };
    let before = count(&h);
    // Far corner of the canvas: empty space.
    let empty = egui::pos2(650.0, 650.0);
    right_click(&mut h, empty);
    shot(&mut h, "sch_context_empty");
    assert!(h.query_by_label("Place Device").is_some(), "empty-space menu should offer the tools");
    h.get_by_label_contains("Select All").click();
    h.run_steps(3);
    assert!(!h.state().schematic_editor(&r).unwrap().selection.is_empty());
    right_click(&mut h, empty);
    assert!(h.query_by_label_contains("Rotate 90°").is_some(), "selection menu should offer edits");
    shot(&mut h, "sch_context_selection");
    // The menu item, not the toolbar's "Duplicate selection" button above it.
    h.get_all_by_label_contains("Duplicate").max_by(|a, b| a.rect().top().total_cmp(&b.rect().top())).unwrap().click();
    h.run_steps(3);
    assert_eq!(count(&h), before * 2, "duplicate should copy every instance");
}

/// Explorer: libraries of generated PCells start folded; the filter lists matching cells only.
#[test]
fn explorer_filter() {
    use egui_kittest::kittest::Queryable;
    let mut h = app(demo_args("cmos_inverter", "schematic", false));
    h.run_steps(4);
    assert!(h.query_by_label("nmos_w1000_l180_nf1").is_none(), "pcells library should start folded");
    assert!(h.query_by_label("ring_osc5").is_some());
    let f = h.get_by_label("Filter");
    f.focus();
    f.type_text("inv_");
    h.run_steps(3);
    assert!(h.query_by_label("inv_chain").is_some());
    assert!(h.query_by_label("ring_osc5").is_none(), "non-matching cells are hidden");
    shot(&mut h, "explorer_filter");
}

/// Cmd+D deselects all in the schematic, as in the layout; Shift+Cmd+D duplicates.
#[test]
fn schematic_cmd_d_matches_layout() {
    let mut h = app(demo_args("cmos_inverter", "schematic", false));
    h.run_steps(4);
    let r = h.state().active_ref().unwrap().clone();
    let count = |h: &Harness<'_, IcStudio>| {
        let p = h.state().project().unwrap();
        p.cell(&r).unwrap().schematic.as_ref().unwrap().instances.len()
    };
    let before = count(&h);
    // Keys reach the canvas while the pointer is over it (empty space).
    h.hover_at(egui::pos2(650.0, 650.0));
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(2);
    assert!(!h.state().schematic_editor(&r).unwrap().selection.is_empty());
    h.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::D);
    h.run_steps(2);
    assert_eq!(count(&h), before * 2, "Shift+Cmd+D should duplicate the selection");
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::D);
    h.run_steps(2);
    assert_eq!(count(&h), before * 2, "Cmd+D must not duplicate");
    assert!(h.state().schematic_editor(&r).unwrap().selection.is_empty(), "Cmd+D should deselect all");
}

/// Tab strip: Close Other Tabs from the command, the same one the tab menu sends.
#[test]
fn close_other_tabs() {
    let mut h = app(demo_args("cmos_inverter", "schematic", false));
    let r = h.state().active_ref().unwrap().clone();
    h.state_mut().command(Command::CloseOtherTabs(r.clone()));
    h.step();
    assert_eq!(h.state().active_ref(), Some(&r));
    h.state_mut().command(Command::CloseAllTabs);
    h.step();
    assert!(h.state().active_ref().is_none());
}

/// The layout canvas pop-up menu: Create on empty space, edits once something is selected.
#[test]
fn layout_context_menu() {
    use egui_kittest::kittest::Queryable;
    use ics_studio::layout_view::LayAction;
    let mut h = app(demo_args("cmos_inverter", "layout", false));
    h.run_steps(4);
    let p = egui::pos2(580.0, 450.0);
    right_click(&mut h, p);
    shot(&mut h, "lay_context_empty");
    assert_eq!(h.query_all_by_label("Create").count(), 2, "empty-space menu should offer Create (besides the menu bar)");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    h.state_mut().command(Command::Lay(LayAction::SelectAll));
    h.run_steps(3);
    right_click(&mut h, p);
    shot(&mut h, "lay_context_selection");
    assert!(h.query_by_label_contains("shapes selected").is_some(), "selection menu should show a summary");
    assert!(h.query_by_label("Modify Shapes").is_some());
}

/// Stretch on a PCell instance changes its parameters: the top edge of a MOS sets W, its
/// right edge L, and the opposite edge stays put.
#[test]
fn layout_stretch_pcell() {
    use ics_core::pcell::PCell;
    let dir = std::env::temp_dir().join(format!("ics-ui-stretch-pcell-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut p = ics_core::examples::demo_project();
    let nmos = PCell::mos(false, 2000, 180, 1);
    let cr = p.ensure_pcell(&nmos).unwrap();
    let mut l = ics_core::Layout::default();
    l.add_instance(cr, ics_core::layout::Trans::new(0, 0), None);
    let mut cell = ics_core::Cell::new("scratch");
    cell.layout = Some(l);
    p.add_cell("work", cell).unwrap();
    p.save_as(&dir).unwrap();
    let mut h = open_layout(&dir, "scratch");
    let key = |h: &mut Harness<'_, IcStudio>, k: egui::Key| {
        h.key_press_modifiers(egui::Modifiers::NONE, k);
        h.run_steps(2);
    };
    let tech = h.state().project().unwrap().tech.clone();
    let inst = |h: &Harness<'_, IcStudio>| {
        let p = h.state().project().unwrap();
        let i = p.cell(&ics_core::CellRef::new("work", "scratch")).unwrap().layout.as_ref().unwrap().instances[0].clone();
        let pc = p.cell(&i.cell).unwrap().pcell.clone().unwrap();
        let b = i.trans.apply_rect(pc.generate(&p.tech).unwrap().shapes_bbox().unwrap());
        (pc, b)
    };
    // Calibrate screen <-> world with a rectangle drawn away from the device, then undo it.
    h.hover_at(egui::pos2(700.0, 450.0));
    h.run_steps(2);
    key(&mut h, egui::Key::G);
    key(&mut h, egui::Key::R);
    let (s0, s1) = (egui::pos2(530.0, 160.0), egui::pos2(650.0, 280.0));
    click(&mut h, s0);
    click(&mut h, s1);
    let r = shapes_of(&h, "scratch")[0].bbox();
    let k = r.width() as f32 / (s1.x - s0.x);
    let to_screen = |x: i64, y: i64| egui::pos2(s0.x + (x - r.x0) as f32 / k, s0.y - (y - r.y1) as f32 / k);
    key(&mut h, egui::Key::U);
    assert!(shapes_of(&h, "scratch").is_empty());
    key(&mut h, egui::Key::Escape);

    let (pc0, b0) = inst(&h);
    assert_eq!(pc0, nmos);
    let top = to_screen((b0.x0 + b0.x1) / 2, b0.y1);
    key(&mut h, egui::Key::S);
    click(&mut h, top);
    click(&mut h, top - egui::vec2(0.0, 40.0));
    let (pc1, b1) = inst(&h);
    let PCell::Mos { w, l, .. } = pc1.clone() else { panic!() };
    let want = (40.0 * k) as i64;
    assert!((w - 2000 - want).abs() <= 2 * tech.grid.max(10), "W grows by the drag: {pc1:?}, want +{want}");
    assert_eq!(l, 180);
    assert_eq!(b1.y0, b0.y0, "the bottom edge stays: {b0:?} -> {b1:?}");

    let right = to_screen(b1.x1, (b1.y0 + b1.y1) / 2);
    key(&mut h, egui::Key::S);
    click(&mut h, right);
    click(&mut h, right + egui::vec2(8.0, 0.0));
    let (pc2, b2) = inst(&h);
    let PCell::Mos { w: w2, l: l2, .. } = pc2.clone() else { panic!() };
    assert_eq!(w2, w);
    assert!(l2 > 180, "L grows: {pc2:?}");
    assert_eq!(b2.x0, b1.x0, "the left edge stays: {b1:?} -> {b2:?}");
    key(&mut h, egui::Key::U);
    assert_eq!(inst(&h).0, pc1, "u undoes the stretch");
}

#[test]
fn demo_memory_schematics() {
    for cell in ["sram6t", "dram1t1c", "nand_string", "aps4t"] {
        scenario(&format!("demo_{cell}_schematic"), true, Some(cell), "schematic", false);
    }
}

/// Layout XL ECO: a device widened in the schematic after layout is reported by Check Against
/// Source, Update Components and Nets resizes it in place, and the check is clean again.
#[test]
fn layout_check_against_source() {
    let args = LaunchArgs {
        project: None,
        demo: true,
        workspace: Some("schematic".into()),
        cell: Some("cmos_inverter".into()),
        run: false,
    };
    let mut h = app(args);
    h.state_mut().command(Command::GenerateLayout);
    h.state_mut().command(Command::RouteFlylines);
    h.run_steps(2);
    let r = ics_core::CellRef::new("work", "cmos_inverter");
    let sch = h.state_mut().project_mut().unwrap().cell_mut(&r).unwrap().schematic.as_mut().unwrap();
    let m = sch.instances.iter_mut().find(|i| i.kind.is_mos()).unwrap();
    m.params.insert("w".into(), "4u".into());
    h.state_mut().command(Command::CheckAgainstSource);
    h.run_steps(2);
    let src = h.state().layout_editor(&r).and_then(|e| e.verify.source.clone()).expect("check result");
    assert_eq!(src.count("parameters"), 1, "{:?}", src.issues);
    settle(&mut h, "layout_check_against_source");
    shot(&mut h, "layout_check_against_source");
    h.state_mut().command(Command::UpdateFromSource(false));
    h.run_steps(2);
    let src = h.state().layout_editor(&r).and_then(|e| e.verify.source.clone()).expect("check result");
    assert_eq!(src.count("parameters") + src.count("missing") + src.count("extra"), 0, "{:?}", src.issues);
    h.state_mut().command(Command::Lay(ics_studio::layout_view::LayAction::Search));
    h.run_steps(4);
    shot(&mut h, "layout_search");
}

/// Routing tracks drawn for the active layer with the Tracks form open, and schematic pins.
#[test]
fn layout_tracks_and_schematic_pins() {
    use ics_studio::layout_view::LayAction;
    let args = LaunchArgs {
        project: None,
        demo: true,
        workspace: Some("layout".into()),
        cell: Some("cmos_inverter".into()),
        run: false,
    };
    let mut h = app(args);
    let r = ics_core::CellRef::new("work", "cmos_inverter");
    let p = h.state_mut().project_mut().unwrap();
    let m1 = p.tech.metal1().unwrap();
    let t = ics_core::tracks::TrackPattern::from_rules(&p.tech, m1, true).unwrap();
    p.cell_mut(&r).unwrap().layout.as_mut().unwrap().tracks.push(t);
    let sch = p.cell_mut(&r).unwrap().schematic.as_mut().unwrap();
    assert_eq!(sch.mark_ports(&[("in".into(), ics_core::PortDir::Input), ("out".into(), ics_core::PortDir::Output)]), 2);
    h.run_steps(2);
    h.state_mut().command(Command::Lay(LayAction::Tool(ics_studio::layout_view::LayTool::Path)));
    h.state_mut().command(Command::Lay(LayAction::EditorOptions));
    h.run_steps(4);
    shot(&mut h, "layout_tracks");
    h.state_mut().command(Command::SetWorkspace(Workspace::Schematic));
    h.run_steps(4);
    shot(&mut h, "schematic_pins");
}

/// Per-net electrical view: the highlighted net's capacitance, coupling and terminal resistance.
#[test]
fn layout_net_electrical() {
    let args = LaunchArgs {
        project: Some(demo_without_layout("net-elec", "cmos_inverter")),
        demo: false,
        workspace: Some("schematic".into()),
        cell: Some("cmos_inverter".into()),
        run: false,
    };
    let mut h = app(args);
    h.state_mut().command(Command::GenerateLayout);
    h.state_mut().command(Command::RouteFlylines);
    h.run_steps(2);
    h.state_mut().command(Command::ExtractNets);
    h.run_steps(2);
    settle(&mut h, "layout_net_electrical");
    let r = ics_core::CellRef::new("work", "cmos_inverter");
    let k = {
        let ed = h.state().layout_editor(&r).unwrap();
        let x = ed.verify.extraction.as_ref().expect("extracted");
        x.nets.iter().position(|n| n.name == "out").expect("net out")
    };
    h.state_mut().layout_editor_mut(&r).unwrap().show_net(k);
    h.run_steps(4);
    shot(&mut h, "layout_net_electrical");
}

/// A drawn L-shaped outline becomes a guard ring path (MPP) in one undoable step.
#[test]
fn layout_guard_ring_along_shape() {
    use ics_studio::layout_view::LayAction;
    let mut h = app(demo_args("inv_chain", "layout", false));
    let r = ics_core::CellRef::new("work", "inv_chain");
    let id = {
        let p = h.state_mut().project_mut().unwrap();
        let l = p.cell_mut(&r).unwrap().layout.as_mut().unwrap();
        let pts = vec![(-3000, -3000), (16000, -3000), (16000, 9000), (6000, 9000), (6000, 14000), (-3000, 14000)];
        l.add(0, ics_core::Geometry::Polygon { points: pts })
    };
    h.run_steps(2);
    let before = shape_count(&h, "inv_chain");
    h.state_mut().layout_editor_mut(&r).unwrap().selection = [id].into();
    h.state_mut().command(Command::Lay(LayAction::PRingAlong));
    h.run_steps(4);
    assert_eq!(shape_count(&h, "inv_chain"), before - 1, "the outline is replaced");
    {
        let p = h.state().project().unwrap();
        let l = p.cell(&r).unwrap().layout.as_ref().unwrap();
        let pc = p.cell(&l.instances.last().unwrap().cell).and_then(|c| c.pcell.clone());
        assert!(matches!(pc, Some(ics_core::pcell::PCell::RingPath { closed: true, well: false, .. })), "{pc:?}");
    }
    h.state_mut().command(Command::FitView);
    h.run_steps(3);
    shot(&mut h, "layout_ring_path");
    h.state_mut().command(Command::Undo);
    h.run_steps(2);
    assert_eq!(shape_count(&h, "inv_chain"), before, "one undo restores the outline");
}

/// A SKILL form displayed from the CIW appears as a dialog; OK runs its callback.
#[test]
fn skill_form_dialog() {
    let mut h = app(demo_args("cmos_inverter", "layout", false));
    let src = r#"procedure(genCB(form)
  let((cv) cv = dbOpenCellViewByType("work" "formgen" "layout" "maskLayout" "w")
    dbCreateRect(cv form->lay->value list(0:0 form->w->value:1))))
hiCreateAppForm(?name 'genForm ?formTitle "Plate Generator"
  ?fields list(hiCreateFloatField(?name 'w ?prompt "Width (um)" ?value 2.0)
               hiCreateCyclicField(?name 'lay ?prompt "Layer" ?choices list("metal1" "metal2") ?value "metal1"))
  ?callback "genCB(hiGetCurrentForm())")
hiDisplayForm(genForm)"#;
    h.state_mut().command(Command::CiwEval(src.into()));
    h.run_steps(4);
    assert_eq!(h.state().pending_skill_forms(), vec!["Plate Generator".to_string()]);
    shot(&mut h, "skill_form_dialog");
    assert!(h.state_mut().answer_skill_form(true, &[("w", "5")]));
    let p = h.state().project().unwrap();
    let l = p.cell(&ics_core::CellRef::new("work", "formgen")).and_then(|c| c.layout.clone()).expect("generated");
    assert_eq!(l.shapes[0].bbox().width(), 5 * p.tech.dbu_per_um);
    assert!(h.state().pending_skill_forms().is_empty());
}

/// Importing a cds.lib asks which of its libraries to dump through Virtuoso.
#[test]
fn virtuoso_cds_lib_import_asks_for_the_library() {
    let dir = std::env::temp_dir().join(format!("ics-ui-cdslib-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("cds.lib"), "DEFINE analogLib /pdk/analogLib\nDEFINE myLib ./myLib\n").unwrap();
    let mut h = app(demo_args("cmos_inverter", "layout", false));
    h.state_mut().command(Command::ImportVirtuoso(Some(dir.join("cds.lib"))));
    h.run_steps(4);
    assert_eq!(h.state().pending_virtuoso_libs(), vec!["analogLib".to_string(), "myLib".to_string()]);
    shot(&mut h, "virtuoso_cds_lib");
    std::fs::remove_dir_all(&dir).ok();
}

/// Extensions: a Python extension's tool runs from the bridge and calls a built-in tool back
/// (undoably), a SKILL command runs from the Extensions menu, and the Extensions window lists
/// them.
#[test]
fn extensions_run_in_the_app() {
    use egui_kittest::kittest::Queryable;
    use std::path::Path;
    let Some(py) = ["python3", "python"]
        .into_iter()
        .find(|p| std::process::Command::new(p).arg("-V").output().is_ok_and(|o| o.status.success()))
    else {
        eprintln!("no Python: skipped");
        return;
    };
    let root = std::env::temp_dir().join(format!("ics-ui-ext-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let rect = root.join("rect");
    std::fs::create_dir_all(&rect).unwrap();
    std::fs::write(
        rect.join("extension.toml"),
        format!(
            "[extension]\nid = \"rect\"\nname = \"Rect maker\"\nversion = \"0.1.0\"\ndescription = \"Adds a metal1 square.\"\n\
             [run]\ncommand = [\"{py}\", \"main.py\"]\n[[tools]]\nname = \"square\"\ndescription = \"Add a square.\"\n\
             [[commands]]\nid = \"square\"\ntitle = \"Add a square\"\ntool = \"square\"\narguments = {{ cell = \"${{cell}}\" }}\n"
        ),
    )
    .unwrap();
    std::fs::write(
        rect.join("main.py"),
        "from ics_extension import Extension\next = Extension()\n@ext.tool('square')\ndef square(cell):\n    \
         r = ext.call('add_rect', cell=cell, layer='metal1', x0=0, y0=-9, x1=1, y1=-8)\n    \
         ext.log('square added')\n    return {'message': 'added ' + str(r['id']), 'id': r['id']}\next.run()\n",
    )
    .unwrap();
    let frame = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extensions/metal-frame");
    ics_mcp::ext::install::install(&root, &ics_mcp::ext::install::Source::Folder(frame), false).unwrap();

    let args = LaunchArgs { project: None, demo: true, workspace: Some("home".into()), cell: None, run: false };
    let mut h = Harness::builder().with_size(egui::vec2(1440.0, 900.0)).wgpu().build_eframe(|cc| IcStudio::new(cc, args));
    h.state_mut().set_extensions(ics_mcp::ext::Registry::load_from(&root));
    let (tx, rx) = std::sync::mpsc::channel();
    h.state_mut().attach_bridge(rx);
    h.run_steps(2);

    let before = shape_count(&h, "cmos_inverter");
    let (reply, answer) = std::sync::mpsc::channel();
    let arguments = serde_json::json!({"cell": "cmos_inverter"}).as_object().unwrap().clone();
    tx.send(ics_mcp::bridge::Pending { tool: "rect__square".into(), arguments, reply }).unwrap();
    let mut res = None;
    for _ in 0..500 {
        h.step();
        if let Ok(r) = answer.try_recv() {
            res = Some(r);
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let res = res.expect("the extension answered");
    assert!(!res.is_error, "{res:?}");
    assert_eq!(shape_count(&h, "cmos_inverter"), before + 1);
    h.state_mut().command(Command::Undo);
    h.run_steps(2);
    assert_eq!(shape_count(&h, "cmos_inverter"), before, "edits an extension makes are undoable");

    // The SKILL command of metal-frame draws four rectangles around the active cell.
    let cell = h.state().project().unwrap().all_cells().find(|(_, c)| c.name == "cmos_inverter").unwrap().0;
    h.state_mut().command(Command::OpenCell(cell, Some(Workspace::Layout)));
    h.run_steps(2);
    h.state_mut().command(Command::RunExtension("metal-frame".into(), "frame".into()));
    h.run_steps(4);
    settle(&mut h, "metal frame");
    assert_eq!(shape_count(&h, "cmos_inverter"), before + 4);

    h.state_mut().command(Command::Extensions);
    h.run_steps(4);
    assert!(h.query_by_label_contains("Installed (2)").is_some());
    assert!(h.query_by_label_contains("rect__square").is_some(), "tools are listed");
    shot(&mut h, "15_extensions");
    h.state().extensions().stop_all();
    let _ = std::fs::remove_dir_all(&root);
}
