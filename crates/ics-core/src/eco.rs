//! Connectivity-driven layout against its schematic (Virtuoso Layout XL "Check Against Source"
//! and "Update Components and Nets").
//!
//! [`check`] compares a schematic-driven layout with its schematic:
//! - **missing**: schematic MOS devices with no named layout instance,
//! - **extra**: named layout instances whose device is no longer in the schematic (row taps and
//!   dummies are left alone),
//! - **parameters**: placed devices whose PCell no longer matches the schematic's W, L, m or
//!   model (units of matched devices are compared by total width),
//! - **shorts**: one layout conductor reaching the terminals of more than one schematic net,
//! - **opens**: a schematic net whose terminals sit on more than one layout conductor.
//!
//! [`update`] brings the layout in line: missing devices are placed (as
//! [`crate::sdl::apply`] does), changed devices get a new PCell master in place (keeping their
//! placement, so neighbours may need compacting), and, if asked, extra devices are deleted.
//! Shorts and opens are reported only: they are wiring, which [`crate::route`] or the designer
//! fixes.

use std::collections::{BTreeMap, BTreeSet};

use crate::extract;
use crate::geom::IRect;
use crate::hier::LayoutSource;
use crate::layout::Layout;
use crate::pcell::PCell;
use crate::project::Project;
use crate::schematic::Schematic;
use crate::sdl::{self, DUMMY, NTAP, PTAP, device_of};
use crate::tech::Tech;

/// One difference between the layout and its schematic.
#[derive(Debug, Clone, PartialEq)]
pub enum Issue {
    /// Schematic device with no layout instance.
    Missing { device: String },
    /// Layout instance (id, name) whose device is not in the schematic.
    Extra { id: u64, name: String, at: IRect },
    /// Placed device whose parameters differ from the schematic.
    Params { device: String, ids: Vec<u64>, layout: String, schematic: String, at: IRect },
    /// Layout conductor connecting several schematic nets.
    Short { nets: Vec<String>, at: IRect },
    /// Schematic net split over `pieces` layout conductors.
    Open { net: String, pieces: usize, at: IRect },
    /// Named schematic net with no pin label in the layout (external LVS sees no port).
    Pin { net: String, at: IRect },
}

impl Issue {
    pub fn kind(&self) -> &'static str {
        match self {
            Issue::Missing { .. } => "missing",
            Issue::Extra { .. } => "extra",
            Issue::Params { .. } => "parameters",
            Issue::Short { .. } => "short",
            Issue::Open { .. } => "open",
            Issue::Pin { .. } => "pin",
        }
    }

    /// Where to look on the canvas (none for a device that is not placed).
    pub fn at(&self) -> Option<IRect> {
        match self {
            Issue::Missing { .. } => None,
            Issue::Extra { at, .. }
            | Issue::Params { at, .. }
            | Issue::Short { at, .. }
            | Issue::Open { at, .. }
            | Issue::Pin { at, .. } => Some(*at),
        }
    }

    pub fn message(&self) -> String {
        match self {
            Issue::Missing { device } => format!("{device}: in the schematic, not placed"),
            Issue::Extra { name, .. } => format!("{name}: placed, but not in the schematic"),
            Issue::Params { device, layout, schematic, .. } => format!("{device}: layout {layout}, schematic {schematic}"),
            Issue::Short { nets, .. } => format!("short between nets {}", nets.join(", ")),
            Issue::Open { net, pieces, .. } => format!("net {net} is open: {pieces} unconnected pieces"),
            Issue::Pin { net, .. } => format!("net {net} has no pin label"),
        }
    }
}

/// Result of [`check`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SourceCheck {
    pub issues: Vec<Issue>,
    /// Schematic MOS devices compared.
    pub devices: usize,
    /// Schematic nets with terminals in the layout.
    pub nets: usize,
}

