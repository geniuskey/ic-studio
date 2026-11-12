//! Parametric cells and via generators. PCell variants are stored as ordinary cells (in the
//! `pcells` library) carrying their parameters, so hierarchy, GDS and DRC need no special cases.

use serde::{Deserialize, Serialize};

use crate::geom::IRect;
use crate::layout::{Geometry, Layout};
use crate::tech::{Tech, ViaDef};

pub const PCELL_LIB: &str = "pcells";

/// Device rules not expressible as single-layer width / spacing (dbu).
const GATE_CONTACT_SPACE: i64 = 160;
const POLY_ENDCAP: i64 = 220;
const IMPLANT_ENC: i64 = 180;
const NWELL_ENC: i64 = 430;
const CAP_INSET: i64 = 100;
const CAP_TAB_LEN: i64 = 500;
const CAP_TAB_W: i64 = 400;
const LABEL_SIZE: i64 = 200;

/// PCell parameters. Lengths are in dbu.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PCell {
    Mos { pmos: bool, w: i64, l: i64, fingers: u32 },
    PolyRes { w: i64, l: i64 },
    MomCap { w: i64, h: i64 },
}

impl PCell {
    /// One default instance of every kind, for the placement palette.
    pub fn defaults() -> Vec<PCell> {
        vec![
            PCell::Mos { pmos: false, w: 1000, l: 180, fingers: 1 },
            PCell::Mos { pmos: true, w: 2000, l: 180, fingers: 1 },
            PCell::PolyRes { w: 400, l: 5000 },
            PCell::MomCap { w: 5000, h: 5000 },
        ]
    }

    /// Short kind name, e.g. "nmos", "pmos", "poly_res", "mom_cap".
    pub fn kind_name(&self) -> &'static str {
        match self {
            PCell::Mos { pmos: false, .. } => "nmos",
            PCell::Mos { pmos: true, .. } => "pmos",
            PCell::PolyRes { .. } => "poly_res",
            PCell::MomCap { .. } => "mom_cap",
        }
    }

    /// Deterministic, valid cell name for this parameter set, e.g. "nmos_w1000_l180_nf2".
    pub fn variant_name(&self) -> String {
        let k = self.kind_name();
        match self {
            PCell::Mos { w, l, fingers, .. } => format!("{k}_w{w}_l{l}_nf{fingers}"),
            PCell::PolyRes { w, l } => format!("{k}_w{w}_l{l}"),
            PCell::MomCap { w, h } => format!("{k}_w{w}_h{h}"),
        }
        .replace('-', "m")
    }

    /// Human readable parameter summary, e.g. "nmos W=1um L=0.18um nf=2".
    pub fn summary(&self, dbu_per_um: i64) -> String {
        let um = |v: i64| format!("{}um", v as f64 / dbu_per_um.max(1) as f64);
        let k = self.kind_name();
        match self {
            PCell::Mos { w, l, fingers, .. } => format!("{k} W={} L={} nf={fingers}", um(*w), um(*l)),
            PCell::PolyRes { w, l } => format!("{k} W={} L={}", um(*w), um(*l)),
            PCell::MomCap { w, h } => format!("{k} W={} H={}", um(*w), um(*h)),
        }
    }

    /// Generate the layout (origin-relative), checking parameters against the technology.
    pub fn generate(&self, tech: &Tech) -> Result<Layout, String> {
        match *self {
            PCell::Mos { pmos, w, l, fingers } => gen_mos(tech, pmos, w, l, fingers),
            PCell::PolyRes { w, l } => gen_poly_res(tech, w, l),
            PCell::MomCap { w, h } => gen_mom_cap(tech, w, h),
        }
    }
}

/// Via stack filling `area`: bottom and top plates covering `area` (grown if needed to meet
/// enclosure) plus the largest centred array of cuts that fits. Returns (layer, geometry).
pub fn via_array(tech: &Tech, via: &ViaDef, area: IRect) -> Result<Vec<(usize, Geometry)>, String> {
    let (b, c, t) = via_layers(tech, via)?;
    let g = tech.grid;
    let enc = via.enc_bottom.max(via.enc_top);
    let need = (via.cut_size + 2 * enc).max(min_width(tech, b)).max(min_width(tech, t));
    let plate = grow_to(area, need, need, g);
    let mut out = vec![(b, Geometry::Rect(plate))];
    out.extend(cut_grid(grow(plate, -enc), via.cut_size, via.cut_spacing, g).into_iter().map(|r| (c, Geometry::Rect(r))));
    out.push((t, Geometry::Rect(plate)));
    Ok(out)
}

