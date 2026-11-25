//! The Settings window: a sidebar of pages (general, appearance, editing, schematic, layout,
//! simulation, saving, AI, about) with a search box that finds a setting on every page.

use eframe::egui::{self, Color32, RichText, Sense, Ui};
use serde::{Deserialize, Serialize};

use super::{Command, IcStudio, Settings};
use crate::canvas::Profile;
use crate::i18n::{t, tf};
use crate::theme::{self, Palette};

/// A page of the Settings window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SettingsPage {
    #[default]
    General,
    Appearance,
    Editing,
    Schematic,
    Layout,
    Simulation,
    Saving,
    Ai,
    About,
}

impl SettingsPage {
    pub const ALL: [SettingsPage; 9] = [
        SettingsPage::General,
        SettingsPage::Appearance,
        SettingsPage::Editing,
        SettingsPage::Schematic,
        SettingsPage::Layout,
        SettingsPage::Simulation,
        SettingsPage::Saving,
        SettingsPage::Ai,
        SettingsPage::About,
    ];

    pub fn title(self) -> &'static str {
        match self {
            SettingsPage::General => t("General"),
            SettingsPage::Appearance => t("Appearance"),
            SettingsPage::Editing => t("Mouse and keyboard"),
            SettingsPage::Schematic => t("Schematic"),
            SettingsPage::Layout => t("Layout"),
            SettingsPage::Simulation => t("Simulation"),
            SettingsPage::Saving => t("Saving and recovery"),
            SettingsPage::Ai => t("AI"),
            SettingsPage::About => t("Updates and about"),
        }
    }

    fn blurb(self) -> &'static str {
        match self {
            SettingsPage::General => t("Language, start-up and the menus"),
            SettingsPage::Appearance => t("Colours and size of the interface"),
            SettingsPage::Editing => t("How the mouse wheel and buttons move the canvas"),
            SettingsPage::Schematic => t("Defaults of the schematic editor, for every cell"),
            SettingsPage::Layout => t("Defaults of the layout editor, for every cell; the toolbar switches them per cell"),
            SettingsPage::Simulation => t("The external simulator"),
            SettingsPage::Saving => t("Backup copies of unsaved work"),
            SettingsPage::Ai => t("The design assistant and AI agents"),
            SettingsPage::About => t("Version and software updates"),
        }
    }

    /// The section of the sidebar the page is listed under.
    fn group(self) -> &'static str {
        match self {
            SettingsPage::General | SettingsPage::Appearance | SettingsPage::Editing => t("App"),
            SettingsPage::Schematic | SettingsPage::Layout | SettingsPage::Simulation => t("Design"),
            SettingsPage::Saving | SettingsPage::Ai | SettingsPage::About => t("System"),
        }
    }
}

/// Defaults of the editors: each new schematic or layout tab starts with them, and changing
/// one in Settings applies it to the open tabs too.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct EditorDefaults {
    pub sch_grid: bool,
    pub sch_annotate_op: bool,
    pub lay_grid: bool,
    pub lay_gravity: bool,
    pub lay_drd: bool,
    pub lay_magnet: bool,
    pub lay_auto_abut: bool,
    pub lay_snap_tracks: bool,
    pub lay_flylines: bool,
}

impl Default for EditorDefaults {
    fn default() -> Self {
        Self {
            sch_grid: true,
            sch_annotate_op: true,
            lay_grid: true,
            lay_gravity: true,
            lay_drd: true,
            lay_magnet: true,
            lay_auto_abut: true,
            lay_snap_tracks: true,
            lay_flylines: false,
        }
    }
}

impl EditorDefaults {
    pub fn apply_sch(&self, ed: &mut crate::schematic_view::SchematicEditor) {
        ed.show_grid = self.sch_grid;
        ed.annotate_op = self.sch_annotate_op;
        ed.configured = true;
    }

    pub fn apply_lay(&self, ed: &mut crate::layout_view::LayoutEditor) {
        ed.show_grid = self.lay_grid;
        ed.gravity = self.lay_gravity;
        ed.drd = self.lay_drd;
        ed.magnet = self.lay_magnet;
        ed.auto_abut = self.lay_auto_abut;
        ed.snap_tracks = self.lay_snap_tracks;
        ed.show_flylines = self.lay_flylines;
        ed.configured = true;
    }
}

