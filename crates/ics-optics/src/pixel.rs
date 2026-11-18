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

use crate::carriers::{Collection, Grid as SiGrid, Silicon};
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
    /// Shift of every lens from its pixel's centre (µm), e.g. to sit over an off-centre
    /// photodiode.
    #[serde(default)]
    pub offset: [f64; 2],
    /// `photodiode`: centre each lens over the photodiode's area (sets `offset`).
    #[serde(default)]
    pub center: Option<String>,
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

/// Shapes of another material within a layer (metal lines of a front-side pixel), repeated in
/// every pixel: rectangles [x0, y0, x1, y1] in µm from the pixel's lower-left corner, or taken
/// from the layout (`layout`: a layer name or derived-layer expression, filled in by the caller).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pattern {
    pub material: String,
    #[serde(default)]
    pub layout: Option<String>,
    #[serde(default)]
    pub rects: Vec<[f64; 4]>,
}

/// Which pixel cell of which project the `layout` entries of a stack read.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutSource {
    /// Cell (`lib/cell` or `cell`).
    pub cell: String,
    /// Layer or expression whose bounding box is the pixel; default all shapes.
    #[serde(default)]
    pub boundary: Option<String>,
}

fn inside(rects: &[[f64; 4]], x: f64, y: f64, p: f64) -> bool {
    let (x, y) = (x.rem_euclid(p), y.rem_euclid(p));
    rects.iter().any(|r| x >= r[0] && x < r[2] && y >= r[1] && y < r[3])
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
    /// Shapes of another material (e.g. metal lines from the layout).
    #[serde(default)]
    pub pattern: Option<Pattern>,
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
    /// Lateral collection area per pixel (disjoint rectangles, µm from the pixel corner), or a
    /// layout layer / expression to take it from; default the whole pixel.
    #[serde(default)]
    pub layout: Option<String>,
    #[serde(default)]
    pub rects: Vec<[f64; 4]>,
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
    /// Pixel pitch (µm); 0 to take it from the layout.
    #[serde(default)]
    pub pitch: f64,
    /// The layout the `layout` entries read.
    #[serde(default)]
    pub layout: Option<LayoutSource>,
    pub microlens: Option<Microlens>,
    /// Thick layers below the microlens, top to bottom.
    #[serde(default)]
    pub layers: Vec<Layer>,
    /// Thin films on the silicon, top to bottom (coherent).
    #[serde(default)]
    pub arc: Vec<Film>,
    pub photodiode: Photodiode,
    /// Carrier diffusion in the silicon: with it, QE counts the carriers each photodiode
    /// collects (also from outside its depletion region) and electrical crosstalk is reported.
    #[serde(default)]
    pub silicon: Option<Silicon>,
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
    /// Read a stack. Stacks with `layout` entries come back unchecked: fill them with
    /// [`Self::fill_from_layout`].
    pub fn from_toml(text: &str) -> Result<PixelStack, String> {
        let mut s: PixelStack = toml::from_str(text).map_err(|e| e.to_string())?;
        if !s.needs_layout() {
            s.center_lenses()?;
            s.check()?;
        }
        Ok(s)
    }

    /// Whether some entry still waits for shapes from the layout (or the pitch does).
    pub fn needs_layout(&self) -> bool {
        (self.layout.is_some() && self.pitch == 0.0)
            || self.photodiode.layout.is_some() && self.photodiode.rects.is_empty()
            || self.layers.iter().any(|l| l.pattern.as_ref().is_some_and(|p| p.layout.is_some() && p.rects.is_empty()))
    }

    /// Fill the `layout` entries: `size` is the pixel frame (µm) and `rects` gives the
    /// rectangles of a layer or expression (µm from the frame's corner).
    pub fn fill_from_layout(
        &mut self,
        size: (f64, f64),
        rects: impl Fn(&str) -> Result<Vec<[f64; 4]>, String>,
    ) -> Result<(), String> {
        if self.pitch == 0.0 {
            if (size.0 - size.1).abs() > 1e-6 * size.0.max(1.0) {
                return Err(format!("the pixel frame is {} x {} µm; the model needs square pixels", size.0, size.1));
            }
            self.pitch = size.0;
        }
        if let Some(expr) = self.photodiode.layout.clone() {
            self.photodiode.rects = rects(&expr).map_err(|e| format!("photodiode '{expr}': {e}"))?;
            if self.photodiode.rects.is_empty() {
                return Err(format!("photodiode '{expr}' is empty in the layout"));
            }
        }
        for l in &mut self.layers {
            if let Some(p) = &mut l.pattern
                && let Some(expr) = p.layout.clone()
            {
                p.rects = rects(&expr).map_err(|e| format!("layer '{}' pattern '{expr}': {e}", l.name))?;
            }
        }
        self.layout = None;
        self.center_lenses()?;
        self.check()
    }

    /// Apply `center = "photodiode"`: the lens offset that puts it over the photodiode's
    /// area-weighted centre.
    fn center_lenses(&mut self) -> Result<(), String> {
        let p = self.pitch;
        let pd = self.photodiode.rects.clone();
        if let Some(ml) = &mut self.microlens
            && let Some(c) = &ml.center
        {
            if !c.eq_ignore_ascii_case("photodiode") {
                return Err(format!("microlens center '{c}': only \"photodiode\" is known"));
            }
            if pd.is_empty() {
                return Err("microlens center = \"photodiode\" needs the photodiode's area (rects or layout)".into());
            }
            let area: f64 = pd.iter().map(|r| (r[2] - r[0]) * (r[3] - r[1])).sum();
            let cx = pd.iter().map(|r| (r[2] - r[0]) * (r[3] - r[1]) * (r[0] + r[2]) / 2.0).sum::<f64>() / area;
            let cy = pd.iter().map(|r| (r[2] - r[0]) * (r[3] - r[1]) * (r[1] + r[3]) / 2.0).sum::<f64>() / area;
            ml.offset = [cx - p / 2.0, cy - p / 2.0];
            ml.center = None;
        }
        Ok(())
    }

    // `!(x > 0)` also rejects NaN.
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    fn check(&self) -> Result<(), String> {
        if self.needs_layout() {
            return Err(match &self.layout {
                Some(l) => format!("the stack reads the layout of {}; fill it from a project first", l.cell),
                None => "a `layout = ...` entry needs a [layout] section naming the pixel cell".into(),
            });
        }
        if !(self.pitch > 0.0) {
            return Err("pitch must be positive (or taken from a [layout] cell)".into());
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
    /// Absorbed at the photodiode's depth but outside its lateral area.
    pub silicon_outside: f64,
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
    /// Electrical crosstalk per wavelength and colour (with a `[silicon]` model): of the
    /// carriers generated under a pixel that are collected, the fraction another pixel collects.
    pub crosstalk_electrical: [Vec<f64>; 4],
    pub budget: Vec<Budget>,
    /// Rays per Bayer cell (ray model).
    pub rays: usize,
    /// `ray` or `rcwa`. RCWA gives no per-lens crosstalk (zeros) and lumps the absorption above
    /// the silicon into `other_layers`.
    pub method: String,
}

/// Solver of the pixel model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Method {
    /// Geometric rays with thin-film optics on the silicon (fast, no diffraction).
    Ray,
    /// Rigorous coupled-wave analysis over the Bayer cell with this many harmonics per
    /// direction (odd); the microlens is staircased.
    Rcwa { harmonics: usize },
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
    pub method: Method,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            wavelengths: (0..=62).map(|i| 0.38 + i as f64 * 0.01).collect(),
            cra: 0.0,
            azimuth: 0.0,
            rays_per_side: 40,
            method: Method::Ray,
        }
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
type LayerOptics = (Complex64, Option<[Complex64; 3]>, Option<(Complex64, f64)>, f64, Option<(Complex64, Vec<[f64; 4]>)>);

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
    let cx = (i as f64 + 0.5) * p + ml.offset[0];
    let cy = (j as f64 + 0.5) * p + ml.offset[1];
    ([cx, cy, top + ml.height - rc], rc)
}

