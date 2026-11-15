//! Ray-traced image sensor pixel: quantum efficiency, losses and optical crosstalk of a Bayer
//! array under a plane wave at a chief ray angle.
//!
//! Geometry (z up, µm): the silicon surface is at z = 0; above it, from the bottom up, the
//! anti-reflection films (coherent thin films), then the thick layers in the order given
//! (listed top to bottom), then one spherical-cap microlens per pixel. The array is periodic
//! with a 2 × 2 Bayer cell; pixel (i, j) covers [i·p, (i+1)·p) × [j·p, (j+1)·p).
//!
//! Rays start uniformly over one Bayer cell. Each refracts at the lens and at every planar
//! interface (Snell, Fresnel power transmission for unpolarised light), is attenuated in
//! absorbing layers (the colour filter at the ray's position), stopped by metal grid lines,
//! enters silicon with the transmission of the anti-reflection films at its angle, and is
//! absorbed in silicon along its path. Carriers generated between the photodiode's top and
//! bottom depth are collected by the pixel above the point where they are generated.

use num_complex::Complex64;
use serde::Deserialize;

use crate::material::{MaterialSpec, Materials};
use crate::tmm;

/// The four pixels of a Bayer cell.
pub const COLORS: [&str; 4] = ["R", "Gr", "Gb", "B"];

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Microlens {
    /// Height of the cap (µm).
    pub height: f64,
    /// Radius of the cap's base (µm); default: half the pixel diagonal (gapless).
    #[serde(default)]
    pub radius: Option<f64>,
    #[serde(default = "resin")]
    pub material: String,
}

