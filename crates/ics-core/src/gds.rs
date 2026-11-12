//! GDSII stream writer and reader: BOUNDARY, BOX, PATH and TEXT elements plus SREF / AREF
//! cell references.

use std::collections::{HashMap, HashSet};

use crate::geom::IRect;
use crate::hier::{LayoutSource, descendants};
use crate::layout::{ArraySpec, Geometry, Layout, Trans};
use crate::project::{CellRef, valid_name};
use crate::tech::{LayerDef, Tech};

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
const SREF: u16 = 0x0A00;
const AREF: u16 = 0x0B00;
const TEXT: u16 = 0x0C00;
const LAYER: u16 = 0x0D02;
const DATATYPE: u16 = 0x0E02;
const WIDTH: u16 = 0x0F03;
const XY: u16 = 0x1003;
const ENDEL: u16 = 0x1100;
const SNAME: u16 = 0x1206;
const COLROW: u16 = 0x1302;
const NODE: u16 = 0x1500;
const TEXTTYPE: u16 = 0x1602;
const STRING: u16 = 0x1906;
const STRANS: u16 = 0x1A01;
const MAG: u16 = 0x1B05;
const ANGLE: u16 = 0x1C05;
const PATHTYPE: u16 = 0x2102;
const BOX: u16 = 0x2D00;
const BOXTYPE: u16 = 0x2E02;

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

fn gds_names(cells: &[(&CellRef, &Layout)]) -> HashMap<CellRef, String> {
    let mut count: HashMap<&str, usize> = HashMap::new();
    for (r, _) in cells {
        *count.entry(r.cell.as_str()).or_default() += 1;
    }
    cells
        .iter()
        .map(|(r, _)| {
            let name = if count[r.cell.as_str()] > 1 { format!("{}__{}", r.lib, r.cell) } else { r.cell.clone() };
            ((*r).clone(), name)
        })
        .collect()
}

/// Indices of `cells` ordered so that every cell comes after the cells it places.
fn child_first_order(cells: &[(&CellRef, &Layout)]) -> Vec<usize> {
    let index: HashMap<&CellRef, usize> = cells.iter().enumerate().map(|(i, (r, _))| (*r, i)).collect();
    let mut seen = vec![false; cells.len()];
    let mut order = Vec::with_capacity(cells.len());
    for start in 0..cells.len() {
        if seen[start] {
            continue;
        }
        seen[start] = true;
        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        while let Some((cur, next)) = stack.last_mut() {
            let insts = &cells[*cur].1.instances;
            if let Some(inst) = insts.get(*next) {
                *next += 1;
                if let Some(&c) = index.get(&inst.cell)
                    && !seen[c]
                {
                    seen[c] = true;
                    stack.push((c, 0));
                }
            } else {
                order.push(*cur);
                stack.pop();
            }
        }
    }
    order
}

/// Serialize the given cells to a GDSII byte stream, children before their parents.
///
/// Structures are named after their cell, or `lib__cell` when the same cell name occurs in
/// more than one library among `cells`.
pub fn write_gds(lib_name: &str, cells: &[(&CellRef, &Layout)], tech: &Tech) -> Vec<u8> {
    let est: usize = cells.iter().map(|(_, l)| 64 + l.shapes.len() * 64 + l.instances.len() * 80).sum();
    let mut w = W(Vec::with_capacity(est + 256));
    let date: [i16; 12] = [2026, 1, 1, 0, 0, 0, 2026, 1, 1, 0, 0, 0];
    w.i16s(HEADER, &[600]);
    w.i16s(BGNLIB, &date);
    w.str(LIBNAME, lib_name);
    let user_per_dbu = 1.0 / tech.dbu_per_um as f64;
    let meters_per_dbu = 1e-6 / tech.dbu_per_um as f64;
    let mut units = real8(user_per_dbu).to_vec();
    units.extend_from_slice(&real8(meters_per_dbu));
    w.rec(UNITS, &units);
    let names = gds_names(cells);
    for k in child_first_order(cells) {
        let (r, layout) = cells[k];
        w.i16s(BGNSTR, &date);
        w.str(STRNAME, &names[r]);
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
        for inst in &layout.instances {
            let (cols, rows) = inst.dims();
            let array = inst.array.filter(|_| cols * rows > 1);
            w.rec(if array.is_some() { AREF } else { SREF }, &[]);
            w.str(SNAME, names.get(&inst.cell).map_or(inst.cell.cell.as_str(), String::as_str));
            let t = inst.trans;
            if t.mirror || t.rot % 4 != 0 {
                w.rec(STRANS, &(if t.mirror { 0x8000u16 } else { 0 }).to_be_bytes());
                if t.rot % 4 != 0 {
                    w.rec(ANGLE, &real8((t.rot % 4) as f64 * 90.0));
                }
            }
            let o = (t.x, t.y);
            match array {
                Some(a) => {
                    w.i16s(COLROW, &[cols as i16, rows as i16]);
                    let (c, r) = (cols as i64, rows as i64);
                    w.xy(&[o, (o.0 + c * a.col.0, o.1 + c * a.col.1), (o.0 + r * a.row.0, o.1 + r * a.row.1)]);
                }
                None => w.xy(&[o]),
            }
            w.rec(ENDEL, &[]);
        }
        w.rec(ENDSTR, &[]);
    }
    w.rec(ENDLIB, &[]);
    w.0
}

