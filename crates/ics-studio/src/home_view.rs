//! Home workspace: welcome screen, recent projects and the project overview with cell cards.

use std::path::PathBuf;

use eframe::egui::{self, Align2, FontId, RichText, Sense, Stroke, StrokeKind, Vec2};
use ics_core::hier::Hier;
use ics_core::{CellRef, Project, ViewKind};

use crate::app::{Command, Workspace};
use crate::canvas::View2D;
use crate::icons::{self, Icon};
use crate::layout_view::{LayoutDrawOpts, draw_layout};
use crate::schematic_view::{DrawOpts, draw_schematic, schematic_bounds};
use crate::theme;

fn action_card(ui: &mut egui::Ui, icon: Icon, title: &str, sub: &str, color: egui::Color32) -> bool {
    let pal = theme::palette(ui.ctx());
    let (r, p) = ui.allocate_painter(Vec2::new(230.0, 74.0), Sense::click());
    let fill = if r.hovered() { pal.bg_hover } else { pal.bg_widget };
    p.rect_filled(r.rect, 10.0, fill);
    if r.hovered() {
        p.rect_stroke(r.rect, 10.0, Stroke::new(1.0, color.gamma_multiply(0.7)), StrokeKind::Inside);
    }
    let ir = egui::Rect::from_min_size(r.rect.min + Vec2::new(14.0, 19.0), Vec2::splat(36.0));
    p.rect_filled(ir, 8.0, color.gamma_multiply(0.18));
    icons::paint(&p, ir.shrink(6.0), icon, color);
    p.text(ir.right_top() + Vec2::new(14.0, 2.0), Align2::LEFT_TOP, title, FontId::proportional(15.0), pal.text);
    p.text(ir.right_top() + Vec2::new(14.0, 22.0), Align2::LEFT_TOP, sub, FontId::proportional(12.0), pal.text_weak);
    r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

pub fn welcome(ui: &mut egui::Ui, recent: &[PathBuf], q: &mut Vec<Command>) {
    let pal = theme::palette(ui.ctx());
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(40.0);
        ui.horizontal(|ui| {
            ui.add_space(48.0);
            ui.vertical(|ui| {
                ui.label(RichText::new("IC Studio").size(38.0).strong().color(pal.text));
                ui.label(
                    RichText::new("Full-custom IC design — schematic, simulation and layout in one place.")
                        .size(16.0)
                        .color(pal.text_weak),
                );
                ui.add_space(28.0);
                theme::section(ui, "Start");
                ui.horizontal_wrapped(|ui| {
                    if action_card(ui, Icon::Library, "Open demo project", "Inverter, RC filter, clipper", pal.accent) {
                        q.push(Command::NewDemoProject);
                    }
                    if action_card(ui, Icon::Cell, "New project…", "Empty, generic 180 nm PDK", pal.schematic) {
                        q.push(Command::NewProjectDialog);
                    }
                    if action_card(ui, Icon::Home, "Open project…", "Browse for a project folder", pal.layout) {
                        q.push(Command::OpenProjectDialog);
                    }
                });
                ui.add_space(24.0);
                if !recent.is_empty() {
                    theme::section(ui, "Recent projects");
                    for p in recent.iter().take(8) {
                        let name = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                        ui.horizontal(|ui| {
                            if ui.link(RichText::new(&name).size(14.0)).clicked() {
                                q.push(Command::OpenProject(p.clone()));
                            }
                            ui.label(RichText::new(p.display().to_string()).small().color(pal.text_weak));
                        });
                    }
                    ui.add_space(24.0);
                }
                theme::section(ui, "Workflow");
                ui.horizontal_wrapped(|ui| {
                    for (icon, title, text, color) in [
                        (
                            Icon::Schematic,
                            "Schematic",
                            "Place devices, wire nets, label pins. Live connectivity and netlisting.",
                            pal.schematic,
                        ),
                        (
                            Icon::Simulation,
                            "Simulation",
                            "OP, DC, AC and transient on the built-in engine or ngspice.",
                            pal.simulation,
                        ),
                        (Icon::Layout, "Layout", "Polygon editing on PDK layers, DRC and GDSII import/export.", pal.layout),
                    ] {
                        egui::Frame::new().fill(pal.bg_widget).corner_radius(10).inner_margin(14).show(ui, |ui| {
                            ui.set_width(230.0);
                            ui.set_min_height(64.0);
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    let (r, p) = ui.allocate_painter(Vec2::splat(22.0), Sense::hover());
                                    icons::paint(&p, r.rect, icon, color);
                                    ui.label(RichText::new(title).strong().size(14.0));
                                });
                                ui.add(egui::Label::new(RichText::new(text).color(pal.text_weak)).wrap());
                            });
                        });
                    }
                });
            });
        });
    });
}

