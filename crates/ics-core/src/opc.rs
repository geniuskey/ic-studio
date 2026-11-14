//! Model-based optical proximity correction (OPC) for Manhattan layers.
//!
//! The lithography model is a Gaussian blur of the mask followed by a constant threshold: the
//! aerial image is the mask convolved with a 2-D Gaussian of width `sigma`, and the resist prints
//! wherever it reaches `threshold`. For rectangles the convolution is exact (a product of `erf`
//! differences), so no image grid is needed. Large features print at their drawn size, while
//! narrow lines shrink, line ends pull back and corners round, as on a real scanner.
//!
//! The correction splits every edge of the target into fragments (short corner fragments at
//! both ends), measures the edge placement error (EPE) at the middle of each fragment and moves
//! the fragment along its normal against the error, for a number of iterations. Moved fragments
//! give the mask: the target with outward strips added and inward strips removed, plus corner
//! squares where both edges of a corner move the same way (serifs and mouse bites).

use std::collections::{BTreeMap, HashMap};

use crate::geom::IRect;
use crate::hier::{LayoutSource, flatten};
use crate::layout::{Geometry, Layout};
use crate::region::{BoolOp, Region};
use crate::tech::Tech;

const MAX_DEPTH: u32 = 64;
const MAX_FRAGMENTS: usize = 1 << 20;
/// Mask pieces per spatial bin are bounded by splitting the bounding box into at most this many bins.
const MAX_BINS: f64 = (1 << 20) as f64;
/// Samples of a printed-image raster.
const IMAGE_SAMPLES: f64 = 250_000.0;
/// Gaussian tails beyond this many sigmas are ignored.
const REACH: f64 = 4.0;

/// Correction settings. Lengths in dbu.
#[derive(Debug, Clone, PartialEq)]
pub struct OpcParams {
    /// Technology layer index to correct.
    pub layer: usize,
    /// Width (standard deviation) of the Gaussian optical blur.
    pub sigma: i64,
    /// Resist threshold as a fraction of the open-frame intensity (0..1).
    pub threshold: f64,
    /// Longest fragment; corner fragments are half as long.
    pub fragment: i64,
    /// Furthest a fragment may move outward / inward.
    pub max_out: i64,
    pub max_in: i64,
    pub iterations: u32,
    /// Fraction of the measured EPE corrected per iteration.
    pub gain: f64,
    /// An edge prints on target when its |EPE| is at most this.
    pub tolerance: i64,
}

impl OpcParams {
    /// Defaults from the layer's min width `w` and spacing `s` (0.18 µm when unset): blur
    /// 0.45 w, fragments 2 w, at most s / 3 outward and w / 4 inward, snapped to the grid.
    pub fn for_layer(tech: &Tech, layer: usize) -> OpcParams {
        let g = tech.grid.max(1);
        let um = tech.dbu_per_um.max(1) as f64;
        let def = |v: i64| if v > 0 { v as f64 } else { 0.18 * um };
        let (w, s) = tech.layer(layer).map_or((def(0), def(0)), |l| (def(l.min_width), def(l.min_spacing)));
        let snap = |v: f64| ((v / g as f64).round() as i64).max(1) * g;
        OpcParams {
            layer,
            sigma: snap(0.45 * w),
            threshold: 0.5,
            fragment: snap(2.0 * w),
            max_out: snap(s / 3.0),
            max_in: snap(w / 4.0),
            iterations: 12,
            gain: 0.6,
            tolerance: snap((0.45 * w / 20.0).max(g as f64)),
        }
    }
}