fn resin() -> String {
    "resin".into()
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grid {
    pub material: String,
    /// Line width centred on the pixel boundaries (µm).
    pub width: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerKind {
    Uniform,
    /// Takes each pixel's colour filter material.
    ColorFilter,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    #[serde(default)]
    pub name: String,
    #[serde(default = "uniform")]
    pub kind: LayerKind,
    /// Material (ignored for colour filter layers).
    #[serde(default)]
    pub material: String,
    pub thickness: f64,
    /// Metal grid lines within the layer.
    #[serde(default)]
    pub grid: Option<Grid>,
}

fn uniform() -> LayerKind {
    LayerKind::Uniform
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Film {
    pub material: String,
    pub thickness: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Photodiode {
    /// Collection depth range below the silicon surface (µm).
    pub top: f64,
    pub bottom: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColorFilters {
    #[serde(default = "cf_red")]
    pub red: String,
    #[serde(default = "cf_green")]
    pub green: String,
    #[serde(default = "cf_blue")]
    pub blue: String,
}

fn cf_red() -> String {
    "cf_red".into()
}
fn cf_green() -> String {
    "cf_green".into()
}
fn cf_blue() -> String {
    "cf_blue".into()
}

impl Default for ColorFilters {
    fn default() -> Self {
        ColorFilters { red: cf_red(), green: cf_green(), blue: cf_blue() }
    }
}

/// A pixel stack as written in a TOML file.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PixelStack {
    #[serde(default)]
    pub name: String,
    /// Pixel pitch (µm).
    pub pitch: f64,
    pub microlens: Option<Microlens>,
    /// Thick layers below the microlens, top to bottom.
    #[serde(default)]
    pub layers: Vec<Layer>,
    /// Thin films on the silicon, top to bottom (coherent).
    #[serde(default)]
    pub arc: Vec<Film>,
    pub photodiode: Photodiode,
    #[serde(default)]
    pub color_filters: ColorFilters,
    /// Extra materials by name.
    #[serde(default)]
    pub materials: std::collections::BTreeMap<String, MaterialSpec>,
    /// Medium above the lens.
    #[serde(default = "air")]
    pub ambient: String,
}

fn air() -> String {
    "air".into()
}

impl PixelStack {
    pub fn from_toml(text: &str) -> Result<PixelStack, String> {
        let s: PixelStack = toml::from_str(text).map_err(|e| e.to_string())?;
        s.check()?;
        Ok(s)
    }

    // `!(x > 0)` also rejects NaN.
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    fn check(&self) -> Result<(), String> {
        if !(self.pitch > 0.0) {
            return Err("pitch must be positive".into());
        }
        if !(self.photodiode.bottom > self.photodiode.top && self.photodiode.top >= 0.0) {
            return Err("photodiode: need 0 <= top < bottom".into());
        }
        for l in &self.layers {
            if !(l.thickness > 0.0) {
                return Err(format!("layer '{}': thickness must be positive", l.name));
            }
            if l.kind == LayerKind::Uniform && l.material.is_empty() {
                return Err(format!("layer '{}': material missing", l.name));
            }
            if let Some(g) = &l.grid
                && !(g.width > 0.0 && g.width < self.pitch)
            {
                return Err(format!("layer '{}': grid width must be between 0 and the pitch", l.name));
            }
        }
        if let Some(m) = &self.microlens
            && !(m.height > 0.0 && m.radius.is_none_or(|r| r > m.height))
        {
            return Err("microlens: need height > 0 and radius > height".into());
        }
        Ok(())
    }

    fn materials(&self) -> Result<Materials, String> {
        let mut m = Materials::default();
        for (name, spec) in &self.materials {
            m.add(name, spec)?;
        }
        Ok(m)
    }

    /// Colour of pixel (i, j) in the RGGB cell: 0 R, 1 Gr, 2 Gb, 3 B.
    fn color(i: i64, j: i64) -> usize {
        match (i.rem_euclid(2), j.rem_euclid(2)) {
            (0, 0) => 0,
            (1, 0) => 1,
            (0, 1) => 2,
            _ => 3,
        }
    }
}

/// Where light goes, as fractions of the light incident on the array.
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize)]
pub struct Budget {
    pub reflected: f64,
    pub color_filter: f64,
    pub metal: f64,
    pub other_layers: f64,
    pub silicon_shallow: f64,
    pub collected: f64,
    pub silicon_deep: f64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PixelResult {
    pub name: String,
    /// Chief ray angle (degrees) and its azimuth (degrees from +x).
    pub cra: f64,
    pub azimuth: f64,
    /// Wavelengths (µm).
    pub wavelengths: Vec<f64>,
    /// Quantum efficiency of R, Gr, Gb, B per wavelength: carriers collected in the pixel per
    /// photon incident on its area.
    pub qe: [Vec<f64>; 4],
    /// Optical crosstalk per wavelength and colour: of the light entering through a pixel's
    /// microlens that is collected, the fraction collected by another pixel.
    pub crosstalk: [Vec<f64>; 4],
    pub budget: Vec<Budget>,
    /// Rays per Bayer cell.
    pub rays: usize,
}

/// Simulation settings.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub wavelengths: Vec<f64>,
    /// Chief ray angle and azimuth (degrees).
    pub cra: f64,
    pub azimuth: f64,
    /// Rays per pixel side (rays per cell = (2 n)²).
    pub rays_per_side: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { wavelengths: (0..=62).map(|i| 0.38 + i as f64 * 0.01).collect(), cra: 0.0, azimuth: 0.0, rays_per_side: 40 }
    }
}

type V3 = [f64; 3];

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Refract unit direction `d` at a surface with unit normal `n` (pointing back toward the
/// incoming side) from index `n1` to `n2`; None on total internal reflection.
fn refract(d: V3, n: V3, n1: f64, n2: f64) -> Option<V3> {
    let eta = n1 / n2;
    let cos_i = -dot(n, d);
    let sin2_t = eta * eta * (1.0 - cos_i * cos_i);
    if sin2_t > 1.0 {
        return None;
    }
    let k = eta * cos_i - (1.0 - sin2_t).sqrt();
    Some([eta * d[0] + k * n[0], eta * d[1] + k * n[1], eta * d[2] + k * n[2]])
}

/// Unpolarised Fresnel power transmission at incidence cosine `cos_i` between the thick layers
/// of indices `n1` and `n2` (real parts: their absorption is applied along the path).
fn fresnel_t(n1: Complex64, n2: Complex64, cos_i: f64) -> f64 {
    let th = cos_i.clamp(-1.0, 1.0).acos();
    let real = |n: Complex64| Complex64::new(n.re, 0.0);
    tmm::rt_unpolarised(real(n1), &[], real(n2), th, 0.55).t.clamp(0.0, 1.0)
}

/// A thick layer at one wavelength: base index, colour filter indices (R, G, B) for filter
/// layers, grid metal index and line width, thickness.
type LayerOptics = (Complex64, Option<[Complex64; 3]>, Option<(Complex64, f64)>, f64);

/// Optical properties of the stack at one wavelength.
struct Optics {
    ambient: Complex64,
    lens: Option<Complex64>,
    layers: Vec<LayerOptics>,
    arc: Vec<(Complex64, f64)>,
    si: Complex64,
}

/// The cap of the microlens of pixel (i, j): (centre, radius) of its sphere, or None.
fn lens_sphere(stack: &PixelStack, ml: &Microlens, top: f64, i: i64, j: i64) -> ([f64; 3], f64) {
    let p = stack.pitch;
    let a = ml.radius.unwrap_or(p / std::f64::consts::SQRT_2);
    let rc = (a * a + ml.height * ml.height) / (2.0 * ml.height);
    let cx = (i as f64 + 0.5) * p;
    let cy = (j as f64 + 0.5) * p;
    ([cx, cy, top + ml.height - rc], rc)
}

/// Run the pixel model.
pub fn simulate(stack: &PixelStack, settings: &Settings) -> Result<PixelResult, String> {
    stack.check()?;
    let mats = stack.materials()?;
    let p = stack.pitch;
    let n_side = settings.rays_per_side.max(2);
    let lens_base: f64 =
        stack.layers.iter().map(|l| l.thickness).sum::<f64>() + stack.arc.iter().map(|f| f.thickness).sum::<f64>();
    let arc_top: f64 = stack.arc.iter().map(|f| f.thickness).sum();
    let (cra, az) = (settings.cra.to_radians(), settings.azimuth.to_radians());
    let d0: V3 = [cra.sin() * az.cos(), cra.sin() * az.sin(), -cra.cos()];

    let mut qe: [Vec<f64>; 4] = Default::default();
    let mut xt: [Vec<f64>; 4] = Default::default();
    let mut budget = vec![];
    for &lambda in &settings.wavelengths {
        let idx = |name: &str| mats.get(name).map(|m| m.index(lambda));
        let cf = [idx(&stack.color_filters.red)?, idx(&stack.color_filters.green)?, idx(&stack.color_filters.blue)?];
        let mut layers = vec![];
        for l in &stack.layers {
            let base = match l.kind {
                LayerKind::ColorFilter => cf[1],
                LayerKind::Uniform => idx(&l.material)?,
            };
            let filters = (l.kind == LayerKind::ColorFilter).then_some(cf);
            let grid = match &l.grid {
                Some(g) => Some((idx(&g.material)?, g.width)),
                None => None,
            };
            layers.push((base, filters, grid, l.thickness));
        }
        let optics = Optics {
            ambient: idx(&stack.ambient)?,
            lens: match &stack.microlens {
                Some(m) => Some(idx(&m.material)?),
                None => None,
            },
            layers,
            arc: stack.arc.iter().map(|f| idx(&f.material).map(|n| (n, f.thickness))).collect::<Result<_, _>>()?,
            si: idx("si")?,
        };

        let mut collected = [0.0; 4];
        // Light entering through each colour's lens: total collected and collected elsewhere.
        let mut entered = [[0.0; 2]; 4];
        let mut b = Budget::default();
        let w = 1.0 / ((2 * n_side) * (2 * n_side)) as f64;
        for iy in 0..2 * n_side {
            for ix in 0..2 * n_side {
                let x0 = (ix as f64 + 0.5) / n_side as f64 * p;
                let y0 = (iy as f64 + 0.5) / n_side as f64 * p;
                trace(stack, &optics, lambda, [x0, y0], d0, lens_base, arc_top, w, &mut collected, &mut entered, &mut b);
            }
        }
        // QE per pixel: the cell holds one pixel of each colour, illuminated with 4 pixels' light.
        for c in 0..4 {
            qe[c].push(collected[c] * 4.0);
            xt[c].push(if entered[c][0] > 0.0 { entered[c][1] / entered[c][0] } else { 0.0 });
        }
        budget.push(b);
    }
    Ok(PixelResult {
        name: stack.name.clone(),
        cra: settings.cra,
        azimuth: settings.azimuth,
        wavelengths: settings.wavelengths.clone(),
        qe,
        crosstalk: xt,
        budget,
        rays: (2 * n_side) * (2 * n_side),
    })
}

fn pixel_of(x: f64, y: f64, p: f64) -> (i64, i64) {
    ((x / p).floor() as i64, (y / p).floor() as i64)
}

/// Whether (x, y) lies on a grid line of `width` centred on the pixel boundaries.
fn on_grid(x: f64, y: f64, p: f64, width: f64) -> bool {
    let near = |v: f64| {
        let r = v.rem_euclid(p);
        r < width / 2.0 || r > p - width / 2.0
    };
    near(x) || near(y)
}

#[allow(clippy::too_many_arguments)]
fn trace(
    stack: &PixelStack,
    o: &Optics,
    lambda: f64,
    start: [f64; 2],
    d0: V3,
    lens_base: f64,
    arc_top: f64,
    w: f64,
    collected: &mut [f64; 4],
    entered: &mut [[f64; 2]; 4],
    b: &mut Budget,
) {
    let p = stack.pitch;
    let absorb = |n: Complex64, len: f64| (-4.0 * std::f64::consts::PI * n.im * len / lambda).exp();
    let mut power = w;
    let mut d = d0;
    let mut n_cur = o.ambient;

    // Microlens: first cap hit among the pixels around the ray, else the lens base plane.
    let mut pos: V3;
    let lens_pixel;
    match (&stack.microlens, o.lens) {
        (Some(ml), Some(n_lens)) => {
            let z_start = lens_base + ml.height + 1.0;
            // Back up the start so that the ray reaches (x0, y0) at the lens top.
            let t_back = (z_start - (lens_base + ml.height)) / -d[2];
            let origin = [start[0] - d[0] * t_back, start[1] - d[1] * t_back, z_start];
            let (pi, pj) = pixel_of(start[0], start[1], p);
            let mut best: Option<(f64, V3, V3, (i64, i64))> = None;
            for di in -1..=1 {
                for dj in -1..=1 {
                    let (i, j) = (pi + di, pj + dj);
                    let (c, rc) = lens_sphere(stack, ml, lens_base, i, j);
                    let oc = [origin[0] - c[0], origin[1] - c[1], origin[2] - c[2]];
                    let bq = dot(oc, d);
                    let disc = bq * bq - (dot(oc, oc) - rc * rc);
                    if disc < 0.0 {
                        continue;
                    }
                    let t = -bq - disc.sqrt();
                    let hit = [origin[0] + t * d[0], origin[1] + t * d[1], origin[2] + t * d[2]];
                    if hit[2] < lens_base || pixel_of(hit[0], hit[1], p) != (i, j) {
                        continue;
                    }
                    if best.as_ref().is_none_or(|b| t < b.0) {
                        let nrm = [(hit[0] - c[0]) / rc, (hit[1] - c[1]) / rc, (hit[2] - c[2]) / rc];
                        best = Some((t, hit, nrm, (i, j)));
                    }
                }
            }
            match best {
                Some((_, hit, nrm, pix)) => {
                    let cos_i = -dot(nrm, d);
                    let t = fresnel_t(n_cur, n_lens, cos_i);
                    b.reflected += power * (1.0 - t);
                    power *= t;
                    let Some(nd) = refract(d, nrm, n_cur.re, n_lens.re) else {
                        b.reflected += w;
                        return;
                    };
                    d = nd;
                    n_cur = n_lens;
                    // Travel through the lens down to its base.
                    let t = (hit[2] - lens_base) / -d[2];
                    pos = [hit[0] + t * d[0], hit[1] + t * d[1], lens_base];
                    let lost = power * (1.0 - absorb(n_lens, t));
                    b.other_layers += lost;
                    power -= lost;
                    lens_pixel = pix;
                }
                None => {
                    // Between caps: straight onto the lens base.
                    let t = (z_start - lens_base) / -d[2];
                    pos = [origin[0] + t * d[0], origin[1] + t * d[1], lens_base];
                    lens_pixel = pixel_of(pos[0], pos[1], p);
                }
            }
        }
        _ => {
            pos = [start[0], start[1], lens_base];
            lens_pixel = pixel_of(start[0], start[1], p);
        }
    }

    // Thick layers, top to bottom.
    let mut z = lens_base;
    for (k, &(base, filters, grid, thickness)) in o.layers.iter().enumerate() {
        let n_layer = match filters {
            Some(cf) => {
                let (i, j) = pixel_of(pos[0], pos[1], p);
                cf[[0, 1, 1, 2][PixelStack::color(i, j)]]
            }
            None => base,
        };
        let nrm = [0.0, 0.0, 1.0];
        let cos_i = -d[2];
        let t = fresnel_t(n_cur, n_layer, cos_i);
        b.reflected += power * (1.0 - t);
        power *= t;
        let Some(nd) = refract(d, nrm, n_cur.re, n_layer.re) else {
            b.reflected += power;
            return;
        };
        d = nd;
        n_cur = n_layer;
        // Through the layer in slices: colour filter by position, grid lines stop the ray.
        const SLICES: usize = 8;
        let dz = thickness / SLICES as f64;
        let step = dz / -d[2];
        for _ in 0..SLICES {
            let mid = [pos[0] + 0.5 * step * d[0], pos[1] + 0.5 * step * d[1]];
            if let Some((_, gw)) = grid
                && on_grid(mid[0], mid[1], p, gw)
            {
                b.metal += power;
                return;
            }
            let n_here = match filters {
                Some(cf) => {
                    let (i, j) = pixel_of(mid[0], mid[1], p);
                    cf[[0, 1, 1, 2][PixelStack::color(i, j)]]
                }
                None => n_layer,
            };
            let lost = power * (1.0 - absorb(n_here, step));
            if filters.is_some() {
                b.color_filter += lost;
            } else {
                b.other_layers += lost;
            }
            power -= lost;
            pos = [pos[0] + step * d[0], pos[1] + step * d[1], pos[2] - dz];
        }
        z -= thickness;
        let _ = k;
    }
    debug_assert!((z - arc_top).abs() < 1e-9);

    // Anti-reflection films and the silicon surface (coherent), at the ray's angle; the films
    // are thin, so the ray's lateral shift through them is neglected.
    let cos_i = -d[2];
    let theta = cos_i.clamp(-1.0, 1.0).acos();
    let rt = tmm::rt_unpolarised(n_cur, &o.arc, o.si, theta, lambda);
    b.reflected += power * rt.r;
    b.other_layers += power * (1.0 - rt.r - rt.t).max(0.0);
    power *= rt.t;
    let Some(dsi) = refract(d, [0.0, 0.0, 1.0], n_cur.re, o.si.re) else {
        b.reflected += power;
        return;
    };

    // Silicon: generation along the path, collected over the photodiode depth by the pixel
    // above the generation point.
    let alpha = 4.0 * std::f64::consts::PI * o.si.im / lambda;
    let pd = &stack.photodiode;
    let path_per_depth = 1.0 / -dsi[2];
    let generated = |z0: f64, z1: f64| (-alpha * z0 * path_per_depth).exp() - (-alpha * z1 * path_per_depth).exp();
    b.silicon_shallow += power * generated(0.0, pd.top);
    b.silicon_deep += power * (-alpha * pd.bottom * path_per_depth).exp();
    const STEPS: usize = 48;
    let lens_color = PixelStack::color(lens_pixel.0, lens_pixel.1);
    for s in 0..STEPS {
        let za = pd.top + (pd.bottom - pd.top) * s as f64 / STEPS as f64;
        let zb = pd.top + (pd.bottom - pd.top) * (s + 1) as f64 / STEPS as f64;
        let zm = 0.5 * (za + zb);
        let x = pos[0] + dsi[0] * zm * path_per_depth;
        let y = pos[1] + dsi[1] * zm * path_per_depth;
        let g = power * generated(za, zb);
        let (i, j) = pixel_of(x, y, p);
        collected[PixelStack::color(i, j)] += g;
        b.collected += g;
        entered[lens_color][0] += g;
        if (i, j) != lens_pixel {
            entered[lens_color][1] += g;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BSI: &str = r#"
name = "bsi_1p4"
pitch = 1.4
[microlens]
height = 0.55
[[layers]]
name = "planarisation"
material = "resin"
thickness = 0.1
[[layers]]
name = "colour filter"
kind = "color_filter"
thickness = 0.6
[[layers]]
name = "grid"
material = "sio2"
thickness = 0.25
grid = { material = "w", width = 0.1 }
[[layers]]
name = "buffer"
material = "sio2"
thickness = 0.1
[[arc]]
material = "ta2o5"
thickness = 0.055
[[arc]]
material = "hfo2"
thickness = 0.01
[photodiode]
top = 0.05
bottom = 3.0
"#;

    fn run(stack: &PixelStack, cra: f64) -> PixelResult {
        simulate(stack, &Settings { cra, rays_per_side: 24, ..Settings::default() }).unwrap()
    }

    fn at(r: &PixelResult, c: usize, l: f64) -> f64 {
        let i = r.wavelengths.iter().position(|&x| (x - l).abs() < 1e-9).unwrap();
        r.qe[c][i]
    }

    #[test]
    fn bare_silicon_matches_thin_film_optics() {
        // No lens, no layers: QE = T(air|Si) · absorbed fraction in the photodiode depth.
        let s = PixelStack::from_toml("pitch = 2.0\n[photodiode]\ntop = 0.0\nbottom = 2.0\n").unwrap();
        let r = run(&s, 0.0);
        let m = Materials::default();
        for (i, &l) in r.wavelengths.iter().enumerate() {
            let si = m.get("si").unwrap().index(l);
            let t = tmm::rt_unpolarised(Complex64::new(1.0, 0.0), &[], si, 0.0, l).t;
            let a = 1.0 - (-4.0 * std::f64::consts::PI * si.im * 2.0 / l).exp();
            for c in 0..4 {
                assert!((r.qe[c][i] - t * a).abs() < 1e-9, "{l}: {} vs {}", r.qe[c][i], t * a);
            }
            let b = r.budget[i];
            let sum = b.reflected + b.color_filter + b.metal + b.other_layers + b.silicon_shallow + b.collected + b.silicon_deep;
            assert!((sum - 1.0).abs() < 1e-9, "{b:?}");
        }
    }

    #[test]
    fn bsi_pixel_has_colour_responses() {
        let s = PixelStack::from_toml(BSI).unwrap();
        let r = run(&s, 0.0);
        let (red, green, blue) = (0, 1, 3);
        // Each colour peaks in its band and the greens agree at normal incidence.
        assert!(at(&r, blue, 0.45) > at(&r, green, 0.45) && at(&r, blue, 0.45) > at(&r, red, 0.45));
        assert!(at(&r, green, 0.54) > at(&r, blue, 0.54) && at(&r, green, 0.54) > at(&r, red, 0.54));
        assert!(at(&r, red, 0.62) > at(&r, green, 0.62) && at(&r, red, 0.62) > at(&r, blue, 0.62));
        for (i, _) in r.wavelengths.iter().enumerate() {
            assert!((r.qe[1][i] - r.qe[2][i]).abs() < 0.01, "Gr / Gb at {}", r.wavelengths[i]);
        }
        // Peak QE of a good BSI pixel: 40 - 80 %.
        let peak = (0..4).map(|c| r.qe[c].iter().cloned().fold(0.0, f64::max)).fold(0.0, f64::max);
        assert!((0.4..0.85).contains(&peak), "peak QE {peak}");
        // Energy is accounted for at every wavelength.
        for b in &r.budget {
            let sum = b.reflected + b.color_filter + b.metal + b.other_layers + b.silicon_shallow + b.collected + b.silicon_deep;
            assert!((sum - 1.0).abs() < 1e-6, "{b:?}");
        }
    }

    #[test]
    fn chief_ray_angle_raises_crosstalk_and_lowers_qe() {
        let s = PixelStack::from_toml(BSI).unwrap();
        let r0 = run(&s, 0.0);
        let r25 = run(&s, 25.0);
        let i = r0.wavelengths.iter().position(|&x| (x - 0.54).abs() < 1e-9).unwrap();
        assert!(r25.crosstalk[1][i] > r0.crosstalk[1][i] + 0.01, "{} vs {}", r25.crosstalk[1][i], r0.crosstalk[1][i]);
        assert!(r25.qe[1][i] < r0.qe[1][i]);
        // The microlens focuses: removing it at normal incidence loses light to the grid.
        let mut flat = s.clone();
        flat.microlens = None;
        let rf = run(&flat, 0.0);
        assert!(rf.budget[i].metal > r0.budget[i].metal);
    }

    #[test]
    fn errors() {
        assert!(PixelStack::from_toml("pitch = 1\n").unwrap_err().contains("photodiode"));
        assert!(PixelStack::from_toml("pitch = 1\n[photodiode]\ntop = 2\nbottom = 1\n").unwrap_err().contains("top < bottom"));
        assert!(PixelStack::from_toml("pitch = 1\nfoo = 2\n[photodiode]\ntop = 0\nbottom = 1\n").is_err());
        let s = PixelStack::from_toml(
            "pitch = 1\n[[layers]]\nmaterial = \"nope\"\nthickness = 1\n[photodiode]\ntop = 0\nbottom = 1\n",
        )
        .unwrap();
        assert!(simulate(&s, &Settings::default()).unwrap_err().contains("unknown material 'nope'"));
    }
}
