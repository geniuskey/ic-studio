//! Layout generators: guard rings around an area and via arrays (Virtuoso "Create Guard Ring" /
//! "Create Via Array"). All output is in dbu on the technology grid, as (layer, geometry) pairs.

use crate::geom::IRect;
use crate::hier::{LayoutSource, flatten};
use crate::layout::{Geometry, Layout};
use crate::region::{BoolOp, Region};
use crate::tech::{Tech, ViaDef};

/// A diffusion guard ring around `area`, drawn as four rectangles per ring layer (bottom and top
/// span the full width, left and right fill between them).
#[derive(Debug, Clone, PartialEq)]
pub struct GuardRing {
    /// Inner keep area the ring surrounds (snapped outward to the grid).
    pub area: IRect,
    /// `true`: n+ ring in an nwell covering ring and area (PMOS guard); `false`: p+ substrate ring.
    pub well: bool,
    /// Diffusion ring width; 0 = the minimum for one contact row.
    pub width: i64,
    /// Ring inner edge to `area`; 0 = the larger of the diffusion spacing and twice the implant
    /// enclosure (so device implants inside the area keep clear of the ring implant).
    pub gap: i64,
    /// Contact cut arrays along each side.
    pub contacts: bool,
    /// Metal1 strap over the ring (same footprint as the diffusion).
    pub metal: bool,
}

/// A via array to fill `area` with.
#[derive(Debug, Clone, PartialEq)]
pub struct ViaArray {
    pub area: IRect,
    /// [`ViaDef`] name.
    pub via: String,
}

pub fn guard_ring(tech: &Tech, g: &GuardRing) -> Result<Vec<(usize, Geometry)>, String> {
    let grid = tech.grid.max(1);
    let active = tech.tap_layer().ok_or_else(|| format!("technology '{}' has no tap layer", tech.name))?;
    let con = tech
        .diff_contact()
        .ok_or_else(|| format!("technology '{}' has no diffusion contact '{}'", tech.name, tech.roles.diff_contact))?;
    let (_, cut, m1) = via_layers(tech, con)?;
    let imp = implant(tech, !g.well)?;
    let enc = con.enc_bottom.max(con.enc_top);
    let mut need = snap_up(min_width(tech, active), grid);
    if g.contacts {
        need = need.max(snap_up(con.cut_size + 2 * enc, grid));
    }
    if g.metal {
        need = need.max(snap_up(min_width(tech, m1), grid));
    }
    let w = if g.width <= 0 { need } else { snap_up(g.width, grid) };
    if w < need {
        return Err(format!("guard ring width {w} is below the minimum {need}"));
    }
    let gap =
        if g.gap <= 0 { snap_up(min_spacing(tech, active).max(2 * tech.rules.implant_enc), grid) } else { snap_up(g.gap, grid) };
    let area =
        IRect::new(snap_down(g.area.x0, grid), snap_down(g.area.y0, grid), snap_up(g.area.x1, grid), snap_up(g.area.y1, grid));
    let outer = grow(area, gap + w);
    let ring = ring_rects(outer, w);
    let mut out: Vec<(usize, Geometry)> = ring.iter().map(|r| (active, Geometry::Rect(*r))).collect();
    if let Some(imp) = imp {
        let e = snap_up(tech.rules.implant_enc, grid);
        let io = grow(outer, e);
        let iw = (w + 2 * e).max(snap_up(min_width(tech, imp), grid));
        let hole = grow(io, -iw);
        let sp = min_spacing(tech, imp);
        if hole.x1 - hole.x0 < sp.max(1) || hole.y1 - hole.y0 < sp.max(1) {
            out.push((imp, Geometry::Rect(io)));
        } else {
            out.extend(ring_rects(io, iw).into_iter().map(|r| (imp, Geometry::Rect(r))));
        }
    }
    if g.well {
        let nwell = tech.nwell().ok_or_else(|| format!("technology '{}' has no nwell layer", tech.name))?;
        let nw_min = snap_up(min_width(tech, nwell), grid);
        out.push((nwell, Geometry::Rect(grow_to(grow(outer, snap_up(tech.rules.nwell_enc, grid)), nw_min, nw_min, grid))));
    }
    if g.contacts {
        let pitch = snap_up(con.cut_size + con.cut_spacing, grid);
        let [b, t, l, r] = ring;
        let bottom = cut_grid(grow(b, -enc), con.cut_size, pitch, grid);
        let top = cut_grid(grow(t, -enc), con.cut_size, pitch, grid);
        let ylo = bottom.iter().map(|c| c.y1).max().unwrap_or(b.y1) + con.cut_spacing;
        let yhi = top.iter().map(|c| c.y0).min().unwrap_or(t.y0) - con.cut_spacing;
        let side = |s: IRect| {
            if yhi - ylo < con.cut_size {
                return vec![];
            }
            let ylo = snap_up(ylo, grid);
            let yhi = snap_down(yhi, grid);
            cut_grid(IRect { x0: s.x0 + enc, y0: ylo, x1: s.x1 - enc, y1: yhi }, con.cut_size, pitch, grid)
        };
        let (left, right) = (side(l), side(r));
        out.extend(bottom.into_iter().chain(top).chain(left).chain(right).map(|c| (cut, Geometry::Rect(c))));
    }
    if g.metal {
        out.extend(ring.iter().map(|r| (m1, Geometry::Rect(*r))));
    }
    Ok(out)
}