impl SourceCheck {
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }

    pub fn count(&self, kind: &str) -> usize {
        self.issues.iter().filter(|i| i.kind() == kind).count()
    }

    pub fn summary(&self) -> String {
        if self.is_clean() {
            return format!("layout matches the schematic ({} devices, {} nets)", self.devices, self.nets);
        }
        let parts: Vec<String> = ["missing", "extra", "parameters", "short", "open", "pin"]
            .iter()
            .filter_map(|k| {
                let n = self.count(k);
                (n > 0).then(|| format!("{n} {k}"))
            })
            .collect();
        format!("{} difference(s): {}", self.issues.len(), parts.join(", "))
    }
}

/// Instances placed by schematic-driven layout that are not devices of their own.
fn is_support(name: &str) -> bool {
    name.starts_with(NTAP) || name.starts_with(PTAP) || name.starts_with(DUMMY)
}

fn pcell_of<'a>(project: &'a Project, inst: &crate::layout::CellInst) -> Option<&'a PCell> {
    project.cell(&inst.cell)?.pcell.as_ref()
}

fn um(v: i64, tech: &Tech) -> String {
    let s = format!("{:.4}", v as f64 / tech.dbu_per_um.max(1) as f64);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    format!("{s}u")
}

fn describe(p: &PCell, total_w: i64, tech: &Tech) -> String {
    match p {
        PCell::Mos { pmos, l, device, .. } => {
            let k = if device.is_empty() { if *pmos { "pmos" } else { "nmos" } } else { device.as_str() };
            format!("{k} W={} L={}", um(total_w, tech), um(*l, tech))
        }
        other => other.summary(tech.dbu_per_um),
    }
}

/// Total gate width of a MOS PCell.
fn total_w(p: &PCell) -> i64 {
    match p {
        PCell::Mos { w, fingers, .. } => w * (*fingers).max(1) as i64,
        _ => 0,
    }
}

/// Whether two MOS PCells describe the same device apart from how the width is split.
fn same_device(a: &PCell, b: &PCell) -> bool {
    match (a, b) {
        (PCell::Mos { pmos: p1, l: l1, device: d1, .. }, PCell::Mos { pmos: p2, l: l2, device: d2, .. }) => {
            p1 == p2 && l1 == l2 && d1 == d2
        }
        _ => a == b,
    }
}