/// A piece of a target edge. `a` to `b` runs along the edge; `normal` points out of the shape.
#[derive(Debug, Clone, PartialEq)]
pub struct Fragment {
    pub a: (i64, i64),
    pub b: (i64, i64),
    pub normal: (i64, i64),
    /// Final mask offset along the normal (positive = outward).
    pub offset: i64,
    /// Printed edge position relative to the target along the normal, without / with correction.
    pub epe_before: f64,
    pub epe_after: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct EpeStats {
    pub rms: f64,
    pub max: f64,
    /// Fragments whose |EPE| is within the tolerance.
    pub within: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct OpcReport {
    /// The drawn shapes of the layer (flattened, merged).
    pub target: Region,
    /// The corrected mask.
    pub mask: Region,
    pub fragments: Vec<Fragment>,
    pub iterations: u32,
    pub before: EpeStats,
    pub after: EpeStats,
    /// What prints from the target and from the mask, sampled on a raster over `area`.
    pub printed_before: Region,
    pub printed_after: Region,
    /// Target bounding box grown by the blur reach.
    pub area: IRect,
}

/// Correct layer `p.layer` of `top` (hierarchy flattened). The layout is not changed.
pub fn correct(top: &Layout, src: &dyn LayoutSource, tech: &Tech, p: &OpcParams) -> Result<OpcReport, String> {
    let def = tech.layer(p.layer).ok_or_else(|| format!("OPC layer #{} does not exist", p.layer))?;
    if p.sigma <= 0 {
        return Err("the blur sigma must be larger than 0".into());
    }
    if !(p.threshold > 0.0 && p.threshold < 1.0) {
        return Err(format!("threshold {} must be between 0 and 1", p.threshold));
    }
    if p.fragment <= 0 {
        return Err("the fragment length must be larger than 0".into());
    }
    if p.max_out < 0 || p.max_in < 0 {
        return Err("the move limits must not be negative".into());
    }
    if !(p.gain > 0.0 && p.gain <= 1.0) {
        return Err(format!("gain {} must be in (0, 1]", p.gain));
    }
    let mut geoms = Vec::new();
    for f in flatten(top, src, MAX_DEPTH) {
        if f.layer == p.layer && !matches!(f.geom, Geometry::Label { .. }) {
            geoms.push(f.geom);
        }
    }
    let target = Region::from_geometries(geoms.iter()).map_err(|e| format!("OPC handles Manhattan shapes only: {e}"))?;
    let bbox = target.bbox().ok_or_else(|| format!("nothing to correct: no shapes on {}", def.name))?;
    let g = tech.grid.max(1);
    run(&target, bbox, g, p)
}

fn run(target: &Region, bbox: IRect, g: i64, p: &OpcParams) -> Result<OpcReport, String> {
    let sigma = p.sigma as f64;
    let reach = (REACH * sigma).ceil() as i64;
    let area = IRect::new(bbox.x0 - reach, bbox.y0 - reach, bbox.x1 + reach, bbox.y1 + reach);
    let edges = boundary(target);
    let (mut frags, corners) = fragment(&edges, target, p.fragment, g);
    if frags.len() > MAX_FRAGMENTS {
        return Err(format!("too many fragments ({}): use longer fragments or a smaller cell", frags.len()));
    }
    let model = |mask: &Region| Image::new(mask, area, sigma, p.threshold);

    let image = model(target);
    let epe_before: Vec<f64> = frags.iter().map(|f| image.epe(f)).collect();
    let mut snapped = vec![0i64; frags.len()];
    let mut prev: Vec<Option<(i64, f64)>> = vec![None; frags.len()];
    let mut epe = epe_before.clone();
    let mut mask = target.clone();
    let mut iterations = 0;
    for _ in 0..p.iterations {
        let mut moved = false;
        for (i, &e) in epe.iter().enumerate() {
            // Secant step on the local mask error enhancement factor once two points are known.
            let step = match prev[i] {
                Some((s0, e0)) if s0 != snapped[i] => -e / ((e - e0) / (snapped[i] - s0) as f64).clamp(0.3, 4.0),
                _ => -p.gain * e,
            };
            let next = (snapped[i] as f64 + step).clamp(-(p.max_in as f64), p.max_out as f64);
            let s = (next / g as f64).round() as i64 * g;
            if s != snapped[i] {
                prev[i] = Some((snapped[i], e));
                moved = true;
            }
            snapped[i] = s;
        }
        if !moved {
            break;
        }
        iterations += 1;
        mask = build_mask(target, &frags, &corners, &snapped);
        let image = model(&mask);
        epe = frags.iter().map(|f| image.epe(f)).collect();
    }
    let final_image = model(&mask);
    let tol = p.tolerance as f64;
    for (i, f) in frags.iter_mut().enumerate() {
        f.offset = snapped[i];
        f.epe_before = epe_before[i];
        f.epe_after = epe[i];
    }
    Ok(OpcReport {
        printed_before: image.printed(area, g),
        printed_after: final_image.printed(area, g),
        before: stats(&epe_before, tol),
        after: stats(&epe, tol),
        target: target.clone(),
        mask,
        fragments: frags,
        iterations,
        area,
    })
}

fn stats(epe: &[f64], tol: f64) -> EpeStats {
    if epe.is_empty() {
        return EpeStats::default();
    }
    let rms = (epe.iter().map(|e| e * e).sum::<f64>() / epe.len() as f64).sqrt();
    let max = epe.iter().fold(0.0f64, |m, e| m.max(e.abs()));
    EpeStats { rms, max, within: epe.iter().filter(|e| e.abs() <= tol).count() }
}

/// A maximal straight piece of the region boundary: along x (`horiz`) at `y = c` or along y at
/// `x = c`, spanning `lo..hi`, with the shape on the side opposite `out` (+1 / -1).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Edge {
    horiz: bool,
    c: i64,
    lo: i64,
    hi: i64,
    out: i64,
}

fn boundary(r: &Region) -> Vec<Edge> {
    let mut edges = Vec::new();
    for horiz in [true, false] {
        // Per coordinate: intervals of rect sides facing + and facing -.
        let mut sides: BTreeMap<i64, [Vec<(i64, i64)>; 2]> = BTreeMap::new();
        for q in r.rects() {
            let (lo, hi, a, b) = if horiz { (q.x0, q.x1, q.y0, q.y1) } else { (q.y0, q.y1, q.x0, q.x1) };
            sides.entry(b).or_default()[0].push((lo, hi));
            sides.entry(a).or_default()[1].push((lo, hi));
        }
        for (c, [plus, minus]) in sides {
            let (plus, minus) = (merge(plus), merge(minus));
            for (lo, hi) in subtract(&plus, &minus) {
                edges.push(Edge { horiz, c, lo, hi, out: 1 });
            }
            for (lo, hi) in subtract(&minus, &plus) {
                edges.push(Edge { horiz, c, lo, hi, out: -1 });
            }
        }
    }
    edges
}

fn merge(mut v: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    v.sort_unstable();
    let mut out: Vec<(i64, i64)> = Vec::with_capacity(v.len());
    for (lo, hi) in v {
        match out.last_mut() {
            Some(last) if lo <= last.1 => last.1 = last.1.max(hi),
            _ => out.push((lo, hi)),
        }
    }
    out
}

/// Sorted disjoint intervals `a` minus `b`.
fn subtract(a: &[(i64, i64)], b: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    let mut j = 0;
    for &(lo, hi) in a {
        let mut cur = lo;
        while j < b.len() && b[j].1 <= cur {
            j += 1;
        }
        let mut k = j;
        while k < b.len() && b[k].0 < hi {
            if b[k].0 > cur {
                out.push((cur, b[k].0));
            }
            cur = cur.max(b[k].1);
            k += 1;
        }
        if cur < hi {
            out.push((cur, hi));
        }
    }
    out
}

/// A corner where one horizontal and one vertical fragment meet. `quad` is the odd quadrant
/// (filled for a convex corner, empty for a concave one), as signs of x and y.
#[derive(Debug, Clone, Copy)]
struct Corner {
    at: (i64, i64),
    h: usize,
    v: usize,
    convex: bool,
    quad: (i64, i64),
}

fn fragment(edges: &[Edge], target: &Region, len: i64, g: i64) -> (Vec<Fragment>, Vec<Corner>) {
    let snap = |v: i64| (v as f64 / g as f64).round() as i64 * g;
    let mut frags = Vec::new();
    let mut ends: HashMap<(i64, i64), Vec<(usize, bool)>> = HashMap::new();
    for e in edges {
        let l = e.hi - e.lo;
        let corner = snap(len / 2).max(g);
        let mut cuts = vec![e.lo];
        if l > len && l - 2 * corner > 0 {
            let middle = l - 2 * corner;
            let n = (middle + len - 1) / len;
            cuts.push(e.lo + corner);
            for k in 1..n {
                cuts.push(snap(e.lo + corner + middle * k / n));
            }
            cuts.push(e.hi - corner);
        }
        cuts.push(e.hi);
        cuts.dedup();
        let first = frags.len();
        for w in cuts.windows(2) {
            let pt = |t: i64| if e.horiz { (t, e.c) } else { (e.c, t) };
            let normal = if e.horiz { (0, e.out) } else { (e.out, 0) };
            frags.push(Fragment { a: pt(w[0]), b: pt(w[1]), normal, offset: 0, epe_before: 0.0, epe_after: 0.0 });
        }
        let last = frags.len() - 1;
        ends.entry(frags[first].a).or_default().push((first, e.horiz));
        ends.entry(frags[last].b).or_default().push((last, e.horiz));
    }
    let index = Bins::new(target.rects().iter().map(|r| (*r, 1.0)), 1024);
    let mut corners = Vec::new();
    for (at, list) in ends {
        let (hs, vs): (Vec<&(usize, bool)>, Vec<_>) = list.iter().partition(|(_, h)| *h);
        if hs.len() != 1 || vs.len() != 1 {
            continue;
        }
        let (x, y) = at;
        let quads = [(-1, -1), (1, -1), (-1, 1), (1, 1)];
        let filled: Vec<bool> = quads
            .iter()
            .map(|&(sx, sy)| index.contains(if sx < 0 { x - 1 } else { x }, if sy < 0 { y - 1 } else { y }))
            .collect();
        let n = filled.iter().filter(|f| **f).count();
        let convex = n == 1;
        if n != 1 && n != 3 {
            continue;
        }
        let Some(k) = filled.iter().position(|f| *f == convex) else { continue };
        corners.push(Corner { at, h: hs[0].0, v: vs[0].0, convex, quad: quads[k] });
    }
    (frags, corners)
}

/// Strip swept by moving fragment `f` by `d` along its normal.
fn strip(f: &Fragment, d: i64) -> IRect {
    let (ax, ay) = (f.a.0 + f.normal.0 * d, f.a.1 + f.normal.1 * d);
    let (bx, by) = f.b;
    IRect::new(ax.min(bx), ay.min(by), ax.max(bx), ay.max(by))
}

fn build_mask(target: &Region, frags: &[Fragment], corners: &[Corner], off: &[i64]) -> Region {
    let mut add = Vec::new();
    let mut cut = Vec::new();
    for (f, &d) in frags.iter().zip(off) {
        match d.signum() {
            1 => add.push(strip(f, d)),
            -1 => cut.push(strip(f, d)),
            _ => {}
        }
    }
    for c in corners {
        let (dh, dv) = (off[c.h], off[c.v]);
        let grow = c.convex && dh > 0 && dv > 0;
        let shrink = !c.convex && dh < 0 && dv < 0;
        if grow || shrink {
            let (x, y) = c.at;
            let (w, h) = (dv.abs() * -c.quad.0, dh.abs() * -c.quad.1);
            let r = IRect::new(x.min(x + w), y.min(y + h), x.max(x + w), y.max(y + h));
            if grow { add.push(r) } else { cut.push(r) }
        }
    }
    target.boolean(&Region::from_rects(add), BoolOp::Or).boolean(&Region::from_rects(cut), BoolOp::Not)
}

/// Weighted rectangles bucketed on a uniform grid, each clipped to its bins so that bins hold
/// only their own pieces.
struct Bins {
    cell: i64,
    map: HashMap<(i64, i64), Vec<(IRect, f64)>>,
}

impl Bins {
    fn new(rects: impl Iterator<Item = (IRect, f64)>, cell: i64) -> Bins {
        let cell = cell.max(1);
        let mut map: HashMap<(i64, i64), Vec<(IRect, f64)>> = HashMap::new();
        for (r, w) in rects {
            if r.width() <= 0 || r.height() <= 0 {
                continue;
            }
            let (i0, i1) = (r.x0.div_euclid(cell), (r.x1 - 1).div_euclid(cell));
            let (j0, j1) = (r.y0.div_euclid(cell), (r.y1 - 1).div_euclid(cell));
            for i in i0..=i1 {
                for j in j0..=j1 {
                    let b = IRect::new(i * cell, j * cell, (i + 1) * cell, (j + 1) * cell);
                    let piece = IRect::new(r.x0.max(b.x0), r.y0.max(b.y0), r.x1.min(b.x1), r.y1.min(b.y1));
                    map.entry((i, j)).or_default().push((piece, w));
                }
            }
        }
        Bins { cell, map }
    }

