//! OASIS (SEMI P39) stream writer and reader.
//!
//! The writer emits a CELLNAME table, one CELL per layout (its body DEFLATE-compressed in a
//! CBLOCK when that pays off), RECTANGLE / POLYGON / PATH / TEXT elements and PLACEMENT records
//! with regular-array repetitions. Instance names travel as `S_GDS_PROPERTY` properties with
//! attribute [`INST_NAME_ATTR`], the convention KLayout uses for GDSII properties.
//!
//! The reader accepts every record type: name tables anywhere in the file (including after
//! their use), all point-list and repetition types, relative xy mode, CBLOCKs, trapezoids and
//! constrained trapezoids and circles (as polygons). XGEOMETRYs are skipped with a warning.

use std::collections::HashMap;
use std::ops::Range;

use crate::gds::{GdsImport, INST_NAME_ATTR, LayerMap, ReadOptions, StreamNames, as_rect, child_first_order, hier_refs};
use crate::geom::IRect;
use crate::hier::LayoutSource;
use crate::layout::{ArraySpec, Geometry, Layout, Trans};
use crate::project::CellRef;
use crate::tech::Tech;

pub const MAGIC: &[u8] = b"%SEMI-OASIS\r\n";

const PAD: u64 = 0;
const START: u64 = 1;
const END: u64 = 2;
const CELLNAME: u64 = 3;
const PROPSTRING_REF: u64 = 10;
const LAYERNAME: u64 = 11;
const LAYERNAME_TEXT: u64 = 12;
const CELL_REF: u64 = 13;
const CELL: u64 = 14;
const XYABSOLUTE: u64 = 15;
const XYRELATIVE: u64 = 16;
const PLACEMENT: u64 = 17;
const PLACEMENT_MAG: u64 = 18;
const TEXT: u64 = 19;
const RECTANGLE: u64 = 20;
const POLYGON: u64 = 21;
const PATH: u64 = 22;
const TRAPEZOID: u64 = 23;
const TRAPEZOID_A: u64 = 24;
const TRAPEZOID_B: u64 = 25;
const CTRAPEZOID: u64 = 26;
const CIRCLE: u64 = 27;
const PROPERTY: u64 = 28;
const PROPERTY_REPEAT: u64 = 29;
const XNAME: u64 = 30;
const XNAME_REF: u64 = 31;
const XELEMENT: u64 = 32;
const XGEOMETRY: u64 = 33;
const CBLOCK: u64 = 34;

const S_GDS_PROPERTY: &str = "S_GDS_PROPERTY";
/// Cell bodies at least this long are written compressed.
const CBLOCK_MIN: usize = 256;

#[derive(Default)]
struct W(Vec<u8>);

impl W {
    fn byte(&mut self, v: u8) {
        self.0.push(v);
    }
    fn uint(&mut self, mut v: u64) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.0.push(b);
                return;
            }
            self.0.push(b | 0x80);
        }
    }
    fn sint(&mut self, v: i64) {
        self.uint((v.unsigned_abs() << 1) | u64::from(v < 0));
    }
    fn string(&mut self, s: &[u8]) {
        self.uint(s.len() as u64);
        self.0.extend_from_slice(s);
    }
    fn real(&mut self, v: f64) {
        if v >= 0.0 && v.fract() == 0.0 && v < 1e15 {
            self.uint(0);
            self.uint(v as u64);
        } else {
            self.uint(7);
            self.0.extend_from_slice(&v.to_le_bytes());
        }
    }
    fn gdelta(&mut self, (dx, dy): (i64, i64)) {
        let oct = match (dx, dy) {
            (x, 0) if x >= 0 => Some((0, x)),
            (x, 0) => Some((2, -x)),
            (0, y) if y > 0 => Some((1, y)),
            (0, y) => Some((3, -y)),
            (x, y) if x == y => Some(if x > 0 { (4, x) } else { (6, -x) }),
            (x, y) if x == -y => Some(if y > 0 { (5, y) } else { (7, x) }),
            _ => None,
        };
        match oct {
            Some((dir, m)) => self.uint(((m as u64) << 4) | (dir << 1)),
            None => {
                self.uint((dx.unsigned_abs() << 2) | (u64::from(dx < 0) << 1) | 1);
                self.sint(dy);
            }
        }
    }
    /// A g-delta point list of `pts` relative to its first point.
    fn point_list(&mut self, pts: &[(i64, i64)]) {
        self.uint(4);
        self.uint(pts.len() as u64 - 1);
        for w in pts.windows(2) {
            self.gdelta((w[1].0 - w[0].0, w[1].1 - w[0].1));
        }
    }
}

/// Serialize the given cells to an OASIS byte stream, children before their parents, with
/// cell names as in [`crate::gds::write_gds`].
pub fn write_oasis(cells: &[(&CellRef, &Layout)], tech: &Tech) -> Vec<u8> {
    let names = StreamNames::new(cells);
    let order = child_first_order(cells);
    let mut table: Vec<String> = Vec::with_capacity(cells.len());
    let mut refnum: HashMap<String, u64> = HashMap::with_capacity(cells.len());
    let mut number = |n: String| {
        if !refnum.contains_key(&n) {
            refnum.insert(n.clone(), table.len() as u64);
            table.push(n);
        }
    };
    for &k in &order {
        number(names.structure(cells[k].0).to_string());
    }
    for &k in &order {
        for inst in &cells[k].1.instances {
            number(names.reference(&inst.cell));
        }
    }
    let mut w = W(MAGIC.to_vec());
    w.uint(START);
    w.string(b"1.0");
    w.real(tech.dbu_per_um as f64);
    w.uint(0);
    for _ in 0..12 {
        w.uint(0);
    }
    for n in &table {
        w.uint(CELLNAME);
        w.string(n.as_bytes());
    }
    for &k in &order {
        let (r, layout) = cells[k];
        w.uint(CELL_REF);
        w.uint(refnum[names.structure(r)]);
        let body = cell_body(layout, tech, &|c| refnum[&names.reference(c)]);
        if body.len() >= CBLOCK_MIN {
            let z = miniz_oxide::deflate::compress_to_vec(&body, 6);
            w.uint(CBLOCK);
            w.uint(0);
            w.uint(body.len() as u64);
            w.uint(z.len() as u64);
            w.0.extend_from_slice(&z);
        } else {
            w.0.extend_from_slice(&body);
        }
    }
    // END is 256 bytes long: record id, padding b-string (2-byte length + 252 bytes), no validation.
    w.uint(END);
    w.string(&[0; 252]);
    w.uint(0);
    w.0
}

/// Serialize `tops` and every cell they place (directly or indirectly) that `src` resolves.
pub fn write_oasis_hier(src: &dyn LayoutSource, tops: &[CellRef], tech: &Tech) -> Vec<u8> {
    let refs = hier_refs(src, tops);
    let cells: Vec<(&CellRef, &Layout)> = refs.iter().filter_map(|r| Some((r, src.layout(r)?))).collect();
    write_oasis(&cells, tech)
}

