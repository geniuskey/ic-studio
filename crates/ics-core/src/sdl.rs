//! Schematic-driven layout: device placement from a schematic and unrouted-connection flylines.
//!
//! Placement puts every MOS device as a PCell instance named after its schematic instance, in two
//! rows: PMOS (mirrored, gate contacts facing down) above NMOS (gate contacts facing up).
//! - Each row is built as chains: starting from the next device in schematic order, a device of
//!   the same type and finger width whose outer source / drain is on the net of the chain's last
//!   column is flipped as needed and abutted, sharing that diffusion column.
//! - Chains are spaced by the largest spacing of the layers other than wells and implants; the
//!   row's well and implant shapes are merged by row-wide fill rectangles.
//! - A bulk tap closes each row: n+ in the nwell for PMOS, p+ substrate for NMOS, named
//!   [`NTAP`] / [`PTAP`] + the row's bulk net, so flylines and the router connect it.
//! - Matched groups ([`matched_groups`]: differential pairs, current mirrors, or devices sharing a
//!   `match` parameter value) are split into equal unit fingers placed as instances named
//!   `<device>.<k>` in common-centroid order (e.g. ABBA), abutted where their nets allow, at the
//!   start of their row. LVS merges the units back into their devices.
//!
//! The placement is DRC clean by construction.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::connectivity::{self, Connectivity};
use crate::extract;
use crate::geom::IRect;
use crate::hier::{LayoutSource, flatten};
use crate::layout::{Geometry, Layout, Trans};
use crate::pcell::{IMPLANT_ENC, PCell, mos_columns};
use crate::project::Project;
use crate::schematic::{DeviceKind, Instance, Schematic};
use crate::tech::Tech;
use crate::units::parse_spice;

/// Devices wider than this (µm) are split into equal fingers.
pub const MAX_FINGER_W_UM: f64 = 5.0;
/// Instance name prefix of the nwell tap of the PMOS row, followed by its net (e.g. `ntap:vdd`).
pub const NTAP: &str = "ntap:";
/// Instance name prefix of the substrate tap of the NMOS row, followed by its net.
pub const PTAP: &str = "ptap:";
/// Instance parameter grouping devices to match (same value = one group; `none` opts out of the
/// automatic grouping).
pub const MATCH_PARAM: &str = "match";
/// Most unit fingers a matched group is split into.
const MAX_UNITS: usize = 32;
/// Layers merged across a row instead of being spaced.
const SOFT_LAYERS: [&str; 3] = ["nwell", "nplus", "pplus"];

/// A schematic device (or row tap) to be placed as a PCell instance named after it.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedDevice {
    pub name: String,
    pub pcell: PCell,
    pub trans: Trans,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SdlPlan {
    pub devices: Vec<PlannedDevice>,
    /// Row-wide well and implant rectangles (layer, rect) covering those of the row's cells.
    pub fills: Vec<(usize, IRect)>,
    /// Schematic devices with no generator (sources, R, C, ...), with the reason.
    pub skipped: Vec<String>,
    /// How matched groups were placed (or why not).
    pub notes: Vec<String>,
}

/// An unrouted connection between two device terminals of the same schematic net.
#[derive(Debug, Clone, PartialEq)]
pub struct Flyline {
    pub net: String,
    pub a: (i64, i64),
    pub b: (i64, i64),
}

struct Candidate {
    name: String,
    pcell: PCell,
    pmos: bool,
    /// Finger width (dbu).
    w: i64,
    /// Source / drain column centres (cell coordinates) and their schematic nets.
    cols: Vec<(i64, Option<String>)>,
    bulk: Option<String>,
    lay: Layout,
    /// Extent without well and implant layers (cell coordinates).
    core: IRect,
    space: i64,
}

impl Candidate {
    /// Nets of the left and right outer columns.
    fn ends(&self, flip: bool) -> (Option<&str>, Option<&str>) {
        let (a, b) = (self.cols.first().and_then(|c| c.1.as_deref()), self.cols.last().and_then(|c| c.1.as_deref()));
        if flip { (b, a) } else { (a, b) }
    }
}

fn snap(v: i64, g: i64) -> i64 {
    if g <= 1 { v } else { (v + g.div_euclid(2)).div_euclid(g) * g }
}

fn snap_up(v: i64, g: i64) -> i64 {
    if g <= 1 { v } else { (v + g - 1).div_euclid(g) * g }
}

fn layer_space(tech: &Tech, layer: usize) -> i64 {
    tech.layer(layer).map_or(0, |l| l.min_spacing)
}

fn soft_layers(tech: &Tech) -> Vec<usize> {
    SOFT_LAYERS.iter().filter_map(|n| tech.layer_index(n)).collect()
}

/// PCell for a schematic MOS instance: `w` is per device and `m` its multiplier. Each of the
/// `m` devices is split into `ceil(w / MAX_FINGER_W_UM)` fingers, keeping the total width.
pub fn mos_pcell(inst: &Instance, tech: &Tech) -> Result<PCell, String> {
    let dbu = |key: &str| -> Result<i64, String> {
        let raw = inst.param(key);
        let v = parse_spice(raw).filter(|v| v.is_finite() && *v > 0.0).ok_or_else(|| format!("invalid {key} '{raw}'"))?;
        Ok(snap((v * 1e6 * tech.dbu_per_um as f64).round() as i64, tech.grid))
    };
    let (w, l) = (dbu("w")?, dbu("l")?);
    let m = match inst.param("m").trim() {
        "" => 1,
        raw => parse_spice(raw)
            .filter(|v| *v >= 1.0 && v.fract() == 0.0 && *v <= u32::MAX as f64)
            .ok_or_else(|| format!("invalid multiplier '{raw}'"))? as u32,
    };
    let max_w = (MAX_FINGER_W_UM * tech.dbu_per_um as f64) as i64;
    let split = if max_w > 0 { ((w + max_w - 1) / max_w).max(1) } else { 1 };
    let fingers = u32::try_from(split).ok().and_then(|s| s.checked_mul(m)).ok_or("too many fingers")?;
    Ok(PCell::Mos { pmos: inst.kind == DeviceKind::Pmos, w: snap(w / split, tech.grid), l, fingers })
}