/// Compare `layout` (the schematic's layout view; its placed cells come from `project`) with
/// `sch`.
pub fn check(sch: &Schematic, layout: &Layout, project: &Project) -> SourceCheck {
    let tech = &project.tech;
    let mut out = SourceCheck::default();
    let mos: BTreeMap<&str, &crate::schematic::Instance> =
        sch.instances.iter().filter(|i| i.kind.is_mos() && !i.lvs_ignore()).map(|i| (i.name.as_str(), i)).collect();
    let all_names: BTreeSet<&str> = sch.instances.iter().map(|i| i.name.as_str()).collect();
    out.devices = mos.len();

    // Layout instances per schematic device.
    // A second instance with a name already seen (a pasted copy keeps the name) is extra, not
    // another unit of the device.
    let mut placed: BTreeMap<&str, Vec<&crate::layout::CellInst>> = BTreeMap::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for inst in &layout.instances {
        let Some(name) = inst.name.as_deref() else { continue };
        if is_support(name) {
            continue;
        }
        let dev = device_of(name);
        if all_names.contains(dev) && seen.insert(name) {
            placed.entry(dev).or_default().push(inst);
        } else {
            let at = inst_bbox(inst, project);
            out.issues.push(Issue::Extra { id: inst.id, name: name.to_string(), at });
        }
    }
    for (&name, inst) in &mos {
        let Some(insts) = placed.get(name) else {
            out.issues.push(Issue::Missing { device: name.to_string() });
            continue;
        };
        let Ok(want) = sdl::mos_pcell(inst, tech) else { continue };
        let have: Vec<&PCell> = insts.iter().filter_map(|i| pcell_of(project, i)).collect();
        if have.is_empty() {
            continue; // placed as a plain cell: nothing to compare
        }
        let whole = insts.len() == 1 && insts[0].name.as_deref() == Some(name);
        let have_w: i64 = have.iter().map(|p| total_w(p)).sum();
        let matches =
            if whole { *have[0] == want } else { have.iter().all(|p| same_device(p, &want)) && have_w == total_w(&want) };
        if !matches {
            let at = insts.iter().map(|i| inst_bbox(i, project)).reduce(|a, b| a.union(&b)).unwrap_or_default();
            let layout_desc = if whole { have[0].summary(tech.dbu_per_um) } else { describe(have[0], have_w, tech) };
            let sch_desc = if whole { want.summary(tech.dbu_per_um) } else { describe(&want, total_w(&want), tech) };
            out.issues.push(Issue::Params {
                device: name.to_string(),
                ids: insts.iter().map(|i| i.id).collect(),
                layout: layout_desc,
                schematic: sch_desc,
                at,
            });
        }
    }

    // Shorts and opens, from the conductors under each device terminal.
    let terms = sdl::terminals(sch, layout, project);
    out.nets = terms.len();
    let labels = crate::route::top_labels(layout);
    // With schematic pins, the ports need layout pins; without, every named net does.
    let ports: BTreeSet<String> = sch.ports().into_iter().map(|(n, _)| n.to_ascii_lowercase()).collect();
    for (net, pts) in terms.iter().filter(|(n, _)| ports.is_empty() || ports.contains(&n.to_ascii_lowercase())) {
        if let (Some(text), Some(&p)) = (crate::route::pin_text(net), pts.first())
            && !labels.contains(&text.to_ascii_lowercase())
        {
            out.issues.push(Issue::Pin { net: net.clone(), at: IRect::new(p.0, p.1, p.0, p.1) });
        }
    }
    if terms.values().any(|t| !t.is_empty()) {
        let ex = extract::extract(layout, project, tech);
        let mut by_layout: BTreeMap<usize, (BTreeSet<String>, IRect)> = BTreeMap::new();
        for (net, pts) in &terms {
            let mut pieces: BTreeSet<Option<usize>> = BTreeSet::new();
            let mut floating = 0;
            let mut at: Option<IRect> = None;
            for &p in pts {
                let pr = IRect::new(p.0, p.1, p.0, p.1);
                at = Some(at.map_or(pr, |a| a.union(&pr)));
                match sdl::pin_net(&ex, tech, p) {
                    Some(k) => {
                        pieces.insert(Some(k));
                        let e = by_layout.entry(k).or_insert_with(|| (BTreeSet::new(), pr));
                        e.0.insert(net.clone());
                        e.1 = e.1.union(&pr);
                    }
                    None => floating += 1,
                }
            }
            let n = pieces.len() + floating;
            if n > 1 {
                out.issues.push(Issue::Open { net: net.clone(), pieces: n, at: at.unwrap_or_default() });
            }
        }
        let mut shorts: Vec<(Vec<String>, IRect)> = by_layout
            .into_values()
            .filter(|(nets, _)| nets.len() > 1)
            .map(|(nets, at)| (nets.into_iter().collect(), at))
            .collect();
        shorts.sort_by(|a, b| a.0.cmp(&b.0));
        out.issues.extend(shorts.into_iter().map(|(nets, at)| Issue::Short { nets, at }));
    }
    out
}

fn inst_bbox(inst: &crate::layout::CellInst, src: &dyn LayoutSource) -> IRect {
    let child = src.layout(&inst.cell).and_then(Layout::shapes_bbox).unwrap_or_default();
    inst.bbox(child)
}