fn cell_body(layout: &Layout, tech: &Tech, cell_ref: &dyn Fn(&CellRef) -> u64) -> Vec<u8> {
    let mut w = W::default();
    // Modal layer / datatype of geometry and of text: the L and D bits are set only on change.
    let mut geo: Option<(u64, u64)> = None;
    let mut txt: Option<(u64, u64)> = None;
    let layer_bits = |cur: &mut Option<(u64, u64)>, l: (u64, u64)| {
        let bits = match *cur {
            Some((a, b)) => u8::from(a != l.0) | (u8::from(b != l.1) << 1),
            None => 3,
        };
        *cur = Some(l);
        bits
    };
    let put_layer = |w: &mut W, bits: u8, l: (u64, u64)| {
        if bits & 1 != 0 {
            w.uint(l.0);
        }
        if bits & 2 != 0 {
            w.uint(l.1);
        }
    };
    for s in &layout.shapes {
        let Some(l) = tech.layer(s.layer) else { continue };
        let ld = (u64::from(l.gds_layer), u64::from(l.gds_datatype));
        match &s.geom {
            Geometry::Rect(r) => {
                let bits = layer_bits(&mut geo, ld);
                let (wd, ht) = (r.width(), r.height());
                let size = if wd == ht { 0xC0 } else { 0x60 };
                w.uint(RECTANGLE);
                w.byte(size | 0x18 | bits);
                put_layer(&mut w, bits, ld);
                w.uint(wd as u64);
                if wd != ht {
                    w.uint(ht as u64);
                }
                w.sint(r.x0);
                w.sint(r.y0);
            }
            Geometry::Polygon { points } if points.len() >= 3 => {
                let bits = layer_bits(&mut geo, ld);
                let n = if points.len() > 3 && points.first() == points.last() { points.len() - 1 } else { points.len() };
                w.uint(POLYGON);
                w.byte(0x38 | bits);
                put_layer(&mut w, bits, ld);
                w.point_list(&points[..n]);
                w.sint(points[0].0);
                w.sint(points[0].1);
            }
            Geometry::Path { points, width } if points.len() >= 2 => {
                let bits = layer_bits(&mut geo, ld);
                w.uint(PATH);
                w.byte(0xF8 | bits);
                put_layer(&mut w, bits, ld);
                w.uint((*width / 2) as u64);
                w.uint(0b1010);
                w.point_list(points);
                w.sint(points[0].0);
                w.sint(points[0].1);
            }
            Geometry::Label { text, at, .. } => {
                let bits = layer_bits(&mut txt, ld);
                w.uint(TEXT);
                w.byte(0x58 | bits);
                w.string(text.as_bytes());
                put_layer(&mut w, bits, ld);
                w.sint(at.0);
                w.sint(at.1);
            }
            _ => {}
        }
    }
    let mut last_cell = None;
    for inst in &layout.instances {
        let n = cell_ref(&inst.cell);
        let t = inst.trans;
        let (cols, rows) = inst.dims();
        let array = inst.array.filter(|_| cols * rows > 1);
        let mut info = 0x30 | ((t.rot % 4) << 1) | u8::from(t.mirror);
        if last_cell != Some(n) {
            info |= 0xC0;
        }
        if array.is_some() {
            info |= 0x08;
        }
        w.uint(PLACEMENT);
        w.byte(info);
        if info & 0x80 != 0 {
            w.uint(n);
        }
        last_cell = Some(n);
        w.sint(t.x);
        w.sint(t.y);
        if let Some(a) = array {
            let (cols, rows) = (u64::from(cols), u64::from(rows));
            match (a.col, a.row) {
                _ if cols > 1 && rows > 1 => {
                    w.uint(8);
                    w.uint(cols - 2);
                    w.uint(rows - 2);
                    w.gdelta(a.col);
                    w.gdelta(a.row);
                }
                ((s, 0), _) if rows == 1 && s >= 0 => {
                    w.uint(2);
                    w.uint(cols - 2);
                    w.uint(s as u64);
                }
                (_, (0, s)) if cols == 1 && s >= 0 => {
                    w.uint(3);
                    w.uint(rows - 2);
                    w.uint(s as u64);
                }
                _ => {
                    w.uint(9);
                    w.uint(cols.max(rows) - 2);
                    w.gdelta(if cols > 1 { a.col } else { a.row });
                }
            }
        }
        if let Some(name) = &inst.name {
            w.uint(PROPERTY);
            w.byte(0x25);
            w.string(S_GDS_PROPERTY.as_bytes());
            w.uint(8);
            w.uint(INST_NAME_ATTR as u64);
            w.uint(11);
            w.string(name.as_bytes());
        }
    }
    w.0
}

struct Cur<'a> {
    d: &'a [u8],
    i: usize,
}

