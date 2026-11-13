//! Tool windows that float inside the main window or pop out into their own native window.

/// Shows `add` in a separate OS window, or in an in-app window when `docked`.
/// A button at the top switches between the two; closing either clears `open`.
pub fn tool_window<R>(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    size: [f32; 2],
    open: &mut bool,
    docked: &mut bool,
    mut add: impl FnMut(&mut egui::Ui) -> R,
) -> Option<R> {
    if !*open {
        return None;
    }
    let mut toggle = false;
    let mut header = |ui: &mut egui::Ui, detached: bool| {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (text, tip) = if detached {
                    ("Dock", "Move back into the main window")
                } else {
                    ("Pop out", "Move into a separate window")
                };
                toggle |= ui.small_button(text).on_hover_text(tip).clicked();
            });
        });
    };
    let r = if !*docked {
        let builder =
            egui::ViewportBuilder::default().with_title(title).with_inner_size(size).with_min_inner_size([360.0, 300.0]);
        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of(id), builder, |ui, _class| {
            if ui.ctx().input(|i| i.viewport().close_requested()) {
                *open = false;
            }
            let pal = crate::theme::palette(ui.ctx());
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(pal.bg_panel).inner_margin(8))
                .show(ui, |ui| {
                    header(ui, true);
                    add(ui)
                })
                .inner
        })
    } else {
        let mut shown = true;
        let r = egui::Window::new(title)
            .id(egui::Id::unique(id))
            .open(&mut shown)
            .default_size(size)
            .resizable(true)
            .show(ctx, |ui| {
                header(ui, false);
                add(ui)
            })
            .and_then(|r| r.inner);
        *open &= shown;
        r?
    };
    if toggle {
        *docked = !*docked;
    }
    Some(r)
}