    /// Whether the unit cell at `(x, y)` is covered.
    fn contains(&self, x: i64, y: i64) -> bool {
        self.map
            .get(&(x.div_euclid(self.cell), y.div_euclid(self.cell)))
            .is_some_and(|v| v.iter().any(|(r, _)| x >= r.x0 && x < r.x1 && y >= r.y0 && y < r.y1))
    }
}

/// The aerial image of a mask.
struct Image {
    bins: Bins,
    reach: f64,
    scale: f64,
    threshold: f64,
    sigma: f64,
}

impl Image {
    fn new(mask: &Region, area: IRect, sigma: f64, threshold: f64) -> Image {
        let reach = REACH * sigma;
        let side = ((area.width() as f64 * area.height() as f64) / MAX_BINS).sqrt();
        let cell = reach.max(side).ceil() as i64;
        Image {
            bins: Bins::new(mask.rects().iter().map(|r| (*r, 1.0)), cell),
            reach,
            scale: 1.0 / (std::f64::consts::SQRT_2 * sigma),
            threshold,
            sigma,
        }
    }

    fn intensity(&self, x: f64, y: f64) -> f64 {
        let c = self.bins.cell as f64;
        let (i0, i1) = (((x - self.reach) / c).floor() as i64, ((x + self.reach) / c).floor() as i64);
        let (j0, j1) = (((y - self.reach) / c).floor() as i64, ((y + self.reach) / c).floor() as i64);
        let mut sum = 0.0;
        for i in i0..=i1 {
            for j in j0..=j1 {
                let Some(v) = self.bins.map.get(&(i, j)) else { continue };
                for (r, w) in v {
                    let (x0, x1, y0, y1) = (r.x0 as f64, r.x1 as f64, r.y0 as f64, r.y1 as f64);
                    if x0 - x > self.reach || x - x1 > self.reach || y0 - y > self.reach || y - y1 > self.reach {
                        continue;
                    }
                    let fx = erf((x1 - x) * self.scale) - erf((x0 - x) * self.scale);
                    let fy = erf((y1 - y) * self.scale) - erf((y0 - y) * self.scale);
                    sum += 0.25 * w * fx * fy;
                }
            }
        }
        sum
    }