impl<'a> Cur<'a> {
    fn byte(&mut self) -> Result<u8, String> {
        let b = *self.d.get(self.i).ok_or("unexpected end of OASIS data")?;
        self.i += 1;
        Ok(b)
    }
    fn uint(&mut self) -> Result<u64, String> {
        let (mut v, mut shift) = (0u64, 0u32);
        loop {
            let b = self.byte()?;
            if shift < 64 {
                v |= u64::from(b & 0x7f) << shift;
            }
            shift += 7;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
    }
    fn sint(&mut self) -> Result<i64, String> {
        let u = self.uint()?;
        let m = (u >> 1) as i64;
        Ok(if u & 1 != 0 { -m } else { m })
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.d.len() - self.i < n {
            return Err("unexpected end of OASIS data".into());
        }
        self.i += n;
        Ok(&self.d[self.i - n..self.i])
    }
    fn string(&mut self) -> Result<String, String> {
        let n = self.uint()? as usize;
        Ok(String::from_utf8_lossy(self.bytes(n)?).into_owned())
    }
    fn real(&mut self) -> Result<f64, String> {
        let t = self.uint()?;
        self.real_of(t)
    }
    fn real_of(&mut self, t: u64) -> Result<f64, String> {
        let u = |c: &mut Self| c.uint().map(|v| v as f64);
        Ok(match t {
            0 => u(self)?,
            1 => -u(self)?,
            2 => 1.0 / u(self)?,
            3 => -1.0 / u(self)?,
            4 => u(self)? / u(self)?,
            5 => -u(self)? / u(self)?,
            6 => f64::from(f32::from_le_bytes(self.bytes(4)?.try_into().unwrap())),
            7 => f64::from_le_bytes(self.bytes(8)?.try_into().unwrap()),
            _ => return Err(format!("invalid real type {t}")),
        })
    }
    fn gdelta(&mut self) -> Result<(i64, i64), String> {
        let u = self.uint()?;
        if u & 1 == 0 {
            return Ok(octangular(u >> 1, (u >> 4) as i64));
        }
        let x = (u >> 2) as i64;
        Ok((if u & 2 != 0 { -x } else { x }, self.sint()?))
    }
    /// Vertices of a point list, starting with the implicit `(0, 0)`.
    fn point_list(&mut self, polygon: bool) -> Result<Vec<(i64, i64)>, String> {
        let t = self.uint()?;
        let n = self.uint()? as usize;
        let mut pts = Vec::with_capacity(n.min(self.d.len() - self.i) + 2);
        pts.push((0, 0));
        let (mut x, mut y) = (0i64, 0i64);
        let mut step = (0i64, 0i64);
        for k in 0..n {
            let d = match t {
                0 | 1 => {
                    let v = self.sint()?;
                    if (k % 2 == 0) == (t == 0) { (v, 0) } else { (0, v) }
                }
                2 => {
                    let u = self.uint()?;
                    octangular(u & 3, (u >> 2) as i64)
                }
                3 => {
                    let u = self.uint()?;
                    octangular(u & 7, (u >> 3) as i64)
                }
                4 | 5 => self.gdelta()?,
                _ => return Err(format!("invalid point list type {t}")),
            };
            if t == 5 {
                step = (step.0 + d.0, step.1 + d.1);
            } else {
                step = d;
            }
            x += step.0;
            y += step.1;
            pts.push((x, y));
        }
        if polygon && t <= 1 {
            let horizontal_next = n.is_multiple_of(2) == (t == 0);
            pts.push(if horizontal_next { (0, y) } else { (x, 0) });
        }
        Ok(pts)
    }
}

fn octangular(dir: u64, m: i64) -> (i64, i64) {
    match dir & 7 {
        0 => (m, 0),
        1 => (0, m),
        2 => (-m, 0),
        3 => (0, -m),
        4 => (m, m),
        5 => (-m, m),
        6 => (-m, -m),
        _ => (m, -m),
    }
}

fn interval(c: &mut Cur) -> Result<(), String> {
    match c.uint()? {
        0 => {}
        1..=3 => {
            c.uint()?;
        }
        4 => {
            c.uint()?;
            c.uint()?;
        }
        t => return Err(format!("invalid interval type {t}")),
    }
    Ok(())
}

#[derive(Debug, Clone)]
enum NameRef {
    Str(String),
    Ref(u64),
}

#[derive(Debug, Clone)]
enum PropValue {
    Num(f64),
    Str(String),
    Ref(u64),
}

#[derive(Debug, Clone)]
enum Rep {
    Matrix { cols: u64, rows: u64, col: (i64, i64), row: (i64, i64) },
    List(Vec<(i64, i64)>),
}

impl Rep {
    fn offsets(&self) -> Vec<(i64, i64)> {
        match self {
            Rep::Matrix { cols, rows, col, row } => (0..*rows as i64)
                .flat_map(|r| (0..*cols as i64).map(move |c| (c * col.0 + r * row.0, c * col.1 + r * row.1)))
                .collect(),
            Rep::List(v) => v.clone(),
        }
    }
}

/// Modal variables (reset at every CELL record).
#[derive(Default)]
struct Modal {
    relative: bool,
    placement: (i64, i64),
    geometry: (i64, i64),
    text: (i64, i64),
    placement_cell: Option<NameRef>,
    layer: u64,
    datatype: u64,
    textlayer: u64,
    texttype: u64,
    text_string: Option<NameRef>,
    width: u64,
    height: u64,
    polygon: Vec<(i64, i64)>,
    path: Vec<(i64, i64)>,
    half_width: u64,
    start_ext: i64,
    end_ext: i64,
    radius: u64,
    ctrapezoid_type: u64,
    repetition: Option<Rep>,
    prop_name: Option<NameRef>,
    prop_values: Vec<PropValue>,
}

/// Element geometry in file units, relative to the element position.
enum Local {
    Rect(i64, i64),
    Poly(Vec<(i64, i64)>),
    Path { points: Vec<(i64, i64)>, half_width: i64, start: i64, end: i64 },
}

const CELLNAMES: usize = 0;
const TEXTSTRINGS: usize = 1;
const PROPNAMES: usize = 2;
const PROPSTRINGS: usize = 3;

struct Reader<'t> {
    tech: &'t Tech,
    lib: String,
    layers: LayerMap,
    out: GdsImport,
    scale: f64,
    label_size: i64,
    tables: [HashMap<u64, String>; 4],
    implicit: [u64; 4],
    cells: Vec<(NameRef, Layout)>,
    /// Every instance (cell index, instance index) and the name of the cell it places.
    insts: Vec<(usize, usize, NameRef)>,
    /// Labels whose text is a TEXTSTRING reference.
    texts: Vec<(usize, usize, u64)>,
    props: Vec<(usize, Range<usize>, NameRef, Vec<PropValue>)>,
    /// Instances created by the last element, which following PROPERTY records describe.
    last_insts: Option<(usize, Range<usize>)>,
    m: Modal,
    done: bool,
    odd_angles: usize,
    mags: usize,
    unsupported: usize,
}

/// [`read_oasis_with`] with default options: shapes on unknown layers are skipped.
pub fn read_oasis(data: &[u8], tech: &Tech, lib: &str) -> Result<GdsImport, String> {
    read_oasis_with(data, tech, lib, ReadOptions::default())
}

/// Parse an OASIS stream. Layers are mapped as in [`crate::gds::read_gds_with`]; instances
/// refer to cells of library `lib` by their cell name. Labels get a size of 0.25 µm, since
/// OASIS texts have none.
pub fn read_oasis_with(data: &[u8], tech: &Tech, lib: &str, opts: ReadOptions) -> Result<GdsImport, String> {
    let body = data.strip_prefix(MAGIC).ok_or("not an OASIS file (missing magic string)")?;
    let mut r = Reader {
        tech,
        lib: lib.to_string(),
        layers: LayerMap::new(tech, opts),
        out: GdsImport { new_layer_base: tech.layers.len(), ..GdsImport::default() },
        scale: 1.0,
        label_size: (tech.dbu_per_um / 4).max(1),
        tables: Default::default(),
        implicit: [0; 4],
        cells: vec![],
        insts: vec![],
        texts: vec![],
        props: vec![],
        last_insts: None,
        m: Modal::default(),
        done: false,
        odd_angles: 0,
        mags: 0,
        unsupported: 0,
    };
    r.records(body)?;
    if !r.done {
        return Err("OASIS file has no END record (truncated?)".into());
    }
    r.finish()
}