/// Run the pixel model.
pub fn simulate(stack: &PixelStack, settings: &Settings) -> Result<PixelResult, String> {
    stack.check()?;
    if let Method::Rcwa { harmonics } = settings.method {
        return simulate_rcwa(stack, settings, harmonics);
    }
    let mats = stack.materials()?;
    let p = stack.pitch;
    let n_side = settings.rays_per_side.max(2);
    let lens_base: f64 =
        stack.layers.iter().map(|l| l.thickness).sum::<f64>() + stack.arc.iter().map(|f| f.thickness).sum::<f64>();
    let arc_top: f64 = stack.arc.iter().map(|f| f.thickness).sum();
    let (cra, az) = (settings.cra.to_radians(), settings.azimuth.to_radians());
    let d0: V3 = [cra.sin() * az.cos(), cra.sin() * az.sin(), -cra.cos()];

    let coll = match &stack.silicon {
        Some(si) => Some(Collection::solve(si, p, &stack.photodiode.rects, (stack.photodiode.top, stack.photodiode.bottom))?),
        None => None,
    };
    let mut qe: [Vec<f64>; 4] = Default::default();
    let mut xt: [Vec<f64>; 4] = Default::default();
    let mut xte: [Vec<f64>; 4] = Default::default();
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
            let pattern = match &l.pattern {
                Some(pt) => Some((idx(&pt.material)?, pt.rects.clone())),
                None => None,
            };
            layers.push((base, filters, grid, l.thickness, pattern));
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
        let mut genv = coll.as_ref().map(|c| (c.grid.clone(), vec![0.0; c.grid.len()]));
        for iy in 0..2 * n_side {
            for ix in 0..2 * n_side {
                let x0 = (ix as f64 + 0.5) / n_side as f64 * p;
                let y0 = (iy as f64 + 0.5) / n_side as f64 * p;
                let g = genv.as_mut().map(|(g, v)| (&*g, v.as_mut_slice()));
                trace(stack, &optics, lambda, [x0, y0], d0, lens_base, arc_top, w, &mut collected, &mut entered, &mut b, g);
            }
        }
        // QE per pixel: the cell holds one pixel of each colour, illuminated with 4 pixels' light.
        match (&coll, &genv) {
            (Some(c), Some((_, g))) => {
                let (q, x, got, total) = collect(c, g);
                b.collected = got;
                b.silicon_outside = total - got;
                for k in 0..4 {
                    qe[k].push(q[k]);
                    xte[k].push(x[k]);
                }
            }
            _ => {
                for k in 0..4 {
                    qe[k].push(collected[k] * 4.0);
                    xte[k].push(0.0);
                }
            }
        }
        for c in 0..4 {
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
        crosstalk_electrical: xte,
        budget,
        rays: (2 * n_side) * (2 * n_side),
        method: "ray".into(),
    })
}

