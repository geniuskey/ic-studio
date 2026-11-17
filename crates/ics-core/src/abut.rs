//! MOS abutment (Virtuoso Layout XL "Abut"): placed MOS PCells of one type and finger width are
//! slid together so the outer source / drain column of one sits on that of its neighbour, sharing
//! the diffusion, with their active areas aligned.
//!
//! When the cell has a schematic, the shared columns must be on the same net: a device whose
//! facing column is on another net is flipped left-right if that puts a matching column there,
//! else the abutment is refused (it would short two nets).

use crate::connectivity;
use crate::layout::{Layout, Trans};
use crate::pcell::{PCell, mos_columns};
use crate::project::Project;
use crate::schematic::Schematic;
use crate::sdl::device_of;

/// A placed MOS PCell as abutment sees it.
struct Mos {
    id: u64,
    name: String,
    pmos: bool,
    w: i64,
    l: i64,
    /// PDK device (Vt flavour); empty for the default one.
    device: String,
    /// Whether the cell has a schematic, so the columns' nets are known (`None` there means the
    /// device or its pin was not found, not "any net").
    checked: bool,
    /// Source / drain column centres (parent x) with their nets, left to right.
    cols: Vec<(i64, Option<String>)>,
    /// Bottom of the active area (parent y).
    active_y0: i64,
    trans: Trans,
    /// Centre x of the placement (parent), the flip axis.
    cx: i64,
}

/// The schematic and its connectivity, extracted once per operation.
type Source<'a> = Option<(&'a Schematic, &'a connectivity::Connectivity)>;

fn mos(layout: &Layout, id: u64, project: &Project, src: Source) -> Result<Mos, String> {
    let tech = &project.tech;
    let inst = layout.instance(id).ok_or("not an instance")?;
    let name = inst.name.clone().unwrap_or_else(|| format!("#{id}"));
    if inst.array.is_some() {
        return Err(format!("{name}: arrayed placements cannot be abutted"));
    }
    if inst.trans.rot % 2 == 1 {
        return Err(format!("{name}: turned by 90°, its columns are not vertical"));
    }
    let cell = project.cell(&inst.cell).ok_or_else(|| format!("{name}: cell {} not found", inst.cell))?;
    let Some(PCell::Mos { pmos, w, l, fingers, device }) = cell.pcell.clone() else {
        return Err(format!("{name}: not a MOS PCell"));
    };
    let child = cell.layout.as_ref().ok_or_else(|| format!("{name}: no layout"))?;
    let active = tech.diffusion();
    let active_y0 = child
        .shapes
        .iter()
        .filter(|s| Some(s.layer) == active)
        .map(|s| inst.trans.apply_rect(s.bbox()).y0)
        .min()
        .ok_or_else(|| format!("{name}: no active area"))?;
    let nets: Option<(Option<String>, Option<String>)> = src.and_then(|(sch, conn)| {
        let dev = device_of(inst.name.as_deref()?);
        let si = sch.instances.iter().find(|i| i.name == dev)?;
        Some((conn.net_of(si.id, "s").map(str::to_string), conn.net_of(si.id, "d").map(str::to_string)))
    });
    let mut cols: Vec<(i64, Option<String>)> = mos_columns(tech, l, fingers)?
        .into_iter()
        .enumerate()
        .map(|(k, x)| {
            let net = nets.as_ref().and_then(|(s, d)| if k % 2 == 0 { s.clone() } else { d.clone() });
            (inst.trans.apply((x, 0)).0, net)
        })
        .collect();
    cols.sort_by_key(|c| c.0);
    let b = child.shapes_bbox().map(|b| inst.trans.apply_rect(b)).unwrap_or_default();
    let checked = src.is_some();
    Ok(Mos { id, name, pmos, w, l, device, checked, cols, active_y0, trans: inst.trans, cx: (b.x0 + b.x1) / 2 })
}

impl Mos {
    fn flipped(&self) -> Mos {
        let f = Trans::flip_x_about((self.cx, 0));
        let mut cols: Vec<(i64, Option<String>)> = self.cols.iter().map(|(x, n)| (2 * self.cx - x, n.clone())).collect();
        cols.sort_by_key(|c| c.0);
        Mos {
            id: self.id,
            name: self.name.clone(),
            pmos: self.pmos,
            w: self.w,
            l: self.l,
            device: self.device.clone(),
            checked: self.checked,
            cols,
            active_y0: self.active_y0,
            trans: f.compose(&self.trans),
            cx: self.cx,
        }
    }
}