/// Place a pin shape with its label for every schematic pin that has no label in `layout` yet,
/// on the cell boundary (Layout XL pin placement): inputs on the left, outputs on the right,
/// inouts on top, evenly spread, on the first metal (labels on its net label layer), outside
/// the layout's extent by the technology's largest spacing. The router then wires them like
/// device terminals. Returns the pins placed.
pub fn place_pins(project: &Project, layout: &mut Layout, sch: &Schematic) -> Vec<String> {
    use crate::schematic::PortDir;
    let tech = &project.tech;
    let Some(m1) = tech.metal1() else { return vec![] };
    let label_layer = tech.net_label_layer(m1).unwrap_or(m1);
    let have = crate::route::top_labels(layout);
    let todo: Vec<(String, PortDir)> = sch.ports().into_iter().filter(|(n, _)| !have.contains(&n.to_ascii_lowercase())).collect();
    if todo.is_empty() {
        return vec![];
    }
    let flat = crate::hier::flatten(layout, project, 64);
    let ext = flat.iter().map(|s| s.geom.bbox()).reduce(|a, b| a.union(&b)).unwrap_or(IRect::new(0, 0, 0, 0));
    let g = tech.grid.max(1);
    let snap = |v: i64| v.div_euclid(g) * g;
    let gap = tech.layers.iter().map(|l| l.min_spacing).max().unwrap_or(0).max(g) * 2;
    let side = {
        let w = tech.layer(m1).map_or(0, |d| d.min_width);
        let via = tech.vias.iter().map(|v| v.cut_size + 2 * v.enc_bottom.max(v.enc_top)).max().unwrap_or(0);
        snap(w.max(via).max(g) + g)
    };
    let size = (tech.dbu_per_um / 5).max(1);
    let mut placed = Vec::new();
    // Pins on one side are at least a pin and a spacing apart, centred on the extent when it is
    // too short for them.
    let pitch = snap(side + gap) + g;
    let spread = |lo: i64, len: i64, n: i64, k: i64| {
        let span = len.max(n * pitch);
        let start = lo + len / 2 - span / 2;
        start + span * (2 * k + 1) / (2 * n)
    };
    for dir in [PortDir::Input, PortDir::Output, PortDir::InOut] {
        let pins: Vec<&String> = todo.iter().filter(|(_, d)| *d == dir).map(|(n, _)| n).collect();
        let n = pins.len() as i64;
        for (k, name) in pins.into_iter().enumerate() {
            let k = k as i64;
            let c = match dir {
                PortDir::Input => (ext.x0 - gap - side / 2, spread(ext.y0, ext.height(), n, k)),
                PortDir::Output => (ext.x1 + gap + side / 2, spread(ext.y0, ext.height(), n, k)),
                PortDir::InOut => (spread(ext.x0, ext.width(), n, k), ext.y1 + gap + side / 2),
            };
            let c = (snap(c.0), snap(c.1));
            layout.add_rect(m1, c.0 - side / 2, c.1 - side / 2, c.0 + side / 2, c.1 + side / 2);
            layout.add(label_layer, crate::layout::Geometry::Label { text: name.clone(), at: c, size });
            placed.push(name.clone());
        }
    }
    placed
}

/// What [`update`] does.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct UpdateOptions {
    /// Delete layout devices that are no longer in the schematic.
    pub remove_extra: bool,
}

/// What [`update`] changed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UpdateReport {
    pub placed: Vec<String>,
    pub updated: Vec<String>,
    pub removed: Vec<String>,
    /// Pin labels added (nets that are fully connected).
    pub pins: Vec<String>,
    /// Shorts and opens left for wiring, and devices that could not be updated.
    pub remaining: Vec<String>,
}

impl UpdateReport {
    pub fn changed(&self) -> bool {
        !(self.placed.is_empty() && self.updated.is_empty() && self.removed.is_empty() && self.pins.is_empty())
    }

    pub fn summary(&self) -> String {
        let list = |v: &[String]| if v.is_empty() { "none".to_string() } else { v.join(", ") };
        format!(
            "placed {}; updated {}; removed {}; pins {}{}",
            list(&self.placed),
            list(&self.updated),
            list(&self.removed),
            list(&self.pins),
            if self.remaining.is_empty() { String::new() } else { format!("; {} left to wire or fix", self.remaining.len()) }
        )
    }
}

