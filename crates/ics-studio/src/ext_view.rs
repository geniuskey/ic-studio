//! Extensions window (Extensions > Manage Extensions): the installed extensions with their
//! tools, commands and PDKs, switching them on and off, installing from a folder, a git
//! repository or the community index, updating and removing them.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use eframe::egui::{self, RichText};
use ics_mcp::ext::Registry;
use ics_mcp::ext::install::{self, IndexEntry, Source};

use crate::i18n::{t, tf};
use crate::theme::{self, Palette};

/// The guide page on using and writing extensions.
pub const GUIDE: &str = "https://pages.euiyun.com/ic-studio/extensions.html";

type Job<T> = Receiver<Result<T, String>>;

#[derive(Default)]
pub struct ExtensionManager {
    pub open: bool,
    url: String,
    sub: String,
    rev: String,
    query: String,
    index: Option<Result<Vec<IndexEntry>, String>>,
    index_job: Option<Job<Vec<IndexEntry>>>,
    /// An install / update running in the background, with what it does.
    job: Option<(String, Job<String>)>,
    status: Option<(bool, String)>,
    /// An index entry whose Install was clicked, waiting for the user to confirm.
    confirm: Option<IndexEntry>,
}

/// What the app should do after the window changed something.
pub enum ExtAction {
    /// The installed set or a switch changed: load the extensions again.
    Reload,
    OpenFolder(PathBuf),
}

/// `file://` URL of a folder, for the system file manager.
pub fn folder_url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') { format!("file://{s}") } else { format!("file:///{s}") }
}

impl ExtensionManager {
    pub fn busy(&self) -> bool {
        self.job.is_some() || self.index_job.is_some()
    }

    fn root(reg: &Registry) -> Option<PathBuf> {
        reg.dir.clone().or_else(ics_mcp::ext::user_dir)
    }

