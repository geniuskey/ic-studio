//! GDSII stream writer and reader: BOUNDARY, BOX, PATH and TEXT elements plus SREF / AREF
//! cell references. [`StreamFormat`] dispatches between GDSII and [OASIS](crate::oasis).

use std::collections::{HashMap, HashSet};

use crate::geom::IRect;
use crate::hier::{LayoutSource, descendants};
use crate::layout::{ArraySpec, Geometry, Layout, Trans};
use crate::project::{CellRef, valid_name};
use crate::region::BoolOp;
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
const BGNEXTN: u16 = 0x3003;
const ENDEXTN: u16 = 0x3103;
const BOX: u16 = 0x2D00;
const BOXTYPE: u16 = 0x2E02;
const PROPATTR: u16 = 0x2B02;
const PROPVALUE: u16 = 0x2C06;

/// Property attribute under which SREF / AREF elements carry the instance name
/// ([`CellInst::name`](crate::layout::CellInst::name)), written as PROPATTR / PROPVALUE.
/// KLayout shows it as user property 61. Other properties are ignored on read.
pub const INST_NAME_ATTR: i16 = 61;
/// Property attribute holding the database id of a shape or instance in GDS-stored libraries
/// ([`write_gds_with_ids`]): constraints refer to items by id.
pub const ITEM_ID_ATTR: i16 = 62;

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

/// Most points one XY record holds: its length field counts at most 65535 bytes.
const MAX_XY: usize = 8191;

/// `points` as BOUNDARY outlines of at most `MAX_XY - 1` vertices (the record repeats the
/// first point): larger polygons are cut into strips, alternating vertical and horizontal.
fn boundary_pieces(points: &[(i64, i64)], depth: u32) -> Vec<Vec<(i64, i64)>> {
    use crate::polybool::PolySet;
    if points.len() < MAX_XY - 1 {
        return vec![points.to_vec()];
    }
    let set = PolySet::from_polygon(points);
    let Some(b) = set.bbox() else { return vec![] };
    let strips = (points.len() / (MAX_XY / 2) + 2) as i64;
    let vertical = depth.is_multiple_of(2);
    let (lo, hi) = if vertical { (b.x0, b.x1) } else { (b.y0, b.y1) };
    let step = ((hi - lo + strips - 1) / strips).max(1);
    let mut out = vec![];
    let mut at = lo;
    while at < hi {
        let next = (at + step).min(hi);
        let cut = if vertical { IRect::new(at, b.y0, next, b.y1) } else { IRect::new(b.x0, at, b.x1, next) };
        for piece in set.boolean(&PolySet::from_rect(cut), BoolOp::And).to_polygons() {
            if piece.len() >= MAX_XY - 1 && depth < 16 {
                out.extend(boundary_pieces(&piece, depth + 1));
            } else {
                out.push(piece);
            }
        }
        at = next;
    }
    out
}

/// After restoring stored ids: give repeated ids (the pieces of a split polygon, a split
/// array, or ids clashing with freshly allocated ones) new ids, and move `next_id` past them all.
fn unique_ids(l: &mut Layout) {
    let max = l.shapes.iter().map(|s| s.id).chain(l.instances.iter().map(|i| i.id)).max().unwrap_or(0);
    let mut next = max.max(l.next_id);
    let mut seen = HashSet::new();
    for id in l.shapes.iter_mut().map(|s| &mut s.id).chain(l.instances.iter_mut().map(|i| &mut i.id)) {
        if !seen.insert(*id) {
            next += 1;
            *id = next;
        }
    }
    l.next_id = next;
}

/// Whether moving path end `k` (neighbour `prev`) outwards by `by` keeps its end segment
/// pointing the same way.
fn path_end_moves(v: &[(i64, i64)], k: usize, prev: usize, by: i64) -> bool {
    let len = ((v[k].0 - v[prev].0) as f64).hypot((v[k].1 - v[prev].1) as f64);
    len > 0.0 && (by >= 0 || (-by as f64) < len)
}