/// Abut the MOS instances `ids` into one chain in left-to-right order: the leftmost stays, each
/// next one is moved (and flipped if its nets need it) onto the right column of the previous one.
/// Returns a line per abutment.
pub fn abut_chain(layout: &mut Layout, ids: &[u64], project: &Project, sch: Option<&Schematic>) -> Result<Vec<String>, String> {
    if ids.len() < 2 {
        return Err("select two or more MOS instances".into());
    }
    let conn = sch.map(connectivity::extract);
    let src = sch.zip(conn.as_ref());
    let mut devs = ids.iter().map(|&id| mos(layout, id, project, src)).collect::<Result<Vec<_>, _>>()?;
    devs.sort_by_key(|d| d.cols.first().map_or(0, |c| c.0));
    let mut notes = Vec::new();
    let mut prev = devs.remove(0);
    for next in devs {
        let id = next.id;
        notes.push(abut_onto(layout, &prev, next, true)?);
        // The moved device is the reference for the next one.
        prev = mos(layout, id, project, src)?;
    }
    Ok(notes)
}

/// Move `moving` onto the right (`right`) or left side of `fixed`, flipping it if its nets
/// need that. Returns a note.
fn abut_onto(layout: &mut Layout, fixed: &Mos, moving: Mos, right: bool) -> Result<String, String> {
    if moving.pmos != fixed.pmos {
        return Err(format!("{} and {}: one is NMOS, the other PMOS", fixed.name, moving.name));
    }
    if moving.w != fixed.w {
        return Err(format!("{} and {}: finger widths differ", fixed.name, moving.name));
    }
    if moving.l != fixed.l || moving.device != fixed.device {
        return Err(format!("{} and {}: different gate lengths or device types", fixed.name, moving.name));
    }
    let fixed_col = if right { fixed.cols.last() } else { fixed.cols.first() };
    let shared = fixed_col.and_then(|c| c.1.clone());
    let facing = |m: &Mos| if right { m.cols.first().cloned() } else { m.cols.last().cloned() };
    // With a schematic, both nets must be known and equal; without one there are no nets.
    let fits = |m: &Mos| match (&shared, facing(m).and_then(|c| c.1)) {
        (Some(a), Some(b)) => *a == b,
        _ => !(fixed.checked || m.checked),
    };
    let (moving, flipped) = if fits(&moving) {
        (moving, false)
    } else {
        let f = moving.flipped();
        if !fits(&f) {
            let theirs = facing(&moving).and_then(|c| c.1).unwrap_or_default();
            return Err(format!(
                "{} and {}: no shared net to abut on ({} has {} on that side, {} has {} at both ends)",
                fixed.name,
                moving.name,
                fixed.name,
                shared.unwrap_or_default(),
                moving.name,
                theirs
            ));
        }
        (f, true)
    };
    let (Some(fc), Some(mc)) = (fixed_col, facing(&moving)) else { return Err("no source / drain columns".into()) };
    let (dx, dy) = (fc.0 - mc.0, fixed.active_y0 - moving.active_y0);
    let inst = layout.instance_mut(moving.id).ok_or("instance not found")?;
    inst.trans = Trans { x: moving.trans.x + dx, y: moving.trans.y + dy, ..moving.trans };
    let net = shared.map(|n| format!(" on net {n}")).unwrap_or_default();
    Ok(format!("{} abutted to {}{net}{}", moving.name, fixed.name, if flipped { ", flipped" } else { "" }))
}