/// Cuts (and enclosure pads) filling `area` with the via's cut array; fails if the area is too small for one cut.
///
/// The cuts are the largest grid that keeps both enclosures inside `area` (snapped inward to the
/// grid), centred. Each pad is the cut array's box grown by its layer's enclosure, then to the
/// layer's minimum width and area.
pub fn via_array(tech: &Tech, v: &ViaArray) -> Result<Vec<(usize, Geometry)>, String> {
    let via = tech.via(&v.via).ok_or_else(|| format!("technology '{}' has no via '{}'", tech.name, v.via))?;
    via_array_def(tech, via, v.area)
}

/// Via arrays for every overlap of shapes on the via's bottom and top layers within `area` (flattened from `top`/`src`).
///
/// Overlaps too small for one cut are skipped; cuts too close to ones already placed (where an
/// overlap splits into several rectangles) are dropped.
pub fn via_arrays_at_overlaps(
    top: &Layout,
    src: &dyn LayoutSource,
    tech: &Tech,
    via: &str,
    area: Option<IRect>,
) -> Result<Vec<(usize, Geometry)>, String> {
    let def = tech.via(via).ok_or_else(|| format!("technology '{}' has no via '{via}'", tech.name))?;
    let (b, c, t) = via_layers(tech, def)?;
    let flat = flatten(top, src, u32::MAX);
    let region = |layer: usize| {
        Region::from_rects(
            flat.iter()
                .filter(|s| s.layer == layer)
                .filter_map(|s| Region::from_geometry(&s.geom).ok())
                .flat_map(|r| r.rects().to_vec()),
        )
    };
    let mut overlap = region(b).boolean(&region(t), BoolOp::And);
    if let Some(a) = area {
        overlap = overlap.boolean(&Region::from_rects([a]), BoolOp::And);
    }
    let mut out = vec![];
    let mut cuts: Vec<IRect> = vec![];
    for part in overlap.parts() {
        for r in part.rects() {
            let Ok(items) = via_array_def(tech, def, *r) else { continue };
            let fresh: Vec<IRect> = items
                .iter()
                .filter(|(l, _)| *l == c)
                .map(|(_, g)| g.bbox())
                .filter(|n| cuts.iter().all(|o| !o.intersects(n) && o.spacing(n) >= def.cut_spacing))
                .collect();
            if fresh.is_empty() {
                continue;
            }
            let bbox = fresh.iter().copied().reduce(|a, b| a.union(&b)).unwrap_or_default();
            out.push((b, Geometry::Rect(pad(tech, b, bbox, def.enc_bottom))));
            out.extend(fresh.iter().map(|r| (c, Geometry::Rect(*r))));
            out.push((t, Geometry::Rect(pad(tech, t, bbox, def.enc_top))));
            cuts.extend(fresh);
        }
    }
    Ok(out)
}