/// Move path end `k` (neighbour `prev`) outwards along its end segment by `by` (inwards when negative).
fn move_path_end(v: &mut [(i64, i64)], k: usize, prev: usize, by: i64) {
    let (dx, dy) = ((v[k].0 - v[prev].0) as f64, (v[k].1 - v[prev].1) as f64);
    let len = dx.hypot(dy);
    if by != 0 && len > 0.0 {
        v[k].0 += (dx / len * by as f64).round() as i64;
        v[k].1 += (dy / len * by as f64).round() as i64;
    }
}

impl W {
    /// The item id property of [`write_gds_with_ids`].
    fn id(&mut self, on: bool, id: u64) {
        if on {
            self.i16s(PROPATTR, &[ITEM_ID_ATTR]);
            self.str(PROPVALUE, &id.to_string());
        }
    }

    fn rec(&mut self, tag: u16, data: &[u8]) {
        debug_assert!(data.len() + 4 <= u16::MAX as usize, "GDS record of {} bytes", data.len() + 4);
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

/// Stream file (GDSII or OASIS) cell names of a set of written cells.
///
/// Structures are named after their cell, or `lib__cell` when the same cell name occurs in
/// more than one library among the cells. References to cells not among them use the plain
/// cell name, or `lib__cell` when a structure already has that name.
pub(crate) struct StreamNames {
    names: HashMap<CellRef, String>,
    structures: HashSet<String>,
}

impl StreamNames {
    pub(crate) fn new(cells: &[(&CellRef, &Layout)]) -> Self {
        let mut count: HashMap<&str, usize> = HashMap::new();
        for (r, _) in cells {
            *count.entry(r.cell.as_str()).or_default() += 1;
        }
        let names: HashMap<CellRef, String> = cells
            .iter()
            .map(|(r, _)| {
                let name = if count[r.cell.as_str()] > 1 { format!("{}__{}", r.lib, r.cell) } else { r.cell.clone() };
                ((*r).clone(), name)
            })
            .collect();
        let structures = names.values().cloned().collect();
        Self { names, structures }
    }

    pub(crate) fn structure(&self, r: &CellRef) -> &str {
        &self.names[r]
    }

    pub(crate) fn reference(&self, r: &CellRef) -> String {
        match self.names.get(r) {
            Some(n) => n.clone(),
            None if self.structures.contains(&r.cell) => format!("{}__{}", r.lib, r.cell),
            None => r.cell.clone(),
        }
    }
}

/// Indices of `cells` ordered so that every cell comes after the cells it places.
pub(crate) fn child_first_order(cells: &[(&CellRef, &Layout)]) -> Vec<usize> {
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

/// Serialize the given cells to a GDSII byte stream, children before their parents, with
/// structure names as described at [`StreamNames`].
pub fn write_gds(lib_name: &str, cells: &[(&CellRef, &Layout)], tech: &Tech) -> Vec<u8> {
    write_gds_opts(lib_name, cells, tech, false)
}

/// [`write_gds`] with each element's database id as a property, so that reading the file back
/// keeps the ids constraints refer to. Used for libraries stored as GDS, not for exports.
pub fn write_gds_with_ids(lib_name: &str, cells: &[(&CellRef, &Layout)], tech: &Tech) -> Vec<u8> {
    write_gds_opts(lib_name, cells, tech, true)
}

fn write_gds_opts(lib_name: &str, cells: &[(&CellRef, &Layout)], tech: &Tech, ids: bool) -> Vec<u8> {
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
    let names = StreamNames::new(cells);
    for k in child_first_order(cells) {
        let (r, layout) = cells[k];
        w.i16s(BGNSTR, &date);
        w.str(STRNAME, names.structure(r));
        for s in &layout.shapes {
            let Some(l) = tech.layer(s.layer) else { continue };
            let (layer, dt) = (l.gds_layer as i16, l.gds_datatype as i16);
            match &s.geom {
                Geometry::Rect(r) => {
                    w.rec(BOUNDARY, &[]);
                    w.i16s(LAYER, &[layer]);
                    w.i16s(DATATYPE, &[dt]);
                    w.xy(&[(r.x0, r.y0), (r.x1, r.y0), (r.x1, r.y1), (r.x0, r.y1), (r.x0, r.y0)]);
                    w.id(ids, s.id);
                    w.rec(ENDEL, &[]);
                }
                Geometry::Polygon { points } if points.len() >= 3 => {
                    for piece in boundary_pieces(points, 0) {
                        w.rec(BOUNDARY, &[]);
                        w.i16s(LAYER, &[layer]);
                        w.i16s(DATATYPE, &[dt]);
                        let mut p = piece.clone();
                        p.push(piece[0]);
                        w.xy(&p);
                        w.id(ids, s.id);
                        w.rec(ENDEL, &[]);
                    }
                }
                Geometry::Path { points, width } if points.len() >= 2 => {
                    // Consecutive pieces share their joining vertex; the half-width end
                    // extensions there cover the bend as the whole path does.
                    let mut start = 0;
                    while start + 1 < points.len() {
                        let end = (start + MAX_XY - 1).min(points.len() - 1);
                        w.rec(PATH, &[]);
                        w.i16s(LAYER, &[layer]);
                        w.i16s(DATATYPE, &[dt]);
                        w.i16s(PATHTYPE, &[2]);
                        w.i32s(WIDTH, &[*width as i32]);
                        w.xy(&points[start..=end]);
                        w.id(ids, s.id);
                        w.rec(ENDEL, &[]);
                        start = end;
                    }
                }
                Geometry::Label { text, at, size } => {
                    let (layer, dt) = l.text_stream.map_or((layer, dt), |(g, d)| (g as i16, d as i16));
                    w.rec(TEXT, &[]);
                    w.i16s(LAYER, &[layer]);
                    w.i16s(TEXTTYPE, &[dt]);
                    w.rec(MAG, &real8(*size as f64 / tech.dbu_per_um as f64));
                    w.xy(&[*at]);
                    w.str(STRING, text);
                    w.id(ids, s.id);
                    w.rec(ENDEL, &[]);
                }
                _ => {}
            }
        }
        for inst in &layout.instances {
            let (cols, rows) = inst.dims();
            let array = inst.array.filter(|_| cols * rows > 1);
            let t = inst.trans;
            // COLROW holds 16-bit counts: a larger array goes out as several AREFs.
            const MAX_COLROW: u32 = i16::MAX as u32;
            let chunks: Vec<(u32, u32, u32, u32)> = match array {
                Some(_) => (0..cols)
                    .step_by(MAX_COLROW as usize)
                    .flat_map(|c0| {
                        (0..rows)
                            .step_by(MAX_COLROW as usize)
                            .map(move |r0| (c0, r0, (cols - c0).min(MAX_COLROW), (rows - r0).min(MAX_COLROW)))
                    })
                    .collect(),
                None => vec![(0, 0, 1, 1)],
            };
            for (c0, r0, nc, nr) in chunks {
                let o = match array {
                    Some(a) => (t.x + c0 as i64 * a.col.0 + r0 as i64 * a.row.0, t.y + c0 as i64 * a.col.1 + r0 as i64 * a.row.1),
                    None => (t.x, t.y),
                };
                let arr = array.filter(|_| nc * nr > 1);
                w.rec(if arr.is_some() { AREF } else { SREF }, &[]);
                w.str(SNAME, &names.reference(&inst.cell));
                if t.mirror || t.rot % 4 != 0 {
                    w.rec(STRANS, &(if t.mirror { 0x8000u16 } else { 0 }).to_be_bytes());
                    if t.rot % 4 != 0 {
                        w.rec(ANGLE, &real8((t.rot % 4) as f64 * 90.0));
                    }
                }
                match arr {
                    Some(a) => {
                        w.i16s(COLROW, &[nc as i16, nr as i16]);
                        let (c, r) = (nc as i64, nr as i64);
                        w.xy(&[o, (o.0 + c * a.col.0, o.1 + c * a.col.1), (o.0 + r * a.row.0, o.1 + r * a.row.1)]);
                    }
                    None => w.xy(&[o]),
                }
                if let Some(name) = &inst.name {
                    w.i16s(PROPATTR, &[INST_NAME_ATTR]);
                    w.str(PROPVALUE, name);
                }
                w.id(ids, inst.id);
                w.rec(ENDEL, &[]);
            }
        }
        w.rec(ENDSTR, &[]);
    }
    w.rec(ENDLIB, &[]);
    w.0
}

/// Serialize `tops` and every cell they place (directly or indirectly) that `src` resolves.
pub fn write_gds_hier(lib_name: &str, src: &dyn LayoutSource, tops: &[CellRef], tech: &Tech) -> Vec<u8> {
    let refs = hier_refs(src, tops);
    let cells: Vec<(&CellRef, &Layout)> = refs.iter().filter_map(|r| Some((r, src.layout(r)?))).collect();
    write_gds(lib_name, &cells, tech)
}

/// `tops` and every cell they place, directly or indirectly, each once.
pub(crate) fn hier_refs(src: &dyn LayoutSource, tops: &[CellRef]) -> Vec<CellRef> {
    let mut seen: HashSet<CellRef> = HashSet::new();
    let mut refs: Vec<CellRef> = vec![];
    for t in tops {
        for r in descendants(src, t) {
            if seen.insert(r.clone()) {
                refs.push(r);
            }
        }
    }
    refs
}

/// Layout stream file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamFormat {
    Gds,
    Oasis,
}

impl StreamFormat {
    /// OASIS for `.oas` / `.oasis` files, GDSII otherwise.
    pub fn from_path(path: &std::path::Path) -> Self {
        match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
            Some("oas" | "oasis") => Self::Oasis,
            _ => Self::Gds,
        }
    }

    /// The format of a stream file's contents (OASIS files start with a magic string).
    pub fn detect(data: &[u8]) -> Self {
        if data.starts_with(crate::oasis::MAGIC) { Self::Oasis } else { Self::Gds }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Gds => "GDSII",
            Self::Oasis => "OASIS",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Gds => "gds",
            Self::Oasis => "oas",
        }
    }
}