impl Reader<'_> {
    fn records(&mut self, d: &[u8]) -> Result<(), String> {
        let mut c = Cur { d, i: 0 };
        while c.i < d.len() && !self.done {
            let at = c.i;
            let id = c.uint()?;
            self.record(id, &mut c).map_err(|e| format!("{e} (record {id} at byte {at})"))?;
            if !matches!(id, PAD | CELLNAME..=PROPSTRING_REF | PLACEMENT | PLACEMENT_MAG | PROPERTY | PROPERTY_REPEAT | CBLOCK) {
                self.last_insts = None;
            }
        }
        Ok(())
    }

    fn record(&mut self, id: u64, c: &mut Cur) -> Result<(), String> {
        match id {
            PAD => {}
            START => {
                c.string()?;
                let unit = c.real()?;
                if !(unit.is_finite() && unit > 0.0) {
                    return Err(format!("invalid unit {unit}"));
                }
                self.scale = self.tech.dbu_per_um as f64 / unit;
                self.out.dbu_meters = 1e-6 / unit;
                if c.uint()? == 0 {
                    for _ in 0..12 {
                        c.uint()?;
                    }
                }
            }
            END => self.done = true,
            CELLNAME..=PROPSTRING_REF => {
                let s = c.string()?;
                let t = ((id - CELLNAME) / 2) as usize;
                let n = if (id - CELLNAME) % 2 == 1 {
                    c.uint()?
                } else {
                    self.implicit[t] += 1;
                    self.implicit[t] - 1
                };
                self.tables[t].insert(n, s);
            }
            LAYERNAME | LAYERNAME_TEXT => {
                c.string()?;
                interval(c)?;
                interval(c)?;
            }
            CELL_REF | CELL => {
                let name = if id == CELL_REF { NameRef::Ref(c.uint()?) } else { NameRef::Str(c.string()?) };
                self.cells.push((name, Layout::default()));
                self.m = Modal::default();
            }
            XYABSOLUTE => self.m.relative = false,
            XYRELATIVE => self.m.relative = true,
            PLACEMENT | PLACEMENT_MAG => self.placement(id, c)?,
            TEXT => self.text(c)?,
            RECTANGLE..=CIRCLE => self.geometry(id, c)?,
            PROPERTY => self.property(c)?,
            PROPERTY_REPEAT => self.attach_property(),
            XNAME | XNAME_REF => {
                c.uint()?;
                c.string()?;
                if id == XNAME_REF {
                    c.uint()?;
                }
            }
            XELEMENT => {
                c.uint()?;
                c.string()?;
            }
            XGEOMETRY => {
                let info = c.byte()?;
                c.uint()?;
                if info & 0x01 != 0 {
                    self.m.layer = c.uint()?;
                }
                if info & 0x02 != 0 {
                    self.m.datatype = c.uint()?;
                }
                c.string()?;
                let rel = self.m.relative;
                coord(c, rel, info & 0x10 != 0, &mut self.m.geometry.0)?;
                coord(c, rel, info & 0x08 != 0, &mut self.m.geometry.1)?;
                if info & 0x04 != 0 {
                    self.repetition(c)?;
                }
                self.unsupported += 1;
            }
            CBLOCK => {
                let kind = c.uint()?;
                let n = c.uint()? as usize;
                let z = c.uint()? as usize;
                let comp = c.bytes(z)?;
                if kind != 0 {
                    return Err(format!("unsupported CBLOCK compression type {kind}"));
                }
                let raw = miniz_oxide::inflate::decompress_to_vec_with_limit(comp, n)
                    .map_err(|e| format!("corrupt CBLOCK: {:?}", e.status))?;
                self.records(&raw)?;
            }
            _ => return Err(format!("unknown record type {id}")),
        }
        Ok(())
    }

    fn sc(&self, v: i64) -> i64 {
        if self.scale == 1.0 { v } else { (v as f64 * self.scale).round() as i64 }
    }

    fn layer(&mut self, layer: u64, dt: u64) -> Option<usize> {
        self.layers.get(layer as u16, dt as u16, &mut self.out)
    }

    fn repetition(&mut self, c: &mut Cur) -> Result<Rep, String> {
        let t = c.uint()?;
        let dim = |c: &mut Cur| c.uint().map(|n| n.saturating_add(2));
        let rep = match t {
            0 => return self.m.repetition.clone().ok_or_else(|| "repetition reused before one was defined".into()),
            1 => {
                let (cols, rows) = (dim(c)?, dim(c)?);
                let (sx, sy) = (c.uint()? as i64, c.uint()? as i64);
                Rep::Matrix { cols, rows, col: (sx, 0), row: (0, sy) }
            }
            2 => Rep::Matrix { cols: dim(c)?, rows: 1, col: (c.uint()? as i64, 0), row: (0, 0) },
            3 => Rep::Matrix { cols: 1, rows: dim(c)?, col: (0, 0), row: (0, c.uint()? as i64) },
            4..=7 => {
                let n = dim(c)?;
                let grid = if t % 2 == 1 { c.uint()? as i64 } else { 1 };
                let mut v = vec![(0, 0)];
                let mut p = 0i64;
                for _ in 1..n {
                    p += c.uint()? as i64 * grid;
                    v.push(if t < 6 { (p, 0) } else { (0, p) });
                }
                Rep::List(v)
            }
            8 => {
                let (cols, rows) = (dim(c)?, dim(c)?);
                Rep::Matrix { cols, rows, col: c.gdelta()?, row: c.gdelta()? }
            }
            9 => Rep::Matrix { cols: dim(c)?, rows: 1, col: c.gdelta()?, row: (0, 0) },
            10 | 11 => {
                let n = dim(c)?;
                let grid = if t == 11 { c.uint()? as i64 } else { 1 };
                let mut v = vec![(0, 0)];
                let mut p = (0i64, 0i64);
                for _ in 1..n {
                    let d = c.gdelta()?;
                    p = (p.0 + d.0 * grid, p.1 + d.1 * grid);
                    v.push(p);
                }
                Rep::List(v)
            }
            _ => return Err(format!("invalid repetition type {t}")),
        };
        self.m.repetition = Some(rep.clone());
        Ok(rep)
    }

    fn placement(&mut self, id: u64, c: &mut Cur) -> Result<(), String> {
        let info = c.byte()?;
        if info & 0x80 != 0 {
            self.m.placement_cell = Some(if info & 0x40 != 0 { NameRef::Ref(c.uint()?) } else { NameRef::Str(c.string()?) });
        }
        let cell = self.m.placement_cell.clone().ok_or("PLACEMENT without a cell")?;
        let (mag, angle, bits) = if id == PLACEMENT {
            (1.0, f64::from((info >> 1) & 3) * 90.0, info >> 3)
        } else {
            let mag = if info & 0x20 != 0 { c.real()? } else { 1.0 };
            let angle = if info & 0x10 != 0 { c.real()? } else { 0.0 };
            (mag, angle, info >> 1)
        };
        let rel = self.m.relative;
        coord(c, rel, bits & 0x04 != 0, &mut self.m.placement.0)?;
        coord(c, rel, bits & 0x02 != 0, &mut self.m.placement.1)?;
        let rep = if bits & 0x01 != 0 { Some(self.repetition(c)?) } else { None };
        let Some(ci) = self.cells.len().checked_sub(1) else { return Ok(()) };
        let q = angle / 90.0;
        if (q - q.round()).abs() > 1e-6 {
            self.odd_angles += 1;
        }
        if (mag - 1.0).abs() > 1e-9 {
            self.mags += 1;
        }
        let rot = (q.round() as i64).rem_euclid(4) as u8;
        let mirror = info & 1 != 0;
        let (x, y) = self.m.placement;
        let start = self.cells[ci].1.instances.len();
        let place = |r: &mut Self, (dx, dy): (i64, i64), array: Option<ArraySpec>| {
            let t = Trans::with(r.sc(x + dx), r.sc(y + dy), rot, mirror);
            let l = &mut r.cells[ci].1;
            l.add_instance(CellRef::new(&r.lib, ""), t, array);
            r.insts.push((ci, l.instances.len() - 1, cell.clone()));
        };
        match rep {
            None => place(self, (0, 0), None),
            Some(Rep::Matrix { cols, rows, col, row }) => {
                let array = ArraySpec {
                    cols: cols.min(u32::MAX as u64) as u32,
                    rows: rows.min(u32::MAX as u64) as u32,
                    col: (self.sc(col.0), self.sc(col.1)),
                    row: (self.sc(row.0), self.sc(row.1)),
                };
                place(self, (0, 0), Some(array));
            }
            Some(Rep::List(v)) => {
                for d in v {
                    place(self, d, None);
                }
            }
        }
        self.last_insts = Some((ci, start..self.cells[ci].1.instances.len()));
        Ok(())
    }

    fn text(&mut self, c: &mut Cur) -> Result<(), String> {
        let info = c.byte()?;
        if info & 0x40 != 0 {
            self.m.text_string = Some(if info & 0x20 != 0 { NameRef::Ref(c.uint()?) } else { NameRef::Str(c.string()?) });
        }
        if info & 0x01 != 0 {
            self.m.textlayer = c.uint()?;
        }
        if info & 0x02 != 0 {
            self.m.texttype = c.uint()?;
        }
        let rel = self.m.relative;
        coord(c, rel, info & 0x10 != 0, &mut self.m.text.0)?;
        coord(c, rel, info & 0x08 != 0, &mut self.m.text.1)?;
        let offsets = if info & 0x04 != 0 { self.repetition(c)?.offsets() } else { vec![(0, 0)] };
        let text = self.m.text_string.clone().ok_or("TEXT without a string")?;
        let Some(ci) = self.cells.len().checked_sub(1) else { return Ok(()) };
        let Some(layer) = self.layer(self.m.textlayer, self.m.texttype) else { return Ok(()) };
        let (x, y) = self.m.text;
        for (dx, dy) in offsets {
            let at = (self.sc(x + dx), self.sc(y + dy));
            let l = &mut self.cells[ci].1;
            let s = match &text {
                NameRef::Str(s) => s.clone(),
                NameRef::Ref(n) => {
                    self.texts.push((ci, l.shapes.len(), *n));
                    String::new()
                }
            };
            l.add(layer, Geometry::Label { text: s, at, size: self.label_size });
        }
        Ok(())
    }

    fn geometry(&mut self, id: u64, c: &mut Cur) -> Result<(), String> {
        let info = c.byte()?;
        if info & 0x01 != 0 {
            self.m.layer = c.uint()?;
        }
        if info & 0x02 != 0 {
            self.m.datatype = c.uint()?;
        }
        let local = match id {
            RECTANGLE => {
                if info & 0x40 != 0 {
                    self.m.width = c.uint()?;
                }
                if info & 0x80 != 0 {
                    self.m.height = self.m.width;
                } else if info & 0x20 != 0 {
                    self.m.height = c.uint()?;
                }
                Local::Rect(self.m.width as i64, self.m.height as i64)
            }
            POLYGON => {
                if info & 0x20 != 0 {
                    self.m.polygon = c.point_list(true)?;
                }
                Local::Poly(self.m.polygon.clone())
            }
            PATH => {
                if info & 0x40 != 0 {
                    self.m.half_width = c.uint()?;
                }
                let hw = self.m.half_width as i64;
                if info & 0x80 != 0 {
                    let s = c.uint()?;
                    for (k, ext) in [((s >> 2) & 3, &mut self.m.start_ext), (s & 3, &mut self.m.end_ext)] {
                        match k {
                            1 => *ext = 0,
                            2 => *ext = hw,
                            3 => *ext = c.sint()?,
                            _ => {}
                        }
                    }
                }
                if info & 0x20 != 0 {
                    self.m.path = c.point_list(false)?;
                }
                Local::Path { points: self.m.path.clone(), half_width: hw, start: self.m.start_ext, end: self.m.end_ext }
            }
            TRAPEZOID | TRAPEZOID_A | TRAPEZOID_B => {
                if info & 0x40 != 0 {
                    self.m.width = c.uint()?;
                }
                if info & 0x20 != 0 {
                    self.m.height = c.uint()?;
                }
                let a = if id != TRAPEZOID_B { c.sint()? } else { 0 };
                let b = if id != TRAPEZOID_A { c.sint()? } else { 0 };
                let (w, h) = (self.m.width as i64, self.m.height as i64);
                Local::Poly(if info & 0x80 != 0 {
                    vec![(0, a.max(0)), (0, h + b.min(0)), (w, h - b.max(0)), (w, -a.min(0))]
                } else {
                    vec![(a.max(0), h), (w + b.min(0), h), (w - b.max(0), 0), (-a.min(0), 0)]
                })
            }
            CTRAPEZOID => {
                if info & 0x80 != 0 {
                    self.m.ctrapezoid_type = c.uint()?;
                }
                if info & 0x40 != 0 {
                    self.m.width = c.uint()?;
                }
                if info & 0x20 != 0 {
                    self.m.height = c.uint()?;
                }
                Local::Poly(ctrapezoid(self.m.ctrapezoid_type, self.m.width as i64, self.m.height as i64)?)
            }
            _ => {
                if info & 0x20 != 0 {
                    self.m.radius = c.uint()?;
                }
                let r = self.m.radius as f64;
                Local::Poly(
                    (0..32)
                        .map(|k| {
                            let a = k as f64 * std::f64::consts::TAU / 32.0;
                            ((r * a.cos()).round() as i64, (r * a.sin()).round() as i64)
                        })
                        .collect(),
                )
            }
        };
        let rel = self.m.relative;
        coord(c, rel, info & 0x10 != 0, &mut self.m.geometry.0)?;
        coord(c, rel, info & 0x08 != 0, &mut self.m.geometry.1)?;
        let offsets = if info & 0x04 != 0 { self.repetition(c)?.offsets() } else { vec![(0, 0)] };
        let Some(ci) = self.cells.len().checked_sub(1) else { return Ok(()) };
        let Some(layer) = self.layer(self.m.layer, self.m.datatype) else { return Ok(()) };
        let (x, y) = self.m.geometry;
        for (dx, dy) in offsets {
            if let Some(g) = self.place_geometry(&local, x + dx, y + dy) {
                self.cells[ci].1.add(layer, g);
            }
        }
        Ok(())
    }

    fn place_geometry(&self, local: &Local, x: i64, y: i64) -> Option<Geometry> {
        let pts = |p: &[(i64, i64)]| -> Vec<(i64, i64)> {
            let mut v: Vec<(i64, i64)> = p.iter().map(|&(px, py)| (self.sc(x + px), self.sc(y + py))).collect();
            v.dedup();
            v
        };
        match local {
            Local::Rect(w, h) => Some(Geometry::Rect(IRect::new(self.sc(x), self.sc(y), self.sc(x + w), self.sc(y + h)))),
            Local::Poly(p) => {
                let mut v = pts(p);
                if v.len() > 1 && v.first() == v.last() {
                    v.pop();
                }
                match as_rect(&v) {
                    Some(r) => Some(Geometry::Rect(r)),
                    None => (v.len() >= 3).then_some(Geometry::Polygon { points: v }),
                }
            }
            Local::Path { points, half_width, start, end } => {
                let mut v = pts(points);
                if v.len() < 2 {
                    return None;
                }
                let n = v.len();
                adjust_end(&mut v, 0, 1, self.sc(*start - half_width));
                adjust_end(&mut v, n - 1, n - 2, self.sc(*end - half_width));
                Some(Geometry::Path { points: v, width: self.sc(2 * half_width) })
            }
        }
    }

    fn property(&mut self, c: &mut Cur) -> Result<(), String> {
        let info = c.byte()?;
        if info & 0x04 != 0 {
            self.m.prop_name = Some(if info & 0x02 != 0 { NameRef::Ref(c.uint()?) } else { NameRef::Str(c.string()?) });
        }
        if info & 0x08 == 0 {
            let mut n = u64::from(info >> 4);
            if n == 15 {
                n = c.uint()?;
            }
            let mut v = vec![];
            for _ in 0..n {
                let t = c.uint()?;
                v.push(match t {
                    0..=7 => PropValue::Num(c.real_of(t)?),
                    8 => PropValue::Num(c.uint()? as f64),
                    9 => PropValue::Num(c.sint()? as f64),
                    10..=12 => PropValue::Str(c.string()?),
                    13..=15 => PropValue::Ref(c.uint()?),
                    _ => return Err(format!("invalid property value type {t}")),
                });
            }
            self.m.prop_values = v;
        }
        self.attach_property();
        Ok(())
    }

    fn attach_property(&mut self) {
        if let (Some((ci, r)), Some(name)) = (self.last_insts.clone(), self.m.prop_name.clone()) {
            self.props.push((ci, r, name, self.m.prop_values.clone()));
        }
    }

    fn name(&self, table: usize, n: &NameRef) -> Option<String> {
        match n {
            NameRef::Str(s) => Some(s.clone()),
            NameRef::Ref(k) => self.tables[table].get(k).cloned(),
        }
    }

    fn finish(mut self) -> Result<GdsImport, String> {
        let undefined = |k: &NameRef| format!("undefined CELLNAME reference {k:?}");
        for (ci, ii, n) in std::mem::take(&mut self.insts) {
            let name = self.name(CELLNAMES, &n).ok_or_else(|| undefined(&n))?;
            self.cells[ci].1.instances[ii].cell = CellRef::new(&self.lib, name);
        }
        for (ci, si, k) in std::mem::take(&mut self.texts) {
            let s = self.tables[TEXTSTRINGS].get(&k).cloned().ok_or_else(|| format!("undefined TEXTSTRING reference {k}"))?;
            if let Geometry::Label { text, .. } = &mut self.cells[ci].1.shapes[si].geom {
                *text = s;
            }
        }
        for (ci, range, name, values) in std::mem::take(&mut self.props) {
            if self.name(PROPNAMES, &name).as_deref() != Some(S_GDS_PROPERTY) {
                continue;
            }
            let [PropValue::Num(attr), value] = values.as_slice() else { continue };
            if *attr != f64::from(INST_NAME_ATTR) {
                continue;
            }
            let value = match value {
                PropValue::Str(s) => Some(s.clone()),
                PropValue::Ref(k) => self.tables[PROPSTRINGS].get(k).cloned(),
                PropValue::Num(_) => None,
            };
            for i in range {
                self.cells[ci].1.instances[i].name = value.clone();
            }
        }
        for (n, l) in std::mem::take(&mut self.cells) {
            let name = self.name(CELLNAMES, &n).ok_or_else(|| undefined(&n))?;
            self.out.cells.push((name, l));
        }
        let w = &mut self.out.warnings;
        if self.odd_angles > 0 {
            w.push(format!("{} instance(s) with a rotation that is not a multiple of 90 degrees were rounded", self.odd_angles));
        }
        if self.mags > 0 {
            w.push(format!("{} instance(s) with magnification other than 1 were placed unscaled", self.mags));
        }
        if self.unsupported > 0 {
            w.push(format!("{} XGEOMETRY element(s) are not supported and were skipped", self.unsupported));
        }
        Ok(self.out)
    }
}