/// Serialize `tops` and every cell they place (directly or indirectly) that `src` resolves.
pub fn write_gds_hier(lib_name: &str, src: &dyn LayoutSource, tops: &[CellRef], tech: &Tech) -> Vec<u8> {
    let mut seen: HashSet<CellRef> = HashSet::new();
    let mut refs: Vec<CellRef> = vec![];
    for t in tops {
        for r in descendants(src, t) {
            if seen.insert(r.clone()) {
                refs.push(r);
            }
        }
    }
    let cells: Vec<(&CellRef, &Layout)> = refs.iter().filter_map(|r| Some((r, src.layout(r)?))).collect();
    write_gds(lib_name, &cells, tech)
}

#[derive(Debug, Default)]
pub struct GdsImport {
    pub cells: Vec<(String, Layout)>,
    pub skipped_unknown_layers: usize,
    /// Layers created for unknown (layer, datatype) pairs (see [`ReadOptions`]); shapes refer
    /// to the `k`-th as layer index `new_layer_base + k`.
    pub new_layers: Vec<LayerDef>,
    /// `tech.layers.len()` of the technology passed to the reader.
    pub new_layer_base: usize,
    pub dbu_meters: f64,
    pub warnings: Vec<String>,
}

impl GdsImport {
    /// Cells not placed by any other imported cell, in file order.
    pub fn top_cells(&self) -> Vec<String> {
        let mut used: HashSet<&str> = HashSet::new();
        for (name, l) in &self.cells {
            for i in &l.instances {
                if i.cell.cell != *name {
                    used.insert(&i.cell.cell);
                }
            }
        }
        self.cells.iter().filter(|(n, _)| !used.contains(n.as_str())).map(|(n, _)| n.clone()).collect()
    }

    /// Rename cells to unique valid project names (case-insensitively unique, since cells are
    /// directories on disk) and point every instance at library `lib` with the new names.
    /// References to cells missing from the file get sanitized, non-clashing names too.
    pub fn sanitize_names(&mut self, lib: &str) {
        let mut used: HashSet<String> = HashSet::with_capacity(self.cells.len());
        let mut map: HashMap<String, String> = HashMap::with_capacity(self.cells.len());
        for (name, _) in &mut self.cells {
            let new = unique_name(&sanitize_name(name), &mut used);
            map.entry(std::mem::replace(name, new.clone())).or_insert(new);
        }
        for (_, l) in &mut self.cells {
            for i in &mut l.instances {
                let new = match map.get(&i.cell.cell) {
                    Some(n) => n.clone(),
                    None => {
                        let n = unique_name(&sanitize_name(&i.cell.cell), &mut used);
                        map.insert(i.cell.cell.clone(), n.clone());
                        n
                    }
                };
                i.cell = CellRef::new(lib, new);
            }
        }
    }
}