/// The schematic device a layout instance belongs to: units of matched devices are named
/// `<device>.<k>`.
pub fn device_of(instance: &str) -> &str {
    match instance.rsplit_once('.') {
        Some((d, k)) if !d.is_empty() && !k.is_empty() && k.bytes().all(|b| b.is_ascii_digit()) => d,
        _ => instance,
    }
}

/// Groups of MOS devices to place matched (instance names, schematic order within a group):
/// devices sharing a [`MATCH_PARAM`] value; differential pairs (same type, W, L and m, a common
/// source net that is not a supply rail, different gates and drains); current mirrors (same
/// type and L, common gate and source, the gate tied to the drain of one of them).
pub fn matched_groups(sch: &Schematic) -> Vec<Vec<String>> {
    let conn = connectivity::extract(sch);
    let mos: Vec<&Instance> = sch.instances.iter().filter(|i| matches!(i.kind, DeviceKind::Nmos | DeviceKind::Pmos)).collect();
    let rails: HashSet<&str> = sch
        .instances
        .iter()
        .filter(|i| matches!(i.kind, DeviceKind::Ground | DeviceKind::Supply | DeviceKind::VSource))
        .flat_map(|i| i.pin_positions().into_iter().filter_map(|(pin, _)| conn.net_of(i.id, pin)))
        .collect();
    let net = |i: &Instance, pin: &str| conn.net_of(i.id, pin);
    let val = |i: &Instance, key: &str| parse_spice(i.param(key));
    let mult = |i: &Instance| val(i, "m").unwrap_or(1.0);
    let tag = |i: &Instance| i.param(MATCH_PARAM).trim().to_string();
    let mut uf: Vec<usize> = (0..mos.len()).collect();
    fn root(uf: &mut [usize], mut i: usize) -> usize {
        while uf[i] != i {
            i = uf[i];
        }
        i
    }
    for a in 0..mos.len() {
        for b in a + 1..mos.len() {
            let (x, y) = (mos[a], mos[b]);
            if x.kind != y.kind || tag(x) == "none" || tag(y) == "none" {
                continue;
            }
            let explicit = !tag(x).is_empty() && tag(x) == tag(y);
            let same_l = val(x, "l").is_some() && val(x, "l") == val(y, "l");
            let source = net(x, "s").filter(|s| net(y, "s") == Some(s));
            let diff_pair = same_l
                && val(x, "w") == val(y, "w")
                && mult(x) == mult(y)
                && source.is_some_and(|s| !rails.contains(s))
                && net(x, "g") != net(y, "g")
                && net(x, "d") != net(y, "d");
            let gate = net(x, "g").filter(|g| net(y, "g") == Some(g));
            let mirror = same_l && source.is_some() && gate.is_some_and(|g| net(x, "d") == Some(g) || net(y, "d") == Some(g));
            let auto = tag(x).is_empty() && tag(y).is_empty() && (diff_pair || mirror);
            if explicit || auto {
                let (ra, rb) = (root(&mut uf, a), root(&mut uf, b));
                uf[ra.max(rb)] = ra.min(rb);
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (i, inst) in mos.iter().enumerate() {
        let r = root(&mut uf, i);
        groups.entry(r).or_default().push(inst.name.clone());
    }
    groups.into_values().filter(|g| g.len() > 1).collect()
}

/// Common-centroid order of units for devices with `counts` units each: half of every device's
/// units interleaved, the odd units in the middle, then the half mirrored.
pub fn common_centroid(counts: &[usize]) -> Vec<usize> {
    let mut left: Vec<usize> = counts.iter().map(|c| c / 2).collect();
    let mut half = Vec::new();
    while left.iter().any(|&c| c > 0) {
        for (i, c) in left.iter_mut().enumerate() {
            if *c > 0 {
                half.push(i);
                *c -= 1;
            }
        }
    }
    let mut out = half.clone();
    out.extend((0..counts.len()).filter(|&i| counts[i] % 2 == 1));
    out.extend(half.iter().rev());
    out
}

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Unit candidates of a matched group in common-centroid order, and the pattern (A = first
/// member, ...). Every member gets at least two units when the unit width allows.
fn matched_units(group: &[&Instance], conn: &Connectivity, tech: &Tech) -> Result<(Vec<Candidate>, String), String> {
    let mut cells = Vec::new();
    for inst in group {
        let PCell::Mos { pmos, w, l, fingers } = mos_pcell(inst, tech)? else { unreachable!("MOS cell") };
        cells.push((inst, pmos, w * fingers as i64, l));
    }
    let g = tech.grid.max(1);
    let fits =
        |w: i64| w % g == 0 && cells.iter().all(|c| PCell::Mos { pmos: c.1, w, l: c.3, fingers: 1 }.generate(tech).is_ok());
    let mut unit = cells.iter().map(|c| c.2).fold(0, gcd);
    let max_w = (MAX_FINGER_W_UM * tech.dbu_per_um as f64) as i64;
    while unit > max_w && unit % 2 == 0 && fits(unit / 2) {
        unit /= 2;
    }
    while cells.iter().any(|c| c.2 / unit < 2) && unit % 2 == 0 && fits(unit / 2) {
        unit /= 2;
    }
    if !fits(unit) || unit > max_w {
        return Err(format!("no common unit width (gcd {} µm)", unit as f64 / tech.dbu_per_um as f64));
    }
    let counts: Vec<usize> = cells.iter().map(|c| (c.2 / unit) as usize).collect();
    if counts.iter().sum::<usize>() > MAX_UNITS {
        return Err(format!("more than {MAX_UNITS} units"));
    }
    let mut units: Vec<Vec<Candidate>> = Vec::new();
    for (inst, pmos, _, l) in &cells {
        let n = (cells.iter().find(|c| c.0.name == inst.name).map_or(0, |c| c.2) / unit) as usize;
        let mut list = Vec::new();
        for k in (1..=n).rev() {
            let pcell = PCell::Mos { pmos: *pmos, w: unit, l: *l, fingers: 1 };
            list.push(candidate_with(inst, conn, tech, pcell, format!("{}.{k}", inst.name))?);
        }
        units.push(list);
    }
    let order = common_centroid(&counts);
    let pattern: String = order.iter().map(|&i| (b'A' + (i % 26) as u8) as char).collect();
    Ok((order.into_iter().filter_map(|i| units[i].pop()).collect(), pattern))
}

/// Flips and abutments for units in a fixed order: each unit is turned to share its left column
/// with the previous one when their nets allow, else towards the next unit's nets.
fn arrange(units: Vec<Candidate>) -> Vec<(Candidate, bool, bool)> {
    let plan = |first: bool| {
        let mut prev: Option<&str> = None;
        let mut flags = Vec::new();
        for (i, u) in units.iter().enumerate() {
            let (l, r) = u.ends(false);
            let towards = |net: Option<&str>| {
                units.get(i + 1).is_some_and(|n| net.is_some() && (n.ends(false).0 == net || n.ends(false).1 == net))
            };
            let (flip, abut) = match prev {
                _ if i == 0 => (first, false),
                Some(p) if l == Some(p) && units[i - 1].w == u.w => (false, true),
                Some(p) if r == Some(p) && units[i - 1].w == u.w => (true, true),
                _ => (towards(l) && !towards(r), false),
            };
            prev = u.ends(flip).1;
            flags.push((flip, abut));
        }
        flags
    };
    let (a, b) = (plan(false), plan(true));
    let shared = |f: &[(bool, bool)]| f.iter().filter(|x| x.1).count();
    let flags = if shared(&b) > shared(&a) { b } else { a };
    units.into_iter().zip(flags).map(|(u, (f, a))| (u, f, a)).collect()
}

fn candidate(inst: &Instance, conn: &Connectivity, tech: &Tech) -> Result<Candidate, String> {
    candidate_with(inst, conn, tech, mos_pcell(inst, tech)?, inst.name.clone())
}

fn candidate_with(inst: &Instance, conn: &Connectivity, tech: &Tech, pcell: PCell, name: String) -> Result<Candidate, String> {
    let PCell::Mos { w, l, fingers, .. } = pcell else { unreachable!("MOS cells only") };
    let lay = pcell.generate(tech)?;
    let net = |pin: &str| conn.net_of(inst.id, pin).map(str::to_string);
    let cols = mos_columns(tech, l, fingers)?
        .into_iter()
        .enumerate()
        .map(|(k, x)| (x, if k % 2 == 0 { net("s") } else { net("d") }))
        .collect();
    let soft = soft_layers(tech);
    let hard = lay.shapes.iter().filter(|s| !soft.contains(&s.layer) && !matches!(s.geom, Geometry::Label { .. }));
    let core = hard.clone().map(|s| s.bbox()).reduce(|a, b| a.union(&b)).ok_or("the generator produced no shapes")?;
    let space = hard.map(|s| layer_space(tech, s.layer)).max().unwrap_or(0);
    let bulk_pin = inst.pin_positions().into_iter().find(|(n, _)| *n == "b").map(|(_, p)| p);
    let bulk = if bulk_pin.is_some_and(|p| conn.unconnected_pins.contains(&p)) { None } else { net("b") };
    Ok(Candidate { name, pcell, pmos: inst.kind == DeviceKind::Pmos, w, cols, bulk, lay, core, space })
}

/// Orientation: PMOS mirrored top-bottom; `flip` also mirrors left-right.
fn orient(pmos: bool, flip: bool) -> Trans {
    match (pmos, flip) {
        (false, false) => Trans::IDENTITY,
        (false, true) => Trans::with(0, 0, 2, true),
        (true, false) => Trans::with(0, 0, 0, true),
        (true, true) => Trans::with(0, 0, 2, false),
    }
}

/// Chains of the row in placement order: (device, flipped, abutted to the previous one). Matched
/// blocks come first, in their own order.
fn chain(blocks: Vec<Vec<Candidate>>, row: Vec<Candidate>) -> Vec<(Candidate, bool, bool)> {
    let mut rest: Vec<Option<Candidate>> = row.into_iter().map(Some).collect();
    let mut out: Vec<(Candidate, bool, bool)> = blocks.into_iter().flat_map(arrange).collect();
    while let Some(i) = rest.iter().position(Option::is_some) {
        let c = rest[i].take().expect("present");
        let partner = |net: Option<&str>, rest: &[Option<Candidate>]| {
            net.is_some() && rest.iter().flatten().any(|o| o.w == c.w && (o.ends(false).0 == net || o.ends(false).1 == net))
        };
        let (l, r) = c.ends(false);
        let flip = !partner(r, &rest) && partner(l, &rest);
        out.push((c, flip, false));
        loop {
            let (last, flip, _) = out.last().expect("chain start");
            let Some(net) = last.ends(*flip).1 else { break };
            let w = last.w;
            let next = rest.iter().position(|o| {
                o.as_ref().is_some_and(|o| o.w == w && (o.ends(false).0 == Some(net) || o.ends(false).1 == Some(net)))
            });
            let Some(j) = next else { break };
            let o = rest[j].take().expect("present");
            let flip = o.ends(false).0 != Some(net);
            out.push((o, flip, true));
        }
    }
    out
}

/// A placed row (core bottom at y = 0): its devices and well / implant fills.
struct Row {
    devices: Vec<PlannedDevice>,
    /// Generated layouts of the devices, for extents.
    layouts: Vec<Layout>,
}

fn place_row(blocks: Vec<Vec<Candidate>>, row: Vec<Candidate>, tech: &Tech) -> Row {
    let g = tech.grid;
    let mut out = Row { devices: Vec::new(), layouts: Vec::new() };
    let mut prev: Option<(i64, i64, i64)> = None;
    let mut bulk: HashMap<String, usize> = HashMap::new();
    let (mut pmos, mut min_w, mut active_y0) = (false, i64::MAX, None);
    let active = tech.layer_index("active");
    for (c, flip, abut) in chain(blocks, row) {
        let t0 = orient(c.pmos, flip);
        let cb = t0.apply_rect(c.core);
        let xs: Vec<i64> = c.cols.iter().map(|(x, _)| t0.apply((*x, 0)).0).collect();
        let (left, right) = (xs.iter().copied().min().unwrap_or(0), xs.iter().copied().max().unwrap_or(0));
        let tx = match prev {
            None => -cb.x0,
            Some((_, rc, _)) if abut => rc - left,
            Some((x1, _, sp)) => x1 + snap_up(sp.max(c.space), g) - cb.x0,
        };
        let trans = Trans { x: tx, y: -cb.y0, ..t0 };
        prev = Some((tx + cb.x1, tx + right, c.space));
        if let Some(b) = &c.bulk {
            *bulk.entry(b.clone()).or_default() += 1;
        }
        (pmos, min_w) = (c.pmos, min_w.min(c.w));
        if active_y0.is_none() {
            active_y0 = c.lay.shapes.iter().filter(|s| Some(s.layer) == active).map(|s| trans.apply_rect(s.bbox()).y0).min();
        }
        out.devices.push(PlannedDevice { name: c.name, pcell: c.pcell, trans });
        out.layouts.push(c.lay);
    }
    let net = bulk.into_iter().max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0))).map(|(n, _)| n);
    if let (Some(net), Some((x1, _, sp)), Some(ay)) = (net, prev, active_y0) {
        let tap = PCell::Tap { well: pmos, w: min_w };
        if let Ok(lay) = tap.generate(tech) {
            let x = x1 + snap_up(sp.max(2 * snap_up(IMPLANT_ENC, g)), g);
            let name = format!("{}{net}", if pmos { NTAP } else { PTAP });
            out.devices.push(PlannedDevice { name, pcell: tap, trans: Trans::new(x, ay) });
            out.layouts.push(lay);
        }
    }
    out
}

impl Row {
    fn extent(&self) -> Option<IRect> {
        self.devices
            .iter()
            .zip(&self.layouts)
            .filter_map(|(d, l)| l.shapes_bbox().map(|b| d.trans.apply_rect(b)))
            .reduce(|a, b| a.union(&b))
    }

    fn fills(&self, tech: &Tech) -> Vec<(usize, IRect)> {
        let mut by_layer: BTreeMap<usize, (IRect, usize)> = BTreeMap::new();
        for (d, lay) in self.devices.iter().zip(&self.layouts) {
            let soft = soft_layers(tech);
            for s in lay.shapes.iter().filter(|s| soft.contains(&s.layer)) {
                let r = d.trans.apply_rect(s.bbox());
                let e = by_layer.entry(s.layer).or_insert((r, 0));
                (e.0, e.1) = (e.0.union(&r), e.1 + 1);
            }
        }
        by_layer.into_iter().filter(|(_, (_, n))| *n > 1).map(|(l, (r, _))| (l, r)).collect()
    }

    fn shift(&mut self, dy: i64) {
        for d in &mut self.devices {
            d.trans.y += dy;
        }
    }
}

/// Map schematic MOS devices to PCells and arrange them in rows (PMOS above NMOS), sharing
/// diffusion where nets allow, with a bulk tap per row.
pub fn plan(sch: &Schematic, tech: &Tech) -> SdlPlan {
    let mut plan = SdlPlan::default();
    let conn = connectivity::extract(sch);
    let mut mos: Vec<&Instance> = Vec::new();
    for inst in &sch.instances {
        match inst.kind {
            DeviceKind::Nmos | DeviceKind::Pmos => mos.push(inst),
            DeviceKind::Ground | DeviceKind::Supply => {}
            k => plan.skipped.push(format!("{} ({}): no layout generator", inst.name, k.label().to_lowercase())),
        }
    }
    mos.sort_by(|a, b| (a.pos.x, a.pos.y, &a.name).cmp(&(b.pos.x, b.pos.y, &b.name)));
    let (mut pblocks, mut nblocks) = (Vec::new(), Vec::new());
    let mut matched: HashSet<String> = HashSet::new();
    for names in matched_groups(sch) {
        let group: Vec<&Instance> = names.iter().filter_map(|n| mos.iter().find(|i| &i.name == n).copied()).collect();
        match matched_units(&group, &conn, tech) {
            Ok((units, pattern)) => {
                let legend: Vec<String> =
                    group.iter().enumerate().map(|(k, i)| format!("{} = {}", (b'A' + (k % 26) as u8) as char, i.name)).collect();
                plan.notes.push(format!("{}: matched as {pattern} ({})", names.join(", "), legend.join(", ")));
                matched.extend(names);
                if group[0].kind == DeviceKind::Pmos { &mut pblocks } else { &mut nblocks }.push(units);
            }
            Err(e) => plan.notes.push(format!("{}: not matched, {e}", names.join(", "))),
        }
    }
    let (mut prow, mut nrow) = (Vec::new(), Vec::new());
    for inst in mos.into_iter().filter(|i| !matched.contains(&i.name)) {
        match candidate(inst, &conn, tech) {
            Ok(c) if c.pmos => prow.push(c),
            Ok(c) => nrow.push(c),
            Err(e) => plan.skipped.push(format!("{}: {e}", inst.name)),
        }
    }
    let row_gap = snap_up(tech.layers.iter().map(|l| l.min_spacing).max().unwrap_or(0), tech.grid);
    let nrow = place_row(nblocks, nrow, tech);
    let mut prow = place_row(pblocks, prow, tech);
    if let (Some(n), Some(p)) = (nrow.extent(), prow.extent()) {
        prow.shift(n.y1 + row_gap - p.y0);
    }
    for row in [prow, nrow] {
        plan.fills.extend(row.fills(tech));
        plan.devices.extend(row.devices);
    }
    plan
}

fn flat_bbox(layout: &Layout, src: &dyn LayoutSource) -> Option<IRect> {
    flatten(layout, src, 64).iter().map(|s| s.geom.bbox()).reduce(|a, b| a.union(&b))
}

/// Place the planned devices that are not in `layout` yet (matched by [`device_of`] their
/// instance names, so a device placed whole is not placed again as units). Existing
/// named instances are left where they are. Returns the names placed. The well / implant fills
/// are added only when every planned device is new.
///
/// When the new devices would come closer than the technology's largest spacing to existing
/// content, they are all shifted to the right of it (bottom aligned).
pub fn apply(project: &mut Project, layout: &mut Layout, plan: &SdlPlan) -> Result<Vec<String>, String> {
    let existing: HashSet<&str> = layout.instances.iter().filter_map(|i| i.name.as_deref().map(device_of)).collect();
    let mut todo = Vec::new();
    for d in plan.devices.iter().filter(|d| !existing.contains(device_of(&d.name))) {
        let cell = project.ensure_pcell(&d.pcell)?;
        let bbox =
            project.cell(&cell).and_then(|c| c.layout.as_ref()).and_then(Layout::shapes_bbox).map(|b| d.trans.apply_rect(b));
        todo.push((d, cell, bbox));
    }
    let fresh = !todo.is_empty() && todo.len() == plan.devices.len();
    let fills: &[(usize, IRect)] = if fresh { &plan.fills } else { &[] };
    let new_bbox = todo.iter().filter_map(|t| t.2).chain(fills.iter().map(|f| f.1)).reduce(|a, b| a.union(&b));
    let gap = snap_up(project.tech.layers.iter().map(|l| l.min_spacing).max().unwrap_or(0), project.tech.grid);
    let (dx, dy) = match (flat_bbox(layout, project), new_bbox) {
        (Some(e), Some(n)) if e.spacing(&n) < gap || e.intersects(&n) => (e.x1 + gap - n.x0, e.y0 - n.y0),
        _ => (0, 0),
    };
    let mut placed = Vec::with_capacity(todo.len());
    for (d, cell, _) in todo {
        let id = layout.add_instance(cell, Trans { x: d.trans.x + dx, y: d.trans.y + dy, ..d.trans }, None);
        if let Some(inst) = layout.instance_mut(id) {
            inst.name = Some(d.name.clone());
        }
        placed.push(d.name.clone());
    }
    for &(l, r) in fills {
        layout.add(l, Geometry::Rect(r.translate(dx, dy)));
    }
    Ok(placed)
}

/// Device terminals per schematic net: the transformed centres of the "D", "G" and "S" labels
/// of each named layout instance matching a schematic MOS instance (MOS bulks are not
/// terminals), and the "B" label of each row tap ([`NTAP`] / [`PTAP`] + net).
pub fn terminals(sch: &Schematic, layout: &Layout, src: &dyn LayoutSource) -> BTreeMap<String, Vec<(i64, i64)>> {
    terminal_pins(sch, layout, src).into_iter().map(|(net, pins)| (net, pins.into_iter().map(|(_, p)| p).collect())).collect()
}

/// A device terminal: its pin ("d", "g", "s", or "b" for a tap) and position.
pub type Pin = (&'static str, (i64, i64));

/// [`terminals`] with the pin of each point.
pub fn terminal_pins(sch: &Schematic, layout: &Layout, src: &dyn LayoutSource) -> BTreeMap<String, Vec<Pin>> {
    let conn = connectivity::extract(sch);
    let mut out: BTreeMap<String, Vec<Pin>> = BTreeMap::new();
    for inst in sch.instances.iter().filter(|i| matches!(i.kind, DeviceKind::Nmos | DeviceKind::Pmos)) {
        for placed in layout.instances.iter().filter(|i| i.name.as_deref().is_some_and(|n| device_of(n) == inst.name)) {
            let Some(child) = src.layout(&placed.cell) else { continue };
            let (cols, rows) = placed.dims();
            for s in &child.shapes {
                let Geometry::Label { text, .. } = &s.geom else { continue };
                let pin = match text.as_str() {
                    "D" => "d",
                    "G" => "g",
                    "S" => "s",
                    _ => continue,
                };
                let Some(net) = conn.net_of(inst.id, pin) else { continue };
                let b = s.geom.bbox();
                let c = ((b.x0 + b.x1) / 2, (b.y0 + b.y1) / 2);
                let pts = out.entry(net.to_string()).or_default();
                for j in 0..rows {
                    for i in 0..cols {
                        pts.push((pin, placed.element_trans(i, j).apply(c)));
                    }
                }
            }
        }
    }
    for placed in &layout.instances {
        let Some(name) = placed.name.as_deref() else { continue };
        let Some(net) = name.strip_prefix(NTAP).or_else(|| name.strip_prefix(PTAP)).filter(|n| !n.is_empty()) else { continue };
        let Some(child) = src.layout(&placed.cell) else { continue };
        for s in &child.shapes {
            if matches!(&s.geom, Geometry::Label { text, .. } if text == "B") {
                let b = s.geom.bbox();
                let c = placed.trans.apply(((b.x0 + b.x1) / 2, (b.y0 + b.y1) / 2));
                out.entry(net.to_string()).or_default().push(("b", c));
            }
        }
    }
    out
}

/// Extracted net of a device terminal at `p`: the contact metal there (terminal labels sit on a
/// text layer, and wires of other nets may pass above), else the topmost conductor.
pub fn pin_net(ex: &extract::Extraction, tech: &Tech, p: (i64, i64)) -> Option<usize> {
    tech.via("contact").and_then(|v| tech.layer_index(&v.top)).and_then(|l| ex.net_at(l, p)).or_else(|| ex.net_at_any(p))
}

type Pt2 = (i64, i64);

fn dist2(a: Pt2, b: Pt2) -> i128 {
    let (dx, dy) = ((a.0 - b.0) as i128, (a.1 - b.1) as i128);
    dx * dx + dy * dy
}

/// Flylines for schematic nets whose device terminals are not yet connected in the layout.
///
/// Terminals of a net are grouped by the extracted layout net they sit on (a terminal on no
/// conductor is a group of its own); the groups are joined by a minimum spanning tree whose
/// edges are the closest terminal pairs between groups.
pub fn ratsnest(sch: &Schematic, layout: &Layout, src: &dyn LayoutSource, tech: &Tech) -> Vec<Flyline> {
    let terms = terminals(sch, layout, src);
    if terms.values().all(|t| t.len() < 2) {
        return Vec::new();
    }
    let ex = extract::extract(layout, src, tech);
    let mut out = Vec::new();
    for (net, pts) in terms {
        let mut groups: Vec<Vec<(i64, i64)>> = Vec::new();
        let mut by_net: HashMap<usize, usize> = HashMap::new();
        for p in pts {
            match pin_net(&ex, tech, p) {
                Some(n) if by_net.contains_key(&n) => groups[by_net[&n]].push(p),
                found => {
                    if let Some(n) = found {
                        by_net.insert(n, groups.len());
                    }
                    groups.push(vec![p]);
                }
            }
        }
        let mut in_tree = vec![false; groups.len()];
        if let Some(first) = in_tree.first_mut() {
            *first = true;
        }
        for _ in 1..groups.len() {
            let mut best: Option<(i128, usize, Pt2, Pt2)> = None;
            for a in groups.iter().zip(&in_tree).filter(|(_, t)| **t).map(|(g, _)| g) {
                for (gj, b) in groups.iter().enumerate().filter(|(j, _)| !in_tree[*j]) {
                    for &p in a {
                        for &q in b {
                            let d = dist2(p, q);
                            if best.is_none_or(|(bd, ..)| d < bd) {
                                best = Some((d, gj, p, q));
                            }
                        }
                    }
                }
            }
            let Some((_, gj, a, b)) = best else { break };
            in_tree[gj] = true;
            out.push(Flyline { net: net.clone(), a, b });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drc::run_tech_deck;
    use crate::examples::cmos_inverter;
    use crate::geom::{Orient, Pt};

    fn inverter() -> Schematic {
        cmos_inverter().schematic.unwrap()
    }

    fn mos(s: &mut Schematic, kind: DeviceKind, at: (i32, i32), params: &[(&str, &str)]) -> String {
        let id = s.add_instance(kind, Pt::new(at.0, at.1), Orient::default());
        let inst = s.instance_mut(id).unwrap();
        for (k, v) in params {
            inst.params.insert(k.to_string(), v.to_string());
        }
        inst.name.clone()
    }

    fn placed_bboxes(layout: &Layout, project: &Project) -> Vec<(String, IRect)> {
        layout
            .instances
            .iter()
            .map(|i| {
                let b = project.cell(&i.cell).unwrap().layout.as_ref().unwrap().shapes_bbox().unwrap();
                (i.name.clone().unwrap(), i.trans.apply_rect(b))
            })
            .collect()
    }

    fn assert_drc_clean(layout: &Layout, project: &Project) {
        let v = run_tech_deck(layout, project, &project.tech).unwrap().violations;
        assert!(v.is_empty(), "{:?}", v.iter().map(|v| (&v.rule, v.marker)).collect::<Vec<_>>());
    }

    #[test]
    fn plan_inverter() {
        let sch = inverter();
        let tech = Tech::generic();
        let p = plan(&sch, &tech);
        let mp = sch.instances.iter().find(|i| i.kind == DeviceKind::Pmos).unwrap().name.clone();
        let mn = sch.instances.iter().find(|i| i.kind == DeviceKind::Nmos).unwrap().name.clone();
        let names: Vec<&str> = p.devices.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, [mp.as_str(), "ntap:vdd", mn.as_str(), "ptap:0"]);
        assert_eq!(p.devices[0].pcell, PCell::Mos { pmos: true, w: 2000, l: 180, fingers: 1 });
        assert_eq!(p.devices[1].pcell, PCell::Tap { well: true, w: 2000 });
        assert_eq!(p.devices[2].pcell, PCell::Mos { pmos: false, w: 1000, l: 180, fingers: 1 });
        assert_eq!(p.devices[3].pcell, PCell::Tap { well: false, w: 1000 });

        let boxes: Vec<IRect> =
            p.devices.iter().map(|d| d.trans.apply_rect(d.pcell.generate(&tech).unwrap().shapes_bbox().unwrap())).collect();
        let (prow, nrow) = (boxes[0].union(&boxes[1]), boxes[2].union(&boxes[3]));
        assert!(prow.y0 >= nrow.y1 + 1400, "{boxes:?}");
        let nwell = tech.layer_index("nwell").unwrap();
        let well = p.fills.iter().find(|f| f.0 == nwell).expect("row nwell").1;
        assert!(well.x0 <= boxes[0].x0 && well.x1 >= boxes[1].x1, "one nwell over the PMOS and its tap");

        let mut skipped = p.skipped.clone();
        skipped.sort();
        assert_eq!(
            skipped,
            [
                "C1 (capacitor): no layout generator",
                "V1 (voltage source): no layout generator",
                "V2 (voltage source): no layout generator"
            ]
        );
        assert_eq!(plan(&sch, &tech), p);
    }

    #[test]
    fn fingers_and_bad_params() {
        let tech = Tech::generic();
        let mut s = Schematic::default();
        let a = mos(&mut s, DeviceKind::Nmos, (0, 0), &[("w", "1u"), ("l", "180n"), ("m", "2")]);
        let b = mos(&mut s, DeviceKind::Nmos, (10, 0), &[("w", "12u"), ("l", "0.18u"), ("m", "1")]);
        let c = mos(&mut s, DeviceKind::Pmos, (20, 0), &[("w", "abc"), ("l", "180n")]);
        let d = mos(&mut s, DeviceKind::Nmos, (30, 0), &[("w", "1u"), ("l", "50n")]);
        let e = mos(&mut s, DeviceKind::Nmos, (40, 0), &[("w", "1u"), ("l", "180n"), ("m", "1.5")]);
        let p = plan(&s, &tech);
        let get = |n: &str| p.devices.iter().find(|d| d.name == n).map(|d| d.pcell.clone());
        assert_eq!(get(&a), Some(PCell::Mos { pmos: false, w: 1000, l: 180, fingers: 2 }));
        assert_eq!(get(&b), Some(PCell::Mos { pmos: false, w: 4000, l: 180, fingers: 3 }));
        assert_eq!(p.devices.len(), 2);
        assert_eq!(p.skipped.len(), 3);
        assert!(p.skipped.iter().any(|m| m.starts_with(&format!("{c}: invalid w"))), "{:?}", p.skipped);
        assert!(p.skipped.iter().any(|m| m.starts_with(&format!("{d}: gate length"))), "{:?}", p.skipped);
        assert!(p.skipped.iter().any(|m| m.starts_with(&format!("{e}: invalid multiplier"))), "{:?}", p.skipped);
    }

    #[test]
    fn apply_is_drc_clean_and_idempotent() {
        let mut project = Project::new("t");
        let mut layout = Layout::default();
        let p = plan(&inverter(), &project.tech);
        let placed = apply(&mut project, &mut layout, &p).unwrap();
        assert_eq!(placed.len(), 4);
        assert_drc_clean(&layout, &project);
        let shapes = layout.shapes.len();
        assert!(apply(&mut project, &mut layout, &p).unwrap().is_empty());
        assert_eq!((layout.instances.len(), layout.shapes.len()), (4, shapes));

        let mut s = Schematic::default();
        for (i, x) in [0, 10, 20].into_iter().enumerate() {
            mos(&mut s, DeviceKind::Nmos, (x, 10), &[("w", "1u"), ("l", "180n"), ("m", &(i + 1).to_string())]);
            mos(&mut s, DeviceKind::Pmos, (x, 0), &[("w", "7u"), ("l", "350n")]);
        }
        let mut layout = Layout::default();
        let p = plan(&s, &project.tech);
        assert_eq!(apply(&mut project, &mut layout, &p).unwrap().len(), 6);
        assert_drc_clean(&layout, &project);
    }

    #[test]
    fn apply_avoids_existing_content() {
        let mut project = Project::new("t");
        let m1 = project.tech.layer_index("metal1").unwrap();
        let mut layout = Layout::default();
        layout.add_rect(m1, 0, 0, 5000, 5000);
        let p = plan(&inverter(), &project.tech);
        apply(&mut project, &mut layout, &p).unwrap();
        for (_, b) in placed_bboxes(&layout, &project) {
            assert!(b.x0 >= 5000 + 1400, "{b:?}");
        }
        assert_drc_clean(&layout, &project);

        let mut partial = Layout::default();
        let first = PlannedDevice { trans: Trans::new(50_000, 0), ..p.devices[0].clone() };
        apply(&mut project, &mut partial, &SdlPlan { devices: vec![first], ..Default::default() }).unwrap();
        let rest: Vec<String> = p.devices[1..].iter().map(|d| d.name.clone()).collect();
        assert_eq!(apply(&mut project, &mut partial, &p).unwrap(), rest);
        assert!(partial.shapes.is_empty(), "no fills for a partial placement");
        assert_eq!(partial.instances[1].trans, p.devices[1].trans);
    }

    #[test]
    fn ratsnest_inverter() {
        let sch = inverter();
        let mut project = Project::new("t");
        let mut layout = Layout::default();
        let p = plan(&sch, &project.tech);
        apply(&mut project, &mut layout, &p).unwrap();
        let fl = ratsnest(&sch, &layout, &project, &project.tech);
        let nets: Vec<&str> = fl.iter().map(|f| f.net.as_str()).collect();
        assert_eq!(nets, ["0", "in", "out", "vdd"], "the taps join their bulk nets");
        assert_eq!(ratsnest(&sch, &layout, &project, &project.tech), fl);

        let m1 = project.tech.layer_index("metal1").unwrap();
        let gate = fl.iter().find(|f| f.net == "in").unwrap();
        let (lo, hi) = if gate.a.1 < gate.b.1 { (gate.a, gate.b) } else { (gate.b, gate.a) };
        let ym = snap((lo.1 + hi.1) / 2, 10);
        layout.add(m1, Geometry::Path { points: vec![lo, (lo.0, ym), (hi.0, ym), hi], width: 230 });
        let fl = ratsnest(&sch, &layout, &project, &project.tech);
        assert_eq!(fl.iter().map(|f| f.net.as_str()).collect::<Vec<_>>(), ["0", "out", "vdd"]);
    }

    #[test]
    fn common_centroid_orders() {
        assert_eq!(common_centroid(&[2, 2]), [0, 1, 1, 0]);
        assert_eq!(common_centroid(&[1, 4]), [1, 1, 0, 1, 1]);
        assert_eq!(common_centroid(&[4, 4]), [0, 1, 0, 1, 1, 0, 1, 0]);
        assert_eq!(common_centroid(&[1, 1]), [0, 1]);
        for (name, dev) in [("M1.12", "M1"), ("M1", "M1"), ("a.b", "a.b"), (".3", ".3"), ("x.", "x.")] {
            assert_eq!(device_of(name), dev);
        }
    }

    #[test]
    fn matched_placement() {
        let mut project = Project::new("t");
        let tech = project.tech.clone();
        let mirror = crate::examples::current_mirror(&mut project).schematic.unwrap();
        let ota = crate::examples::ota5t(&mut project).schematic.unwrap();
        for f in [crate::examples::nand2, crate::examples::nor2, crate::examples::ring_osc5] {
            let sch = f(&mut project).schematic.unwrap();
            assert!(matched_groups(&sch).is_empty(), "{:?}", matched_groups(&sch));
        }
        assert!(matched_groups(&inverter()).is_empty());
        assert_eq!(matched_groups(&mirror).len(), 1);
        assert_eq!(matched_groups(&ota).len(), 2, "input pair and load mirror");

        for sch in [&mirror, &ota] {
            let p = plan(sch, &tech);
            assert!(!p.notes.is_empty() && p.notes.iter().all(|n| n.contains("matched as ABBA")), "{:?}", p.notes);
            for group in matched_groups(sch) {
                let centroids: Vec<f64> = group
                    .iter()
                    .map(|name| {
                        let xs: Vec<i64> = p
                            .devices
                            .iter()
                            .filter(|d| d.name != *name && device_of(&d.name) == name)
                            .map(|d| {
                                let PCell::Mos { l, .. } = d.pcell else { panic!() };
                                let c = mos_columns(&tech, l, 1).unwrap();
                                d.trans.apply(((c[0] + c[1]) / 2, 0)).0
                            })
                            .collect();
                        assert!(xs.len() >= 2, "{name}: {xs:?}");
                        xs.iter().sum::<i64>() as f64 / xs.len() as f64
                    })
                    .collect();
                assert!(centroids.windows(2).all(|w| (w[0] - w[1]).abs() <= tech.grid as f64), "{group:?}: {centroids:?}");
            }
            let mut layout = Layout::default();
            apply(&mut project, &mut layout, &p).unwrap();
            assert_drc_clean(&layout, &project);
            let mut whole = Layout::default();
            let first = p.devices.iter().find(|d| d.name.ends_with(".1")).unwrap();
            let named = PlannedDevice { name: device_of(&first.name).to_string(), ..first.clone() };
            apply(&mut project, &mut whole, &SdlPlan { devices: vec![named], ..Default::default() }).unwrap();
            let again = apply(&mut project, &mut whole, &p).unwrap();
            assert!(!again.iter().any(|n| device_of(n) == device_of(&first.name)), "{again:?}");
            let x = extract::extract(&layout, &project, &tech);
            let lvs = crate::lvs::compare(sch, &x, &crate::lvs::LvsOptions::default());
            assert!(!lvs.errors.iter().any(|e| e.kind == crate::lvs::LvsErrorKind::Parameter), "{:?}", lvs.errors);
        }

        let mut s = Schematic::default();
        let a = mos(&mut s, DeviceKind::Nmos, (0, 0), &[("w", "1u"), ("l", "180n"), ("match", "x")]);
        let b = mos(&mut s, DeviceKind::Nmos, (10, 0), &[("w", "3u"), ("l", "500n"), ("match", "x")]);
        mos(&mut s, DeviceKind::Nmos, (20, 0), &[("w", "1u"), ("l", "180n")]);
        assert_eq!(matched_groups(&s), [vec![a, b]]);
        let p = plan(&s, &tech);
        assert!(p.notes[0].contains("matched as"), "{:?}", p.notes);
        let mut opt_out = mirror.clone();
        let id = opt_out.instances.iter().find(|i| i.kind == DeviceKind::Nmos).unwrap().id;
        opt_out.instance_mut(id).unwrap().params.insert(MATCH_PARAM.into(), "none".into());
        assert!(matched_groups(&opt_out).is_empty());
    }

    #[test]
    fn shared_diffusion_and_merged_wells() {
        let mut project = Project::new("t");
        let tech = project.tech.clone();
        let sch = crate::examples::nand2(&mut project).schematic.unwrap();
        let p = plan(&sch, &tech);
        let columns = |d: &PlannedDevice| -> Vec<i64> {
            let PCell::Mos { l, fingers, .. } = d.pcell else { return vec![] };
            mos_columns(&tech, l, fingers).unwrap().into_iter().map(|x| d.trans.apply((x, 0)).0).collect()
        };
        let nmos: Vec<&PlannedDevice> = p.devices.iter().filter(|d| matches!(d.pcell, PCell::Mos { pmos: false, .. })).collect();
        let pmos: Vec<&PlannedDevice> = p.devices.iter().filter(|d| matches!(d.pcell, PCell::Mos { pmos: true, .. })).collect();
        assert_eq!((nmos.len(), pmos.len()), (2, 2));
        for pair in [&nmos, &pmos] {
            let (a, b) = (columns(pair[0]), columns(pair[1]));
            assert!(a.iter().any(|x| b.contains(x)), "series NMOS and parallel PMOS share a column: {a:?} {b:?}");
        }
        let mut layout = Layout::default();
        apply(&mut project, &mut layout, &p).unwrap();
        assert_drc_clean(&layout, &project);
        let x = extract::extract(&layout, &project, &tech);
        let nwell = tech.layer_index("nwell").unwrap();
        assert_eq!(x.nets.iter().filter(|n| n.shapes.iter().any(|(l, _)| *l == nwell)).count(), 1, "one merged nwell");
        assert_eq!(x.devices.len(), 4);
    }

    #[test]
    fn ratsnest_fingers_and_unplaced() {
        let tech = Tech::generic();
        let mut project = Project::new("t");
        let mut s = Schematic::default();
        let a = mos(&mut s, DeviceKind::Nmos, (0, 0), &[("w", "1u"), ("l", "180n"), ("m", "2")]);
        let mut layout = Layout::default();
        assert!(ratsnest(&s, &layout, &project, &tech).is_empty());
        apply(&mut project, &mut layout, &plan(&s, &tech)).unwrap();
        let terms = terminals(&s, &layout, &project);
        assert_eq!(terms.values().map(Vec::len).sum::<usize>(), 4, "{terms:?} for {a}");
        let fl = ratsnest(&s, &layout, &project, &tech);
        assert_eq!(fl.len(), 1, "two unconnected source strips of {a}: {fl:?}");
    }
}