/// Minimal single-cut via centred at `at`.
pub fn via_at(tech: &Tech, via: &ViaDef, at: (i64, i64)) -> Result<Vec<(usize, Geometry)>, String> {
    let (b, c, t) = via_layers(tech, via)?;
    let g = tech.grid;
    let s = via.cut_size;
    let (x0, y0) = (snap(at.0 - s / 2, g), snap(at.1 - s / 2, g));
    let cut = IRect::new(x0, y0, x0 + s, y0 + s);
    let plate = |layer: usize, enc: i64| {
        let mw = min_width(tech, layer);
        Geometry::Rect(grow_to(grow(cut, enc), mw, mw, g))
    };
    Ok(vec![(b, plate(b, via.enc_bottom)), (c, Geometry::Rect(cut)), (t, plate(t, via.enc_top))])
}

fn via_layers(tech: &Tech, via: &ViaDef) -> Result<(usize, usize, usize), String> {
    if via.cut_size <= 0 || via.cut_spacing < 0 {
        return Err(format!("via '{}' has an invalid cut size or spacing", via.name));
    }
    Ok((layer(tech, &via.bottom)?, layer(tech, &via.cut)?, layer(tech, &via.top)?))
}

fn layer(tech: &Tech, name: &str) -> Result<usize, String> {
    tech.layer_index(name).ok_or_else(|| format!("technology '{}' has no layer '{name}'", tech.name))
}

fn via_def<'a>(tech: &'a Tech, name: &str) -> Result<&'a ViaDef, String> {
    tech.via(name).ok_or_else(|| format!("technology '{}' has no via '{name}'", tech.name))
}

fn min_width(tech: &Tech, layer: usize) -> i64 {
    tech.layer(layer).map_or(0, |l| l.min_width)
}

fn min_spacing(tech: &Tech, layer: usize) -> i64 {
    tech.layer(layer).map_or(0, |l| l.min_spacing)
}

fn snap(v: i64, g: i64) -> i64 {
    if g <= 1 { v } else { (v + g.div_euclid(2)).div_euclid(g) * g }
}

fn snap_up(v: i64, g: i64) -> i64 {
    if g <= 1 { v } else { (v + g - 1).div_euclid(g) * g }
}

fn grow(r: IRect, d: i64) -> IRect {
    IRect { x0: r.x0 - d, y0: r.y0 - d, x1: r.x1 + d, y1: r.y1 + d }
}

