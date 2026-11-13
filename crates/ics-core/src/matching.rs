//! Matched-device placement ("modgen"): re-place MOS devices already in a layout as unit fingers
//! in an interdigitated or common-centroid pattern, optionally closed by dummy units.
//!
//! - The devices (layout instance names or schematic device names) must be MOS PCell instances of
//!   the same type, PDK device, L and finger width; every device's total finger count must be a
//!   multiple of the unit. All instances of a device ([`sdl::device_of`]: the instance itself or
//!   its `<device>.<k>` units) are replaced, so the total W of every device is kept.
//! - Units are named `<device>.<k>`, so terminals, flylines, routing and LVS still map them to
//!   their schematic device. The extractor sees each unit as a device of its own; once routed,
//!   LVS merges parallel units (same gate, source / drain pair, bulk and L) back into one device
//!   with the summed W.
//! - Dummies are named after the [`sdl::DUMMY`] convention, `dummy:mg_<first device>_<n>:gsd:<bulk>`
//!   with the bulk net of the first device from the cell's schematic, so routing ties their
//!   gate, source and drain to the bulk net and LVS drops them (gate on bulk, see
//!   [`crate::lvs::LvsOptions::filter_off`]). Without a schematic bulk net they are named
//!   `dummy:mg_<first device>_<n>::`, left floating and reported by LVS as extra devices.
//! - The units are spaced by the largest spacing of their layers other than wells and implants,
//!   which are merged by fill rectangles over the block, and the block is centred on the
//!   finger-weighted centroid of the replaced instances. Content in the block's horizontal band
//!   is pushed left / right to make room: instances and shapes by their side of the centre, and
//!   well / implant rectangles spanning the centre are stretched. Wires spanning the centre are
//!   left alone: run this before routing (or re-route).

use std::collections::{BTreeMap, BTreeSet};

