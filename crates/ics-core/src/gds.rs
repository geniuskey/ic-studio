//! Minimal GDSII stream writer and reader (BOUNDARY, PATH and TEXT elements).

use crate::geom::IRect;
use crate::layout::{Geometry, Layout};
use crate::tech::Tech;

const HEADER: u16 = 0x0002;
const BGNLIB: u16 = 0x0102;
const LIBNAME: u16 = 0x0206;
const UNITS: u16 = 0x0305;
const ENDLIB: u16 = 0x0400;
const BGNSTR: u16 = 0x0502;
const STRNAME: u16 = 0x0606;
const ENDSTR: u16 = 0x0700;
const BOUNDARY: u16 = 0x0800;
const PATH: u16 = 0x0900;
const TEXT: u16 = 0x0C00;
const LAYER: u16 = 0x0D02;
const DATATYPE: u16 = 0x0E02;
const WIDTH: u16 = 0x0F03;
const XY: u16 = 0x1003;
const ENDEL: u16 = 0x1100;
const TEXTTYPE: u16 = 0x1602;
const STRING: u16 = 0x1906;
const MAG: u16 = 0x1B05;
const PATHTYPE: u16 = 0x2102;

fn real8(v: f64) -> [u8; 8] {
    if v == 0.0 {
        return [0; 8];
    }
    let sign = v < 0.0;
    let mut m = v.abs();
    let mut e: i32 = 64;
    while m >= 1.0 {
        m /= 16.0;
        e += 1;
    }
    while m < 1.0 / 16.0 {
        m *= 16.0;
        e -= 1;
    }
    let mant = (m * (1u64 << 56) as f64).round() as u64;
    let mut out = mant.to_be_bytes();
    out[0] = ((sign as u8) << 7) | (e as u8 & 0x7f);
    out
}

fn from_real8(b: &[u8]) -> f64 {
    let sign = b[0] & 0x80 != 0;
    let e = (b[0] & 0x7f) as i32 - 64;
    let mut mant = 0u64;
    for &x in &b[1..8] {
        mant = (mant << 8) | x as u64;
    }
    let v = mant as f64 / (1u64 << 56) as f64 * 16f64.powi(e);
    if sign { -v } else { v }
}

struct W(Vec<u8>);

impl W {
    fn rec(&mut self, tag: u16, data: &[u8]) {
        let len = (data.len() + 4) as u16;
        self.0.extend_from_slice(&len.to_be_bytes());
        self.0.extend_from_slice(&tag.to_be_bytes());
        self.0.extend_from_slice(data);
    }
    fn i16s(&mut self, tag: u16, v: &[i16]) {
        let d: Vec<u8> = v.iter().flat_map(|x| x.to_be_bytes()).collect();
        self.rec(tag, &d);
    }
    fn i32s(&mut self, tag: u16, v: &[i32]) {
        let d: Vec<u8> = v.iter().flat_map(|x| x.to_be_bytes()).collect();
        self.rec(tag, &d);
    }
    fn str(&mut self, tag: u16, s: &str) {
        let mut d = s.as_bytes().to_vec();
        if d.len() % 2 == 1 {
            d.push(0);
        }
        self.rec(tag, &d);
    }
    fn xy(&mut self, pts: &[(i64, i64)]) {
        let v: Vec<i32> = pts.iter().flat_map(|&(x, y)| [x as i32, y as i32]).collect();
        self.i32s(XY, &v);
    }
}

