//! Tool windows that dock inside the main window or pop out into their own native window.

use crate::icons::{self, Icon};

/// Shows `add` in an in-app window (the default) or, once popped out, in a separate OS window.
/// The pop-out state is remembered per `id`; closing either window clears `open`.
pub fn tool_window<R>(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    size: [f32; 2],
    open: &mut bool,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<R> {
    if !*open {
        return None;
    }
    let key = egui::Id::unique(("tool_popped", id));
    let popped = ctx.data_mut(|d| d.get_persisted::<bool>(key)).unwrap_or(false);
    let mut toggle = false;
    let mut add = Some(add);
    let r = if popped {
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_icon(crate::app_icon::icon())
            .with_inner_size(size)
            .with_min_inner_size([360.0, 300.0]);
        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of(id), builder, |ui, _class| {
            if ui.ctx().input(|i| i.viewport().close_requested()) {
                *open = false;
            }
            let pal = crate::theme::palette(ui.ctx());
            egui::Panel::top(egui::Id::unique(("tool_dock_bar", id)))
                .frame(egui::Frame::new().fill(pal.bg_panel).inner_margin(egui::Margin { left: 8, right: 8, top: 4, bottom: 0 }))
                .show_separator_line(false)
                .show(ui, |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        toggle = icon_button(ui, Icon::DockIn, "Dock", "Move back into the main window").clicked();
                    });
                });
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(pal.bg_panel).inner_margin(8))
                .show(ui, |ui| add.take().map(|f| f(ui)))
                .inner
        })
    } else {
        let mut shown = true;
        let resp = egui::Window::new(title)
            .id(egui::Id::unique(("tool_window", id)))
            .open(&mut shown)
            .default_size(size)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .resizable(true)
            .show(ctx, |ui| add.take().map(|f| f(ui)));
        *open &= shown;
        let resp = resp?;
        toggle = pop_out_button(ctx, id, &resp.response);
        resp.inner.flatten()
    };
    if toggle {
        ctx.data_mut(|d| d.insert_persisted(key, !popped));
    }
    r
}

/// Paints a pop-out button in the window's title bar, just left of the close button.
fn pop_out_button(ctx: &egui::Context, id: &str, window: &egui::Response) -> bool {
    let style = ctx.global_style();
    let m = style.spacing.window_margin;
    let h = ctx.fonts_mut(|f| f.row_height(&egui::TextStyle::Heading.resolve(&style)));
    let close = egui::Rect::from_min_size(
        egui::pos2(window.rect.max.x - m.right as f32 - h, window.rect.min.y + m.top as f32),
        egui::vec2(h, h),
    );
    let rect = close.translate(egui::vec2(-(h + style.spacing.item_spacing.x), 0.0));
    let ui = egui::Ui::new(
        ctx.clone(),
        egui::Id::unique(("tool_pop_out", id)),
        egui::UiBuilder::new().layer_id(window.layer_id).max_rect(rect),
    );
    let resp = ui.interact(rect, ui.scope_id().with("btn"), egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Pop out"));
    let v = ui.style().interact(&resp);
    let r = rect.shrink((h - style.spacing.icon_width) * 0.5).expand(v.expansion);
    icons::paint(ui.painter(), r, Icon::PopOut, v.fg_stroke.color);
    resp.on_hover_text("Pop out into a separate window").clicked()
}

fn icon_button(ui: &mut egui::Ui, icon: Icon, label: &str, tip: &str) -> egui::Response {
    let s = ui.spacing().icon_width + 4.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(s, s), egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let v = ui.style().interact(&resp);
    if resp.hovered() {
        ui.painter().rect_filled(rect, 3.0, v.weak_bg_fill);
    }
    icons::paint(ui.painter(), rect.shrink(2.0), icon, v.fg_stroke.color);
    resp.on_hover_text(tip)
}