/// [`write_gds_hier`] or [`crate::oasis::write_oasis_hier`].
pub fn write_stream_hier(format: StreamFormat, lib_name: &str, src: &dyn LayoutSource, tops: &[CellRef], tech: &Tech) -> Vec<u8> {
    match format {
        StreamFormat::Gds => write_gds_hier(lib_name, src, tops, tech),
        StreamFormat::Oasis => crate::oasis::write_oasis_hier(src, tops, tech),
    }
}

/// Read a GDSII or OASIS file, whichever `data` is.
pub fn read_stream_with(data: &[u8], tech: &Tech, lib: &str, opts: ReadOptions) -> Result<GdsImport, String> {
    match StreamFormat::detect(data) {
        StreamFormat::Gds => read_gds_with(data, tech, lib, opts),
        StreamFormat::Oasis => crate::oasis::read_oasis_with(data, tech, lib, opts),
    }
}

#[derive(Debug, Default)]
pub struct GdsImport {
    pub cells: Vec<(String, Layout)>,
    pub skipped_unknown_layers: usize,
    /// Shapes skipped, per (layer, datatype).
    pub skipped_pairs: std::collections::BTreeMap<(u16, u16), usize>,
    /// Shapes of a (layer, datatype) the technology lacks that were read onto the layer's
    /// datatype 0 (a pin or label datatype then lands on the drawing layer), per pair.
    pub datatype_fallbacks: std::collections::BTreeMap<(u16, u16), usize>,
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
    propattr: i16,
    inst_name: Option<String>,
    /// Path end style: 0 flush, 1 round, 2 half-width extension, 4 BGNEXTN / ENDEXTN.
    pathtype: i16,
    ext: (i64, i64),
    item_id: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ReadOptions {
    /// Turn every (layer, datatype) pair `tech` does not define into a new layer instead of
    /// skipping its shapes. Datatypes are then matched exactly (no fallback to datatype 0).
    pub add_unknown_layers: bool,
    /// Apply the technology's stream layer map ([`Tech::stream_map`]) before its own layer pairs.
    pub use_stream_map: bool,
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
    let mut layers = LayerMap::new(tech, opts);
    let mut i = 0usize;
    let mut cur: Option<(String, Layout)> = None;
    let mut el = El::default();
    let mut in_el = false;
    let mut scale = 1.0f64;
    let (mut odd_angles, mut mags, mut round_paths, mut short_paths) = (0usize, 0usize, 0usize, 0usize);
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
                if let Some(mut c) = cur.take() {
                    unique_ids(&mut c.1);
                    out.cells.push(c);
                }
            }
            BOUNDARY | PATH | TEXT | BOX | SREF | AREF | NODE => {
                el.kind = tag;
                el.layer = 0;
                el.dt = 0;
                el.width = 0;
                el.pathtype = 0;
                el.ext = (0, 0);
                el.item_id = None;
                el.xy.clear();
                el.mag = 1.0;
                el.angle = 0.0;
                el.mirror = false;
                el.colrow = (1, 1);
                el.propattr = 0;
                el.inst_name = None;
                in_el = true;
            }
            _ if !in_el => {}
            LAYER if d.len() >= 2 => el.layer = i16_at(0) as u16,
            DATATYPE | TEXTTYPE | BOXTYPE if d.len() >= 2 => el.dt = i16_at(0) as u16,
            WIDTH if d.len() >= 4 => el.width = i32::from_be_bytes([d[0], d[1], d[2], d[3]]).unsigned_abs() as i64,
            PATHTYPE if d.len() >= 2 => el.pathtype = i16_at(0),
            BGNEXTN if d.len() >= 4 => el.ext.0 = i32::from_be_bytes([d[0], d[1], d[2], d[3]]) as i64,
            ENDEXTN if d.len() >= 4 => el.ext.1 = i32::from_be_bytes([d[0], d[1], d[2], d[3]]) as i64,
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
            PROPATTR if d.len() >= 2 => el.propattr = i16_at(0),
            PROPVALUE if el.propattr == ITEM_ID_ATTR => {
                el.item_id = String::from_utf8_lossy(d).trim_end_matches('\0').trim().parse().ok();
            }
            PROPVALUE if el.propattr == INST_NAME_ATTR && matches!(el.kind, SREF | AREF) => {
                el.inst_name = Some(String::from_utf8_lossy(d).trim_end_matches('\0').to_string());
            }
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
                    if let Some(i) = c.1.instances.last_mut() {
                        i.name = el.inst_name.take();
                        if let Some(id) = el.item_id {
                            i.id = id;
                        }
                    }
                    continue;
                }
                if el.kind == NODE {
                    continue;
                }
                let layer = if el.kind == TEXT {
                    layers.get_text(el.layer, el.dt, &mut out)
                } else {
                    layers.get(el.layer, el.dt, &mut out)
                };
                let Some(layer) = layer else { continue };
                let geom = match el.kind {
                    BOUNDARY | BOX => {
                        let mut p: Vec<(i64, i64)> = pts().collect();
                        if p.len() > 1 && p.first() == p.last() {
                            p.pop();
                        }
                        if let Some(r) = as_rect(&p) { Geometry::Rect(r) } else { Geometry::Polygon { points: p } }
                    }
                    PATH => {
                        let width = (el.width as f64 * scale) as i64;
                        let mut points: Vec<(i64, i64)> = pts().collect();
                        // The database extends every path end by half its width (PATHTYPE 2):
                        // other end styles move the end points by the difference.
                        let ext = match el.pathtype {
                            0 => Some((0, 0)),
                            4 => Some(((el.ext.0 as f64 * scale) as i64, (el.ext.1 as f64 * scale) as i64)),
                            1 => {
                                round_paths += 1;
                                None
                            }
                            _ => None,
                        };
                        if let Some((b, e)) = ext
                            && points.len() >= 2
                        {
                            let n = points.len();
                            if !path_end_moves(&points, 0, 1, b - width / 2)
                                || !path_end_moves(&points, n - 1, n - 2, e - width / 2)
                            {
                                short_paths += 1;
                            } else {
                                move_path_end(&mut points, 0, 1, b - width / 2);
                                move_path_end(&mut points, n - 1, n - 2, e - width / 2);
                            }
                        }
                        Geometry::Path { points, width }
                    }
                    _ => Geometry::Label {
                        text: std::mem::take(&mut el.text),
                        at: pts().next().unwrap_or_default(),
                        size: (el.mag * tech.dbu_per_um as f64).max(50.0).round() as i64,
                    },
                };
                c.1.add(layer, geom);
                if let (Some(id), Some(s)) = (el.item_id, c.1.shapes.last_mut()) {
                    s.id = id;
                }
            }
            _ => {}
        }
    }
    if odd_angles > 0 {
        out.warnings.push(format!("{odd_angles} instance(s) with a rotation that is not a multiple of 90 degrees were rounded"));
    }
    if round_paths > 0 {
        out.warnings.push(format!("{round_paths} path(s) with round ends (PATHTYPE 1) were read with square ends"));
    }
    if short_paths > 0 {
        out.warnings.push(format!(
            "{short_paths} path(s) with flush or custom ends have an end segment shorter than the change: read with half-width ends"
        ));
    }
    if mags > 0 {
        out.warnings.push(format!("{mags} instance(s) with magnification other than 1 were placed unscaled"));
    }
    Ok(out)
}

