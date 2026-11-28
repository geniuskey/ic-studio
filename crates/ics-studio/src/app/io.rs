//! Import and export (GDS, LEF/DEF, CDL, Virtuoso, SKILL, technology) and SKILL jobs.

use super::*;

impl IcStudio {
    pub(super) fn export_stream(&mut self, library: bool, format: StreamFormat) {
        let Some(p) = &self.project else { return };
        let tops: Vec<CellRef> = if library {
            p.all_cells().filter(|(_, c)| c.layout.is_some()).map(|(r, _)| r).collect()
        } else {
            match (self.active.as_ref(), self.active_cell().and_then(|c| c.layout.as_ref())) {
                (Some(r), Some(_)) => vec![r.clone()],
                _ => {
                    self.warn("Active cell has no layout".into());
                    return;
                }
            }
        };
        if tops.is_empty() {
            self.warn("No layouts to export".into());
            return;
        }
        let stem = if library { &p.name } else { &tops[0].cell };
        let filter: &[&str] = if format == StreamFormat::Oasis { &["oas", "oasis"] } else { &["gds", "gds2"] };
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("{stem}.{}", format.extension()))
            .add_filter(format.label(), filter)
            .save_file()
        else {
            return;
        };
        let n = tops.iter().flat_map(|t| ics_core::hier::descendants(p, t)).collect::<std::collections::HashSet<_>>().len();
        let bytes = ics_core::gds::write_stream_hier(format, &p.name, p, &tops, &ics_core::stream_map::export_tech(&p.tech));
        match std::fs::write(&path, &bytes) {
            Ok(()) => self.ok(format!("Exported {n} cell(s) to {} ({} bytes)", path.display(), bytes.len())),
            Err(e) => self.error(format!("{} export failed: {e}", format.label())),
        }
    }

    /// Export library `lib`: its top cells and the hierarchy below them, which may include
    /// cells of other libraries (PCells, for example).
    pub(super) fn export_library_stream(&mut self, lib: &str, format: StreamFormat) {
        let Some(p) = &self.project else { return };
        let tops = p.library_tops(lib);
        if tops.is_empty() {
            self.warn(format!("Library '{lib}' has no layouts to export"));
            return;
        }
        let filter: &[&str] = if format == StreamFormat::Oasis { &["oas", "oasis"] } else { &["gds", "gds2"] };
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("{lib}.{}", format.extension()))
            .add_filter(format.label(), filter)
            .save_file()
        else {
            return;
        };
        let n = tops.iter().flat_map(|t| ics_core::hier::descendants(p, t)).collect::<std::collections::HashSet<_>>().len();
        let bytes = ics_core::gds::write_stream_hier(format, lib, p, &tops, &ics_core::stream_map::export_tech(&p.tech));
        match std::fs::write(&path, &bytes) {
            Ok(()) => self.ok(format!("Exported {n} cell(s) of '{lib}' to {} ({} bytes)", path.display(), bytes.len())),
            Err(e) => self.error(format!("{} export failed: {e}", format.label())),
        }
    }

    /// After library `old` became `new` (or was deleted, `None`): retarget or drop the open
    /// tabs, editors and results of its cells.
    pub(super) fn library_changed(&mut self, old: &str, new: Option<&str>) {
        let map = |r: &CellRef| new.map(|n| CellRef::new(n, &r.cell));
        self.hier.clear();
        self.lay_return.clear();
        self.probe = None;
        self.tabs =
            std::mem::take(&mut self.tabs).into_iter().filter_map(|t| if t.lib == old { map(&t) } else { Some(t) }).collect();
        if let Some(a) = self.active.clone().filter(|a| a.lib == old) {
            self.active = map(&a).or_else(|| self.tabs.first().cloned());
        }
        fn remap<V>(m: &mut HashMap<CellRef, V>, old: &str, new: Option<&str>) {
            for k in m.keys().filter(|k| k.lib == old).cloned().collect::<Vec<_>>() {
                let v = m.remove(&k).expect("key exists");
                if let Some(n) = new {
                    m.insert(CellRef::new(n, &k.cell), v);
                }
            }
        }
        remap(&mut self.sch_eds, old, new);
        remap(&mut self.lay_eds, old, new);
        remap(&mut self.sims, old, new);
        if self.selected_lib.as_deref() == Some(old) {
            self.selected_lib = new.map(str::to_string);
        }
    }

    pub(super) fn import_gds(&mut self, path: Option<PathBuf>, layer_map: Option<PathBuf>) {
        if self.project.is_none() {
            return;
        }
        let Some(path) = path.or_else(|| {
            rfd::FileDialog::new()
                .add_filter("GDSII / OASIS", &["gds", "gds2", "gdsii", "oas", "oasis"])
                .add_filter("GDSII", &["gds", "gds2", "gdsii"])
                .add_filter("OASIS", &["oas", "oasis"])
                .pick_file()
        }) else {
            return;
        };
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(e) => {
                self.error(format!("Cannot read {}: {e}", path.display()));
                return;
            }
        };
        let map_text = match layer_map.as_ref().map(std::fs::read_to_string).transpose() {
            Ok(t) => t,
            Err(e) => {
                self.error(format!(
                    "Cannot read {}: {e}",
                    layer_map.as_ref().map_or_else(String::new, |m| m.display().to_string())
                ));
                return;
            }
        };
        let Some(p) = self.project.as_mut() else { return };
        let t0 = Instant::now();
        // The map is applied to a copy, kept only when the import succeeds.
        let (mut mapped, mut map_info, mut map_warnings) = (None, None, vec![]);
        if let (Some(text), Some(m)) = (&map_text, &layer_map) {
            let mut tech = p.tech.clone();
            let r = ics_core::stream_map::apply_layer_map(&mut tech, text);
            let name = m.file_name().unwrap_or_default().to_string_lossy().to_string();
            map_warnings.extend(r.warnings.into_iter().map(|w| format!("{name}: {w}")));
            let mut info = format!("Layer map {name}: {} row(s) applied", r.rows);
            if !r.added_layers.is_empty() {
                info += &format!("; added layer(s) {}", r.added_layers.join(", "));
            }
            map_info = Some(info);
            mapped = Some(tech);
        }
        let opts = ics_core::gds::ReadOptions { add_unknown_layers: true, use_stream_map: true };
        let label = StreamFormat::detect(&data).label();
        let imp = match ics_core::gds::read_stream_with(&data, mapped.as_ref().unwrap_or(&p.tech), "gds", opts) {
            Ok(imp) => imp,
            Err(e) => {
                self.error(format!("{label} import failed: {e}"));
                return;
            }
        };
        let skipped = imp.skipped_unknown_layers;
        let pairs = |m: &std::collections::BTreeMap<(u16, u16), usize>| {
            let mut v: Vec<String> = m.iter().take(12).map(|((l, d), n)| format!("{l}/{d} ({n})")).collect();
            if m.len() > 12 {
                v.push(format!("and {} more", m.len() - 12));
            }
            v.join(", ")
        };
        let skipped_pairs = pairs(&imp.skipped_pairs);
        let fallbacks = (!imp.datatype_fallbacks.is_empty())
            .then(|| (imp.datatype_fallbacks.values().sum::<usize>(), pairs(&imp.datatype_fallbacks)));
        let added = imp.new_layers.len();
        let warnings = imp.warnings.clone();
        let n = imp.cells.len();
        let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let original = mapped.map(|t| std::mem::replace(&mut p.tech, t));
        match p.add_gds_library(&stem, imp) {
            Ok((lib, tops)) => {
                self.hier.clear();
                self.mark_dirty();
                self.ok(format!(
                    "Imported {n} cell(s) from {} into library '{lib}' in {:.2} s",
                    path.display(),
                    t0.elapsed().as_secs_f64()
                ));
                if let Some(info) = map_info {
                    self.info(info);
                }
                for w in map_warnings {
                    self.warn(w);
                }
                for w in warnings {
                    self.warn(format!("{label}: {w}"));
                }
                if skipped > 0 {
                    self.warn(format!(
                        "{skipped} shape(s) on layers not in the technology were skipped: {skipped_pairs}. Add the \
                         layers to the technology or import with unknown layers added"
                    ));
                }
                if let Some((n, list)) = fallbacks {
                    self.warn(format!(
                        "{n} shape(s) on datatypes the technology lacks were read onto datatype 0 of their layer: {list}. \
                         Pin, label or blockage datatypes then sit on the drawing layer"
                    ));
                }
                if added > 0 {
                    self.warn(format!("{added} layer(s) not in the technology were added to it"));
                }
                if let Some(r) = tops.into_iter().next() {
                    self.queue.push(Command::OpenCell(r, Some(Workspace::Layout)));
                }
            }
            Err(e) => {
                if let Some(t) = original {
                    p.tech = t;
                }
                self.error(format!("{label} import failed: {e}"))
            }
        }
    }

    pub(super) fn export_lef_def(&mut self, def: bool) {
        let Some(p) = &self.project else { return };
        let (label, ext) = if def { ("DEF", "def") } else { ("LEF", "lef") };
        let Some(r) = self.active.clone().filter(|_| self.active_cell().is_some_and(|c| c.layout.is_some())) else {
            self.warn("Active cell has no layout".into());
            return;
        };
        let Some(path) = rfd::FileDialog::new().set_file_name(format!("{}.{ext}", r.cell)).add_filter(label, &[ext]).save_file()
        else {
            return;
        };
        let res = if def {
            ics_core::lefdef::export_def(p, &r).map(|(text, d, w)| {
                (text, format!("{} component(s), {} pin(s), {} net(s)", d.components.len(), d.pins.len(), d.nets.len()), w)
            })
        } else {
            ics_core::lefdef::export_lef(p, std::slice::from_ref(&r), &Default::default())
                .map(|(text, w)| (text, format!("the abstract of {}", r.cell), w))
        };
        match res.and_then(|(text, what, w)| std::fs::write(&path, text).map(|_| (what, w)).map_err(|e| e.to_string())) {
            Ok((what, warnings)) => {
                self.ok(format!("Exported {what} to {}", path.display()));
                for w in warnings {
                    self.warn(format!("{label}: {w}"));
                }
            }
            Err(e) => self.error(format!("{label} export failed: {e}")),
        }
    }

    pub(super) fn import_lef_def(&mut self, def: bool, path: Option<PathBuf>) {
        let Some(p) = self.project.as_mut() else { return };
        let (label, ext) = if def { ("DEF", "def") } else { ("LEF", "lef") };
        let Some(path) = path.or_else(|| rfd::FileDialog::new().add_filter(label, &[ext]).pick_file()) else {
            return;
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                self.error(format!("Cannot read {}: {e}", path.display()));
                return;
            }
        };
        let res = if def {
            ics_core::lefdef::import_def(p, &text, None).map(|i| {
                let msg = format!("Imported {} from {} ({} component(s) placed)", i.cell, path.display(), i.placed);
                (Some(i.cell), msg, i.warnings)
            })
        } else {
            let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
            ics_core::lefdef::import_lef(p, &text, &stem).map(|i| {
                let msg = format!("Imported {} macro(s) from {} into library '{}'", i.cells.len(), path.display(), i.lib);
                (i.cells.first().cloned(), msg, i.warnings)
            })
        };
        match res {
            Ok((open, msg, warnings)) => {
                self.hier.clear();
                self.mark_dirty();
                self.ok(msg);
                for w in warnings {
                    self.warn(format!("{label}: {w}"));
                }
                if let Some(r) = open {
                    self.queue.push(Command::OpenCell(r, Some(Workspace::Layout)));
                }
            }
            Err(e) => self.error(format!("{label} import failed: {e}")),
        }
    }

    /// After `Project::set_tech`: shapes moved to new layer indices (and maybe a new dbu), so
    /// the layout editors' undo snapshots, layer visibility and pending edits are stale.
    pub(super) fn tech_replaced(&mut self) {
        self.hier.clear();
        self.lay_eds.clear();
        self.lay_return.clear();
    }

    pub(super) fn load_pdk(&mut self, path: &Path) {
        let Some(p) = self.project.as_mut() else { return };
        let loaded = ics_core::pdk::load(path).and_then(|pdk| {
            let w = p.set_tech(pdk.tech)?;
            let t = &p.tech;
            let msg = format!(
                "PDK '{}' loaded from {} ({} layers, {} devices)",
                t.name,
                path.display(),
                t.layers.len(),
                t.devices.len()
            );
            Ok((msg, pdk.warnings, w))
        });
        match loaded {
            Ok((msg, pdk_warnings, warnings)) => {
                self.tech_replaced();
                self.mark_dirty();
                self.ok(msg);
                for w in pdk_warnings.into_iter().chain(warnings) {
                    self.warn(format!("PDK: {w}"));
                }
            }
            Err(e) => self.error(format!("Cannot load PDK: {e}")),
        }
    }

    /// Load the active layout into the Pixel Circuit window and characterise it in the background.
    pub(super) fn pixel_circuit_start(&mut self, ctx: &egui::Context) {
        let (Some(r), Some(p)) = (self.active.as_ref(), self.project.as_ref()) else {
            return self.warn("Open a layout cell to find its pixels".into());
        };
        self.pixel_circuit.load(p, r);
        self.pixel_circuit.start(Some(ctx), p);
    }

    pub(super) fn schematic_from_layout(&mut self, ctx: &egui::Context) {
        use ics_core::pixel_circuit::{Testbench, classify, schematic};
        let (Some(r), Some(p)) = (self.active.clone(), self.project.as_mut()) else { return };
        let Some(lay) = p.cell(&r).and_then(|c| c.layout.as_ref()) else {
            return self.warn(format!("{r} has no layout"));
        };
        let x = ics_core::extract::extract(lay, p, &p.tech);
        let pc = classify(&x);
        let bench = Testbench::for_tech(&x, &p.tech);
        let (sch, sim) = schematic(&x, &pc, Some(&bench));
        // Into the cell itself when it has no schematic (ready for LVS), else a new cell.
        let target = if p.cell(&r).is_some_and(|c| c.schematic.is_none()) {
            r.clone()
        } else {
            let base = format!("{}_from_layout", r.cell);
            let mut name = base.clone();
            let mut k = 1;
            while p.cell(&CellRef::new(&r.lib, &name)).is_some() {
                name = format!("{base}_{k}");
                k += 1;
            }
            match p.add_cell(&r.lib, ics_core::Cell::new(&name)) {
                Ok(t) => t,
                Err(e) => return self.error(e),
            }
        };
        let c = p.cell_mut(&target).expect("target cell");
        self.sch_eds.entry(target.clone()).or_default().history.push(c.schematic.as_ref().unwrap_or(&Default::default()));
        c.schematic = Some(sch);
        c.sim = sim;
        self.hier.clear();
        self.mark_dirty();
        let pixels = pc.pixels.len();
        self.ok(format!(
            "Schematic from the layout of {r}: {} transistor(s), {} photodiode(s), {pixels} pixel(s) -> {target}",
            x.devices.len(),
            x.photodiodes.len()
        ));
        for w in x.warnings.iter().chain(&pc.warnings) {
            self.warn(format!("{r}: {w}"));
        }
        // The pixels' characterisation runs in the background in the Pixel Circuit window.
        if pixels > 0
            && let Some(p) = self.project.as_ref()
        {
            self.pixel_circuit.load(p, &r);
            self.pixel_circuit.start(Some(ctx), p);
        }
        self.queue.push(Command::OpenCell(target, Some(Workspace::Schematic)));
    }

    pub(super) fn import_cdl(&mut self, path: Option<PathBuf>) {
        let Some(path) = path.or_else(|| {
            rfd::FileDialog::new().add_filter("CDL / SPICE netlist", &["cdl", "cir", "sp", "spi", "net"]).pick_file()
        }) else {
            return;
        };
        let Some(p) = self.project.as_mut() else { return };
        // Subcircuits placing each other keep their hierarchy: a cell per subcircuit.
        if let Ok(text) = std::fs::read_to_string(&path)
            && ics_core::cadence::cdl_is_hierarchical(&text)
        {
            // Into a new library named after the file, so no existing cell is touched.
            let stem: String = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
                .collect();
            let stem = if stem.is_empty() || stem.starts_with('-') { "cdl".to_string() } else { stem };
            let mut lib = stem.clone();
            let mut k = 1;
            while p.libraries.iter().any(|l| l.name.eq_ignore_ascii_case(&lib)) {
                lib = format!("{stem}_{k}");
                k += 1;
            }
            match ics_core::cadence::import_cdl_hier(&text, &p.tech, None, &lib).and_then(|h| {
                let cells = h.add_to(p, &lib, false)?;
                Ok((h, cells))
            }) {
                Ok((h, cells)) => {
                    self.hier.clear();
                    self.mark_dirty();
                    let names: Vec<String> = cells.iter().map(|r| r.cell.clone()).collect();
                    let lib = cells.last().map_or(String::new(), |r| r.lib.clone());
                    self.ok(format!(
                        "CDL: {} cell(s) imported with their hierarchy into library {lib}, top {}: {}",
                        cells.len(),
                        h.top,
                        names.join(", ")
                    ));
                    for w in h.warnings.iter().chain(h.cells.iter().flat_map(|c| &c.imported.warnings)) {
                        self.warn(format!("CDL: {w}"));
                    }
                    if let Some(top) = cells.last() {
                        self.queue.push(Command::OpenCell(top.clone(), Some(Workspace::Schematic)));
                    }
                    return;
                }
                Err(e) => self.warn(format!("CDL: hierarchy not kept ({e}); importing it flat")),
            }
        }
        let Some(p) = self.project.as_mut() else { return };
        let res = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| ics_core::cadence::import_cdl(&t, &p.tech, None));
        let imp = match res {
            Ok(i) => i,
            Err(e) => return self.error(format!("Cannot import {}: {e}", path.display())),
        };
        let base = if imp.flat.top.is_empty() {
            path.file_stem().unwrap_or_default().to_string_lossy().to_string()
        } else {
            imp.flat.top.clone()
        };
        if p.library_mut("work").is_none()
            && let Err(e) = p.add_library("work")
        {
            return self.error(e);
        }
        // A free cell name.
        let mut name = base.clone();
        let mut k = 1;
        while p.cell(&CellRef::new("work", &name)).is_some() {
            name = format!("{base}_{k}");
            k += 1;
        }
        let mut cell = ics_core::Cell::new(&name);
        cell.schematic = Some(imp.imported.schematic);
        match p.add_cell("work", cell) {
            Ok(r) => {
                self.hier.clear();
                self.mark_dirty();
                self.ok(format!(
                    "CDL: {} device(s) of {} imported as {r} (subcircuits: {})",
                    imp.flat.devices,
                    if imp.flat.top.is_empty() { "the top level" } else { &imp.flat.top },
                    imp.subckts.join(", ")
                ));
                for w in imp.imported.warnings {
                    self.warn(format!("CDL: {w}"));
                }
                self.queue.push(Command::OpenCell(r, Some(Workspace::Schematic)));
            }
            Err(e) => self.error(e),
        }
    }

    pub(super) fn import_virtuoso(&mut self, ctx: &egui::Context, path: Option<PathBuf>, lib: Option<String>) {
        let Some(path) = path.or_else(|| {
            rfd::FileDialog::new().add_filter("Virtuoso library dump or cds.lib", &["icsdump", "txt", "lib"]).pick_file()
        }) else {
            return;
        };
        // A cds.lib names several libraries: ask which one.
        let is_cds = path.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("cds.lib"));
        if lib.is_none() && is_cds {
            match ics_core::virtuoso_batch::parse_cds_lib(&path) {
                Ok(libs) => {
                    let found = ics_core::virtuoso_batch::find_virtuoso().is_some();
                    self.virtuoso_pick = Some((path, libs.into_iter().map(|(n, _)| n).collect(), found));
                }
                Err(e) => self.error(format!("Cannot read {}: {e}", path.display())),
            }
            return;
        }
        if self.project.is_none() {
            return;
        }
        let is_dump = matches!(
            ics_core::virtuoso_batch::resolve_source(&path, lib.as_deref()),
            Ok(ics_core::virtuoso_batch::Source::Dump(_))
        );
        if !is_dump && ics_core::virtuoso_batch::find_virtuoso().is_some() {
            // Virtuoso can take minutes: dump in the background, import when it is done.
            if self.virtuoso_job.is_some() {
                return self.warn("A Virtuoso import is already running".to_string());
            }
            self.info(format!("Running Virtuoso in batch mode to dump {}…", lib.as_deref().unwrap_or("the library")));
            let (tx, rx) = mpsc::channel();
            let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (src, ctx, stop) = (path.clone(), ctx.clone(), cancel.clone());
            let what = lib.clone().unwrap_or_else(|| path.display().to_string());
            std::thread::spawn(move || {
                let _ = tx.send(ics_core::virtuoso_batch::read_dump_with(&src, lib.as_deref(), &stop));
                ctx.request_repaint();
            });
            self.virtuoso_job = Some(VirtuosoJob { path, what, rx, cancel, started: std::time::Instant::now() });
            return;
        }
        let text = ics_core::virtuoso_batch::read_dump(&path, lib.as_deref());
        self.import_virtuoso_text(&path, text);
    }

    /// Finishes a background Virtuoso dump.
    pub(super) fn poll_virtuoso_job(&mut self) {
        let Some(job) = &self.virtuoso_job else { return };
        let res = match job.rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("the Virtuoso run stopped".to_string()),
        };
        let job = self.virtuoso_job.take().expect("checked");
        self.import_virtuoso_text(&job.path, res);
    }

    /// While Virtuoso dumps a library: its elapsed time and a Cancel button.
    pub(super) fn virtuoso_job_window(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.virtuoso_job else { return };
        let mut cancel = false;
        egui::Window::new("Virtuoso") // i18n: ignore
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::RIGHT_BOTTOM, [-12.0, -36.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tf(
                        "Dumping {} in batch mode: {} s",
                        &[&job.what, &format!("{:.0}", job.started.elapsed().as_secs_f32())],
                    ));
                });
                cancel = ui.button(t("Cancel")).clicked();
            });
        if cancel {
            self.queue.push(Command::CancelVirtuoso);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }

    /// Whether Virtuoso is dumping a library in the background.
    pub fn virtuoso_running(&self) -> bool {
        self.virtuoso_job.is_some()
    }

    pub(super) fn import_virtuoso_text(&mut self, path: &Path, text: Result<String, String>) {
        let Some(p) = self.project.as_mut() else { return };
        let res = text.and_then(|t| {
            let d = ics_core::virtuoso_dump::Dump::parse(&t)?;
            ics_core::virtuoso_dump::import_dump(&d, &p.tech, &ics_core::cadence::SymbolMap::analog_lib(), 0.1875)
        });
        let imp = match res {
            Ok(i) => i,
            Err(e) => return self.error(format!("Cannot import {}: {e}", path.display())),
        };
        let mut warnings = imp.warnings.clone();
        let done = match ics_core::virtuoso_dump::apply(p, &imp, false, &mut warnings) {
            Ok(d) => d,
            Err(e) => return self.error(e),
        };
        self.hier.clear();
        self.mark_dirty();
        let (sch, lay) = (done.iter().filter(|a| a.schematic).count(), done.iter().filter(|a| a.layout).count());
        self.ok(format!("Virtuoso: {sch} schematic(s) and {lay} layout(s) imported from {}", path.display()));
        for w in warnings {
            self.warn(format!("Virtuoso: {w}"));
        }
        if let Some(a) = done.iter().find(|a| a.schematic).or_else(|| done.iter().find(|a| a.layout)) {
            let ws = if a.schematic { Workspace::Schematic } else { Workspace::Layout };
            self.queue.push(Command::OpenCell(a.cell.clone(), Some(ws)));
        }
    }

    pub(super) fn save_dump_skill(&mut self, path: Option<PathBuf>) {
        let Some(path) =
            path.or_else(|| rfd::FileDialog::new().set_file_name("icsDump.il").add_filter("SKILL script", &["il"]).save_file())
        else {
            return;
        };
        match std::fs::write(&path, ics_core::virtuoso_dump::DUMP_SKILL) {
            Ok(()) => self.ok(format!(
                "Wrote {}. In the Virtuoso CIW: load(\"icsDump.il\") then icsDumpLib(\"myLib\" \"/tmp/myLib.icsdump\")",
                path.display()
            )),
            Err(e) => self.error(format!("Cannot write {}: {e}", path.display())),
        }
    }

    pub(super) fn export_skill(&mut self) {
        let (Some(p), Some(r)) = (self.project.as_ref(), self.active.clone()) else { return };
        let Some(sch) = p.cell(&r).and_then(|c| c.schematic.as_ref()) else {
            return self.warn("Open a cell with a schematic to export it to Virtuoso".into());
        };
        // A symbol map next to the project (virtuoso_symbols.toml), else analogLib.
        let map_file = p.root.as_ref().map(|d| d.join("virtuoso_symbols.toml")).filter(|f| f.exists());
        let map = match &map_file {
            Some(f) => match std::fs::read_to_string(f)
                .map_err(|e| e.to_string())
                .and_then(|t| ics_core::cadence::SymbolMap::from_toml(&t))
            {
                Ok(m) => m,
                Err(e) => return self.error(format!("{}: {e}", f.display())),
            },
            None => ics_core::cadence::SymbolMap::analog_lib(),
        };
        let (il, warnings) = ics_core::cadence::skill_schematic(sch, &r.lib, &r.cell, &map, 0.1875);
        let Some(path) =
            rfd::FileDialog::new().add_filter("Cadence SKILL", &["il"]).set_file_name(format!("{}.il", r.cell)).save_file()
        else {
            return;
        };
        match std::fs::write(&path, il) {
            Ok(()) => self.ok(format!(
                "Wrote {} (load it in the Virtuoso CIW; symbols: {})",
                path.display(),
                map_file.map_or("analogLib".into(), |f| f.display().to_string())
            )),
            Err(e) => self.error(format!("{}: {e}", path.display())),
        }
        for w in warnings {
            self.warn(format!("SKILL export: {w}"));
        }
    }

    pub(super) fn import_tech(&mut self) {
        let Some(p) = self.project.as_mut() else { return };
        let Some(path) = rfd::FileDialog::new().add_filter("KLayout layer properties", &["lyp"]).pick_file() else { return };
        let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let tech = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|xml| ics_core::tech_import::from_lyp(&xml, &name))
            .and_then(|t| p.set_tech(t).map(|w| (w, p.tech.layers.len())));
        match tech {
            Ok((warnings, layers)) => {
                self.tech_replaced();
                self.mark_dirty();
                self.ok(format!("Technology '{name}' imported from {} ({layers} layers)", path.display()));
                for w in warnings {
                    self.warn(format!("Technology: {w}"));
                }
            }
            Err(e) => self.error(format!("Cannot import {}: {e}", path.display())),
        }
    }

    pub(super) fn load_skill(&mut self, path: Option<PathBuf>) {
        if self.project.is_none() {
            return;
        }
        let Some(path) = path.or_else(|| rfd::FileDialog::new().add_filter("Cadence SKILL", &["il", "ils"]).pick_file()) else {
            return;
        };
        self.ciw.push(CiwKind::Input, format!("load(\"{}\")", path.display().to_string().replace('\\', "/")));
        self.skill_start(ics_core::skill::Task::Load(path.clone()), SkillJobKind::Load(path));
    }

    /// Evaluate a CIW command in the persistent SKILL session.
    pub(super) fn ciw_eval(&mut self, src: &str) {
        if self.project.is_none() {
            self.ciw.push(CiwKind::Error, "*Error* no project is open");
            return;
        }
        self.skill_start(ics_core::skill::Task::Eval { src: src.to_string(), file: None }, SkillJobKind::Ciw);
    }

    /// Run `task` in the CIW's SKILL session on a copy of the project. A quick script finishes
    /// at once; a longer one runs in the background behind a dialog that can cancel it, and its
    /// result replaces the project when it ends.
    pub(super) fn skill_start(&mut self, task: ics_core::skill::Task, kind: SkillJobKind) {
        if self.skill_job.is_some() {
            return self.warn("A SKILL script is still running".to_string());
        }
        let current = self.active.clone();
        let Some(p) = self.project.as_ref() else { return };
        let before = layout_snapshot(p);
        self.ciw.session.start(p.clone(), task, current);
        self.skill_job = Some(SkillJob { kind, before, started: std::time::Instant::now() });
        self.poll_skill_job(std::time::Duration::from_millis(150));
    }

    /// Collect the background SKILL evaluation if it has ended (waiting up to `wait`).
    pub(super) fn poll_skill_job(&mut self, wait: std::time::Duration) {
        if self.skill_job.is_none() {
            return;
        }
        let Some(f) = self.ciw.session.wait_timeout(wait) else { return };
        let job = self.skill_job.take().expect("checked");
        if let Some(p) = f.project {
            self.project = Some(p);
        }
        let res = f.result;
        self.ciw.show(&res);
        if let Ok(r) = &res {
            self.skill_forms.extend(r.forms.iter().map(|f| (f.clone(), f.fields.clone())));
        }
        let changed = self.layouts_edited(job.before);
        match &job.kind {
            SkillJobKind::Ciw => {}
            SkillJobKind::Load(path) => {
                let (output, err) = match &res {
                    Ok(r) => (r.output.clone(), None),
                    Err(e) => (e.output.clone(), Some(e.to_string())),
                };
                for line in output.lines() {
                    self.info(format!("SKILL: {line}"));
                }
                match err {
                    None => self.ok(format!("Loaded {}: {} cell(s) modified", path.display(), changed.len())),
                    Some(e) => {
                        self.error(format!("SKILL error: {e}"));
                        if !changed.is_empty() {
                            self.warn(format!("Edits made before the error are kept in {} cell(s)", changed.len()));
                        }
                    }
                }
            }
            SkillJobKind::Form => {
                if let Err(e) = &res {
                    self.error(format!("SKILL error: {e}"));
                }
            }
        }
        if self.active.is_none()
            && let Some(r) = changed.into_iter().next()
        {
            self.queue.push(Command::OpenCell(r, Some(Workspace::Layout)));
        }
    }

    /// Whether a SKILL script is running in the background.
    pub fn skill_running(&self) -> bool {
        self.skill_job.is_some()
    }

    /// While a SKILL script runs: a dialog with its elapsed time and a Cancel button.
    pub(super) fn skill_job_window(&mut self, ctx: &egui::Context) {
        self.poll_skill_job(std::time::Duration::ZERO);
        let Some(job) = &self.skill_job else { return };
        let secs = job.started.elapsed().as_secs_f32();
        let what = match &job.kind {
            SkillJobKind::Ciw => t("CIW command").to_string(),
            SkillJobKind::Load(p) => p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().to_string()),
            SkillJobKind::Form => t("form callback").to_string(),
        };
        let mut cancel = false;
        egui::Modal::new(egui::Id::unique("skill_running")).show(ctx, |ui| {
            ui.heading(t("Running SKILL"));
            ui.label(format!("{what}: {secs:.1} s"));
            ui.add_space(6.0);
            cancel = ui.button(t("Cancel")).clicked();
        });
        if cancel {
            self.queue.push(Command::CancelSkill);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    /// After an external edit of any number of layouts (a SKILL script): record undo history
    /// for each changed cell that had a layout, refresh the editors and mark the project dirty.
    /// Returns the changed cells.
    /// Push an undo step for every schematic that differs from `before` (an MCP tool or a
    /// script changed it) and refit its editor.
    pub(super) fn schematics_edited(&mut self, before: HashMap<CellRef, Schematic>) {
        let Some(p) = &self.project else { return };
        let changed: Vec<(CellRef, Schematic)> = p
            .all_cells()
            .filter(|(r, c)| c.schematic.is_some() && c.schematic.as_ref() != before.get(r))
            .map(|(r, _)| {
                let old = before.get(&r).cloned().unwrap_or_default();
                (r, old)
            })
            .collect();
        if changed.is_empty() {
            return;
        }
        for (r, old) in changed {
            let ed = self.sch_eds.entry(r).or_default();
            ed.history.push(&old);
            ed.selection.clear();
            ed.request_fit();
        }
        self.mark_dirty();
    }

    pub(super) fn layouts_edited(&mut self, before: HashMap<CellRef, Layout>) -> Vec<CellRef> {
        let Some(p) = &self.project else { return vec![] };
        let mut changed: Vec<CellRef> =
            p.all_cells().filter(|(r, c)| c.layout.is_some() && c.layout.as_ref() != before.get(r)).map(|(r, _)| r).collect();
        changed.sort();
        if changed.is_empty() {
            return changed;
        }
        for r in &changed {
            if let Some(lay) = before.get(r) {
                self.lay_eds.entry(r.clone()).or_default().history.push(lay);
            }
        }
        for (r, ed) in self.lay_eds.iter_mut() {
            ed.layout_changed();
            let lay = p.cell(r).and_then(|c| c.layout.as_ref());
            ed.selection.retain(|id| lay.is_some_and(|l| l.ids().any(|i| i == *id)));
        }
        self.hier.clear();
        self.mark_dirty();
        changed
    }
}
