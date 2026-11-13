//! IC Studio desktop application entry point.

#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

use std::sync::{Arc, OnceLock};

fn main() -> eframe::Result {
    if std::env::args().nth(1).as_deref() == Some("--mcp") {
        ics_mcp::server::serve_stdio();
        return Ok(());
    }
    let args = ics_studio::parse_args();
    ics_studio::update::cleanup();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("IC Studio")
            .with_app_id("ic-studio")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 600.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    let ctx: Arc<OnceLock<eframe::egui::Context>> = Arc::default();
    let wake = ctx.clone();
    let bridge = ics_mcp::bridge::Server::start(move || {
        if let Some(c) = wake.get() {
            c.request_repaint();
        }
    });
    let (server, rx) = match bridge {
        Ok((s, rx)) => (Some(s), Some(rx)),
        Err(e) => {
            eprintln!("ic-studio: MCP live mode unavailable: {e}");
            (None, None)
        }
    };
    let res = eframe::run_native(
        "IC Studio",
        options,
        Box::new(move |cc| {
            let _ = ctx.set(cc.egui_ctx.clone());
            let mut app = ics_studio::app::IcStudio::new(cc, args);
            app.check_updates_on_start(&cc.egui_ctx);
            if let Some(rx) = rx {
                app.attach_bridge(rx);
            }
            Ok(Box::new(app))
        }),
    );
    drop(server);
    res
}