    fn start(&mut self, ctx: &egui::Context, what: String, f: impl FnOnce() -> Result<String, String> + Send + 'static) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(f());
            ctx.request_repaint();
        });
        self.status = None;
        self.job = Some((what, rx));
    }

    fn install(&mut self, ctx: &egui::Context, reg: &Registry, src: Source) {
        let Some(root) = Self::root(reg) else {
            self.status = Some((false, t("No per-user data folder for extensions").into()));
            return;
        };
        let what = tf("Installing {}", &[&src.describe()]);
        self.start(ctx, what, move || {
            let (m, _) = install::install(&root, &src, false)?;
            let runs = m.run.as_ref().map_or(String::new(), |r| format!("; it runs `{}`", r.argv().join(" ")));
            Ok(format!("Installed {} {}{runs}", m.extension.name, m.extension.version))
        });
    }

    fn fetch_index(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(install::fetch_index(&install::index_location()));
            ctx.request_repaint();
        });
        self.index_job = Some(rx);
    }

    /// Pick up finished background work; `true` when the extensions changed.
    fn poll(&mut self) -> bool {
        if let Some(r) = self.index_job.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.index = Some(r);
            self.index_job = None;
        }
        if let Some(r) = self.job.as_ref().and_then(|(_, rx)| rx.try_recv().ok()) {
            self.job = None;
            let ok = r.is_ok();
            self.status = Some(match r {
                Ok(m) => (true, m),
                Err(e) => (false, e),
            });
            return ok;
        }
        false
    }

    pub fn window(&mut self, ctx: &egui::Context, reg: &Registry) -> Option<ExtAction> {
        let changed = self.poll();
        if !self.open {
            return changed.then_some(ExtAction::Reload);
        }
        let mut open = true;
        let r = crate::float::tool_window(ctx, "extensions", "Extensions", [760.0, 680.0], &mut open, |ui| self.ui(ui, reg));
        self.open &= open;
        if changed { Some(ExtAction::Reload) } else { r.flatten() }
    }

    fn ui(&mut self, ui: &mut egui::Ui, reg: &Registry) -> Option<ExtAction> {
        let pal = theme::palette(ui.ctx());
        let mut action = None;
        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg_panel).inner_margin(12)).show(ui, |ui| {
            ui.label(
                RichText::new(t(
                    "Extensions add tools, menu commands, SKILL scripts and PDKs. Ones with a program run it with your \
                     rights: install only extensions you trust.",
                ))
                .small()
                .color(pal.text_weak),
            );
            ui.horizontal(|ui| {
                if let Some(root) = Self::root(reg) {
                    ui.label(RichText::new(tf("Folder: {}", &[&root.display().to_string()])).small().color(pal.text_weak));
                    if ui.small_button(t("Open")).clicked() {
                        let _ = std::fs::create_dir_all(&root);
                        action = Some(ExtAction::OpenFolder(root));
                    }
                }
                if ui.small_button(t("Reload")).clicked() {
                    action = Some(ExtAction::Reload);
                }
                if ui.small_button(t("Writing extensions (guide)")).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(GUIDE));
                }
            });
            if let Some((what, _)) = &self.job {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(what);
                });
            }
            if let Some((ok, m)) = &self.status {
                ui.label(RichText::new(m).color(if *ok { pal.ok } else { pal.danger }));
            }
            ui.add_space(6.0);
            egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
                ui.heading(tf("Installed ({})", &[&reg.extensions.len().to_string()]));
                if reg.extensions.is_empty() {
                    ui.label(RichText::new(t("None yet. Add one below or from the index.")).color(pal.text_weak));
                }
                for e in &reg.extensions {
                    if let Some(a) = self.installed_row(ui, reg, e, &pal) {
                        action = Some(a);
                    }
                }
                for (p, err) in &reg.errors {
                    ui.label(RichText::new(format!("{}: {err}", p.display())).small().color(pal.warning));
                }
                ui.add_space(10.0);
                ui.heading(t("Add"));
                self.add_ui(ui, reg);
                ui.add_space(10.0);
                ui.heading(t("Community index"));
                self.index_ui(ui, reg, &pal);
            });
        });
        action
    }

    fn installed_row(
        &mut self,
        ui: &mut egui::Ui,
        reg: &Registry,
        e: &ics_mcp::ext::Extension,
        pal: &Palette,
    ) -> Option<ExtAction> {
        let mut action = None;
        let i = &e.manifest.extension;
        egui::Frame::new().fill(pal.bg_sidebar).inner_margin(8).corner_radius(4.0).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let mut on = e.enabled;
                let l = ui.label(RichText::new(&i.name).strong().color(pal.text_strong));
                if ui.checkbox(&mut on, t("Enabled")).labelled_by(l.id).changed()
                    && let Some(root) = Self::root(reg)
                {
                    match install::set_enabled(&root, e.id(), on) {
                        Ok(()) => {
                            e.stop();
                            action = Some(ExtAction::Reload);
                        }
                        Err(err) => self.status = Some((false, err)),
                    }
                }
                ui.label(RichText::new(format!("{} · {}", i.version, i.id)).small().color(pal.text_weak));
                if e.running() {
                    ui.label(RichText::new(t("running")).small().color(pal.ok));
                }
            });
            if !i.description.is_empty() {
                ui.label(&i.description);
            }
            if let Some(p) = &e.problem {
                ui.label(RichText::new(p).color(pal.warning));
            }
            let m = &e.manifest;
            let list = |ui: &mut egui::Ui, what: &str, items: Vec<String>| {
                if !items.is_empty() {
                    ui.label(RichText::new(format!("{}: {}", t(what), items.join(", "))).small().color(pal.text_weak));
                }
            };
            list(ui, "Tools", m.tools.iter().map(|t| m.tool_name(&t.name)).collect());
            list(ui, "Commands", m.commands.iter().map(|c| c.title.clone()).collect());
            list(ui, "PDKs", m.pdks.iter().map(|p| p.name.clone()).collect());
            if let Some(run) = &m.run {
                list(ui, "Runs", vec![run.argv().join(" ")]);
            }
            let source = e.source();
            if let Some(s) = &source {
                list(ui, "From", vec![s.describe()]);
            }
            let meta: Vec<String> =
                [i.license.clone(), (!i.authors.is_empty()).then(|| i.authors.join(", "))].into_iter().flatten().collect();
            list(ui, "License / authors", meta);
            ui.horizontal(|ui| {
                let busy = self.job.is_some();
                if ui.add_enabled(!busy && source.is_some(), egui::Button::new(t("Update"))).clicked()
                    && let Some(root) = Self::root(reg)
                {
                    e.stop();
                    let id = e.id().to_string();
                    self.start(ui.ctx(), tf("Updating {}", &[&id]), move || {
                        let (m, _) = install::update(&root, &id)?;
                        Ok(format!("Updated {id} to {}", m.extension.version))
                    });
                }
                if ui.add_enabled(!busy, egui::Button::new(t("Remove"))).clicked()
                    && let Some(root) = Self::root(reg)
                {
                    e.stop();
                    self.status = Some(match install::remove(&root, e.id()) {
                        Ok(()) => (true, tf("Removed {}", &[&i.name])),
                        Err(err) => (false, err),
                    });
                    action = Some(ExtAction::Reload);
                }
                if ui.button(t("Open Folder")).clicked() {
                    action = Some(ExtAction::OpenFolder(e.dir.clone()));
                }
                if let Some(url) = i.repository.as_ref().or(i.homepage.as_ref())
                    && ui.button(t("Website")).clicked()
                {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                }
                if e.running() && ui.button(t("Stop")).on_hover_text(t("Stop its program; it starts again when needed")).clicked()
                {
                    e.stop();
                }
            });
        });
        ui.add_space(4.0);
        action
    }

    fn add_ui(&mut self, ui: &mut egui::Ui, reg: &Registry) {
        let busy = self.job.is_some();
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, egui::Button::new(t("Install from Folder…"))).clicked()
                && let Some(dir) = rfd::FileDialog::new().set_title(t("Extension folder (with extension.toml)")).pick_folder()
            {
                self.install(ui.ctx(), reg, Source::Folder(dir));
            }
        });
        egui::Grid::new("ext_git").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            let l = ui.label(t("Git URL"));
            ui.add(egui::TextEdit::singleline(&mut self.url).hint_text("https://github.com/you/ics-ext").desired_width(380.0)) // i18n: ignore
                .labelled_by(l.id);
            ui.end_row();
            let l = ui.label(t("Sub-folder"));
            ui.add(egui::TextEdit::singleline(&mut self.sub).hint_text(t("optional")).desired_width(380.0)).labelled_by(l.id);
            ui.end_row();
            let l = ui.label(t("Tag / branch"));
            ui.add(egui::TextEdit::singleline(&mut self.rev).hint_text(t("optional")).desired_width(380.0)).labelled_by(l.id);
            ui.end_row();
        });
        let opt = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
        if ui.add_enabled(!busy && !self.url.trim().is_empty(), egui::Button::new(t("Install from Git"))).clicked() {
            match Source::parse(self.url.trim(), opt(&self.rev), opt(&self.sub)) {
                Ok(src) => self.install(ui.ctx(), reg, src),
                Err(e) => self.status = Some((false, e)),
            }
        }
    }

    fn index_ui(&mut self, ui: &mut egui::Ui, reg: &Registry, pal: &Palette) {
        ui.horizontal(|ui| {
            let l = ui.label(t("Search"));
            ui.add(egui::TextEdit::singleline(&mut self.query).desired_width(240.0)).labelled_by(l.id);
            let label = t(if self.index.is_some() { "Refresh" } else { "Load Index" });
            if ui.add_enabled(self.index_job.is_none(), egui::Button::new(label)).clicked() {
                self.fetch_index(ui.ctx());
            }
            if self.index_job.is_some() {
                ui.spinner();
            }
        });
        ui.label(
            RichText::new(t("Publish yours by adding it to extensions/index.json of the IC Studio repository."))
                .small()
                .color(pal.text_weak),
        );
        let entries = match &self.index {
            None => return,
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(pal.danger));
                return;
            }
            Some(Ok(v)) => v.iter().filter(|e| e.matches(self.query.trim())).cloned().collect::<Vec<_>>(),
        };
        if entries.is_empty() {
            ui.label(RichText::new(t("No extension matches")).color(pal.text_weak));
        }
        for e in entries {
            ui.horizontal(|ui| {
                let installed = reg.get(&e.id).is_some();
                let button = t(if installed { "Installed" } else { "Install" });
                if ui.add_enabled(!installed && self.job.is_none(), egui::Button::new(button)).clicked() {
                    self.confirm = Some(e.clone());
                }
                ui.label(RichText::new(&e.name).strong());
                ui.label(RichText::new(&e.description).color(pal.text_weak));
            });
        }
        // One click on an index entry asks first: what gets installed, and that it runs code.
        if let Some(e) = self.confirm.clone() {
            egui::Frame::new().fill(pal.warning.gamma_multiply(0.10)).corner_radius(6).inner_margin(8).show(ui, |ui| {
                ui.label(RichText::new(tf("Install {}?", &[&e.name])).strong());
                ui.label(RichText::new(tf("From {}", &[&e.source().describe()])).small().color(pal.text_weak));
                if e.commit.is_none() {
                    ui.label(
                        RichText::new(t("Not pinned to a commit: you get whatever the repository holds now."))
                            .small()
                            .color(pal.warning),
                    );
                }
                ui.label(t("An extension with a program runs it with your rights. Install only ones you trust."));
                ui.horizontal(|ui| {
                    if ui.add_enabled(self.job.is_none(), egui::Button::new(t("Install"))).clicked() {
                        self.confirm = None;
                        self.install(ui.ctx(), reg, e.source());
                    }
                    if ui.button(t("Cancel")).clicked() {
                        self.confirm = None;
                    }
                });
            });
        }
    }
}