/// `name` with every character outside `[A-Za-z0-9_-]` replaced by `_`, never starting with `-`.
pub(crate) fn sanitize_name(name: &str) -> String {
    let mut s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    if s.starts_with('-') {
        s.replace_range(0..1, "_");
    }
    if s.is_empty() { "cell".into() } else { s }
}

/// `base`, or `base_1`, `base_2`, ... whichever is not yet in `used` (compared case-insensitively).
pub(crate) fn unique_name(base: &str, used: &mut HashSet<String>) -> String {
    debug_assert!(valid_name(base));
    let mut name = base.to_string();
    let mut k = 0;
    while used.contains(&name.to_ascii_lowercase()) {
        k += 1;
        name = format!("{base}_{k}");
    }
    used.insert(name.to_ascii_lowercase());
    name
}

#[derive(Default)]
struct El {
    kind: u16,
    layer: u16,
    dt: u16,
    width: i64,
    xy: Vec<(i32, i32)>,
    text: String,
    sname: String,
    mag: f64,
    angle: f64,
    mirror: bool,
    colrow: (u16, u16),
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ReadOptions {
    /// Turn every (layer, datatype) pair `tech` does not define into a new layer instead of
    /// skipping its shapes. Datatypes are then matched exactly (no fallback to datatype 0).
    pub add_unknown_layers: bool,
}

/// [`read_gds_with`] with default options: shapes on unknown layers are skipped.
pub fn read_gds(data: &[u8], tech: &Tech, lib: &str) -> Result<GdsImport, String> {
    read_gds_with(data, tech, lib, ReadOptions::default())
}

/// Parse a GDSII stream. Shapes are mapped to `tech` layers by (layer, datatype), falling back
/// to datatype 0; unmatched ones are skipped or, with [`ReadOptions::add_unknown_layers`], put
/// on new layers. Instances refer to cells of library `lib` by their structure name.
pub fn read_gds_with(data: &[u8], tech: &Tech, lib: &str, opts: ReadOptions) -> Result<GdsImport, String> {
    let mut out = GdsImport { new_layer_base: tech.layers.len(), ..GdsImport::default() };
    let mut new_index: HashMap<(u16, u16), usize> = HashMap::new();
    let mut i = 0usize;
    let mut cur: Option<(String, Layout)> = None;
    let mut el = El::default();
    let mut in_el = false;
    let mut scale = 1.0f64;
    let (mut odd_angles, mut mags) = (0usize, 0usize);
    let sc = |v: f64, scale: f64| (v * scale).round() as i64;
    while i + 4 <= data.len() {
        let len = u16::from_be_bytes([data[i], data[i + 1]]) as usize;
        let tag = u16::from_be_bytes([data[i + 2], data[i + 3]]);
        if len < 4 || i + len > data.len() {
            return Err(format!("corrupt record at byte {i}"));
        }
        let d = &data[i + 4..i + len];
        i += len;
        let i16_at = |k: usize| i16::from_be_bytes([d[2 * k], d[2 * k + 1]]);
        let set_str = |s: &mut String| {
            s.clear();
            s.push_str(String::from_utf8_lossy(d).trim_end_matches('\0'));
        };
        match tag {
            UNITS if d.len() >= 16 => {
                out.dbu_meters = from_real8(&d[8..16]);
                let tech_dbu_m = 1e-6 / tech.dbu_per_um as f64;
                scale = out.dbu_meters / tech_dbu_m;
            }
            ENDLIB => break,
            BGNSTR => cur = Some((String::new(), Layout::default())),
            STRNAME => {
                if let Some(c) = cur.as_mut() {
                    set_str(&mut c.0);
                }
            }
            ENDSTR => {
                if let Some(c) = cur.take() {
                    out.cells.push(c);
                }
            }
            BOUNDARY | PATH | TEXT | BOX | SREF | AREF | NODE => {
                el.kind = tag;
                el.layer = 0;
                el.dt = 0;
                el.width = 0;
                el.xy.clear();
                el.mag = 1.0;
                el.angle = 0.0;
                el.mirror = false;
                el.colrow = (1, 1);
                in_el = true;
            }
            _ if !in_el => {}
            LAYER if d.len() >= 2 => el.layer = i16_at(0) as u16,
            DATATYPE | TEXTTYPE | BOXTYPE if d.len() >= 2 => el.dt = i16_at(0) as u16,
            WIDTH if d.len() >= 4 => el.width = i32::from_be_bytes([d[0], d[1], d[2], d[3]]).unsigned_abs() as i64,
            STRANS if d.len() >= 2 => el.mirror = d[0] & 0x80 != 0,
            MAG if d.len() >= 8 => el.mag = from_real8(d),
            ANGLE if d.len() >= 8 => el.angle = from_real8(d),
            COLROW if d.len() >= 4 => el.colrow = (i16_at(0).max(1) as u16, i16_at(1).max(1) as u16),
            XY => {
                el.xy.extend(
                    d.as_chunks::<8>()
                        .0
                        .iter()
                        .map(|c| (i32::from_be_bytes([c[0], c[1], c[2], c[3]]), i32::from_be_bytes([c[4], c[5], c[6], c[7]]))),
                );
            }
            STRING => set_str(&mut el.text),
            SNAME => set_str(&mut el.sname),
            ENDEL => {
                in_el = false;
                let Some(c) = cur.as_mut() else { continue };
                let pts = || el.xy.iter().map(|&(x, y)| (sc(x as f64, scale), sc(y as f64, scale)));
                if matches!(el.kind, SREF | AREF) {
                    let Some(&(x0, y0)) = el.xy.first() else { continue };
                    let q = el.angle / 90.0;
                    if (q - q.round()).abs() > 1e-6 {
                        odd_angles += 1;
                    }
                    if (el.mag - 1.0).abs() > 1e-9 {
                        mags += 1;
                    }
                    let rot = (q.round() as i64).rem_euclid(4) as u8;
                    let trans = Trans::with(sc(x0 as f64, scale), sc(y0 as f64, scale), rot, el.mirror);
                    let (cols, rows) = el.colrow;
                    let array = match (el.kind, el.xy.get(1), el.xy.get(2)) {
                        (AREF, Some(&p2), Some(&p3)) if cols as u32 * rows as u32 > 1 => {
                            let step = |p: (i32, i32), n: u16| {
                                let f = |a: i32, b: i32| sc((a as f64 - b as f64) / n as f64, scale);
                                (f(p.0, x0), f(p.1, y0))
                            };
                            Some(ArraySpec { cols: cols as u32, rows: rows as u32, col: step(p2, cols), row: step(p3, rows) })
                        }
                        _ => None,
                    };
                    c.1.add_instance(CellRef::new(lib, el.sname.as_str()), trans, array);
                    continue;
                }
                if el.kind == NODE {
                    continue;
                }
                let layer = if opts.add_unknown_layers {
                    tech.layer_by_gds(el.layer, el.dt).unwrap_or_else(|| {
                        *new_index.entry((el.layer, el.dt)).or_insert_with(|| {
                            let k = out.new_layers.len();
                            out.new_layers.push(LayerDef::generated(el.layer, el.dt, tech.layers.len() + k));
                            tech.layers.len() + k
                        })
                    })
                } else {
                    let Some(l) = tech.layer_by_gds(el.layer, el.dt).or_else(|| tech.layer_by_gds(el.layer, 0)) else {
                        out.skipped_unknown_layers += 1;
                        continue;
                    };
                    l
                };
                let geom = match el.kind {
                    BOUNDARY | BOX => {
                        let mut p: Vec<(i64, i64)> = pts().collect();
                        if p.len() > 1 && p.first() == p.last() {
                            p.pop();
                        }
                        if let Some(r) = as_rect(&p) { Geometry::Rect(r) } else { Geometry::Polygon { points: p } }
                    }
                    PATH => Geometry::Path { points: pts().collect(), width: (el.width as f64 * scale) as i64 },
                    _ => Geometry::Label {
                        text: std::mem::take(&mut el.text),
                        at: pts().next().unwrap_or_default(),
                        size: (el.mag * tech.dbu_per_um as f64).max(50.0) as i64,
                    },
                };
                c.1.add(layer, geom);
            }
            _ => {}
        }
    }
    if odd_angles > 0 {
        out.warnings.push(format!("{odd_angles} instance(s) with a rotation that is not a multiple of 90 degrees were rounded"));
    }
    if mags > 0 {
        out.warnings.push(format!("{mags} instance(s) with magnification other than 1 were placed unscaled"));
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
    use crate::hier::flatten;
    use crate::tech::FillStyle;

    #[test]
    fn real8_roundtrip() {
        for v in [1e-3, 1e-9, 0.5, 123.25, -7.0, 90.0, 270.0] {
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
        let top = CellRef::new("lib", "top");
        let bytes = write_gds("lib", &[(&top, &l)], &tech);
        let imp = read_gds(&bytes, &tech, "lib").unwrap();
        assert_eq!(imp.cells.len(), 1);
        assert_eq!(imp.cells[0].0, "top");
        let shapes = &imp.cells[0].1.shapes;
        assert_eq!(shapes.len(), 4);
        assert_eq!(shapes[0].geom, Geometry::Rect(IRect::new(0, 0, 1000, 500)));
        assert!(matches!(&shapes[1].geom, Geometry::Path { width: 280, .. }));
        assert!(matches!(&shapes[2].geom, Geometry::Label { text, .. } if text == "out"));
        assert!(matches!(&shapes[3].geom, Geometry::Polygon { points } if points.len() == 5));
        assert!(imp.warnings.is_empty());
    }

    #[test]
    fn unknown_layers_become_new_layers() {
        let generic = Tech::generic();
        let mut tech = generic.clone();
        tech.layers.push(LayerDef { name: "pad".into(), gds_layer: 99, ..tech.layers[0].clone() });
        tech.layers.push(LayerDef { name: "m1pin".into(), gds_layer: 7, gds_datatype: 16, ..tech.layers[0].clone() });
        let (pad, pin) = (generic.layers.len(), generic.layers.len() + 1);
        let mut l = Layout::default();
        l.add_rect(6, 0, 0, 100, 100);
        l.add_rect(pad, 0, 0, 5000, 5000);
        l.add_rect(pin, 10, 10, 20, 20);
        l.add(pad, Geometry::Label { text: "PAD".into(), at: (0, 0), size: 200 });
        let top = CellRef::new("lib", "top");
        let bytes = write_gds("lib", &[(&top, &l)], &tech);

        let skip = read_gds(&bytes, &generic, "lib").unwrap();
        assert_eq!(skip.skipped_unknown_layers, 2);
        assert!(skip.new_layers.is_empty());
        let layers: Vec<usize> = skip.cells[0].1.shapes.iter().map(|s| s.layer).collect();
        assert_eq!(layers, [6, 6], "without the option 7/16 falls back to 7/0");

        let imp = read_gds_with(&bytes, &generic, "lib", ReadOptions { add_unknown_layers: true }).unwrap();
        assert_eq!(imp.skipped_unknown_layers, 0);
        let new: Vec<(&str, u16, u16)> = imp.new_layers.iter().map(|d| (d.name.as_str(), d.gds_layer, d.gds_datatype)).collect();
        assert_eq!(new, [("L99D0", 99, 0), ("L7D16", 7, 16)]);
        assert_ne!(imp.new_layers[0].color, imp.new_layers[1].color);
        assert!(imp.new_layers.iter().all(|d| d.fill == FillStyle::Hatch));
        let shapes = &imp.cells[0].1.shapes;
        let layers: Vec<usize> = shapes.iter().map(|s| s.layer).collect();
        assert_eq!(layers, [6, pad, pin, pad]);
        assert_eq!(shapes[1].geom, Geometry::Rect(IRect::new(0, 0, 5000, 5000)));

        let mut merged = generic.clone();
        merged.layers.extend(imp.new_layers.iter().cloned());
        let again = write_gds("lib", &[(&top, &imp.cells[0].1)], &merged);
        assert_eq!(again, bytes, "round trip through the new layers is lossless");
    }

    fn hier_lib() -> (HashMap<CellRef, Layout>, CellRef) {
        let (leaf, mid, top) = (CellRef::new("a", "leaf"), CellRef::new("a", "mid"), CellRef::new("a", "top"));
        let mut m = HashMap::new();
        let mut l = Layout::default();
        l.add_rect(6, 0, 0, 300, 100);
        l.add(4, Geometry::Polygon { points: vec![(0, 0), (500, 0), (500, 500), (250, 800), (0, 500)] });
        l.add(7, Geometry::Path { points: vec![(0, 0), (2000, 0), (2000, 3000)], width: 280 });
        m.insert(leaf.clone(), l);
        let mut md = Layout::default();
        md.add_rect(7, -100, -100, 0, 0);
        md.add_instance(leaf.clone(), Trans::with(1000, 2000, 1, true), None);
        md.add_instance(leaf.clone(), Trans::with(-500, 0, 3, false), None);
        md.add_instance(leaf.clone(), Trans::with(0, 0, 2, true), Some(ArraySpec { cols: 1, rows: 1, col: (0, 0), row: (0, 0) }));
        m.insert(mid.clone(), md);
        let mut t = Layout::default();
        t.add_instance(
            mid.clone(),
            Trans::with(0, 0, 0, true),
            Some(ArraySpec { cols: 3, rows: 2, col: (5000, 0), row: (0, 7000) }),
        );
        t.add_instance(
            mid.clone(),
            Trans::with(100, 100, 1, false),
            Some(ArraySpec { cols: 4, rows: 3, col: (300, 400), row: (-800, 600) }),
        );
        t.add_instance(CellRef::new("a", "missing"), Trans::new(5, 5), None);
        m.insert(top.clone(), t);
        (m, top)
    }

    fn flat(top: &Layout, src: &dyn LayoutSource) -> Vec<(usize, Geometry)> {
        flatten(top, src, 100).into_iter().map(|s| (s.layer, s.geom)).collect()
    }

    #[test]
    fn hierarchy_roundtrip() {
        let tech = Tech::generic();
        let (m, top) = hier_lib();
        let bytes = write_gds_hier("a", &m, std::slice::from_ref(&top), &tech);
        let imp = read_gds(&bytes, &tech, "a").unwrap();
        let names: Vec<&str> = imp.cells.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["leaf", "mid", "top"], "children are written first");
        assert!(imp.warnings.is_empty());
        assert_eq!(imp.top_cells(), ["top"]);
        let back: HashMap<CellRef, Layout> = imp.cells.into_iter().map(|(n, l)| (CellRef::new("a", n), l)).collect();
        let t2 = &back[&top];
        assert_eq!(t2.instances.len(), 3);
        assert_eq!(t2.instances[1].array, m[&top].instances[1].array);
        assert_eq!(back[&CellRef::new("a", "mid")].instances[2].array, None);
        let before = flat(&m[&top], &m);
        assert_eq!(before.len(), 18 * 10);
        assert_eq!(before, flat(t2, &back));
    }

    #[test]
    fn name_collisions_get_library_prefix() {
        let tech = Tech::generic();
        let (ia, ib, top) = (CellRef::new("a", "inv"), CellRef::new("b", "inv"), CellRef::new("a", "top"));
        let mut leaf = Layout::default();
        leaf.add_rect(6, 0, 0, 10, 10);
        let mut t = Layout::default();
        t.add_instance(ia.clone(), Trans::IDENTITY, None);
        t.add_instance(ib.clone(), Trans::new(100, 0), None);
        t.add_instance(CellRef::new("c", "ext"), Trans::IDENTITY, None);
        let bytes = write_gds("x", &[(&top, &t), (&ia, &leaf), (&ib, &leaf)], &tech);
        let imp = read_gds(&bytes, &tech, "x").unwrap();
        let names: Vec<&str> = imp.cells.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["a__inv", "b__inv", "top"]);
        let refs: Vec<&str> = imp.cells[2].1.instances.iter().map(|i| i.cell.cell.as_str()).collect();
        assert_eq!(refs, ["a__inv", "b__inv", "ext"]);
        assert_eq!(imp.top_cells(), ["top"]);
    }

    #[test]
    fn cyclic_references_do_not_hang() {
        let tech = Tech::generic();
        let (a, b) = (CellRef::new("l", "a"), CellRef::new("l", "b"));
        let mut la = Layout::default();
        la.add_instance(b.clone(), Trans::IDENTITY, None);
        let mut lb = Layout::default();
        lb.add_instance(a.clone(), Trans::IDENTITY, None);
        let imp = read_gds(&write_gds("l", &[(&a, &la), (&b, &lb)], &tech), &tech, "l").unwrap();
        assert_eq!(imp.cells.len(), 2);
        assert!(imp.top_cells().is_empty());
    }

    #[test]
    fn sanitize_names_remaps_references() {
        let mut imp = GdsImport::default();
        let mut parent = Layout::default();
        for n in ["my cell", "MY_CELL", "-x", "gone.1"] {
            parent.add_instance(CellRef::new("tmp", n), Trans::IDENTITY, None);
        }
        for n in ["my cell", "MY_CELL", "-x", ""] {
            imp.cells.push((n.into(), Layout::default()));
        }
        imp.cells.push(("top".into(), parent));
        imp.sanitize_names("lib");
        let names: Vec<&str> = imp.cells.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["my_cell", "MY_CELL_1", "_x", "cell", "top"]);
        assert!(names.iter().all(|n| valid_name(n)));
        let refs: Vec<String> = imp.cells[4].1.instances.iter().map(|i| i.cell.to_string()).collect();
        assert_eq!(refs, ["lib/my_cell", "lib/MY_CELL_1", "lib/_x", "lib/gone_1"]);
        assert_eq!(imp.top_cells(), ["cell", "top"]);
    }