    /// Distance along the outward normal from the middle of `f` to the nearest printed edge,
    /// within the blur reach (clamped to it when none is found).
    fn epe(&self, f: &Fragment) -> f64 {
        let (mx, my) = ((f.a.0 + f.b.0) as f64 / 2.0, (f.a.1 + f.b.1) as f64 / 2.0);
        let (nx, ny) = (f.normal.0 as f64, f.normal.1 as f64);
        let at = |t: f64| self.intensity(mx + nx * t, my + ny * t) - self.threshold;
        let f0 = at(0.0);
        let dir = if f0 >= 0.0 { 1.0 } else { -1.0 };
        let step = self.sigma / 4.0;
        let mut t0: f64 = 0.0;
        while t0.abs() < self.reach {
            let t1 = t0 + dir * step;
            if (at(t1) >= 0.0) != (f0 >= 0.0) {
                let (mut lo, mut hi) = (t0, t1);
                for _ in 0..24 {
                    let mid = (lo + hi) / 2.0;
                    if (at(mid) >= 0.0) == (f0 >= 0.0) {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                return (lo + hi) / 2.0;
            }
            t0 = t1;
        }
        dir * self.reach
    }

    /// Printed area sampled at cell centres of a raster over `area`, as a region.
    fn printed(&self, area: IRect, g: i64) -> Region {
        let side = ((area.width() as f64 * area.height() as f64) / IMAGE_SAMPLES).sqrt();
        let h = ((side.max(self.sigma / 3.0) / g as f64).ceil() as i64).max(1) * g;
        let mut rects = Vec::new();
        let mut y = area.y0;
        while y < area.y1 {
            let mut run: Option<i64> = None;
            let mut x = area.x0;
            while x < area.x1 {
                let on = self.intensity((x as f64) + h as f64 / 2.0, (y as f64) + h as f64 / 2.0) >= self.threshold;
                match (on, run) {
                    (true, None) => run = Some(x),
                    (false, Some(s)) => {
                        rects.push(IRect::new(s, y, x, y + h));
                        run = None;
                    }
                    _ => {}
                }
                x += h;
            }
            if let Some(s) = run {
                rects.push(IRect::new(s, y, x, y + h));
            }
            y += h;
        }
        Region::from_rects(rects)
    }
}

/// Error function (Abramowitz & Stegun 7.1.26, |error| < 1.5e-7).
fn erf(x: f64) -> f64 {
    let t = 1.0 / (1.0 + 0.327_591_1 * x.abs());
    let poly = t * (0.254_829_592 + t * (-0.284_496_736 + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    let y = 1.0 - poly * (-x * x).exp();
    if x < 0.0 { -y } else { y }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> OpcParams {
        OpcParams {
            layer: 0,
            sigma: 80,
            threshold: 0.5,
            fragment: 360,
            max_out: 80,
            max_in: 45,
            iterations: 12,
            gain: 0.6,
            tolerance: 3,
        }
    }

    #[test]
    fn erf_matches_known_values() {
        assert!(erf(0.0).abs() < 1e-7);
        assert!((erf(1.0) - 0.842_700_79).abs() < 2e-7);
        assert!((erf(-0.5) + 0.520_499_88).abs() < 2e-7);
    }

    #[test]
    fn boundary_of_an_l_shape() {
        let r = Region::from_rects([IRect::new(0, 0, 300, 100), IRect::new(0, 100, 100, 300)]);
        let e = boundary(&r);
        assert_eq!(e.len(), 6);
        let len: i64 = e.iter().map(|e| e.hi - e.lo).sum();
        assert_eq!(len, r.perimeter());
        let (frags, corners) = fragment(&e, &r, 1000, 1);
        assert_eq!(frags.len(), 6);
        assert_eq!(corners.iter().filter(|c| c.convex).count(), 5);
        assert_eq!(corners.iter().filter(|c| !c.convex).count(), 1);
    }

    #[test]
    fn zero_offsets_give_the_target() {
        let r = Region::from_rects([IRect::new(0, 0, 3000, 180), IRect::new(0, 180, 180, 2000)]);
        let (frags, corners) = fragment(&boundary(&r), &r, 360, 5);
        assert!(frags.len() > 6);
        assert_eq!(build_mask(&r, &frags, &corners, &vec![0; frags.len()]), r);
        let grown = build_mask(&r, &frags, &corners, &vec![10; frags.len()]);
        assert_eq!(grown, r.sized(10));
        let shrunk = build_mask(&r, &frags, &corners, &vec![-10; frags.len()]);
        assert_eq!(shrunk, r.sized(-10));
    }

    #[test]
    fn wide_edges_print_on_target() {
        let r = Region::from_rects([IRect::new(0, 0, 5000, 5000)]);
        let img = Image::new(&r, IRect::new(-500, -500, 5500, 5500), 60.0, 0.5);
        assert!((img.intensity(2500.0, 0.0) - 0.5).abs() < 1e-6);
        let f = Fragment { a: (2000, 0), b: (3000, 0), normal: (0, -1), offset: 0, epe_before: 0.0, epe_after: 0.0 };
        assert!(img.epe(&f).abs() < 0.5);
    }

    #[test]
    fn correction_reduces_line_end_pullback() {
        let r = Region::from_rects([IRect::new(0, 0, 180, 2000), IRect::new(540, 0, 720, 2000), IRect::new(1500, 0, 3500, 1200)]);
        let bbox = r.bbox().unwrap();
        let rep = run(&r, bbox, 5, &params()).unwrap();
        assert!(rep.before.rms > 2.0 * rep.after.rms, "{:?} -> {:?}", rep.before, rep.after);
        assert_eq!(rep.after.within, rep.fragments.len(), "{:?} -> {:?}", rep.before, rep.after);
        assert!(rep.after.max < rep.before.max / 3.0);
        let end = rep.fragments.iter().find(|f| f.normal == (0, 1) && f.a.1 == 2000 && f.a.0 == 0).unwrap();
        assert!(end.epe_before < -20.0, "{end:?}");
        assert!(end.offset > 0);
        assert!(rep.mask.area() > r.area());
        assert!(rep.mask.rects().iter().all(|q| q.x0 % 5 == 0 && q.y0 % 5 == 0 && q.x1 % 5 == 0 && q.y1 % 5 == 0));
        assert!(!rep.printed_after.is_empty());
    }
}
