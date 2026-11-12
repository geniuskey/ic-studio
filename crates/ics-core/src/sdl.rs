//! Schematic-driven layout: device placement from a schematic and unrouted-connection flylines.
//!
//! Placement puts every MOS device as a PCell instance named after its schematic instance, in two
//! rows: PMOS (mirrored, gate contacts facing down) above NMOS (gate contacts facing up), each
//! row ordered by schematic position. Neighbouring bounding boxes are kept apart by the largest
//! minimum spacing of the layers the two cells use, so the placement is DRC clean by construction.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::connectivity;
use crate::extract;
use crate::geom::IRect;
use crate::hier::{LayoutSource, flatten};
use crate::layout::{Geometry, Layout, Trans};
use crate::pcell::PCell;
use crate::project::Project;
use crate::schematic::{DeviceKind, Instance, Schematic};
use crate::tech::Tech;
use crate::units::parse_spice;

/// Devices wider than this (µm) are split into equal fingers.
pub const MAX_FINGER_W_UM: f64 = 5.0;

/// A schematic device to be placed as a PCell instance named after it.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedDevice {
    pub name: String,
    pub pcell: PCell,
    pub trans: Trans,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SdlPlan {
    pub devices: Vec<PlannedDevice>,
    /// Schematic devices with no generator (sources, R, C, ...), with the reason.
    pub skipped: Vec<String>,
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
    orient: Trans,
    bbox: IRect,
    space: i64,
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

fn candidate(inst: &Instance, tech: &Tech) -> Result<Candidate, String> {
    let pcell = mos_pcell(inst, tech)?;
    let lay = pcell.generate(tech)?;
    let bbox = lay.shapes_bbox().ok_or("the generator produced no shapes")?;
    let orient = if inst.kind == DeviceKind::Pmos { Trans::with(0, 0, 0, true) } else { Trans::IDENTITY };
    let space = lay.shapes.iter().map(|s| layer_space(tech, s.layer)).max().unwrap_or(0);
    Ok(Candidate { name: inst.name.clone(), pcell, bbox: orient.apply_rect(bbox), orient, space })
}

fn place_row(row: &[Candidate], y: i64, grid: i64, out: &mut Vec<PlannedDevice>) {
    let mut x = 0;
    for (i, c) in row.iter().enumerate() {
        if i > 0 {
            x += snap_up(c.space.max(row[i - 1].space), grid);
        }
        let trans = Trans { x: x - c.bbox.x0, y: y - c.bbox.y0, ..c.orient };
        out.push(PlannedDevice { name: c.name.clone(), pcell: c.pcell.clone(), trans });
        x += c.bbox.width();
    }
}

/// Map schematic MOS devices to PCells and arrange them in rows (PMOS above NMOS), ordered
/// like the schematic, without overlaps.
pub fn plan(sch: &Schematic, tech: &Tech) -> SdlPlan {
    let mut plan = SdlPlan::default();
    let mut mos: Vec<&Instance> = Vec::new();
    for inst in &sch.instances {
        match inst.kind {
            DeviceKind::Nmos | DeviceKind::Pmos => mos.push(inst),
            DeviceKind::Ground | DeviceKind::Supply => {}
            k => plan.skipped.push(format!("{} ({}): no layout generator", inst.name, k.label().to_lowercase())),
        }
    }
    mos.sort_by(|a, b| (a.pos.x, a.pos.y, &a.name).cmp(&(b.pos.x, b.pos.y, &b.name)));
    let (mut prow, mut nrow) = (Vec::new(), Vec::new());
    for inst in mos {
        match candidate(inst, tech) {
            Ok(c) if inst.kind == DeviceKind::Pmos => prow.push(c),
            Ok(c) => nrow.push(c),
            Err(e) => plan.skipped.push(format!("{}: {e}", inst.name)),
        }
    }
    let n_height = nrow.iter().map(|c| c.bbox.height()).max();
    let row_gap = snap_up(prow.iter().chain(&nrow).map(|c| c.space).max().unwrap_or(0), tech.grid);
    let p_y = n_height.map_or(0, |h| h + row_gap);
    place_row(&prow, p_y, tech.grid, &mut plan.devices);
    place_row(&nrow, 0, tech.grid, &mut plan.devices);
    plan
}

fn flat_bbox(layout: &Layout, src: &dyn LayoutSource) -> Option<IRect> {
    flatten(layout, src, 64).iter().map(|s| s.geom.bbox()).reduce(|a, b| a.union(&b))
}

/// Place the planned devices that are not in `layout` yet (matched by instance name). Existing
/// named instances are left where they are. Returns the names placed.
///
/// When the new devices would come closer than the technology's largest spacing to existing
/// content, they are all shifted to the right of it (bottom aligned).
pub fn apply(project: &mut Project, layout: &mut Layout, plan: &SdlPlan) -> Result<Vec<String>, String> {
    let existing: HashSet<String> = layout.instances.iter().filter_map(|i| i.name.clone()).collect();
    let mut todo = Vec::new();
    for d in plan.devices.iter().filter(|d| !existing.contains(&d.name)) {
        let cell = project.ensure_pcell(&d.pcell)?;
        let bbox =
            project.cell(&cell).and_then(|c| c.layout.as_ref()).and_then(Layout::shapes_bbox).map(|b| d.trans.apply_rect(b));
        todo.push((d, cell, bbox));
    }
    let new_bbox = todo.iter().filter_map(|t| t.2).reduce(|a, b| a.union(&b));
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
    Ok(placed)
}

/// Device terminals per schematic net: the transformed centres of the "D", "G" and "S" labels
/// of each named layout instance matching a schematic MOS instance (bulk is not a terminal).
pub fn terminals(sch: &Schematic, layout: &Layout, src: &dyn LayoutSource) -> BTreeMap<String, Vec<(i64, i64)>> {
    terminal_pins(sch, layout, src).into_iter().map(|(net, pins)| (net, pins.into_iter().map(|(_, p)| p).collect())).collect()
}

/// A device terminal: its pin ("d", "g" or "s") and position.
pub type Pin = (&'static str, (i64, i64));

/// [`terminals`] with the pin of each point.
pub fn terminal_pins(sch: &Schematic, layout: &Layout, src: &dyn LayoutSource) -> BTreeMap<String, Vec<Pin>> {
    let conn = connectivity::extract(sch);
    let mut out: BTreeMap<String, Vec<Pin>> = BTreeMap::new();
    for inst in sch.instances.iter().filter(|i| matches!(i.kind, DeviceKind::Nmos | DeviceKind::Pmos)) {
        let Some(placed) = layout.instances.iter().find(|i| i.name.as_deref() == Some(inst.name.as_str())) else { continue };
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
        assert_eq!(p.devices.len(), 2);
        assert_eq!(p.devices[0].name, mp);
        assert_eq!(p.devices[0].pcell, PCell::Mos { pmos: true, w: 2000, l: 180, fingers: 1 });
        assert_eq!(p.devices[1].name, mn);
        assert_eq!(p.devices[1].pcell, PCell::Mos { pmos: false, w: 1000, l: 180, fingers: 1 });

        let boxes: Vec<IRect> =
            p.devices.iter().map(|d| d.trans.apply_rect(d.pcell.generate(&tech).unwrap().shapes_bbox().unwrap())).collect();
        assert_eq!((boxes[1].x0, boxes[1].y0), (0, 0));
        assert!(boxes[0].y0 >= boxes[1].y1 + 1400, "{boxes:?}");

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
        assert_eq!(placed.len(), 2);
        assert_drc_clean(&layout, &project);
        assert!(apply(&mut project, &mut layout, &p).unwrap().is_empty());
        assert_eq!(layout.instances.len(), 2);

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
        apply(&mut project, &mut partial, &SdlPlan { devices: vec![first], skipped: vec![] }).unwrap();
        assert_eq!(apply(&mut project, &mut partial, &p).unwrap(), [p.devices[1].name.clone()]);
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
        assert_eq!(nets, ["in", "out"]);
        assert_eq!(ratsnest(&sch, &layout, &project, &project.tech), fl);

        let m1 = project.tech.layer_index("metal1").unwrap();
        let gate = &fl[0];
        let (lo, hi) = if gate.a.1 < gate.b.1 { (gate.a, gate.b) } else { (gate.b, gate.a) };
        let ym = snap((lo.1 + hi.1) / 2, 10);
        layout.add(m1, Geometry::Path { points: vec![lo, (lo.0, ym), (hi.0, ym), hi], width: 230 });
        let fl = ratsnest(&sch, &layout, &project, &project.tech);
        assert_eq!(fl.iter().map(|f| f.net.as_str()).collect::<Vec<_>>(), ["out"]);
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
