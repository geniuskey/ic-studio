//! Layout versus schematic: compare an extracted layout circuit with a schematic.
//!
//! Both sides are reduced to device graphs: MOS devices with gate, symmetric source / drain and
//! optional bulk terminals, resistors and capacitors with two symmetric terminals. Parallel
//! devices (same kind, model, terminals and, for MOS, bulk and L) are merged into one: MOS widths
//! and capacitances add, resistances combine as conductances. A schematic `m` multiplies W, so a
//! multi-finger layout transistor matches a single wide schematic device. Resistors of one model
//! in series through an unnamed net that nothing else touches merge into one as well. Merged
//! devices are reported with their names joined by `+`.
//!
//! Matching: nets named alike on both sides (schematic labels / supplies, layout labels,
//! case-insensitive, with "0" = "gnd" = "vss") are fixed anchors. Device and net classes are then
//! refined by iterated hashing of their neighbourhoods; classes holding exactly one schematic
//! and one layout item are matched and become anchors, symmetric classes are broken by pairing
//! their first members. Leftovers are paired by kind and best agreement with matched
//! neighbours, so a wrong connection reports as a net error instead of a missing device.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};

use crate::connectivity;
use crate::extract::{DevKind, Extraction, PassiveKind};
use crate::geom::IRect;
use crate::schematic::{DeviceKind, Schematic};
use crate::tech::{MosDef, Tech, mos_device_in};
use crate::units::{format_eng, parse_spice};

#[derive(Debug, Clone, PartialEq)]
pub struct LvsOptions {
    /// Ignore MOS bulk terminals.
    pub ignore_bulk: bool,
    /// Schematic device kinds that have no layout counterpart (test bench sources).
    pub ignore_kinds: Vec<DeviceKind>,
    /// Relative tolerance for W / L and resistance / capacitance comparison.
    pub tolerance: f64,
    /// Leave MOS devices whose gate is on their bulk net (always off: dummies, tie-offs, ESD
    /// clamps such as a GGNMOS) out of the matching. Layout ones need no schematic
    /// counterpart, but each schematic one whose drain or source is on another net must still
    /// have an off layout device of its type between the same nets, of the same total W.
    pub filter_off: bool,
    /// PDK devices ([`Tech::devices`]): when given, device models must match too (schematic
    /// models resolved as in [`Tech::mos_device`]).
    pub devices: Vec<MosDef>,
}

impl LvsOptions {
    /// The default options with the technology's devices.
    pub fn for_tech(tech: &Tech) -> Self {
        Self { devices: tech.devices.clone(), ..Self::default() }
    }
}