/// Serialize the given cells to a GDSII byte stream.
pub fn write_gds(lib_name: &str, cells: &[(&str, &Layout)], tech: &Tech) -> Vec<u8> {
    let mut w = W(Vec::new());
    let date: [i16; 12] = [2026, 1, 1, 0, 0, 0, 2026, 1, 1, 0, 0, 0];
    w.i16s(HEADER, &[600]);
    w.i16s(BGNLIB, &date);
    w.str(LIBNAME, lib_name);
    let user_per_dbu = 1.0 / tech.dbu_per_um as f64;
    let meters_per_dbu = 1e-6 / tech.dbu_per_um as f64;
    let mut units = real8(user_per_dbu).to_vec();
    units.extend_from_slice(&real8(meters_per_dbu));
    w.rec(UNITS, &units);
    for (name, layout) in cells {
        w.i16s(BGNSTR, &date);
        w.str(STRNAME, name);
        for s in &layout.shapes {
            let Some(l) = tech.layer(s.layer) else { continue };
            let (layer, dt) = (l.gds_layer as i16, l.gds_datatype as i16);
            match &s.geom {
                Geometry::Rect(r) => {
                    w.rec(BOUNDARY, &[]);
                    w.i16s(LAYER, &[layer]);
                    w.i16s(DATATYPE, &[dt]);
                    w.xy(&[(r.x0, r.y0), (r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1), (r.x0, r.y0)]);
                    w.rec(ENDEL, &[]);
                }
                Geometry::Polygon { points } if points.len() >= 3 => {
                    w.rec(BOUNDARY, &[]);
                    w.i16s(LAYER, &[layer]);
                    w.i16s(DATATYPE, &[dt]);
                    let mut p = points.clone();
                    p.push(points[0]);
                    w.xy(&p);
                    w.rec(ENDEL, &[]);
                }
                Geometry::Path { points, width } if points.len() >= 2 => {
                    w.rec(PATH, &[]);
                    w.i16s(LAYER, &[layer]);
                    w.i16s(DATATYPE, &[dt]);
                    w.i16s(PATHTYPE, &[2]);
                    w.i32s(WIDTH, &[*width as i32]);
                    w.xy(points);
                    w.rec(ENDEL, &[]);
                }
                Geometry::Label { text, at, size } => {
                    w.rec(TEXT, &[]);
                    w.i16s(LAYER, &[layer]);
                    w.i16s(TEXTTYPE, &[dt]);
                    w.rec(MAG, &real8(*size as f64 / tech.dbu_per_um as f64));
                    w.xy(&[*at]);
                    w.str(STRING, text);
                    w.rec(ENDEL, &[]);
                }
                _ => {}
            }
        }
        w.rec(ENDSTR, &[]);
    }
    w.rec(ENDLIB, &[]);
    w.0
}

#[derive(Debug, Default)]
pub struct GdsImport {
    pub cells: Vec<(String, Layout)>,
    pub skipped_unknown_layers: usize,
    pub dbu_meters: f64,
}

/// Parse a GDSII stream. Shapes on layers unknown to `tech` are skipped.
pub fn read_gds(data: &[u8], tech: &Tech) -> Result<GdsImport, String> {
    let mut out = GdsImport::default();
    let mut i = 0usize;
    let mut cur: Option<(String, Layout)> = None;
    #[derive(Default)]
    struct El {
        kind: u16,
        layer: u16,
        dt: u16,
        width: i64,
        xy: Vec<(i64, i64)>,
        text: String,
        mag: f64,
    }
    let mut el: Option<El> = None;
    let mut scale = 1.0f64; // file dbu -> tech dbu
    while i + 4 <= data.len() {
        let len = u16::from_be_bytes([data[i], data[i + 1]]) as usize;
        let tag = u16::from_be_bytes([data[i + 2], data[i + 3]]);
        if len < 4 || i + len > data.len() {
            return Err(format!("corrupt record at byte {i}"));
        }
        let d = &data[i + 4..i + len];
        i += len;
        let i16_at = |k: usize| i16::from_be_bytes([d[2 * k], d[2 * k + 1]]);
        let s = || String::from_utf8_lossy(d).trim_end_matches('\0').to_string();
        match tag {
            UNITS if d.len() >= 16 => {
                out.dbu_meters = from_real8(&d[8..16]);
                let tech_dbu_m = 1e-6 / tech.dbu_per_um as f64;
                scale = out.dbu_meters / tech_dbu_m;
            }
            BGNSTR => cur = Some((String::new(), Layout::default())),
            STRNAME => {
                if let Some(c) = cur.as_mut() {
                    c.0 = s();
                }
            }
            ENDSTR => {
                if let Some(c) = cur.take() {
                    out.cells.push(c);
                }
            }
            BOUNDARY | PATH | TEXT => el = Some(El { kind: tag, mag: 1.0, ..Default::default() }),
            LAYER if d.len() >= 2 => {
                if let Some(e) = el.as_mut() {
                    e.layer = i16_at(0) as u16;
                }
            }
            DATATYPE | TEXTTYPE if d.len() >= 2 => {
                if let Some(e) = el.as_mut() {
                    e.dt = i16_at(0) as u16;
                }
            }
            WIDTH if d.len() >= 4 => {
                if let Some(e) = el.as_mut() {
                    e.width = i32::from_be_bytes([d[0], d[1], d[2], d[3]]).unsigned_abs() as i64;
                }
            }
            MAG if d.len() >= 8 => {
                if let Some(e) = el.as_mut() {
                    e.mag = from_real8(d);
                }
            }
            XY => {
                if let Some(e) = el.as_mut() {
                    e.xy = d
                        .as_chunks::<8>()
                        .0
                        .iter()
                        .map(|c| {
                            let x = i32::from_be_bytes([c[0], c[1], c[2], c[3]]) as f64 * scale;
                            let y = i32::from_be_bytes([c[4], c[5], c[6], c[7]]) as f64 * scale;
                            (x.round() as i64, y.round() as i64)
                        })
                        .collect();
                }
            }
            STRING => {
                if let Some(e) = el.as_mut() {
                    e.text = s();
                }
            }
            ENDEL => {
                if let (Some(e), Some(c)) = (el.take(), cur.as_mut()) {
                    let Some(layer) = tech.layer_by_gds(e.layer, e.dt).or_else(|| tech.layer_by_gds(e.layer, 0)) else {
                        out.skipped_unknown_layers += 1;
                        continue;
                    };
                    let geom = match e.kind {
                        BOUNDARY => {
                            let mut pts = e.xy;
                            if pts.len() > 1 && pts.first() == pts.last() {
                                pts.pop();
                            }
                            if let Some(r) = as_rect(&pts) { Geometry::Rect(r) } else { Geometry::Polygon { points: pts } }
                        }
                        PATH => Geometry::Path { points: e.xy, width: (e.width as f64 * scale) as i64 },
                        _ => Geometry::Label {
                            text: e.text,
                            at: e.xy.first().copied().unwrap_or_default(),
                            size: (e.mag * tech.dbu_per_um as f64).max(50.0) as i64,
                        },
                    };
                    c.1.add(layer, geom);
                }
            }
            ENDLIB => break,
            _ => {}
        }
    }
    Ok(out)
}