pub fn project_overview(ui: &mut egui::Ui, project: &Project, hier: &mut Hier, q: &mut Vec<Command>) {
    let pal = theme::palette(ui.ctx());
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(24.0);
        ui.horizontal(|ui| {
            ui.add_space(32.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(&project.name).size(30.0).strong());
                let loc = project.root.as_ref().map(|r| r.display().to_string()).unwrap_or("not saved yet".into());
                ui.label(
                    RichText::new(format!("{}  ·  {}  ·  {}", project.description, project.tech.name, loc)).color(pal.text_weak),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("+ New cell").clicked() {
                        q.push(Command::NewCellDialog);
                    }
                    if ui.button("⬆ Import GDS…").clicked() {
                        q.push(Command::ImportGds);
                    }
                    if ui.button("⬇ Export library GDS…").clicked() {
                        q.push(Command::ExportLibraryGds);
                    }
                });
                ui.add_space(18.0);
                for lib in &project.libraries {
                    theme::section(ui, &format!("Library · {} ({} cells)", lib.name, lib.cells.len()));
                    if lib.cells.is_empty() {
                        ui.label(RichText::new("No cells yet. Create one to start designing.").color(pal.text_weak));
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(14.0, 14.0);
                        for cell in &lib.cells {
                            let r = CellRef::new(&lib.name, &cell.name);
                            let (resp, p) = ui.allocate_painter(Vec2::new(250.0, 200.0), Sense::click());
                            let rect = resp.rect;
                            p.rect_filled(rect, 10.0, if resp.hovered() { pal.bg_hover } else { pal.bg_widget });
                            let thumb =
                                egui::Rect::from_min_size(rect.min + Vec2::new(8.0, 8.0), Vec2::new(rect.width() - 16.0, 120.0));
                            p.rect_filled(thumb, 6.0, pal.bg_canvas);
                            let tp = p.with_clip_rect(thumb);
                            if let Some(s) = &cell.schematic {
                                if let Some((a, b)) = schematic_bounds(s) {
                                    let mut v = View2D::new(4.0, false, 0.5, 50.0);
                                    v.fit(thumb, a, b, 10.0);
                                    let conn = ics_core::connectivity::extract(s);
                                    draw_schematic(
                                        &tp,
                                        &v,
                                        thumb,
                                        s,
                                        &conn,
                                        &DrawOpts {
                                            pal,
                                            selection: None,
                                            highlight_net: None,
                                            op: None,
                                            show_grid: false,
                                            thumbnail: true,
                                        },
                                    );
                                }
                            } else if let Some(l) = &cell.layout
                                && let Some(b) = hier.get_for(&r, l, project).bbox
                            {
                                let mut v = View2D::new(0.01, true, 1e-9, 10.0);
                                v.fit(thumb, (b.x0 as f64, b.y0 as f64), (b.x1 as f64, b.y1 as f64), 10.0);
                                let vis = vec![true; project.tech.layers.len()];
                                draw_layout(
                                    &tp,
                                    &v,
                                    thumb,
                                    &r,
                                    l,
                                    project,
                                    hier,
                                    &project.tech,
                                    &LayoutDrawOpts {
                                        pal,
                                        visible: &vis,
                                        selection: None,
                                        thumbnail: true,
                                        show_grid: false,
                                        grid: 10,
                                        max_depth: 32,
                                    },
                                );
                            }
                            p.text(
                                thumb.left_bottom() + Vec2::new(4.0, 10.0),
                                Align2::LEFT_TOP,
                                &cell.name,
                                FontId::proportional(15.0),
                                pal.text,
                            );
                            let desc: String = cell.description.chars().take(36).collect();
                            p.text(
                                thumb.left_bottom() + Vec2::new(4.0, 32.0),
                                Align2::LEFT_TOP,
                                desc,
                                FontId::proportional(11.5),
                                pal.text_weak,
                            );
                            // view chips
                            let mut x = rect.min.x + 12.0;
                            let y = rect.max.y - 22.0;
                            for (vk, color, icon) in [
                                (ViewKind::Schematic, pal.schematic, Icon::Schematic),
                                (ViewKind::Layout, pal.layout, Icon::Layout),
                            ] {
                                if cell.has_view(vk) {
                                    let chip = egui::Rect::from_min_size(egui::pos2(x, y), Vec2::new(86.0, 16.0));
                                    p.rect_filled(chip, 8.0, color.gamma_multiply(0.15));
                                    icons::paint(
                                        &p,
                                        egui::Rect::from_min_size(chip.min + Vec2::new(4.0, 1.0), Vec2::splat(14.0)),
                                        icon,
                                        color,
                                    );
                                    p.text(
                                        chip.min + Vec2::new(21.0, 1.5),
                                        Align2::LEFT_TOP,
                                        vk.label(),
                                        FontId::proportional(11.0),
                                        color,
                                    );
                                    x += 92.0;
                                }
                            }
                            if resp.clicked() {
                                let ws = if cell.schematic.is_some() { Workspace::Schematic } else { Workspace::Layout };
                                q.push(Command::OpenCell(r.clone(), Some(ws)));
                            }
                            resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                        }
                    });
                    ui.add_space(16.0);
                }
            });
        });
    });
}