/// Autosave intervals offered, in seconds (0 = off).
const AUTOSAVE: [u32; 6] = [0, 30, 60, 120, 300, 600];

fn autosave_label(s: u32) -> String {
    match s {
        0 => t("Off").to_string(),
        s if s < 60 => tf("Every {} s", &[&s.to_string()]),
        s => tf("Every {} min", &[&(s / 60).to_string()]),
    }
}

/// State of the open Settings window.
#[derive(Default)]
pub struct SettingsState {
    pub page: SettingsPage,
    pub query: String,
}

/// Rows of a page; with a search query, only the rows whose label or hint has it, under the
/// title of their page.
struct Rows<'a> {
    query: String,
    pal: &'a Palette,
    /// Page title still to draw above the first matching row (search results).
    header: Option<&'static str>,
    shown: usize,
}

impl Rows<'_> {
    fn matches(&self, label: &str, hint: &str) -> bool {
        self.query.is_empty() || label.to_lowercase().contains(&self.query) || hint.to_lowercase().contains(&self.query)
    }
}

/// A row of a settings page: the label (and hint underneath) on the left, the control on the
/// right. The control gets the label's id for `labelled_by`.
fn setting_row(ui: &mut Ui, rows: &mut Rows, label: &str, hint: &str, add: impl FnOnce(&mut Ui, egui::Id)) {
    if !rows.matches(label, hint) {
        return;
    }
    if let Some(h) = rows.header.take() {
        if rows.shown > 0 {
            ui.add_space(10.0);
        }
        ui.label(RichText::new(h).strong().color(rows.pal.accent));
        ui.add_space(2.0);
    }
    rows.shown += 1;
    egui::Frame::new()
        .fill(rows.pal.bg_widget.gamma_multiply(0.55))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(12, 9))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let width = (ui.available_width() * 0.6).clamp(180.0, 420.0);
                let id = ui
                    .allocate_ui_with_layout(egui::vec2(width, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.set_width(width);
                        let l = ui.label(RichText::new(label).color(rows.pal.text_strong));
                        if !hint.is_empty() {
                            ui.add(egui::Label::new(RichText::new(hint).small().color(rows.pal.text_weak)).wrap());
                        }
                        l.id
                    })
                    .inner;
                // Controls sit at the right end: their width from the last frame sets the gap.
                let wid = id.with("controls");
                let last: f32 = ui.ctx().data(|d| d.get_temp(wid)).unwrap_or(0.0);
                ui.add_space((ui.available_width() - last - 2.0).max(0.0));
                let w = ui.horizontal(|ui| add(ui, id)).response.rect.width();
                if (w - last).abs() > 0.5 {
                    ui.ctx().data_mut(|d| d.insert_temp(wid, w));
                    ui.ctx().request_repaint();
                }
            });
        });
    ui.add_space(4.0);
}

/// An on / off switch (the look of design tools' settings), read by screen readers as a
/// checkbox named `label`.
fn toggle_switch(ui: &mut Ui, on: &mut bool, label: &str) -> egui::Response {
    let size = egui::vec2(34.0, 18.0);
    let (rect, mut r) = ui.allocate_exact_size(size, Sense::click());
    if r.clicked() {
        *on = !*on;
        r.mark_changed();
    }
    r.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, *on, label));
    if ui.is_rect_visible(rect) {
        let pal = theme::palette(ui.ctx());
        let k = ui.ctx().animate_bool_responsive(r.id, *on);
        let track = if *on { pal.accent } else { pal.bg_hover };
        let track = if r.hovered() { track.gamma_multiply(1.15) } else { track };
        ui.painter().rect_filled(rect, rect.height() / 2.0, track);
        let x = egui::lerp(rect.left() + 9.0..=rect.right() - 9.0, k);
        ui.painter().circle_filled(egui::pos2(x, rect.center().y), 6.5, Color32::WHITE);
    }
    r
}