fn as_rect(pts: &[(i64, i64)]) -> Option<IRect> {
    if pts.len() != 4 {
        return None;
    }
    let xs: std::collections::BTreeSet<i64> = pts.iter().map(|p| p.0).collect();
    let ys: std::collections::BTreeSet<i64> = pts.iter().map(|p| p.1).collect();
    if xs.len() != 2 || ys.len() != 2 {
        return None;
    }
    // Every edge must be axis-aligned.
    for k in 0..4 {
        let (a, b) = (pts[k], pts[(k + 1) % 4]);
        if a.0 != b.0 && a.1 != b.1 {
            return None;
        }
    }
    let (x0, x1) = (*xs.first()?, *xs.last()?);
    let (y0, y1) = (*ys.first()?, *ys.last()?);
    Some(IRect::new(x0, y0, x1, y1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real8_roundtrip() {
        for v in [1e-3, 1e-9, 0.5, 123.25, -7.0] {
            let r = from_real8(&real8(v));
            assert!((r - v).abs() / v.abs() < 1e-12, "{v} -> {r}");
        }
    }

    #[test]
    fn gds_roundtrip() {
        let tech = Tech::generic();
        let mut l = Layout::default();
        l.add_rect(6, 0, 0, 1000, 500);
        l.add(7, Geometry::Path { points: vec![(0, 0), (2000, 0), (2000, 3000)], width: 280 });
        l.add(6, Geometry::Label { text: "out".into(), at: (100, 100), size: 200 });
        l.add(4, Geometry::Polygon { points: vec![(0, 0), (500, 0), (500, 500), (250, 800), (0, 500)] });
        let bytes = write_gds("lib", &[("top", &l)], &tech);
        let imp = read_gds(&bytes, &tech).unwrap();
        assert_eq!(imp.cells.len(), 1);
        assert_eq!(imp.cells[0].0, "top");
        let shapes = &imp.cells[0].1.shapes;
        assert_eq!(shapes.len(), 4);
        assert_eq!(shapes[0].geom, Geometry::Rect(IRect::new(0, 0, 1000, 500)));
        assert!(matches!(&shapes[1].geom, Geometry::Path { width: 280, .. }));
        assert!(matches!(&shapes[2].geom, Geometry::Label { text, .. } if text == "out"));
        assert!(matches!(&shapes[3].geom, Geometry::Polygon { points } if points.len() == 5));
    }
}
