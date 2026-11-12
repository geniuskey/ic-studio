//! IC Studio desktop application entry point.

#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

fn main() -> eframe::Result {
    let args = ics_studio::parse_args();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("IC Studio")
            .with_app_id("ic-studio")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 600.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native("IC Studio", options, Box::new(|cc| Ok(Box::new(ics_studio::app::IcStudio::new(cc, args)))))
}
