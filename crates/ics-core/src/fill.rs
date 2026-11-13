//! Dummy metal fill: grid-aligned squares placed in the free space of a layer, optionally only
//! up to a target density per window (like Virtuoso / Calibre fill).

use std::ops::Range;

use crate::drc::{self, Rule};
use crate::geom::IRect;
use crate::hier::{LayoutSource, flatten};
use crate::layout::{Geometry, Layout};
use crate::region::{BoolOp, Region};
use crate::tech::Tech;

const MAX_DEPTH: u32 = 64;
/// Fill positions considered at most (the grid of candidate squares over the area).
const MAX_POSITIONS: usize = 64 << 20;

/// What to fill and how. Lengths in dbu.
#[derive(Debug, Clone, PartialEq)]
pub struct FillParams {
    /// Technology layer index to fill.
    pub layer: usize,
    /// Side of the fill squares; a multiple of the technology grid.
    pub size: i64,
    /// Gap between neighbouring fill squares; a multiple of the grid, at least the layer's min spacing.
    pub spacing: i64,
    /// Clearance from shapes of the `avoid` layers (raised to the min spacing for `layer` itself).
    pub keepout: i64,
    /// Layers whose shapes block fill (normally includes `layer`).
    pub avoid: Vec<usize>,
    /// Region to fill; `None` = bounding box of the flattened cell.
    pub area: Option<IRect>,
    /// Target density (0..=1) per window; `None` fills every eligible position.
    pub target: Option<f64>,
    /// Density window side used with `target`.
    pub window: i64,
    /// Window step used with `target`.
    pub step: i64,
}

