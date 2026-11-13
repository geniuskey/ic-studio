//! The IC Studio app icon (same design as `docs/theme/favicon.svg`), rasterized at startup.

use std::sync::{Arc, OnceLock};

pub fn icon() -> Arc<egui::IconData> {
    static ICON: OnceLock<Arc<egui::IconData>> = OnceLock::new();
    ICON.get_or_init(|| {
        const SIZE: u32 = 256;
        Arc::new(egui::IconData { rgba: rgba(SIZE), width: SIZE, height: SIZE })
    })
    .clone()
}

fn rgba(size: u32) -> Vec<u8> {
    const SS: u32 = 4;
    const PINS: [f32; 3] = [26.0, 32.0, 38.0];
    let mut segs = vec![];
    for p in PINS {
        segs.push(([p, 20.0], [p, 13.0]));
        segs.push(([p, 44.0], [p, 51.0]));
        segs.push(([20.0, p], [13.0, p]));
        segs.push(([44.0, p], [51.0, p]));
    }
    let (c0, c1) = ([0x6cu8, 0x5c, 0xe7].map(f32::from), [0x3fu8, 0x8f, 0xf5].map(f32::from));
    let scale = 64.0 / size as f32;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            let (mut acc, mut alpha) = ([0.0f32; 3], 0.0f32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (px as f32 + (sx as f32 + 0.5) / SS as f32) * scale;
                    let y = (py as f32 + (sy as f32 + 0.5) / SS as f32) * scale;
                    if round_rect(x, y, 32.0, 28.0, 14.0) > 0.0 {
                        continue;
                    }
                    let t = ((x - 4.0) + (y - 4.0)) / 112.0;
                    let stroke =
                        round_rect(x, y, 32.0, 12.0, 4.0).abs() <= 1.5 || segs.iter().any(|&(a, b)| segment(x, y, a, b) <= 1.5);
                    let w = if stroke { 0.95 } else { 0.0 };
                    for i in 0..3 {
                        acc[i] += (c0[i] + (c1[i] - c0[i]) * t) * (1.0 - w) + 255.0 * w;
                    }
                    alpha += 1.0;
                }
            }
            let n = (SS * SS) as f32;
            for c in acc {
                out.push(if alpha > 0.0 { (c / alpha).round() as u8 } else { 0 });
            }
            out.push((alpha / n * 255.0).round() as u8);
        }
    }
    out
}

fn round_rect(x: f32, y: f32, c: f32, half: f32, r: f32) -> f32 {
    let qx = (x - c).abs() - (half - r);
    let qy = (y - c).abs() - (half - r);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

fn segment(x: f32, y: f32, a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let t = (((x - a[0]) * dx + (y - a[1]) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    (x - a[0] - t * dx).hypot(y - a[1] - t * dy)
}
