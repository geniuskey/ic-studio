//! Modal dialogs.

use super::*;

impl IcStudio {
    pub(super) fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut dlg) = self.dialog.take() else { return };
        let pal = theme::palette(ctx);
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("dialog")).show(ctx, |ui| {
            ui.set_min_width(380.0);
            match &mut dlg {
                Dialog::ImportDroppedLayout(paths) => {
                    ui.heading(t("Import GDSII / OASIS"));
                    egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                        for path in paths.iter() {
                            ui.label(path.file_name().unwrap_or_default().to_string_lossy());
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button(t("Import")).clicked() {
                            self.queue.extend(paths.drain(..).map(|path| Command::ImportGds(Some(path))));
                            close = true;
                        }
                        if ui.button(t("Cancel")).clicked() {
                            close = true;
                        }
                    });
                }
                Dialog::NewProject { name, dir, demo, error } => {
                    ui.heading(t("New project"));
                    ui.add_space(8.0);
                    egui::Grid::new("np").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                        ui.label(t("Name"));
                        ui.text_edit_singleline(name);
                        ui.end_row();
                        ui.label(t("Location"));
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(dir).desired_width(220.0));
                            if ui.button(t("…")).clicked()
                                && let Some(d) = rfd::FileDialog::new().pick_folder()
                            {
                                *dir = d.display().to_string();
                            }
                        });
                        ui.end_row();
                        ui.label(t("Template"));
                        ui.horizontal(|ui| {
                            ui.radio_value(demo, false, t("Empty"));
                            ui.radio_value(demo, true, t("Demo designs"));
                        });
                        ui.end_row();
                    });
                    ui.label(
                        RichText::new(tf("Will be created at {}", &[&Path::new(dir.as_str()).join(name.as_str()).display().to_string()]))
                            .small()
                            .color(pal.text_weak),
                    );
                    if !error.is_empty() {
                        ui.colored_label(pal.danger, error.as_str());
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(t("Cancel")).clicked() {
                            close = true;
                        }
                        if ui.button(RichText::new(t("Create")).strong()).clicked() {
                            if !ics_core::project::valid_name(name) {
                                *error = "Name may contain letters, digits, _ and - only".into();
                            } else {
                                let root = Path::new(dir.as_str()).join(name.as_str());
                                if root.join(ics_core::project::MANIFEST).exists() {
                                    *error = "A project already exists there".into();
                                } else {
                                    let mut p = if *demo { examples::demo_project() } else { Project::new(name.as_str()) };
                                    p.name = name.clone();
                                    match p.save_as(&root) {
                                        Ok(()) => {
                                            self.set_project(p);
                                            self.remember_recent(&root);
                                            self.ok(format!("Created project at {}", root.display()));
                                            close = true;
                                        }
                                        Err(e) => *error = e.to_string(),
                                    }
                                }
                            }
                        }
                    });
                }
                Dialog::NewCell { lib, name, description, schematic, layout, error } => {
                    ui.heading(t("New cell"));
                    ui.add_space(8.0);
                    egui::Grid::new("nc").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                        ui.label(t("Library"));
                        egui::ComboBox::from_id_salt("nc_lib").selected_text(lib.as_str()).show_ui(ui, |ui| {
                            if let Some(p) = &self.project {
                                for l in &p.libraries {
                                    ui.selectable_value(lib, l.name.clone(), &l.name);
                                }
                            }
                        });
                        ui.end_row();
                        ui.label(t("Cell name"));
                        let r = ui.text_edit_singleline(name);
                        if name.is_empty() && !r.has_focus() {
                            r.request_focus();
                        }
                        ui.end_row();
                        ui.label(t("Description"));
                        ui.text_edit_singleline(description);
                        ui.end_row();
                        ui.label(t("Views"));
                        ui.horizontal(|ui| {
                            ui.checkbox(schematic, t("schematic"));
                            ui.checkbox(layout, t("layout"));
                        });
                        ui.end_row();
                    });
                    if !error.is_empty() {
                        ui.colored_label(pal.danger, error.as_str());
                    }
                    ui.add_space(8.0);
                    let enter = ui.input(|i| i.key_pressed(Key::Enter));
                    ui.horizontal(|ui| {
                        if ui.button(t("Cancel")).clicked() {
                            close = true;
                        }
                        if ui.button(RichText::new(t("Create")).strong()).clicked() || enter {
                            let mut c = Cell::new(name.trim());
                            c.description = description.clone();
                            if *schematic {
                                c.schematic = Some(Schematic::default());
                            }
                            if *layout {
                                c.layout = Some(Layout::default());
                            }
                            let ws = if *schematic { Workspace::Schematic } else { Workspace::Layout };
                            match self.project.as_mut().map(|p| {
                                if !p.libraries.iter().any(|l| l.name == *lib) {
                                    p.add_library(lib)?;
                                }
                                p.add_cell(lib, c)
                            }) {
                                Some(Ok(r)) => {
                                    self.info(format!("Created cell {r}"));
                                    self.dirty = true;
                                    self.queue.push(Command::OpenCell(r, Some(ws)));
                                    close = true;
                                }
                                Some(Err(e)) => *error = e,
                                None => close = true,
                            }
                        }
                    });
                }
                Dialog::RenameCell { cell, name, error } => {
                    ui.heading(t("Rename cell"));
                    let l = ui.label(t("New name"));
                    let r = ui.text_edit_singleline(name).labelled_by(l.id);
                    if !r.has_focus() && error.is_empty() && *name == cell.cell {
                        r.request_focus();
                    }
                    if !error.is_empty() {
                        ui.colored_label(pal.danger, error.as_str());
                    }
                    let enter = ui.input(|i| i.key_pressed(Key::Enter));
                    ui.horizontal(|ui| {
                        if ui.button(t("Cancel")).clicked() {
                            close = true;
                        }
                        if (ui.button(t("Rename")).clicked() || enter)
                            && let Some(p) = self.project.as_mut()
                        {
                            match p.rename_cell(cell, name.trim()) {
                                Ok(new) => {
                                    self.hier.clear();
                                    let old = cell.clone();
                                    for t in &mut self.tabs {
                                        if *t == old {
                                            *t = new.clone();
                                        }
                                    }
                                    if self.active.as_ref() == Some(&old) {
                                        self.active = Some(new.clone());
                                    }
                                    if let Some(e) = self.sch_eds.remove(&old) {
                                        self.sch_eds.insert(new.clone(), e);
                                    }
                                    if let Some(e) = self.lay_eds.remove(&old) {
                                        self.lay_eds.insert(new.clone(), e);
                                    }
                                    if let Some(e) = self.sims.remove(&old) {
                                        self.sims.insert(new.clone(), e);
                                    }
                                    self.dirty = true;
                                    close = true;
                                }
                                Err(e) => *error = e,
                            }
                        }
                    });
                }
                Dialog::CopyCell { from, lib, name, error } => {
                    ui.heading(t("Copy as new cell"));
                    ui.label(
                        RichText::new(tf(
                            "Start a new design from {}: the schematic, layout, simulation setup, specs and notes are copied.",
                            &[&from.to_string()],
                        ))
                        .color(pal.text_weak),
                    );
                    ui.add_space(8.0);
                    egui::Grid::new("cc").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                        let l = ui.label(t("Library"));
                        egui::ComboBox::from_id_salt("cc_lib")
                            .selected_text(lib.as_str())
                            .show_ui(ui, |ui| {
                                if let Some(p) = &self.project {
                                    for l in p.libraries.iter().filter(|l| l.name != ics_core::pcell::PCELL_LIB) {
                                        ui.selectable_value(lib, l.name.clone(), &l.name);
                                    }
                                }
                            })
                            .response
                            .labelled_by(l.id);
                        ui.end_row();
                        let l = ui.label(t("New name"));
                        ui.text_edit_singleline(name).labelled_by(l.id);
                        ui.end_row();
                    });
                    if !error.is_empty() {
                        ui.colored_label(pal.danger, error.as_str());
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(t("Cancel")).clicked() {
                            close = true;
                        }
                        if ui.button(RichText::new(t("Copy")).strong()).clicked()
                            && let Some(p) = self.project.as_mut()
                        {
                            match p.copy_cell(from, lib, name.trim()) {
                                Ok(new) => {
                                    let ws = if p.cell(&new).is_some_and(|c| c.schematic.is_some()) {
                                        Workspace::Schematic
                                    } else {
                                        Workspace::Layout
                                    };
                                    self.dirty = true;
                                    self.ok(format!("Copied {from} to {new}"));
                                    self.queue.push(Command::OpenCell(new, Some(ws)));
                                    close = true;
                                }
                                Err(e) => *error = e,
                            }
                        }
                    });
                }
                Dialog::ConfirmDelete(r) => {
                    ui.heading(t("Delete cell?"));
                    ui.label(tf("'{}' and all of its views will be deleted; its files go when the project is saved.", &[&r.to_string()]));
                    let users = self.project.as_ref().map(|p| p.cell_users(r)).unwrap_or_default();
                    if !users.is_empty() {
                        let names: Vec<String> = users.iter().take(8).map(|c| c.to_string()).collect();
                        let more = if users.len() > 8 { format!(" (+{})", users.len() - 8) } else { String::new() };
                        ui.colored_label(
                            pal.warning,
                            tf("It is placed by {}{}: they would lose it.", &[&names.join(", "), &more]),
                        );
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(t("Cancel")).clicked() {
                            close = true;
                        }
                        if ui.button(RichText::new(t("Delete")).color(pal.danger)).clicked() {
                            let r = r.clone();
                            if let Some(p) = self.project.as_mut() {
                                self.hier.clear();
                                match p.delete_cell(&r) {
                                    Ok(()) => {
                                        self.warn(format!("Deleted {r}"));
                                        // A new cell of the same name must not inherit the
                                        // deleted one's undo history, view or results.
                                        self.forget_cell(&r);
                                        self.queue.push(Command::CloseTab(r));
                                        self.dirty = true;
                                    }
                                    Err(e) => self.error(format!("Delete failed: {e}")),
                                }
                            }
                            close = true;
                        }
                    });
                }
                Dialog::RenameLibrary { lib, name, error } => {
                    ui.heading(t("Rename library"));
                    let l = ui.label(t("New name"));
                    let r = ui.text_edit_singleline(name).labelled_by(l.id);
                    if !r.has_focus() && error.is_empty() && name == lib {
                        r.request_focus();
                    }
                    if !error.is_empty() {
                        ui.colored_label(pal.danger, error.as_str());
                    }
                    let enter = ui.input(|i| i.key_pressed(Key::Enter));
                    ui.horizontal(|ui| {
                        if ui.button(t("Cancel")).clicked() {
                            close = true;
                        }
                        if (ui.button(t("Rename")).clicked() || enter)
                            && let Some(p) = self.project.as_mut()
                        {
                            let new = name.trim().to_string();
                            match p.rename_library(lib, &new) {
                                Ok(()) => {
                                    let old = lib.clone();
                                    self.library_changed(&old, Some(&new));
                                    self.info(format!("Renamed library '{old}' to '{new}'"));
                                    self.dirty = true;
                                    close = true;
                                }
                                Err(e) => *error = e,
                            }
                        }
                    });
                }
                Dialog::ConfirmDeleteLibrary(lib) => {
                    ui.heading(t("Delete library?"));
                    let (cells, users) = self.project.as_ref().map_or((0, 0), |p| {
                        (p.libraries.iter().find(|l| &l.name == lib).map_or(0, |l| l.cells.len()), p.library_users(lib))
                    });
                    ui.label(tf(
                        "Library '{}' and its {} cell(s) will be deleted; the files go when the project is saved.",
                        &[lib, &cells.to_string()],
                    ));
                    if users > 0 {
                        ui.colored_label(pal.danger, tf("{} instance(s) in other libraries place its cells.", &[&users.to_string()]));
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(t("Cancel")).clicked() {
                            close = true;
                        }
                        if ui.button(RichText::new(t("Delete")).color(pal.danger)).clicked() {
                            let lib = lib.clone();
                            if let Some(p) = self.project.as_mut() {
                                match p.delete_library(&lib) {
                                    Ok(()) => {
                                        self.library_changed(&lib, None);
                                        self.warn(format!("Deleted library '{lib}'"));
                                    }
                                    Err(e) => self.error(format!("Delete failed: {e}")),
                                }
                            }
                            self.dirty = true;
                            close = true;
                        }
                    });
                }
                Dialog::Unsaved { then } => {
                    ui.heading(t("Unsaved changes"));
                    ui.label(t("The current project has unsaved changes."));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(t("Cancel")).clicked() {
                            close = true;
                        }
                        if ui.button(RichText::new(t("Discard")).color(pal.danger)).clicked() {
                            self.dirty = false;
                            if let Some(root) = self.project.as_ref().and_then(|p| p.root.as_deref()) {
                                crate::recovery::discard(root);
                            }
                            self.queue.push((**then).clone());
                            close = true;
                        }
                        if ui.button(RichText::new(t("Save")).strong()).clicked() {
                            if self.save(false) {
                                self.queue.push((**then).clone());
                            } else if let Some(Dialog::SaveConflict { then: after, .. }) = &mut self.dialog {
                                *after = Some(then.clone());
                            }
                            close = true;
                        }
                    });
                }
                Dialog::PlaceCell { filter } => {
                    ui.heading(t("Place cell instance"));
                    let l = ui.label(t("Filter"));
                    ui.text_edit_singleline(filter).labelled_by(l.id);
                    let here = self.active.clone();
                    let f = filter.to_ascii_lowercase();
                    let cells: Vec<(CellRef, usize)> = self
                        .project
                        .as_ref()
                        .map(|p| {
                            p.all_cells()
                                .filter(|(r, c)| {
                                    Some(r) != here.as_ref()
                                        && r.lib != ics_core::pcell::PCELL_LIB
                                        && c.schematic.as_ref().is_some_and(|s| !s.ports().is_empty())
                                        && r.to_string().to_ascii_lowercase().contains(&f)
                                })
                                .map(|(r, c)| {
                                    let n = c.schematic.as_ref().map_or(0, |s| s.ports().len());
                                    (r, n)
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    ui.label(
                        RichText::new(t("Cells whose schematic has pins (labels marked as pins)")).small().color(pal.text_weak),
                    );
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        if cells.is_empty() {
                            ui.label(RichText::new(t("No cell with schematic pins")).small());
                        }
                        for (r, n) in cells {
                            if ui.button(tf("{}  ({} pins)", &[&r.to_string(), &n.to_string()])).clicked() {
                                self.queue.push(Command::PlaceCell(r));
                                close = true;
                            }
                        }
                    });
                    ui.add_space(6.0);
                    if ui.button(t("Cancel")).clicked() {
                        close = true;
                    }
                }
                Dialog::EditSymbol { cell, pins } => {
                    use ics_core::schematic::{PinSide, SymbolSpec};
                    ui.heading(tf("Symbol pins of {}", &[&cell.to_string()]));
                    ui.label(RichText::new(t("The side and order of each pin on the box other schematics place")).small().color(pal.text_weak));
                    let mut swap: Option<(usize, usize)> = None;
                    // A cell with many pins scrolls, so the preview and Apply stay on screen.
                    let max_h = (ui.ctx().content_rect().height() * 0.45).max(120.0);
                    egui::ScrollArea::vertical().max_height(max_h).show(ui, |ui| {
                    egui::Grid::new("symbol_pins").num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
                        let n = pins.len();
                        for (k, (name, side)) in pins.iter_mut().enumerate() {
                            let l = ui.label(RichText::new(name.as_str()).monospace());
                            egui::ComboBox::from_id_salt(("pin_side", k))
                                .selected_text(side.name())
                                .show_ui(ui, |ui| {
                                    for s in PinSide::ALL {
                                        ui.selectable_value(side, s, s.name());
                                    }
                                })
                                .response
                                .labelled_by(l.id);
                            if ui.add_enabled(k > 0, egui::Button::new(t("Up"))).clicked() {
                                swap = Some((k, k - 1));
                            }
                            if ui.add_enabled(k + 1 < n, egui::Button::new(t("Down"))).clicked() {
                                swap = Some((k, k + 1));
                            }
                            ui.end_row();
                        }
                    });
                    });
                    if let Some((a, b)) = swap {
                        pins.swap(a, b);
                    }
                    // Preview of the box.
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(360.0, 160.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 6.0, pal.bg_canvas);
                    let ghost = ics_core::Instance {
                        id: 0,
                        kind: DeviceKind::Cell,
                        name: String::new(),
                        pos: ics_core::Pt::new(0, 0),
                        orient: Default::default(),
                        params: Default::default(),
                        cell: Some(cell.clone()),
                        ports: pins.iter().map(|(n, _)| (n.clone(), PortDir::InOut)).collect(),
                        symbol: Some(SymbolSpec { pins: pins.clone() }),
                    };
                    let b = ghost.bbox();
                    let scale = ((rect.width() - 40.0) / (b.2 - b.0)).min((rect.height() - 30.0) / (b.3 - b.1)).clamp(4.0, 40.0);
                    let mut view = crate::canvas::View2D::new(scale as f64, false, 1.0, 100.0);
                    view.center = (((b.0 + b.2) / 2.0) as f64, ((b.1 + b.3) / 2.0) as f64);
                    crate::schematic_view::draw_symbol(ui.painter(), &view, rect, &ghost, pal.symbol, 1.4);
                    for (pin, at) in ghost.pin_positions() {
                        let q = view.to_screen(rect, at.x as f64, at.y as f64);
                        ui.painter().text(q, egui::Align2::CENTER_BOTTOM, pin, egui::FontId::proportional(11.0), pal.text_weak);
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button(t("Cancel")).clicked() {
                            close = true;
                        }
                        let reset = ui.button(t("Default")).clicked();
                        if (ui.button(RichText::new(t("Apply")).strong()).clicked() || reset)
                            && let Some(p) = self.project.as_mut()
                            && let Some(sch) = p.cell_mut(cell).and_then(|c| c.schematic.as_mut())
                        {
                            let default = SymbolSpec::arrange(None, &sch.ports());
                            let spec = (!reset && *pins != default).then(|| SymbolSpec { pins: pins.clone() });
                            if sch.symbol != spec {
                                self.sch_eds.entry(cell.clone()).or_default().history.push(sch);
                                sch.symbol = spec;
                                self.mark_dirty();
                            }
                            let n = self.refresh_cell_instances();
                            self.ok(format!("Symbol of {cell} updated; {n} placed instance(s) follow it"));
                            close = true;
                        }
                    });
                }
                Dialog::Recover { dir, root, saved } => {
                    ui.heading(t("Recover unsaved changes?"));
                    let ago = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_secs())
                        .saturating_sub(*saved);
                    let when = if ago < 120 {
                        tf("{} s ago", &[&ago.to_string()])
                    } else if ago < 7200 {
                        tf("{} min ago", &[&(ago / 60).to_string()])
                    } else {
                        tf("{} h ago", &[&(ago / 3600).to_string()])
                    };
                    ui.label(tf(
                        "IC Studio closed with unsaved changes to this project. An automatic copy from {} is newer than the saved files.",
                        &[&when],
                    ));
                    ui.label(
                        RichText::new(t("Recover opens the copy; Save then writes it to the project. Discard deletes the copy."))
                            .small()
                            .color(pal.text_weak),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new(t("Discard copy")).color(pal.danger)).clicked() {
                            crate::recovery::discard(root);
                            close = true;
                        }
                        if ui.button(RichText::new(t("Recover")).strong()).clicked() {
                            match crate::recovery::restore(dir, root) {
                                Ok(p) => {
                                    self.set_project(p);
                                    self.dirty = true;
                                    self.ok(t("Recovered the unsaved changes: save to keep them").into());
                                }
                                Err(e) => self.error(format!("Cannot read the recovery copy: {e}")),
                            }
                            close = true;
                        }
                    });
                }
                Dialog::SaveConflict { files, then } => {
                    ui.heading(t("Files changed on disk"));
                    ui.label(
                        t("Another program (git, the ics command line, an AI agent) changed these files after the \
                         project was opened, so nothing was saved:"),
                    );
                    let root = self.project.as_ref().and_then(|p| p.root.clone()).unwrap_or_default();
                    for f in files.iter().take(8) {
                        let shown = f.strip_prefix(&root).unwrap_or(f);
                        ui.label(RichText::new(shown.display().to_string()).monospace().small());
                    }
                    if files.len() > 8 {
                        ui.label(RichText::new(tf("and {} more", &[&(files.len() - 8).to_string()])).small());
                    }
                    ui.label(
                        RichText::new(
                            t("Overwrite replaces them with the project in this window. To keep their version, \
                             cancel and reopen the project."),
                        )
                        .small()
                        .color(pal.text_weak),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(t("Cancel")).clicked() {
                            close = true;
                        }
                        if ui.button(RichText::new(t("Overwrite")).color(pal.danger)).clicked() {
                            if self.save_overwriting()
                                && let Some(t) = then.take()
                            {
                                self.queue.push(*t);
                            }
                            close = true;
                        }
                    });
                }
                Dialog::Settings(st) => self.settings_dialog(ui, st, &mut close),
                Dialog::Shortcuts => {
                    ui.heading(t("Keyboard shortcuts"));
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical().max_height(ctx.content_rect().height() * 0.75).show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            for half in SHORTCUTS.chunks(2) {
                                ui.vertical(|ui| {
                                    ui.set_width(400.0);
                                    for (title, rows) in half {
                                        ui.add_space(6.0);
                                        ui.label(RichText::new(t(title)).strong().color(pal.accent));
                                        egui::Grid::new(title).num_columns(2).min_col_width(170.0).spacing([12.0, 4.0]).show(
                                            ui,
                                            |ui| {
                                                for (keys, what) in *rows {
                                                    ui.label(RichText::new(keys.replace("Mod+", MOD)).monospace());
                                                    ui.label(t(what));
                                                    ui.end_row();
                                                }
                                            },
                                        );
                                    }
                                });
                            }
                        });
                    });
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button(t("Open user guide")).clicked() {
                            self.queue.push(Command::UserGuide);
                        }
                        if ui.button(t("Close")).clicked() {
                            close = true;
                        }
                    });
                }
                Dialog::Update => {
                    ui.heading(t("Software update"));
                    ui.label(RichText::new(tf("Installed version {}", &[update::CURRENT])).color(pal.text_weak));
                    ui.add_space(8.0);
                    let release_notes = |ui: &mut egui::Ui, r: &update::Release| {
                        if !r.notes.trim().is_empty() {
                            egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                                ui.label(RichText::new(r.notes.trim()).small());
                            });
                        }
                        ui.hyperlink_to(t("Release page"), &r.page);
                    };
                    match &self.updater.state {
                        update::State::Idle | update::State::Checking(_) => {
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new().size(14.0));
                                ui.label(t("Checking GitHub for a newer release…"));
                            });
                        }
                        update::State::UpToDate => {
                            ui.colored_label(pal.ok, t("IC Studio is up to date."));
                        }
                        update::State::Available(r) => {
                            ui.label(RichText::new(tf("IC Studio {} is available.", &[&r.version])).strong());
                            release_notes(ui, r);
                            ui.add_space(6.0);
                            if r.url.is_none() {
                                ui.colored_label(pal.warning, t("No build for this platform; download it from the release page."));
                            } else if ui.button(RichText::new(t("Download and install")).strong()).clicked() {
                                self.queue.push(Command::InstallUpdate);
                            }
                        }
                        update::State::Downloading { release, .. } => {
                            ui.label(tf("Downloading IC Studio {}…", &[&release.version]));
                            match self.updater.progress() {
                                Some(f) => ui.add(egui::ProgressBar::new(f).show_percentage()),
                                None => ui.add(egui::Spinner::new()),
                            };
                        }
                        update::State::Installed { release, .. } => {
                            ui.colored_label(pal.ok, tf("IC Studio {} is installed.", &[&release.version]));
                            ui.label(t("Restart to start using it."));
                            ui.add_space(6.0);
                            if ui.button(RichText::new(t("Restart now")).strong()).clicked() {
                                self.queue.push(Command::RestartToUpdate);
                                close = true;
                            }
                        }
                        update::State::Failed(e) => {
                            ui.colored_label(pal.danger, tf("Update failed: {}", &[&e.to_string()]));
                            if ui.button(t("Try again")).clicked() {
                                self.queue.push(Command::CheckForUpdates);
                            }
                        }
                    }
                    ui.add_space(10.0);
                    if ui.button(t("Close")).clicked() {
                        close = true;
                    }
                }
                Dialog::About => {
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(t("IC Studio")).size(26.0).strong());
                        ui.label(tf("version {}", &[env!("CARGO_PKG_VERSION")]));
                        ui.add_space(6.0);
                        ui.label(t("A modern, open full-custom IC design environment."));
                        ui.label(RichText::new("Rust · egui/wgpu · ics-core · ics-sim · ngspice").small().color(pal.text_weak)); // i18n: ignore
                        ui.add_space(6.0);
                        ui.hyperlink_to(t("User guide"), GUIDE_URL);
                        ui.add_space(10.0);
                        if ui.button(t("Close")).clicked() {
                            close = true;
                        }
                    });
                }
            }
        });
        if modal.should_close() && !matches!(dlg, Dialog::Unsaved { .. }) {
            close = true;
        }
        if !close && self.dialog.is_none() {
            self.dialog = Some(dlg);
        }
    }
}