/// Vertices of CTRAPEZOID type `t` with width `w` and height `h`. Types 0-7 are horizontal
/// trapezoids with 45° sides, 8-15 their vertical counterparts, 16-23 right and isosceles
/// triangles and 24 / 25 a rectangle / square; dimensions a type implies replace the given ones.
fn ctrapezoid(t: u64, w: i64, h: i64) -> Result<Vec<(i64, i64)>, String> {
    Ok(match t {
        0 => vec![(0, 0), (0, h), (w - h, h), (w, 0)],
        1 => vec![(0, 0), (0, h), (w, h), (w - h, 0)],
        2 => vec![(0, 0), (h, h), (w, h), (w, 0)],
        3 => vec![(h, 0), (0, h), (w, h), (w, 0)],
        4 => vec![(0, 0), (h, h), (w - h, h), (w, 0)],
        5 => vec![(h, 0), (0, h), (w, h), (w - h, 0)],
        6 => vec![(0, 0), (h, h), (w, h), (w - h, 0)],
        7 => vec![(h, 0), (0, h), (w - h, h), (w, 0)],
        8 => vec![(0, 0), (0, h), (w, h - w), (w, 0)],
        9 => vec![(0, 0), (0, h - w), (w, h), (w, 0)],
        10 => vec![(0, 0), (0, h), (w, h), (w, w)],
        11 => vec![(0, w), (0, h), (w, h), (w, 0)],
        12 => vec![(0, 0), (0, h), (w, h - w), (w, w)],
        13 => vec![(0, w), (0, h - w), (w, h), (w, 0)],
        14 => vec![(0, 0), (0, h - w), (w, h), (w, w)],
        15 => vec![(0, w), (0, h), (w, h - w), (w, 0)],
        16 => vec![(0, 0), (0, w), (w, 0)],
        17 => vec![(0, 0), (0, w), (w, w)],
        18 => vec![(0, 0), (w, w), (w, 0)],
        19 => vec![(0, w), (w, w), (w, 0)],
        20 => vec![(0, 0), (h, h), (2 * h, 0)],
        21 => vec![(0, h), (2 * h, h), (h, 0)],
        22 => vec![(0, 0), (0, 2 * w), (w, w)],
        23 => vec![(w, 0), (0, w), (w, 2 * w)],
        24 => vec![(0, 0), (0, h), (w, h), (w, 0)],
        25 => vec![(0, 0), (0, w), (w, w), (w, 0)],
        _ => return Err(format!("invalid CTRAPEZOID type {t}")),
    })
}