/// Grow `r` symmetrically (on grid) until it is at least `w` x `h`.
fn grow_to(mut r: IRect, w: i64, h: i64, g: i64) -> IRect {
    let split = |d: i64| {
        let lo = if g <= 1 { d / 2 } else { (d / 2).div_euclid(g) * g };
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

/// Largest centred grid of `cut` x `cut` squares spaced `sp` apart inside `inner`.
fn cut_grid(inner: IRect, cut: i64, sp: i64, g: i64) -> Vec<IRect> {
    let axis = |len: i64| -> (i64, i64) {
        if len < cut {
            return (0, 0);
        }
        let n = (len - cut) / (cut + sp) + 1;
        let off = (len - (n * cut + (n - 1) * sp)) / 2;
        (n, if g <= 1 { off } else { off.div_euclid(g) * g })
    };
    let ((nx, ox), (ny, oy)) = (axis(inner.width()), axis(inner.height()));
    let mut out = Vec::with_capacity((nx * ny) as usize);
    for j in 0..ny {
        for i in 0..nx {
            let x = inner.x0 + ox + i * (cut + sp);
            let y = inner.y0 + oy + j * (cut + sp);
            out.push(IRect::new(x, y, x + cut, y + cut));
        }
    }
    out
}

fn add_all(lay: &mut Layout, items: Vec<(usize, Geometry)>, skip: Option<usize>) {
    for (l, g) in items {
        if Some(l) != skip {
            lay.add(l, g);
        }
    }
}

fn label(lay: &mut Layout, tech: &Tech, text: &str, (cx, cy): (i64, i64)) {
    if let Some(t) = tech.layer_index("text") {
        let w = text.len() as i64 * LABEL_SIZE * 6 / 10;
        lay.add(t, Geometry::Label { text: text.into(), at: (cx - w / 2, cy - LABEL_SIZE / 2), size: LABEL_SIZE });
    }
}

fn centre(r: IRect) -> (i64, i64) {
    ((r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2)
}

fn gen_mos(tech: &Tech, pmos: bool, w: i64, l: i64, fingers: u32) -> Result<Layout, String> {
    let g = tech.grid;
    let (w, l) = (snap(w, g), snap(l, g));
    let active = layer(tech, "active")?;
    let poly = layer(tech, "poly")?;
    let m1 = layer(tech, "metal1")?;
    let imp = layer(tech, if pmos { "pplus" } else { "nplus" })?;
    let con = via_def(tech, "contact")?;
    let pcon = via_def(tech, "polycon")?;
    let con_enc = con.enc_bottom.max(con.enc_top);
    if fingers == 0 {
        return Err("a MOS device needs at least one finger".into());
    }
    if l < min_width(tech, poly) {
        return Err(format!("gate length {l} is below the poly minimum width {}", min_width(tech, poly)));
    }
    let w_min = min_width(tech, active).max(con.cut_size + 2 * con_enc);
    if w < w_min {
        return Err(format!("width {w} is below the minimum {w_min} (active width / one contact)"));
    }

    let c = con.cut_size;
    let gcs = snap_up(GATE_CONTACT_SPACE, g);
    let sd_in = c + 2 * gcs;
    let sd_end = con.enc_bottom + c + gcs;
    let pitch = l + sd_in;
    let n = fingers as i64;
    let total = 2 * sd_end + n * l + (n - 1) * sd_in;
    let mut lay = Layout::default();
    let act = IRect::new(0, 0, total, w);
    lay.add(active, Geometry::Rect(act));

    for k in 0..=n {
        let cx = sd_end - gcs - c / 2 + k * pitch;
        let half = c / 2 + con_enc;
        let items = via_array(tech, con, IRect::new(cx - half, 0, cx + half, w))?;
        add_all(&mut lay, items, Some(active));
        label(&mut lay, tech, if k % 2 == 0 { "S" } else { "D" }, (cx, w / 2));
    }

    let ext_top = snap_up(POLY_ENDCAP.max(min_spacing(tech, m1)), g);
    let endcap = snap_up(POLY_ENDCAP, g);
    let head_y = w + ext_top;
    for i in 0..n {
        let x0 = sd_end + i * pitch;
        lay.add(poly, Geometry::Rect(IRect::new(x0, -endcap, x0 + l, head_y)));
    }
    let head_h = pcon.cut_size + 2 * pcon.enc_bottom.max(pcon.enc_top);
    let head = IRect::new(sd_end, head_y, sd_end + (n - 1) * pitch + l, head_y + head_h);
    let items = via_array(tech, pcon, head)?;
    let pad = items.last().map(|(_, g)| g.bbox()).unwrap_or(head);
    add_all(&mut lay, items, None);
    label(&mut lay, tech, "G", centre(pad));

    let imp_r = grow(act, snap_up(IMPLANT_ENC, g));
    let imp_min = min_width(tech, imp);
    lay.add(imp, Geometry::Rect(grow_to(imp_r, imp_min, imp_min, g)));
    if pmos {
        let nwell = layer(tech, "nwell")?;
        let nw_min = min_width(tech, nwell);
        lay.add(nwell, Geometry::Rect(grow_to(grow(act, snap_up(NWELL_ENC, g)), nw_min, nw_min, g)));
    }
    Ok(lay)
}

fn gen_poly_res(tech: &Tech, w: i64, l: i64) -> Result<Layout, String> {
    let g = tech.grid;
    let (w, l) = (snap(w, g), snap(l, g));
    let poly = layer(tech, "poly")?;
    let m1 = layer(tech, "metal1")?;
    let pcon = via_def(tech, "polycon")?;
    if w < min_width(tech, poly) {
        return Err(format!("resistor width {w} is below the poly minimum width {}", min_width(tech, poly)));
    }
    let l_min = min_spacing(tech, poly).max(min_spacing(tech, m1)).max(min_width(tech, poly));
    if l < l_min {
        return Err(format!("resistor length {l} is below the minimum {l_min}"));
    }
    let head_h = pcon.cut_size + 2 * pcon.enc_bottom.max(pcon.enc_top);
    let mut lay = Layout::default();
    lay.add(poly, Geometry::Rect(IRect::new(0, 0, w, l)));
    for (name, area) in [("A", IRect::new(0, -head_h, w, 0)), ("B", IRect::new(0, l, w, l + head_h))] {
        let items = via_array(tech, pcon, area)?;
        let pad = items.last().map(|(_, g)| g.bbox()).unwrap_or(area);
        add_all(&mut lay, items, None);
        label(&mut lay, tech, name, centre(pad));
    }
    Ok(lay)
}

fn gen_mom_cap(tech: &Tech, w: i64, h: i64) -> Result<Layout, String> {
    let g = tech.grid;
    let (w, h) = (snap(w, g), snap(h, g));
    let m1 = layer(tech, "metal1")?;
    let m2 = layer(tech, "metal2")?;
    let inset = snap_up(CAP_INSET, g);
    let tab_w = snap_up(CAP_TAB_W.max(min_width(tech, m1)).max(min_width(tech, m2)), g);
    let w_min = 2 * inset + min_width(tech, m2).max(min_width(tech, m1));
    let h_min = 2 * inset + tab_w;
    if w < w_min || h < h_min {
        return Err(format!("capacitor must be at least {w_min} x {h_min}"));
    }
    let tab_len = snap_up(CAP_TAB_LEN, g);
    let cy = (h / 2).div_euclid(g.max(1)) * g.max(1);
    let (ty0, ty1) = (cy - tab_w / 2, cy - tab_w / 2 + tab_w);
    let mut lay = Layout::default();
    lay.add(m1, Geometry::Rect(IRect::new(0, 0, w, h)));
    lay.add(m2, Geometry::Rect(IRect::new(inset, inset, w - inset, h - inset)));
    let bot = IRect::new(-tab_len, ty0, 0, ty1);
    let top = IRect::new(w - inset, ty0, w + tab_len, ty1);
    lay.add(m1, Geometry::Rect(bot));
    lay.add(m2, Geometry::Rect(top));
    label(&mut lay, tech, "BOT", centre(bot));
    label(&mut lay, tech, "TOP", (top.x1 - tab_len / 2, cy));
    Ok(lay)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hier::NoCells;
    use crate::layout::run_basic_drc;
    use crate::project::valid_name;

    fn rects_on(items: &[(usize, Geometry)], layer: usize) -> Vec<IRect> {
        items.iter().filter(|(l, _)| *l == layer).map(|(_, g)| g.bbox()).collect()
    }

    #[test]
    fn via_array_counts() {
        let tech = Tech::generic();
        let v1 = tech.via("via1").unwrap();
        let cut = tech.layer_index("via1").unwrap();
        let m1 = tech.layer_index("metal1").unwrap();
        let cases = [
            (IRect::new(0, 0, 1000, 1000), 4),
            (IRect::new(0, 0, 380, 380), 1),
            (IRect::new(0, 0, 100, 100), 1),
            (IRect::new(0, 0, 5000, 380), 9),
        ];
        for (area, n) in cases {
            let items = via_array(&tech, v1, area).unwrap();
            let cuts = rects_on(&items, cut);
            assert_eq!(cuts.len(), n, "{area:?}");
            let plate = rects_on(&items, m1)[0];
            assert!(plate.width() >= 380 && plate.height() >= 380);
            for c in &cuts {
                assert_eq!((c.width(), c.height()), (260, 260));
                assert!(c.x0 - plate.x0 >= 60 && plate.x1 - c.x1 >= 60 && c.y0 - plate.y0 >= 60 && plate.y1 - c.y1 >= 60);
                assert_eq!((c.x0 % 10, c.y0 % 10), (0, 0));
            }
        }
        let con = tech.via("contact").unwrap();
        let items = via_array(&tech, con, IRect::new(0, 0, 2000, 500)).unwrap();
        assert_eq!(rects_on(&items, tech.layer_index("contact").unwrap()).len(), 4);
        let bad = ViaDef { bottom: "nope".into(), ..con.clone() };
        assert!(via_array(&tech, &bad, IRect::new(0, 0, 500, 500)).is_err());
    }

    #[test]
    fn via_at_plates() {
        let tech = Tech::generic();
        let items = via_at(&tech, tech.via("contact").unwrap(), (0, 0)).unwrap();
        assert_eq!(items[0].1.bbox(), IRect::new(-210, -210, 210, 210));
        assert_eq!(items[1].1.bbox(), IRect::new(-110, -110, 110, 110));
        assert_eq!(items[2].1.bbox(), IRect::new(-170, -170, 170, 170));
        let bare = ViaDef { enc_bottom: 0, enc_top: 0, ..tech.via("via2").unwrap().clone() };
        let items = via_at(&tech, &bare, (1000, 1000)).unwrap();
        assert_eq!((items[0].1.bbox().width(), items[2].1.bbox().width()), (280, 280));
        assert_eq!(items[1].1.bbox(), IRect::new(870, 870, 1130, 1130));
    }

    #[test]
    fn generated_variants_are_drc_clean() {
        let tech = Tech::generic();
        let mut all = PCell::defaults();
        for pmos in [false, true] {
            for w in [420, 500, 1000, 5000] {
                for l in [180, 500] {
                    for fingers in [1, 2, 5] {
                        all.push(PCell::Mos { pmos, w, l, fingers });
                    }
                }
            }
        }
        for (w, l) in [(180, 240), (400, 5000), (1000, 2000), (2000, 300)] {
            all.push(PCell::PolyRes { w, l });
        }
        for (w, h) in [(600, 600), (5000, 5000), (20000, 3000)] {
            all.push(PCell::MomCap { w, h });
        }
        for p in &all {
            let lay = p.generate(&tech).unwrap_or_else(|e| panic!("{p:?}: {e}"));
            assert!(!lay.is_empty());
            let v = run_basic_drc(&lay, &NoCells, &tech);
            assert!(v.is_empty(), "{p:?}: {:?}", v.iter().map(|v| &v.rule).collect::<Vec<_>>());
            for s in &lay.shapes {
                let b = s.bbox();
                if matches!(s.geom, Geometry::Rect(_)) {
                    assert!([b.x0, b.y0, b.x1, b.y1].iter().all(|v| v % tech.grid == 0), "{p:?}: off grid {b:?}");
                }
            }
            assert!(valid_name(&p.variant_name()));
        }
    }

    #[test]
    fn mos_structure() {
        let tech = Tech::generic();
        let lay = PCell::Mos { pmos: true, w: 2000, l: 180, fingers: 3 }.generate(&tech).unwrap();
        let on = |n: &str| lay.shapes.iter().filter(|s| s.layer == tech.layer_index(n).unwrap()).count();
        assert_eq!(on("poly"), 4);
        assert_eq!(on("metal1"), 5);
        assert_eq!(on("nwell"), 1);
        assert_eq!(on("pplus"), 1);
        assert_eq!(on("nplus"), 0);
        assert_eq!(on("contact"), 4 * 4 + 3);
    }

    #[test]
    fn invalid_parameters() {
        let tech = Tech::generic();
        for p in [
            PCell::Mos { pmos: false, w: 1000, l: 100, fingers: 1 },
            PCell::Mos { pmos: false, w: 300, l: 180, fingers: 1 },
            PCell::Mos { pmos: false, w: 1000, l: 180, fingers: 0 },
            PCell::PolyRes { w: 100, l: 1000 },
            PCell::PolyRes { w: 400, l: 100 },
            PCell::MomCap { w: 300, h: 5000 },
            PCell::MomCap { w: 5000, h: 300 },
        ] {
            assert!(p.generate(&tech).is_err(), "{p:?}");
        }
        let mut t = tech.clone();
        t.vias.clear();
        assert!(PCell::defaults()[0].generate(&t).is_err());
    }

    #[test]
    fn names() {
        assert_eq!(PCell::Mos { pmos: false, w: 1000, l: 180, fingers: 2 }.variant_name(), "nmos_w1000_l180_nf2");
        assert_eq!(PCell::PolyRes { w: 400, l: 5000 }.variant_name(), "poly_res_w400_l5000");
        assert_eq!(PCell::MomCap { w: 5000, h: 5000 }.variant_name(), "mom_cap_w5000_h5000");
        assert_eq!(PCell::Mos { pmos: false, w: 1000, l: 180, fingers: 2 }.summary(1000), "nmos W=1um L=0.18um nf=2");
        let kinds: Vec<_> = PCell::defaults().iter().map(|p| p.kind_name()).collect();
        assert_eq!(kinds, ["nmos", "pmos", "poly_res", "mom_cap"]);
    }
}