/// Update `layout` from `sch` (see the module documentation).
pub fn update(project: &mut Project, layout: &mut Layout, sch: &Schematic, opts: UpdateOptions) -> Result<UpdateReport, String> {
    let before = check(sch, layout, project);
    let mut rep = UpdateReport::default();
    for issue in &before.issues {
        match issue {
            Issue::Extra { id, name, .. } if opts.remove_extra => {
                layout.remove(*id);
                rep.removed.push(name.clone());
            }
            Issue::Params { device, ids, .. } => match resize(project, layout, sch, device, ids) {
                Ok(()) => rep.updated.push(device.clone()),
                Err(e) => rep.remaining.push(format!("{device}: {e}")),
            },
            _ => {}
        }
    }
    let plan = sdl::plan(sch, &project.tech);
    rep.placed = sdl::apply(project, layout, &plan)?;
    // Boundary pins for schematic pins that have none.
    rep.pins = place_pins(project, layout, sch);
    // Pin labels for the named nets that are connected.
    let mid = check(sch, layout, project);
    let open: BTreeSet<&str> =
        mid.issues.iter().filter_map(|i| if let Issue::Open { net, .. } = i { Some(net.as_str()) } else { None }).collect();
    let terms = sdl::terminals(sch, layout, project);
    let at: Vec<(String, (i64, i64))> = mid
        .issues
        .iter()
        .filter_map(|i| match i {
            Issue::Pin { net, .. } if !open.contains(net.as_str()) => Some((net.clone(), *terms.get(net)?.first()?)),
            _ => None,
        })
        .collect();
    crate::route::label_nets(layout, &project.tech, &at);
    rep.pins.extend(at.into_iter().map(|(n, _)| n));
    let after = check(sch, layout, project);
    rep.remaining.extend(
        after
            .issues
            .iter()
            .filter(|i| matches!(i, Issue::Short { .. } | Issue::Open { .. } | Issue::Missing { .. } | Issue::Pin { .. }))
            .map(Issue::message),
    );
    Ok(rep)
}