/// Maps stream (layer, datatype) pairs to technology layer indices as described at
/// [`read_gds_with`].
pub(crate) struct LayerMap {
    by_gds: HashMap<(u16, u16), usize>,
    /// Pairs the stream map drops.
    dropped: HashSet<(u16, u16)>,
    /// Pairs the stream map's label rows read texts from.
    text_by_gds: HashMap<(u16, u16), usize>,
    new_index: HashMap<(u16, u16), usize>,
    base: usize,
    add_unknown: bool,
}

impl LayerMap {
    pub(crate) fn new(tech: &Tech, opts: ReadOptions) -> Self {
        let mut by_gds: HashMap<(u16, u16), usize> = HashMap::with_capacity(tech.layers.len());
        for (k, l) in tech.layers.iter().enumerate() {
            by_gds.entry((l.gds_layer, l.gds_datatype)).or_insert(k);
        }
        let mut dropped = HashSet::new();
        let mut text_by_gds = HashMap::new();
        if opts.use_stream_map {
            text_by_gds = crate::stream_map::import_text_pairs(tech);
            for (pair, to) in crate::stream_map::import_pairs(tech) {
                match to {
                    Some(k) => {
                        by_gds.insert(pair, k);
                    }
                    None => {
                        dropped.insert(pair);
                    }
                }
            }
        }
        Self {
            by_gds,
            dropped,
            text_by_gds,
            new_index: HashMap::new(),
            base: tech.layers.len(),
            add_unknown: opts.add_unknown_layers,
        }
    }