fn coord(c: &mut Cur, relative: bool, present: bool, v: &mut i64) -> Result<(), String> {
    if present {
        let d = c.sint()?;
        *v = if relative { *v + d } else { d };
    }
    Ok(())
}

/// Move path end point `k` (whose neighbour is `prev`) outwards by `by`: paths in the database
/// extend by half their width, so other OASIS extensions shift the end point instead.
fn adjust_end(v: &mut [(i64, i64)], k: usize, prev: usize, by: i64) {
    if by == 0 {
        return;
    }
    let (dx, dy) = ((v[k].0 - v[prev].0) as f64, (v[k].1 - v[prev].1) as f64);
    let len = dx.hypot(dy);
    if len > 0.0 {
        v[k].0 += (dx / len * by as f64).round() as i64;
        v[k].1 += (dy / len * by as f64).round() as i64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gds::{StreamFormat, read_gds, read_stream_with, write_gds_hier};
    use crate::hier::flatten;
    use crate::tech::LayerDef;

    fn uint(v: u64) -> Vec<u8> {
        let mut w = W::default();
        w.uint(v);
        w.0
    }

    #[test]
    fn integer_encoding() {
        assert_eq!(uint(0), [0]);
        assert_eq!(uint(127), [0x7f]);
        assert_eq!(uint(128), [0x80, 0x01]);
        assert_eq!(uint(16383), [0xff, 0x7f]);
        assert_eq!(uint(16384), [0x80, 0x80, 0x01]);
        for v in [0i64, 1, -1, 63, -64, 1 << 40, -(1 << 40), i32::MAX as i64, i32::MIN as i64] {
            let mut w = W::default();
            w.sint(v);
            w.gdelta((v, -v / 3));
            w.gdelta((v, v));
            w.gdelta((0, v));
            w.real(v as f64 / 4.0);
            let mut c = Cur { d: &w.0, i: 0 };
            assert_eq!(c.sint().unwrap(), v);
            assert_eq!(c.gdelta().unwrap(), (v, -v / 3));
            assert_eq!(c.gdelta().unwrap(), (v, v));
            assert_eq!(c.gdelta().unwrap(), (0, v));
            assert_eq!(c.real().unwrap(), v as f64 / 4.0);
            assert_eq!(c.i, w.0.len());
        }
    }

    #[test]
    fn shapes_roundtrip() {
        let tech = Tech::generic();
        let mut l = Layout::default();
        l.add_rect(6, 0, 0, 1000, 500);
        l.add_rect(6, -300, -300, 0, 0);
        l.add(7, Geometry::Path { points: vec![(0, 0), (2000, 0), (2000, 3000), (2500, 3500)], width: 280 });
        l.add(6, Geometry::Label { text: "out".into(), at: (100, 100), size: 250 });
        l.add(4, Geometry::Polygon { points: vec![(0, 0), (500, 0), (500, 500), (250, 800), (0, 500)] });
        l.add(4, Geometry::Polygon { points: vec![(0, 0), (7, 0), (3, 11)] });
        let top = CellRef::new("lib", "top");
        let bytes = write_oasis(&[(&top, &l)], &tech);
        assert!(bytes.starts_with(MAGIC));
        let imp = read_oasis(&bytes, &tech, "lib").unwrap();
        assert_eq!(imp.cells.len(), 1);
        assert_eq!(imp.cells[0].0, "top");
        let back: Vec<(usize, &Geometry)> = imp.cells[0].1.shapes.iter().map(|s| (s.layer, &s.geom)).collect();
        let orig: Vec<(usize, &Geometry)> = l.shapes.iter().map(|s| (s.layer, &s.geom)).collect();
        assert_eq!(back, orig);
        assert!(imp.warnings.is_empty());
        assert!((imp.dbu_meters - 1e-9).abs() < 1e-18);
    }

    #[test]
    fn hierarchy_matches_gds() {
        let tech = Tech::generic();
        let (m, top) = crate::gds::tests::hier_lib();
        let bytes = write_oasis_hier(&m, std::slice::from_ref(&top), &tech);
        let imp = read_oasis(&bytes, &tech, "a").unwrap();
        let gds = read_gds(&write_gds_hier("a", &m, std::slice::from_ref(&top), &tech), &tech, "a").unwrap();
        let names: Vec<&str> = imp.cells.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["leaf", "mid", "top"]);
        assert_eq!(imp.top_cells(), ["top"]);
        let map = |imp: GdsImport| -> HashMap<CellRef, Layout> {
            imp.cells.into_iter().map(|(n, l)| (CellRef::new("a", n), l)).collect()
        };
        let (o, g) = (map(imp), map(gds));
        for (r, l) in &g {
            assert_eq!(o[r].instances, l.instances, "{r}");
        }
        let flat = |top: &Layout, src: &dyn LayoutSource| -> Vec<(usize, Geometry)> {
            flatten(top, src, 100).into_iter().map(|s| (s.layer, s.geom)).collect()
        };
        let before = flat(&m[&top], &m);
        assert_eq!(before.len(), 180);
        assert_eq!(before, flat(&o[&top], &o));
    }

    #[test]
    fn instance_names_unknown_layers_and_cblocks() {
        let generic = Tech::generic();
        let mut tech = generic.clone();
        tech.layers.push(LayerDef { name: "pad".into(), gds_layer: 99, gds_datatype: 3, ..tech.layers[0].clone() });
        let pad = generic.layers.len();
        let (leaf, top) = (CellRef::new("l", "leaf"), CellRef::new("l", "top"));
        let mut lf = Layout::default();
        for k in 0..200 {
            lf.add_rect(if k % 2 == 0 { 6 } else { pad }, k * 10, 0, k * 10 + 5, 5);
        }
        let mut t = Layout::default();
        let a = t.add_instance(leaf.clone(), Trans::with(10, 20, 1, true), None);
        t.instance_mut(a).unwrap().name = Some("M1".into());
        t.add_instance(leaf.clone(), Trans::with(-5, 0, 3, false), None);
        let b = t.add_instance(leaf.clone(), Trans::IDENTITY, Some(ArraySpec { cols: 1, rows: 3, col: (0, 0), row: (0, 7) }));
        t.instance_mut(b).unwrap().name = Some("XARR".into());
        let bytes = write_oasis(&[(&top, &t), (&leaf, &lf)], &tech);
        assert!(bytes.len() < 1000, "the leaf body is compressed ({} bytes)", bytes.len());

        let imp = read_oasis_with(&bytes, &generic, "l", ReadOptions { add_unknown_layers: true }).unwrap();
        let t2 = &imp.cells[1].1;
        let names: Vec<Option<&str>> = t2.instances.iter().map(|i| i.name.as_deref()).collect();
        assert_eq!(names, [Some("M1"), None, Some("XARR")]);
        assert_eq!(
            t2.instances.iter().map(|i| i.trans).collect::<Vec<_>>(),
            t.instances.iter().map(|i| i.trans).collect::<Vec<_>>()
        );
        assert_eq!(t2.instances[2].array, t.instances[2].array);
        assert_eq!(imp.new_layers.len(), 1);
        assert_eq!((imp.new_layers[0].gds_layer, imp.new_layers[0].gds_datatype), (99, 3));
        assert_eq!(imp.cells[0].1.shapes.len(), 200);
        assert_eq!(imp.cells[0].1.shapes[1].layer, pad);

        let skip = read_oasis(&bytes, &generic, "l").unwrap();
        assert_eq!(skip.skipped_unknown_layers, 100);
        assert_eq!(read_stream_with(&bytes, &generic, "l", ReadOptions::default()).unwrap().cells.len(), 2);
    }

    /// A file in the style of other writers: names after their use, relative coordinates and
    /// the compact point-list / repetition forms.
    #[test]
    fn reads_foreign_constructs() {
        let tech = Tech::generic();
        let m1 = tech.layer(6).unwrap();
        let (l1, d1) = (u64::from(m1.gds_layer), u64::from(m1.gds_datatype));
        let mut w = W(MAGIC.to_vec());
        w.uint(START);
        w.string(b"1.0");
        w.uint(4); // unit 2000/1 grid steps per µm: coordinates halve
        w.uint(2000);
        w.uint(1);
        w.uint(1);
        w.uint(CELL_REF);
        w.uint(0);
        w.uint(XYRELATIVE);
        // Two rectangles through a uniform x repetition, positions relative.
        w.uint(RECTANGLE);
        w.byte(0x7F);
        w.uint(l1);
        w.uint(d1);
        w.uint(20);
        w.uint(10);
        w.sint(100);
        w.sint(100);
        w.uint(2);
        w.uint(0);
        w.uint(50);
        // Square reusing layer/datatype, x relative to the previous rectangle.
        w.uint(RECTANGLE);
        w.byte(0xD0);
        w.uint(8);
        w.sint(-100);
        // L-shaped polygon from a 1-delta list with the implied closing vertex.
        w.uint(XYABSOLUTE);
        w.uint(POLYGON);
        w.byte(0x38);
        w.uint(0);
        w.uint(4);
        w.sint(40);
        w.sint(20);
        w.sint(-20);
        w.sint(20);
        w.sint(0);
        w.sint(0);
        // Polygon from a 3-delta list, and a double-delta path with a flush start.
        w.uint(POLYGON);
        w.byte(0x38);
        w.uint(3);
        w.uint(2);
        w.uint((10 << 3) | 4);
        w.uint((20 << 3) | 2);
        w.sint(0);
        w.sint(0);
        w.uint(PATH);
        w.byte(0xF8);
        w.uint(10);
        w.uint(0b0110);
        w.uint(5);
        w.uint(2);
        w.gdelta((100, 0));
        w.gdelta((-100, 100));
        w.sint(0);
        w.sint(0);
        // Text by TEXTSTRING reference, repeated by an arbitrary x list.
        w.uint(TEXT);
        w.byte(0x7F);
        w.uint(0);
        w.uint(l1);
        w.uint(d1);
        w.sint(2);
        w.sint(4);
        w.uint(4);
        w.uint(0);
        w.uint(30);
        // Horizontal trapezoid.
        w.uint(TRAPEZOID);
        w.byte(0x78);
        w.uint(40);
        w.uint(20);
        w.sint(10);
        w.sint(-10);
        w.sint(0);
        w.sint(0);
        // Constrained trapezoids: type 0 sets the modal type, the second reuses it with a new
        // width; types 20 and 25 take their implied dimension from the other one.
        w.uint(CTRAPEZOID);
        w.byte(0xF8);
        w.uint(0);
        w.uint(40);
        w.uint(20);
        w.sint(0);
        w.sint(0);
        w.uint(CTRAPEZOID);
        w.byte(0x40);
        w.uint(60);
        w.uint(CTRAPEZOID);
        w.byte(0xA0);
        w.uint(20);
        w.uint(10);
        w.uint(CTRAPEZOID);
        w.byte(0xC0);
        w.uint(25);
        w.uint(8);
        w.uint(CELL);
        w.string(b"top");
        // Explicit name with magnification and odd angle, then a list repetition by refnum.
        w.uint(PLACEMENT_MAG);
        w.byte(0x80 | 0x20 | 0x10 | 0x08 | 0x04 | 0x01);
        w.string(b"leaf");
        w.real(2.0);
        w.real(45.0);
        w.sint(10);
        w.sint(20);
        w.uint(PLACEMENT);
        w.byte(0xC0 | 0x20 | 0x08 | 0x02);
        w.uint(0);
        w.sint(0);
        w.uint(10);
        w.uint(0);
        w.gdelta((4, 6));
        w.uint(PROPERTY);
        w.byte(0x20 | 0x04 | 0x02);
        w.uint(0);
        w.uint(8);
        w.uint(61);
        w.uint(13);
        w.uint(0);
        // Name tables after their use.
        w.uint(CELLNAME);
        w.string(b"leaf");
        w.uint(5);
        w.string(b"vdd");
        w.uint(7);
        w.string(S_GDS_PROPERTY.as_bytes());
        w.uint(9);
        w.string(b"XA");
        w.uint(END);
        w.string(&[0; 252]);
        w.uint(0);

        let imp = read_oasis(&w.0, &tech, "lib").unwrap();
        let names: Vec<&str> = imp.cells.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["leaf", "top"]);
        let g: Vec<&Geometry> = imp.cells[0].1.shapes.iter().map(|s| &s.geom).collect();
        assert_eq!(g[0], &Geometry::Rect(IRect::new(50, 50, 60, 55)));
        assert_eq!(g[1], &Geometry::Rect(IRect::new(75, 50, 85, 55)));
        assert_eq!(g[2], &Geometry::Rect(IRect::new(0, 50, 4, 54)), "square at the modal y, relative x");
        assert_eq!(g[3], &Geometry::Polygon { points: vec![(0, 0), (20, 0), (20, 10), (10, 10), (10, 20), (0, 20)] });
        assert_eq!(g[4], &Geometry::Polygon { points: vec![(0, 0), (5, 5), (-5, 5)] });
        assert_eq!(g[5], &Geometry::Path { points: vec![(5, 0), (50, 0), (50, 50)], width: 10 }, "flush start pulled in");
        assert_eq!(g[6], &Geometry::Label { text: "vdd".into(), at: (1, 2), size: 250 });
        assert_eq!(g[7], &Geometry::Label { text: "vdd".into(), at: (16, 2), size: 250 });
        assert_eq!(g[8], &Geometry::Polygon { points: vec![(5, 10), (15, 10), (20, 0), (0, 0)] });
        assert_eq!(g[9], &Geometry::Polygon { points: vec![(0, 0), (0, 10), (10, 10), (20, 0)] });
        assert_eq!(g[10], &Geometry::Polygon { points: vec![(0, 0), (0, 10), (20, 10), (30, 0)] });
        assert_eq!(g[11], &Geometry::Polygon { points: vec![(0, 0), (5, 5), (10, 0)] });
        assert_eq!(g[12], &Geometry::Rect(IRect::new(0, 0, 4, 4)));
        assert_eq!(g.len(), 13);
        let insts = &imp.cells[1].1.instances;
        assert_eq!(insts.len(), 3);
        assert!(insts.iter().all(|i| i.cell == CellRef::new("lib", "leaf")));
        assert_eq!(insts[0].trans, Trans::with(5, 10, 1, true));
        assert_eq!((insts[1].trans, insts[2].trans), (Trans::with(0, 10, 1, false), Trans::with(2, 13, 1, false)));
        let names: Vec<Option<&str>> = insts.iter().map(|i| i.name.as_deref()).collect();
        assert_eq!(names, [None, Some("XA"), Some("XA")]);
        assert_eq!(imp.warnings.len(), 2, "{:?}", imp.warnings);
    }

    #[test]
    fn ctrapezoid_types_are_simple_polygons() {
        let area = |p: &[(i64, i64)]| -> i64 {
            let n = p.len();
            (0..n).map(|k| p[k].0 * p[(k + 1) % n].1 - p[(k + 1) % n].0 * p[k].1).sum::<i64>().abs() / 2
        };
        for t in 0..26 {
            let (w, h) = if (8..16).contains(&t) { (20, 60) } else { (60, 20) };
            let p = ctrapezoid(t, w, h).unwrap();
            let expected = match t {
                0..=3 => w * h - h * h / 2,
                4..=7 => w * h - h * h,
                8..=11 => w * h - w * w / 2,
                12..=15 => w * h - w * w,
                16..=19 => w * w / 2,
                20 | 21 => h * h,
                22 | 23 => w * w,
                24 => w * h,
                _ => w * w,
            };
            assert_eq!(area(&p), expected, "type {t}: {p:?}");
            let (x0, y0) = (p.iter().map(|q| q.0).min().unwrap(), p.iter().map(|q| q.1).min().unwrap());
            assert_eq!((x0, y0), (0, 0), "type {t} starts at the origin");
        }
        assert!(ctrapezoid(26, 1, 1).is_err());
    }

    #[test]
    fn format_detection() {
        use std::path::Path;
        assert_eq!(StreamFormat::from_path(Path::new("a/b.OAS")), StreamFormat::Oasis);
        assert_eq!(StreamFormat::from_path(Path::new("b.oasis")), StreamFormat::Oasis);
        assert_eq!(StreamFormat::from_path(Path::new("b.gds")), StreamFormat::Gds);
        assert_eq!(StreamFormat::detect(MAGIC), StreamFormat::Oasis);
        assert_eq!(StreamFormat::detect(&[0, 6, 0, 2]), StreamFormat::Gds);
        assert!(read_oasis(&MAGIC[..5], &Tech::generic(), "x").is_err());
        let mut truncated = write_oasis(&[], &Tech::generic());
        truncated.truncate(truncated.len() - 256);
        assert!(read_oasis(&truncated, &Tech::generic(), "x").unwrap_err().contains("END"));
    }
}