impl Default for LvsOptions {
    fn default() -> Self {
        Self {
            ignore_bulk: true,
            ignore_kinds: vec![DeviceKind::VSource, DeviceKind::ISource],
            tolerance: 0.01,
            filter_off: true,
            devices: vec![],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LvsErrorKind {
    /// Device only in the schematic.
    MissingDevice,
    /// Device only in the layout.
    ExtraDevice,
    /// Matched devices whose W / L, value or model differ.
    Parameter,
    /// Nets that could not be matched, or labels whose connections differ.
    Net,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LvsError {
    pub kind: LvsErrorKind,
    pub message: String,
    /// Where to look in the layout, when known.
    pub marker: Option<IRect>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LvsReport {
    pub matched: bool,
    /// (schematic device, layout device) pairs.
    pub devices: Vec<(String, String)>,
    /// (schematic net, layout net) pairs.
    pub nets: Vec<(String, String)>,
    pub errors: Vec<LvsError>,
    /// Findings that do not fail the comparison: wells or substrate only device bodies
    /// connect to (leaf cells are often tapped by the cell that places them) and extraction
    /// warnings.
    pub warnings: Vec<LvsError>,
    pub schematic_devices: usize,
    pub layout_devices: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Role {
    Gate,
    SourceDrain,
    Bulk,
    /// BJT emitter and collector (the base takes `Gate`).
    Emitter,
    Collector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    Nmos,
    Pmos,
    Res,
    Cap,
    Npn,
    Pnp,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Nmos => "nmos",
            Kind::Pmos => "pmos",
            Kind::Res => "resistor",
            Kind::Cap => "capacitor",
            Kind::Npn => "npn",
            Kind::Pnp => "pnp",
        }
    }

    fn mos(self) -> bool {
        matches!(self, Kind::Nmos | Kind::Pmos)
    }

    fn bjt(self) -> bool {
        matches!(self, Kind::Npn | Kind::Pnp)
    }
}

#[derive(Clone)]
struct Dev {
    kind: Kind,
    /// Model name: for MOS compared only when the technology defines devices, for resistors and
    /// capacitors when the schematic names one.
    model: Option<String>,
    /// MOS W and L in µm.
    w: f64,
    l: f64,
    /// Resistance in Ω, capacitance in F or BJT area factor.
    value: f64,
    /// Gate (MOS) or base (BJT).
    g: Option<usize>,
    /// Source and drain, resistor and capacitor terminals, or BJT emitter and collector.
    s: usize,
    d: usize,
    b: Option<usize>,
    name: String,
    gate: Option<IRect>,
}

struct Node {
    name: String,
    /// Carries a user-given name (schematic label / supply, layout label).
    named: bool,
    seed: Option<String>,
    marker: Option<IRect>,
    conns: Vec<(usize, Role)>,
}

#[derive(Default)]
struct Side {
    devs: Vec<Dev>,
    nets: Vec<Node>,
    index: HashMap<String, usize>,
}

impl Side {
    fn net(&mut self, name: &str, seed: Option<String>, marker: Option<IRect>) -> usize {
        if let Some(&i) = self.index.get(name) {
            return i;
        }
        self.nets.push(Node { name: name.to_string(), named: seed.is_some(), seed, marker, conns: Vec::new() });
        self.index.insert(name.to_string(), self.nets.len() - 1);
        self.nets.len() - 1
    }

    /// Merge parallel devices and series resistors, then record every net's terminal connections.
    fn finish(&mut self) {
        while self.merge_parallel() | self.merge_series() {}
        for (i, d) in self.devs.iter().enumerate() {
            if let Some(g) = d.g {
                self.nets[g].conns.push((i, Role::Gate));
            }
            let (rs, rd) = if d.kind.bjt() { (Role::Emitter, Role::Collector) } else { (Role::SourceDrain, Role::SourceDrain) };
            self.nets[d.s].conns.push((i, rs));
            self.nets[d.d].conns.push((i, rd));
            if let Some(b) = d.b {
                self.nets[b].conns.push((i, Role::Bulk));
            }
        }
    }

    /// Merge parallel devices; whether any merged.
    fn merge_parallel(&mut self) -> bool {
        let key = |d: &Dev| {
            let model = d.model.as_ref().map(|m| m.to_ascii_lowercase());
            let l = if d.kind.mos() { (d.l * 1e6).round() as i64 } else { 0 };
            let (s, dd) = if d.kind.bjt() { (d.s, d.d) } else { (d.s.min(d.d), d.s.max(d.d)) };
            (d.kind, model, d.g, s, dd, d.b, l)
        };
        let before = self.devs.len();
        let mut groups: Vec<Dev> = Vec::new();
        let mut at: HashMap<_, usize> = HashMap::new();
        for d in std::mem::take(&mut self.devs) {
            match at.get(&key(&d)) {
                Some(&i) => {
                    let m = &mut groups[i];
                    match d.kind {
                        Kind::Nmos | Kind::Pmos => m.w += d.w,
                        Kind::Res => m.value = 1.0 / (1.0 / m.value + 1.0 / d.value),
                        Kind::Cap | Kind::Npn | Kind::Pnp => m.value += d.value,
                    }
                    m.absorb(&d);
                }
                None => {
                    at.insert(key(&d), groups.len());
                    groups.push(d);
                }
            }
        }
        self.devs = groups;
        self.devs.len() < before
    }

    /// Merge resistors in series through unnamed nets; whether any merged. Each pass merges
    /// every pair whose resistors no other merge of the pass touched, so a chain of k
    /// resistors takes about log k passes instead of k.
    fn merge_series(&mut self) -> bool {
        let mut uses: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (i, d) in self.devs.iter().enumerate() {
            for n in [d.g, Some(d.s), Some(d.d), d.b].into_iter().flatten() {
                uses.entry(n).or_default().push(i);
            }
        }
        let same_model =
            |a: &Dev, b: &Dev| a.model.as_deref().map(str::to_ascii_lowercase) == b.model.as_deref().map(str::to_ascii_lowercase);
        let mut touched = vec![false; self.devs.len()];
        let mut gone = vec![false; self.devs.len()];
        let mut merged = false;
        for (&n, ds) in &uses {
            let [i, j] = ds[..] else { continue };
            if i == j || self.nets[n].named || touched[i] || touched[j] {
                continue;
            }
            let (a, b) = (&self.devs[i], &self.devs[j]);
            if !(a.kind == Kind::Res && b.kind == Kind::Res && same_model(a, b)) {
                continue;
            }
            (touched[i], touched[j], gone[j]) = (true, true, true);
            let keep = self.devs[i].clone();
            let d = std::mem::replace(&mut self.devs[j], keep);
            let far = if d.s == n { d.d } else { d.s };
            let m = &mut self.devs[i];
            if m.s == n {
                m.s = far;
            } else {
                m.d = far;
            }
            m.value += d.value;
            m.absorb(&d);
            merged = true;
        }
        if merged {
            let mut k = 0;
            self.devs.retain(|_| {
                k += 1;
                !gone[k - 1]
            });
        }
        merged
    }
}

impl Dev {
    fn absorb(&mut self, d: &Dev) {
        self.name = format!("{}+{}", self.name, d.name);
        self.gate = match (self.gate, d.gate) {
            (Some(a), Some(b)) => Some(a.union(&b)),
            (a, b) => a.or(b),
        };
    }
}

fn normalize(name: &str) -> String {
    let n = name.trim().to_ascii_lowercase();
    match n.trim_end_matches('!') {
        "0" | "gnd" | "vss" => "0".to_string(),
        _ => n,
    }
}

fn is_auto_name(name: &str) -> bool {
    name.strip_prefix("net").is_some_and(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit()))
}

fn um(v: f64) -> String {
    let s = format!("{v:.4}");
    format!("{}µm", s.trim_end_matches('0').trim_end_matches('.'))
}

fn value(kind: Kind, v: f64) -> String {
    match kind {
        Kind::Res => format!("R={}", format_eng(v, " ohm")),
        Kind::Npn | Kind::Pnp => format!("area={}", format_eng(v, "")), // i18n: ignore
        _ => format!("C={}", format_eng(v, "F")),
    }
}

fn describe(d: &Dev) -> String {
    let model = d.model.as_deref().unwrap_or(d.kind.label());
    if d.kind.mos() {
        format!("{} ({model} W={} L={})", d.name, um(d.w), um(d.l))
    } else {
        format!("{} ({model} {})", d.name, value(d.kind, d.value))
    }
}

fn hash<T: Hash>(t: T) -> u64 {
    let mut h = DefaultHasher::new();
    t.hash(&mut h);
    h.finish()
}

#[derive(Clone)]
struct State {
    dev: Vec<u64>,
    net: Vec<u64>,
    dev_fixed: Vec<bool>,
    net_fixed: Vec<bool>,
    dev_match: Vec<Option<usize>>,
    net_match: Vec<Option<usize>>,
}

impl State {
    fn new(side: &Side) -> State {
        State {
            dev: side.devs.iter().map(|d| hash(("dev", d.kind))).collect(),
            net: side.nets.iter().map(|_| hash("net")).collect(),
            dev_fixed: vec![false; side.devs.len()],
            net_fixed: vec![false; side.nets.len()],
            dev_match: vec![None; side.devs.len()],
            net_match: vec![None; side.nets.len()],
        }
    }

    fn refine(&mut self, side: &Side) {
        let net = &self.net;
        let dev: Vec<u64> = side
            .devs
            .iter()
            .enumerate()
            .map(|(i, d)| {
                if self.dev_fixed[i] {
                    return self.dev[i];
                }
                let (a, b) = (net[d.s], net[d.d]);
                let (a, b) = if d.kind.bjt() { (a, b) } else { (a.min(b), a.max(b)) };
                hash((self.dev[i], d.g.map(|g| net[g]), a, b, d.b.map(|b| net[b])))
            })
            .collect();
        let net: Vec<u64> = side
            .nets
            .iter()
            .enumerate()
            .map(|(i, n)| {
                if self.net_fixed[i] {
                    return self.net[i];
                }
                let mut around: Vec<(u64, Role)> = n.conns.iter().map(|&(d, r)| (dev[d], r)).collect();
                around.sort_unstable_by_key(|&(c, r)| (c, r as u8));
                hash((self.net[i], around))
            })
            .collect();
        self.dev = dev;
        self.net = net;
    }

    fn classes(&self) -> usize {
        self.dev.iter().chain(&self.net).collect::<HashSet<_>>().len()
    }
}

fn pair_dev(a: &mut State, b: &mut State, i: usize, j: usize, tag: u64) {
    a.dev_match[i] = Some(j);
    b.dev_match[j] = Some(i);
    (a.dev_fixed[i], b.dev_fixed[j]) = (true, true);
    (a.dev[i], b.dev[j]) = (tag, tag);
}

fn pair_net(a: &mut State, b: &mut State, i: usize, j: usize, tag: u64) {
    a.net_match[i] = Some(j);
    b.net_match[j] = Some(i);
    (a.net_fixed[i], b.net_fixed[j]) = (true, true);
    (a.net[i], b.net[j]) = (tag, tag);
}

/// A schematic MOS left out of the matching by [`LvsOptions::filter_off`].
struct OffDev {
    name: String,
    kind: Kind,
    d: String,
    s: String,
    g: String,
    /// W (times m) and L in µm.
    w: f64,
    l: f64,
}

fn schematic_side(
    sch: &Schematic,
    opts: &LvsOptions,
    errors: &mut Vec<LvsError>,
    count: &mut usize,
    off: &mut Vec<OffDev>,
) -> Side {
    let conn = connectivity::extract(sch);
    let mut side = Side::default();
    let mut insts: Vec<_> = sch.instances.iter().collect();
    insts.sort_by(|a, b| a.name.cmp(&b.name));
    for inst in insts {
        if matches!(inst.kind, DeviceKind::Ground | DeviceKind::Supply)
            || opts.ignore_kinds.contains(&inst.kind)
            || inst.lvs_ignore()
        {
            continue;
        }
        if opts.filter_off && inst.kind.is_mos() {
            let floating = inst.pin_positions().iter().any(|(n, p)| *n == "b" && conn.unconnected_pins.contains(p));
            let bulk = conn.net_of(inst.id, if floating { "s" } else { "b" });
            if bulk.is_some() && conn.net_of(inst.id, "g") == bulk {
                let n = |pin: &str| conn.net_of(inst.id, pin).unwrap_or("?").to_string();
                let p = |k: &str| parse_spice(inst.param(k)).map_or(f64::NAN, |v| v * 1e6);
                let m = parse_spice(inst.param("m")).map_or(1.0, |m| m.round().max(1.0));
                let kind = if matches!(inst.kind, DeviceKind::Pmos | DeviceKind::Pmos3) { Kind::Pmos } else { Kind::Nmos };
                off.push(OffDev { name: inst.name.clone(), kind, d: n("d"), s: n("s"), g: n("g"), w: p("w") * m, l: p("l") });
                continue;
            }
        }
        *count += 1;
        let kind = match inst.kind {
            DeviceKind::Nmos | DeviceKind::Nmos3 => Kind::Nmos,
            DeviceKind::Pmos | DeviceKind::Pmos3 => Kind::Pmos,
            DeviceKind::Resistor => Kind::Res,
            DeviceKind::Capacitor => Kind::Cap,
            DeviceKind::Npn => Kind::Npn,
            DeviceKind::Pnp => Kind::Pnp,
            k => {
                errors.push(LvsError {
                    kind: LvsErrorKind::MissingDevice,
                    message: format!(
                        "{} ({}): not extracted from layout (only MOS and bipolar transistors, resistors and capacitors are recognised)",
                        inst.name,
                        k.label().to_ascii_lowercase()
                    ),
                    marker: None,
                });
                continue;
            }
        };
        let mut net = |pin: &str| {
            let name = conn.net_of(inst.id, pin).unwrap_or("?").to_string();
            let seed = (!is_auto_name(&name) && name != "?").then(|| normalize(&name));
            side.net(&name, seed, None)
        };
        if kind.bjt() {
            let area = inst.param("area").trim();
            let a = if area.is_empty() { Some(1.0) } else { parse_spice(area) };
            if a.is_none() {
                errors.push(LvsError {
                    kind: LvsErrorKind::Parameter,
                    message: format!("{}: cannot read area='{area}'", inst.name),
                    marker: None,
                });
            }
            let m = parse_spice(inst.param("m")).map_or(1.0, |m| m.round().max(1.0));
            let model = inst.param("model").trim();
            let (b, e, c) = (net("b"), net("e"), net("c"));
            side.devs.push(Dev {
                kind,
                // The generic model names match any PDK device.
                model: (!model.is_empty() && !model.eq_ignore_ascii_case(kind.label())).then(|| model.to_string()),
                w: f64::NAN,
                l: f64::NAN,
                value: a.map_or(f64::NAN, |a| a * m),
                g: Some(b),
                s: e,
                d: c,
                b: None,
                name: inst.name.clone(),
                gate: None,
            });
            continue;
        }
        if !kind.mos() {
            let key = if kind == Kind::Res { "r" } else { "c" };
            let v = parse_spice(inst.param(key));
            if v.is_none() {
                errors.push(LvsError {
                    kind: LvsErrorKind::Parameter,
                    message: format!("{}: cannot read {}='{}'", inst.name, key.to_ascii_uppercase(), inst.param(key)),
                    marker: None,
                });
            }
            let m = parse_spice(inst.param("m")).map_or(1.0, |m| m.round().max(1.0));
            let model = inst.param("model").trim();
            let (s, d) = (net("p"), net("n"));
            side.devs.push(Dev {
                kind,
                model: (!model.is_empty()).then(|| model.to_string()),
                w: f64::NAN,
                l: f64::NAN,
                value: v.map_or(f64::NAN, |v| if kind == Kind::Res { v / m } else { v * m }),
                g: None,
                s,
                d,
                b: None,
                name: inst.name.clone(),
                gate: None,
            });
            continue;
        }
        let (g, s, d) = (net("g"), net("s"), net("d"));
        let b = if opts.ignore_bulk {
            None
        } else {
            let floating = inst.pin_positions().iter().any(|(n, p)| *n == "b" && conn.unconnected_pins.contains(p));
            Some(if floating { s } else { net("b") })
        };
        let param = |k: &str| parse_spice(inst.param(k)).map(|v| v * 1e6);
        let (w, l) = (param("w"), param("l"));
        if w.is_none() || l.is_none() {
            errors.push(LvsError {
                kind: LvsErrorKind::Parameter,
                message: format!("{}: cannot read W='{}' / L='{}'", inst.name, inst.param("w"), inst.param("l")),
                marker: None,
            });
        }
        let m = parse_spice(inst.param("m")).map_or(1.0, |m| m.round().max(1.0));
        let model = (!opts.devices.is_empty()).then(|| {
            mos_device_in(&opts.devices, kind == Kind::Pmos, inst.param("model"))
                .map_or_else(|| inst.param("model").trim().to_string(), |d| d.name.clone())
        });
        side.devs.push(Dev {
            kind,
            model,
            w: w.unwrap_or(f64::NAN) * m,
            l: l.unwrap_or(f64::NAN),
            value: f64::NAN,
            g: Some(g),
            s,
            d,
            b,
            name: inst.name.clone(),
            gate: None,
        });
    }
    side
}

fn layout_side(lay: &Extraction, opts: &LvsOptions) -> Side {
    let dbu = if lay.dbu_per_um > 0 { lay.dbu_per_um } else { 1000 } as f64;
    let mut side = Side::default();
    let mut map: HashMap<usize, usize> = HashMap::new();
    let mut net = |side: &mut Side, i: usize| {
        *map.entry(i).or_insert_with(|| {
            let n = &lay.nets[i];
            let key = format!("{}#{i}", n.name);
            side.net(&key, n.labelled.then(|| normalize(&n.name)), n.bbox);
            let k = side.nets.len() - 1;
            side.nets[k].name = n.name.clone();
            k
        })
    };
    for d in lay.devices.iter().filter(|d| !(opts.filter_off && d.g == d.b)) {
        let (g, s, dd) = (net(&mut side, d.g), net(&mut side, d.s), net(&mut side, d.d));
        let b = (!opts.ignore_bulk).then(|| net(&mut side, d.b));
        side.devs.push(Dev {
            kind: if d.kind == DevKind::Pmos { Kind::Pmos } else { Kind::Nmos },
            model: (!opts.devices.is_empty()).then(|| d.model.clone()),
            w: d.w as f64 / dbu,
            l: d.l as f64 / dbu,
            value: f64::NAN,
            g: Some(g),
            s,
            d: dd,
            b,
            name: d.name.clone(),
            gate: Some(d.gate),
        });
    }
    for q in &lay.bjts {
        let (b, e, c) = (net(&mut side, q.b), net(&mut side, q.e), net(&mut side, q.c));
        side.devs.push(Dev {
            kind: if q.pnp { Kind::Pnp } else { Kind::Npn },
            model: Some(q.model.clone()),
            w: f64::NAN,
            l: f64::NAN,
            value: q.area,
            g: Some(b),
            s: e,
            d: c,
            b: None,
            name: q.name.clone(),
            gate: Some(q.emitter),
        });
    }
    for p in &lay.passives {
        let (s, d) = (net(&mut side, p.a), net(&mut side, p.b));
        side.devs.push(Dev {
            kind: if p.kind == PassiveKind::Resistor { Kind::Res } else { Kind::Cap },
            model: Some(p.model.clone()),
            w: f64::NAN,
            l: f64::NAN,
            value: p.value,
            g: None,
            s,
            d,
            b: None,
            name: p.name.clone(),
            gate: Some(p.body),
        });
    }
    side
}

/// Drop seeds that are not unique on both sides.
fn prune_seeds(a: &mut Side, b: &mut Side) {
    let count = |s: &Side| {
        let mut m: HashMap<String, usize> = HashMap::new();
        for n in &s.nets {
            if let Some(k) = &n.seed {
                *m.entry(k.clone()).or_default() += 1;
            }
        }
        m
    };
    let (ca, cb) = (count(a), count(b));
    let ok = |k: &String| ca.get(k) == Some(&1) && cb.get(k) == Some(&1);
    for n in a.nets.iter_mut().chain(b.nets.iter_mut()) {
        if n.seed.as_ref().is_some_and(|k| !ok(k)) {
            n.seed = None;
        }
    }
}

/// Refine both sides until the number of classes stops growing.
fn settle(sa: &Side, sb: &Side, a: &mut State, b: &mut State, limit: usize) {
    let mut classes = (a.classes(), b.classes());
    for _ in 0..limit {
        a.refine(sa);
        b.refine(sb);
        let now = (a.classes(), b.classes());
        if now == classes {
            break;
        }
        classes = now;
    }
}

/// Unmatched devices and nets by (is a net, class): their indices on side a and side b.
type Classes = BTreeMap<(bool, u64), (Vec<usize>, Vec<usize>)>;

fn unmatched_classes(a: &State, b: &State) -> Classes {
    let mut groups = Classes::new();
    for (st, first) in [(a, true), (b, false)] {
        let items = st.dev.iter().enumerate().filter(|(i, _)| st.dev_match[*i].is_none()).map(|(i, &c)| (false, c, i));
        let nets = st.net.iter().enumerate().filter(|(i, _)| st.net_match[*i].is_none()).map(|(i, &c)| (true, c, i));
        for (is_net, c, i) in items.chain(nets) {
            let g = groups.entry((is_net, c)).or_default();
            if first { g.0.push(i) } else { g.1.push(i) }
        }
    }
    groups
}

/// How many unmatched items have no counterpart in their class.
fn imbalance(groups: &Classes) -> usize {
    groups.values().map(|(xs, ys)| xs.len().abs_diff(ys.len())).sum()
}

fn match_graphs(sa: &Side, sb: &Side, a: &mut State, b: &mut State) {
    let mut tag = 0u64;
    let mut fresh = || {
        tag += 1;
        hash(("match", tag))
    };
    let seeds: BTreeMap<&str, usize> = sb.nets.iter().enumerate().filter_map(|(j, n)| Some((n.seed.as_deref()?, j))).collect();
    for (i, n) in sa.nets.iter().enumerate() {
        if let Some(&j) = n.seed.as_deref().and_then(|k| seeds.get(k)) {
            pair_net(a, b, i, j, fresh());
        }
    }
    let limit = sa.devs.len() + sa.nets.len() + sb.devs.len() + sb.nets.len() + 2;
    loop {
        settle(sa, sb, a, b, limit);
        let groups = unmatched_classes(a, b);
        let mut progress = false;
        for (&(is_net, _), (xs, ys)) in &groups {
            if xs.len() == 1 && ys.len() == 1 {
                let t = fresh();
                if is_net {
                    pair_net(a, b, xs[0], ys[0], t)
                } else {
                    pair_dev(a, b, xs[0], ys[0], t)
                }
                progress = true;
            }
        }
        if progress {
            continue;
        }
        // Symmetric classes (copies of a cell, mirrored halves) need a guess. One guess per
        // full refinement is quadratic on arrays, so guess in every such class at once (nets
        // first, as they settle devices fastest): all members of each, paired in order. Keep
        // the guesses if no class ends up less balanced than before; else try the first half
        // of them, down to the single guess of the smallest class, which is always kept.
        let mut ambiguous: Vec<_> = groups.iter().filter(|(_, (xs, ys))| !xs.is_empty() && !ys.is_empty()).collect();
        let nets_left = ambiguous.iter().any(|((is_net, _), _)| *is_net);
        ambiguous.retain(|((is_net, _), _)| *is_net == nets_left);
        ambiguous.sort_by_key(|(key, (xs, ys))| (xs.len().max(ys.len()), **key));
        let guesses: Vec<(bool, usize, usize)> = ambiguous
            .iter()
            .flat_map(|((is_net, _), (xs, ys))| {
                let is_net = *is_net;
                xs.iter().zip(ys.iter()).map(move |(&x, &y)| (is_net, x, y))
            })
            .collect();
        if guesses.is_empty() {
            break;
        }
        let before = imbalance(&groups);
        let mut n = guesses.len();
        loop {
            let saved = (n > 1).then(|| (a.clone(), b.clone()));
            for &(is_net, x, y) in &guesses[..n] {
                let t = fresh();
                if is_net { pair_net(a, b, x, y, t) } else { pair_dev(a, b, x, y, t) }
            }
            let Some((sa0, sb0)) = saved else { break };
            settle(sa, sb, a, b, limit);
            if imbalance(&unmatched_classes(a, b)) <= before {
                break;
            }
            (*a, *b) = (sa0, sb0);
            n /= 2;
        }
    }

    // What refinement left: pair by the most matched neighbours. Only items next to a matched
    // neighbour can score above zero, so the candidates come from the matched nets' and
    // devices' connections instead of a scan of the whole other side.
    for _ in 0..2 {
        for (i, d) in sa.devs.iter().enumerate() {
            if a.dev_match[i].is_some() {
                continue;
            }
            let m = |n: usize| a.net_match[n];
            let mut cands: Vec<usize> = [d.g, Some(d.s), Some(d.d), d.b]
                .into_iter()
                .flatten()
                .filter_map(m)
                .flat_map(|n| sb.nets[n].conns.iter().map(|&(j, _)| j))
                .filter(|&j| b.dev_match[j].is_none() && sb.devs[j].kind == d.kind)
                .collect();
            cands.sort_unstable();
            cands.dedup();
            let best = cands
                .into_iter()
                .map(|j| {
                    let e = &sb.devs[j];
                    let sd = [m(d.s), m(d.d)];
                    let score = usize::from(d.g.is_some() && d.g.and_then(m) == e.g)
                        + usize::from(sd.contains(&Some(e.s)))
                        + usize::from(sd.contains(&Some(e.d)))
                        + usize::from(d.b.is_some() && d.b.and_then(m) == e.b);
                    (score, std::cmp::Reverse(j))
                })
                .max();
            if let Some((score, std::cmp::Reverse(j))) = best
                && score > 0
            {
                let t = fresh();
                pair_dev(a, b, i, j, t);
            }
        }
        for (i, n) in sa.nets.iter().enumerate() {
            if a.net_match[i].is_some() {
                continue;
            }
            let want: Vec<(usize, Role)> = n.conns.iter().filter_map(|&(d, r)| Some((a.dev_match[d]?, r))).collect();
            let mut cands: Vec<usize> = want
                .iter()
                .flat_map(|&(j, _)| {
                    let e = &sb.devs[j];
                    [e.g, Some(e.s), Some(e.d), e.b].into_iter().flatten()
                })
                .filter(|&k| b.net_match[k].is_none())
                .collect();
            cands.sort_unstable();
            cands.dedup();
            let best = cands
                .into_iter()
                .map(|j| (want.iter().filter(|c| sb.nets[j].conns.contains(c)).count(), std::cmp::Reverse(j)))
                .max();
            if let Some((score, std::cmp::Reverse(j))) = best
                && score > 0
            {
                let t = fresh();
                pair_net(a, b, i, j, t);
            }
        }
    }
}

pub fn compare(sch: &Schematic, lay: &Extraction, opts: &LvsOptions) -> LvsReport {
    let mut errors = Vec::new();
    let mut schematic_devices = 0;
    let mut off = Vec::new();
    let mut sa = schematic_side(sch, opts, &mut errors, &mut schematic_devices, &mut off);
    let mut sb = layout_side(lay, opts);
    sa.finish();
    sb.finish();
    prune_seeds(&mut sa, &mut sb);
    let (mut a, mut b) = (State::new(&sa), State::new(&sb));
    match_graphs(&sa, &sb, &mut a, &mut b);

    let err = |kind, message: String, marker| LvsError { kind, message, marker };
    let net_name = |s: &Side, n: usize| format!("'{}'", s.nets[n].name);
    let mut devices = Vec::new();
    for (i, d) in sa.devs.iter().enumerate() {
        let Some(j) = a.dev_match[i] else {
            errors.push(err(
                LvsErrorKind::MissingDevice,
                format!("schematic device {} has no layout counterpart", describe(d)),
                None,
            ));
            continue;
        };
        let e = &sb.devs[j];
        devices.push((d.name.clone(), e.name.clone()));
        let who = format!("{} / {}", d.name, e.name);
        let m = |n: usize| a.net_match[n];
        if let (Some(x), Some(y)) = (d.g, e.g)
            && m(x) != Some(y)
        {
            let what = if d.kind.bjt() { "base" } else { "gate" };
            let msg = format!("{who}: {what} on {} in the schematic but {} in the layout", net_name(&sa, x), net_name(&sb, y));
            errors.push(err(LvsErrorKind::Net, msg, e.gate));
        }
        let (ms, md) = (m(d.s), m(d.d));
        let swapped = !d.kind.bjt() && ms == Some(e.d) && md == Some(e.s);
        if !((ms == Some(e.s) && md == Some(e.d)) || swapped) {
            let msg = format!(
                "{who}: {} on {}, {} in the schematic but {}, {} in the layout",
                if d.kind.mos() {
                    "source/drain"
                } else if d.kind.bjt() {
                    "emitter/collector"
                } else {
                    "terminals"
                },
                net_name(&sa, d.s),
                net_name(&sa, d.d),
                net_name(&sb, e.s),
                net_name(&sb, e.d)
            );
            errors.push(err(LvsErrorKind::Net, msg, e.gate));
        }
        if let (Some(x), Some(y)) = (d.b, e.b)
            && m(x) != Some(y)
        {
            let msg = format!("{who}: bulk on {} in the schematic but {} in the layout", net_name(&sa, x), net_name(&sb, y));
            errors.push(err(LvsErrorKind::Net, msg, e.gate));
        }
        if let (Some(s), Some(l)) = (&d.model, &e.model)
            && !s.eq_ignore_ascii_case(l)
        {
            errors.push(err(LvsErrorKind::Parameter, format!("{who}: model {s} (schematic) vs {l} (layout)"), e.gate));
        }
        if d.kind.mos() {
            for (what, s, l) in [("W", d.w, e.w), ("L", d.l, e.l)] {
                if s.is_finite() && (s - l).abs() > opts.tolerance * s.abs() {
                    let msg = format!("{who}: {what} {} (schematic) vs {} (layout)", um(s), um(l));
                    errors.push(err(LvsErrorKind::Parameter, msg, e.gate));
                }
            }
        } else if d.value.is_finite() && (d.value - e.value).abs() > opts.tolerance * d.value.abs() {
            let msg = format!("{who}: {} (schematic) vs {} (layout)", value(d.kind, d.value), value(e.kind, e.value));
            errors.push(err(LvsErrorKind::Parameter, msg, e.gate));
        }
    }
    for (j, e) in sb.devs.iter().enumerate() {
        if b.dev_match[j].is_none() {
            errors.push(err(LvsErrorKind::ExtraDevice, format!("layout device {} is not in the schematic", describe(e)), e.gate));
        }
    }

    let touches_matched = |n: &Node, st: &State| n.conns.iter().any(|&(d, _)| st.dev_match[d].is_some());
    let mut nets = Vec::new();
    for (i, n) in sa.nets.iter().enumerate() {
        match a.net_match[i] {
            Some(_) if n.conns.is_empty() => {}
            Some(j) => {
                let o = &sb.nets[j];
                nets.push((n.name.clone(), o.name.clone()));
                if n.named && o.named && normalize(&n.name) != normalize(&o.name) {
                    let msg = format!("schematic net '{}' corresponds to the layout net labelled '{}'", n.name, o.name);
                    errors.push(err(LvsErrorKind::Net, msg, o.marker));
                }
            }
            None if touches_matched(n, &a) => {
                errors.push(err(LvsErrorKind::Net, format!("schematic net '{}' has no layout counterpart", n.name), None));
            }
            None => {}
        }
    }
    for (j, n) in sb.nets.iter().enumerate() {
        if b.net_match[j].is_none() && touches_matched(n, &b) {
            errors.push(err(LvsErrorKind::Net, format!("layout net '{}' has no schematic counterpart", n.name), n.marker));
        }
    }
    check_off_devices(&off, lay, &nets, opts, &mut errors);
    let mut warnings = floating_bulks(lay);
    for sc in &lay.soft_conflicts {
        let names: Vec<String> = sc.nets.iter().map(|&n| format!("'{}'", lay.nets[n].name)).collect();
        let what = if sc.well.is_some() { "a well" } else { "the substrate" };
        let msg = format!(
            "soft connection: {what} is tapped from nets {}; the well does not join them and is taken as {}",
            names.join(", "),
            names[0]
        );
        warnings.push(err(LvsErrorKind::Net, msg, sc.well));
    }
    warnings.extend(lay.warnings.iter().map(|w| err(LvsErrorKind::Net, format!("extraction: {w}"), None)));
    devices.sort();
    nets.sort();
    let layout_devices =
        lay.devices.iter().filter(|d| !(opts.filter_off && d.g == d.b)).count() + lay.passives.len() + lay.bjts.len();
    LvsReport { matched: errors.is_empty(), devices, nets, errors, warnings, schematic_devices, layout_devices }
}

/// Each schematic off device ([`LvsOptions::filter_off`]) that is not a plain dummy (drain or
/// source off its gate net) needs off layout devices of its type between the same nets (through
/// the matched net names), of the same total W.
fn check_off_devices(off: &[OffDev], lay: &Extraction, nets: &[(String, String)], opts: &LvsOptions, errors: &mut Vec<LvsError>) {
    let to_layout: HashMap<String, String> = nets.iter().map(|(a, b)| (normalize(a), normalize(b))).collect();
    let layout = |n: &str| to_layout.get(&normalize(n)).cloned().unwrap_or_else(|| normalize(n));
    let dbu = if lay.dbu_per_um > 0 { lay.dbu_per_um } else { 1000 } as f64;
    let mut used = vec![false; lay.devices.len()];
    for o in off.iter().filter(|o| o.d != o.g || o.s != o.g) {
        let mut want = [layout(&o.d), layout(&o.s)];
        want.sort();
        let mut w = 0.0;
        let mut found = None;
        for (j, d) in lay.devices.iter().enumerate() {
            let kind = if d.kind == DevKind::Pmos { Kind::Pmos } else { Kind::Nmos };
            if used[j] || d.g != d.b || kind != o.kind {
                continue;
            }
            let mut have = [normalize(&lay.nets[d.d].name), normalize(&lay.nets[d.s].name)];
            have.sort();
            if have == want && (!o.l.is_finite() || (d.l as f64 / dbu - o.l).abs() <= opts.tolerance * o.l.abs()) {
                used[j] = true;
                w += d.w as f64 / dbu;
                found.get_or_insert(d.gate);
            }
        }
        let what = format!("{} ({} W={} L={}, gate on its bulk)", o.name, o.kind.label(), um(o.w), um(o.l));
        match found {
            None => errors.push(LvsError {
                kind: LvsErrorKind::MissingDevice,
                message: format!("schematic device {what} has no off layout device between '{}' and '{}'", o.d, o.s),
                marker: None,
            }),
            Some(gate) if o.w.is_finite() && (w - o.w).abs() > opts.tolerance * o.w.abs() => errors.push(LvsError {
                kind: LvsErrorKind::Parameter,
                message: format!("{}: W {} (schematic) vs {} (layout, gate on its bulk)", o.name, um(o.w), um(w)),
                marker: Some(gate),
            }),
            Some(_) => {}
        }
    }
}

/// Wells and substrate regions that only MOS bulks connect to: no tap ties them to a supply
/// and no label makes them a port of the cell, so the devices' bodies float unless the cell
/// placing this one taps them. Reported whether or not bulks are compared
/// ([`LvsOptions::ignore_bulk`]).
fn floating_bulks(lay: &Extraction) -> Vec<LvsError> {
    let mut other = vec![false; lay.nets.len()];
    let mut bulks = vec![0usize; lay.nets.len()];
    for d in &lay.devices {
        for n in [d.d, d.g, d.s] {
            if let Some(o) = other.get_mut(n) {
                *o = true;
            }
        }
        if let Some(b) = bulks.get_mut(d.b) {
            *b += 1;
        }
    }
    for p in &lay.passives {
        for n in [p.a, p.b] {
            if let Some(o) = other.get_mut(n) {
                *o = true;
            }
        }
    }
    for q in &lay.bjts {
        for n in [q.c, q.b, q.e] {
            if let Some(o) = other.get_mut(n) {
                *o = true;
            }
        }
    }
    lay.nets
        .iter()
        .enumerate()
        .filter(|(i, n)| bulks[*i] > 0 && !other[*i] && !n.labelled)
        .map(|(i, n)| LvsError {
            kind: LvsErrorKind::Net,
            message: format!(
                "layout net '{}' is a well / substrate that only device bodies connect to ({} device(s)): add a tap to its supply, or label it as a port",
                n.name, bulks[i]
            ),
            marker: n.bbox,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::extract;
    use crate::geom::{Orient, Pt};
    use crate::hier::NoCells;
    use crate::layout::{Geometry, Layout};
    use crate::pcell::PCell;
    use crate::tech::Tech;

    /// MOS devices connected through labels on their pins: (kind, w, [d, g, s] net names).
    fn schematic(devs: &[(DeviceKind, &str, [&str; 3])]) -> Schematic {
        let mut s = Schematic::default();
        for (k, (kind, w, nets)) in devs.iter().enumerate() {
            let id = s.add_instance(*kind, Pt::new(10 * k as i32, 0), Orient::default());
            s.instance_mut(id).unwrap().params.insert("w".into(), (*w).into());
            let pins = s.instance(id).unwrap().pin_positions();
            for (pin, net) in ["d", "g", "s"].iter().zip(nets) {
                let p = pins.iter().find(|(n, _)| n == pin).unwrap().1;
                s.add_label(*net, p);
            }
        }
        s
    }

    /// Metal1 plates under each PCell terminal label ("S", "D", "G"), left to right.
    fn plates(lay: &Layout, tech: &Tech) -> HashMap<String, Vec<IRect>> {
        let m1 = tech.layer_index("metal1").unwrap();
        let mut out: HashMap<String, Vec<IRect>> = HashMap::new();
        for s in &lay.shapes {
            if let Geometry::Label { text, at, .. } = &s.geom
                && let Some(r) = lay.shapes.iter().find(|q| q.layer == m1 && q.bbox().contains(at.0, at.1))
            {
                out.entry(text.clone()).or_default().push(r.bbox());
            }
        }
        for v in out.values_mut() {
            v.sort_by_key(|r| r.x0);
        }
        out
    }

    fn place(dst: &mut Layout, src: &Layout, dy: i64) {
        for s in &src.shapes {
            if !matches!(s.geom, Geometry::Label { .. }) {
                let mut s = s.clone();
                s.translate(0, dy);
                dst.add(s.layer, s.geom);
            }
        }
    }

    fn centre(r: IRect) -> (i64, i64) {
        ((r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2)
    }

    fn label(lay: &mut Layout, tech: &Tech, text: &str, at: (i64, i64)) {
        lay.add(tech.layer_index("metal1").unwrap(), Geometry::Label { text: text.into(), at, size: 100 });
    }

    /// Inverter from MOS PCells, wired in metal1 and labelled in / out / VDD / gnd.
    fn pcell_inverter(tech: &Tech, pw: i64) -> Layout {
        let n = PCell::mos(false, 1000, 180, 1).generate(tech).unwrap();
        let p = PCell::mos(true, pw, 180, 1).generate(tech).unwrap();
        let dy = 4000;
        let (pn, pp) = (plates(&n, tech), plates(&p, tech));
        let up = |r: IRect| r.translate(0, dy);
        let (nd, pd, ng, pg) = (pn["D"][0], up(pp["D"][0]), pn["G"][0], up(pp["G"][0]));
        let mut lay = Layout::default();
        place(&mut lay, &n, 0);
        place(&mut lay, &p, dy);
        let m1 = tech.layer_index("metal1").unwrap();
        let x = nd.x1.max(pd.x1) + 600;
        lay.add_rect(m1, nd.x0, nd.y0 + 100, x + 300, nd.y0 + 400);
        lay.add_rect(m1, x, nd.y0 + 100, x + 300, pd.y0 + 400);
        lay.add_rect(m1, pd.x0, pd.y0 + 100, x + 300, pd.y0 + 400);
        let xl = -1500;
        lay.add_rect(m1, xl, ng.y0, ng.x0 + 50, ng.y1);
        lay.add_rect(m1, xl, ng.y0, xl + 300, pg.y1);
        lay.add_rect(m1, xl, pg.y0, pg.x0 + 50, pg.y1);
        label(&mut lay, tech, "gnd", centre(pn["S"][0]));
        label(&mut lay, tech, "VDD", centre(up(pp["S"][0])));
        label(&mut lay, tech, "in", centre(ng));
        label(&mut lay, tech, "out", centre(nd));
        lay
    }

    fn inverter_schematic(pw: &str) -> Schematic {
        schematic(&[(DeviceKind::Pmos, pw, ["out", "in", "vdd"]), (DeviceKind::Nmos, "1u", ["out", "in", "0"])])
    }

    fn kinds(r: &LvsReport) -> Vec<LvsErrorKind> {
        r.errors.iter().map(|e| e.kind).collect()
    }

    #[test]
    fn demo_inverter_findings() {
        let tech = Tech::generic();
        let cell = crate::examples::cmos_inverter();
        let x = extract(cell.layout.as_ref().unwrap(), &NoCells, &tech);
        let r = compare(cell.schematic.as_ref().unwrap(), &x, &LvsOptions::default());
        assert!(!r.matched);
        assert_eq!((r.schematic_devices, r.layout_devices), (3, 2));
        assert_eq!(r.devices, [("M1".to_string(), "M2".to_string()), ("M2".into(), "M1".into())]);
        let nets: Vec<(&str, &str)> = r.nets.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        assert_eq!(nets, [("0", "gnd"), ("in", "in"), ("out", "out"), ("vdd", "vdd")]);
        assert_eq!(kinds(&r), [LvsErrorKind::MissingDevice, LvsErrorKind::Parameter]);
        assert!(r.errors[0].message.starts_with("schematic device C1 (capacitor C="), "{}", r.errors[0].message);
        assert_eq!(r.errors[1].message, "M1 / M2: W 2µm (schematic) vs 1.2µm (layout)");
        assert_eq!(r.errors[1].marker, Some(x.devices[1].gate));
        // Its n-well has no tap and the substrate none either: both float, which LVS reports.
        let floating: Vec<&str> =
            r.warnings.iter().filter(|w| w.message.contains("only device bodies")).map(|w| w.message.as_str()).collect();
        assert_eq!(floating.len(), 2, "{floating:?}");
        assert!(floating.iter().any(|m| m.contains("'nwell1'")) && floating.iter().any(|m| m.contains("'sub'")));
    }

    #[test]
    fn pcell_inverter_matches() {
        let tech = Tech::generic();
        let x = extract(&pcell_inverter(&tech, 2000), &NoCells, &tech);
        assert!(x.warnings.is_empty(), "{:?}", x.warnings);
        let r = compare(&inverter_schematic("2u"), &x, &LvsOptions::default());
        assert!(r.matched, "{:#?}", r.errors);
        assert_eq!(r.devices.len(), 2);
        assert_eq!(r.nets.len(), 4);
        assert!(r.nets.contains(&("vdd".into(), "VDD".into())));
    }

    #[test]
    fn unlabelled_nets_match_by_structure() {
        let tech = Tech::generic();
        let mut lay = pcell_inverter(&tech, 2000);
        lay.shapes.retain(|s| !matches!(&s.geom, Geometry::Label { text, .. } if text == "in" || text == "out"));
        let x = extract(&lay, &NoCells, &tech);
        let sch = schematic(&[(DeviceKind::Pmos, "2u", ["a", "net7", "vdd"]), (DeviceKind::Nmos, "1u", ["a", "net7", "gnd"])]);
        let r = compare(&sch, &x, &LvsOptions::default());
        assert!(r.matched, "{:#?}", r.errors);
        assert!(
            r.nets.contains(&("a".into(), "net2".into())) && r.nets.contains(&("net7".into(), "net1".into())),
            "{:?}",
            r.nets
        );
    }

    #[test]
    fn swapped_gate_and_drain() {
        let tech = Tech::generic();
        let x = extract(&pcell_inverter(&tech, 2000), &NoCells, &tech);
        let sch = schematic(&[(DeviceKind::Pmos, "2u", ["out", "in", "vdd"]), (DeviceKind::Nmos, "1u", ["in", "out", "0"])]);
        let r = compare(&sch, &x, &LvsOptions::default());
        assert!(!r.matched);
        assert!(r.errors.iter().all(|e| e.kind == LvsErrorKind::Net), "{:#?}", r.errors);
        assert!(
            r.errors.iter().any(|e| e.message.starts_with("M2 / M1: gate on 'out' in the schematic but 'in'")),
            "{:#?}",
            r.errors
        );
    }

    #[test]
    fn missing_device() {
        let tech = Tech::generic();
        let x = extract(&pcell_inverter(&tech, 2000), &NoCells, &tech);
        let sch = schematic(&[
            (DeviceKind::Pmos, "2u", ["out", "in", "vdd"]),
            (DeviceKind::Nmos, "1u", ["out", "in", "0"]),
            (DeviceKind::Nmos, "1u", ["out", "en", "x"]),
        ]);
        let r = compare(&sch, &x, &LvsOptions::default());
        assert_eq!(kinds(&r), [LvsErrorKind::MissingDevice], "{:#?}", r.errors);
        assert!(r.errors[0].message.contains("M3 (nmos W=1µm L=0.18µm)"), "{}", r.errors[0].message);
    }

    #[test]
    fn width_mismatch_and_extra_device() {
        let tech = Tech::generic();
        let x = extract(&pcell_inverter(&tech, 1500), &NoCells, &tech);
        let r = compare(&inverter_schematic("2u"), &x, &LvsOptions::default());
        assert_eq!(kinds(&r), [LvsErrorKind::Parameter]);
        assert_eq!(r.errors[0].message, "M1 / M2: W 2µm (schematic) vs 1.5µm (layout)");
        let loose = LvsOptions { tolerance: 0.3, ..LvsOptions::default() };
        assert!(compare(&inverter_schematic("2u"), &x, &loose).matched);

        let sch = schematic(&[(DeviceKind::Nmos, "1u", ["out", "in", "0"])]);
        let r = compare(&sch, &x, &LvsOptions::default());
        assert_eq!(kinds(&r), [LvsErrorKind::ExtraDevice], "{:#?}", r.errors);
        assert_eq!(r.errors[0].marker, Some(x.devices[1].gate));
    }

    #[test]
    fn fingers_merge_as_parallel_devices() {
        let tech = Tech::generic();
        let mut lay = PCell::mos(false, 1000, 180, 2).generate(&tech).unwrap();
        let s = plates(&lay, &tech)["S"].clone();
        let (a, b) = (s[0], s[1]);
        let m1 = tech.layer_index("metal1").unwrap();
        lay.add_rect(m1, a.x0, -800, a.x1, a.y0 + 10);
        lay.add_rect(m1, a.x0, -800, b.x1, -500);
        lay.add_rect(m1, b.x0, -800, b.x1, b.y0 + 10);
        let x = extract(&lay, &NoCells, &tech);
        assert_eq!(x.devices.len(), 2);
        let sch = schematic(&[(DeviceKind::Nmos, "2u", ["D", "G", "S"])]);
        let r = compare(&sch, &x, &LvsOptions::default());
        assert!(r.matched, "{:#?}", r.errors);
        assert_eq!(r.devices, [("M1".to_string(), "M1+M2".to_string())]);

        let mut m2 = schematic(&[(DeviceKind::Nmos, "1u", ["D", "G", "S"])]);
        m2.instances[0].params.insert("m".into(), "2".into());
        assert!(compare(&m2, &x, &LvsOptions::default()).matched);
    }

    /// Resistors / capacitors connected through labels on their pins: (kind, value, [p, n]).
    fn two_pins(devs: &[(DeviceKind, &str, [&str; 2])]) -> Schematic {
        let mut s = Schematic::default();
        for (k, (kind, v, nets)) in devs.iter().enumerate() {
            let id = s.add_instance(*kind, Pt::new(10 * k as i32, 0), Orient::default());
            let key = if *kind == DeviceKind::Resistor { "r" } else { "c" };
            s.instance_mut(id).unwrap().params.insert(key.into(), (*v).into());
            let pins = s.instance(id).unwrap().pin_positions();
            for (pin, net) in ["p", "n"].iter().zip(nets) {
                let p = pins.iter().find(|(n, _)| n == pin).unwrap().1;
                s.add_label(*net, p);
            }
        }
        s
    }

    #[test]
    fn resistor_value_and_reduction() {
        let tech = crate::extract::rc_tech();
        let x = extract(&PCell::PolyRes { w: 400, l: 5000 }.generate(&tech).unwrap(), &NoCells, &tech);
        let opts = LvsOptions::for_tech(&tech);
        let r = compare(&two_pins(&[(DeviceKind::Resistor, "93.75", ["B", "A"])]), &x, &opts);
        assert!(r.matched, "{:#?}", r.errors);
        assert_eq!(r.devices, [("R1".to_string(), "R1".to_string())]);
        assert_eq!((r.schematic_devices, r.layout_devices), (1, 1));

        let r = compare(&two_pins(&[(DeviceKind::Resistor, "100", ["A", "B"])]), &x, &opts);
        assert_eq!(kinds(&r), [LvsErrorKind::Parameter]);
        assert_eq!(r.errors[0].message, "R1 / R1: R=100 ohm (schematic) vs R=93.75 ohm (layout)");
        assert_eq!(r.errors[0].marker, Some(x.passives[0].body));
        assert!(
            compare(&two_pins(&[(DeviceKind::Resistor, "100", ["A", "B"])]), &x, &LvsOptions { tolerance: 0.1, ..opts.clone() })
                .matched
        );

        let series = two_pins(&[(DeviceKind::Resistor, "50", ["A", "net9"]), (DeviceKind::Resistor, "43.75", ["net9", "B"])]);
        let r = compare(&series, &x, &opts);
        assert!(r.matched, "{:#?}", r.errors);
        assert_eq!(r.devices, [("R1+R2".to_string(), "R1".to_string())]);
        let parallel = two_pins(&[(DeviceKind::Resistor, "187.5", ["A", "B"]), (DeviceKind::Resistor, "187.5", ["B", "A"])]);
        assert!(compare(&parallel, &x, &opts).matched);
        let tapped = two_pins(&[(DeviceKind::Resistor, "50", ["A", "mid"]), (DeviceKind::Resistor, "43.75", ["mid", "B"])]);
        assert!(!compare(&tapped, &x, &opts).matched, "a named net stops series reduction");

        let r = compare(&two_pins(&[(DeviceKind::Resistor, "93.75", ["A", "C"])]), &x, &opts);
        assert!(!r.matched && r.errors.iter().all(|e| e.kind == LvsErrorKind::Net), "{:#?}", r.errors);
    }

    #[test]
    fn capacitor_value_and_extra_device() {
        let tech = crate::extract::rc_tech();
        let x = extract(&PCell::MomCap { w: 5000, h: 5000 }.generate(&tech).unwrap(), &NoCells, &tech);
        let opts = LvsOptions::for_tech(&tech);
        assert!(compare(&two_pins(&[(DeviceKind::Capacitor, "25f", ["TOP", "BOT"])]), &x, &opts).matched);
        let two =
            two_pins(&[(DeviceKind::Capacitor, "12.51f", ["TOP", "BOT"]), (DeviceKind::Capacitor, "12.51f", ["BOT", "TOP"])]);
        assert!(compare(&two, &x, &opts).matched);

        let r = compare(&two_pins(&[(DeviceKind::Capacitor, "30f", ["TOP", "BOT"])]), &x, &opts);
        assert_eq!(kinds(&r), [LvsErrorKind::Parameter]);
        assert_eq!(r.errors[0].message, "C1 / C1: C=30fF (schematic) vs C=25.02fF (layout)");

        let r = compare(&two_pins(&[(DeviceKind::Resistor, "1k", ["TOP", "BOT"])]), &x, &opts);
        assert_eq!(kinds(&r), [LvsErrorKind::MissingDevice, LvsErrorKind::ExtraDevice], "{:#?}", r.errors);
        assert_eq!(r.errors[0].message, "schematic device R1 (resistor R=1k ohm) has no layout counterpart");
        assert_eq!(r.errors[1].message, "layout device C1 (cmom C=25.02fF) is not in the schematic");
    }

    #[test]
    fn bulk_terminals() {
        let tech = Tech::generic();
        let x = extract(&pcell_inverter(&tech, 2000), &NoCells, &tech);
        let opts = LvsOptions { ignore_bulk: false, ..LvsOptions::default() };
        let r = compare(&inverter_schematic("2u"), &x, &opts);
        assert!(!r.matched);
        assert!(r.errors.iter().any(|e| e.message.contains("bulk on 'vdd' in the schematic but 'nwell1'")), "{:#?}", r.errors);
    }

    /// A diffusion tap or emitter: `implant` diffusion with a contact and a metal1 pad labelled `net`.
    fn contacted(lay: &mut Layout, tech: &Tech, implant: &str, r: (i64, i64, i64, i64), net: &str) {
        let l = |n: &str| tech.layer_index(n).unwrap();
        let (x0, y0, x1, y1) = r;
        lay.add_rect(l("active"), x0, y0, x1, y1);
        lay.add_rect(l(implant), x0 - 200, y0 - 200, x1 + 200, y1 + 200);
        let (cx, cy) = ((x0 + x1) / 2, (y0 + y1) / 2);
        lay.add_rect(l("contact"), cx - 110, cy - 110, cx + 110, cy + 110);
        lay.add_rect(l("metal1"), cx - 200, cy - 200, cx + 200, cy + 200);
        label(lay, tech, net, (cx, cy));
    }

    /// Substrate taps on `vss` and `agnd`, and a vertical PNP: a marked p+ emitter (1 µm²) on
    /// `e` in an nwell tapped on `base`.
    fn pnp_layout(tech: &mut Tech) -> Layout {
        let mut m = crate::tech::LayerDef::generated(30, 0, tech.layers.len());
        m.name = "pnpmark".into();
        tech.layers.push(m);
        tech.bjts = vec![crate::tech::BjtDef {
            name: "pnp_vert".into(),
            kind: crate::tech::BjtType::Pnp,
            layers: vec!["pnpmark".into()],
            exclude: vec![],
            unit_area: Some(0.25),
        }];
        let mut lay = Layout::default();
        contacted(&mut lay, tech, "pplus", (0, 0, 500, 500), "vss");
        contacted(&mut lay, tech, "pplus", (0, 3000, 500, 3500), "vss");
        contacted(&mut lay, tech, "pplus", (2000, 0, 2500, 500), "agnd");
        let l = |n: &str| tech.layer_index(n).unwrap();
        lay.add_rect(l("metal1"), 50, 50, 450, 3450);
        lay.add_rect(l("nwell"), 4000, 0, 9000, 4000);
        contacted(&mut lay, tech, "nplus", (4600, 600, 5100, 1100), "base");
        contacted(&mut lay, tech, "pplus", (6500, 1500, 7500, 2500), "e");
        lay.add_rect(l("pnpmark"), 6000, 1000, 8000, 3000);
        lay
    }

    #[test]
    fn taps_are_soft_connections() {
        let mut tech = Tech::generic();
        let lay = pnp_layout(&mut tech);
        let x = extract(&lay, &NoCells, &tech);
        let net = |n: &str| x.nets.iter().position(|q| q.name == n).unwrap();
        // The substrate does not short vss and agnd; it is taken as vss (two taps against one).
        assert!(!x.warnings.iter().any(|w| w.contains("shorted")), "{:?}", x.warnings);
        assert_eq!(x.soft_conflicts.len(), 1);
        assert_eq!((x.soft_conflicts[0].well, x.soft_conflicts[0].nets.clone()), (None, vec![net("vss"), net("agnd")]));
        let q = &x.bjts[0];
        assert_eq!((q.c, q.b, q.e, q.model.as_str()), (net("vss"), net("base"), net("e"), "pnp_vert"));
        assert!((q.emitter_area - 1.0).abs() < 1e-9 && (q.area - 4.0).abs() < 1e-9);
        assert!(x.spice("t", 1000).contains("Q1 vss base e pnp_vert area=4\n"), "{}", x.spice("t", 1000));

        // LVS: a PNP of area 4 between the same nets matches; area and terminals are compared.
        let sch = |area: &str, e: &str| {
            let mut s = Schematic::default();
            let id = s.add_instance(DeviceKind::Pnp, Pt::new(0, 0), Orient::default());
            s.instance_mut(id).unwrap().params.insert("area".into(), area.into());
            let pins = s.instance(id).unwrap().pin_positions();
            for (pin, n) in [("c", "vss"), ("b", "base"), ("e", e)] {
                s.add_label(n, pins.iter().find(|(p, _)| *p == pin).unwrap().1);
            }
            s
        };
        let r = compare(&sch("4", "e"), &x, &LvsOptions::default());
        assert!(r.matched, "{:#?}", r.errors);
        assert!(
            r.warnings.iter().any(|w| w.message.contains("soft connection: the substrate is tapped from nets 'vss', 'agnd'"))
        );
        let r = compare(&sch("1", "e"), &x, &LvsOptions::default());
        assert!(r.errors.iter().any(|e| e.message.contains("area=4 (layout)")), "{:#?}", r.errors);
        let r = compare(&sch("4", "vss"), &x, &LvsOptions::default());
        assert!(!r.matched);

        // A second nwell tap on another net: the well is a soft-connect conflict too.
        let mut lay2 = lay.clone();
        contacted(&mut lay2, &tech, "nplus", (4600, 2600, 5100, 3100), "base2");
        let x = extract(&lay2, &NoCells, &tech);
        let well = x.soft_conflicts.iter().find(|c| c.well.is_some()).expect("well conflict");
        assert_eq!(well.nets.len(), 2);
        assert_ne!(x.nets.iter().position(|n| n.name == "base"), x.nets.iter().position(|n| n.name == "base2"));
    }

    #[test]
    fn triple_well_npn_and_isolated_nmos() {
        let mut tech = Tech::generic();
        for (k, name) in ["dnwell", "npnmark"].into_iter().enumerate() {
            let mut m = crate::tech::LayerDef::generated(40 + k as u16, 0, tech.layers.len());
            m.name = name.into();
            tech.layers.push(m);
        }
        tech.roles.dnwell = "dnwell".into();
        tech.bjts = vec![crate::tech::BjtDef {
            name: "npn_vert".into(),
            kind: crate::tech::BjtType::Npn,
            layers: vec!["npnmark".into()],
            exclude: vec![],
            unit_area: None,
        }];
        let l = |n: &str| tech.layer_index(n).unwrap();
        let mut lay = Layout::default();
        contacted(&mut lay, &tech, "pplus", (0, 0, 500, 500), "vss");
        lay.add_rect(l("dnwell"), 10000, 0, 16000, 6000);
        // An nwell strip over the deep n-well's edge, tapped on `coll`.
        lay.add_rect(l("nwell"), 9500, 0, 11000, 6000);
        contacted(&mut lay, &tech, "nplus", (10000, 500, 10500, 1000), "coll");
        // The isolated p-well: a tap on `pb`, an emitter on `ne` and an NMOS.
        contacted(&mut lay, &tech, "pplus", (12000, 500, 12500, 1000), "pb");
        contacted(&mut lay, &tech, "nplus", (13500, 2500, 14500, 3500), "ne");
        lay.add_rect(l("npnmark"), 13000, 2000, 15000, 4000);
        lay.add_rect(l("active"), 12000, 4500, 13000, 5500);
        lay.add_rect(l("nplus"), 11800, 4300, 13200, 5700);
        lay.add_rect(l("poly"), 12410, 4300, 12590, 5700);
        let x = extract(&lay, &NoCells, &tech);
        let net = |n: &str| x.nets.iter().position(|q| q.name == n).unwrap();
        assert_eq!(x.bjts.len(), 1, "{:?}", x.warnings);
        let q = &x.bjts[0];
        assert!(!q.pnp);
        assert_eq!((q.c, q.b, q.e), (net("coll"), net("pb"), net("ne")));
        assert_eq!(x.devices[0].b, net("pb"), "the NMOS body is the isolated p-well, not the substrate");
        assert!(x.soft_conflicts.is_empty());

        let mut sch = Schematic::default();
        let id = sch.add_instance(DeviceKind::Npn, Pt::new(0, 0), Orient::default());
        let pins = sch.instance(id).unwrap().pin_positions();
        for (pin, n) in [("c", "coll"), ("b", "pb"), ("e", "ne")] {
            sch.add_label(n, pins.iter().find(|(p, _)| *p == pin).unwrap().1);
        }
        let opts = LvsOptions { filter_off: false, ..LvsOptions::default() };
        let r = compare(&sch, &x, &opts);
        assert!(r.devices.contains(&("Q1".into(), "Q1".into())), "{:#?} {:#?}", r.devices, r.errors);
        assert!(!r.errors.iter().any(|e| e.message.contains("Q1")), "{:#?}", r.errors);
    }

    #[test]
    fn schematic_ggnmos_needs_an_off_layout_device() {
        // An ESD clamp: drain on the pad, gate, source and body on ground. Off devices are left
        // out of the matching, but this one must still be in the layout.
        let sch = schematic(&[(DeviceKind::Nmos, "10u", ["pad", "0", "0"])]);
        let finger = |x: i64| crate::extract::Device {
            kind: DevKind::Nmos,
            model: "nmos".into(),
            name: format!("M{x}"),
            w: 5000,
            l: 180,
            d: 0,
            g: 1,
            s: 1,
            b: 1,
            gate: IRect::new(x, 0, x + 180, 5000),
            jd: Default::default(),
            js: Default::default(),
            od: None,
        };
        let net = |n: &str| crate::extract::Net { name: n.into(), labelled: true, shapes: vec![], bbox: None };
        let lay = |fingers: &[i64]| Extraction {
            nets: vec![net("pad"), net("gnd")],
            devices: fingers.iter().map(|&x| finger(x)).collect(),
            dbu_per_um: 1000,
            ..Default::default()
        };
        let r = compare(&sch, &lay(&[0, 1000]), &LvsOptions::default());
        assert!(r.matched, "{:#?}", r.errors);
        let r = compare(&sch, &lay(&[0]), &LvsOptions::default());
        assert_eq!(kinds(&r), [LvsErrorKind::Parameter], "{:#?}", r.errors);
        assert!(r.errors[0].message.contains("W 10µm (schematic) vs 5µm"), "{}", r.errors[0].message);
        let r = compare(&sch, &lay(&[]), &LvsOptions::default());
        assert_eq!(kinds(&r), [LvsErrorKind::MissingDevice], "{:#?}", r.errors);
        // A plain dummy (every pin on one net) needs no layout device.
        let dummy = schematic(&[(DeviceKind::Nmos, "1u", ["0", "0", "0"])]);
        assert!(compare(&dummy, &lay(&[]), &LvsOptions::default()).matched);
    }
}