impl IcStudio {
    /// The Settings window; `close` ends it.
    pub(super) fn settings_dialog(&mut self, ui: &mut Ui, st: &mut SettingsState, close: &mut bool) {
        let pal = theme::palette(ui.ctx());
        let screen = ui.ctx().content_rect();
        let width = (screen.width() - 80.0).clamp(560.0, 900.0);
        let height = (screen.height() - 160.0).clamp(320.0, 560.0);
        ui.set_width(width);
        let before = (self.settings.editors.clone(), self.settings.ui_scale);
        // A fixed size: the modal is centred on its size, so a size that followed the content
        // would move every control under the pointer while it settles.
        let fixed = |ui: &mut Ui, size: egui::Vec2| {
            ui.set_min_size(size);
            ui.set_max_size(size);
        };
        let body = egui::vec2(width, height);
        ui.allocate_ui_with_layout(body, egui::Layout::left_to_right(egui::Align::Min), |ui| {
            fixed(ui, body);
            // Sidebar: search and pages.
            ui.allocate_ui_with_layout(egui::vec2(190.0, height), egui::Layout::top_down(egui::Align::Min), |ui| {
                fixed(ui, egui::vec2(190.0, height));
                ui.label(RichText::new(t("Settings")).heading().strong());
                ui.add_space(6.0);
                let search = ui
                    .add(egui::TextEdit::singleline(&mut st.query).hint_text(t("Search settings")).desired_width(f32::INFINITY));
                search.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, t("Search settings")));
                if search.changed() && !st.query.is_empty() {
                    ui.ctx().request_repaint();
                }
                let mut group = "";
                for page in SettingsPage::ALL {
                    if page.group() != group {
                        group = page.group();
                        ui.add_space(8.0);
                        ui.label(RichText::new(group.to_uppercase()).small().color(pal.text_weak));
                    }
                    let selected = st.query.is_empty() && st.page == page;
                    let text = RichText::new(page.title()).color(if selected { pal.text_strong } else { pal.text });
                    let r = ui
                        .with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                            ui.add(egui::Button::selectable(selected, text).min_size(egui::vec2(0.0, 26.0)))
                        })
                        .inner;
                    if r.clicked() {
                        st.page = page;
                        st.query.clear();
                    }
                }
            });
            ui.separator();
            // The page, or the search results of every page.
            let size = egui::vec2(ui.available_width(), height);
            ui.allocate_ui_with_layout(size, egui::Layout::top_down(egui::Align::Min), |ui| {
                fixed(ui, size);
                let query = st.query.trim().to_lowercase();
                if query.is_empty() {
                    ui.label(RichText::new(st.page.title()).size(20.0).strong().color(pal.text_strong));
                    ui.label(RichText::new(st.page.blurb()).color(pal.text_weak));
                    ui.add_space(10.0);
                }
                egui::ScrollArea::vertical()
                    .id_salt("settings_page")
                    .max_height(ui.available_height())
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let mut rows = Rows { query: query.clone(), pal: &pal, header: None, shown: 0 };
                        let pages: Vec<SettingsPage> = if query.is_empty() { vec![st.page] } else { SettingsPage::ALL.to_vec() };
                        for page in pages {
                            rows.header = (!query.is_empty()).then(|| page.title());
                            self.settings_page(ui, page, &mut rows);
                        }
                        if rows.shown == 0 {
                            ui.add_space(20.0);
                            ui.label(RichText::new(tf("No setting matches '{}'", &[st.query.trim()])).color(pal.text_weak));
                        }
                    });
            });
        });
        ui.separator();
        ui.horizontal(|ui| {
            if st.query.is_empty()
                && st.page != SettingsPage::About
                && ui
                    .button(t("Restore defaults"))
                    .on_hover_text(tf("Set every setting of '{}' back to its default", &[st.page.title()]))
                    .clicked()
            {
                self.reset_settings_page(st.page);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(t("Close")).clicked() {
                    *close = true;
                }
            });
        });
        if self.settings.editors != before.0 {
            let d = self.settings.editors.clone();
            self.sch_eds.values_mut().for_each(|ed| d.apply_sch(ed));
            self.lay_eds.values_mut().for_each(|ed| d.apply_lay(ed));
        }
        if self.settings.ui_scale != before.1 && !ui.input(|i| i.pointer.any_down()) {
            ui.ctx().set_zoom_factor(self.settings.ui_scale);
        }
        if *close {
            ui.ctx().set_zoom_factor(self.settings.ui_scale);
        }
    }

    fn reset_settings_page(&mut self, page: SettingsPage) {
        let d = Settings::default();
        let s = &mut self.settings;
        match page {
            SettingsPage::General => {
                s.reopen_last = d.reopen_last;
                s.recent_max = d.recent_max;
                if s.beginner {
                    self.queue.push(Command::BeginnerMode(false));
                }
                if crate::i18n::current() != d.lang {
                    self.queue.push(Command::SetLanguage(d.lang));
                }
            }
            SettingsPage::Appearance => {
                if s.dark != d.dark {
                    self.queue.push(Command::ToggleTheme);
                }
                s.ui_scale = d.ui_scale;
            }
            SettingsPage::Editing => s.interaction = d.interaction,
            SettingsPage::Schematic => {
                s.mos_body_pin = d.mos_body_pin;
                s.editors.sch_grid = d.editors.sch_grid;
                s.editors.sch_annotate_op = d.editors.sch_annotate_op;
            }
            SettingsPage::Layout => {
                let sch = (s.editors.sch_grid, s.editors.sch_annotate_op);
                s.editors = d.editors;
                (s.editors.sch_grid, s.editors.sch_annotate_op) = sch;
            }
            SettingsPage::Simulation => {
                s.ngspice_path = d.ngspice_path;
                self.ngspice = ics_sim::ngspice::detect(&self.settings.ngspice_path);
            }
            SettingsPage::Saving => s.autosave_secs = d.autosave_secs,
            SettingsPage::Ai => s.llm = d.llm,
            SettingsPage::About => s.check_updates = d.check_updates,
        }
    }

    fn settings_page(&mut self, ui: &mut Ui, page: SettingsPage, rows: &mut Rows) {
        let pal = *rows.pal;
        match page {
            SettingsPage::General => {
                setting_row(ui, rows, t("Language"), t("Menus, tool windows, messages and the Learn course"), |ui, _| {
                    use crate::i18n::Lang;
                    for lang in [Lang::En, Lang::Ko] {
                        let on = crate::i18n::current() == lang;
                        let ok = lang == Lang::En || crate::i18n::korean_available();
                        let r = ui.add_enabled(ok, egui::RadioButton::new(on, lang.label())).on_disabled_hover_text(t(
                            "Korean needs a system font with Hangul (Malgun Gothic, Apple SD Gothic Neo or Noto Sans CJK)",
                        ));
                        if r.clicked() && !on {
                            self.queue.push(Command::SetLanguage(lang));
                        }
                    }
                });
                setting_row(
                    ui,
                    rows,
                    t("Beginner mode"),
                    t("Menus show the everyday design flow; specialist tools stay in the command palette"),
                    |ui, _| {
                        let mut on = self.settings.beginner;
                        if toggle_switch(ui, &mut on, t("Beginner mode")).changed() {
                            self.queue.push(Command::BeginnerMode(on));
                        }
                    },
                );
                setting_row(
                    ui,
                    rows,
                    t("Reopen last project"),
                    t("Open the project you worked on last when IC Studio starts"),
                    |ui, _| {
                        toggle_switch(ui, &mut self.settings.reopen_last, t("Reopen last project"));
                    },
                );
                setting_row(ui, rows, t("Recent projects"), t("How many projects File > Open Recent lists"), |ui, id| {
                    ui.add(egui::DragValue::new(&mut self.settings.recent_max).range(1..=30)).labelled_by(id);
                    let n = self.settings.recent.len();
                    if ui.add_enabled(n > 0, egui::Button::new(tf("Clear the list ({})", &[&n.to_string()]))).clicked() {
                        self.settings.recent.clear();
                    }
                    self.settings.recent.truncate(self.settings.recent_max);
                });
                let done = self.settings.learn.done.len();
                setting_row(
                    ui,
                    rows,
                    t("Learn course"),
                    &tf("{} of {} chapters done", &[&done.to_string(), &crate::learn::CHAPTERS.len().to_string()]),
                    |ui, _| {
                        if ui.button(t("Open")).clicked() {
                            self.queue.push(Command::Learn(None));
                        }
                        if ui
                            .add_enabled(done > 0 || !self.settings.learn.quiz.is_empty(), egui::Button::new(t("Reset progress")))
                            .clicked()
                        {
                            self.settings.learn = Default::default();
                        }
                    },
                );
            }
            SettingsPage::Appearance => {
                setting_row(ui, rows, t("Theme"), t("Dark is easier on the eyes for long layout sessions"), |ui, _| {
                    if ui.radio(self.settings.dark, t("Dark")).clicked() && !self.settings.dark {
                        self.queue.push(Command::ToggleTheme);
                    }
                    if ui.radio(!self.settings.dark, t("Light")).clicked() && self.settings.dark {
                        self.queue.push(Command::ToggleTheme);
                    }
                });
                setting_row(
                    ui,
                    rows,
                    t("UI scale"),
                    t("Size of text and controls; applied when you let go of the slider"),
                    |ui, id| {
                        ui.add(egui::Slider::new(&mut self.settings.ui_scale, 0.75..=1.75).step_by(0.05)).labelled_by(id);
                        if ui.add_enabled(self.settings.ui_scale != 1.0, egui::Button::new(t("Reset"))).clicked() {
                            self.settings.ui_scale = 1.0;
                        }
                    },
                );
            }
            SettingsPage::Editing => {
                for p in Profile::ALL {
                    let label = match p {
                        Profile::Virtuoso => t("Virtuoso style"),
                        Profile::Conventional => t("Conventional style"),
                    };
                    setting_row(ui, rows, label, t(p.describe()), |ui, id| {
                        ui.radio_value(&mut self.settings.interaction, p, t("Use")).labelled_by(id);
                    });
                }
                setting_row(ui, rows, t("Keyboard shortcuts"), t("Every key of the editors and the menus"), |ui, _| {
                    if ui.button(t("Show…")).clicked() {
                        self.queue.push(Command::Shortcuts);
                    }
                });
            }
            SettingsPage::Schematic => {
                setting_row(ui, rows, t("MOS symbol"), t("How new NMOS / PMOS devices are drawn"), |ui, id| {
                    ui.radio_value(&mut self.settings.mos_body_pin, true, t("Body pin (4-terminal)"))
                        .labelled_by(id)
                        .on_hover_text(t("New NMOS / PMOS devices have a body pin to wire"));
                    ui.radio_value(&mut self.settings.mos_body_pin, false, t("Arrow on source (3-terminal)"))
                        .labelled_by(id)
                        .on_hover_text(t(
                            "New NMOS / PMOS devices hide the body: it is on ground (NMOS) or vdd (PMOS), set by the bulk parameter",
                        ));
                });
                let e = &mut self.settings.editors;
                setting_row(ui, rows, t("Grid"), t("Draw the connection grid behind the schematic"), |ui, _| {
                    toggle_switch(ui, &mut e.sch_grid, t("Grid"));
                });
                setting_row(
                    ui,
                    rows,
                    t("Operating point on the schematic"),
                    t("After a simulation, show node voltages and device currents next to the devices"),
                    |ui, _| {
                        toggle_switch(ui, &mut e.sch_annotate_op, t("Operating point on the schematic"));
                    },
                );
            }
            SettingsPage::Layout => {
                let e = &mut self.settings.editors;
                let items: [(&mut bool, &str, &str); 7] = [
                    (&mut e.lay_grid, t("Grid"), t("Draw the snap grid behind the layout")),
                    (&mut e.lay_gravity, t("Gravity"), t("The cursor snaps to nearby vertices and edges")),
                    (
                        &mut e.lay_drd,
                        t("Spacing check while drawing"),
                        t("Show width and spacing violations while drawing and moving (DRD)"),
                    ),
                    (
                        &mut e.lay_magnet,
                        t("Magnet"),
                        t("Pull drawn and moved geometry to exactly the minimum spacing from its neighbours"),
                    ),
                    (&mut e.lay_auto_abut, t("Auto abut"), t("A moved MOS PCell dropped next to a compatible one abuts it")),
                    (
                        &mut e.lay_snap_tracks,
                        t("Snap wires to tracks"),
                        t("Path points snap onto the active layer's tracks and take the track's width"),
                    ),
                    (&mut e.lay_flylines, t("Flylines"), t("Draw the schematic connections that are not routed yet")),
                ];
                for (on, label, hint) in items {
                    setting_row(ui, rows, label, hint, |ui, _| {
                        toggle_switch(ui, on, label);
                    });
                }
            }
            SettingsPage::Simulation => {
                setting_row(
                    ui,
                    rows,
                    t("ngspice program"),
                    t("Used for the ngspice backend; without it the built-in engine runs every analysis"),
                    |ui, id| {
                        ui.add(egui::TextEdit::singleline(&mut self.settings.ngspice_path).desired_width(240.0)).labelled_by(id);
                        if ui.button(t("Detect")).clicked() {
                            self.ngspice = ics_sim::ngspice::detect(&self.settings.ngspice_path);
                        }
                    },
                );
                let status = match &self.ngspice {
                    Some(v) => tf("Found: {}", &[v]),
                    None => t("not found — built-in engine will be used").to_string(),
                };
                setting_row(ui, rows, t("ngspice status"), "", |ui, _| {
                    ui.colored_label(if self.ngspice.is_some() { pal.ok } else { pal.text_weak }, status);
                });
            }
            SettingsPage::Saving => {
                setting_row(
                    ui,
                    rows,
                    t("Autosave"),
                    t(
                        "A copy of unsaved work goes to a recovery folder outside the project; after a crash IC Studio offers it when the project opens again",
                    ),
                    |ui, id| {
                        egui::ComboBox::from_id_salt("autosave")
                            .selected_text(autosave_label(self.settings.autosave_secs))
                            .show_ui(ui, |ui| {
                                for s in AUTOSAVE {
                                    ui.selectable_value(&mut self.settings.autosave_secs, s, autosave_label(s));
                                }
                            })
                            .response
                            .labelled_by(id);
                    },
                );
                let dir = crate::recovery::base_dir().map(|d| d.display().to_string()).unwrap_or_default();
                setting_row(ui, rows, t("Recovery folder"), &dir, |ui, _| {
                    if ui.button(t("Copy path")).clicked() {
                        ui.ctx().copy_text(dir.clone());
                    }
                });
            }
            SettingsPage::Ai => {
                setting_row(ui, rows, t("AI model"), &self.settings.llm.describe(), |ui, _| {
                    if ui.button(t("AI Models…")).clicked() {
                        self.ai_models.open = true;
                    }
                });
                setting_row(
                    ui,
                    rows,
                    t("AI agents (MCP)"),
                    t("Let AI agents such as Claude Code drive IC Studio. They start the ics command line tool with ics mcp; \
                       while this window is open they work on its project."),
                    |ui, _| {
                        if ui
                            .button(t("Add to Claude Code"))
                            .on_hover_text(t("Runs: claude mcp add --scope user ic-studio -- <folder>/ics mcp"))
                            .clicked()
                        {
                            self.queue.push(Command::AddMcpToClaudeCode);
                        }
                        if ui
                            .button(t("Copy MCP config"))
                            .on_hover_text(t("JSON for other MCP clients (Cursor, Claude Desktop, ...)"))
                            .clicked()
                        {
                            self.queue.push(Command::CopyMcpConfig);
                        }
                    },
                );
            }
            SettingsPage::About => {
                setting_row(ui, rows, t("Version"), &tf("Installed version {}", &[crate::update::CURRENT]), |ui, _| {
                    if ui.button(t("Check now")).clicked() {
                        self.queue.push(Command::CheckForUpdates);
                    }
                    if ui.button(t("About IC Studio")).clicked() {
                        self.queue.push(Command::About);
                    }
                });
                setting_row(
                    ui,
                    rows,
                    t("Check for updates"),
                    t("Look for a newer release on GitHub when IC Studio starts"),
                    |ui, _| {
                        toggle_switch(ui, &mut self.settings.check_updates, t("Check for updates"));
                    },
                );
            }
        }
    }
}