    /// The layer of a text on `(layer, dt)`: a label row of the stream map, else [`Self::get`].
    pub(crate) fn get_text(&mut self, layer: u16, dt: u16, out: &mut GdsImport) -> Option<usize> {
        match self.text_by_gds.get(&(layer, dt)) {
            Some(&k) => Some(k),
            None => self.get(layer, dt, out),
        }
    }

    /// The layer of a shape on `(layer, dt)`, or `None` (counted in `out`) when it is skipped.
    pub(crate) fn get(&mut self, layer: u16, dt: u16, out: &mut GdsImport) -> Option<usize> {
        if self.dropped.contains(&(layer, dt)) {
            out.skipped_unknown_layers += 1;
            *out.skipped_pairs.entry((layer, dt)).or_default() += 1;
            return None;
        }
        if self.add_unknown {
            return Some(self.by_gds.get(&(layer, dt)).copied().unwrap_or_else(|| {
                *self.new_index.entry((layer, dt)).or_insert_with(|| {
                    let k = self.base + out.new_layers.len();
                    out.new_layers.push(LayerDef::generated(layer, dt, k));
                    k
                })
            }));
        }
        if let Some(&l) = self.by_gds.get(&(layer, dt)) {
            return Some(l);
        }
        let l = self.by_gds.get(&(layer, 0)).copied();
        if l.is_some() {
            *out.datatype_fallbacks.entry((layer, dt)).or_default() += 1;
        } else {
            out.skipped_unknown_layers += 1;
            *out.skipped_pairs.entry((layer, dt)).or_default() += 1;
        }
        l
    }
}