/// QE per colour, light budget and electrical crosstalk per colour at one wavelength.
type WavelengthResult = ([f64; 4], Budget, [f64; 4]);

/// Steps of the staircased microlens in the RCWA model.
const LENS_SLICES: usize = 8;
/// Permittivity samples per pixel side in the RCWA model.
const SAMPLES_PER_PIXEL: usize = 32;

/// The RCWA model: the Bayer cell as one period.
fn simulate_rcwa(stack: &PixelStack, settings: &Settings, harmonics: usize) -> Result<PixelResult, String> {
    use crate::rcwa::{Eps, Layer, Problem};
    let mats = stack.materials()?;
    let p = stack.pitch;
    let h = harmonics.max(1) | 1;
    let ns = 2 * SAMPLES_PER_PIXEL;
    let cell = 2.0 * p;
    let xy = |i: usize, j: usize| ((i as f64 + 0.5) * cell / ns as f64, (j as f64 + 0.5) * cell / ns as f64);
    // Check every material once up front, so the per-wavelength work cannot fail.
    for name in [stack.ambient.as_str(), "si", &stack.color_filters.red, &stack.color_filters.green, &stack.color_filters.blue] {
        mats.get(name)?;
    }
    for l in &stack.layers {
        if l.kind == LayerKind::Uniform {
            mats.get(&l.material)?;
        }
        if let Some(g) = &l.grid {
            mats.get(&g.material)?;
        }
        if let Some(pt) = &l.pattern {
            mats.get(&pt.material)?;
        }
    }
    for f in &stack.arc {
        mats.get(&f.material)?;
    }
    if let Some(m) = &stack.microlens {
        mats.get(&m.material)?;
    }

    let problem = |lambda: f64| -> Problem {
        let idx = |name: &str| mats.get(name).map(|m| m.index(lambda)).unwrap();
        let eps = |n: Complex64| n * n;
        let ambient = idx(&stack.ambient);
        let cf = [idx(&stack.color_filters.red), idx(&stack.color_filters.green), idx(&stack.color_filters.blue)];
        let mut layers = vec![];
        if let Some(ml) = &stack.microlens {
            let n_lens = idx(&ml.material);
            let a = ml.radius.unwrap_or(p / std::f64::consts::SQRT_2);
            let rc = (a * a + ml.height * ml.height) / (2.0 * ml.height);
            for k in 0..LENS_SLICES {
                // Height of the slice's middle above the lens base.
                let zm = ml.height * (1.0 - (k as f64 + 0.5) / LENS_SLICES as f64);
                let r = (rc * rc - (zm - (ml.height - rc)).powi(2)).max(0.0).sqrt();
                let samples = (0..ns * ns)
                    .map(|s| {
                        let (x, y) = xy(s % ns, s / ns);
                        let [ox, oy] = ml.offset;
                        let (cx, cy) = ((((x - ox) / p).floor() + 0.5) * p + ox, (((y - oy) / p).floor() + 0.5) * p + oy);
                        eps(if (x - cx).hypot(y - cy) <= r { n_lens } else { ambient })
                    })
                    .collect();
                layers.push(Layer { thickness: ml.height / LENS_SLICES as f64, eps: Eps::Grid { nx: ns, ny: ns, samples } });
            }
        }
        for l in &stack.layers {
            let base = match l.kind {
                LayerKind::ColorFilter => None,
                LayerKind::Uniform => Some(idx(&l.material)),
            };
            let grid = l.grid.as_ref().map(|g| (idx(&g.material), g.width));
            let pattern = l.pattern.as_ref().map(|pt| (idx(&pt.material), &pt.rects));
            let e = match (base, grid, pattern) {
                (Some(n), None, None) => Eps::Uniform(eps(n)),
                _ => Eps::Grid {
                    nx: ns,
                    ny: ns,
                    samples: (0..ns * ns)
                        .map(|s| {
                            let (x, y) = xy(s % ns, s / ns);
                            if let Some((nm, w)) = grid
                                && on_grid(x, y, p, w)
                            {
                                return eps(nm);
                            }
                            if let Some((nm, rects)) = pattern
                                && inside(rects, x, y, p)
                            {
                                return eps(nm);
                            }
                            let (i, j) = pixel_of(x, y, p);
                            eps(base.unwrap_or_else(|| cf[[0, 1, 1, 2][PixelStack::color(i, j)]]))
                        })
                        .collect(),
                },
            };
            layers.push(Layer { thickness: l.thickness, eps: e });
        }
        for f in &stack.arc {
            layers.push(Layer { thickness: f.thickness, eps: Eps::Uniform(eps(idx(&f.material))) });
        }
        Problem {
            period: (cell, cell),
            harmonics: (h, h),
            n_in: ambient,
            layers,
            n_sub: idx("si"),
            wavelength: lambda,
            theta: settings.cra.to_radians(),
            phi: settings.azimuth.to_radians(),
        }
    };

    let pd = &stack.photodiode;
    let coll = match &stack.silicon {
        Some(si) => Some(Collection::solve(si, p, &pd.rects, (pd.top, pd.bottom))?),
        None => None,
    };
    let one = |lambda: f64| -> ([f64; 4], Budget, [f64; 4]) {
        let sols = problem(lambda).solve();
        let mut qe = [0.0; 4];
        let mut b = Budget::default();
        let w = 1.0 / sols.len() as f64;
        if let Some(c) = &coll {
            let mut genv = vec![0.0; c.grid.len()];
            let whole = (0.0, cell);
            let depth = c.grid.nz as f64 * c.grid.dz;
            for s in &sols {
                s.add_generation(&c.grid, &mut genv, w);
                b.reflected += s.r * w;
                b.silicon_deep += s.absorbed(whole, whole, (depth, f64::INFINITY)) * w;
                let total = s.absorbed(whole, whole, (0.0, f64::INFINITY));
                b.other_layers += (1.0 - s.r - total).max(0.0) * w;
            }
            let (q, x, got, total) = collect(c, &genv);
            b.collected = got;
            b.silicon_outside = (total - got).max(0.0);
            return (q, b, x);
        }
        for s in &sols {
            let whole_pixel = [[0.0, 0.0, p, p]];
            let area: &[[f64; 4]] = if pd.rects.is_empty() { &whole_pixel } else { &pd.rects };
            let mut got = 0.0;
            for (i, j) in [(0usize, 0usize), (1, 0), (0, 1), (1, 1)] {
                let (ox, oy) = (i as f64 * p, j as f64 * p);
                let a: f64 =
                    area.iter().map(|r| s.absorbed((ox + r[0], ox + r[2]), (oy + r[1], oy + r[3]), (pd.top, pd.bottom))).sum();
                qe[PixelStack::color(i as i64, j as i64)] += 4.0 * a * w;
                b.collected += a * w;
                got += a;
            }
            let whole = (0.0, cell);
            let at_depth = s.absorbed(whole, whole, (pd.top, pd.bottom));
            b.silicon_outside += (at_depth - got).max(0.0) * w;
            let whole = (0.0, cell);
            let total = s.absorbed(whole, whole, (0.0, f64::INFINITY));
            b.reflected += s.r * w;
            b.silicon_shallow += s.absorbed(whole, whole, (0.0, pd.top)) * w;
            b.silicon_deep += s.absorbed(whole, whole, (pd.bottom, f64::INFINITY)) * w;
            b.other_layers += (1.0 - s.r - total).max(0.0) * w;
        }
        (qe, b, [0.0; 4])
    };

    // Wavelengths in parallel; the linear algebra itself stays sequential.
    faer::set_global_parallelism(faer::Par::Seq);
    let wl = &settings.wavelengths;
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(wl.len().max(1));
    let mut out: Vec<Option<WavelengthResult>> = vec![None; wl.len()];
    std::thread::scope(|sc| {
        let chunks: Vec<_> = out.chunks_mut(wl.len().div_ceil(threads).max(1)).enumerate().collect();
        let size = wl.len().div_ceil(threads).max(1);
        for (c, chunk) in chunks {
            let one = &one;
            sc.spawn(move || {
                for (k, slot) in chunk.iter_mut().enumerate() {
                    *slot = Some(one(wl[c * size + k]));
                }
            });
        }
    });
    let mut qe: [Vec<f64>; 4] = Default::default();
    let mut xte: [Vec<f64>; 4] = Default::default();
    let mut budget = vec![];
    for (q, b, x) in out.into_iter().map(Option::unwrap) {
        for c in 0..4 {
            qe[c].push(q[c]);
            xte[c].push(x[c]);
        }
        budget.push(b);
    }
    Ok(PixelResult {
        name: stack.name.clone(),
        cra: settings.cra,
        azimuth: settings.azimuth,
        wavelengths: wl.clone(),
        qe,
        crosstalk: std::array::from_fn(|_| vec![0.0; wl.len()]),
        crosstalk_electrical: xte,
        budget,
        rays: 0,
        method: "rcwa".into(),
    })
}

