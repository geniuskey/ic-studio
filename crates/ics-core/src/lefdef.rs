//! LEF / DEF: abstract (LEF macro) generation from layout cells, a LEF writer with a minimal
//! technology header and reader for macros, and a DEF writer and reader for placed top cells.
//!
//! Layout coordinates are database units; LEF values are microns and DEF values are integers
//! in `UNITS DISTANCE MICRONS` per micron. A macro's geometry is written in its LEF frame, where
//! the lower-left of its box is (0, 0) and `ORIGIN` is where the cell origin lies:
//! `lef = cell + origin`.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::extract::{Extraction, extract};
use crate::gds::{GdsImport, sanitize_name, unique_name};
use crate::geom::IRect;
use crate::hier::{Hier, LayoutSource, flatten};
use crate::layout::{Geometry, Layout, Trans};
use crate::project::{Cell, CellRef, Project};
use crate::region::{BoolOp, Region};
use crate::tech::Tech;

#[derive(Debug, Clone, PartialEq)]
pub struct LefShape {
    pub layer: String,
    /// Rect, Polygon or Path, in the macro's LEF frame (dbu).
    pub geom: Geometry,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LefPin {
    pub name: String,
    /// INPUT, OUTPUT, INOUT (empty = not given).
    pub direction: String,
    /// SIGNAL, POWER, GROUND, ... (empty = not given).
    pub usage: String,
    pub ports: Vec<LefShape>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LefMacro {
    pub name: String,
    pub class: String,
    /// Position of the cell origin in the LEF frame (dbu).
    pub origin: (i64, i64),
    pub size: (i64, i64),
    pub symmetry: String,
    pub site: String,
    pub pins: Vec<LefPin>,
    pub obs: Vec<LefShape>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LefLibrary {
    /// `UNITS DATABASE MICRONS` of the file.
    pub units: Option<i64>,
    /// Layer names with their TYPE.
    pub layers: Vec<(String, String)>,
    pub macros: Vec<LefMacro>,
}

/// What the abstract keeps of metal that is not a pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ObsMode {
    /// The merged shapes, per layer.
    #[default]
    Detailed,
    /// Per layer, the extent of the layer's metal minus the pins (grown by the layer spacing).
    Block,
    None,
}

impl std::str::FromStr for ObsMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "detailed" => Ok(ObsMode::Detailed),
            "block" => Ok(ObsMode::Block),
            "none" => Ok(ObsMode::None),
            _ => Err(format!("unknown OBS mode '{s}' (detailed, block or none)")),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AbstractOptions {
    pub obs: ObsMode,
    /// Macro CLASS, e.g. BLOCK or CORE.
    pub class: String,
}

impl Default for AbstractOptions {
    fn default() -> Self {
        AbstractOptions { obs: ObsMode::Detailed, class: "BLOCK".into() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LefLayerKind {
    Routing,
    Cut,
}

/// The layers a LEF describes, bottom-up: routing (metal) layers and the via cuts between them.
/// Poly and diffusion are left out.
pub fn lef_layers(tech: &Tech) -> Vec<(usize, LefLayerKind)> {
    let excluded: Vec<usize> = tech.poly().into_iter().chain(tech.diffusion_layers()).collect();
    let routing: Vec<usize> =
        tech.layers.iter().enumerate().filter(|(i, l)| l.routing && !excluded.contains(i)).map(|(i, _)| i).collect();
    let via_up = |b: usize| {
        tech.vias.iter().find_map(|v| {
            let (vb, c, t) = (tech.layer_index(&v.bottom)?, tech.layer_index(&v.cut)?, tech.layer_index(&v.top)?);
            (vb == b && routing.contains(&t)).then_some((c, t))
        })
    };
    let tops: HashSet<usize> = routing.iter().filter_map(|&b| via_up(b).map(|(_, t)| t)).collect();
    let mut out: Vec<(usize, LefLayerKind)> = vec![];
    let seen = |out: &[(usize, LefLayerKind)], l: usize| out.iter().any(|&(x, _)| x == l);
    for &start in routing.iter().filter(|l| !tops.contains(l)).chain(&routing) {
        let mut cur = start;
        while !seen(&out, cur) {
            out.push((cur, LefLayerKind::Routing));
            let Some((cut, top)) = via_up(cur) else { break };
            if !seen(&out, cut) {
                out.push((cut, LefLayerKind::Cut));
            }
            cur = top;
        }
    }
    out
}

/// Canonical form of a layer name for matching: `M1`, `met1` and `metal1` are all `metal1`,
/// `V1` and `via1` are `via1`.
fn layer_key(name: &str) -> String {
    let s = name.trim().to_ascii_lowercase();
    for (prefix, canon) in [("metal", "metal"), ("met", "metal"), ("m", "metal"), ("via", "via"), ("v", "via")] {
        if let Some(n) = s.strip_prefix(prefix)
            && !n.is_empty()
            && n.chars().all(|c| c.is_ascii_digit())
        {
            let n = n.trim_start_matches('0');
            return format!("{canon}{}", if n.is_empty() { "0" } else { n });
        }
    }
    s
}

/// Technology layer for a LEF / DEF layer name: same name (ignoring case), else the same
/// canonical metal / via name.
pub fn map_layer(tech: &Tech, name: &str) -> Option<usize> {
    tech.layer_index(name).or_else(|| {
        let k = layer_key(name);
        tech.layers.iter().position(|l| layer_key(&l.name) == k)
    })
}

/// LEF USE of a supply net name: POWER for vdd-like names, GROUND for vss / gnd-like names.
pub fn supply_use(name: &str) -> Option<&'static str> {
    let n = name.trim().trim_end_matches('!').to_ascii_lowercase();
    let power = ["vpwr", "avdd", "dvdd", "pwr", "power", "vpb"];
    let ground = ["vgnd", "agnd", "dgnd", "avss", "dvss", "ground", "vnb", "0"];
    if power.contains(&n.as_str()) || n.starts_with("vdd") || n.starts_with("vcc") {
        Some("POWER")
    } else if ground.contains(&n.as_str()) || n.starts_with("vss") || n.starts_with("gnd") {
        Some("GROUND")
    } else {
        None
    }
}

fn boundary_layer(tech: &Tech) -> Option<usize> {
    ["boundary", "prboundary", "prbndry", "prbound"].iter().find_map(|n| tech.layer_index(n))
}

/// The macro box of a cell: its boundary-layer shapes' extent, else its full hierarchical extent.
pub fn macro_box(layout: &Layout, src: &dyn LayoutSource, tech: &Tech) -> Option<IRect> {
    let own = boundary_layer(tech).and_then(|b| {
        layout
            .shapes
            .iter()
            .filter(|s| s.layer == b && !matches!(s.geom, Geometry::Label { .. }))
            .map(|s| s.bbox())
            .reduce(|a, b| a.union(&b))
    });
    own.or_else(|| Hier::default().get_for(&CellRef::new("", ""), layout, src).bbox)
}

/// A labelled net of a cell: its geometry on the LEF routing layers (cell coordinates).
struct PinNet {
    name: String,
    direction: &'static str,
    usage: &'static str,
    shapes: Vec<(usize, Region)>,
}

fn pin_nets(x: &Extraction, metal: &[usize]) -> Vec<PinNet> {
    let gates: HashSet<usize> = x.devices.iter().map(|d| d.g).collect();
    let sd: HashSet<usize> = x.devices.iter().flat_map(|d| [d.s, d.d]).collect();
    type Acc = (bool, bool, BTreeMap<usize, Vec<IRect>>);
    let mut by_name: BTreeMap<&str, Acc> = BTreeMap::new();
    for (i, n) in x.nets.iter().enumerate().filter(|(_, n)| n.labelled) {
        let e = by_name.entry(n.name.as_str()).or_default();
        e.0 |= gates.contains(&i);
        e.1 |= sd.contains(&i);
        for (l, r) in n.shapes.iter().filter(|(l, _)| metal.contains(l)) {
            e.2.entry(*l).or_default().extend_from_slice(r.rects());
        }
    }
    by_name
        .into_iter()
        .map(|(name, (gate, sd, shapes))| {
            let usage = supply_use(name);
            let direction = match (usage, gate, sd) {
                (Some(_), _, _) => "INOUT",
                (None, true, false) => "INPUT",
                (None, false, true) => "OUTPUT",
                _ => "INOUT",
            };
            let mut shapes: Vec<(usize, Region)> = shapes.into_iter().map(|(l, rs)| (l, Region::from_rects(rs))).collect();
            shapes.sort_by_key(|(l, _)| metal.iter().position(|m| m == l));
            PinNet { name: name.to_string(), direction, usage: usage.unwrap_or("SIGNAL"), shapes }
        })
        .collect()
}

/// Generate the LEF abstract of a cell: pins from its labelled nets (ports are the net's shapes
/// on the routing layers), OBS from the other metal and the cuts between routing layers.
pub fn generate_abstract(
    name: &str,
    layout: &Layout,
    src: &dyn LayoutSource,
    tech: &Tech,
    opts: &AbstractOptions,
) -> (LefMacro, Vec<String>) {
    let x = extract(layout, src, tech);
    let mut warnings = x.warnings.clone();
    let layers = lef_layers(tech);
    let metal: Vec<usize> = layers.iter().filter(|(_, k)| *k == LefLayerKind::Routing).map(|&(l, _)| l).collect();
    let bx = macro_box(layout, src, tech).unwrap_or_default();
    let origin = (-bx.x0, -bx.y0);
    let lname = |l: usize| tech.layers[l].name.clone();
    let shapes_of = |l: usize, r: &Region| -> Vec<LefShape> {
        r.translated(origin.0, origin.1).to_geometry().into_iter().map(|geom| LefShape { layer: lname(l), geom }).collect()
    };

    let nets = pin_nets(&x, &metal);
    let mut pin_rects: HashMap<usize, Vec<IRect>> = HashMap::new();
    let pins = nets
        .iter()
        .map(|n| {
            if n.shapes.is_empty() {
                warnings.push(format!("pin '{}' has no shapes on routing layers", n.name));
            }
            for (l, r) in &n.shapes {
                pin_rects.entry(*l).or_default().extend_from_slice(r.rects());
            }
            LefPin {
                name: n.name.clone(),
                direction: n.direction.into(),
                usage: n.usage.into(),
                ports: n.shapes.iter().flat_map(|(l, r)| shapes_of(*l, r)).collect(),
            }
        })
        .collect();

    let mut obs = vec![];
    if opts.obs != ObsMode::None {
        let mut other: HashMap<usize, Vec<IRect>> = HashMap::new();
        for n in x.nets.iter().filter(|n| !n.labelled) {
            for (l, r) in n.shapes.iter().filter(|(l, _)| metal.contains(l)) {
                other.entry(*l).or_default().extend_from_slice(r.rects());
            }
        }
        let cuts: Vec<usize> = layers.iter().filter(|(_, k)| *k == LefLayerKind::Cut).map(|&(l, _)| l).collect();
        if !cuts.is_empty() {
            for s in flatten(layout, src, 64).into_iter().filter(|s| cuts.contains(&s.layer)) {
                if let Ok(r) = Region::from_geometry(&s.geom) {
                    other.entry(s.layer).or_default().extend_from_slice(r.rects());
                }
            }
        }
        for &(l, _) in &layers {
            let Some(rs) = other.get(&l).filter(|rs| !rs.is_empty()) else { continue };
            let region = Region::from_rects(rs.iter().copied());
            let region = match opts.obs {
                ObsMode::Block => {
                    let pins = Region::from_rects(pin_rects.get(&l).into_iter().flatten().copied());
                    let all = region.boolean(&pins, BoolOp::Or).bbox().unwrap_or_default();
                    Region::from_rects([all]).boolean(&pins.sized(tech.layers[l].min_spacing.max(0)), BoolOp::Not)
                }
                _ => region,
            };
            obs.extend(shapes_of(l, &region));
        }
    }
    let m = LefMacro {
        name: name.to_string(),
        class: opts.class.clone(),
        origin,
        size: (bx.width(), bx.height()),
        pins,
        obs,
        ..LefMacro::default()
    };
    (m, warnings)
}

fn um(v: i64, dbu_per_um: i64) -> String {
    um_f(v as f64 / dbu_per_um.max(1) as f64)
}

fn um_f(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn write_shapes(out: &mut String, shapes: &[LefShape], indent: &str, dbu: i64) {
    let mut cur: Option<&str> = None;
    let mut width: Option<i64> = None;
    for s in shapes {
        if cur != Some(s.layer.as_str()) {
            out.push_str(&format!("{indent}LAYER {} ;\n", s.layer));
            cur = Some(&s.layer);
            width = None;
        }
        let pts =
            |p: &[(i64, i64)]| p.iter().map(|&(x, y)| format!("{} {}", um(x, dbu), um(y, dbu))).collect::<Vec<_>>().join(" ");
        match &s.geom {
            Geometry::Rect(r) => {
                out.push_str(&format!("{indent}  RECT {} ;\n", pts(&[(r.x0, r.y0), (r.x1, r.y1)])));
            }
            Geometry::Polygon { points } => out.push_str(&format!("{indent}  POLYGON {} ;\n", pts(points))),
            Geometry::Path { points, width: w } => {
                if width != Some(*w) {
                    out.push_str(&format!("{indent}  WIDTH {} ;\n", um(*w, dbu)));
                    width = Some(*w);
                }
                out.push_str(&format!("{indent}  PATH {} ;\n", pts(points)));
            }
            Geometry::Label { .. } => {}
        }
    }
}

/// Write macros as LEF; `with_tech` adds the technology header (units, grid, routing and cut
/// layers, default vias).
pub fn write_lef(tech: &Tech, macros: &[LefMacro], with_tech: bool) -> String {
    let dbu = tech.dbu_per_um.max(1);
    let mut s = String::from("VERSION 5.8 ;\nBUSBITCHARS \"[]\" ;\nDIVIDERCHAR \"/\" ;\n\n");
    if with_tech {
        s += &format!("UNITS\n  DATABASE MICRONS {dbu} ;\nEND UNITS\n\n");
        if tech.grid > 0 {
            s += &format!("MANUFACTURINGGRID {} ;\n\n", um(tech.grid, dbu));
        }
        let layers = lef_layers(tech);
        let mut nth_metal = 0;
        for &(l, kind) in &layers {
            let ld = &tech.layers[l];
            s += &format!("LAYER {}\n", ld.name);
            match kind {
                LefLayerKind::Routing => {
                    s += "  TYPE ROUTING ;\n";
                    s += &format!("  DIRECTION {} ;\n", if nth_metal % 2 == 0 { "HORIZONTAL" } else { "VERTICAL" });
                    nth_metal += 1;
                    if ld.min_width > 0 && ld.min_spacing > 0 {
                        s += &format!("  PITCH {} ;\n", um(ld.min_width + ld.min_spacing, dbu));
                    }
                    if ld.min_width > 0 {
                        s += &format!("  WIDTH {} ;\n", um(ld.min_width, dbu));
                    }
                    if ld.min_spacing > 0 {
                        s += &format!("  SPACING {} ;\n", um(ld.min_spacing, dbu));
                    }
                    if ld.min_area > 0 {
                        s += &format!("  AREA {} ;\n", um_f(ld.min_area as f64 / (dbu * dbu) as f64));
                    }
                }
                LefLayerKind::Cut => {
                    s += "  TYPE CUT ;\n";
                    let via = tech.vias.iter().find(|v| tech.layer_index(&v.cut) == Some(l));
                    let (w, sp) = via.map_or((ld.min_width, ld.min_spacing), |v| (v.cut_size, v.cut_spacing));
                    if w > 0 {
                        s += &format!("  WIDTH {} ;\n", um(w, dbu));
                    }
                    if sp > 0 {
                        s += &format!("  SPACING {} ;\n", um(sp, dbu));
                    }
                }
            }
            s += &format!("END {}\n\n", ld.name);
        }
        let has = |n: &str| tech.layer_index(n).is_some_and(|i| layers.iter().any(|&(l, _)| l == i));
        for v in tech.vias.iter().filter(|v| has(&v.bottom) && has(&v.cut) && has(&v.top) && v.cut_size > 0) {
            let half = v.cut_size as f64 / 2.0 / dbu as f64;
            let rect = |e: i64| {
                let h = half + e as f64 / dbu as f64;
                format!("RECT {} {} {} {} ;", um_f(-h), um_f(-h), um_f(h), um_f(h))
            };
            s += &format!("VIA {} DEFAULT\n", v.name);
            s += &format!("  LAYER {} ;\n    {}\n", v.bottom, rect(v.enc_bottom));
            s += &format!("  LAYER {} ;\n    {}\n", v.cut, rect(0));
            s += &format!("  LAYER {} ;\n    {}\n", v.top, rect(v.enc_top));
            s += &format!("END {}\n\n", v.name);
        }
    }
    for m in macros {
        s += &format!("MACRO {}\n", m.name);
        if !m.class.is_empty() {
            s += &format!("  CLASS {} ;\n", m.class);
        }
        s += &format!("  ORIGIN {} {} ;\n", um(m.origin.0, dbu), um(m.origin.1, dbu));
        s += &format!("  SIZE {} BY {} ;\n", um(m.size.0, dbu), um(m.size.1, dbu));
        if !m.symmetry.is_empty() {
            s += &format!("  SYMMETRY {} ;\n", m.symmetry);
        }
        if !m.site.is_empty() {
            s += &format!("  SITE {} ;\n", m.site);
        }
        for p in &m.pins {
            s += &format!("  PIN {}\n", p.name);
            if !p.direction.is_empty() {
                s += &format!("    DIRECTION {} ;\n", p.direction);
            }
            if !p.usage.is_empty() {
                s += &format!("    USE {} ;\n", p.usage);
            }
            if !p.ports.is_empty() {
                s += "    PORT\n";
                write_shapes(&mut s, &p.ports, "      ", dbu);
                s += "    END\n";
            }
            s += &format!("  END {}\n", p.name);
        }
        if !m.obs.is_empty() {
            s += "  OBS\n";
            write_shapes(&mut s, &m.obs, "    ", dbu);
            s += "  END\n";
        }
        s += &format!("END {}\n\n", m.name);
    }
    s += "END LIBRARY\n";
    s
}

/// Whitespace separated tokens, with `;`, `(` and `)` as tokens of their own, `#` comments
/// removed and quoted strings kept whole.
struct Toks {
    t: Vec<String>,
    i: usize,
}

impl Toks {
    fn new(text: &str) -> Toks {
        let mut t = vec![];
        for line in text.lines() {
            let mut cur = String::new();
            let mut quoted = false;
            for c in line.chars() {
                if quoted {
                    cur.push(c);
                    quoted = c != '"';
                    continue;
                }
                match c {
                    '"' => {
                        cur.push(c);
                        quoted = true;
                    }
                    '#' if cur.is_empty() => break,
                    ';' | '(' | ')' => {
                        if !cur.is_empty() {
                            t.push(std::mem::take(&mut cur));
                        }
                        t.push(c.to_string());
                    }
                    c if c.is_whitespace() => {
                        if !cur.is_empty() {
                            t.push(std::mem::take(&mut cur));
                        }
                    }
                    c => cur.push(c),
                }
            }
            if !cur.is_empty() {
                t.push(cur);
            }
        }
        Toks { t, i: 0 }
    }

    fn next(&mut self) -> Option<String> {
        let s = self.t.get(self.i).cloned();
        self.i += 1;
        s
    }

    fn peek(&self) -> Option<&str> {
        self.t.get(self.i).map(String::as_str)
    }

    fn word(&mut self) -> Result<String, String> {
        self.next().ok_or_else(|| "unexpected end of file".to_string())
    }

    /// Tokens up to the next `;` (consumed).
    fn stmt(&mut self) -> Vec<String> {
        let mut out = vec![];
        while let Some(s) = self.next() {
            if s == ";" {
                break;
            }
            out.push(s);
        }
        out
    }

    /// Skip to just after `END <name>`.
    fn skip_block(&mut self, name: &str) {
        while let Some(s) = self.next() {
            if s.eq_ignore_ascii_case("END") && self.peek().is_some_and(|n| n.eq_ignore_ascii_case(name)) {
                self.i += 1;
                return;
            }
        }
    }
}

fn to_dbu(v: f64, dbu_per_um: f64) -> i64 {
    (v * dbu_per_um).round() as i64
}

fn parse_nums(words: &[String], dbu_per_um: f64) -> Result<Vec<i64>, String> {
    words
        .iter()
        .map(|w| w.parse::<f64>().map(|v| to_dbu(v, dbu_per_um)).map_err(|_| format!("expected a number, found '{w}'")))
        .collect()
}

/// Shapes of a PORT or OBS section up to its `END`.
fn read_shapes(t: &mut Toks, dbu_per_um: f64) -> Result<Vec<LefShape>, String> {
    let mut out = vec![];
    let mut layer: Option<String> = None;
    let mut width = 0;
    loop {
        let k = t.word()?.to_ascii_uppercase();
        match k.as_str() {
            "END" => return Ok(out),
            "LAYER" => layer = t.stmt().into_iter().next(),
            "WIDTH" => width = t.stmt().first().and_then(|w| w.parse::<f64>().ok()).map_or(0, |w| to_dbu(w, dbu_per_um)),
            "RECT" | "POLYGON" | "PATH" => {
                let mut words = t.stmt();
                if words.first().is_some_and(|w| w.eq_ignore_ascii_case("MASK")) {
                    words.drain(..2.min(words.len()));
                }
                if words.first().is_some_and(|w| w.eq_ignore_ascii_case("ITERATE")) {
                    continue;
                }
                let v = parse_nums(&words, dbu_per_um)?;
                let pts: Vec<(i64, i64)> = v.as_chunks::<2>().0.iter().map(|&[x, y]| (x, y)).collect();
                let geom = match k.as_str() {
                    "RECT" if pts.len() == 2 => Geometry::Rect(IRect::new(pts[0].0, pts[0].1, pts[1].0, pts[1].1)),
                    "POLYGON" if pts.len() >= 3 => Geometry::Polygon { points: pts },
                    "PATH" if !pts.is_empty() && width > 0 => Geometry::Path { points: pts, width },
                    _ => return Err(format!("malformed {k} statement")),
                };
                if let Some(l) = &layer {
                    out.push(LefShape { layer: l.clone(), geom });
                }
            }
            _ => {
                t.stmt();
            }
        }
    }
}

fn read_macro(t: &mut Toks, name: String, dbu_per_um: f64) -> Result<LefMacro, String> {
    let mut m = LefMacro { name, ..LefMacro::default() };
    loop {
        let k = t.word().map_err(|e| format!("MACRO {}: {e}", m.name))?.to_ascii_uppercase();
        match k.as_str() {
            "END" => {
                t.next();
                return Ok(m);
            }
            "CLASS" => m.class = t.stmt().join(" "),
            "SYMMETRY" => m.symmetry = t.stmt().join(" "),
            "SITE" => m.site = t.stmt().into_iter().next().unwrap_or_default(),
            "ORIGIN" => {
                let v = parse_nums(&t.stmt(), dbu_per_um)?;
                if v.len() >= 2 {
                    m.origin = (v[0], v[1]);
                }
            }
            "SIZE" => {
                let w = t.stmt();
                let v = parse_nums(&w.into_iter().filter(|w| !w.eq_ignore_ascii_case("BY")).collect::<Vec<_>>(), dbu_per_um)?;
                if v.len() >= 2 {
                    m.size = (v[0], v[1]);
                }
            }
            "PIN" => {
                let mut p = LefPin { name: t.word()?, direction: String::new(), usage: String::new(), ports: vec![] };
                loop {
                    let k = t.word()?.to_ascii_uppercase();
                    match k.as_str() {
                        "END" => {
                            t.next();
                            break;
                        }
                        "DIRECTION" => p.direction = t.stmt().join(" "),
                        "USE" => p.usage = t.stmt().join(" "),
                        "PORT" => p.ports.extend(read_shapes(t, dbu_per_um)?),
                        _ => {
                            t.stmt();
                        }
                    }
                }
                m.pins.push(p);
            }
            "OBS" => m.obs.extend(read_shapes(t, dbu_per_um)?),
            "DENSITY" => {
                read_shapes(t, dbu_per_um)?;
            }
            _ => {
                t.stmt();
            }
        }
    }
}

/// Parse a LEF file; coordinates are converted to dbu at `dbu_per_um`.
pub fn read_lef(text: &str, dbu_per_um: i64) -> Result<LefLibrary, String> {
    let scale = dbu_per_um.max(1) as f64;
    let mut t = Toks::new(text);
    let mut lib = LefLibrary::default();
    while let Some(k) = t.next() {
        match k.to_ascii_uppercase().as_str() {
            "MACRO" => {
                let name = t.word()?;
                lib.macros.push(read_macro(&mut t, name, scale)?);
            }
            "LAYER" => {
                let name = t.word()?;
                let mut kind = String::new();
                loop {
                    let s = t.word()?;
                    if s.eq_ignore_ascii_case("END") && t.peek().is_some_and(|n| n == name) {
                        t.next();
                        break;
                    }
                    let st = t.stmt();
                    if s.eq_ignore_ascii_case("TYPE") {
                        kind = st.into_iter().next().unwrap_or_default().to_ascii_uppercase();
                    }
                }
                lib.layers.push((name, kind));
            }
            "UNITS" => loop {
                let s = t.word()?;
                if s.eq_ignore_ascii_case("END") {
                    t.next();
                    break;
                }
                let st = t.stmt();
                if s.eq_ignore_ascii_case("DATABASE") {
                    lib.units = st.get(1).and_then(|v| v.parse::<f64>().ok()).map(|v| v as i64);
                }
            },
            "VIA" | "VIARULE" | "SITE" | "NONDEFAULTRULE" => {
                let name = t.word()?;
                t.skip_block(&name);
            }
            "PROPERTYDEFINITIONS" | "SPACING" | "IRDROP" | "NOISETABLE" | "CORRECTIONTABLE" | "ARRAY" => t.skip_block(&k),
            "BEGINEXT" => while t.next().is_some_and(|s| !s.eq_ignore_ascii_case("ENDEXT")) {},
            "END" => {
                if t.next().is_some_and(|s| s.eq_ignore_ascii_case("LIBRARY")) {
                    break;
                }
            }
            _ => {
                t.stmt();
            }
        }
    }
    Ok(lib)
}

fn probe(g: &Geometry) -> Option<(i64, i64)> {
    let r = *Region::from_geometry(g).ok()?.rects().first()?;
    Some(((r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2))
}

/// A layout cell for a LEF macro: its box on the boundary layer, the pin ports with a label of
/// the pin name on each pin's first port shape, and the obstructions.
pub fn macro_layout(m: &LefMacro, tech: &Tech) -> (Layout, Vec<String>) {
    let t = Trans::new(-m.origin.0, -m.origin.1);
    let mut lay = Layout::default();
    let mut unknown: BTreeMap<String, usize> = BTreeMap::new();
    let mut warnings = vec![];
    if let Some(b) = boundary_layer(tech)
        && m.size.0 > 0
        && m.size.1 > 0
    {
        lay.add(b, Geometry::Rect(t.apply_rect(IRect::new(0, 0, m.size.0, m.size.1))));
    }
    let size = (tech.dbu_per_um / 5).max(1);
    for p in &m.pins {
        let mut labelled = false;
        for s in &p.ports {
            let Some(l) = map_layer(tech, &s.layer) else {
                *unknown.entry(s.layer.clone()).or_default() += 1;
                continue;
            };
            let g = s.geom.transformed(&t);
            if !labelled && let (Some(at), Some(ll)) = (probe(&g), tech.net_label_layer(l)) {
                lay.add(ll, Geometry::Label { text: p.name.clone(), at, size });
                labelled = true;
            }
            lay.add(l, g);
        }
        if !labelled {
            warnings.push(format!("{}: pin '{}' has no port shapes on known layers", m.name, p.name));
        }
    }
    for s in &m.obs {
        match map_layer(tech, &s.layer) {
            Some(l) => {
                lay.add(l, s.geom.transformed(&t));
            }
            None => *unknown.entry(s.layer.clone()).or_default() += 1,
        }
    }
    for (l, n) in unknown {
        warnings.push(format!("{}: {n} shape(s) on layer '{l}' unknown to the technology were skipped", m.name));
    }
    (lay, warnings)
}

/// DEF orientation name of a placement's mirror / rotation.
pub fn orient_name(rot: u8, mirror: bool) -> &'static str {
    match (mirror, rot % 4) {
        (false, 0) => "N",
        (false, 1) => "W",
        (false, 2) => "S",
        (false, _) => "E",
        (true, 0) => "FS",
        (true, 1) => "FW",
        (true, 2) => "FN",
        (true, _) => "FE",
    }
}

/// Rotation and mirror of a DEF (or OpenAccess) orientation name.
pub fn parse_orient(s: &str) -> Option<(u8, bool)> {
    Some(match s.to_ascii_uppercase().as_str() {
        "N" | "R0" => (0, false),
        "W" | "R90" => (1, false),
        "S" | "R180" => (2, false),
        "E" | "R270" => (3, false),
        "FS" | "MX" => (0, true),
        "FW" | "MXR90" => (1, true),
        "FN" | "MY" => (2, true),
        "FE" | "MYR90" => (3, true),
        _ => return None,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct DefComponent {
    pub name: String,
    pub macro_name: String,
    /// Placement: (x, y) is the lower-left of the placed macro box, with the orientation's
    /// rotation and mirror. `None` when unplaced.
    pub place: Option<Trans>,
    pub fixed: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DefPin {
    pub name: String,
    pub net: String,
    pub direction: String,
    pub usage: String,
    /// Layer and rectangle relative to the placement point.
    pub shape: Option<(String, IRect)>,
    pub place: Option<Trans>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DefNet {
    pub name: String,
    /// (component, pin), with component `PIN` for an I/O pin.
    pub conns: Vec<(String, String)>,
    pub usage: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DefDesign {
    pub name: String,
    pub die: Option<IRect>,
    pub components: Vec<DefComponent>,
    pub pins: Vec<DefPin>,
    pub nets: Vec<DefNet>,
}

fn def_name(s: &str) -> String {
    let s: String = s.trim().chars().map(|c| if c.is_whitespace() || matches!(c, ';' | '(' | ')') { '_' } else { c }).collect();
    if s.is_empty() { "_".into() } else { s }
}

/// The DEF of a top layout: die area from its box, components from its instances (array
/// elements become `name_i_j`), pins from its top-level labels and nets from the extracted
/// connectivity between the pins of the placed cells' abstracts.
pub fn def_from_layout(name: &str, top: &Layout, src: &dyn LayoutSource, tech: &Tech) -> (DefDesign, Vec<String>) {
    let mut warnings = vec![];
    let metal: Vec<usize> = lef_layers(tech).into_iter().filter(|(_, k)| *k == LefLayerKind::Routing).map(|(l, _)| l).collect();
    let mut d = DefDesign { name: def_name(name), die: macro_box(top, src, tech), ..DefDesign::default() };
    struct Child {
        bx: Option<IRect>,
        pins: Vec<(String, usize, (i64, i64))>,
    }
    let mut children: HashMap<CellRef, Child> = HashMap::new();
    let mut placed: Vec<(String, CellRef, Trans)> = vec![];
    let mut used: HashSet<String> = HashSet::new();
    for inst in &top.instances {
        let Some(child) = src.layout(&inst.cell) else {
            warnings.push(format!("instance of missing cell '{}' skipped", inst.cell));
            continue;
        };
        let c = children.entry(inst.cell.clone()).or_insert_with(|| {
            let x = extract(child, src, tech);
            let pins = pin_nets(&x, &metal)
                .into_iter()
                .filter_map(|p| {
                    let (l, r) = p.shapes.first()?;
                    let q = *r.rects().first()?;
                    Some((p.name, *l, ((q.x0 + q.x1) / 2, (q.y0 + q.y1) / 2)))
                })
                .collect();
            Child { bx: macro_box(child, src, tech), pins }
        });
        let Some(bx) = c.bx else {
            warnings.push(format!("instance of empty cell '{}' skipped", inst.cell));
            continue;
        };
        let base = def_name(inst.name.as_deref().unwrap_or(&format!("I{}", inst.id)));
        let (cols, rows) = inst.dims();
        for j in 0..rows {
            for i in 0..cols {
                let mut n = if inst.array.is_some() { format!("{base}_{i}_{j}") } else { base.clone() };
                while !used.insert(n.clone()) {
                    n += "_";
                }
                let t = inst.element_trans(i, j);
                let pb = t.apply_rect(bx);
                d.components.push(DefComponent {
                    name: n.clone(),
                    macro_name: def_name(&inst.cell.cell),
                    place: Some(Trans { x: pb.x0, y: pb.y0, ..t }),
                    fixed: false,
                });
                placed.push((n, inst.cell.clone(), t));
            }
        }
    }

    let x = extract(top, src, tech);
    let top_pins = pin_nets(&x, &metal);
    let mut nets: Vec<DefNet> = vec![];
    let mut net_idx: HashMap<String, usize> = HashMap::new();
    let mut conn = |net: &str, a: String, b: String| {
        let k = *net_idx.entry(net.to_string()).or_insert_with(|| {
            nets.push(DefNet { name: def_name(net), conns: vec![], usage: supply_use(net).unwrap_or("").into() });
            nets.len() - 1
        });
        nets[k].conns.push((a, b));
    };
    let mut seen_pins: HashSet<String> = HashSet::new();
    for s in &top.shapes {
        let Geometry::Label { text, at, .. } = &s.geom else { continue };
        let text = text.trim();
        let Some(pn) = top_pins.iter().find(|p| p.name == text) else { continue };
        if !seen_pins.insert(text.to_string()) {
            continue;
        }
        let shape = pn.shapes.iter().find_map(|(l, r)| {
            let q = r.rects().iter().find(|q| q.contains(at.0, at.1))?;
            Some((tech.layers[*l].name.clone(), q.translate(-at.0, -at.1)))
        });
        let name = def_name(text);
        d.pins.push(DefPin {
            name: name.clone(),
            net: name.clone(),
            direction: pn.direction.into(),
            usage: pn.usage.into(),
            place: shape.as_ref().map(|_| Trans::new(at.0, at.1)),
            shape,
        });
        conn(text, "PIN".into(), name);
    }
    for (comp, cell, t) in &placed {
        for (pin, l, p) in &children[cell].pins {
            match x.net_at(*l, t.apply(*p)) {
                Some(n) => conn(&x.nets[n].name, comp.clone(), def_name(pin)),
                None => warnings.push(format!("pin {comp}/{pin} is not on any net")),
            }
        }
    }
    d.nets = nets;
    (d, warnings)
}

pub fn write_def(d: &DefDesign, dbu_per_um: i64) -> String {
    let pt = |x: i64, y: i64| format!("( {x} {y} )");
    let mut s = String::from("VERSION 5.8 ;\nDIVIDERCHAR \"/\" ;\nBUSBITCHARS \"[]\" ;\n");
    s += &format!("DESIGN {} ;\nUNITS DISTANCE MICRONS {} ;\n\n", d.name, dbu_per_um.max(1));
    if let Some(r) = d.die {
        s += &format!("DIEAREA {} {} ;\n\n", pt(r.x0, r.y0), pt(r.x1, r.y1));
    }
    s += &format!("COMPONENTS {} ;\n", d.components.len());
    for c in &d.components {
        s += &format!("  - {} {}", c.name, c.macro_name);
        match c.place {
            Some(t) => {
                s += &format!(" + {} {} {}", if c.fixed { "FIXED" } else { "PLACED" }, pt(t.x, t.y), orient_name(t.rot, t.mirror))
            }
            None => s += " + UNPLACED",
        }
        s += " ;\n";
    }
    s += "END COMPONENTS\n\n";
    s += &format!("PINS {} ;\n", d.pins.len());
    for p in &d.pins {
        s += &format!("  - {} + NET {}", p.name, p.net);
        if !p.direction.is_empty() {
            s += &format!(" + DIRECTION {}", p.direction);
        }
        if !p.usage.is_empty() {
            s += &format!(" + USE {}", p.usage);
        }
        if let Some((l, r)) = &p.shape {
            s += &format!("\n    + LAYER {l} {} {}", pt(r.x0, r.y0), pt(r.x1, r.y1));
        }
        if let Some(t) = p.place {
            s += &format!("\n    + PLACED {} {}", pt(t.x, t.y), orient_name(t.rot, t.mirror));
        }
        s += " ;\n";
    }
    s += "END PINS\n\n";
    s += &format!("NETS {} ;\n", d.nets.len());
    for n in &d.nets {
        s += &format!("  - {}", n.name);
        for (c, p) in &n.conns {
            s += &format!(" ( {c} {p} )");
        }
        if !n.usage.is_empty() {
            s += &format!(" + USE {}", n.usage);
        }
        s += " ;\n";
    }
    s += "END NETS\n\nEND DESIGN\n";
    s
}

fn rec_point(r: &[String], i: &mut usize, scale: f64) -> Option<(i64, i64)> {
    while *i < r.len() && r[*i] != "(" {
        *i += 1;
    }
    let mut v = vec![];
    *i += 1;
    while *i < r.len() && r[*i] != ")" {
        v.push(r[*i].parse::<f64>().ok().map(|x| (x * scale).round() as i64));
        *i += 1;
    }
    *i += 1;
    match v.as_slice() {
        [Some(x), Some(y), ..] => Some((*x, *y)),
        _ => None,
    }
}

/// Parse a DEF file (design name, die area, components, pins and net connections);
/// coordinates are converted to dbu at `dbu_per_um`.
pub fn read_def(text: &str, dbu_per_um: i64) -> Result<DefDesign, String> {
    let mut t = Toks::new(text);
    let mut d = DefDesign::default();
    let mut scale = 1.0;
    let section = |t: &mut Toks, name: &str| -> Vec<Vec<String>> {
        t.stmt();
        let mut out = vec![];
        while let Some(s) = t.next() {
            if s == "-" {
                out.push(t.stmt());
            } else if s.eq_ignore_ascii_case("END") {
                if t.peek().is_some_and(|n| n.eq_ignore_ascii_case(name)) {
                    t.next();
                }
                break;
            }
        }
        out
    };
    let placement = |r: &[String], i: usize, scale: f64| -> Option<Trans> {
        let mut j = i;
        let (x, y) = rec_point(r, &mut j, scale)?;
        let (rot, mirror) = r.get(j).and_then(|o| parse_orient(o)).unwrap_or((0, false));
        Some(Trans { x, y, rot, mirror })
    };
    while let Some(k) = t.next() {
        match k.to_ascii_uppercase().as_str() {
            "DESIGN" => d.name = t.stmt().into_iter().next().unwrap_or_default(),
            "UNITS" => {
                let st = t.stmt();
                if let Some(u) = st.get(2).and_then(|v| v.parse::<f64>().ok()).filter(|u| *u > 0.0) {
                    scale = dbu_per_um.max(1) as f64 / u;
                }
            }
            "DIEAREA" => {
                let st = t.stmt();
                let mut i = 0;
                let mut pts = vec![];
                while let Some(p) = rec_point(&st, &mut i, scale) {
                    pts.push(p);
                }
                d.die = pts.iter().map(|&(x, y)| IRect::new(x, y, x, y)).reduce(|a, b| a.union(&b));
            }
            "COMPONENTS" => {
                for r in section(&mut t, "COMPONENTS") {
                    if r.len() < 2 {
                        continue;
                    }
                    let mut c = DefComponent { name: r[0].clone(), macro_name: r[1].clone(), place: None, fixed: false };
                    for i in 2..r.len() {
                        if r[i] != "+" {
                            continue;
                        }
                        let kw = r.get(i + 1).map(|s| s.to_ascii_uppercase()).unwrap_or_default();
                        if matches!(kw.as_str(), "PLACED" | "FIXED" | "COVER") {
                            c.place = placement(&r, i + 2, scale);
                            c.fixed = kw != "PLACED";
                        }
                    }
                    d.components.push(c);
                }
            }
            "PINS" => {
                for r in section(&mut t, "PINS") {
                    let Some(name) = r.first() else { continue };
                    let mut p = DefPin { name: name.clone(), ..DefPin::default() };
                    for i in 1..r.len() {
                        if r[i] != "+" {
                            continue;
                        }
                        let kw = r.get(i + 1).map(|s| s.to_ascii_uppercase()).unwrap_or_default();
                        let arg = r.get(i + 2).cloned().unwrap_or_default();
                        match kw.as_str() {
                            "NET" => p.net = arg,
                            "DIRECTION" => p.direction = arg.to_ascii_uppercase(),
                            "USE" => p.usage = arg.to_ascii_uppercase(),
                            "LAYER" if p.shape.is_none() => {
                                let mut j = i + 3;
                                if let (Some(a), Some(b)) = (rec_point(&r, &mut j, scale), rec_point(&r, &mut j, scale)) {
                                    p.shape = Some((arg, IRect::new(a.0, a.1, b.0, b.1)));
                                }
                            }
                            "PLACED" | "FIXED" | "COVER" if p.place.is_none() => p.place = placement(&r, i + 2, scale),
                            _ => {}
                        }
                    }
                    d.pins.push(p);
                }
            }
            "NETS" => {
                for r in section(&mut t, "NETS") {
                    let Some(name) = r.first() else { continue };
                    let mut n = DefNet { name: name.clone(), ..DefNet::default() };
                    let mut i = 1;
                    while i < r.len() && r[i] != "+" {
                        if r[i] == "(" && i + 2 < r.len() {
                            n.conns.push((r[i + 1].clone(), r[i + 2].clone()));
                            while i < r.len() && r[i] != ")" {
                                i += 1;
                            }
                        }
                        i += 1;
                    }
                    for j in i..r.len() {
                        if r[j] == "+" && r.get(j + 1).is_some_and(|k| k.eq_ignore_ascii_case("USE")) {
                            n.usage = r.get(j + 2).map(|u| u.to_ascii_uppercase()).unwrap_or_default();
                        }
                    }
                    d.nets.push(n);
                }
            }
            "VIAS"
            | "SPECIALNETS"
            | "REGIONS"
            | "GROUPS"
            | "BLOCKAGES"
            | "FILLS"
            | "SLOTS"
            | "STYLES"
            | "NONDEFAULTRULES"
            | "PROPERTYDEFINITIONS"
            | "SCANCHAINS"
            | "PINPROPERTIES"
            | "COMPONENTMASKSHIFT"
            | "EMULATECELL" => t.skip_block(&k),
            "BEGINEXT" => while t.next().is_some_and(|s| !s.eq_ignore_ascii_case("ENDEXT")) {},
            "END" => {
                if t.next().is_some_and(|s| s.eq_ignore_ascii_case("DESIGN")) {
                    break;
                }
            }
            _ => {
                t.stmt();
            }
        }
    }
    Ok(d)
}

#[derive(Debug, Clone, Default)]
pub struct DefLayout {
    pub layout: Layout,
    pub placed: usize,
    pub unplaced: usize,
    /// Macros no cell was found for.
    pub missing: Vec<String>,
    pub warnings: Vec<String>,
}

/// A top layout for a DEF design: instances of the placed components (`macro_of` gives a
/// macro's cell and box), pin shapes with a label of their net, and the die area on the
/// boundary layer.
pub fn def_layout(d: &DefDesign, tech: &Tech, mut macro_of: impl FnMut(&str) -> Option<(CellRef, IRect)>) -> DefLayout {
    let mut out = DefLayout::default();
    let lay = &mut out.layout;
    let mut missing: Vec<String> = vec![];
    for c in &d.components {
        let Some(pl) = c.place else {
            out.unplaced += 1;
            continue;
        };
        let Some((cell, bx)) = macro_of(&c.macro_name) else {
            if !missing.contains(&c.macro_name) {
                missing.push(c.macro_name.clone());
            }
            continue;
        };
        let pb = Trans::with(0, 0, pl.rot, pl.mirror).apply_rect(bx);
        let id = lay.add_instance(cell, Trans::with(pl.x - pb.x0, pl.y - pb.y0, pl.rot, pl.mirror), None);
        if let Some(i) = lay.instance_mut(id) {
            i.name = Some(c.name.clone());
        }
        out.placed += 1;
    }
    let size = (tech.dbu_per_um / 5).max(1);
    for p in &d.pins {
        let text = if p.net.is_empty() { p.name.clone() } else { p.net.clone() };
        let Some(pl) = p.place else { continue };
        match &p.shape {
            Some((ln, r)) => {
                let Some(l) = map_layer(tech, ln) else {
                    out.warnings.push(format!("pin '{}' on layer '{ln}' unknown to the technology skipped", p.name));
                    continue;
                };
                let g = pl.apply_rect(*r);
                lay.add(l, Geometry::Rect(g));
                if let Some(ll) = tech.net_label_layer(l) {
                    lay.add(ll, Geometry::Label { text, at: ((g.x0 + g.x1) / 2, (g.y0 + g.y1) / 2), size });
                }
            }
            None => {
                if let Some(ll) = tech.label_layer() {
                    lay.add(ll, Geometry::Label { text, at: (pl.x, pl.y), size });
                }
            }
        }
    }
    if let (Some(b), Some(die)) = (boundary_layer(tech), d.die) {
        lay.add(b, Geometry::Rect(die));
    }
    for m in &missing {
        out.warnings.push(format!("no cell for macro '{m}'; its components were skipped"));
    }
    out.missing = missing;
    out
}

/// LEF text of the abstracts of `cells`, with the technology header.
pub fn export_lef(p: &Project, cells: &[CellRef], opts: &AbstractOptions) -> Result<(String, Vec<String>), String> {
    let mut macros = vec![];
    let mut warnings = vec![];
    for r in cells {
        let l = p.layout(r).ok_or_else(|| format!("cell '{r}' has no layout"))?;
        let (m, w) = generate_abstract(&r.cell, l, p, &p.tech, opts);
        warnings.extend(w.into_iter().map(|w| format!("{}: {w}", r.cell)));
        macros.push(m);
    }
    Ok((write_lef(&p.tech, &macros, true), warnings))
}

/// Cells placed directly by `top` (the macros of its DEF), in first-use order.
pub fn def_macros(p: &Project, top: &CellRef) -> Vec<CellRef> {
    let mut out: Vec<CellRef> = vec![];
    for i in p.layout(top).map(|l| l.instances.as_slice()).unwrap_or_default() {
        if p.layout(&i.cell).is_some() && !out.contains(&i.cell) {
            out.push(i.cell.clone());
        }
    }
    out
}

/// DEF text of a top cell.
pub fn export_def(p: &Project, top: &CellRef) -> Result<(String, DefDesign, Vec<String>), String> {
    let l = p.layout(top).ok_or_else(|| format!("cell '{top}' has no layout"))?;
    let (d, w) = def_from_layout(&top.cell, l, p, &p.tech);
    Ok((write_def(&d, p.tech.dbu_per_um), d, w))
}

#[derive(Debug, Clone)]
pub struct LefImport {
    pub lib: String,
    pub cells: Vec<CellRef>,
    pub warnings: Vec<String>,
}

/// Import the macros of a LEF file as layout cells of a new library named after `lib_hint`.
pub fn import_lef(p: &mut Project, text: &str, lib_hint: &str) -> Result<LefImport, String> {
    let lib = read_lef(text, p.tech.dbu_per_um)?;
    if lib.macros.is_empty() {
        return Err("the LEF file contains no macros".into());
    }
    let mut warnings = vec![];
    let cells = lib
        .macros
        .iter()
        .map(|m| {
            let (l, w) = macro_layout(m, &p.tech);
            warnings.extend(w);
            (m.name.clone(), l)
        })
        .collect();
    let imp = GdsImport {
        cells,
        new_layer_base: p.tech.layers.len(),
        dbu_meters: 1e-6 / p.tech.dbu_per_um.max(1) as f64,
        ..GdsImport::default()
    };
    let (name, _) = p.add_gds_library(lib_hint, imp)?;
    let l = p.library_mut(&name).expect("library just added");
    for c in &mut l.cells {
        c.description = "Imported from LEF".into();
    }
    let cells = l.cells.iter().map(|c| CellRef::new(&name, &c.name)).collect();
    Ok(LefImport { lib: name, cells, warnings })
}

#[derive(Debug, Clone)]
pub struct DefImport {
    pub cell: CellRef,
    pub placed: usize,
    pub unplaced: usize,
    pub missing: Vec<String>,
    pub pins: usize,
    pub nets: usize,
    pub warnings: Vec<String>,
}

/// Import a DEF file as a new layout cell in `lib` (default `work`, else the first library)
/// placing the project cells named like its components' macros (those in `lib` first).
pub fn import_def(p: &mut Project, text: &str, lib: Option<&str>) -> Result<DefImport, String> {
    let d = read_def(text, p.tech.dbu_per_um)?;
    let lib = match lib {
        Some(l) => l.to_string(),
        None if p.libraries.iter().any(|l| l.name == "work") => "work".into(),
        None => match p.libraries.first() {
            Some(l) => l.name.clone(),
            None => {
                p.add_library("work")?;
                "work".into()
            }
        },
    };
    if !p.libraries.iter().any(|l| l.name == lib) {
        return Err(format!("no library '{lib}'"));
    }
    let mut boxes: HashMap<String, Option<(CellRef, IRect)>> = HashMap::new();
    let pr: &Project = p;
    let res = def_layout(&d, &pr.tech, |m| {
        boxes
            .entry(m.to_string())
            .or_insert_with(|| {
                let mut cands: Vec<(CellRef, &Layout)> =
                    pr.all_cells().filter(|(r, _)| r.cell == m).filter_map(|(r, c)| Some((r, c.layout.as_ref()?))).collect();
                cands.sort_by_key(|(r, _)| r.lib != lib);
                let (r, l) = cands.into_iter().next()?;
                Some((r, macro_box(l, pr, &pr.tech)?))
            })
            .clone()
    });
    let mut used: HashSet<String> = p
        .libraries
        .iter()
        .find(|l| l.name == lib)
        .map(|l| l.cells.iter().map(|c| c.name.to_ascii_lowercase()).collect())
        .unwrap_or_default();
    let name = unique_name(&sanitize_name(if d.name.is_empty() { "def_top" } else { &d.name }), &mut used);
    let cell = Cell { description: "Imported from DEF".into(), layout: Some(res.layout), ..Cell::new(name) };
    let r = p.add_cell(&lib, cell)?;
    Ok(DefImport {
        cell: r,
        placed: res.placed,
        unplaced: res.unplaced,
        missing: res.missing,
        pins: d.pins.len(),
        nets: d.nets.len(),
        warnings: res.warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::examples::demo_project;

    fn inverter() -> (Project, CellRef) {
        (demo_project(), CellRef::new("work", "cmos_inverter"))
    }

    #[test]
    fn layer_names_map() {
        let t = Tech::generic();
        assert_eq!(map_layer(&t, "M1"), t.layer_index("metal1"));
        assert_eq!(map_layer(&t, "met2"), t.layer_index("metal2"));
        assert_eq!(map_layer(&t, "VIA1"), t.layer_index("via1"));
        assert_eq!(map_layer(&t, "Metal3"), t.layer_index("metal3"));
        assert_eq!(map_layer(&t, "M9"), None);
        let names: Vec<(&str, LefLayerKind)> = lef_layers(&t).iter().map(|&(l, k)| (t.layers[l].name.as_str(), k)).collect();
        use LefLayerKind::*;
        assert_eq!(names, vec![("metal1", Routing), ("via1", Cut), ("metal2", Routing), ("via2", Cut), ("metal3", Routing)]);
        assert_eq!(supply_use("VDD"), Some("POWER"));
        assert_eq!(supply_use("vdda"), Some("POWER"));
        assert_eq!(supply_use("gnd!"), Some("GROUND"));
        assert_eq!(supply_use("vss"), Some("GROUND"));
        assert_eq!(supply_use("in"), None);
        assert_eq!(supply_use("vin"), None);
    }

    #[test]
    fn inverter_abstract() {
        let (p, r) = inverter();
        let (m, w) = generate_abstract("cmos_inverter", p.layout(&r).unwrap(), &p, &p.tech, &AbstractOptions::default());
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(m.size, (3400, 5800));
        assert_eq!(m.origin, (600, 400));
        let pins: Vec<(&str, &str, &str)> =
            m.pins.iter().map(|p| (p.name.as_str(), p.direction.as_str(), p.usage.as_str())).collect();
        assert_eq!(
            pins,
            vec![("gnd", "INOUT", "GROUND"), ("in", "INPUT", "SIGNAL"), ("out", "OUTPUT", "SIGNAL"), ("vdd", "INOUT", "POWER")]
        );
        for p in &m.pins {
            assert!(!p.ports.is_empty() && p.ports.iter().all(|s| s.layer == "metal1"), "{p:?}");
            for s in &p.ports {
                let b = s.geom.bbox();
                assert!(b.x0 >= 0 && b.y0 >= 0 && b.x1 <= m.size.0 && b.y1 <= m.size.1, "{b:?}");
            }
        }
        let out = &m.pins[2].ports;
        assert!(out.iter().any(|s| s.geom.bbox().contains(1710 + 600, 2000 + 400)));
    }

    #[test]
    fn lef_round_trip() {
        let p = demo_project();
        let cells: Vec<CellRef> = ["cmos_inverter", "nand2", "ota5t"].iter().map(|c| CellRef::new("work", *c)).collect();
        for obs in [ObsMode::Detailed, ObsMode::Block] {
            let opts = AbstractOptions { obs, ..AbstractOptions::default() };
            let macros: Vec<LefMacro> =
                cells.iter().map(|r| generate_abstract(&r.cell, p.layout(r).unwrap(), &p, &p.tech, &opts).0).collect();
            let text = write_lef(&p.tech, &macros, true);
            let lib = read_lef(&text, p.tech.dbu_per_um).unwrap();
            assert_eq!(lib.units, Some(1000));
            assert_eq!(lib.macros, macros);
            let layers: Vec<(&str, &str)> = lib.layers.iter().map(|(n, k)| (n.as_str(), k.as_str())).collect();
            assert_eq!(
                layers,
                vec![("metal1", "ROUTING"), ("via1", "CUT"), ("metal2", "ROUTING"), ("via2", "CUT"), ("metal3", "ROUTING")]
            );
            for (r, m) in cells.iter().zip(&macros).filter(|_| obs == ObsMode::Detailed) {
                let (lay, w) = macro_layout(m, &p.tech);
                assert!(w.is_empty(), "{w:?}");
                let (again, _) = generate_abstract(&r.cell, &lay, &p, &p.tech, &opts);
                assert_eq!(again.size, m.size);
                assert_eq!(again.origin, m.origin);
                let ports = |m: &LefMacro| m.pins.iter().map(|p| (p.name.clone(), p.ports.clone())).collect::<Vec<_>>();
                assert_eq!(ports(&again), ports(m), "{}", r.cell);
            }
        }
        let nand = generate_abstract("nand2", p.layout(&cells[1]).unwrap(), &p, &p.tech, &AbstractOptions::default()).0;
        let none = generate_abstract(
            "nand2",
            p.layout(&cells[1]).unwrap(),
            &p,
            &p.tech,
            &AbstractOptions { obs: ObsMode::None, ..AbstractOptions::default() },
        )
        .0;
        assert!(none.obs.is_empty());
        assert_eq!(none.pins, nand.pins);
    }

    #[test]
    fn reads_foreign_lef() {
        let text = r#"
# comment
VERSION 5.7 ;
NAMESCASESENSITIVE ON ;
UNITS DATABASE MICRONS 2000 ; END UNITS
SITE core SIZE 0.46 BY 2.72 ; CLASS CORE ; END core
LAYER met1 TYPE ROUTING ; DIRECTION HORIZONTAL ; WIDTH 0.14 ; END met1
LAYER via TYPE CUT ; END via
VIA L1M1_PR DEFAULT LAYER met1 ; RECT -0.1 -0.1 0.1 0.1 ; END L1M1_PR
MACRO buf
  CLASS CORE ;
  FOREIGN buf 0 0 ;
  ORIGIN 0 0 ;
  SIZE 1.38 BY 2.72 ;
  SYMMETRY X Y R90 ;
  SITE core ;
  PIN A
    DIRECTION INPUT ;
    USE SIGNAL ;
    ANTENNAGATEAREA 0.126 ;
    PORT
      LAYER met1 ;
        RECT 0.1 0.2 0.3 0.4;
        POLYGON MASK 1 0.5 0.5 0.9 0.5 0.9 0.7 0.7 0.7 0.7 0.9 0.5 0.9 ;
    END
  END A
  PIN VPWR
    USE POWER ;
    PORT
      LAYER met1 ;
        RECT 0 2.48 1.38 2.96 ;
    END
  END VPWR
  OBS
    LAYER met2 ;
      RECT 0.2 1.0 1.1 1.2 ;
    LAYER li1 ;
      RECT 0 0 1 1 ;
  END
END buf
END LIBRARY
"#;
        let lib = read_lef(text, 1000).unwrap();
        assert_eq!(lib.units, Some(2000));
        assert_eq!(lib.layers, vec![("met1".into(), "ROUTING".into()), ("via".into(), "CUT".into())]);
        let m = &lib.macros[0];
        assert_eq!((m.name.as_str(), m.class.as_str(), m.symmetry.as_str(), m.site.as_str()), ("buf", "CORE", "X Y R90", "core"));
        assert_eq!(m.size, (1380, 2720));
        assert_eq!(m.pins.len(), 2);
        assert_eq!(m.pins[0].ports[0].geom, Geometry::Rect(IRect::new(100, 200, 300, 400)));
        assert!(matches!(&m.pins[0].ports[1].geom, Geometry::Polygon { points } if points.len() == 6));
        assert_eq!((m.pins[1].usage.as_str(), m.pins[1].direction.as_str()), ("POWER", ""));
        assert_eq!(m.obs.len(), 2);

        let t = Tech::generic();
        let (lay, w) = macro_layout(m, &t);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("li1"));
        let labels: Vec<&str> = lay
            .shapes
            .iter()
            .filter_map(|s| match &s.geom {
                Geometry::Label { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(labels, vec!["A", "VPWR"]);
        let x = extract(&lay, &crate::hier::NoCells, &t);
        assert!(x.nets.iter().any(|n| n.labelled && n.name == "A"));
    }

    #[test]
    fn def_round_trip() {
        let p = demo_project();
        let top = CellRef::new("work", "inv_chain");
        let (text, d, w) = export_def(&p, &top).unwrap();
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(d.components.len(), 4);
        assert!(d.components.iter().all(|c| c.macro_name == "cmos_inverter"));
        let mut pins: Vec<&str> = d.pins.iter().map(|p| p.name.as_str()).collect();
        pins.sort();
        assert_eq!(pins, vec!["gnd", "in", "out", "vdd"]);
        assert!(d.pins.iter().all(|p| p.shape.as_ref().is_some_and(|(l, _)| l == "metal1")));
        let net = |n: &str| d.nets.iter().find(|x| x.name == n).unwrap();
        assert_eq!(net("vdd").conns.len(), 5);
        assert_eq!(net("vdd").usage, "POWER");
        assert_eq!(net("in").conns, vec![("PIN".into(), "in".into()), (d.components[0].name.clone(), "in".into())]);
        let internal: Vec<&DefNet> = d.nets.iter().filter(|n| n.conns.iter().all(|c| c.0 != "PIN")).collect();
        assert_eq!(internal.len(), 3);
        assert!(internal.iter().all(|n| n.conns.len() == 2));

        let back = read_def(&text, p.tech.dbu_per_um).unwrap();
        assert_eq!(back, d);
        let half = read_def(&text.replace("MICRONS 1000", "MICRONS 2000"), p.tech.dbu_per_um).unwrap();
        assert_eq!(half.components[1].place.map(|t| t.x * 2), d.components[1].place.map(|t| t.x));

        let mut q = p.clone();
        let imp = import_def(&mut q, &text, None).unwrap();
        assert_eq!((imp.placed, imp.unplaced, imp.missing.len(), imp.pins), (4, 0, 0, 4));
        assert_eq!(imp.cell.cell, "inv_chain_1");
        let orig = p.layout(&top).unwrap();
        let new = q.layout(&imp.cell).unwrap();
        let trans = |l: &Layout| l.instances.iter().map(|i| (i.cell.clone(), i.trans)).collect::<Vec<_>>();
        assert_eq!(trans(new), trans(orig));
        let x = extract(new, &q, &q.tech);
        let mut labelled: Vec<&str> = x.nets.iter().filter(|n| n.labelled).map(|n| n.name.as_str()).collect();
        labelled.sort();
        assert_eq!(labelled, vec!["gnd", "in", "out", "vdd"]);
    }

    #[test]
    fn def_orientations_round_trip() {
        let p = demo_project();
        let inv = CellRef::new("work", "cmos_inverter");
        let mut top = Layout::default();
        let mut k = 0;
        for rot in 0..4 {
            for mirror in [false, true] {
                top.add_instance(inv.clone(), Trans::with(k * 10_000, 20_000 + 70 * k, rot, mirror), None);
                k += 1;
            }
        }
        let mut q = p.clone();
        q.add_cell("work", Cell { layout: Some(top.clone()), ..Cell::new("orients") }).unwrap();
        let (text, d, _) = export_def(&q, &CellRef::new("work", "orients")).unwrap();
        let orients: Vec<&str> =
            d.components.iter().map(|c| c.place.map(|t| orient_name(t.rot, t.mirror)).unwrap_or("?")).collect();
        assert_eq!(orients, vec!["N", "FS", "W", "FW", "S", "FN", "E", "FE"]);
        let back = read_def(&text, q.tech.dbu_per_um).unwrap();
        let out = def_layout(&back, &q.tech, |m| {
            let r = CellRef::new("work", m);
            Some((r.clone(), macro_box(q.layout(&r)?, &q, &q.tech)?))
        });
        assert_eq!(out.placed, 8);
        for (a, b) in out.layout.instances.iter().zip(&top.instances) {
            assert_eq!(a.trans, b.trans);
        }
    }

    #[test]
    fn lef_import_and_def_import_into_project() {
        let (p, r) = inverter();
        let (text, w) = export_lef(&p, std::slice::from_ref(&r), &AbstractOptions::default()).unwrap();
        assert!(w.is_empty(), "{w:?}");
        let mut q = crate::project::Project::new("lefdef");
        let imp = import_lef(&mut q, &text, "cells").unwrap();
        assert_eq!(imp.lib, "cells");
        assert_eq!(imp.cells, vec![CellRef::new("cells", "cmos_inverter")]);
        let lay = q.layout(&imp.cells[0]).unwrap();
        assert_eq!(macro_box(lay, &q, &q.tech), Some(IRect::new(-600, -400, 2800, 5400)));

        let (def, _, _) = export_def(&p, &CellRef::new("work", "inv_chain")).unwrap();
        let di = import_def(&mut q, &def, None).unwrap();
        assert_eq!(di.cell, CellRef::new("work", "inv_chain"));
        assert_eq!(di.placed, 4);
        let top = q.layout(&di.cell).unwrap();
        assert!(top.instances.iter().all(|i| i.cell == imp.cells[0]));
        assert!(
            import_def(&mut q, "DESIGN x ; COMPONENTS 1 ; - u1 nosuch + PLACED ( 0 0 ) N ; END COMPONENTS END DESIGN", None)
                .unwrap()
                .missing
                .contains(&"nosuch".to_string())
        );
    }
}