pub(crate) fn as_rect(pts: &[(i64, i64)]) -> Option<IRect> {
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
pub(crate) mod tests {
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
    fn large_records_are_split() {
        use crate::polybool::PolySet;
        let tech = Tech::generic();
        // A staircase of 8402 vertices: more than one XY record holds.
        let mut pts = vec![];
        for i in 0..4200i64 {
            pts.push((i * 200, i * 200));
            pts.push(((i + 1) * 200, i * 200));
        }
        pts.push((4200 * 200, -1000));
        pts.push((0, -1000));
        let mut l = Layout::default();
        l.add(6, Geometry::Polygon { points: pts.clone() });
        l.add(7, Geometry::Path { points: (0..9000).map(|i| (i * 100, (i % 2) * 100)).collect(), width: 50 });
        let leaf = CellRef::new("lib", "leaf");
        let mut top = Layout::default();
        top.add_instance(leaf.clone(), Trans::IDENTITY, Some(ArraySpec { cols: 40000, rows: 2, col: (10, 0), row: (0, 10) }));
        let tr = CellRef::new("lib", "top");
        let bytes = write_gds("lib", &[(&leaf, &l), (&tr, &top)], &tech);
        let imp = read_gds(&bytes, &tech, "lib").expect("the file reads back");
        let back = &imp.cells.iter().find(|(n, _)| n == "leaf").unwrap().1;
        let polys: Vec<&Geometry> = back.shapes.iter().filter(|s| s.layer == 6).map(|s| &s.geom).collect();
        assert!(polys.len() > 1);
        let area = |g: Vec<&Geometry>| PolySet::from_geometries(g).area();
        assert_eq!(area(polys), PolySet::from_polygon(&pts).area(), "the pieces cover the polygon");
        let paths = back.shapes.iter().filter(|s| s.layer == 7).count();
        assert_eq!(paths, 2);
        let t = &imp.cells.iter().find(|(n, _)| n == "top").unwrap().1;
        assert_eq!(t.instances.iter().map(|i| i.count()).sum::<u64>(), 80000, "two AREFs of 32767 and 7233 columns");
    }

    #[test]
    fn path_end_types() {
        // PATHTYPE 0 (flush) and 4 (custom extensions) read as the same mask with half-width ends.
        let tech = Tech::generic();
        let rec = |t: u16, d: &[u8]| {
            let mut v = ((d.len() + 4) as u16).to_be_bytes().to_vec();
            v.extend_from_slice(&t.to_be_bytes());
            v.extend_from_slice(d);
            v
        };
        let i4 = |v: &[i32]| v.iter().flat_map(|x| x.to_be_bytes()).collect::<Vec<u8>>();
        let path = |pt: i16, ext: Option<(i32, i32)>| {
            let mut d = rec(HEADER, &600i16.to_be_bytes());
            d.extend(rec(BGNLIB, &[0; 24]));
            d.extend(rec(LIBNAME, b"LB"));
            let mut u = real8(1e-3).to_vec();
            u.extend_from_slice(&real8(1e-9));
            d.extend(rec(UNITS, &u));
            d.extend(rec(BGNSTR, &[0; 24]));
            d.extend(rec(STRNAME, b"P\0"));
            d.extend(rec(PATH, &[]));
            d.extend(rec(LAYER, &7i16.to_be_bytes()));
            d.extend(rec(DATATYPE, &0i16.to_be_bytes()));
            d.extend(rec(PATHTYPE, &pt.to_be_bytes()));
            d.extend(rec(WIDTH, &i4(&[200])));
            if let Some((b, e)) = ext {
                d.extend(rec(BGNEXTN, &i4(&[b])));
                d.extend(rec(ENDEXTN, &i4(&[e])));
            }
            d.extend(rec(XY, &i4(&[0, 0, 2000, 0])));
            d.extend(rec(ENDEL, &[]));
            d.extend(rec(ENDSTR, &[]));
            d.extend(rec(ENDLIB, &[]));
            let imp = read_gds(&d, &tech, "lib").unwrap();
            imp.cells[0].1.shapes[0].bbox()
        };
        assert_eq!(path(2, None), IRect::new(-100, -100, 2100, 100));
        assert_eq!(path(0, None), IRect::new(0, -100, 2000, 100), "flush ends");
        assert_eq!(path(4, Some((300, 0))), IRect::new(-300, -100, 2000, 100), "custom extensions");
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
        assert_eq!(skip.datatype_fallbacks.into_iter().collect::<Vec<_>>(), [((7, 16), 1)], "the fallback is reported");
        assert_eq!(skip.skipped_pairs.into_iter().collect::<Vec<_>>(), [((99, 0), 2)]);

        let imp = read_gds_with(&bytes, &generic, "lib", ReadOptions { add_unknown_layers: true, ..Default::default() }).unwrap();
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

    pub(crate) fn hier_lib() -> (HashMap<CellRef, Layout>, CellRef) {
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
        t.add_instance(CellRef::new("c", "top"), Trans::IDENTITY, None);
        let bytes = write_gds("x", &[(&top, &t), (&ia, &leaf), (&ib, &leaf)], &tech);
        let imp = read_gds(&bytes, &tech, "x").unwrap();
        let names: Vec<&str> = imp.cells.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["a__inv", "b__inv", "top"]);
        let refs: Vec<&str> = imp.cells[2].1.instances.iter().map(|i| i.cell.cell.as_str()).collect();
        assert_eq!(refs, ["a__inv", "b__inv", "ext", "c__top"]);
        assert_eq!(imp.top_cells(), ["top"]);
    }

    #[test]
    fn instance_names_survive_as_properties() {
        let tech = Tech::generic();
        let (leaf, top) = (CellRef::new("l", "leaf"), CellRef::new("l", "top"));
        let mut t = Layout::default();
        let a = t.add_instance(leaf.clone(), Trans::with(10, 20, 1, true), None);
        t.instance_mut(a).unwrap().name = Some("M1".into());
        t.add_instance(leaf.clone(), Trans::IDENTITY, None);
        let b = t.add_instance(leaf.clone(), Trans::IDENTITY, Some(ArraySpec { cols: 2, rows: 3, col: (5, 0), row: (0, 7) }));
        t.instance_mut(b).unwrap().name = Some("XARR".into());
        let mut bytes = write_gds("l", &[(&top, &t), (&leaf, &Layout::default())], &tech);
        let imp = read_gds(&bytes, &tech, "l").unwrap();
        let names: Vec<Option<&str>> = imp.cells[1].1.instances.iter().map(|i| i.name.as_deref()).collect();
        assert_eq!(names, [Some("M1"), None, Some("XARR")]);

        let mut w = W(Vec::new());
        w.i16s(PROPATTR, &[1]);
        w.str(PROPVALUE, "other");
        let second_endel = bytes.windows(4).enumerate().filter(|(_, r)| *r == [0, 4, 0x11, 0]).nth(1).unwrap().0;
        bytes.splice(second_endel..second_endel, w.0);
        let imp = read_gds(&bytes, &tech, "l").unwrap();
        let again: Vec<Option<&str>> = imp.cells[1].1.instances.iter().map(|i| i.name.as_deref()).collect();
        assert_eq!(again, names, "only attribute {INST_NAME_ATTR} names instances");
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