/// After `id` was moved: if one of its outer columns lands within `reach` (dbu) of a facing
/// outer column of another compatible MOS instance, with active areas within `reach`
/// vertically, abut it there (flipping it if the nets need that). Returns the note, or `None`
/// when no neighbour is close or the nets do not allow it (nothing is changed then).
pub fn auto_abut(layout: &mut Layout, id: u64, project: &Project, sch: Option<&Schematic>, reach: i64) -> Option<String> {
    let conn = sch.map(connectivity::extract);
    let src = sch.zip(conn.as_ref());
    let me = mos(layout, id, project, src).ok()?;
    let mut best: Option<(i64, u64, bool)> = None;
    for other in layout.instances.iter().filter(|i| i.id != id) {
        let Ok(o) = mos(layout, other.id, project, src) else { continue };
        if o.pmos != me.pmos || o.w != me.w || o.l != me.l || o.device != me.device || (o.active_y0 - me.active_y0).abs() > reach
        {
            continue;
        }
        let (Some(of), Some(ol), Some(mf), Some(ml)) = (o.cols.first(), o.cols.last(), me.cols.first(), me.cols.last()) else {
            continue;
        };
        // `me` to the right of `o`, or to its left.
        for (d, right) in [((mf.0 - ol.0).abs(), true), ((ml.0 - of.0).abs(), false)] {
            let side_ok = if right { mf.0 >= of.0 } else { ml.0 <= ol.0 };
            if side_ok && d <= reach && best.is_none_or(|b| d < b.0) {
                best = Some((d, o.id, right));
            }
        }
    }
    let (_, other, right) = best?;
    let fixed = mos(layout, other, project, src).ok()?;
    let mut next = layout.clone();
    let note = abut_onto(&mut next, &fixed, me, right).ok()?;
    *layout = next;
    Some(note)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::examples;
    use crate::sdl;

    /// Two series NMOS of a NAND share their middle net: pulled apart and flipped the wrong way,
    /// abutment flips one back and brings their columns together.
    #[test]
    fn abuts_on_the_shared_net_and_refuses_otherwise() {
        let mut p = examples::demo_project();
        let r = p.all_cells().map(|(r, _)| r).find(|r| r.cell == "nand2").expect("nand2");
        let sch = p.cell(&r).and_then(|c| c.schematic.clone()).expect("schematic");
        let mut lay = Layout::default();
        let plan = sdl::plan(&sch, &p.tech);
        sdl::apply(&mut p, &mut lay, &plan).expect("apply");
        let conn = connectivity::extract(&sch);
        let nmos: Vec<&crate::Instance> = sch.instances.iter().filter(|i| i.kind.is_mos() && !i.kind.is_pmos()).collect();
        assert_eq!(nmos.len(), 2);
        let ids: Vec<u64> = nmos
            .iter()
            .map(|i| lay.instances.iter().find(|x| x.name.as_deref() == Some(i.name.as_str())).expect("placed").id)
            .collect();
        // Pull the second one away and flip it.
        let far = lay.instance(ids[1]).unwrap().trans;
        lay.instance_mut(ids[1]).unwrap().trans =
            Trans::flip_x_about((0, 0)).compose(&Trans { x: far.x + 5000, y: far.y + 700, ..far });
        let notes = abut_chain(&mut lay, &ids, &p, Some(&sch)).expect("abut");
        assert_eq!(notes.len(), 1, "{notes:?}");
        let a = mos(&lay, ids[0], &p, Some((&sch, &conn))).unwrap();
        let b = mos(&lay, ids[1], &p, Some((&sch, &conn))).unwrap();
        let (left, right) = if a.cols[0].0 < b.cols[0].0 { (a, b) } else { (b, a) };
        assert_eq!(left.cols.last().unwrap().0, right.cols[0].0, "shared column");
        assert_eq!(left.cols.last().unwrap().1, right.cols[0].1, "on one net");
        assert_eq!(left.active_y0, right.active_y0);
        // The shared net is the series node, not a supply or the output.
        let mid = left.cols.last().unwrap().1.clone().unwrap();
        assert!(nmos.iter().all(|i| conn.net_of(i.id, "s") == Some(&mid) || conn.net_of(i.id, "d") == Some(&mid)));

        // A PMOS cannot abut an NMOS.
        let pm = lay
            .instances
            .iter()
            .find(|x| x.name.as_deref().is_some_and(|n| sch.instances.iter().any(|i| i.name == n && i.kind.is_pmos())))
            .unwrap()
            .id;
        assert!(abut_chain(&mut lay, &[ids[0], pm], &p, Some(&sch)).is_err());

        // A device the schematic does not know (unnamed) is not abutted: its nets are unknown.
        lay.instance_mut(ids[1]).unwrap().name = None;
        lay.instance_mut(ids[1]).unwrap().trans.x += 5000;
        assert!(abut_chain(&mut lay, &ids, &p, Some(&sch)).is_err());
    }

    /// A device dropped near a compatible neighbour snaps onto its shared column.
    #[test]
    fn auto_abut_snaps_a_nearby_device() {
        let mut p = examples::demo_project();
        let r = p.all_cells().map(|(r, _)| r).find(|r| r.cell == "nand2").expect("nand2");
        let sch = p.cell(&r).and_then(|c| c.schematic.clone()).expect("schematic");
        let mut lay = Layout::default();
        let plan = sdl::plan(&sch, &p.tech);
        sdl::apply(&mut p, &mut lay, &plan).expect("apply");
        let ids: Vec<u64> = sch
            .instances
            .iter()
            .filter(|i| i.kind.is_mos() && !i.kind.is_pmos())
            .map(|i| lay.instances.iter().find(|x| x.name.as_deref() == Some(i.name.as_str())).unwrap().id)
            .collect();
        let before = lay.instance(ids[1]).unwrap().trans;
        // Far away: nothing happens.
        lay.instance_mut(ids[1]).unwrap().trans.x += 20_000;
        assert!(auto_abut(&mut lay, ids[1], &p, Some(&sch), 300).is_none());
        // Dropped 120 nm off: snaps back.
        lay.instance_mut(ids[1]).unwrap().trans = Trans { x: before.x + 120, y: before.y - 50, ..before };
        let note = auto_abut(&mut lay, ids[1], &p, Some(&sch), 300).expect("abutted");
        assert!(note.contains("abutted"), "{note}");
        assert_eq!(lay.instance(ids[1]).unwrap().trans, before);
    }
}
