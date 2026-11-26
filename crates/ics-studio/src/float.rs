//! Tool windows that dock inside the main window or pop out into their own native window.

use crate::i18n::t;
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
            .with_title(t(title))
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
        // Open inside the work area, against its right edge: the Explorer, the editor's left
        // panel (analyses, layers) and the console stay visible beside a narrower window.
        let area = work_area(ctx).unwrap_or_else(|| ctx.content_rect());
        let gap = 8.0;
        let room = (area.size() - egui::vec2(2.0 * gap, 2.0 * gap)).max(egui::vec2(360.0, 300.0));
        let resp = egui::Window::new(crate::theme::window_title(t(title)))
            .id(egui::Id::unique(("tool_window", id)))
            .open(&mut shown)
            .default_size(egui::vec2(size[0], size[1]).min(room))
            .pivot(egui::Align2::RIGHT_TOP)
            .default_pos(area.right_top() + egui::vec2(-gap, gap))
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

fn work_area_id() -> egui::Id {
    egui::Id::unique("ics_tool_work_area")
}

/// Records the area tool windows open in: the editor area, between the side panels and above
/// the console. Called by the app each frame before the tool windows show.
pub fn set_work_area(ctx: &egui::Context, rect: egui::Rect) {
    ctx.data_mut(|d| d.insert_temp(work_area_id(), rect));
}

fn work_area(ctx: &egui::Context) -> Option<egui::Rect> {
    ctx.data(|d| d.get_temp(work_area_id()))
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
    resp.on_hover_text(t("Pop out into a separate window")).clicked()
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
    resp.on_hover_text(t(tip))
}