    #[test]
    fn odd_angles_magnification_box_and_node() {
        let tech = Tech::generic();
        let mut w = W(Vec::new());
        w.i16s(HEADER, &[600]);
        w.i16s(BGNLIB, &[0; 12]);
        w.str(LIBNAME, "t");
        let mut units = real8(1e-3).to_vec();
        units.extend_from_slice(&real8(1e-9));
        w.rec(UNITS, &units);
        w.i16s(BGNSTR, &[0; 12]);
        w.str(STRNAME, "c");
        for angle in [45.0, 100.0, 180.0] {
            w.rec(SREF, &[]);
            w.str(SNAME, "leaf");
            w.rec(STRANS, &[0, 0]);
            w.rec(MAG, &real8(2.0));
            w.rec(ANGLE, &real8(angle));
            w.xy(&[(10, 20)]);
            w.rec(ENDEL, &[]);
        }
        w.rec(TEXT, &[]);
        w.i16s(LAYER, &[tech.layer(6).unwrap().gds_layer as i16]);
        w.i16s(TEXTTYPE, &[0]);
        w.rec(STRANS, &[0x80, 0]);
        w.rec(MAG, &real8(0.5));
        w.rec(ANGLE, &real8(30.0));
        w.xy(&[(0, 0)]);
        w.str(STRING, "lbl");
        w.rec(ENDEL, &[]);
        w.rec(NODE, &[]);
        w.i16s(LAYER, &[1]);
        w.i16s(0x2A02, &[0]);
        w.xy(&[(0, 0), (5, 5)]);
        w.rec(ENDEL, &[]);
        w.rec(BOX, &[]);
        w.i16s(LAYER, &[tech.layer(6).unwrap().gds_layer as i16]);
        w.i16s(BOXTYPE, &[0]);
        w.xy(&[(0, 0), (40, 0), (40, 30), (0, 30), (0, 0)]);
        w.rec(ENDEL, &[]);
        w.rec(ENDSTR, &[]);
        w.rec(ENDLIB, &[]);
        let imp = read_gds(&w.0, &tech, "t").unwrap();
        let l = &imp.cells[0].1;
        let rots: Vec<u8> = l.instances.iter().map(|i| i.trans.rot).collect();
        assert_eq!(rots, [1, 1, 2]);
        assert!(l.instances.iter().all(|i| !i.trans.mirror && (i.trans.x, i.trans.y) == (10, 20)));
        assert_eq!(imp.warnings.len(), 2, "{:?}", imp.warnings);
        assert!(imp.warnings[0].starts_with("2 instance"));
        assert!(imp.warnings[1].starts_with("3 instance"));
        assert_eq!(l.shapes.len(), 2);
        assert!(matches!(&l.shapes[0].geom, Geometry::Label { text, size: 500, .. } if text == "lbl"));
        assert_eq!(l.shapes[1].geom, Geometry::Rect(IRect::new(0, 0, 40, 30)));
    }
}
