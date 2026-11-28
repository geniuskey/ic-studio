//! Menu bar, activity bar, explorer, tabs, workspaces, status bar, console and command palette.

use super::*;

impl IcStudio {
    // ----- UI pieces -----
    pub(super) fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let pal = theme::palette(ui.ctx());
        let (has_sch, has_lay) = self.active_cell().map_or((false, false), |c| (c.schematic.is_some(), c.layout.is_some()));
        let q = &mut self.queue;
        let has_project = self.project.is_some();
        let lay = self.workspace == Workspace::Layout && self.active.is_some();
        egui::MenuBar::new().ui(ui, |ui| {
            let adv = !self.settings.beginner;
            let menu_rect = ui.menu_button(t("File"), |ui| {
                if ui.button(t("New Project…")).clicked() {
                    q.push(Command::NewProjectDialog);
                }
                if ui.button(t("New Demo Project")).clicked() {
                    q.push(Command::NewDemoProject);
                }
                ui.menu_button(t("Open Example"), |ui| {
                    for t in &examples::TOPICS {
                        if ui.button(t.title).on_hover_text(t.description).clicked() {
                            q.push(Command::OpenExample(t.name.into()));
                        }
                    }
                });
                if ui
                    .button(t("Save Examples to Folder…"))
                    .on_hover_text(t("Write the example projects, one folder per topic"))
                    .clicked()
                {
                    q.push(Command::SaveExamples);
                }
                if ui.add(egui::Button::new(t("Open Project…")).shortcut_text(theme::kbd("⌘O"))).clicked() {
                    q.push(Command::OpenProjectDialog);
                }
                ui.menu_button(t("Open Recent"), |ui| {
                    if self.settings.recent.is_empty() {
                        ui.label(t("No recent projects"));
                    }
                    for p in &self.settings.recent {
                        if ui.button(p.display().to_string()).clicked() {
                            q.push(Command::OpenProject(p.clone()));
                        }
                    }
                });
                ui.separator();
                if ui.add_enabled(has_project, egui::Button::new(t("Save")).shortcut_text(theme::kbd("⌘S / F2"))).clicked() {
                    q.push(Command::Save);
                }
                if ui.add_enabled(has_project, egui::Button::new(t("Save As…")).shortcut_text(theme::kbd("⇧⌘S"))).clicked() {
                    q.push(Command::SaveAs);
                }
                if ui
                    .add_enabled(has_project, egui::Button::new(t("Check and Save")))
                    .on_hover_text(t("Check the active schematic, save if it has no errors"))
                    .clicked()
                {
                    q.push(Command::CheckSchematic(true));
                }
                ui.separator();
                if ui.add_enabled(has_project, egui::Button::new(t("New Cell…")).shortcut_text(theme::kbd("⌘N"))).clicked() {
                    q.push(Command::NewCellDialog);
                }
                if ui
                    .add_enabled(self.active.is_some(), egui::Button::new(t("Copy Cell as New…")))
                    .on_hover_text(t("Start a new design from the active cell: every view, specs and notes"))
                    .clicked()
                    && let Some(r) = self.active.clone()
                {
                    q.push(Command::CopyCellDialog(r));
                }
                ui.menu_button(t("Import"), |ui| {
                    if ui.add_enabled(has_project, egui::Button::new(t("GDSII / OASIS…"))).clicked() {
                        q.push(Command::ImportGds(None));
                    }
                    if ui.add_enabled(has_project, egui::Button::new(t("PDK…"))).clicked() {
                        q.push(Command::LoadPdk(None));
                    }
                    if !adv {
                        return;
                    }
                    ui.separator();
                    if ui.add_enabled(has_project, egui::Button::new(t("GDSII / OASIS with Layer Map…"))).clicked() {
                        q.push(Command::ImportGdsWithLayerMap(None, None));
                    }
                    if ui.add_enabled(has_project, egui::Button::new(t("LEF abstracts…"))).clicked() {
                        q.push(Command::ImportLefDef(false, None));
                    }
                    if ui.add_enabled(has_project, egui::Button::new(t("DEF placement…"))).clicked() {
                        q.push(Command::ImportLefDef(true, None));
                    }
                    if ui.add_enabled(has_project, egui::Button::new(t("KLayout layer properties…"))).clicked() {
                        q.push(Command::ImportTech);
                    }
                    if ui.add_enabled(has_project, egui::Button::new(t("CDL netlist (Virtuoso)…"))).clicked() {
                        q.push(Command::ImportCdl(None));
                    }
                    if ui.add_enabled(has_project, egui::Button::new(t("Virtuoso library (dump or cds.lib)…"))).clicked() {
                        q.push(Command::ImportVirtuoso(None));
                    }
                });
                ui.menu_button(t("Export"), |ui| {
                    if ui.add_enabled(has_project, egui::Button::new(t("SPICE netlist…"))).clicked() {
                        q.push(Command::ExportNetlist);
                    }
                    if ui.add_enabled(has_project, egui::Button::new(t("Cell GDSII…"))).clicked() {
                        q.push(Command::ExportGds);
                    }
                    if ui.add_enabled(has_project, egui::Button::new(t("Library GDSII…"))).clicked() {
                        q.push(Command::ExportLibraryGds);
                    }
                    if !adv {
                        return;
                    }
                    ui.separator();
                    if ui.add_enabled(has_project, egui::Button::new(t("Virtuoso schematic (SKILL)…"))).clicked() {
                        q.push(Command::ExportSkill);
                    }
                    if ui.button(t("Virtuoso dump script (icsDump.il)…")).clicked() {
                        q.push(Command::SaveDumpSkill(None));
                    }
                    if ui.add_enabled(has_project, egui::Button::new(t("Cell OASIS…"))).clicked() {
                        q.push(Command::ExportOasis);
                    }
                    if ui.add_enabled(has_project, egui::Button::new(t("Library OASIS…"))).clicked() {
                        q.push(Command::ExportLibraryOasis);
                    }
                    if ui.add_enabled(has_project, egui::Button::new(t("Cell LEF abstract…"))).clicked() {
                        q.push(Command::ExportLefDef(false));
                    }
                    if ui.add_enabled(has_project, egui::Button::new(t("Cell DEF…"))).clicked() {
                        q.push(Command::ExportLefDef(true));
                    }
                });
                ui.separator();
                if ui.add_enabled(has_project, egui::Button::new(t("Close Project"))).clicked() {
                    q.push(Command::CloseProject);
                }
                if ui.add(egui::Button::new(t("Settings…")).shortcut_text(theme::kbd("⌘,"))).clicked() {
                    q.push(Command::Settings);
                }
                if ui.add(egui::Button::new(t("Quit")).shortcut_text(theme::kbd("⌘Q"))).clicked() {
                    q.push(Command::Quit);
                }
            }).response.rect;
            crate::spot::mark(ui.ctx(), "menu.file", menu_rect);
            ui.menu_button(t("Edit"), |ui| {
                if ui.add(egui::Button::new(t("Undo")).shortcut_text(theme::kbd(if lay { "u / ⌘Z" } else { "⌘Z" }))).clicked() {
                    q.push(Command::Undo);
                }
                if ui
                    .add(egui::Button::new(t("Redo")).shortcut_text(theme::kbd(if lay { "Shift+U / ⇧⌘Z" } else { "⇧⌘Z" })))
                    .clicked()
                {
                    q.push(Command::Redo);
                }
                ui.separator();
                if ui.add(egui::Button::new(t("Delete")).shortcut_text("Del")).clicked() {
                    q.push(Command::Delete);
                }
                if lay {
                    ui.separator();
                    lay_menu_items(ui, "Edit", q);
                }
            });
            ui.menu_button(t("View"), |ui| {
                for (i, w) in Workspace::ALL.iter().enumerate() {
                    if ui.add(egui::Button::new(w.name()).shortcut_text(theme::kbd(&format!("⌘{}", i + 1)))).clicked() {
                        q.push(Command::SetWorkspace(*w));
                    }
                }
                ui.separator();
                if ui.add(egui::Button::new(t("Fit to Window")).shortcut_text("F")).clicked() {
                    q.push(Command::FitView);
                }
                if ui.add(egui::Button::new(t("Toggle Console")).shortcut_text(theme::kbd("⌘J"))).clicked() {
                    q.push(Command::ToggleConsole);
                }
                if ui.add(egui::Button::new(t("Toggle Explorer")).shortcut_text(theme::kbd("⌘B"))).clicked() {
                    q.push(Command::ToggleExplorer);
                }
                if ui.button(if self.settings.dark { "Light Theme" } else { "Dark Theme" }).clicked() {
                    q.push(Command::ToggleTheme);
                }
                if lay {
                    ui.separator();
                    lay_menu_items(ui, "View", q);
                }
            });
            if lay {
                ui.menu_button(t("Create"), |ui| lay_menu_items(ui, "Create", q));
            }
            let menu_rect = ui.menu_button(t("Simulation"), |ui| {
                if ui
                    .add_enabled(has_sch, egui::Button::new(t("Run")).shortcut_text("F5"))
                    .on_disabled_hover_text(NEED_SCH)
                    .clicked()
                {
                    q.push(Command::RunSimulation);
                }
                if menu_item(ui, has_sch, t("Export Netlist…"), NEED_SCH) {
                    q.push(Command::ExportNetlist);
                }
                ui.separator();
                if menu_item(ui, has_sch, t("Corner / Sweep Manager…"), NEED_SCH) {
                    q.push(Command::SweepManager);
                }
                if adv && menu_item(ui, has_sch, t("Monte Carlo…"), NEED_SCH) {
                    q.push(Command::MonteCarlo);
                }
                if menu_item(ui, has_sch, t("Optimizer…"), NEED_SCH) {
                    q.push(Command::Optimizer);
                }
                if menu_item(ui, has_sch, t("Derive New Design…"), NEED_SCH)
                    && let Some(r) = self.active.clone()
                {
                    q.push(Command::DeriveDialog(r));
                }
                if menu_item(ui, has_project, t("Verification…"), NEED_PROJECT) {
                    q.push(Command::Verification);
                }
                ui.separator();
                if ui.button(t("Waveform Calculator…")).clicked() {
                    q.push(Command::Calculator);
                }
                if ui.button(t("Results Browser…")).clicked() {
                    q.push(Command::ResultsBrowser);
                }
                if !adv {
                    return;
                }
                ui.separator();
                if ui.button(t("Pixel Optics…")).clicked() {
                    q.push(Command::PixelOptics);
                }
                if menu_item(ui, has_sch, t("Silicon Correlation…"), NEED_SCH) {
                    q.push(Command::SiliconCorrelation);
                }
                if menu_item(ui, has_lay, t("Process Emulation…"), NEED_LAY) {
                    q.push(Command::ProcessEmulation);
                }
                if ui.button(t("Process Data…")).clicked() {
                    q.push(Command::ProcessData);
                }
            }).response.rect;
            crate::spot::mark(ui.ctx(), "menu.simulation", menu_rect);
            let menu_rect = ui.menu_button(t("Verify"), |ui| {
                if menu_item(ui, has_lay, t("Run DRC"), NEED_LAY) {
                    q.push(Command::RunDrc);
                }
                if menu_item(ui, has_lay, t("Extract Nets"), NEED_LAY) {
                    q.push(Command::ExtractNets);
                }
                if adv && menu_item(ui, has_lay, t("Schematic from Layout"), NEED_LAY) {
                    q.push(Command::SchematicFromLayout);
                }
                if adv && menu_item(ui, has_lay, t("Pixel Circuit…"), NEED_LAY) {
                    q.push(Command::PixelCircuit);
                }
                if menu_item(ui, has_lay && has_sch, t("Run LVS"), t("Open a cell with a schematic and a layout first")) {
                    q.push(Command::RunLvs);
                }
                if lay && ui.add(egui::Button::new(t("Mark Net")).shortcut_text("Shift+N")).clicked() {
                    q.push(Command::Lay(LayAction::MarkNet));
                }
                if adv && menu_item(ui, has_lay, t("Run Sign-off Runset"), NEED_LAY) {
                    q.push(Command::RunSignoff(None));
                }
                if adv && menu_item(ui, has_lay, t("Import Sign-off Results…"), NEED_LAY) {
                    q.push(Command::ImportSignoffResults(None));
                }
                ui.separator();
                if menu_item(ui, has_sch, t("Generate Layout from Schematic"), NEED_SCH) {
                    q.push(Command::GenerateLayout);
                }
                if menu_item(ui, has_lay, t("Route Flylines"), NEED_LAY) {
                    q.push(Command::RouteFlylines);
                }
                if menu_item(ui, has_lay && has_sch, t("Check Against Source"), NEED_SCH) {
                    q.push(Command::CheckAgainstSource);
                }
                if menu_item(ui, has_lay && has_sch, t("Update Components and Nets"), NEED_SCH) {
                    q.push(Command::UpdateFromSource(false));
                }
                if adv && menu_item(ui, has_lay && has_sch, t("Update and Delete Extra Devices"), NEED_SCH) {
                    q.push(Command::UpdateFromSource(true));
                }
            }).response.rect;
            crate::spot::mark(ui.ctx(), "menu.verify", menu_rect);
            ui.menu_button(t("Tools"), |ui| {
                if ui.button(t("Design Copilot…")).clicked() {
                    q.push(Command::Copilot);
                }
                if ui.button(t("AI Models…")).clicked() {
                    q.push(Command::AiModels);
                }
                ui.separator();
                if menu_item(ui, has_sch, t("Check Schematic"), NEED_SCH) {
                    q.push(Command::CheckSchematic(false));
                }
                if menu_item(ui, has_sch, t("Place Cell Instance…"), NEED_SCH) {
                    q.push(Command::PlaceCellDialog);
                }
                if menu_item(ui, has_sch, t("Update Cell Instance Pins"), NEED_SCH) {
                    q.push(Command::UpdateCellInstances);
                }
                if adv && menu_item(ui, has_sch, t("Edit Symbol Pins…"), NEED_SCH) {
                    q.push(Command::EditSymbolDialog);
                }
                if menu_item(ui, has_project, t("Hierarchy Browser…"), NEED_PROJECT) {
                    q.push(Command::HierarchyBrowser);
                }
                if menu_item(ui, has_project, t("Net Browser…"), NEED_PROJECT) {
                    q.push(Command::NetBrowser);
                }
                if !adv {
                    return;
                }
                ui.menu_button(t("Layout Generators"), |ui| {
                    if menu_item(ui, has_project, t("Guard Ring…"), NEED_PROJECT) {
                        q.push(Command::GuardRing);
                    }
                    if menu_item(ui, has_project, t("Via Array…"), NEED_PROJECT) {
                        q.push(Command::ViaArray);
                    }
                    if menu_item(ui, has_project, t("Matched Placement…"), NEED_PROJECT) {
                        q.push(Command::MatchedPlacement);
                    }
                });
                ui.separator();
                if ui.button(t("DRC Rule Designer…")).clicked() {
                    q.push(Command::RuleDesigner);
                }
                if menu_item(ui, has_project, t("PCell Designer…"), NEED_PROJECT) {
                    q.push(Command::PCellDesigner);
                }
                ui.menu_button(t("DRC Deck"), |ui| {
                    let linked = self.project.as_ref().and_then(Project::drc_deck_path);
                    if let Some(f) = &linked {
                        ui.label(tf("Shared: {}", &[&f.display().to_string()]));
                        ui.separator();
                    }
                    if menu_item(ui, has_project, t("Import Deck…"), NEED_PROJECT) {
                        q.push(Command::ImportDrcDeck(None));
                    }
                    if menu_item(ui, has_project, t("Export Deck…"), NEED_PROJECT) {
                        q.push(Command::ExportDrcDeck(None));
                    }
                    ui.separator();
                    if menu_item(ui, has_project, t("Link Shared Deck…"), NEED_PROJECT) {
                        q.push(Command::LinkDrcDeck(None));
                    }
                    let why = if has_project { t("The technology is not linked to a shared deck") } else { NEED_PROJECT };
                    if menu_item(ui, linked.is_some(), t("Reload Shared Deck"), why) {
                        q.push(Command::ReloadDrcDeck);
                    }
                    if menu_item(ui, linked.is_some(), t("Unlink Shared Deck"), why) {
                        q.push(Command::UnlinkDrcDeck);
                    }
                });
                if menu_item(ui, has_project, t("Stream Layer Map…"), NEED_PROJECT) {
                    q.push(Command::StreamMap);
                }
                if menu_item(ui, has_project, t("Layer Display…"), NEED_PROJECT) {
                    q.push(Command::LayerDisplay(None));
                }
                if menu_item(ui, has_project, t("Dummy Fill…"), NEED_PROJECT) {
                    q.push(Command::DummyFill);
                }
                if menu_item(ui, has_project, t("OPC…"), NEED_PROJECT) {
                    q.push(Command::Opc);
                }
                ui.separator();
                if ui.button(t("SKILL CIW")).clicked() {
                    q.push(Command::Ciw);
                }
                if menu_item(ui, has_project, t("Load SKILL (.il)…"), NEED_PROJECT) {
                    q.push(Command::LoadSkill(None));
                }
            });
            if lay {
                ui.menu_button(t("Options"), |ui| lay_menu_items(ui, "Options", q));
            }
            if adv {
                Self::extensions_menu(&self.extensions, ui, has_project, q);
            }
            let menu_rect = ui.menu_button(t("Help"), |ui| {
                if ui.button(t("Learn IC Design…")).clicked() {
                    q.push(Command::Learn(None));
                }
                if ui.button(t("Interactive Tutorial")).clicked() {
                    q.push(Command::Tutorial);
                }
                let mut beginner = self.settings.beginner;
                if ui
                    .checkbox(&mut beginner, t("Beginner Mode"))
                    .on_hover_text(t("Show the everyday design flow in the menus and hide specialist tools (the command palette still finds every command)"))
                    .clicked()
                {
                    q.push(Command::BeginnerMode(beginner));
                }
                if ui.button(t("User Guide")).clicked() {
                    q.push(Command::UserGuide);
                }
                if ui.add(egui::Button::new(t("Keyboard Shortcuts")).shortcut_text("F1")).clicked() {
                    q.push(Command::Shortcuts);
                }
                if ui.add(egui::Button::new(t("Command Palette")).shortcut_text(theme::kbd("⌘K"))).clicked() {
                    q.push(Command::CommandPalette);
                }
                ui.separator();
                if ui.button(t("Check for Updates…")).clicked() {
                    q.push(Command::CheckForUpdates);
                }
                if ui.button(t("About IC Studio")).clicked() {
                    q.push(Command::About);
                }
            }).response.rect;
            crate::spot::mark(ui.ctx(), "menu.help", menu_rect);
            // breadcrumb
            ui.add_space(16.0);
            if let Some(p) = &self.project {
                ui.label(RichText::new(&p.name).strong());
                if let Some(r) = &self.active {
                    ui.label(RichText::new("›").color(pal.text_weak));
                    ui.label(RichText::new(&r.lib).color(pal.text_weak));
                    ui.label(RichText::new("›").color(pal.text_weak));
                    ui.label(RichText::new(&r.cell));
                }
                if self.dirty {
                    ui.label(RichText::new("●").color(pal.warning)).on_hover_text(t("Unsaved changes"));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let b = egui::Button::new(RichText::new(theme::kbd(&format!("🔍  {}   ⌘K", t("Search commands…")))).color(pal.text_weak))
                    .min_size(Vec2::new(220.0, 22.0));
                let r = ui.add(b);
                crate::spot::mark(ui.ctx(), "palette", r.rect);
                if r.clicked() {
                    q.push(Command::CommandPalette);
                }
            });
        });
    }

    pub(super) fn activity_bar(&mut self, ui: &mut egui::Ui) {
        let pal = theme::palette(ui.ctx());
        let spot = self.tutorial.spotlight().filter(|w| *w != self.workspace);
        ui.add_space(6.0);
        for w in Workspace::ALL {
            let active = self.workspace == w;
            let (r, p) = ui.allocate_painter(Vec2::new(ui.available_width(), 48.0), Sense::click());
            crate::spot::mark(ui.ctx(), crate::spot::workspace_key(w), r.rect);
            let color = w.color(&pal);
            if spot == Some(w) {
                let t = ui.input(|i| i.time);
                let a = 0.45 + 0.4 * (t * 4.0).sin().abs() as f32;
                p.rect_stroke(
                    r.rect.shrink2(Vec2::new(4.0, 1.0)),
                    9.0,
                    Stroke::new(2.0, color.gamma_multiply(a)),
                    egui::StrokeKind::Inside,
                );
                ui.ctx().request_repaint_after(Duration::from_millis(50));
            }
            if active {
                p.rect_filled(r.rect.shrink2(Vec2::new(6.0, 3.0)), 8.0, color.gamma_multiply(0.16));
                p.rect_filled(egui::Rect::from_min_size(r.rect.min + Vec2::new(0.0, 10.0), Vec2::new(3.0, 28.0)), 2.0, color);
            } else if r.hovered() {
                p.rect_filled(r.rect.shrink2(Vec2::new(6.0, 3.0)), 8.0, pal.bg_hover);
            }
            let ic = egui::Rect::from_center_size(r.rect.center(), Vec2::splat(24.0));
            icons::paint(
                &p,
                ic,
                w.icon(),
                if active {
                    color
                } else if r.hovered() {
                    pal.text
                } else {
                    pal.text_weak
                },
            );
            if r.clicked() {
                self.queue.push(Command::SetWorkspace(w));
            }
            let i = Workspace::ALL.iter().position(|x| *x == w).unwrap() + 1;
            r.on_hover_text(theme::kbd(&format!("{} (⌘{i})", w.name())));
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
            ui.add_space(6.0);
            let (r, p) = ui.allocate_painter(Vec2::new(ui.available_width(), 44.0), Sense::click());
            if r.hovered() {
                p.rect_filled(r.rect.shrink2(Vec2::new(6.0, 3.0)), 8.0, pal.bg_hover);
            }
            icons::paint(&p, egui::Rect::from_center_size(r.rect.center(), Vec2::splat(22.0)), Icon::Settings, pal.text_weak);
            if r.clicked() {
                self.queue.push(Command::Settings);
            }
            r.on_hover_text(theme::kbd("Settings (⌘,)"));
        });
    }

    pub(super) fn explorer(&mut self, ui: &mut egui::Ui) {
        let pal = theme::palette(ui.ctx());
        let Some(p) = &self.project else {
            ui.label(RichText::new(t("No project open")).color(pal.text_weak));
            if ui.button(t("Open demo project")).clicked() {
                self.queue.push(Command::NewDemoProject);
            }
            return;
        };
        ui.horizontal(|ui| {
            theme::section(ui, t("Explorer"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("+").on_hover_text(theme::kbd("New cell (⌘N)")).clicked() {
                    self.queue.push(Command::NewCellDialog);
                }
            });
        });
        ui.horizontal(|ui| {
            let l = ui.label(RichText::new(t("Filter")).small().color(pal.text_weak));
            let te = egui::TextEdit::singleline(&mut self.explorer_filter).hint_text(t("cell name")).desired_width(f32::INFINITY);
            let resp = ui.add(te).labelled_by(l.id);
            if (resp.has_focus() || resp.lost_focus()) && ui.input(|i| i.key_pressed(Key::Escape)) {
                self.explorer_filter.clear();
            }
        });
        // With the course open, only the cells its chapter uses (and the open one).
        let course: Option<std::collections::BTreeSet<String>> = self.learn.open.then(|| {
            let mut cells = crate::learn::chapter_cells(crate::learn::blocks(crate::i18n::current(), self.learn.chapter));
            cells.extend(cells.iter().filter_map(|c| self.learn.aliases.get(c).cloned()).collect::<Vec<_>>());
            cells.extend(self.active.as_ref().map(|r| r.cell.clone()));
            cells
        });
        if course.is_some() {
            ui.checkbox(&mut self.explorer_course, t("Only the cells of this chapter"))
                .on_hover_text(t("While the Learn course is open, list only the cells the chapter uses"));
        }
        let course = course.filter(|_| self.explorer_course);
        let filter = self.explorer_filter.trim().to_lowercase();
        if !filter.is_empty() || course.is_some() {
            let mut any = false;
            for lib in &p.libraries {
                let hits: Vec<&Cell> = lib
                    .cells
                    .iter()
                    .filter(|c| c.name.to_lowercase().contains(&filter))
                    .filter(|c| course.as_ref().is_none_or(|s| s.contains(&c.name)))
                    .collect();
                if hits.is_empty() {
                    continue;
                }
                any = true;
                ui.label(RichText::new(&lib.name).small().strong().color(pal.text_weak));
                for cell in hits {
                    let r = CellRef::new(&lib.name, &cell.name);
                    let active = self.active.as_ref() == Some(&r);
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        let (ir, painter) = ui.allocate_painter(Vec2::splat(14.0), Sense::hover());
                        icons::paint(&painter, ir.rect, Icon::Cell, if active { pal.accent } else { pal.text_weak });
                        let resp = ui.selectable_label(active, &cell.name);
                        if resp.clicked() {
                            self.selected_lib = None;
                            self.queue.push(Command::OpenCell(r.clone(), None));
                        }
                        let resp = Self::cell_menu(Some(p), &mut self.queue, &resp, &r, None);
                        resp.on_hover_text(cell_tip(cell));
                    });
                }
            }
            if !any {
                let none = if course.is_some() && filter.is_empty() {
                    t("This chapter uses no cell of the project yet")
                } else {
                    t("No cell matches")
                };
                ui.label(RichText::new(none).color(pal.text_weak));
            }
            return;
        }
        for lib in &p.libraries {
            // Libraries of generated PCell variants start folded: they are many, and rarely opened.
            let generated = !lib.cells.is_empty() && lib.cells.iter().all(|c| c.pcell.is_some());
            let id = ui.make_persistent_id(("lib", &lib.name));
            let mut toggle = false;
            let selected = self.selected_lib.as_deref() == Some(lib.name.as_str());
            let mut header = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, !generated)
                .show_header(ui, |ui| {
                    let (r, painter) = ui.allocate_painter(Vec2::splat(16.0), Sense::hover());
                    icons::paint(&painter, r.rect, Icon::Library, if selected { pal.accent } else { pal.text_weak });
                    let resp = ui.selectable_label(selected, RichText::new(&lib.name).strong());
                    if resp.clicked() {
                        self.selected_lib = Some(lib.name.clone());
                        toggle = true;
                    }
                    if resp.secondary_clicked() {
                        self.selected_lib = Some(lib.name.clone());
                    }
                    let gds = lib.storage == LibStorage::Gds;
                    if gds {
                        ui.label(RichText::new("GDS").small().color(pal.text_weak));
                    }
                    resp.context_menu(|ui| {
                        ui.label(
                            RichText::new(tf(
                                "{} · {} cell(s) · {}",
                                &[&lib.name, &lib.cells.len().to_string(), if gds { t("GDSII file") } else { "JSON" }],
                            ))
                            .small()
                            .color(pal.text_weak),
                        );
                        ui.separator();
                        if ui.button(t("New cell…")).clicked() {
                            self.queue.push(Command::NewCellInLibrary(lib.name.clone()));
                        }
                        let tops = p.library_tops(&lib.name);
                        ui.add_enabled_ui(!tops.is_empty(), |ui| {
                            ui.menu_button(t("Open top cell layout"), |ui| {
                                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                                    for t in &tops {
                                        if ui.button(&t.cell).clicked() {
                                            self.queue.push(Command::OpenCell(t.clone(), Some(Workspace::Layout)));
                                        }
                                    }
                                });
                            });
                        });
                        ui.separator();
                        if ui.add_enabled(!tops.is_empty(), egui::Button::new(t("Export GDSII…"))).clicked() {
                            self.queue.push(Command::ExportLibraryStream(lib.name.clone(), StreamFormat::Gds));
                        }
                        if ui.add_enabled(!tops.is_empty(), egui::Button::new(t("Export OASIS…"))).clicked() {
                            self.queue.push(Command::ExportLibraryStream(lib.name.clone(), StreamFormat::Oasis));
                        }
                        let mut store_gds = gds;
                        if ui
                            .checkbox(&mut store_gds, t("Store as one GDSII file"))
                            .on_hover_text(t("Save the library's layouts in a single .gds file instead of JSON cell directories"))
                            .changed()
                        {
                            let storage = if store_gds { LibStorage::Gds } else { LibStorage::Json };
                            self.queue.push(Command::SetLibraryStorage(lib.name.clone(), storage));
                        }
                        ui.separator();
                        if ui.button(t("Rename…")).clicked() {
                            self.queue.push(Command::RenameLibraryDialog(lib.name.clone()));
                        }
                        if ui.button(t("Delete…")).clicked() {
                            self.queue.push(Command::DeleteLibraryConfirm(lib.name.clone()));
                        }
                    });
                });
            if toggle {
                header.toggle();
            }
            header.body(|ui| {
                for cell in &lib.cells {
                    let r = CellRef::new(&lib.name, &cell.name);
                    let active = self.active.as_ref() == Some(&r);
                    let cid = ui.make_persistent_id(("cell", &r));
                    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), cid, active)
                        .show_header(ui, |ui| {
                            let (ir, painter) = ui.allocate_painter(Vec2::splat(14.0), Sense::hover());
                            icons::paint(&painter, ir.rect, Icon::Cell, if active { pal.accent } else { pal.text_weak });
                            let resp = ui.selectable_label(active, &cell.name);
                            if resp.clicked() {
                                self.selected_lib = None;
                                self.queue.push(Command::OpenCell(r.clone(), None));
                            }
                            let resp = Self::cell_menu(Some(p), &mut self.queue, &resp, &r, None);
                            resp.on_hover_text(cell_tip(cell));
                        })
                        .body(|ui| {
                            for (vk, icon, color, ws) in [
                                (ViewKind::Schematic, Icon::Schematic, pal.schematic, Workspace::Schematic),
                                (ViewKind::Simulation, Icon::Simulation, pal.simulation, Workspace::Simulation),
                                (ViewKind::Layout, Icon::Layout, pal.layout, Workspace::Layout),
                            ] {
                                let exists = cell.has_view(vk);
                                ui.horizontal(|ui| {
                                    let (ir, painter) = ui.allocate_painter(Vec2::splat(14.0), Sense::hover());
                                    icons::paint(
                                        &painter,
                                        ir.rect,
                                        icon,
                                        if exists { color } else { pal.text_weak.gamma_multiply(0.5) },
                                    );
                                    let text = RichText::new(t(vk.label())).color(if exists { pal.text } else { pal.text_weak });
                                    let is_open = active && self.workspace == ws;
                                    if ui.selectable_label(is_open, text).clicked() {
                                        self.queue.push(if exists {
                                            Command::OpenCell(r.clone(), Some(ws))
                                        } else {
                                            Command::CreateView(r.clone(), vk)
                                        });
                                    }
                                    if !exists {
                                        ui.label(RichText::new(t("+ create")).small().color(pal.text_weak));
                                    }
                                });
                            }
                        });
                }
            });
        }
    }

    pub(super) fn tab_strip(&mut self, ui: &mut egui::Ui) {
        let pal = theme::palette(ui.ctx());
        // Wrapped: many open cells (descending a GDS hierarchy) take a second row instead of
        // running off the edge.
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(2.0, 2.0);
            let tabs = self.tabs.clone();
            for t in tabs.clone() {
                let active = self.active.as_ref() == Some(&t);
                // Cells of the same name in two libraries are told apart by the library.
                let name = if tabs.iter().any(|o| o.cell == t.cell && o.lib != t.lib) { t.to_string() } else { t.cell.clone() };
                egui::Frame::new()
                    .fill(if active { pal.bg_canvas } else { pal.bg_panel })
                    .corner_radius(egui::CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 })
                    .inner_margin(egui::Margin { left: 10, right: 4, top: 4, bottom: 4 })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let c = if active { self.workspace.color(&pal) } else { pal.text_weak };
                            let (r, p) = ui.allocate_painter(Vec2::splat(8.0), Sense::hover());
                            p.circle_filled(r.rect.center(), 3.5, c);
                            let resp = ui.add(
                                egui::Label::new(RichText::new(name).color(if active { pal.text } else { pal.text_weak }))
                                    .sense(Sense::click()),
                            );
                            if resp.clicked() {
                                self.queue.push(Command::OpenCell(t.clone(), None));
                            }
                            if resp.middle_clicked() {
                                self.queue.push(Command::CloseTab(t.clone()));
                            }
                            let resp = resp
                                .on_hover_text(format!("{t}\n{}", crate::i18n::t("Middle-click to close, right-click for more")));
                            Self::cell_menu(self.project.as_ref(), &mut self.queue, &resp, &t, Some(self.tabs.len()));
                            if ui.add(egui::Button::new(RichText::new("×").color(pal.text_weak)).frame(false).small()).clicked()
                            {
                                self.queue.push(Command::CloseTab(t.clone()));
                            }
                        });
                    });
            }
        });
    }

    /// Right-click menu of a cell, shared by the explorer and the tab strip (`n_tabs` adds the tab items).
    pub(super) fn cell_menu(
        p: Option<&Project>,
        q: &mut Vec<Command>,
        resp: &egui::Response,
        r: &CellRef,
        n_tabs: Option<usize>,
    ) -> egui::Response {
        let views: Vec<(ViewKind, bool)> = p.and_then(|p| p.cell(r)).map_or_else(Vec::new, |c| {
            [ViewKind::Schematic, ViewKind::Simulation, ViewKind::Layout].into_iter().map(|vk| (vk, c.has_view(vk))).collect()
        });
        resp.context_menu(|ui| {
            ui.label(RichText::new(r.to_string()).small().strong());
            ui.separator();
            for (vk, exists) in &views {
                let ws = match vk {
                    ViewKind::Schematic => Workspace::Schematic,
                    ViewKind::Simulation => Workspace::Simulation,
                    _ => Workspace::Layout,
                };
                let text = if *exists { tf("Open {}", &[t(vk.label())]) } else { tf("Create {} view", &[t(vk.label())]) };
                if ui.button(text).clicked() {
                    q.push(if *exists { Command::OpenCell(r.clone(), Some(ws)) } else { Command::CreateView(r.clone(), *vk) });
                }
            }
            if views.iter().any(|(vk, e)| *vk == ViewKind::Schematic && *e) {
                ui.separator();
                if ui.add(egui::Button::new(t("Run Simulation")).shortcut_text("F5")).clicked() {
                    q.push(Command::OpenCell(r.clone(), None));
                    q.push(Command::RunSimulation);
                }
                if ui.button(t("Place in Active Schematic")).clicked() {
                    q.push(Command::PlaceCell(r.clone()));
                }
            }
            ui.separator();
            if ui.button(t("Copy Name")).clicked() {
                ui.ctx().copy_text(r.to_string());
            }
            if ui
                .button(t("Copy as New Cell…"))
                .on_hover_text(t("Start a new design from this one: every view, specs and notes"))
                .clicked()
            {
                q.push(Command::CopyCellDialog(r.clone()));
            }
            if ui.button(t("Rename…")).clicked() {
                q.push(Command::RenameCellDialog(r.clone()));
            }
            if ui.button(t("Delete…")).clicked() {
                q.push(Command::DeleteCellConfirm(r.clone()));
            }
            if let Some(n_tabs) = n_tabs {
                ui.separator();
                if ui.button(t("Close Tab")).clicked() {
                    q.push(Command::CloseTab(r.clone()));
                }
                if ui.add_enabled(n_tabs > 1, egui::Button::new(t("Close Other Tabs"))).clicked() {
                    q.push(Command::CloseOtherTabs(r.clone()));
                }
                if ui.button(t("Close All Tabs")).clicked() {
                    q.push(Command::CloseAllTabs);
                }
            }
        });
        resp.clone()
    }

    pub(super) fn no_view_placeholder(&mut self, ui: &mut egui::Ui, r: &CellRef, vk: ViewKind) {
        let pal = theme::palette(ui.ctx());
        ui.centered_and_justified(|ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() / 3.0);
                ui.label(RichText::new(tf("{} has no {} view", &[&r.cell, t(vk.label())])).size(16.0).color(pal.text_weak));
                ui.add_space(8.0);
                if ui.button(tf("Create {} view", &[t(vk.label())])).clicked() {
                    self.queue.push(Command::CreateView(r.clone(), vk));
                }
            });
        });
    }

    pub(super) fn no_cell_placeholder(&mut self, ui: &mut egui::Ui) {
        let pal = theme::palette(ui.ctx());
        ui.centered_and_justified(|ui| {
            ui.label(
                RichText::new(theme::kbd(t("Open a cell from the Explorer, or create one with ⌘N")))
                    .size(15.0)
                    .color(pal.text_weak),
            );
        });
    }

    pub(super) fn workspace_ui(&mut self, ui: &mut egui::Ui) {
        let pal = theme::palette(ui.ctx());
        let ws = self.workspace;
        if ws == Workspace::Home {
            egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg_canvas)).show(ui, |ui| match &self.project {
                Some(p) => home_view::project_overview(ui, p, &mut self.hier, &mut self.queue),
                None => home_view::welcome(ui, &self.settings.recent, &mut self.queue),
            });
            return;
        }
        let Some(r) = self.active.clone() else {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(pal.bg_canvas))
                .show(ui, |ui| self.no_cell_placeholder(ui));
            return;
        };
        let Some(project) = self.project.as_mut() else { return };
        let tech = project.tech.clone();
        let bulk_nets = [&project.settings.nmos_bulk, &project.settings.pmos_bulk].map(|n| n.clone().unwrap_or_default());
        let Some(cell) = project.cell_mut(&r) else {
            self.queue.push(Command::CloseTab(r));
            return;
        };
        let mut changed = false;
        match ws {
            Workspace::Schematic => {
                let op = self.sims.get(&r).and_then(|s| s.op_annotation());
                let ed = self.sch_eds.entry(r.clone()).or_default();
                if !ed.configured {
                    self.settings.editors.apply_sch(ed);
                }
                ed.mos_body_pin = self.settings.mos_body_pin;
                ed.bulk_nets = bulk_nets.clone();
                if let Some(sch) = cell.schematic.as_mut() {
                    theme::aside(egui::Panel::left("sch_tools").resizable(true).default_size(220.0), ui, |ui| {
                        crate::spot::mark(ui.ctx(), "sch.library", ui.max_rect());
                        egui::ScrollArea::vertical().show(ui, |ui| device_palette(ui, ed));
                    });
                    self.settings.mos_body_pin = ed.mos_body_pin;
                    for (k, (new, old)) in ed.bulk_nets.iter().zip(&bulk_nets).enumerate() {
                        if new != old {
                            self.queue.push(Command::DefaultBulk(k == 1, new.clone()));
                        }
                    }
                    let notes = &mut cell.notes;
                    let specs = &cell.sim.sweep.specs;
                    let q = &mut self.queue;
                    theme::aside(egui::Panel::right("sch_inspector").resizable(true).default_size(250.0), ui, |ui| {
                        crate::spot::mark(ui.ctx(), "sch.inspector", ui.max_rect());
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            if ed.selection.is_empty() {
                                let d = ui.scope(|ui| crate::guide_view::design_panel(ui, &r, notes, specs, q));
                                crate::spot::mark(ui.ctx(), "sch.design", d.response.rect);
                                changed |= d.inner;
                            }
                            changed |= ed.inspector(ui, sch);
                        });
                    });
                    egui::Panel::top("sch_toolbar").show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            for (t, icon, tip) in [
                                (SchTool::Select, Icon::Pointer, "Select (Esc)"),
                                (SchTool::Wire, Icon::Wire, "Draw wire (W)"),
                                (SchTool::Label, Icon::NetLabel, "Net label (T)"),
                            ] {
                                if icons::button(ui, icon, ed.tool == t, true, tip).clicked() {
                                    ed.set_tool(t);
                                }
                            }
                            icons::divider(ui);
                            let tip = theme::kbd("Undo (⌘Z)");
                            if icons::button(ui, Icon::Undo, false, ed.history.can_undo(), &tip).clicked() {
                                changed |= ed.undo(sch);
                            }
                            let tip = theme::kbd("Redo (⇧⌘Z)");
                            if icons::button(ui, Icon::Redo, false, ed.history.can_redo(), &tip).clicked() {
                                changed |= ed.redo(sch);
                            }
                            icons::divider(ui);
                            let any = !ed.selection.is_empty();
                            let inst = ed.selection.iter().any(|o| matches!(o, SchObj::Instance(_)));
                            if icons::button(ui, Icon::Rotate, false, inst, "Rotate selection (Space)").clicked() {
                                changed |= ed.rotate_selection(sch, false);
                            }
                            if icons::button(ui, Icon::Mirror, false, inst, "Mirror selection (X)").clicked() {
                                changed |= ed.rotate_selection(sch, true);
                            }
                            let tip = theme::kbd("Duplicate selection (⇧⌘D)");
                            if icons::button(ui, Icon::Duplicate, false, any, &tip).clicked() {
                                changed |= ed.duplicate_selection(sch);
                            }
                            if icons::button(ui, Icon::Delete, false, any, "Delete selection (Del)").clicked() {
                                changed |= ed.delete_selection(sch);
                            }
                            icons::divider(ui);
                            if icons::button(ui, Icon::Fit, false, true, "Fit to window (F)").clicked() {
                                ed.request_fit();
                            }
                            icons::toggle(ui, Icon::Grid, &mut ed.show_grid, "Show grid");
                            if op.is_some() {
                                icons::toggle(ui, Icon::Annotate, &mut ed.annotate_op, "Annotate operating point");
                            } else {
                                icons::button(
                                    ui,
                                    Icon::Annotate,
                                    false,
                                    false,
                                    "Annotate operating point (run an OP simulation first)",
                                );
                            }
                            icons::divider(ui);
                            ui.label(RichText::new(ed.tool.name()).color(pal.schematic));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let b = ui.button(RichText::new(t("▶ Simulate")).color(pal.simulation)).on_hover_text("F5");
                                crate::spot::mark(ui.ctx(), "sch.run", b.rect);
                                if b.clicked() {
                                    self.queue.push(Command::RunSimulation);
                                }
                            });
                        });
                    });
                    egui::CentralPanel::default().frame(egui::Frame::new()).show(ui, |ui| {
                        crate::spot::mark(ui.ctx(), "sch.canvas", ui.max_rect());
                        let out = ed.canvas(ui, sch, op.as_ref());
                        changed |= out.changed;
                        for req in out.requests {
                            self.queue.push(match req {
                                SchRequest::Simulate => Command::RunSimulation,
                                SchRequest::Check => Command::CheckSchematic(false),
                                SchRequest::ExportNetlist => Command::ExportNetlist,
                                SchRequest::PlaceCell => Command::PlaceCellDialog,
                                SchRequest::Descend(id) => Command::DescendSchematic(id),
                                SchRequest::Return => Command::ReturnSchematic,
                            });
                        }
                    });
                } else {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::new().fill(pal.bg_canvas))
                        .show(ui, |ui| self.no_view_placeholder(ui, &r, ViewKind::Schematic));
                }
            }
            Workspace::Layout => {
                let Some(mut lay) = cell.layout.take() else {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::new().fill(pal.bg_canvas))
                        .show(ui, |ui| self.no_view_placeholder(ui, &r, ViewKind::Layout));
                    return;
                };
                // The edited layout is taken out of the project while the editor runs, so the
                // project can resolve instances immutably (and cannot resolve the cell into itself).
                let l = &mut lay;
                let src: &Project = project;
                let placeable: Vec<CellRef> = src.all_cells().filter(|(_, c)| c.layout.is_some()).map(|(r, _)| r).collect();
                let context = eip_context(&self.lay_return, &r);
                let ed = self.lay_eds.entry(r.clone()).or_default();
                if !ed.configured {
                    self.settings.editors.apply_lay(ed);
                }
                let orient = context.as_ref().map_or(ics_core::Trans::IDENTITY, |c| c.to_root);
                if ed.view.orient != orient {
                    ed.view.set_orient(orient);
                }
                let mut cx =
                    LayCtx { cell: &r, src, hier: &mut self.hier, tech: &tech, cells: &placeable, context: context.as_ref() };
                let mut open: Option<CellRef> = None;
                {
                    theme::aside(egui::Panel::left("lay_tools").resizable(true).default_size(230.0), ui, |ui| {
                        if let Some(o) = ed.layer_palette(ui, l, &mut cx) {
                            open = Some(o);
                        }
                    });
                    theme::aside(egui::Panel::right("lay_inspector").resizable(true).default_size(250.0), ui, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            let (c, o) = ed.inspector(ui, l, &mut cx);
                            changed |= c;
                            open = open.take().or(o);
                        });
                    });
                    egui::Panel::top("lay_toolbar").show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            let tip = theme::kbd("Undo (u / ⌘Z)");
                            let undo = icons::button(ui, Icon::Undo, false, ed.history.can_undo(), &tip);
                            let tip = theme::kbd("Redo (Shift+U / ⇧⌘Z)");
                            let redo = icons::button(ui, Icon::Redo, false, ed.history.can_redo(), &tip);
                            if (undo.clicked() && ed.undo(l)) || (redo.clicked() && ed.redo(l)) {
                                changed = true;
                                cx.hier.invalidate(&r);
                            }
                            icons::divider(ui);
                            for (i, t) in LayTool::ALL.into_iter().enumerate() {
                                // Groups: select / edit / create / modify / measure.
                                if [1, 4, 10, 12].contains(&i) {
                                    icons::divider(ui);
                                }
                                if icons::button(ui, t.icon(), ed.tool == t, true, &format!("{} ({})", t.name(), t.key()))
                                    .clicked()
                                {
                                    ed.set_tool(t);
                                }
                            }
                            icons::divider(ui);
                            if icons::button(ui, Icon::Fit, false, true, "Fit to window (F)").clicked() {
                                ed.request_fit();
                            }
                            if icons::button(ui, Icon::ZoomBox, false, true, "Zoom box: drag or click a box to zoom into (Z)")
                                .clicked()
                            {
                                ed.arm_zoom_box();
                            }
                            if icons::button(ui, Icon::PrevView, false, ed.can_step_view(true), "Previous view (W)").clicked() {
                                ed.step_view(true);
                            }
                            icons::divider(ui);
                            icons::toggle(ui, Icon::Grid, &mut ed.show_grid, "Show grid");
                            icons::toggle(
                                ui,
                                Icon::Gravity,
                                &mut ed.gravity,
                                "Gravity: snap the cursor to nearby vertices and edges (G)",
                            );
                            icons::toggle(
                                ui,
                                Icon::Drd,
                                &mut ed.drd,
                                "Design-rule-driven editing: show spacing and width violations while drawing and moving",
                            );
                            icons::toggle(
                                ui,
                                Icon::Magnet,
                                &mut ed.magnet,
                                "Magnet: pull drawn and moved shapes to exactly the minimum spacing from their neighbours",
                            );
                            if icons::toggle(
                                ui,
                                Icon::LiveDrc,
                                &mut ed.live.enabled,
                                "Live DRC: check the rule deck around every edit; Ctrl+. offers quick fixes",
                            )
                            .changed()
                            {
                                ed.live.reset();
                            }
                            icons::toggle(
                                ui,
                                Icon::Constraints,
                                &mut ed.show_constraints,
                                "Constraints: show symmetry axes, alignments and fixed items",
                            );
                            icons::toggle(
                                ui,
                                Icon::Flylines,
                                &mut ed.show_flylines,
                                "Flylines: unrouted connections from the schematic",
                            );
                            icons::divider(ui);
                            ed.snap_control(ui, &tech);
                            icons::divider(ui);
                            if icons::button(ui, Icon::Export, false, true, "Export this cell as GDSII").clicked() {
                                self.queue.push(Command::ExportGds);
                            }
                            icons::divider(ui);
                            ui.label(RichText::new(ed.tool.name()).color(pal.layout));
                            ui.label(RichText::new(tf("on {}", &[&tech.layers[ed.active_layer].name])).color(pal.text_weak));
                            if let Some(c) = cx.context {
                                icons::divider(ui);
                                ui.label(RichText::new(tf("Editing in place in {}", &[&c.root.cell])).color(pal.accent));
                                if icons::button(ui, Icon::Return, false, true, "Return: leave edit-in-place (Shift+B)").clicked()
                                {
                                    ed.requests.push(LayRequest::Return);
                                }
                            }
                        });
                    });
                    egui::CentralPanel::default().frame(egui::Frame::new()).show(ui, |ui| {
                        crate::spot::mark(ui.ctx(), "lay.canvas", ui.max_rect());
                        changed |= ed.canvas(ui, l, &mut cx).changed;
                    });
                }
                let mut log: Vec<(bool, String)> = vec![];
                let mut touched = false;
                let mut handoff: Option<(CellRef, View2D)> = None;
                let mut refit: Option<CellRef> = None;
                for req in std::mem::take(&mut ed.requests) {
                    match req {
                        LayRequest::Open(c) => open = Some(c),
                        LayRequest::Descend { cell, inst, elem, trans, in_place: true } => {
                            handoff = Some((cell.clone(), View2D { orient: ed.view.orient.compose(&trans), ..ed.view }));
                            self.lay_return.push(Descent {
                                parent: r.clone(),
                                child: cell.clone(),
                                at: Some((inst, elem, trans)),
                            });
                            log.push((true, format!("Editing {cell} in place (Shift+B returns)")));
                            open = Some(cell);
                        }
                        LayRequest::Descend { cell, in_place: false, .. } => {
                            self.lay_return.push(Descent { parent: r.clone(), child: cell.clone(), at: None });
                            log.push((true, format!("Descended into {cell} (Shift+B returns)")));
                            refit = Some(cell.clone());
                            open = Some(cell);
                        }
                        LayRequest::Return => match self.lay_return.pop() {
                            Some(d) => {
                                if let Some((_, _, t)) = d.at.filter(|_| d.child == r) {
                                    handoff = Some((
                                        d.parent.clone(),
                                        View2D { orient: ed.view.orient.compose(&t.inverse()), ..ed.view },
                                    ));
                                }
                                open = Some(d.parent);
                            }
                            None => log.push((false, "Return: already at the top cell".into())),
                        },
                        LayRequest::OpenRuleDesigner => self.queue.push(Command::RuleDesigner),
                        LayRequest::UpdateFromSource(remove) => self.queue.push(Command::UpdateFromSource(remove)),
                        LayRequest::EditLayerDisplay(name) => self.queue.push(Command::LayerDisplay(Some(name))),
                        LayRequest::SetDeck(text) => match project.set_drc_deck(text) {
                            Ok(msg) => {
                                changed = true;
                                log.push((true, msg));
                            }
                            Err(e) => log.push((false, e)),
                        },
                        LayRequest::ReplaceWithPCell { shape, pcell } => match project.ensure_pcell(&pcell) {
                            Ok(cr) => {
                                ed.history.push(&lay);
                                lay.remove(shape);
                                ed.selection = [lay.add_instance(cr, ics_core::Trans::IDENTITY, None)].into();
                                touched = true;
                            }
                            Err(e) => log.push((false, format!("PCell: {e}"))),
                        },
                        LayRequest::PlacePCell { pcell, trans, array } => match project.ensure_pcell(&pcell) {
                            Ok(cr) => {
                                ed.history.push(&lay);
                                ed.selection = [lay.add_instance(cr, trans, array)].into();
                                touched = true;
                            }
                            Err(e) => log.push((false, format!("PCell: {e}"))),
                        },
                        LayRequest::SetPCell { inst, pcell, trans } => match project.ensure_pcell(&pcell) {
                            Ok(cr) => {
                                ed.history.push(&lay);
                                if let Some(i) = lay.instance_mut(inst) {
                                    i.cell = cr;
                                    i.trans = trans.unwrap_or(i.trans);
                                    touched = true;
                                }
                            }
                            Err(e) => log.push((false, format!("PCell: {e}"))),
                        },
                        LayRequest::MakeCell { ids, origin } => {
                            let mut k = 1;
                            let name = loop {
                                let n = format!("{}_sub{k}", r.cell);
                                if project.cell(&CellRef::new(&r.lib, &n)).is_none() {
                                    break n;
                                }
                                k += 1;
                            };
                            ed.history.push(&lay);
                            let mut c = Cell::new(&name);
                            c.description = format!("Extracted from {}", r.cell);
                            c.layout = Some(lay.extract(&ids, origin));
                            match project.add_cell(&r.lib, c) {
                                Ok(cr) => {
                                    log.push((true, format!("Moved {} item(s) into new cell {cr}", ids.len())));
                                    ed.selection = [lay.add_instance(cr, ics_core::Trans::new(origin.0, origin.1), None)].into();
                                    touched = true;
                                }
                                Err(e) => {
                                    ed.history.undo(&mut lay);
                                    log.push((false, e));
                                }
                            }
                        }
                    }
                }
                if touched {
                    ed.layout_changed();
                    self.hier.invalidate(&r);
                    changed = true;
                }
                if let Some((c, v)) = handoff {
                    self.lay_eds.entry(c).or_default().adopt_view(v);
                }
                if let Some(c) = refit {
                    self.lay_eds.entry(c).or_default().request_fit();
                }
                if let Some(c) = project.cell_mut(&r) {
                    c.layout = Some(lay);
                }
                for (ok, m) in log {
                    if ok {
                        self.ok(m);
                    } else {
                        self.error(m);
                    }
                }
                if let Some(o) = open {
                    self.queue.push(Command::OpenCell(o, Some(Workspace::Layout)));
                }
            }
            Workspace::Simulation => {
                if cell.schematic.is_none() {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::new().fill(pal.bg_canvas))
                        .show(ui, |ui| self.no_view_placeholder(ui, &r, ViewKind::Schematic));
                    return;
                }
                let st = self.sims.entry(r.clone()).or_default();
                let mut run = false;
                let mut tool = None;
                theme::aside(egui::Panel::left("sim_setup").resizable(true).default_size(270.0), ui, |ui| {
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        for (label, hint, cmd) in [
                            ("Sweep...", "Corner and parametric sweeps", Command::SweepManager),
                            ("Monte Carlo...", "Process and mismatch statistics", Command::MonteCarlo),
                            ("Optimizer...", "Size devices to meet specs", Command::Optimizer),
                            ("Verify...", "Run every test of the project and check its specs", Command::Verification),
                        ] {
                            if ui.small_button(label).on_hover_text(hint).clicked() {
                                tool = Some(cmd);
                            }
                        }
                    });
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let (c, rr) = sim_view::setup_panel(ui, cell, st, &self.ngspice, tech.models.as_ref());
                        changed |= c;
                        run = rr;
                    });
                });
                if run {
                    self.queue.push(Command::RunSimulation);
                }
                self.queue.extend(tool);
                let deck = st.show_netlist.then(|| {
                    let nl = ics_core::netlist::netlist_for(cell.schematic.as_ref().unwrap(), &tech, &cell.sim.corner);
                    nl.deck(&r.to_string(), &cell.sim)
                });
                if let Some(deck) = deck {
                    theme::aside(egui::Panel::right("sim_netlist").resizable(true).default_size(340.0), ui, |ui| {
                        theme::section(ui, t("Netlist"));
                        egui::ScrollArea::both().show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(deck).monospace().size(11.5)).selectable(true));
                        });
                    });
                }
                let mut msg = None;
                egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg_canvas).inner_margin(8)).show(ui, |ui| {
                    msg = sim_view::results_panel(ui, st);
                });
                if let Some(m) = msg {
                    self.info(m);
                }
            }
            Workspace::Home => unreachable!(),
        }
        if changed {
            self.mark_dirty();
        }
    }

    pub(super) fn status_bar(&mut self, ui: &mut egui::Ui) {
        let pal = theme::palette(ui.ctx());
        // The right side (ngspice, PDK, console, version) keeps its room: the left readouts are
        // clipped to what is left of it, using the width the right side took last frame.
        let avail = ui.available_rect_before_wrap();
        let key = egui::Id::unique("status_right_width");
        let reserve = ui.ctx().data(|d| d.get_temp::<f32>(key)).unwrap_or(360.0);
        let left_rect =
            egui::Rect::from_min_max(avail.min, egui::pos2((avail.max.x - reserve - 16.0).max(avail.min.x), avail.max.y));
        let mut left =
            ui.new_child(egui::UiBuilder::new().max_rect(left_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
        left.shrink_clip_rect(left_rect);
        {
            let ui = &mut left;
            let c = self.workspace.color(&pal);
            let (r, p) = ui.allocate_painter(Vec2::splat(14.0), Sense::hover());
            icons::paint(&p, r.rect, self.workspace.icon(), c);
            ui.label(RichText::new(self.workspace.name()).small().color(c));
            ui.separator();
            if let Some(r) = &self.active {
                match self.workspace {
                    Workspace::Schematic => {
                        if let Some(ed) = self.sch_eds.get(r) {
                            if let Some(c) = ed.cursor {
                                ui.label(RichText::new(format!("x {}  y {}", c.x, c.y)).small().monospace());
                            }
                            ui.label(
                                RichText::new(tf("zoom {}%", &[&format!("{:.0}", ed.view.scale / 22.0 * 100.0)]))
                                    .small()
                                    .color(pal.text_weak),
                            );
                            ui.label(
                                RichText::new(tf("{} selected", &[&ed.selection.len().to_string()])).small().color(pal.text_weak),
                            );
                            if matches!(ed.tool, SchTool::Wire) {
                                ui.label(
                                    RichText::new(t("click to route · / flips corner · Esc ends")).small().color(pal.text_weak),
                                );
                            }
                        }
                    }
                    Workspace::Layout => {
                        if let (Some(ed), Some(p)) = (self.lay_eds.get(r), &self.project) {
                            if let Some(c) = ed.cursor {
                                ui.label(
                                    RichText::new(format!("X: {}  Y: {}", um(c.0, &p.tech), um(c.1, &p.tech)))
                                        .small()
                                        .monospace(),
                                );
                                if let Some(a) = ed.anchor() {
                                    let (dx, dy) = (c.0 - a.0, c.1 - a.1);
                                    let dist = ((dx as f64).hypot(dy as f64)).round() as i64;
                                    ui.label(
                                        RichText::new(format!(
                                            "dX: {}  dY: {}  Dist: {}",
                                            um(dx, &p.tech),
                                            um(dy, &p.tech),
                                            um(dist, &p.tech)
                                        ))
                                        .small()
                                        .monospace()
                                        .color(pal.text_weak),
                                    );
                                }
                            }
                            let sel = if ed.partial_mode || !ed.part.is_empty() {
                                format!("Sel(P): {}", ed.part.len())
                            } else {
                                format!("Sel: {}", ed.selection.len())
                            };
                            ui.label(RichText::new(sel).small().monospace().color(pal.text_weak));
                            ui.label(RichText::new(format!("Cmd: {}", ed.tool.name())).small().monospace().color(pal.layout));
                            ui.separator();
                            let [ml, mm, mr] = ed.mouse_hints();
                            ui.label(
                                RichText::new(tf("mouse L: {}   M: {}   R: {}", &[t(ml), t(mm), t(mr)]))
                                    .small()
                                    .color(pal.text_weak),
                            )
                            .on_hover_text(t("What each mouse button does now"));
                            let s = ed.stats;
                            let txt = format!(
                                "{}{}",
                                tf("{} shapes drawn", &[&fmt_count(s.drawn as u64)]),
                                if s.truncated { format!(" {}", t("(limit)")) } else { String::new() }
                            );
                            ui.label(RichText::new(txt).small().color(if s.truncated { pal.warning } else { pal.text_weak }));
                            if let Some((what, t)) = ed.verify.running() {
                                ui.separator();
                                ui.add(egui::Spinner::new().size(12.0));
                                ui.label(
                                    RichText::new(tf(
                                        "Running {}… {} s",
                                        &[crate::i18n::t(what.name()), &format!("{:.1}", t.as_secs_f64())],
                                    ))
                                    .small(),
                                );
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        let left_end = left.min_rect().max.x.min(left_rect.max.x);
        let right_rect = egui::Rect::from_min_max(egui::pos2(left_end + 8.0, avail.min.y), avail.max);
        let mut right =
            ui.new_child(egui::UiBuilder::new().max_rect(right_rect).layout(egui::Layout::right_to_left(egui::Align::Center)));
        {
            let ui = &mut right;
            {
                ui.label(RichText::new(format!("v{}", env!("CARGO_PKG_VERSION"))).small().color(pal.text_weak));
                if let Some(r) = self.updater.available() {
                    let txt = match self.updater.state {
                        update::State::Installed { .. } => t("Restart to update").to_string(),
                        _ => tf("Update {} available", &[&r.version]),
                    };
                    if ui.link(RichText::new(txt).small().color(pal.accent)).clicked() {
                        self.queue.push(Command::CheckForUpdates);
                    }
                }
                ui.separator();
                if let Some(b) = &self.bridge {
                    let hover = match &b.last {
                        Some((t, tool)) => tf(
                            "{} tool call(s) from ics mcp; last: {}, {} s ago",
                            &[&b.calls.to_string(), tool, &format!("{:.0}", t.elapsed().as_secs_f64())],
                        ),
                        None => t("Waiting for tool calls from ics mcp (Claude Code)").to_string(),
                    };
                    ui.label(RichText::new(t("● MCP live")).small().color(pal.ok)).on_hover_text(hover);
                    ui.separator();
                    if let Some(j) = &b.job {
                        let queued = if b.waiting.is_empty() {
                            String::new()
                        } else {
                            format!(" {}", tf("({} queued)", &[&b.waiting.len().to_string()]))
                        };
                        let txt = tf(
                            "MCP: {} running… {} s{}",
                            &[&j.tool, &format!("{:.1}", j.started.elapsed().as_secs_f64()), &queued],
                        );
                        ui.label(RichText::new(txt).small());
                        ui.add(egui::Spinner::new().size(12.0));
                        ui.separator();
                    }
                }
                let (txt, col) = match &self.ngspice {
                    Some(v) => (format!("● {v}"), pal.ok),
                    None => (t("○ ngspice not found").to_string(), pal.text_weak),
                };
                ui.label(RichText::new(txt).small().color(col));
                ui.separator();
                if let Some(p) = &self.project {
                    ui.label(RichText::new(format!("PDK {}", p.tech.name)).small().color(pal.text_weak));
                    ui.separator();
                }
                if ui
                    .add(
                        egui::Label::new(RichText::new(if self.show_console { t("▾ Console") } else { t("▸ Console") }).small())
                            .sense(Sense::click()),
                    )
                    .clicked()
                {
                    self.show_console = !self.show_console;
                }
                let fixed = avail.max.x - ui.min_rect().min.x;
                ui.ctx().data_mut(|d| d.insert_temp(key, fixed));
                // A warning the hidden console would swallow: show it here for a few seconds.
                let console_shown = self.show_console && self.bottom_tab == BottomTab::Console;
                if let Some((text, at)) = &self.flash
                    && !console_shown
                {
                    let age = at.elapsed();
                    if age < FLASH_TIME {
                        let l = ui
                            .add(
                                egui::Label::new(RichText::new(format!("⚠ {text}")).small().color(pal.warning))
                                    .truncate()
                                    .sense(Sense::click()),
                            )
                            .on_hover_text(t("Click to show the console"));
                        if l.clicked() {
                            self.show_console = true;
                            self.bottom_tab = BottomTab::Console;
                        }
                        ui.ctx().request_repaint_after(FLASH_TIME - age);
                    }
                }
            }
        }
        ui.advance_cursor_after_rect(avail);
    }

    pub(super) fn console(&mut self, ui: &mut egui::Ui) {
        let pal = theme::palette(ui.ctx());
        let findings = self.active_findings();
        let problems: Vec<String> = self
            .netlist_active()
            .map(|(_, w)| w)
            .unwrap_or_default()
            .into_iter()
            .filter(|w| !w.starts_with("Unconnected pin") && !w.starts_with("Duplicate instance name"))
            .collect();
        let count = problems.len() + findings.iter().filter(|f| f.severity != ics_core::sch_check::Severity::Info).count();
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.bottom_tab, BottomTab::Console, t("Console"));
            if ui
                .selectable_value(&mut self.bottom_tab, BottomTab::Ciw, "CIW")
                .on_hover_text(t("SKILL command interpreter"))
                .clicked()
            {
                self.ciw.request_focus();
            }
            let ptxt = if count == 0 { t("Problems").to_string() } else { tf("Problems ({})", &[&count.to_string()]) };
            ui.selectable_value(&mut self.bottom_tab, BottomTab::Problems, ptxt);
            ui.selectable_value(&mut self.bottom_tab, BottomTab::Netlist, t("Netlist"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("×").clicked() {
                    self.show_console = false;
                }
                if self.bottom_tab == BottomTab::Console && ui.small_button(t("Clear")).clicked() {
                    self.log.clear();
                }
                if self.bottom_tab == BottomTab::Ciw && ui.small_button(t("Clear")).clicked() {
                    self.ciw.clear();
                }
            });
        });
        ui.separator();
        match self.bottom_tab {
            BottomTab::Ciw => {
                if let Some(src) = self.ciw.ui(ui) {
                    self.queue.push(Command::CiwEval(src));
                }
            }
            BottomTab::Console => {
                egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                    for l in &self.log {
                        let (tag, c) = match l.level {
                            Level::Info => ("INFO", pal.text_weak),
                            Level::Ok => ("DONE", pal.ok),
                            Level::Warn => ("WARN", pal.warning),
                            Level::Error => ("FAIL", pal.danger),
                        };
                        ui.horizontal_top(|ui| {
                            ui.label(
                                RichText::new(format!("{:>7.2}s", l.t))
                                    .monospace()
                                    .small()
                                    .color(pal.text_weak.gamma_multiply(0.7)),
                            );
                            ui.label(RichText::new(tag).monospace().small().color(c));
                            ui.add(egui::Label::new(RichText::new(&l.text).monospace().size(12.0)).wrap());
                        });
                    }
                });
            }
            BottomTab::Problems => {
                let mut pick = None;
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    if problems.is_empty() && findings.is_empty() {
                        ui.label(RichText::new(t("No problems detected in the active schematic.")).color(pal.text_weak));
                    }
                    for f in &findings {
                        use ics_core::sch_check::Severity;
                        let (tag, c) = match f.severity {
                            Severity::Error => ("ERROR", pal.danger),
                            Severity::Warning => ("WARN", pal.warning),
                            Severity::Info => ("INFO", pal.text_weak),
                        };
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(tag).monospace().small().color(c));
                            ui.label(RichText::new(f.code).monospace().small().color(pal.text_weak));
                            let r = ui.add(
                                egui::Label::new(RichText::new(crate::i18n::t_text(&f.message)).color(c))
                                    .sense(egui::Sense::click()),
                            );
                            if r.on_hover_text(t("Select in the schematic")).clicked() {
                                pick = Some(f.instances.clone());
                            }
                        });
                    }
                    for w in problems {
                        ui.label(RichText::new(format!("⚠ {w}")).color(pal.warning));
                    }
                });
                if let Some(ids) = pick
                    && let Some(r) = self.active.clone()
                {
                    self.sch_eds.entry(r).or_default().selection = ids.into_iter().map(SchObj::Instance).collect();
                    self.workspace = Workspace::Schematic;
                }
            }
            BottomTab::Netlist => {
                let deck = self.netlist_active().map(|d| d.0).unwrap_or_else(|| "* active cell has no schematic".into());
                egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(deck).monospace().size(12.0)).selectable(true));
                });
            }
        }
    }

    pub(super) fn palette_commands(&self) -> Vec<(String, &'static str, Command)> {
        let examples =
            examples::TOPICS.iter().map(|t| (tf("Open example: {}", &[t.title]), "", Command::OpenExample(t.name.into())));
        let mut v: Vec<(String, &'static str, Command)> = vec![
            (t("Run simulation").into(), "F5", Command::RunSimulation),
            (t("Stop simulation").into(), "", Command::StopSimulation),
            (t("Undo").into(), "⌘Z", Command::Undo),
            (t("Redo").into(), "⇧⌘Z", Command::Redo),
            (t("Delete selection").into(), "Del", Command::Delete),
            (t("Close all tabs").into(), "", Command::CloseAllTabs),
            (t("Close project").into(), "", Command::CloseProject),
            (t("Quit").into(), "⌘Q", Command::Quit),
            (t("Save project").into(), "⌘S", Command::Save),
            (t("Save project as…").into(), "⇧⌘S", Command::SaveAs),
            (t("Open project…").into(), "⌘O", Command::OpenProjectDialog),
            (t("New project…").into(), "", Command::NewProjectDialog),
            (t("New demo project").into(), "", Command::NewDemoProject),
            (t("Save example projects to a folder…").into(), "", Command::SaveExamples),
            (t("New cell…").into(), "⌘N", Command::NewCellDialog),
            (t("Run DRC").into(), "", Command::RunDrc),
            (t("Extract nets").into(), "", Command::ExtractNets),
            (t("Schematic from layout (pixels: PD / TG / RST / SF / SEL)").into(), "", Command::SchematicFromLayout),
            (t("Pixel circuit: conversion gain, full well, lag, noise, dark current").into(), "", Command::PixelCircuit),
            (t("Run LVS").into(), "", Command::RunLvs),
            (t("IR drop / EM analysis").into(), "", Command::IrDrop(None)),
            (t("DRC rule designer").into(), "", Command::RuleDesigner),
            (t("PCell designer").into(), "", Command::PCellDesigner),
            (t("PCell designer: make a PCell from the active layout").into(), "", Command::PCellFromActive),
            (t("MOS symbol: body pin (4-terminal)").into(), "", Command::MosSymbol(true)),
            (t("MOS symbol: arrow on source, body hidden (3-terminal)").into(), "", Command::MosSymbol(false)),
            (t("DRC deck: import from a file").into(), "", Command::ImportDrcDeck(None)),
            (t("DRC deck: export to a file").into(), "", Command::ExportDrcDeck(None)),
            (t("DRC deck: link a shared deck file (common to several projects)").into(), "", Command::LinkDrcDeck(None)),
            (t("DRC deck: reload the shared deck").into(), "", Command::ReloadDrcDeck),
            (t("DRC deck: unlink the shared deck").into(), "", Command::UnlinkDrcDeck),
            (t("Stream layer map (GDSII / OASIS layer numbers)").into(), "", Command::StreamMap),
            (t("Layer display (fill, stipple, outline, line style)").into(), "", Command::LayerDisplay(None)),
            (t("Dummy fill (metal density)").into(), "", Command::DummyFill),
            (t("OPC (optical proximity correction)").into(), "", Command::Opc),
            (t("Waveform calculator").into(), "", Command::Calculator),
            (t("Pixel optics (QE, crosstalk, photocurrent)").into(), "", Command::PixelOptics),
            (t("Run pixel optics model").into(), "", Command::RunPixelOptics),
            (t("Pixel device model into the technology's photodiode").into(), "", Command::ApplyPixelDevice),
            (t("Load measured QE…").into(), "", Command::LoadMeasuredQe(None)),
            (t("Fit pixel stack to measured QE").into(), "", Command::FitPixelStack),
            (t("Pixel stack of the open cell from the PDK cross section").into(), "", Command::PixelStackFromPdk),
            (t("Corner / sweep manager").into(), "", Command::SweepManager),
            (t("Verification (all tests)").into(), "", Command::Verification),
            (t("Run verification").into(), "", Command::RunVerification),
            (t("Monte Carlo (process and mismatch of device models)").into(), "", Command::MonteCarlo),
            (t("Run Monte Carlo").into(), "", Command::RunMonteCarlo),
            (t("Silicon correlation (measured curves, model fitting)").into(), "", Command::SiliconCorrelation),
            (t("Process emulation (cross-section of the layout)").into(), "", Command::ProcessEmulation),
            (t("Process data (WAT / PCM, SPC, corners)").into(), "", Command::ProcessData),
            (t("Fit the correlation parameters to the measurements").into(), "", Command::FitSilicon),
            (t("Stop Monte Carlo").into(), "", Command::StopMonteCarlo),
            (t("Optimizer (size devices to meet specs)").into(), "", Command::Optimizer),
            (t("Results browser (compare runs)").into(), "", Command::ResultsBrowser),
            (t("Check schematic").into(), "", Command::CheckSchematic(false)),
            (t("Place cell instance (hierarchical schematic)").into(), "Shift+I", Command::PlaceCellDialog),
            (t("Return to the parent schematic").into(), "Shift+B", Command::ReturnSchematic),
            (t("Update cell instance pins").into(), "", Command::UpdateCellInstances),
            (t("Edit symbol pins (side and order)").into(), "", Command::EditSymbolDialog),
            (t("Check and save").into(), "", Command::CheckSchematic(true)),
            (t("Net browser").into(), "", Command::NetBrowser),
            (t("Hierarchy browser").into(), "", Command::HierarchyBrowser),
            (t("Create guard ring").into(), "", Command::GuardRing),
            (t("Create via array").into(), "", Command::ViaArray),
            (t("Matched placement (common centroid / interdigitated)").into(), "", Command::MatchedPlacement),
            (t("Run corner / parametric sweep").into(), "", Command::RunSweep),
            (t("Run sign-off runset (KLayout / Calibre / netgen)").into(), "", Command::RunSignoff(None)),
            (t("Import sign-off results (Calibre / KLayout / netgen)").into(), "", Command::ImportSignoffResults(None)),
            (t("Generate layout from schematic").into(), "", Command::GenerateLayout),
            (t("Route flylines").into(), "", Command::RouteFlylines),
            (t("Check layout against source (schematic)").into(), "", Command::CheckAgainstSource),
            (t("Update components and nets from schematic").into(), "", Command::UpdateFromSource(false)),
            (t("Update from schematic and delete extra devices").into(), "", Command::UpdateFromSource(true)),
            (t("Export cell GDSII…").into(), "", Command::ExportGds),
            (t("Export library GDSII…").into(), "", Command::ExportLibraryGds),
            (t("Export cell OASIS…").into(), "", Command::ExportOasis),
            (t("Export library OASIS…").into(), "", Command::ExportLibraryOasis),
            (t("Import GDSII / OASIS…").into(), "", Command::ImportGds(None)),
            (t("Import GDSII / OASIS with Virtuoso layer map…").into(), "", Command::ImportGdsWithLayerMap(None, None)),
            (t("Export cell LEF abstract…").into(), "", Command::ExportLefDef(false)),
            (t("Export cell DEF…").into(), "", Command::ExportLefDef(true)),
            (t("Import LEF abstracts…").into(), "", Command::ImportLefDef(false, None)),
            (t("Import DEF placement…").into(), "", Command::ImportLefDef(true, None)),
            (t("Import technology (KLayout .lyp)…").into(), "", Command::ImportTech),
            (t("Load PDK…").into(), "", Command::LoadPdk(None)),
            (t("Load SKILL (.il)…").into(), "", Command::LoadSkill(None)),
            (t("Manage extensions…").into(), "", Command::Extensions),
            (t("Reload extensions").into(), "", Command::ReloadExtensions),
            (t("SKILL command interpreter (CIW)").into(), "", Command::Ciw),
            (t("Design copilot (AI chat)").into(), "", Command::Copilot),
            (t("AI models (provider, model, API key)…").into(), "", Command::AiModels),
            (t("Export SPICE netlist…").into(), "", Command::ExportNetlist),
            (t("Import CDL netlist (Virtuoso)…").into(), "", Command::ImportCdl(None)),
            (t("Import Virtuoso library (dump or cds.lib)…").into(), "", Command::ImportVirtuoso(None)),
            (t("Save Virtuoso dump script (icsDump.il)…").into(), "", Command::SaveDumpSkill(None)),
            (t("Export Virtuoso schematic (SKILL)…").into(), "", Command::ExportSkill),
            (t("Toggle light / dark theme").into(), "", Command::ToggleTheme),
            (t("Toggle console").into(), "⌘J", Command::ToggleConsole),
            (t("Toggle explorer").into(), "⌘B", Command::ToggleExplorer),
            (t("Fit view").into(), "F", Command::FitView),
            (t("Settings").into(), "⌘,", Command::Settings),
            (t("Keyboard shortcuts").into(), "F1", Command::Shortcuts),
            (t("Start the interactive tutorial").into(), "", Command::Tutorial),
            (t("Learn IC Design…").into(), "", Command::Learn(None)),
            (t("Learn progress report…").into(), "", Command::ExportLearnReport),
            (t("Reload the project's course chapters").into(), "", Command::ReloadProjectChapters),
            (t("Open user guide (web)").into(), "", Command::UserGuide),
            (t("About IC Studio").into(), "", Command::About),
            (t("Check for updates").into(), "", Command::CheckForUpdates),
            (t("Add IC Studio MCP server to Claude Code").into(), "", Command::AddMcpToClaudeCode),
            (t("Copy MCP server config").into(), "", Command::CopyMcpConfig),
        ];
        v.extend(examples);
        if let Some(r) = &self.active {
            let name = r.to_string();
            v.push((tf("Close tab {}", &[&name]), "⌘W", Command::CloseTab(r.clone())));
            v.push((t("Close the other tabs").into(), "", Command::CloseOtherTabs(r.clone())));
            v.push((tf("Rename {}…", &[&name]), "", Command::RenameCellDialog(r.clone())));
            v.push((tf("Delete {}…", &[&name]), "", Command::DeleteCellConfirm(r.clone())));
            if let Some(c) = self.active_cell() {
                for (vk, label) in
                    [(ViewKind::Schematic, "Create the schematic view of {}"), (ViewKind::Layout, "Create the layout view of {}")]
                {
                    if !c.has_view(vk) {
                        v.push((tf(label, &[&name]), "", Command::CreateView(r.clone(), vk)));
                    }
                }
            }
            v.push((
                tf("Copy {} as a new cell (start a new design from it)…", &[&r.to_string()]),
                "",
                Command::CopyCellDialog(r.clone()),
            ));
            v.push((
                tf("Derive a new design from {} (new targets, sized and laid out)…", &[&r.to_string()]),
                "",
                Command::DeriveDialog(r.clone()),
            ));
        }
        if crate::i18n::korean_available() {
            v.push((t("Language: English").into(), "", Command::SetLanguage(crate::i18n::Lang::En)));
            v.push((
                tf("Language: Korean ({})", &[crate::i18n::Lang::Ko.label()]),
                "",
                Command::SetLanguage(crate::i18n::Lang::Ko),
            ));
        }
        v.push(if self.settings.beginner {
            (t("Beginner mode: off (show every tool in the menus)").into(), "", Command::BeginnerMode(false))
        } else {
            (t("Beginner mode: on (menus with the everyday design flow)").into(), "", Command::BeginnerMode(true))
        });
        for (e, c) in self.extensions.commands() {
            v.push((format!("{}: {}", e.manifest.extension.name, c.title), "", Command::RunExtension(e.id().to_string(), c.id)));
        }
        for (label, file) in self.extensions.pdks() {
            v.push((tf("Load PDK: {}", &[&label]), "", Command::LoadPdk(Some(file))));
        }
        if self.workspace == Workspace::Schematic && self.active.is_some() {
            for a in SchAction::PALETTE {
                v.push((tf("Schematic: {}", &[t(&a.name())]), "", Command::Sch(*a)));
            }
        }
        if self.workspace == Workspace::Layout && self.active.is_some() {
            for (menu, items) in LayAction::MENUS {
                for a in items.iter().flatten() {
                    v.push((tf("Layout {}: {}", &[t(menu), t(a.name())]), a.key(), Command::Lay(*a)));
                }
            }
        }
        for (i, w) in Workspace::ALL.iter().enumerate() {
            let sc: &'static str = ["⌘1", "⌘2", "⌘3", "⌘4"][i];
            v.push((tf("Go to {} workspace", &[t(w.name())]), sc, Command::SetWorkspace(*w)));
        }
        if let Some(p) = &self.project
            && let Some(lib) = self.palette_library()
            && let Some(l) = p.libraries.iter().find(|l| l.name == lib)
        {
            let name = |what: &str| tf("Library {}: {}", &[&lib, t(what)]);
            v.push((name("new cell…"), "", Command::NewCellInLibrary(lib.clone())));
            v.push((name("export GDSII…"), "", Command::ExportLibraryStream(lib.clone(), StreamFormat::Gds)));
            v.push((name("export OASIS…"), "", Command::ExportLibraryStream(lib.clone(), StreamFormat::Oasis)));
            let (storage, how) = match l.storage {
                LibStorage::Json => (LibStorage::Gds, "store as one GDSII file"),
                LibStorage::Gds => (LibStorage::Json, "store as JSON cell directories"),
            };
            v.push((name(how), "", Command::SetLibraryStorage(lib.clone(), storage)));
            v.push((name("rename…"), "", Command::RenameLibraryDialog(lib.clone())));
            v.push((name("delete…"), "", Command::DeleteLibraryConfirm(lib.clone())));
            for t in p.library_tops(&lib) {
                v.push((
                    tf("Library {}: open top cell {} · layout", &[&lib, &t.cell]),
                    "",
                    Command::OpenCell(t, Some(Workspace::Layout)),
                ));
            }
        }
        if let Some(p) = &self.project {
            for (r, c) in p.all_cells() {
                if c.schematic.is_some() {
                    v.push((
                        tf("Open {} · schematic", &[&r.to_string()]),
                        "",
                        Command::OpenCell(r.clone(), Some(Workspace::Schematic)),
                    ));
                }
                if c.layout.is_some() {
                    v.push((
                        tf("Open {} · layout", &[&r.to_string()]),
                        "",
                        Command::OpenCell(r.clone(), Some(Workspace::Layout)),
                    ));
                }
            }
        }
        v
    }

    /// The library that library commands of the palette act on: the one selected in the
    /// Explorer, else the active cell's.
    pub(super) fn palette_library(&self) -> Option<String> {
        let p = self.project.as_ref()?;
        self.selected_lib
            .clone()
            .filter(|l| p.libraries.iter().any(|x| &x.name == l))
            .or_else(|| self.active.as_ref().map(|r| r.lib.clone()))
    }

    pub(super) fn command_palette(&mut self, ctx: &egui::Context) {
        let Some(mut st) = self.palette.take() else { return };
        let pal = theme::palette(ctx);
        let q = st.query.to_lowercase();
        let items: Vec<(String, &'static str, Command)> = self
            .palette_commands()
            .into_iter()
            .filter(|(n, _, _)| q.split_whitespace().all(|w| n.to_lowercase().contains(w)))
            .collect();
        let mut close = false;
        let mut chosen: Option<Command> = None;
        let (up, down, enter, esc) = ctx.input(|i| {
            (i.key_pressed(Key::ArrowUp), i.key_pressed(Key::ArrowDown), i.key_pressed(Key::Enter), i.key_pressed(Key::Escape))
        });
        if down {
            st.sel = (st.sel + 1).min(items.len().saturating_sub(1));
        }
        if up {
            st.sel = st.sel.saturating_sub(1);
        }
        if esc {
            close = true;
        }
        if enter && let Some(it) = items.get(st.sel) {
            chosen = Some(it.2.clone());
        }
        let modal = egui::Modal::new(egui::Id::unique("cmd_palette")).show(ctx, |ui| {
            ui.set_width(520.0);
            let r = ui.add(
                egui::TextEdit::singleline(&mut st.query)
                    .hint_text(t("Type a command or cell name…"))
                    .desired_width(f32::INFINITY)
                    .font(FontId::proportional(15.0)),
            );
            if !r.has_focus() {
                r.request_focus();
            }
            // Scroll to the highlighted row only when it moved: every frame would undo the wheel.
            let mut moved = up || down;
            if r.changed() {
                st.sel = 0;
                moved = true;
            }
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                for (i, (name, sc, cmd)) in items.iter().enumerate() {
                    let selected = i == st.sel;
                    let (resp, p) = ui.allocate_painter(Vec2::new(ui.available_width(), 26.0), Sense::click());
                    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, name));
                    if selected || resp.hovered() {
                        p.rect_filled(resp.rect, 5.0, if selected { pal.accent.gamma_multiply(0.3) } else { pal.bg_hover });
                    }
                    p.text(
                        resp.rect.left_center() + Vec2::new(10.0, 0.0),
                        Align2::LEFT_CENTER,
                        name,
                        FontId::proportional(13.5),
                        pal.text,
                    );
                    p.text(
                        resp.rect.right_center() - Vec2::new(10.0, 0.0),
                        Align2::RIGHT_CENTER,
                        theme::kbd(sc),
                        FontId::monospace(11.5),
                        pal.text_weak,
                    );
                    if selected && moved {
                        resp.scroll_to_me(None);
                    }
                    if resp.clicked() {
                        chosen = Some(cmd.clone());
                    }
                }
                if items.is_empty() {
                    ui.label(RichText::new(t("No matching commands")).color(pal.text_weak));
                }
            });
        });
        if modal.should_close() {
            close = true;
        }
        if let Some(c) = chosen {
            self.queue.push(c);
            close = true;
        }
        if !close {
            self.palette = Some(st);
        }
    }
}