/// Vias connecting layers `bottom` and `top` (in either order).
pub fn vias_between(tech: &Tech, bottom: usize, top: usize) -> Vec<&ViaDef> {
    tech.vias
        .iter()
        .filter(|v| {
            let (b, t) = (tech.layer_index(&v.bottom), tech.layer_index(&v.top));
            (b == Some(bottom) && t == Some(top)) || (b == Some(top) && t == Some(bottom))
        })
        .collect()
}

fn via_array_def(tech: &Tech, via: &ViaDef, area: IRect) -> Result<Vec<(usize, Geometry)>, String> {
    let (b, c, t) = via_layers(tech, via)?;
    let grid = tech.grid.max(1);
    let enc = via.enc_bottom.max(via.enc_top);
    let area = IRect::new(snap_up(area.x0, grid), snap_up(area.y0, grid), snap_down(area.x1, grid), snap_down(area.y1, grid));
    let inner = grow(area, -enc);
    if inner.x1 - inner.x0 < via.cut_size || inner.y1 - inner.y0 < via.cut_size {
        return Err(format!("area {}x{} is too small for one '{}' cut", area.width(), area.height(), via.name));
    }
    let cuts = cut_grid(inner, via.cut_size, snap_up(via.cut_size + via.cut_spacing, grid), grid);
    let bbox = cuts.iter().copied().reduce(|a, b| a.union(&b)).unwrap_or(inner);
    let mut out = vec![(b, Geometry::Rect(pad(tech, b, bbox, via.enc_bottom)))];
    out.extend(cuts.into_iter().map(|r| (c, Geometry::Rect(r))));
    out.push((t, Geometry::Rect(pad(tech, t, bbox, via.enc_top))));
    Ok(out)
}

fn pad(tech: &Tech, layer: usize, cuts: IRect, enc: i64) -> IRect {
    let grid = tech.grid.max(1);
    let mw = snap_up(min_width(tech, layer), grid);
    grow_to_area(grow_to(grow(cuts, enc), mw, mw, grid), tech.layer(layer).map_or(0, |l| l.min_area), grid)
}

fn via_layers(tech: &Tech, via: &ViaDef) -> Result<(usize, usize, usize), String> {
    if via.cut_size <= 0 || via.cut_spacing < 0 {
        return Err(format!("via '{}' has an invalid cut size or spacing", via.name));
    }
    let layer = |n: &str| tech.layer_index(n).ok_or_else(|| format!("technology '{}' has no layer '{n}'", tech.name));
    Ok((layer(&via.bottom)?, layer(&via.cut)?, layer(&via.top)?))
}

fn implant(tech: &Tech, pmos: bool) -> Result<Option<usize>, String> {
    if !pmos && tech.roles.nimplant.is_empty() {
        return Ok(None);
    }
    tech.implant(pmos).map(Some).ok_or_else(|| format!("technology '{}' has no implant layer", tech.name))
}

fn min_width(tech: &Tech, layer: usize) -> i64 {
    tech.layer(layer).map_or(0, |l| l.min_width)
}

fn min_spacing(tech: &Tech, layer: usize) -> i64 {
    tech.layer(layer).map_or(0, |l| l.min_spacing)
}

/// Bottom, top, left and right sides of a ring of width `w` inside `outer`.
fn ring_rects(o: IRect, w: i64) -> [IRect; 4] {
    [
        IRect { x0: o.x0, y0: o.y0, x1: o.x1, y1: o.y0 + w },
        IRect { x0: o.x0, y0: o.y1 - w, x1: o.x1, y1: o.y1 },
        IRect { x0: o.x0, y0: o.y0 + w, x1: o.x0 + w, y1: o.y1 - w },
        IRect { x0: o.x1 - w, y0: o.y0 + w, x1: o.x1, y1: o.y1 - w },
    ]
}