impl FillParams {
    /// Defaults derived from the layer rules: squares of 4x the min width (at least 1 µm and the
    /// min area), spacing 2x the min spacing (at least 0.5 µm), keepout 2x the min spacing (at
    /// least 1 µm), all snapped up to the grid. With a deck density rule on the layer its window /
    /// step are used and the target is 5 % above its min (at most halfway to its max); otherwise
    /// 100 µm windows and no target.
    pub fn for_layer(tech: &Tech, layer: usize) -> FillParams {
        let g = tech.grid.max(1);
        let um = tech.dbu_per_um.max(1);
        let (w, s, a) = tech.layer(layer).map_or((0, 0, 0), |l| (l.min_width, l.min_spacing, l.min_area));
        let area_side = (a.max(0) as f64).sqrt().ceil() as i64;
        let rule = density_rule(tech, layer);
        let (window, step) = rule.map_or((100 * um, 50 * um), |r| (r.window, r.step));
        FillParams {
            layer,
            size: snap_up((4 * w).max(um).max(area_side), g),
            spacing: snap_up((2 * s).max(um / 2), g),
            keepout: snap_up((2 * s).max(um), g),
            avoid: vec![layer],
            area: None,
            target: rule.map(|r| (r.min + 0.05).min((r.min + r.max) / 2.0)),
            window,
            step,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FillReport {
    /// Fill squares in top-cell coordinates, row by row from the bottom left.
    pub rects: Vec<IRect>,
    /// The filled area.
    pub area: IRect,
    /// Density of the layer over `area` before and after fill (0..=1).
    pub before: f64,
    pub after: f64,
    /// Eligible positions (squares that fit the free space).
    pub candidates: usize,
    /// Min / max density (0..=1) of the deck's density rule on the layer, if any.
    pub limits: Option<(f64, f64)>,
    /// Deck windows over `area` still below the min / above the max after fill (0 without a rule).
    pub windows_below: usize,
    pub windows_above: usize,
}

/// A deck density rule: limits as fractions, window and step in dbu.
#[derive(Debug, Clone, Copy)]
pub struct DensityRule {
    pub min: f64,
    pub max: f64,
    pub window: i64,
    pub step: i64,
}

/// The first density rule on `layer` of the technology deck, limits as fractions.
pub fn density_rule(tech: &Tech, layer: usize) -> Option<DensityRule> {
    let deck = drc::parse_any(&tech.deck_text(), tech).ok()?.deck;
    deck.rules.iter().find_map(|r| match r.rule {
        Rule::Density { layer: l, min, max, window, step } if l == layer => {
            Some(DensityRule { min: min / 100.0, max: max / 100.0, window, step })
        }
        _ => None,
    })
}

fn snap_up(v: i64, g: i64) -> i64 {
    -(-v).div_euclid(g) * g
}

fn grow(r: &IRect, d: i64) -> IRect {
    IRect { x0: r.x0 - d, y0: r.y0 - d, x1: r.x1 + d, y1: r.y1 + d }
}

fn area_of(r: &IRect) -> i128 {
    r.width() as i128 * r.height() as i128
}

/// Window spans over `lo..hi` as in the DRC density check: `window` wide (clipped to the
/// extent), stepped by `step`, the last one aligned to `hi`.
fn spans(lo: i64, hi: i64, window: i64, step: i64) -> Vec<(i64, i64)> {
    let w = window.min(hi - lo);
    let mut v = Vec::new();
    if w <= 0 {
        return v;
    }
    let mut s = lo;
    loop {
        let s0 = s.min(hi - w);
        v.push((s0, s0 + w));
        if s0 + w >= hi {
            break;
        }
        s += step.max(1);
    }
    v
}

/// Density windows with their covered area.
struct Windows {
    xs: Vec<(i64, i64)>,
    ys: Vec<(i64, i64)>,
    cov: Vec<i128>,
}

impl Windows {
    fn new(extent: IRect, window: i64, step: i64) -> Windows {
        let xs = spans(extent.x0, extent.x1, window, step);
        let ys = spans(extent.y0, extent.y1, window, step);
        let cov = vec![0; xs.len() * ys.len()];
        Windows { xs, ys, cov }
    }

    fn rect(&self, k: usize) -> IRect {
        let (x, y) = (self.xs[k % self.xs.len()], self.ys[k / self.xs.len()]);
        IRect::new(x.0, y.0, x.1, y.1)
    }

    fn density(&self, k: usize) -> f64 {
        self.cov[k] as f64 / area_of(&self.rect(k)) as f64
    }

    /// Windows overlapping `r` with the overlap area.
    fn hits(&self, r: &IRect) -> Vec<(usize, i128)> {
        let range = |v: &[(i64, i64)], lo: i64, hi: i64| v.partition_point(|w| w.1 <= lo)..v.partition_point(|w| w.0 < hi);
        let (xr, yr) = (range(&self.xs, r.x0, r.x1), range(&self.ys, r.y0, r.y1));
        let mut out = Vec::with_capacity(xr.len() * yr.len());
        for j in yr {
            let (y0, y1) = self.ys[j];
            for i in xr.clone() {
                let (x0, x1) = self.xs[i];
                let ov = (r.x1.min(x1) - r.x0.max(x0)) as i128 * (r.y1.min(y1) - r.y0.max(y0)) as i128;
                out.push((j * self.xs.len() + i, ov));
            }
        }
        out
    }

    fn add(&mut self, r: &IRect) {
        for (k, ov) in self.hits(r) {
            self.cov[k] += ov;
        }
    }
}

/// Candidate square grid over the fill area.
struct Grid {
    ox: i64,
    oy: i64,
    pitch: i64,
    size: i64,
    nx: usize,
    ny: usize,
}

impl Grid {
    fn square(&self, k: usize) -> IRect {
        let (i, j) = ((k % self.nx) as i64, (k / self.nx) as i64);
        let (x, y) = (self.ox + i * self.pitch, self.oy + j * self.pitch);
        IRect::new(x, y, x + self.size, y + self.size)
    }

    /// Indices of squares overlapping `lo..hi` along an axis starting at `o`.
    fn overlapping(&self, lo: i64, hi: i64, o: i64, len: usize) -> Range<usize> {
        let a = ((lo - self.size - o).div_euclid(self.pitch) + 1).clamp(0, len as i64) as usize;
        let b = (hi - o + self.pitch - 1).div_euclid(self.pitch).clamp(0, len as i64) as usize;
        a.min(b)..b
    }

    /// Indices of squares whose centre lies in `lo..hi`.
    fn centred(&self, lo: i64, hi: i64, o: i64, len: usize) -> Range<usize> {
        let first = |v: i64| (v - self.size / 2 - o + self.pitch - 1).div_euclid(self.pitch).clamp(0, len as i64) as usize;
        let (a, b) = (first(lo), first(hi));
        a.min(b)..b
    }
}

/// Fill squares for layer `p.layer` of `top` (hierarchy flattened) in the free space of
/// `p.area`: on a grid snapped to the technology grid, each square at least `p.keepout` from
/// the shapes of `p.avoid` (at least the layer's min spacing from its own shapes). With a
/// target, squares are spread evenly per window until it reaches the target, never pushing a
/// window above the deck's max density. The squares are not added to the layout.
pub fn generate(top: &Layout, src: &dyn LayoutSource, tech: &Tech, p: &FillParams) -> Result<FillReport, String> {
    let n = tech.layers.len();
    let def = tech.layer(p.layer).ok_or_else(|| format!("fill layer #{} does not exist", p.layer))?;
    if let Some(bad) = p.avoid.iter().find(|&&l| l >= n) {
        return Err(format!("avoid layer #{bad} does not exist"));
    }
    let g = tech.grid.max(1);
    if p.size <= 0 {
        return Err("the fill size must be larger than 0".into());
    }
    if p.spacing <= 0 {
        return Err("the fill spacing must be larger than 0".into());
    }
    if p.size % g != 0 || p.spacing % g != 0 {
        return Err(format!("fill size {} and spacing {} must be multiples of the {g} dbu grid", p.size, p.spacing));
    }
    if p.size < def.min_width {
        return Err(format!("fill size {} is below the {} min width {}", p.size, def.name, def.min_width));
    }
    if (p.size as i128 * p.size as i128) < def.min_area as i128 {
        return Err(format!("fill squares of {} are below the {} min area {}", p.size, def.name, def.min_area));
    }
    if p.spacing < def.min_spacing {
        return Err(format!("fill spacing {} is below the {} min spacing {}", p.spacing, def.name, def.min_spacing));
    }
    if p.keepout < 0 {
        return Err("the keepout must not be negative".into());
    }
    if let Some(t) = p.target {
        if !(0.0..=1.0).contains(&t) {
            return Err(format!("target density {t} must be between 0 and 1"));
        }
        if p.window <= 0 || p.step <= 0 {
            return Err("the density window and step must be larger than 0".into());
        }
    }

    let mut existing = Vec::new();
    let mut blockers = Vec::new();
    let mut extent: Option<IRect> = None;
    let own_keepout = p.keepout.max(def.min_spacing);
    for f in flatten(top, src, MAX_DEPTH) {
        if f.layer >= n || matches!(f.geom, Geometry::Label { .. }) {
            continue;
        }
        let b = f.geom.bbox();
        extent = Some(extent.map_or(b, |e| e.union(&b)));
        let (own, avoided) = (f.layer == p.layer, p.avoid.contains(&f.layer));
        if !own && !avoided {
            continue;
        }
        let rects = match &f.geom {
            Geometry::Rect(r) => vec![*r],
            geom => Region::from_geometry(geom).map_or_else(|_| vec![b], |r| r.rects().to_vec()),
        };
        if avoided {
            let k = if own { own_keepout } else { p.keepout };
            blockers.extend(rects.iter().map(|r| grow(r, k)));
        }
        if own {
            existing.extend(rects);
        }
    }
    let area = p.area.or(extent).ok_or("nothing to fill: the cell is empty and no fill area is given")?;
    if area.width() <= 0 || area.height() <= 0 {
        return Err("the fill area is empty".into());
    }
    let area_region = Region::from_rects([area]);
    let existing = Region::from_rects(existing).boolean(&area_region, BoolOp::And);
    let blocked = Region::from_rects(blockers).boolean(&area_region, BoolOp::And);

    let pitch = p.size + p.spacing;
    let (ox, oy) = (snap_up(area.x0, g), snap_up(area.y0, g));
    let count = |o: i64, hi: i64| if o + p.size > hi { 0 } else { ((hi - p.size - o) / pitch + 1) as usize };
    let grid = Grid { ox, oy, pitch, size: p.size, nx: count(ox, area.x1), ny: count(oy, area.y1) };
    if grid.nx.saturating_mul(grid.ny) > MAX_POSITIONS {
        return Err(format!("too many fill positions ({} x {}): use larger squares or a smaller area", grid.nx, grid.ny));
    }
    let mut free = vec![true; grid.nx * grid.ny];
    for r in blocked.rects() {
        let cols = grid.overlapping(r.x0, r.x1, ox, grid.nx);
        for j in grid.overlapping(r.y0, r.y1, oy, grid.ny) {
            free[j * grid.nx + cols.start..j * grid.nx + cols.end].fill(false);
        }
    }
    let candidates = free.iter().filter(|&&f| f).count();
    let rule = density_rule(tech, p.layer);

    let chosen = match p.target {
        None => free,
        Some(t) => {
            let mut fill = TargetFill {
                grid: &grid,
                sets: [Windows::new(area, p.window, p.window), Windows::new(area, p.window, p.step)],
                chosen: vec![false; free.len()],
                free,
                limit: rule.map_or(1.0, |r| r.max),
            };
            for r in existing.rects() {
                fill.sets[0].add(r);
                fill.sets[1].add(r);
            }
            for which in 0..2 {
                for k in 0..fill.sets[which].cov.len() {
                    fill.top_up(which, k, t);
                }
            }
            fill.chosen
        }
    };
    let rects: Vec<IRect> = (0..chosen.len()).filter(|&k| chosen[k]).map(|k| grid.square(k)).collect();

    let total = area_of(&area) as f64;
    let filled: Vec<IRect> = if p.avoid.contains(&p.layer) {
        existing.rects().iter().chain(&rects).copied().collect()
    } else {
        Region::from_rects(existing.rects().iter().chain(&rects).copied()).rects().to_vec()
    };
    let filled_area: i128 = filled.iter().map(area_of).sum();
    let (mut windows_below, mut windows_above) = (0, 0);
    if let Some(r) = rule {
        let mut w = Windows::new(area, r.window, r.step);
        for q in &filled {
            w.add(q);
        }
        for k in 0..w.cov.len() {
            let d = w.density(k);
            windows_below += (d < r.min) as usize;
            windows_above += (d > r.max) as usize;
        }
    }
    Ok(FillReport {
        rects,
        area,
        before: existing.area() as f64 / total,
        after: filled_area as f64 / total,
        candidates,
        limits: rule.map(|r| (r.min, r.max)),
        windows_below,
        windows_above,
    })
}

/// Greedy target-density fill: first per tile (windows stepped by their size), then per
/// stepped window, each topped up with evenly spread squares.
struct TargetFill<'g> {
    grid: &'g Grid,
    /// Tiles and the stepped windows; the stepped ones also enforce `limit`.
    sets: [Windows; 2],
    free: Vec<bool>,
    chosen: Vec<bool>,
    limit: f64,
}

impl TargetFill<'_> {
    fn top_up(&mut self, which: usize, k: usize, target: f64) {
        let w = self.sets[which].rect(k);
        let g = self.grid;
        let cols = g.centred(w.x0, w.x1, g.ox, g.nx);
        let mut avail: Vec<usize> = g
            .centred(w.y0, w.y1, g.oy, g.ny)
            .flat_map(|j| cols.clone().map(move |i| j * g.nx + i))
            .filter(|&c| self.free[c] && !self.chosen[c])
            .collect();
        let goal = target * area_of(&w) as f64;
        let sq = (g.size as f64).powi(2);
        while !avail.is_empty() {
            let missing = goal - self.sets[which].cov[k] as f64;
            if missing <= 0.0 {
                break;
            }
            let need = ((missing / sq).ceil() as usize).clamp(1, avail.len());
            let len = avail.len();
            let picks: Vec<usize> = (0..need).map(|j| (2 * j + 1) * len / (2 * need)).collect();
            for &i in &picks {
                self.try_add(avail[i]);
            }
            let mut next = picks.into_iter().peekable();
            let mut idx = 0;
            avail.retain(|_| {
                let keep = next.peek() != Some(&idx);
                if !keep {
                    next.next();
                }
                idx += 1;
                keep
            });
        }
    }

    fn try_add(&mut self, c: usize) -> bool {
        let r = self.grid.square(c);
        let set = &self.sets[1];
        if set.hits(&r).into_iter().any(|(k, ov)| (set.cov[k] + ov) as f64 > self.limit * area_of(&set.rect(k)) as f64) {
            return false;
        }
        self.sets[0].add(&r);
        self.sets[1].add(&r);
        self.chosen[c] = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hier::NoCells;

    fn tech_with_density() -> Tech {
        let mut t = Tech::generic();
        t.drc_deck = Some(drc::default_deck_text(&t) + "density metal1 20 80 50 25\n");
        t
    }

    fn m1(t: &Tech) -> usize {
        t.layer_index("metal1").unwrap()
    }

    fn params(t: &Tech, area: IRect) -> FillParams {
        FillParams { size: 1000, spacing: 1000, keepout: 1000, area: Some(area), target: None, ..FillParams::for_layer(t, m1(t)) }
    }

    #[test]
    fn empty_cell_fills_area_fully() {
        let t = Tech::generic();
        let r = generate(&Layout::default(), &NoCells, &t, &params(&t, IRect::new(0, 0, 10_000, 10_000))).unwrap();
        assert_eq!(r.rects.len(), 25);
        assert_eq!(r.candidates, 25);
        assert_eq!(r.before, 0.0);
        assert!((r.after - 0.25).abs() < 1e-9);
        assert_eq!((r.windows_below, r.limits), (0, None));
        let r = generate(&Layout::default(), &NoCells, &t, &params(&t, IRect::new(0, 0, 8_999, 10_000))).unwrap();
        assert_eq!(r.rects.len(), 20, "the last column no longer fits");
    }

    #[test]
    fn squares_are_snapped_and_spaced() {
        let t = Tech::generic();
        let p = FillParams { spacing: 500, ..params(&t, IRect::new(-1234, 3, 20_000, 20_000)) };
        let r = generate(&Layout::default(), &NoCells, &t, &p).unwrap();
        assert!(!r.rects.is_empty());
        for q in &r.rects {
            assert!(q.x0 % t.grid == 0 && q.y0 % t.grid == 0, "{q:?}");
            assert_eq!((q.width(), q.height()), (1000, 1000));
            assert!(q.x0 >= -1234 && q.y0 >= 3 && q.x1 <= 20_000 && q.y1 <= 20_000);
            assert_eq!((q.x0 - r.rects[0].x0) % 1500, 0);
            assert_eq!((q.y0 - r.rects[0].y0) % 1500, 0);
        }
        assert_eq!((r.rects[0].x0, r.rects[0].y0), (-1230, 10));
        assert_eq!(r.rects[1].x0 - r.rects[0].x1, 500);
    }

    #[test]
    fn keepout_around_shapes_is_respected() {
        let t = Tech::generic();
        let mut l = Layout::default();
        let shape = IRect::new(4_000, 4_000, 7_000, 6_000);
        l.add_rect(m1(&t), shape.x0, shape.y0, shape.x1, shape.y1);
        let other = IRect::new(15_000, 15_000, 16_000, 16_000);
        let m2 = t.layer_index("metal2").unwrap();
        l.add_rect(m2, other.x0, other.y0, other.x1, other.y1);
        let area = IRect::new(0, 0, 20_000, 20_000);
        let p = FillParams { keepout: 1500, avoid: vec![m1(&t), m2], ..params(&t, area) };
        let r = generate(&l, &NoCells, &t, &p).unwrap();
        assert!(r.rects.len() < 100 && r.rects.len() > 80, "{}", r.rects.len());
        for q in &r.rects {
            assert!(!q.intersects(&shape) && q.spacing(&shape) >= 1500, "{q:?}");
            assert!(!q.intersects(&other) && q.spacing(&other) >= 1500, "{q:?}");
        }
        assert!((r.before - 6.0 / 400.0).abs() < 1e-9);
        let open = generate(&l, &NoCells, &t, &FillParams { avoid: vec![], ..p }).unwrap();
        assert_eq!(open.rects.len(), 100);
    }

    #[test]
    fn target_density_limits_fill() {
        let t = tech_with_density();
        let p = FillParams::for_layer(&t, m1(&t));
        assert_eq!((p.window, p.step), (50_000, 25_000));
        assert!((p.target.unwrap() - 0.25).abs() < 1e-9);
        let area = IRect::new(0, 0, 200_000, 200_000);
        let p = FillParams { area: Some(area), target: Some(0.3), ..p };
        let r = generate(&Layout::default(), &NoCells, &t, &p).unwrap();
        assert!((r.after - 0.3).abs() < 0.02, "after {}", r.after);
        assert!(r.rects.len() < r.candidates);
        assert_eq!((r.windows_below, r.windows_above), (0, 0));
        assert_eq!(r.limits, Some((0.2, 0.8)));

        let full = generate(&Layout::default(), &NoCells, &t, &FillParams { target: None, ..p.clone() }).unwrap();
        assert_eq!(full.rects.len(), full.candidates);
        assert!(full.after > r.after);
    }

    #[test]
    fn deck_max_caps_target() {
        let mut t = Tech::generic();
        t.drc_deck = Some("density metal1 10 20 10 5".into());
        let area = IRect::new(0, 0, 40_000, 40_000);
        let p = FillParams { target: Some(0.9), window: 10_000, step: 5_000, ..params(&t, area) };
        let r = generate(&Layout::default(), &NoCells, &t, &p).unwrap();
        assert!(r.after <= 0.2 + 1e-9 && r.after > 0.1, "after {}", r.after);
        assert_eq!(r.windows_above, 0);
    }

    #[test]
    fn filled_layout_passes_drc() {
        let t = tech_with_density();
        let mut l = Layout::default();
        l.add_rect(m1(&t), 30_000, 30_000, 60_000, 32_000);
        l.add_rect(m1(&t), 80_000, 10_000, 82_000, 90_000);
        l.add_rect(t.layer_index("boundary").unwrap(), 0, 0, 120_000, 100_000);
        let before = drc::run_tech_deck(&l, &NoCells, &t).unwrap();
        assert!(before.violations.iter().any(|v| v.rule == "metal1.DEN"));
        let r = generate(&l, &NoCells, &t, &FillParams::for_layer(&t, m1(&t))).unwrap();
        assert_eq!(r.windows_below, 0);
        for q in &r.rects {
            l.add_rect(m1(&t), q.x0, q.y0, q.x1, q.y1);
        }
        let after = drc::run_tech_deck(&l, &NoCells, &t).unwrap();
        assert!(after.violations.is_empty(), "{:?}", after.violations);
    }

    #[test]
    fn invalid_params_are_errors() {
        let t = Tech::generic();
        let area = IRect::new(0, 0, 10_000, 10_000);
        let p = params(&t, area);
        let e = Layout::default();
        let err = |q: FillParams| generate(&e, &NoCells, &t, &q).unwrap_err();
        assert!(err(FillParams { size: 200, ..p.clone() }).contains("min width"));
        assert!(err(FillParams { size: 1005, ..p.clone() }).contains("grid"));
        assert!(err(FillParams { spacing: 100, ..p.clone() }).contains("min spacing"));
        assert!(err(FillParams { size: 0, ..p.clone() }).contains("size"));
        assert!(err(FillParams { keepout: -1, ..p.clone() }).contains("keepout"));
        assert!(err(FillParams { layer: 99, ..p.clone() }).contains("does not exist"));
        assert!(err(FillParams { avoid: vec![99], ..p.clone() }).contains("does not exist"));
        assert!(err(FillParams { target: Some(1.5), ..p.clone() }).contains("between 0 and 1"));
        assert!(err(FillParams { target: Some(0.3), window: 0, ..p.clone() }).contains("window"));
        assert!(err(FillParams { area: None, ..p.clone() }).contains("empty"));
        let mut big = t.clone();
        big.layers[m1(&t)].min_area = 2_000_000;
        assert!(generate(&e, &NoCells, &big, &p).unwrap_err().contains("min area"));
        assert!(FillParams::for_layer(&big, m1(&t)).size >= 1415);
    }

    #[test]
    fn large_area_is_fast() {
        let t = Tech::generic();
        let mut l = Layout::default();
        for i in 0..200 {
            l.add_rect(m1(&t), i * 10_000, 0, i * 10_000 + 500, 2_000_000);
        }
        let area = IRect::new(0, 0, 2_000_000, 2_000_000);
        let p = FillParams { area: Some(area), target: Some(0.25), ..FillParams::for_layer(&t, m1(&t)) };
        let start = std::time::Instant::now();
        let r = generate(&l, &NoCells, &t, &p).unwrap();
        assert!(r.rects.len() > 100_000);
        assert!(start.elapsed().as_secs() < 30);
    }
}