/// QE per colour, electrical crosstalk per colour, carriers collected and generated in the grid,
/// from generation `genv` (fractions of the light incident on the cell per voxel).
fn collect(c: &Collection, genv: &[f64]) -> ([f64; 4], [f64; 4], f64, f64) {
    let g = &c.grid;
    let half = g.nx / 2;
    let mut qe = [0.0; 4];
    // Per source pixel (column): carriers collected by it and by the others.
    let mut col = [[0.0; 2]; 4];
    let mut total = 0.0;
    for (v, &gv) in genv.iter().enumerate() {
        if gv == 0.0 {
            continue;
        }
        total += gv;
        let (i, j) = (v % g.nx, (v / g.nx) % g.ny);
        let src = PixelStack::color((i / half) as i64, (j / half) as i64);
        for (pi, pj) in [(0usize, 0usize), (1, 0), (0, 1), (1, 1)] {
            let got = gv * c.prob(pi, pj, v);
            let dst = PixelStack::color(pi as i64, pj as i64);
            qe[dst] += got;
            col[src][0] += got;
            if dst != src {
                col[src][1] += got;
            }
        }
    }
    let collected: f64 = qe.iter().sum();
    let xt = std::array::from_fn(|k| if col[k][0] > 0.0 { col[k][1] / col[k][0] } else { 0.0 });
    (qe.map(|q| 4.0 * q), xt, collected, total)
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
    genv: Option<(&SiGrid, &mut [f64])>,
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
            let (pi, pj) = pixel_of(start[0] - ml.offset[0], start[1] - ml.offset[1], p);
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
                    if hit[2] < lens_base || pixel_of(hit[0] - ml.offset[0], hit[1] - ml.offset[1], p) != (i, j) {
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
    for (k, (base, filters, grid, thickness, pattern)) in o.layers.iter().enumerate() {
        let (base, filters, grid, thickness) = (*base, *filters, *grid, *thickness);
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
            if let Some((_, rects)) = pattern
                && inside(rects, mid[0], mid[1], p)
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
    let lens_color = PixelStack::color(lens_pixel.0, lens_pixel.1);
    if let Some((g, cells)) = genv {
        // Carrier model: deposit the generation in the voxels it falls in, to the epi bottom;
        // collection is computed afterwards. Optical crosstalk counts, as without the carrier
        // model, the generation over the photodiode depth that lands under another pixel.
        let depth = g.nz as f64 * g.dz;
        for k in 0..g.nz {
            let (za, zb) = (k as f64 * g.dz, (k + 1) as f64 * g.dz);
            let zm = 0.5 * (za + zb);
            let (x, y) = (pos[0] + dsi[0] * zm * path_per_depth, pos[1] + dsi[1] * zm * path_per_depth);
            let dep = power * generated(za, zb);
            if let Some(v) = g.voxel(x, y, zm) {
                cells[v] += dep;
            }
            if zm >= pd.top && zm <= pd.bottom {
                entered[lens_color][0] += dep;
                if pixel_of(x, y, p) != lens_pixel {
                    entered[lens_color][1] += dep;
                }
            }
        }
        b.silicon_deep += power * (-alpha * depth * path_per_depth).exp();
        return;
    }
    b.silicon_shallow += power * generated(0.0, pd.top);
    b.silicon_deep += power * (-alpha * pd.bottom * path_per_depth).exp();
    const STEPS: usize = 48;
    for s in 0..STEPS {
        let za = pd.top + (pd.bottom - pd.top) * s as f64 / STEPS as f64;
        let zb = pd.top + (pd.bottom - pd.top) * (s + 1) as f64 / STEPS as f64;
        let zm = 0.5 * (za + zb);
        let x = pos[0] + dsi[0] * zm * path_per_depth;
        let y = pos[1] + dsi[1] * zm * path_per_depth;
        let g = power * generated(za, zb);
        if !pd.rects.is_empty() && !inside(&pd.rects, x, y, p) {
            b.silicon_outside += g;
            continue;
        }
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
            let sum = b.reflected
                + b.color_filter
                + b.metal
                + b.other_layers
                + b.silicon_shallow
                + b.collected
                + b.silicon_outside
                + b.silicon_deep;
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
            let sum = b.reflected
                + b.color_filter
                + b.metal
                + b.other_layers
                + b.silicon_shallow
                + b.collected
                + b.silicon_outside
                + b.silicon_deep;
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
        // The carrier model keeps the optical crosstalk of the ray model.
        let mut si = s.clone();
        si.silicon = Some(Silicon { diffusion_length: 0.01, epi: Some(4.0), grid: Some(0.1), ..Silicon::default() });
        let (a, b) = (run(&si, 25.0).crosstalk[1][i], r25.crosstalk[1][i]);
        assert!(a > 0.01 && (a - b).abs() < 0.2 * b, "with silicon {a} vs {b}");
    }

    #[test]
    fn rcwa_agrees_with_thin_film_optics_without_patterns() {
        // Only anti-reflection films on silicon: both solvers reduce to thin-film optics.
        let s = PixelStack::from_toml(
            "pitch = 1.0\n[[arc]]\nmaterial = \"ta2o5\"\nthickness = 0.055\n[[arc]]\nmaterial = \"hfo2\"\nthickness = 0.01\n\
             [photodiode]\ntop = 0.05\nbottom = 2.0\n",
        )
        .unwrap();
        for cra in [0.0, 20.0] {
            let wl = vec![0.45, 0.55, 0.65];
            let ray = simulate(&s, &Settings { wavelengths: wl.clone(), cra, rays_per_side: 4, ..Settings::default() }).unwrap();
            let rc =
                simulate(&s, &Settings { wavelengths: wl, cra, method: Method::Rcwa { harmonics: 3 }, ..Settings::default() })
                    .unwrap();
            for c in 0..4 {
                for i in 0..3 {
                    assert!((ray.qe[c][i] - rc.qe[c][i]).abs() < 1e-6, "CRA {cra}: {} vs {}", ray.qe[c][i], rc.qe[c][i]);
                }
            }
        }
    }

    #[test]
    fn rcwa_bsi_pixel_converges_and_balances() {
        let s = PixelStack::from_toml(BSI).unwrap();
        let wl = vec![0.45, 0.55, 0.63];
        let r = |h| {
            simulate(&s, &Settings { wavelengths: wl.clone(), method: Method::Rcwa { harmonics: h }, ..Settings::default() })
                .unwrap()
        };
        let (r9, r13) = (r(9), r(13));
        assert_eq!(r13.method, "rcwa");
        // Colours where they belong, close between harmonics, energy balanced.
        assert!(r13.qe[3][0] > 0.4 && r13.qe[1][1] > 0.4 && r13.qe[0][2] > 0.3, "{:?}", r13.qe);
        for c in 0..4 {
            for i in 0..3 {
                assert!((r9.qe[c][i] - r13.qe[c][i]).abs() < 0.05, "{c} {i}: {} vs {}", r9.qe[c][i], r13.qe[c][i]);
            }
        }
        for b in &r13.budget {
            let sum = b.reflected + b.other_layers + b.silicon_shallow + b.collected + b.silicon_outside + b.silicon_deep;
            assert!((sum - 1.0).abs() < 1e-3, "{b:?}");
        }
    }

    #[test]
    fn patterns_and_photodiode_area() {
        // Flat stack, no lens: a metal plate over half of each pixel halves the light, and a
        // photodiode over a quarter of the pixel collects a quarter.
        let base = "pitch = 2.0\n[[layers]]\nname = \"m1\"\nmaterial = \"sio2\"\nthickness = 0.5\n\
                    [photodiode]\ntop = 0.0\nbottom = 3.0\n";
        let wl = vec![0.55];
        let run = |s: &PixelStack, method| {
            simulate(s, &Settings { wavelengths: wl.clone(), rays_per_side: 20, method, ..Settings::default() }).unwrap()
        };
        let open = PixelStack::from_toml(base).unwrap();
        let mut half = open.clone();
        half.layers[0].pattern = Some(Pattern { material: "al".into(), layout: None, rects: vec![[0.0, 0.0, 1.0, 2.0]] });
        let mut quarter = open.clone();
        quarter.photodiode.rects = vec![[0.5, 0.5, 1.5, 1.5]];
        let (o, h, q) = (run(&open, Method::Ray), run(&half, Method::Ray), run(&quarter, Method::Ray));
        assert!((h.qe[0][0] / o.qe[0][0] - 0.5).abs() < 0.02, "{} {}", h.qe[0][0], o.qe[0][0]);
        assert!((q.qe[0][0] / o.qe[0][0] - 0.25).abs() < 0.02, "{} {}", q.qe[0][0], o.qe[0][0]);
        assert!(h.budget[0].metal > 0.45 && q.budget[0].silicon_outside > 0.3);
        // RCWA sees the same shapes (with diffraction, so only roughly the same numbers).
        // (11 harmonics resolve the 1 µm opening of the 4 µm Bayer cell only roughly; it
        // converges to the ray result: 0.27, 0.30, 0.32 at 11, 15, 21 harmonics.)
        let rc = Method::Rcwa { harmonics: 11 };
        let (oh, hh, qh) = (run(&open, rc), run(&half, rc), run(&quarter, rc));
        assert!((hh.qe[0][0] / oh.qe[0][0] - 0.5).abs() < 0.15, "{} {}", hh.qe[0][0], oh.qe[0][0]);
        assert!((qh.qe[0][0] / oh.qe[0][0] - 0.25).abs() < 0.1, "{} {}", qh.qe[0][0], oh.qe[0][0]);
        let b = qh.budget[0];
        let sum = b.reflected + b.other_layers + b.silicon_shallow + b.collected + b.silicon_outside + b.silicon_deep;
        assert!((sum - 1.0).abs() < 1e-3, "{b:?}");
    }

    #[test]
    fn stacks_read_from_a_layout() {
        let text = "[layout]\ncell = \"px\"\n[[layers]]\nname = \"m1\"\nmaterial = \"sio2\"\nthickness = 0.5\n\
                    pattern = { material = \"al\", layout = \"metal1\" }\n[photodiode]\ntop = 0\nbottom = 3\nlayout = \"pd\"\n";
        let mut s = PixelStack::from_toml(text).unwrap();
        assert!(s.needs_layout());
        assert!(simulate(&s, &Settings::default()).unwrap_err().contains("fill it from a project"));
        s.fill_from_layout((4.0, 4.0), |e| Ok(if e == "pd" { vec![[1.0, 1.0, 3.0, 3.0]] } else { vec![[0.0, 0.0, 4.0, 0.5]] }))
            .unwrap();
        // A lens centred over an off-centre photodiode collects more of the light.
        let off =
            "pitch = 4.0\n[microlens]\nheight = 1.2\nCENTER\n[photodiode]\ntop = 0\nbottom = 3\nrects = [[0.2, 0.2, 1.4, 1.4]]\n";
        let plain = PixelStack::from_toml(&off.replace("CENTER", "")).unwrap();
        let shifted = PixelStack::from_toml(&off.replace("CENTER", "center = \"photodiode\"")).unwrap();
        let o = shifted.microlens.as_ref().unwrap().offset;
        assert!((o[0] + 1.2).abs() < 1e-12 && (o[1] + 1.2).abs() < 1e-12, "{o:?}");
        let q = |st: &PixelStack, m| {
            simulate(st, &Settings { wavelengths: vec![0.55], rays_per_side: 24, method: m, ..Settings::default() }).unwrap().qe
                [0][0]
        };
        assert!(
            q(&shifted, Method::Ray) > 1.1 * q(&plain, Method::Ray),
            "{} vs {}",
            q(&shifted, Method::Ray),
            q(&plain, Method::Ray)
        );
        assert!(q(&shifted, Method::Rcwa { harmonics: 9 }) > q(&plain, Method::Rcwa { harmonics: 9 }));
        assert_eq!((s.pitch, s.photodiode.rects.len()), (4.0, 1));
        assert_eq!(s.layers[0].pattern.as_ref().unwrap().rects, vec![[0.0, 0.0, 4.0, 0.5]]);
        assert!(simulate(&s, &Settings { wavelengths: vec![0.55], rays_per_side: 8, ..Settings::default() }).is_ok());
        let mut t = PixelStack::from_toml(text).unwrap();
        assert!(t.fill_from_layout((4.0, 3.0), |_| Ok(vec![])).unwrap_err().contains("square"));
    }

    #[test]
    fn carrier_collection() {
        // A small photodiode in a BSI-like stack.
        let base = PixelStack::from_toml(&BSI.replace(
            "[photodiode]\ntop = 0.05\nbottom = 3.0",
            "[photodiode]\ntop = 0.1\nbottom = 1.5\nrects = [[0.2, 0.2, 1.2, 1.2]]",
        ))
        .unwrap();
        let wl = vec![0.45, 0.55, 0.65, 0.85];
        let run = |st: &PixelStack, m| {
            simulate(st, &Settings { wavelengths: wl.clone(), rays_per_side: 20, method: m, ..Settings::default() }).unwrap()
        };
        let with = |l: f64, dti: bool| {
            let mut st = base.clone();
            st.silicon = Some(Silicon {
                diffusion_length: l,
                epi: Some(4.0),
                // On the photodiode's edges, so that the voxels match the box model's volume.
                grid: Some(0.1),
                dti: dti.then_some(crate::carriers::Dti { depth: 4.0, width: 0.1 }),
                ..Silicon::default()
            });
            st
        };
        for m in [Method::Ray, Method::Rcwa { harmonics: 7 }] {
            let boxed = run(&base, m);
            // A very short diffusion length collects only the depletion region: the box model.
            let short = run(&with(0.01, false), m);
            for c in 0..4 {
                for i in 0..wl.len() {
                    let (a, b) = (short.qe[c][i], boxed.qe[c][i]);
                    assert!((a - b).abs() < 0.03 * b.max(0.05), "{m:?} c{c} {}: {a} vs {b}", wl[i]);
                }
            }
            // Diffusion adds carriers from outside the depletion region, and spills them into
            // neighbours at long wavelengths; trenches keep them home.
            let long = run(&with(30.0, false), m);
            let trench = run(&with(30.0, true), m);
            assert!(long.qe[1][1] > short.qe[1][1] * 1.05, "{m:?}: {} vs {}", long.qe[1][1], short.qe[1][1]);
            assert!(long.crosstalk_electrical[1][3] > 0.05, "{m:?}: {:?}", long.crosstalk_electrical[1]);
            assert!(trench.crosstalk_electrical[1][3] < 0.5 * long.crosstalk_electrical[1][3], "{m:?}");
            for b in &long.budget {
                let sum = b.reflected
                    + b.color_filter
                    + b.metal
                    + b.other_layers
                    + b.silicon_shallow
                    + b.collected
                    + b.silicon_outside
                    + b.silicon_deep;
                assert!((sum - 1.0).abs() < 2e-3, "{m:?} {b:?}");
            }
        }
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