use crate::connectivity;
use crate::geom::IRect;
use crate::hier::flatten;
use crate::layout::{Geometry, Layout, Trans};
use crate::pcell::PCell;
use crate::project::{CellRef, Project};
use crate::sdl::{DUMMY, device_of};
use crate::tech::Tech;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pattern {
    /// ABAB...: the units of every device spread evenly along one row.
    Interdigitated,
    /// One row mirror-symmetric about its centre (ABBA).
    CommonCentroid,
    /// `rows` rows, point-symmetric about the block centre (AB / BA).
    CommonCentroid2D,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchSpec {
    /// Layout instance names or schematic device names.
    pub devices: Vec<String>,
    pub pattern: Pattern,
    /// Fingers per unit.
    pub unit_fingers: u32,
    /// Close every row with a dummy unit at both ends.
    pub dummies: bool,
    /// Unit rows of [`Pattern::CommonCentroid2D`].
    pub rows: u32,
    /// Spacing between units (dbu); 0 = the minimum legal spacing.
    pub spacing: i64,
}

impl Default for MatchSpec {
    fn default() -> Self {
        Self { devices: vec![], pattern: Pattern::CommonCentroid, unit_fingers: 1, dummies: true, rows: 2, spacing: 0 }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchPlan {
    /// Rows (top first) of device indices into [`MatchSpec::devices`]; `None` is a dummy.
    pub sequence: Vec<Vec<Option<usize>>>,
    /// Letters per row (A = first device, d = dummy), rows separated by `/`, e.g. "dABBAd".
    pub pattern: String,
    pub notes: Vec<String>,
}

fn letter(i: Option<usize>) -> char {
    i.map_or('d', |i| (b'A' + (i % 26) as u8) as char)
}

/// Units of every device spread evenly: unit `j` of device `i` at `(j + 1/2) / counts[i]`.
fn interleave(counts: &[usize]) -> Vec<usize> {
    let mut keys: Vec<(u64, u64, usize)> = Vec::new();
    for (i, &c) in counts.iter().enumerate() {
        keys.extend((0..c).map(|j| (2 * j as u64 + 1, 2 * c as u64, i)));
    }
    keys.sort_by(|a, b| (a.0 * b.1).cmp(&(b.0 * a.1)).then(a.2.cmp(&b.2)));
    keys.into_iter().map(|k| k.2).collect()
}

/// Mirror-symmetric sequence: half of every device interleaved, the odd unit (at most one
/// device may have an odd count) in the middle, then the half reversed.
fn symmetric(counts: &[usize]) -> Result<Vec<usize>, String> {
    let odd: Vec<usize> = (0..counts.len()).filter(|&i| counts[i] % 2 == 1).collect();
    if odd.len() > 1 {
        return Err(format!(
            "common centroid needs an even unit count for all devices but one ({} have an odd count); use smaller units",
            odd.len()
        ));
    }
    let half = interleave(&counts.iter().map(|c| c / 2).collect::<Vec<_>>());
    let mut out = half.clone();
    out.extend(odd);
    out.extend(half.iter().rev());
    Ok(out)
}

/// Pattern of `counts[i]` units of device `i` (pure: no layout).
pub fn plan(spec: &MatchSpec, counts: &[usize]) -> Result<MatchPlan, String> {
    if counts.len() < 2 {
        return Err("matching needs at least two devices".into());
    }
    if counts.contains(&0) {
        return Err("every device needs at least one unit".into());
    }
    let total: usize = counts.iter().sum();
    let mut sequence: Vec<Vec<Option<usize>>> = match spec.pattern {
        Pattern::Interdigitated => vec![interleave(counts).into_iter().map(Some).collect()],
        Pattern::CommonCentroid => vec![symmetric(counts)?.into_iter().map(Some).collect()],
        Pattern::CommonCentroid2D => {
            let rows = spec.rows as usize;
            if rows < 2 {
                return Err("a 2-D common centroid needs at least two rows".into());
            }
            if !total.is_multiple_of(rows) {
                return Err(format!("{total} units do not fill {rows} equal rows"));
            }
            // Row-major positions p and total - 1 - p are point-symmetric.
            let flat = symmetric(counts)?;
            flat.chunks(total / rows).map(|r| r.iter().copied().map(Some).collect()).collect()
        }
    };
    if spec.dummies {
        for row in &mut sequence {
            row.insert(0, None);
            row.push(None);
        }
    }
    let pattern = sequence.iter().map(|r| r.iter().map(|&i| letter(i)).collect::<String>()).collect::<Vec<_>>().join("/");
    let mut notes: Vec<String> =
        spec.devices.iter().zip(counts).enumerate().map(|(i, (d, c))| format!("{} = {d} ({c} units)", letter(Some(i)))).collect();
    notes.insert(0, format!("placed as {pattern}"));
    Ok(MatchPlan { sequence, pattern, notes })
}

/// Mean (column, row) position of every device's units in `plan`.
pub fn centroids(plan: &MatchPlan, devices: usize) -> Vec<(f64, f64)> {
    let mut sum = vec![(0.0, 0.0, 0usize); devices];
    for (r, row) in plan.sequence.iter().enumerate() {
        for (c, i) in row.iter().enumerate() {
            if let Some(s) = i.and_then(|i| sum.get_mut(i)) {
                *s = (s.0 + c as f64, s.1 + r as f64, s.2 + 1);
            }
        }
    }
    sum.into_iter().map(|(x, y, n)| (x / n.max(1) as f64, y / n.max(1) as f64)).collect()
}

fn snap(v: i64, g: i64) -> i64 {
    if g <= 1 { v } else { (v + g.div_euclid(2)).div_euclid(g) * g }
}

fn snap_up(v: i64, g: i64) -> i64 {
    if g <= 1 { v } else { (v + g - 1).div_euclid(g) * g }
}

/// Well and implant layers, merged over the block instead of spaced.
fn soft_layers(tech: &Tech) -> Vec<usize> {
    [tech.nwell(), tech.implant(false), tech.implant(true)].into_iter().flatten().collect()
}

fn largest_spacing(tech: &Tech) -> i64 {
    snap_up(tech.layers.iter().map(|l| l.min_spacing).max().unwrap_or(0), tech.grid)
}

/// The MOS parameters every unit must share: type, finger width, L and PDK device.
type MosKey = (bool, i64, i64, String);

/// Replace the instances of `spec.devices` in the layout of `cell` by unit fingers placed in
/// `spec.pattern` (see the module docs). Returns the names of the placed instances, row by row.
/// The layout is unchanged on error.
pub fn apply(project: &mut Project, cell: &CellRef, spec: &MatchSpec) -> Result<Vec<String>, String> {
    if spec.unit_fingers == 0 {
        return Err("a unit needs at least one finger".into());
    }
    let c = project.cell(cell).ok_or_else(|| format!("no cell {cell}"))?;
    let mut lay = c.layout.clone().ok_or_else(|| format!("{cell} has no layout"))?;
    let sch = c.schematic.clone();
    let mut names: Vec<String> = Vec::new();
    for d in &spec.devices {
        let d = device_of(d).to_string();
        if names.contains(&d) {
            return Err(format!("device {d} is listed twice"));
        }
        names.push(d);
    }

    let mut key: Option<MosKey> = None;
    let mut counts = Vec::new();
    let mut originals = BTreeSet::new();
    let (mut weight, mut sx, mut sy) = (0i128, 0i128, 0i128);
    let mut orient: Option<(u8, bool)> = None;
    for name in &names {
        let mut fingers = 0u64;
        for inst in lay.instances.iter().filter(|i| i.name.as_deref().is_some_and(|n| device_of(n) == name)) {
            let child = project.cell(&inst.cell);
            let Some(PCell::Mos { pmos, w, l, fingers: nf, device }) = child.and_then(|c| c.pcell.clone()) else {
                return Err(format!("{name}: instance of {} is not a MOS PCell", inst.cell));
            };
            let k = (pmos, w, l, device);
            match &key {
                None => key = Some(k),
                Some(first) if *first != k => {
                    return Err(format!("{name} differs from {} in type, finger width, L or device", names[0]));
                }
                _ => {}
            }
            let n = nf as u64 * inst.count();
            fingers += n;
            if let Some(b) = child.and_then(|c| c.layout.as_ref()).and_then(Layout::shapes_bbox) {
                let b = inst.bbox(b);
                let n = n as i128;
                (weight, sx, sy) = (weight + n, sx + n * (b.x0 + b.x1) as i128, sy + n * (b.y0 + b.y1) as i128);
            }
            orient.get_or_insert((inst.trans.rot, inst.trans.mirror));
            originals.insert(inst.id);
        }
        if fingers == 0 {
            return Err(format!("{name} has no instance in the layout of {cell}"));
        }
        if !fingers.is_multiple_of(spec.unit_fingers as u64) {
            return Err(format!("{name} has {fingers} fingers, not a multiple of {} per unit", spec.unit_fingers));
        }
        counts.push((fingers / spec.unit_fingers as u64) as usize);
    }
    let plan = plan(&MatchSpec { devices: names.clone(), ..spec.clone() }, &counts)?;
    let Some((pmos, w, l, device)) = key else { return Err("no devices given".into()) };
    let (rot, mirror) = orient.unwrap_or_default();
    let g = project.tech.grid.max(1);
    let (cx2, cy2) = ((sx / weight.max(1)) as i64, (sy / weight.max(1)) as i64);

    let unit = PCell::Mos { pmos, w, l, fingers: spec.unit_fingers, device };
    let unit_ref = project.ensure_pcell(&unit)?;
    let unit_lay = project.cell(&unit_ref).and_then(|c| c.layout.clone()).ok_or("unit cell without layout")?;
    let tech = &project.tech;
    let soft = soft_layers(tech);
    let hard: Vec<_> =
        unit_lay.shapes.iter().filter(|s| !soft.contains(&s.layer) && !matches!(s.geom, Geometry::Label { .. })).collect();
    let t0 = Trans::with(0, 0, rot, mirror);
    let core = hard.iter().map(|s| s.bbox()).reduce(|a, b| a.union(&b)).map(|b| t0.apply_rect(b)).ok_or("empty unit cell")?;
    let legal = snap_up(hard.iter().filter_map(|s| tech.layer(s.layer)).map(|l| l.min_spacing).max().unwrap_or(0), g);
    if spec.spacing < 0 || (spec.spacing > 0 && spec.spacing < legal) {
        return Err(format!("spacing {} is below the minimum {legal}", spec.spacing));
    }
    let gap = if spec.spacing > 0 { snap_up(spec.spacing, g) } else { legal };
    let (px, py) = (core.width() + gap, core.height() + gap);
    let (cols, rows) = (plan.sequence[0].len() as i64, plan.sequence.len() as i64);
    let x0 = snap((cx2 - (cols * px - gap)) / 2, g);
    let y0 = snap((cy2 - (rows * py - gap)) / 2, g);

    let bulk = sch.as_ref().and_then(|s| {
        let conn = connectivity::extract(s);
        let inst = s.instances.iter().find(|i| i.name == names[0])?;
        let pin = inst.pin_positions().into_iter().find(|(n, _)| *n == "b").map(|(_, p)| p);
        if pin.is_some_and(|p| conn.unconnected_pins.contains(&p)) {
            return None;
        }
        conn.net_of(inst.id, "b").map(str::to_string)
    });
    let tag = format!("{DUMMY}mg_{}_", names[0]);
    let mut next = vec![0usize; names.len()];
    let mut dummies = 0;
    let mut placed: Vec<(String, Trans)> = Vec::new();
    for (r, row) in plan.sequence.iter().enumerate() {
        for (c, dev) in row.iter().enumerate() {
            let x = x0 + c as i64 * px - core.x0;
            let y = y0 + (rows - 1 - r as i64) * py - core.y0;
            let name = match dev {
                Some(i) => {
                    next[*i] += 1;
                    format!("{}.{}", names[*i], next[*i])
                }
                None => {
                    dummies += 1;
                    match &bulk {
                        Some(b) => format!("{tag}{dummies}:gsd:{b}"),
                        None => format!("{tag}{dummies}::"),
                    }
                }
            };
            placed.push((name, Trans { x, y, rot, mirror }));
        }
    }

    for i in lay.instances.iter().filter(|i| i.name.as_deref().is_some_and(|n| n.starts_with(&tag))) {
        originals.insert(i.id);
    }
    for id in originals {
        lay.remove(id);
    }
    let unit_bbox = unit_lay.shapes_bbox().ok_or("empty unit cell")?;
    let block = placed.iter().map(|(_, t)| t.apply_rect(unit_bbox)).reduce(|a, b| a.union(&b)).ok_or("nothing to place")?;
    make_room(&mut lay, project, block, cx2 / 2, largest_spacing(tech), &soft);

    let mut fills: BTreeMap<usize, IRect> = BTreeMap::new();
    for (_, t) in &placed {
        for s in unit_lay.shapes.iter().filter(|s| soft.contains(&s.layer)) {
            let r = t.apply_rect(s.bbox());
            fills.entry(s.layer).and_modify(|f| *f = f.union(&r)).or_insert(r);
        }
    }
    let mut out = Vec::with_capacity(placed.len());
    for (name, trans) in placed {
        let id = lay.add_instance(unit_ref.clone(), trans, None);
        if let Some(inst) = lay.instance_mut(id) {
            inst.name = Some(name.clone());
        }
        out.push(name);
    }
    for (layer, r) in fills {
        lay.add(layer, Geometry::Rect(r));
    }
    if let Some(c) = project.cell_mut(cell) {
        c.layout = Some(lay);
    }
    Ok(out)
}

/// Push the content of `lay` in the horizontal band of `block` away from `cx` until it is `gap`
/// clear of the block; soft-layer rectangles spanning `cx` are stretched instead.
fn make_room(lay: &mut Layout, project: &Project, block: IRect, cx: i64, gap: i64, soft: &[usize]) {
    let in_band = |b: &IRect| b.y1 > block.y0 - gap && b.y0 < block.y1 + gap;
    let inst_boxes: Vec<(u64, IRect)> = lay
        .instances
        .iter()
        .filter_map(|i| {
            let one = Layout { instances: vec![i.clone()], ..Default::default() };
            let b = flatten(&one, project, 64).iter().map(|s| s.geom.bbox()).reduce(|a, b| a.union(&b))?;
            Some((i.id, b))
        })
        .filter(|(_, b)| in_band(b))
        .collect();
    let shape_boxes: Vec<(u64, IRect, bool)> = lay
        .shapes
        .iter()
        .map(|s| (s.id, s.bbox(), soft.contains(&s.layer) && matches!(s.geom, Geometry::Rect(_))))
        .filter(|(_, b, _)| in_band(b))
        .collect();
    let left = |b: &IRect| b.x1 <= cx || (b.x0 < cx && (b.x0 + b.x1) / 2 < cx);
    let (mut dl, mut dr) = (0, 0);
    let pieces = inst_boxes.iter().map(|(_, b)| (b, false)).chain(shape_boxes.iter().map(|(_, b, s)| (b, *s)));
    for (b, stretch) in pieces {
        if stretch && b.x0 < cx && b.x1 > cx {
            continue;
        }
        if left(b) {
            dl = dl.max(b.x1 + gap - block.x0);
        } else {
            dr = dr.max(block.x1 + gap - b.x0);
        }
    }
    let g = project.tech.grid;
    let (dl, dr) = (snap_up(dl.max(0), g), snap_up(dr.max(0), g));
    if dl == 0 && dr == 0 {
        return;
    }
    for (id, b) in inst_boxes {
        lay.translate_item(id, if left(&b) { -dl } else { dr }, 0);
    }
    for (id, b, stretch) in shape_boxes {
        if stretch && b.x0 < cx && b.x1 > cx {
            if let Some(Geometry::Rect(r)) = lay.shape_mut(id).map(|s| &mut s.geom) {
                (r.x0, r.x1) = (r.x0 - dl, r.x1 + dr);
            }
        } else if b.x1 <= cx || b.x0 >= cx {
            lay.translate_item(id, if left(&b) { -dl } else { dr }, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drc::run_tech_deck;
    use crate::extract;
    use crate::lvs::{LvsErrorKind, LvsOptions, compare};
    use crate::pcell::mos_columns;
    use crate::project::Cell;
    use crate::schematic::{DeviceKind, Schematic};

    fn spec(pattern: Pattern, n: usize) -> MatchSpec {
        MatchSpec { devices: (0..n).map(|i| format!("M{i}")).collect(), pattern, dummies: false, ..Default::default() }
    }

    fn letters(p: &MatchPlan) -> &str {
        &p.pattern
    }

    fn is_mirror(row: &[Option<usize>]) -> bool {
        row.iter().eq(row.iter().rev())
    }

    fn assert_common_centroid(p: &MatchPlan, n: usize) {
        let c = centroids(p, n);
        assert!(c.windows(2).all(|w| (w[0].0 - w[1].0).abs() < 1e-9 && (w[0].1 - w[1].1).abs() < 1e-9), "{p:?}: {c:?}");
    }

    fn unit_counts(p: &MatchPlan, n: usize) -> Vec<usize> {
        (0..n).map(|i| p.sequence.iter().flatten().filter(|d| **d == Some(i)).count()).collect()
    }

    #[test]
    fn two_device_patterns() {
        let p = plan(&spec(Pattern::Interdigitated, 2), &[2, 2]).unwrap();
        assert_eq!(letters(&p), "ABAB");
        let p = plan(&spec(Pattern::Interdigitated, 2), &[3, 3]).unwrap();
        assert_eq!(letters(&p), "ABABAB");
        let p = plan(&spec(Pattern::CommonCentroid, 2), &[2, 2]).unwrap();
        assert_eq!(letters(&p), "ABBA");
        assert_common_centroid(&p, 2);
        let p = plan(&spec(Pattern::CommonCentroid, 2), &[4, 4]).unwrap();
        assert_eq!(letters(&p), "ABABBABA");
        assert_common_centroid(&p, 2);
        let p = plan(&MatchSpec { dummies: true, ..spec(Pattern::CommonCentroid, 2) }, &[2, 2]).unwrap();
        assert_eq!(letters(&p), "dABBAd");
        assert_eq!(p.sequence[0][0], None);
        assert!(p.notes[0].contains("dABBAd") && p.notes[1] == "A = M0 (2 units)", "{:?}", p.notes);
        assert_common_centroid(&p, 2);
    }

    #[test]
    fn three_devices_and_unequal_counts() {
        let p = plan(&spec(Pattern::CommonCentroid, 3), &[2, 4, 2]).unwrap();
        assert_eq!(letters(&p), "BACBBCAB");
        assert!(is_mirror(&p.sequence[0]));
        assert_common_centroid(&p, 3);
        assert_eq!(unit_counts(&p, 3), [2, 4, 2]);

        let p = plan(&spec(Pattern::CommonCentroid, 2), &[1, 4]).unwrap();
        assert_eq!(letters(&p), "BBABB");
        assert_common_centroid(&p, 2);

        let p = plan(&spec(Pattern::Interdigitated, 2), &[2, 4]).unwrap();
        assert_eq!(letters(&p), "BABBAB");
        assert_eq!(unit_counts(&p, 2), [2, 4]);
        let p = plan(&spec(Pattern::Interdigitated, 3), &[1, 1, 1]).unwrap();
        assert_eq!(letters(&p), "ABC");
    }

    #[test]
    fn two_dimensional_point_symmetry() {
        for (counts, rows) in [(vec![4, 4], 2), (vec![2, 2], 2), (vec![4, 8, 4], 4), (vec![3, 6], 3), (vec![2, 4, 2, 4], 2)] {
            let n = counts.len();
            for dummies in [false, true] {
                let s = MatchSpec { rows, dummies, ..spec(Pattern::CommonCentroid2D, n) };
                let p = plan(&s, &counts).unwrap_or_else(|e| panic!("{counts:?}: {e}"));
                assert_eq!(p.sequence.len(), rows as usize);
                assert!(p.sequence.iter().all(|r| r.len() == p.sequence[0].len()));
                let flat: Vec<Option<usize>> = p.sequence.iter().flatten().copied().collect();
                assert!(is_mirror(&flat), "point symmetric: {}", p.pattern);
                assert_eq!(unit_counts(&p, n), counts);
                assert_common_centroid(&p, n);
            }
        }
        let p = plan(&MatchSpec { rows: 2, ..spec(Pattern::CommonCentroid2D, 2) }, &[4, 4]).unwrap();
        assert_eq!(p.pattern, "ABAB/BABA");
    }

    #[test]
    fn plan_errors() {
        assert!(plan(&spec(Pattern::CommonCentroid, 1), &[4]).is_err());
        assert!(plan(&spec(Pattern::CommonCentroid, 2), &[2, 0]).is_err());
        assert!(plan(&spec(Pattern::CommonCentroid, 2), &[1, 3]).unwrap_err().contains("odd"));
        assert!(plan(&spec(Pattern::Interdigitated, 2), &[1, 3]).is_ok());
        let s = MatchSpec { rows: 1, ..spec(Pattern::CommonCentroid2D, 2) };
        assert!(plan(&s, &[2, 2]).is_err());
        let s = MatchSpec { rows: 3, ..spec(Pattern::CommonCentroid2D, 2) };
        assert!(plan(&s, &[2, 2]).unwrap_err().contains("equal rows"));
    }

    /// The current mirror schematic with both devices at m = `m`, placed whole (no SDL matching)
    /// into a new cell.
    fn placed_cell(project: &mut Project, sch: Schematic, name: &str) -> CellRef {
        let plan = crate::sdl::plan(&sch, &project.tech);
        let mut lay = Layout::default();
        crate::sdl::apply(project, &mut lay, &plan).unwrap();
        project.add_cell("work", Cell { schematic: Some(sch), layout: Some(lay), ..Cell::new(name) }).unwrap()
    }

    fn with_params(mut sch: Schematic, devices: &[&str], params: &[(&str, &str)]) -> Schematic {
        for inst in sch.instances.iter_mut().filter(|i| devices.contains(&i.name.as_str())) {
            for (k, v) in params {
                inst.params.insert(k.to_string(), v.to_string());
            }
        }
        for inst in sch.instances.iter_mut().filter(|i| matches!(i.kind, DeviceKind::Nmos | DeviceKind::Pmos)) {
            inst.params.insert(crate::sdl::MATCH_PARAM.into(), "none".into());
        }
        sch
    }

    fn gate_centroid(lay: &Layout, project: &Project, device: &str) -> (f64, f64) {
        let pts: Vec<(i64, i64)> = lay
            .instances
            .iter()
            .filter(|i| i.name.as_deref().is_some_and(|n| n != device && device_of(n) == device))
            .map(|i| {
                let Some(PCell::Mos { l, w, fingers, .. }) = project.cell(&i.cell).and_then(|c| c.pcell.clone()) else {
                    panic!()
                };
                let c = mos_columns(&project.tech, l, fingers).unwrap();
                i.trans.apply(((c[0] + c[c.len() - 1]) / 2, w / 2))
            })
            .collect();
        let n = pts.len() as f64;
        (pts.iter().map(|p| p.0 as f64).sum::<f64>() / n, pts.iter().map(|p| p.1 as f64).sum::<f64>() / n)
    }

    fn check_flow(project: &mut Project, cell: &CellRef, devices: &[&str]) {
        let lay = project.cell(cell).unwrap().layout.clone().unwrap();
        let sch = project.cell(cell).unwrap().schematic.clone().unwrap();
        let v = run_tech_deck(&lay, &*project, &project.tech).unwrap().violations;
        assert!(v.is_empty(), "{:?}", v.iter().map(|v| (&v.rule, v.marker)).collect::<Vec<_>>());
        let c: Vec<(f64, f64)> = devices.iter().map(|d| gate_centroid(&lay, project, d)).collect();
        let g = project.tech.grid as f64;
        assert!(c.windows(2).all(|w| (w[0].0 - w[1].0).abs() <= g && (w[0].1 - w[1].1).abs() <= g), "{c:?}");

        let mut lay = lay;
        let rep = crate::route::route(&sch, &mut lay, &*project, &project.tech).unwrap();
        assert!(rep.failed.is_empty(), "{rep:?}");
        let v = run_tech_deck(&lay, &*project, &project.tech).unwrap().violations;
        assert!(v.is_empty(), "{:?}", v.iter().map(|v| (&v.rule, v.marker)).collect::<Vec<_>>());
        let x = extract::extract(&lay, &*project, &project.tech);
        let opts = LvsOptions {
            ignore_bulk: false,
            ignore_kinds: vec![DeviceKind::VSource, DeviceKind::ISource, DeviceKind::Capacitor, DeviceKind::Resistor],
            ..Default::default()
        };
        let lvs = compare(&sch, &x, &opts);
        assert!(lvs.matched, "{:?}", lvs.errors);
        let raw = compare(&sch, &x, &LvsOptions { filter_off: false, ..opts });
        assert!(raw.errors.iter().any(|e| e.kind == LvsErrorKind::ExtraDevice), "the dummies are real, off devices");
    }

    #[test]
    fn apply_current_mirror_common_centroid() {
        let mut project = Project::new("t");
        let sch = crate::examples::current_mirror(&mut project).schematic.unwrap();
        let sch = with_params(sch, &["M1", "M2"], &[("m", "2")]);
        let cell = placed_cell(&mut project, sch, "cm");
        let s = MatchSpec { devices: vec!["M1".into(), "M2".into()], ..Default::default() };
        let placed = apply(&mut project, &cell, &s).unwrap();
        assert_eq!(placed.len(), 6, "{placed:?}");
        assert_eq!(&placed[1..5], ["M1.1", "M2.1", "M2.2", "M1.2"]);
        assert!(placed[0].starts_with("dummy:mg_M1_1:gsd:0") && placed[5].ends_with(":gsd:0"), "{placed:?}");
        let lay = project.cell(&cell).unwrap().layout.as_ref().unwrap();
        assert!(!lay.instances.iter().any(|i| matches!(i.name.as_deref(), Some("M1" | "M2"))), "originals removed");
        check_flow(&mut project, &cell, &["M1", "M2"]);

        // Re-placing the units again replaces them (and the old dummies).
        let s = MatchSpec { pattern: Pattern::Interdigitated, ..s };
        let placed = apply(&mut project, &cell, &s).unwrap();
        assert_eq!(&placed[1..5], ["M1.1", "M2.1", "M1.2", "M2.2"]);
        assert_eq!(project.cell(&cell).unwrap().layout.as_ref().unwrap().instances.len(), 6 + 1, "units, dummies and tap");

        let s = MatchSpec { pattern: Pattern::CommonCentroid2D, rows: 2, ..s };
        let placed = apply(&mut project, &cell, &s).unwrap();
        assert_eq!(placed.len(), 8, "two rows of dummy, two units, dummy: {placed:?}");
        check_flow(&mut project, &cell, &["M1", "M2"]);
    }

    #[test]
    fn apply_ota_pair_makes_room() {
        let mut project = Project::new("t");
        let sch = crate::examples::ota5t(&mut project).schematic.unwrap();
        let sch = with_params(sch, &["M1", "M2"], &[("m", "2")]);
        let cell = placed_cell(&mut project, sch, "ota");
        let x_of = |p: &Project, n: &str| {
            let lay = p.cell(&cell).unwrap().layout.as_ref().unwrap();
            lay.instances.iter().find(|i| i.name.as_deref() == Some(n)).map(|i| i.trans.x).unwrap()
        };
        let tail = x_of(&project, "M5");
        let s = MatchSpec { devices: vec!["M1".into(), "M2.1".into()], pattern: Pattern::CommonCentroid, ..Default::default() };
        apply(&mut project, &cell, &s).unwrap();
        assert_ne!(x_of(&project, "M5"), tail, "the tail device makes room");
        check_flow(&mut project, &cell, &["M1", "M2"]);
    }

    #[test]
    fn apply_errors_leave_layout_unchanged() {
        let mut project = Project::new("t");
        let sch = crate::examples::ota5t(&mut project).schematic.unwrap();
        let sch = with_params(sch, &["M1"], &[("m", "2")]);
        let cell = placed_cell(&mut project, sch, "ota");
        let before = project.cell(&cell).unwrap().layout.clone();
        let names = |d: &[&str]| d.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let cases = [
            (MatchSpec { devices: names(&["M1", "M3"]), ..Default::default() }, "differs"),
            (MatchSpec { devices: names(&["M1", "M5"]), ..Default::default() }, "differs"),
            (MatchSpec { devices: names(&["M1", "M9"]), ..Default::default() }, "no instance"),
            (MatchSpec { devices: names(&["M1", "M1.2"]), ..Default::default() }, "twice"),
            (MatchSpec { devices: names(&["M1", "M2"]), unit_fingers: 2, ..Default::default() }, "multiple"),
            (MatchSpec { devices: names(&["M1", "M2"]), pattern: Pattern::CommonCentroid2D, ..Default::default() }, "rows"),
            (
                MatchSpec { devices: names(&["M1", "M2"]), pattern: Pattern::Interdigitated, spacing: 10, ..Default::default() },
                "below",
            ),
            (MatchSpec { devices: names(&["M1", "M2"]), unit_fingers: 0, ..Default::default() }, "finger"),
        ];
        for (s, msg) in cases {
            let e = apply(&mut project, &cell, &s).unwrap_err();
            assert!(e.contains(msg), "{s:?}: {e}");
            assert_eq!(project.cell(&cell).unwrap().layout, before);
        }
        assert!(apply(&mut project, &CellRef::new("work", "nope"), &MatchSpec::default()).is_err());
    }
}