/// Largest centred grid of `cut` x `cut` squares at `pitch` inside `inner`.
fn cut_grid(inner: IRect, cut: i64, pitch: i64, g: i64) -> Vec<IRect> {
    let axis = |len: i64| -> (i64, i64) {
        if len < cut {
            return (0, 0);
        }
        let n = (len - cut) / pitch + 1;
        (n, snap_down((len - (n - 1) * pitch - cut) / 2, g))
    };
    let ((nx, ox), (ny, oy)) = (axis(inner.x1 - inner.x0), axis(inner.y1 - inner.y0));
    let mut out = Vec::with_capacity((nx * ny) as usize);
    for j in 0..ny {
        for i in 0..nx {
            let (x, y) = (inner.x0 + ox + i * pitch, inner.y0 + oy + j * pitch);
            out.push(IRect { x0: x, y0: y, x1: x + cut, y1: y + cut });
        }
    }
    out
}

fn snap_up(v: i64, g: i64) -> i64 {
    if g <= 1 { v } else { (v + g - 1).div_euclid(g) * g }
}

fn snap_down(v: i64, g: i64) -> i64 {
    if g <= 1 { v } else { v.div_euclid(g) * g }
}

fn grow(r: IRect, d: i64) -> IRect {
    IRect { x0: r.x0 - d, y0: r.y0 - d, x1: r.x1 + d, y1: r.y1 + d }
}

/// Grow `r` symmetrically (on grid) until it is at least `w` x `h`.
fn grow_to(mut r: IRect, w: i64, h: i64, g: i64) -> IRect {
    let split = |d: i64| {
        let lo = snap_down(d / 2, g);
        (lo, d - lo)
    };
    if r.width() < w {
        let (lo, hi) = split(w - r.width());
        r.x0 -= lo;
        r.x1 += hi;
    }
    if r.height() < h {
        let (lo, hi) = split(h - r.height());
        r.y0 -= lo;
        r.y1 += hi;
    }
    r
}