/// Give the instances of `device` the schematic's parameters, in place.
fn resize(project: &mut Project, layout: &mut Layout, sch: &Schematic, device: &str, ids: &[u64]) -> Result<(), String> {
    let inst = sch.instances.iter().find(|i| i.name == device).ok_or("not in the schematic")?;
    let want = sdl::mos_pcell(inst, &project.tech)?;
    let PCell::Mos { w: want_w, fingers: want_nf, .. } = want else { return Err("not a MOS device".into()) };
    let whole = ids.len() == 1 && layout.instance(ids[0]).and_then(|i| i.name.as_deref()) == Some(device);
    let grid = project.tech.grid.max(1);
    let mut targets = Vec::new();
    if whole {
        targets.push((ids[0], want.clone()));
    } else {
        // Units keep their count and fingers; the finger width carries the new total width.
        let have: Vec<(u64, u32)> = ids
            .iter()
            .filter_map(|&id| {
                let i = layout.instance(id)?;
                match project.cell(&i.cell)?.pcell.as_ref()? {
                    PCell::Mos { fingers, .. } => Some((id, (*fingers).max(1))),
                    _ => None,
                }
            })
            .collect();
        let fingers: i64 = have.iter().map(|h| h.1 as i64).sum();
        if fingers == 0 {
            return Err("no MOS units".into());
        }
        let total = want_w * want_nf.max(1) as i64;
        if total % (fingers * grid) != 0 {
            return Err(format!(
                "W={}u does not split evenly over {fingers} finger(s) on the {}u grid; resize the units by hand",
                total as f64 / project.tech.dbu_per_um as f64,
                grid as f64 / project.tech.dbu_per_um as f64,
            ));
        }
        let w = total / fingers;
        for (id, nf) in have {
            let mut p = want.clone();
            if let PCell::Mos { w: pw, fingers: pf, .. } = &mut p {
                (*pw, *pf) = (w, nf);
            }
            targets.push((id, p));
        }
    }
    for (id, p) in targets {
        let cell = project.ensure_pcell(&p)?;
        if let Some(i) = layout.instance_mut(id) {
            i.cell = cell;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::examples;

    fn demo() -> (Project, crate::CellRef) {
        let p = examples::demo_project();
        let r = p.all_cells().map(|(r, _)| r).find(|r| r.cell == "cmos_inverter").expect("inverter");
        (p, r)
    }

    fn generated(p: &mut Project, r: &crate::CellRef) -> (Schematic, Layout) {
        let sch = p.cell(r).and_then(|c| c.schematic.clone()).expect("schematic");
        let mut lay = Layout::default();
        let plan = sdl::plan(&sch, &p.tech);
        sdl::apply(p, &mut lay, &plan).expect("apply");
        (sch, lay)
    }

    #[test]
    fn fresh_placement_has_only_opens() {
        let (mut p, r) = demo();
        let (sch, lay) = generated(&mut p, &r);
        let c = check(&sch, &lay, &p);
        assert!(c.devices >= 2);
        assert_eq!(c.count("missing") + c.count("extra") + c.count("parameters") + c.count("short"), 0, "{:?}", c.issues);
    }

    #[test]
    fn detects_and_updates_missing_extra_and_params() {
        let (mut p, r) = demo();
        let (mut sch, mut lay) = generated(&mut p, &r);
        let names: Vec<String> = sch.instances.iter().filter(|i| i.kind.is_mos()).map(|i| i.name.clone()).collect();
        // Rename one device (extra + missing) and widen the other (parameters).
        let renamed = names[0].clone();
        let widened = names[1].clone();
        for i in sch.instances.iter_mut() {
            if i.name == renamed {
                i.name = format!("{renamed}x");
            } else if i.name == widened {
                i.params.insert("w".into(), "3u".into());
            }
        }
        let c = check(&sch, &lay, &p);
        assert_eq!(c.count("missing"), 1, "{:?}", c.issues);
        assert_eq!(c.count("extra"), 1, "{:?}", c.issues);
        assert_eq!(c.count("parameters"), 1, "{:?}", c.issues);

        let rep = update(&mut p, &mut lay, &sch, UpdateOptions { remove_extra: true }).expect("update");
        assert_eq!(rep.removed, vec![renamed.clone()]);
        assert_eq!(rep.updated, vec![widened.clone()]);
        assert_eq!(rep.placed, vec![format!("{renamed}x")]);
        let c = check(&sch, &lay, &p);
        assert_eq!(c.count("missing") + c.count("extra") + c.count("parameters"), 0, "{:?}", c.issues);
    }

    #[test]
    fn detects_short() {
        let (mut p, r) = demo();
        let (sch, mut lay) = generated(&mut p, &r);
        // A metal1 sheet over the whole placement shorts every terminal together.
        let m1 = p.tech.metal1().expect("metal1");
        let b = lay.instances.iter().map(|i| inst_bbox(i, &p)).reduce(|a, b| a.union(&b)).expect("bbox");
        lay.add(m1, crate::layout::Geometry::Rect(b));
        let c = check(&sch, &lay, &p);
        assert_eq!(c.count("short"), 1, "{:?}", c.issues);
        assert_eq!(c.count("open"), 0, "{:?}", c.issues);
    }

    #[test]
    fn missing_pins_are_reported_and_added() {
        let (mut p, r) = demo();
        let (sch, mut lay) = generated(&mut p, &r);
        crate::route::route(&sch, &mut lay, &p, &p.tech).expect("route");
        assert!(check(&sch, &lay, &p).is_clean(), "{:?}", check(&sch, &lay, &p).issues);
        let labels = lay.shapes.iter().filter(|s| matches!(s.geom, crate::layout::Geometry::Label { .. })).count();
        lay.shapes.retain(|s| !matches!(s.geom, crate::layout::Geometry::Label { .. }));
        let c = check(&sch, &lay, &p);
        assert_eq!(c.count("pin"), labels, "{:?}", c.issues);
        let rep = update(&mut p, &mut lay, &sch, UpdateOptions::default()).expect("update");
        assert_eq!(rep.pins.len(), labels, "{rep:?}");
        assert!(check(&sch, &lay, &p).is_clean());
    }

    /// Schematic pins become boundary pins that the router connects, and the check is clean.
    #[test]
    fn boundary_pins_are_placed_and_routed() {
        let (mut p, r) = demo();
        let (mut sch, mut lay) = generated(&mut p, &r);
        sch.mark_ports(&[("in".into(), crate::PortDir::Input), ("out".into(), crate::PortDir::Output)]);
        let pins = place_pins(&p, &mut lay, &sch);
        assert_eq!(pins, vec!["in".to_string(), "out".to_string()]);
        let terms = sdl::terminal_pins(&sch, &lay, &p);
        assert!(terms["in"].iter().any(|t| t.0 == "p") && terms["out"].iter().any(|t| t.0 == "p"), "{terms:?}");
        let c = check(&sch, &lay, &p);
        assert!(c.count("open") >= 2 && c.count("pin") == 0, "{:?}", c.issues);
        let rep = crate::route::route(&sch, &mut lay, &p, &p.tech).expect("route");
        assert!(rep.failed.is_empty(), "{rep:?}");
        let c = check(&sch, &lay, &p);
        assert!(c.is_clean(), "{:?}", c.issues);
        assert!(place_pins(&p, &mut lay, &sch).is_empty(), "placed once");
    }

    /// A pin label over a non-conducting shape (a well, the boundary) is not a pin.
    #[test]
    fn pin_label_needs_its_metal() {
        let (mut p, r) = demo();
        let (mut sch, mut lay) = generated(&mut p, &r);
        sch.mark_ports(&[("in".into(), crate::PortDir::Input)]);
        place_pins(&p, &mut lay, &sch);
        let m1 = p.tech.metal1().unwrap();
        let other = (0..p.tech.layers.len()).find(|&k| k != m1 && p.tech.labelled_layer(k).is_none()).unwrap();
        for s in lay.shapes.iter_mut().filter(|s| s.layer == m1 && matches!(s.geom, crate::layout::Geometry::Rect(_))) {
            s.layer = other;
        }
        let terms = sdl::terminal_pins(&sch, &lay, &p);
        assert!(!terms.get("in").is_some_and(|t| t.iter().any(|t| t.0 == "p")), "{terms:?}");
    }

    /// A pasted copy keeps the device's name: it is extra, and the original is left alone.
    #[test]
    fn pasted_copy_is_extra_not_a_unit() {
        let (mut p, r) = demo();
        let (sch, mut lay) = generated(&mut p, &r);
        let orig = lay.instances.iter().find(|i| i.name.as_deref().is_some_and(|n| n.starts_with('M'))).cloned().unwrap();
        let mut copy = orig.clone();
        copy.id = lay.instances.iter().map(|i| i.id).max().unwrap() + 1;
        copy.trans.x += 10_000;
        lay.instances.push(copy.clone());
        let c = check(&sch, &lay, &p);
        assert_eq!((c.count("extra"), c.count("parameters")), (1, 0), "{:?}", c.issues);
        let rep = update(&mut p, &mut lay, &sch, UpdateOptions { remove_extra: true }).unwrap();
        assert!(rep.updated.is_empty(), "{rep:?}");
        assert!(lay.instance(copy.id).is_none() && lay.instance(orig.id).map(|i| &i.cell) == Some(&orig.cell));
    }

    /// Pins on one side never overlap, even with no layout to spread them along.
    #[test]
    fn boundary_pins_keep_apart() {
        let (p, r) = demo();
        let mut sch = p.cell(&r).and_then(|c| c.schematic.clone()).unwrap();
        let ports: Vec<(String, crate::PortDir)> = (0..6).map(|k| (format!("d{k}"), crate::PortDir::Input)).collect();
        for (n, _) in &ports {
            sch.add_label(n, crate::geom::Pt::new(-100, 0));
        }
        sch.mark_ports(&ports);
        let mut lay = Layout::default();
        assert_eq!(place_pins(&p, &mut lay, &sch).len(), 6);
        let rects: Vec<IRect> = lay
            .shapes
            .iter()
            .filter_map(|s| if let crate::layout::Geometry::Rect(r) = s.geom { Some(r) } else { None })
            .collect();
        for (k, a) in rects.iter().enumerate() {
            for b in &rects[k + 1..] {
                assert!(!a.intersects(b) && a != b, "{a:?} overlaps {b:?}");
            }
        }
    }
}