/// Grow `r` (on grid, along its longer side) to at least `area` dbu².
fn grow_to_area(r: IRect, area: i64, g: i64) -> IRect {
    if area <= 0 || r.width() <= 0 || r.height() <= 0 || r.width() * r.height() >= area {
        return r;
    }
    if r.width() >= r.height() {
        grow_to(r, snap_up((area + r.height() - 1) / r.height(), 2 * g), 0, g)
    } else {
        grow_to(r, 0, snap_up((area + r.width() - 1) / r.width(), 2 * g), g)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drc::run_tech_deck;
    use crate::hier::NoCells;

    fn on(items: &[(usize, Geometry)], tech: &Tech, name: &str) -> Vec<IRect> {
        let l = tech.layer_index(name).unwrap();
        items.iter().filter(|(k, _)| *k == l).map(|(_, g)| g.bbox()).collect()
    }

    fn assert_clean(tech: &Tech, items: &[(usize, Geometry)], what: &str) {
        let mut lay = Layout::default();
        for (l, g) in items {
            lay.add(*l, g.clone());
        }
        let v = run_tech_deck(&lay, &NoCells, tech).unwrap().violations;
        assert!(v.is_empty(), "{what}: {:?}", v.iter().map(|v| (&v.rule, v.marker)).collect::<Vec<_>>());
    }

    fn assert_cuts_ok(cuts: &[IRect], size: i64, spacing: i64, what: &str) {
        for (i, a) in cuts.iter().enumerate() {
            assert_eq!((a.width(), a.height()), (size, size), "{what}");
            for b in &cuts[i + 1..] {
                assert!(!a.intersects(b) && a.spacing(b) >= spacing, "{what}: {a:?} {b:?}");
            }
        }
    }

    fn ring(area: IRect, well: bool, width: i64) -> GuardRing {
        GuardRing { area, well, width, gap: 0, contacts: true, metal: true }
    }

    #[test]
    fn ring_structure_and_symmetry() {
        let tech = Tech::generic();
        let area = IRect::new(0, 0, 5000, 3000);
        let items = guard_ring(&tech, &ring(area, true, 0)).unwrap();
        let active = on(&items, &tech, "active");
        assert_eq!((active.len(), on(&items, &tech, "metal1").len()), (4, 4));
        assert_eq!(
            (on(&items, &tech, "nplus").len(), on(&items, &tech, "pplus").len(), on(&items, &tech, "nwell").len()),
            (4, 0, 1)
        );
        let bb = active.iter().copied().reduce(|a, b| a.union(&b)).unwrap();
        assert_eq!((bb.x0 + bb.x1, bb.y0 + bb.y1), (area.x0 + area.x1, area.y0 + area.y1));
        assert_eq!(active[0].height(), 420);
        assert_eq!(area.x0 - bb.x0, 360 + 420);
        let cuts = on(&items, &tech, "contact");
        let (cx, cy) = (area.x0 + area.x1, area.y0 + area.y1);
        let count = |f: &dyn Fn(&IRect) -> bool| cuts.iter().filter(|c| f(c)).count();
        assert_eq!(count(&|c| c.y1 <= area.y0), count(&|c| c.y0 >= area.y1));
        assert_eq!(
            count(&|c| c.x1 <= area.x0 && c.y0 >= area.y0 && c.y1 <= area.y1),
            count(&|c| c.x0 >= area.x1 && c.y0 >= area.y0 && c.y1 <= area.y1)
        );
        for c in &cuts {
            let m = IRect::new(cx - c.x1, cy - c.y1, cx - c.x0, cy - c.y0);
            assert!(cuts.iter().any(|o| (o.x0 - m.x0).abs() <= tech.grid && (o.y0 - m.y0).abs() <= tech.grid), "{c:?}");
        }
        let substrate = guard_ring(&tech, &ring(area, false, 0)).unwrap();
        assert_eq!((on(&substrate, &tech, "pplus").len(), on(&substrate, &tech, "nwell").len()), (4, 0));
        let bare = guard_ring(&tech, &GuardRing { contacts: false, metal: false, ..ring(area, false, 0) }).unwrap();
        assert!(on(&bare, &tech, "contact").is_empty() && on(&bare, &tech, "metal1").is_empty());
    }

    #[test]
    fn rings_on_grid_without_cut_conflicts_and_drc_clean() {
        let tech = Tech::generic();
        let con = tech.via("contact").unwrap();
        let active = tech.layer_index("active").unwrap();
        for well in [false, true] {
            for area in [IRect::new(0, 0, 0, 0), IRect::new(-1234, 55, 3333, 1007), IRect::new(0, 0, 20000, 8000)] {
                for width in [0, 1000] {
                    let what = format!("well={well} {area:?} w={width}");
                    let items = guard_ring(&tech, &ring(area, well, width)).unwrap();
                    for (_, g) in &items {
                        let b = g.bbox();
                        assert!([b.x0, b.y0, b.x1, b.y1].iter().all(|v| v % tech.grid == 0), "{what}: off grid {b:?}");
                    }
                    let cuts = on(&items, &tech, "contact");
                    assert!(cuts.len() >= 4, "{what}");
                    assert_cuts_ok(&cuts, con.cut_size, con.cut_spacing, &what);
                    let diff = Region::from_rects(items.iter().filter(|(l, _)| *l == active).map(|(_, g)| g.bbox()));
                    let grown = Region::from_rects(cuts.iter().map(|c| grow(*c, con.enc_bottom)));
                    assert!(grown.boolean(&diff, BoolOp::Not).is_empty(), "{what}: contacts not enclosed");
                    assert_clean(&tech, &items, &what);
                }
            }
        }
    }

    #[test]
    fn ring_errors() {
        let tech = Tech::generic();
        assert!(guard_ring(&tech, &ring(IRect::new(0, 0, 1000, 1000), false, 300)).is_err());
        let mut t = tech.clone();
        t.roles.diff_contact = "none".into();
        assert!(guard_ring(&t, &ring(IRect::new(0, 0, 1000, 1000), false, 0)).is_err());
    }

    #[test]
    fn via_array_counts_and_drc() {
        let tech = Tech::generic();
        let va = |area, via: &str| via_array(&tech, &ViaArray { area, via: via.into() });
        for (area, n) in [(IRect::new(0, 0, 1000, 1000), 4), (IRect::new(0, 0, 5000, 380), 9), (IRect::new(3, 7, 393, 397), 1)] {
            let items = va(area, "via1").unwrap();
            let cuts = on(&items, &tech, "via1");
            assert_eq!(cuts.len(), n, "{area:?}");
            assert_cuts_ok(&cuts, 260, 260, "via1");
            for c in &cuts {
                assert!(c.x0 - area.x0 >= 60 && area.x1 - c.x1 >= 60 && c.y0 - area.y0 >= 60 && area.y1 - c.y1 >= 60);
                assert_eq!((c.x0 % tech.grid, c.y0 % tech.grid), (0, 0));
            }
            assert_eq!((on(&items, &tech, "metal1").len(), on(&items, &tech, "metal2").len()), (1, 1));
            assert_clean(&tech, &items, &format!("via1 {area:?}"));
        }
        let items = va(IRect::new(0, 0, 2000, 500), "contact").unwrap();
        assert_eq!(on(&items, &tech, "contact").len(), 4);
        assert_clean(&tech, &items, "contact");
        assert!(va(IRect::new(0, 0, 300, 2000), "via1").is_err());
        assert!(va(IRect::new(0, 0, 2000, 2000), "nope").is_err());
    }

    #[test]
    fn overlaps_get_via_arrays() {
        let tech = Tech::generic();
        let (m1, m2) = (tech.layer_index("metal1").unwrap(), tech.layer_index("metal2").unwrap());
        let mut lay = Layout::default();
        lay.add(m1, Geometry::Rect(IRect::new(0, 0, 3000, 1000)));
        lay.add(m2, Geometry::Rect(IRect::new(1000, -2000, 2000, 3000)));
        let items = via_arrays_at_overlaps(&lay, &NoCells, &tech, "via1", None).unwrap();
        let overlap = IRect::new(1000, 0, 2000, 1000);
        let cuts = on(&items, &tech, "via1");
        assert_eq!(cuts.len(), 4);
        assert!(items.iter().all(|(_, g)| {
            let b = g.bbox();
            b.x0 >= overlap.x0 && b.y0 >= overlap.y0 && b.x1 <= overlap.x1 && b.y1 <= overlap.y1
        }));
        for (l, g) in items {
            lay.add(l, g);
        }
        assert!(run_tech_deck(&lay, &NoCells, &tech).unwrap().violations.is_empty());
        let none = via_arrays_at_overlaps(&lay, &NoCells, &tech, "via1", Some(IRect::new(5000, 5000, 6000, 6000))).unwrap();
        assert!(none.is_empty());
        assert!(via_arrays_at_overlaps(&lay, &NoCells, &tech, "nope", None).is_err());
    }

    #[test]
    fn vias_between_layers() {
        let tech = Tech::generic();
        let (m1, m2, m3) =
            (tech.layer_index("metal1").unwrap(), tech.layer_index("metal2").unwrap(), tech.layer_index("metal3").unwrap());
        assert_eq!(vias_between(&tech, m1, m2).iter().map(|v| v.name.as_str()).collect::<Vec<_>>(), ["via1"]);
        assert_eq!(vias_between(&tech, m2, m1).len(), 1);
        assert!(vias_between(&tech, m1, m3).is_empty());
    }
}
