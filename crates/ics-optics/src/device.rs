//! Semiconductor device model of a pixel's silicon: doping from implants, the electrostatic
//! potential from Poisson's equation, electron transport by drift and diffusion, the
//! photodiodes' pinning voltage and full well, and the charge transfer through a transfer gate.
//!
//! The domain is the 2 × 2 Bayer cell of the carrier model ([`crate::carriers::Grid`]: periodic
//! sides, depth from the illuminated surface), cut by the deep trenches (no field or current
//! crosses a trench). Holes stay in equilibrium with the grounded p regions (quasi-Fermi level
//! 0), electrons follow their quasi-Fermi level: the voltage of an n+ contact, or the fill level
//! of a photodiode.
//!
//! - **Poisson** (finite volumes, damped Newton): ∇·(ε∇ψ) = -q (p - n + N_D - N_A), with
//!   n = n_i exp((ψ - φn) / V_T), p = n_i exp(-ψ / V_T). Gates couple through their oxide
//!   (ψ_gate = V_g + work-function offset); n+ contacts fix ψ; free surfaces have no field.
//! - **Collection** (by reciprocity): the probability P_k(r) that an electron generated at r ends
//!   in photodiode k solves the adjoint of the electron continuity equation with
//!   Scharfetter-Gummel fluxes, so carriers drift down the potential into the wells as well as
//!   diffuse; SRH recombination (doping-dependent lifetime) and surface recombination lose
//!   them; n+ contacts (floating diffusions, sources and drains) collect them too, for nobody's
//!   signal. P_k = 1 in photodiode k's well and 0 in the others'. The mean time an electron
//!   takes to be collected follows from the same operator (first-passage time).
//! - **Pinning voltage**: the potential at the bottom of an empty, fully depleted well.
//! - **Full well**: the electrons a photodiode holds when its quasi-Fermi level reaches the
//!   lowest barrier to any other sink (the transfer gate's, an n+ region's, a neighbour's).
//! - **Charge transfer**: a full photodiode, then its transfer gate on and off again: Poisson and
//!   the electron continuity equation in time (implicit Euler, Gummel iteration with frozen
//!   quasi-Fermi levels). The electrons left in the well are the lag.
//!
//! Units inside: µm, V, s; densities per µm³ (input in cm⁻³).

use serde::{Deserialize, Serialize};

use crate::carriers::{Grid, Silicon};

/// Thermal voltage at `t` kelvin.
fn thermal_voltage(t: f64) -> f64 {
    8.617_333e-5 * t
}

/// q / ε_Si in V·µm (densities per µm³).
const Q_EPS: f64 = 1.602_176_634e-19 / (11.7 * 8.854_187_8e-14) * 1e4;
/// Relative permittivity of silicon and of the gate oxide.
const EPS_SI: f64 = 11.7;
const EPS_OX: f64 = 3.9;
/// Electron charge (C).
const Q: f64 = 1.602_176_634e-19;
/// cm⁻³ → µm⁻³.
const PER_CM3: f64 = 1e-12;

/// The `[device]` section of a pixel stack.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    /// Kelvin.
    #[serde(default = "t300")]
    pub temperature: f64,
    /// Uniform doping of the silicon (the epitaxial layer).
    #[serde(default)]
    pub background: Background,
    #[serde(default)]
    pub doping: Vec<Implant>,
    #[serde(default)]
    pub gate: Vec<Gate>,
    /// Electron SRH lifetime in lightly doped silicon (s); it falls as τ / (1 + N / 5e16).
    #[serde(default = "lifetime")]
    pub lifetime: f64,
    /// Surface recombination velocity (cm/s) on the illuminated surface, the trench walls and
    /// the far surface outside contacts.
    #[serde(default)]
    pub surface_recombination: f64,
    /// Grid step (µm); default the `[silicon]` grid.
    #[serde(default)]
    pub grid: Option<f64>,
    /// Simulate the charge transfer of one pixel.
    #[serde(default)]
    pub transfer: Option<Transfer>,
}

fn t300() -> f64 {
    300.0
}

fn lifetime() -> f64 {
    1e-6
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Background {
    #[serde(rename = "type", default = "p_type")]
    pub kind: String,
    /// cm⁻³
    #[serde(default = "epi_doping")]
    pub concentration: f64,
}

fn p_type() -> String {
    "p".into()
}

fn epi_doping() -> f64 {
    1e15
}

impl Default for Background {
    fn default() -> Self {
        Background { kind: p_type(), concentration: epi_doping() }
    }
}

/// An implant: Gaussian in depth below the surface it enters from, with a Gaussian lateral
/// spread at the mask's edges.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Implant {
    #[serde(default)]
    pub name: String,
    /// `n` (donors) or `p` (acceptors).
    #[serde(rename = "type")]
    pub kind: String,
    /// Peak concentration (cm⁻³).
    pub peak: f64,
    /// Depth of the peak below its surface (µm).
    #[serde(default)]
    pub depth: f64,
    /// Vertical standard deviation (µm).
    #[serde(default = "straggle")]
    pub straggle: f64,
    /// Lateral standard deviation at the mask's edges (µm).
    #[serde(default = "lateral")]
    pub lateral: f64,
    /// `front` (default: the transistors' side, the far side of a back-side illuminated
    /// silicon), `back` (the illuminated surface of a back-side illuminated silicon), or `dti`:
    /// along the trench walls, `depth` being the distance from the wall.
    #[serde(default = "front")]
    pub side: String,
    /// Mask: a layout layer or expression (filled in from the layout), or rectangles in µm
    /// (pixel coordinates, or Bayer-cell coordinates for a stack read from a 2 × 2 cell). No
    /// mask: everywhere.
    #[serde(default)]
    pub layout: Option<String>,
    #[serde(default)]
    pub rects: Vec<[f64; 4]>,
    /// The photodiodes' storage wells: one per pixel, where this implant is the main donor.
    #[serde(default)]
    pub photodiode: bool,
    /// An ohmic n+ contact at this voltage where this implant is the main donor (floating
    /// diffusions after reset, sources and drains).
    #[serde(default)]
    pub contact: Option<f64>,
}

fn straggle() -> f64 {
    0.05
}

fn lateral() -> f64 {
    0.05
}

fn front() -> String {
    "front".into()
}

/// A gate on the front surface (a transfer gate).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gate {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub layout: Option<String>,
    #[serde(default)]
    pub rects: Vec<[f64; 4]>,
    /// Gate oxide thickness (µm).
    #[serde(default = "tox")]
    pub oxide: f64,
    /// Voltage while integrating and while transferring.
    #[serde(default)]
    pub off: f64,
    #[serde(default = "v_on")]
    pub on: f64,
    /// Electrostatic potential of the gate over its Fermi level (V): 0.56 for n+ poly.
    #[serde(default = "npoly")]
    pub work_function: f64,
}

fn tox() -> f64 {
    0.007
}

fn v_on() -> f64 {
    2.8
}

fn npoly() -> f64 {
    0.56
}

/// Charge transfer simulation settings.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transfer {
    /// Pixel whose gates pulse: `R`, `Gr`, `Gb` or `B`.
    #[serde(default = "gr")]
    pub pixel: String,
    /// Gate on time and the settling time after it (ns).
    #[serde(default = "on_time")]
    pub on_time: f64,
    #[serde(default = "settle")]
    pub settle: f64,
    /// Electrons in the photodiode at the start; default its full well.
    #[serde(default)]
    pub electrons: Option<f64>,
}

fn gr() -> String {
    "Gr".into()
}

fn on_time() -> f64 {
    20.0
}

fn settle() -> f64 {
    10.0
}

/// Geometry of the silicon the device model needs from the stack.
#[derive(Debug, Clone)]
pub struct SiliconGeometry {
    pub pitch: f64,
    /// Silicon depth modelled (µm): the thickness of a thinned silicon, else the epi.
    pub depth: f64,
    /// The far side is the front (transistor) surface at `depth`; else the illuminated surface
    /// is the front.
    pub back_illuminated: bool,
    /// Trench depth range (from the illuminated surface) and width.
    pub dti: Option<((f64, f64), f64)>,
    /// Rectangles of masks are in Bayer-cell coordinates.
    pub cell_rects: bool,
}

/// Per-pixel results.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PixelDevice {
    /// R, Gr, Gb or B.
    pub color: String,
    /// Potential at the bottom of the empty well (V).
    pub pinning_voltage: f64,
    /// Electrons at full well, and the quasi-Fermi level (V) and barrier potential there.
    pub full_well: f64,
    pub full_well_level: f64,
    /// Lowest barrier of the empty well to another sink (V), and where it leads.
    pub barrier: f64,
    pub barrier_to: String,
    /// Where that barrier is: x, y in the cell and depth from the illuminated surface (µm).
    pub barrier_at: [f64; 3],
    /// With the pixel's gates on, the lowest potential (V) on the best path from the empty well
    /// to an n+ region, and where: below the pinning voltage, electrons climb a barrier of
    /// Vpin minus this on their way out (lag).
    pub transfer_barrier: f64,
    pub transfer_barrier_at: [f64; 3],
    /// Storage capacitance q·FW / (Vpin - level) (fF).
    pub capacitance: f64,
    /// Fill level (V) against stored electrons.
    pub charge_curve: Vec<(f64, f64)>,
    /// Photodiode well volume (µm³).
    pub well_volume: f64,
}

/// Charge transfer result.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct TransferResult {
    pub color: String,
    /// Electrons in the photodiode at the start.
    pub electrons: f64,
    /// Times (ns) and electrons left in the photodiode, and arrived at n+ contacts.
    pub time: Vec<f64>,
    pub in_photodiode: Vec<f64>,
    pub transferred: Vec<f64>,
    /// Time (ns) to move 90 % and 99 % of the charge (None: not within the pulse).
    pub t90: Option<f64>,
    pub t99: Option<f64>,
    /// Electrons left in the photodiode after the gate closes (lag).
    pub residual: f64,
}

/// A 2-D map for display: values on a `nx × ny` grid (row-major), physical extent and units.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Map {
    pub name: String,
    pub unit: String,
    pub nx: usize,
    pub ny: usize,
    /// Extent: horizontal (µm), vertical (µm, depth from the illuminated surface for sections).
    pub width: f64,
    pub height: f64,
    pub values: Vec<f64>,
}

/// Device model results.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DeviceResult {
    pub pixels: Vec<PixelDevice>,
    pub transfer: Option<TransferResult>,
    /// Mean collection time (ns) per colour, weighted by the generation at the wavelength of
    /// the colour's peak QE (filled in by the pixel model).
    pub collection_time: [f64; 4],
    /// Sections along x through the pixels' centres of row 0 (y = p / 2) and planes.
    pub maps: Vec<Map>,
    pub warnings: Vec<String>,
    pub nodes: usize,
}

/// The solved device in equilibrium (empty wells), ready to give collection probabilities.
pub struct DeviceModel {
    pub grid: Grid,
    geom: SiliconGeometry,
    vt: f64,
    ni: f64,
    /// Net doping N_D - N_A (µm⁻³).
    net: Vec<f64>,
    /// Silicon voxel (not trench).
    active: Vec<bool>,
    /// Neighbour per voxel and direction (-x, +x, -y, +y, -z, +z), u32::MAX = none.
    nbr: Vec<[u32; 6]>,
    /// Contact voltage per voxel (NaN: none).
    contact: Vec<f64>,
    /// Photodiode well per voxel (pixel index, or -1).
    well: Vec<i8>,
    /// Gate coupling per voxel: (gate index, conductance) for front-surface voxels under gates.
    gate_at: Vec<Option<(usize, f64)>>,
    gates: Vec<Gate>,
    /// Electron mobility (µm²/Vs) and SRH lifetime (s) per voxel.
    mobility: Vec<f64>,
    tau: Vec<f64>,
    /// Surface recombination rate (1/s) per voxel (S × area / volume of its free faces).
    surface: Vec<f64>,
    /// Potential with empty wells and gates off.
    pub psi: Vec<f64>,
    /// Collection probability per pixel and the mean collection time (s) of pixel k's
    /// electrons (expected time × probability; divide by P).
    pub prob: Vec<Vec<f64>>,
    pub time: Vec<Vec<f64>>,
    pub result: DeviceResult,
}

const NONE: u32 = u32::MAX;

/// Bernoulli function x / (e^x - 1).
fn bern(x: f64) -> f64 {
    if x.abs() < 1e-6 {
        1.0 - x / 2.0
    } else if x > 700.0 {
        0.0
    } else {
        x / x.exp_m1()
    }
}

/// Electron mobility (Masetti, cm²/Vs) at total doping `n` (cm⁻³).
fn mobility_cm(n: f64) -> f64 {
    65.0 + (1414.0 - 65.0) / (1.0 + (n / 9.2e16).powf(0.711))
}

fn erf(x: f64) -> f64 {
    // Abramowitz & Stegun 7.1.26, |error| < 1.5e-7.
    let t = 1.0 / (1.0 + 0.327_591_1 * x.abs());
    let y = 1.0
        - (((((1.061_405_429 * t - 1.453_152_027) * t) + 1.421_413_741) * t - 0.284_496_736) * t + 0.254_829_592)
            * t
            * (-x * x).exp();
    if x >= 0.0 { y } else { -y }
}

/// Fraction of a Gaussian-smoothed interval [a, b] at x.
fn smooth_box(x: f64, a: f64, b: f64, s: f64) -> f64 {
    if s <= 0.0 {
        return if x >= a && x < b { 1.0 } else { 0.0 };
    }
    let k = std::f64::consts::SQRT_2 * s;
    0.5 * (erf((x - a) / k) - erf((x - b) / k))
}

pub fn color_index(name: &str) -> Option<usize> {
    crate::pixel::COLORS.iter().position(|c| c.eq_ignore_ascii_case(name))
}

/// Colour name of pixel index `pj * 2 + pi` (RGGB: R (0, 0), Gr (1, 0), Gb (0, 1), B (1, 1)).
pub fn pixel_color(k: usize) -> &'static str {
    ["R", "Gr", "Gb", "B"][k]
}

impl Device {
    /// Whether some entry still waits for shapes from the layout.
    pub fn needs_layout(&self) -> bool {
        self.doping.iter().any(|d| d.layout.is_some() && d.rects.is_empty())
            || self.gate.iter().any(|g| g.layout.is_some() && g.rects.is_empty())
    }

    /// Fill the `layout` masks from the layout.
    pub fn fill_from_layout(&mut self, rects: &crate::pixel::RectsOf<'_>) -> Result<(), String> {
        for d in &mut self.doping {
            if let Some(e) = d.layout.clone() {
                d.rects = rects(&e).map_err(|x| format!("device doping '{}' layout '{e}': {x}", d.name))?;
                if d.rects.is_empty() {
                    return Err(format!("device doping '{}': '{e}' is empty in the layout", d.name));
                }
            }
        }
        for g in &mut self.gate {
            if let Some(e) = g.layout.clone() {
                g.rects = rects(&e).map_err(|x| format!("device gate '{}' layout '{e}': {x}", g.name))?;
                if g.rects.is_empty() {
                    return Err(format!("device gate '{}': '{e}' is empty in the layout", g.name));
                }
            }
        }
        Ok(())
    }

    // `!(x > 0)` also rejects NaN.
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    pub fn check(&self) -> Result<(), String> {
        if !(self.temperature > 0.0) || !(self.lifetime > 0.0) || !(self.surface_recombination >= 0.0) {
            return Err("device: temperature and lifetime must be positive, surface_recombination >= 0".into());
        }
        if !matches!(self.background.kind.as_str(), "n" | "p") || !(self.background.concentration >= 0.0) {
            return Err("device: background type is n or p, with a concentration >= 0".into());
        }
        for d in &self.doping {
            if !matches!(d.kind.as_str(), "n" | "p") {
                return Err(format!("device doping '{}': type is n or p", d.name));
            }
            if !(d.peak > 0.0) || !(d.straggle > 0.0) || !(d.lateral >= 0.0) {
                return Err(format!("device doping '{}': peak and straggle must be positive", d.name));
            }
            if !matches!(d.side.as_str(), "front" | "back" | "dti") {
                return Err(format!("device doping '{}': side is front, back or dti", d.name));
            }
            if (d.photodiode || d.contact.is_some()) && d.kind != "n" {
                return Err(format!("device doping '{}': photodiodes and contacts are n type", d.name));
            }
        }
        if !self.doping.iter().any(|d| d.photodiode) {
            return Err("device: no doping is marked `photodiode = true`".into());
        }
        for g in &self.gate {
            if !(g.oxide > 0.0) || g.rects.is_empty() && g.layout.is_none() {
                return Err(format!("device gate '{}': needs a positive oxide and rects or a layout", g.name));
            }
        }
        if let Some(t) = &self.transfer {
            if color_index(&t.pixel).is_none() {
                return Err(format!("device transfer pixel '{}' (R, Gr, Gb, B)", t.pixel));
            }
            if !(t.on_time > 0.0) || !(t.settle >= 0.0) {
                return Err("device transfer: on_time must be positive".into());
            }
        }
        Ok(())
    }

    /// Build the grid and doping, solve the empty-well potential, the collection probabilities,
    /// the wells' pinning voltage and full well, and (when asked) the charge transfer.
    pub fn solve(&self, si: &Silicon, geom: &SiliconGeometry) -> Result<DeviceModel, String> {
        self.check()?;
        let p = geom.pitch;
        let mut si = si.clone();
        if let Some(h) = self.grid {
            si.grid = Some(h);
        }
        let mut grid = Grid::for_cell(&si, p, geom.depth, geom.depth)?;
        // The device needs a finer vertical step than the optics' default near the surfaces;
        // keep it close to the lateral one.
        let nz = ((geom.depth / grid.dx.min(0.1)).round() as usize).max(8);
        grid.nz = nz;
        grid.dz = geom.depth / nz as f64;
        if grid.len() > 1_500_000 {
            return Err(format!("device: {} nodes; use a coarser grid", grid.len()));
        }
        let mut m = DeviceModel::build(self, grid, geom.clone())?;
        m.equilibrium()?;
        m.collection()?;
        m.wells()?;
        if let Some(t) = &self.transfer {
            m.transfer(t)?;
        }
        m.maps();
        Ok(m)
    }
}

impl DeviceModel {
    fn build(dev: &Device, grid: Grid, geom: SiliconGeometry) -> Result<DeviceModel, String> {
        let (nx, ny, nz) = (grid.nx, grid.ny, grid.nz);
        let n = grid.len();
        let p = geom.pitch;
        let cell = 2.0 * p;
        let vt = thermal_voltage(dev.temperature);
        // Intrinsic density ∝ T^1.5 exp(-Eg / 2kT), 1e10 cm⁻³ at 300 K.
        let ni = 1e10
            * PER_CM3
            * (dev.temperature / 300.0).powf(1.5)
            * (-1.12 / 2.0 * (1.0 / vt - 1.0 / thermal_voltage(300.0))).exp();
        let front_z = |z: f64| if geom.back_illuminated { geom.depth - z } else { z };
        let (dti_span, dti_w) = geom.dti.unwrap_or(((0.0, 0.0), 0.0));
        let in_dti_depth = |z: f64| z >= dti_span.0 && z < dti_span.1;
        let on_wall = |x: f64, y: f64, z: f64| {
            geom.dti.is_some() && in_dti_depth(z) && {
                let near = |v: f64| {
                    let r = v.rem_euclid(p);
                    r < dti_w / 2.0 || r > p - dti_w / 2.0
                };
                near(x) || near(y)
            }
        };
        // Distance from (x, y) to the nearest trench wall face (µm), laterally.
        let wall_distance = |x: f64, y: f64| {
            let d = |v: f64| {
                let r = v.rem_euclid(p);
                (r - dti_w / 2.0).abs().min((p - dti_w / 2.0 - r).abs())
            };
            d(x).min(d(y))
        };
        let period = if geom.cell_rects { cell } else { p };
        // Lateral mask factor of `rects` at (x, y), with periodic images.
        let mask = |rects: &[[f64; 4]], s: f64, x: f64, y: f64| -> f64 {
            if rects.is_empty() {
                return 1.0;
            }
            let mut f = 0.0;
            for r in rects {
                for ix in -1..=1 {
                    for iy in -1..=1 {
                        let (ox, oy) = (ix as f64 * period, iy as f64 * period);
                        let lx = x.rem_euclid(period);
                        let ly = y.rem_euclid(period);
                        f += smooth_box(lx, r[0] + ox, r[2] + ox, s) * smooth_box(ly, r[1] + oy, r[3] + oy, s);
                    }
                }
            }
            f.min(1.0)
        };

        let mut net = vec![0.0; n];
        let mut total = vec![0.0; n];
        let mut active = vec![true; n];
        let mut contact = vec![f64::NAN; n];
        let mut pd_seed = vec![false; n];
        let bg = dev.background.concentration * PER_CM3 * if dev.background.kind == "n" { 1.0 } else { -1.0 };
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    let v = grid.index(i, j, k);
                    let (x, y, z) = grid.centre(i, j, k);
                    if on_wall(x, y, z) {
                        active[v] = false;
                        continue;
                    }
                    let mut nd = if bg > 0.0 { bg } else { 0.0 };
                    let mut na = if bg < 0.0 { -bg } else { 0.0 };
                    let mut best: Option<(f64, usize)> = None;
                    for (di, d) in dev.doping.iter().enumerate() {
                        let dist = match d.side.as_str() {
                            "front" => front_z(z),
                            "back" => {
                                if geom.back_illuminated {
                                    z
                                } else {
                                    geom.depth - z
                                }
                            }
                            _ => {
                                if geom.dti.is_none() || !in_dti_depth(z) {
                                    continue;
                                }
                                wall_distance(x, y)
                            }
                        };
                        let vert = (-(dist - d.depth).powi(2) / (2.0 * d.straggle * d.straggle)).exp();
                        if vert < 1e-12 {
                            continue;
                        }
                        let c = d.peak * PER_CM3 * vert * mask(&d.rects, d.lateral, x, y);
                        if d.kind == "n" {
                            nd += c;
                            if best.is_none_or(|b| c > b.0) {
                                best = Some((c, di));
                            }
                        } else {
                            na += c;
                        }
                    }
                    net[v] = nd - na;
                    total[v] = nd + na;
                    if net[v] > 0.0
                        && let Some((c, di)) = best
                        && c > 0.5 * nd
                    {
                        let d = &dev.doping[di];
                        if let Some(vc) = d.contact {
                            contact[v] = vc;
                        }
                        if d.photodiode {
                            pd_seed[v] = true;
                        }
                    }
                }
            }
        }

        // Neighbours: periodic sides, no coupling across the trenches' faces or into them.
        let half = nx / 2;
        let boundary_face = |a: usize| (a + 1).is_multiple_of(half);
        let mut nbr = vec![[NONE; 6]; n];
        for k in 0..nz {
            let z = (k as f64 + 0.5) * grid.dz;
            let walled = |a: usize| geom.dti.is_some() && in_dti_depth(z) && boundary_face(a);
            for j in 0..ny {
                for i in 0..nx {
                    let v = grid.index(i, j, k);
                    if !active[v] {
                        continue;
                    }
                    let mut e = [NONE; 6];
                    let cand = [
                        (!walled((i + nx - 1) % nx)).then(|| grid.index((i + nx - 1) % nx, j, k)),
                        (!walled(i)).then(|| grid.index((i + 1) % nx, j, k)),
                        (!walled((j + ny - 1) % ny)).then(|| grid.index(i, (j + ny - 1) % ny, k)),
                        (!walled(j)).then(|| grid.index(i, (j + 1) % ny, k)),
                        (k > 0).then(|| grid.index(i, j, k - 1)),
                        (k + 1 < nz).then(|| grid.index(i, j, k + 1)),
                    ];
                    for (q, c) in cand.into_iter().enumerate() {
                        if let Some(u) = c
                            && active[u]
                        {
                            e[q] = u as u32;
                        }
                    }
                    nbr[v] = e;
                }
            }
        }

        // Gates on the front surface: the voxels of the front layer under each gate.
        let k_front = if geom.back_illuminated { nz - 1 } else { 0 };
        let mut gate_at = vec![None; n];
        for (gi, g) in dev.gate.iter().enumerate() {
            // Oxide in series with half a voxel of silicon, per area, over the voxel's depth.
            let cond = 1.0 / ((g.oxide * EPS_SI / EPS_OX + grid.dz / 2.0) * grid.dz);
            for j in 0..ny {
                for i in 0..nx {
                    let (x, y, _) = grid.centre(i, j, k_front);
                    let v = grid.index(i, j, k_front);
                    if active[v] && mask(&g.rects, 0.0, x, y) > 0.5 {
                        gate_at[v] = Some((gi, cond));
                    }
                }
            }
        }

        // Wells: net-n, non-contact voxels connected to a photodiode seed, by pixel.
        let mut well = vec![-1i8; n];
        let pixel_of = |v: usize| {
            let (i, j) = (v % nx, (v / nx) % ny);
            (i / half) + 2 * (j / half)
        };
        let mut warnings = vec![];
        for v0 in 0..n {
            if !pd_seed[v0] || well[v0] >= 0 {
                continue;
            }
            let owner = pixel_of(v0) as i8;
            let mut stack = vec![v0];
            well[v0] = owner;
            let mut touches = false;
            while let Some(v) = stack.pop() {
                for &u in &nbr[v] {
                    if u == NONE {
                        continue;
                    }
                    let u = u as usize;
                    if net[u] <= 0.0 || well[u] >= 0 {
                        if well[u] >= 0 && well[u] != owner {
                            touches = true;
                        }
                        continue;
                    }
                    if !contact[u].is_nan() {
                        touches = true;
                        continue;
                    }
                    well[u] = owner;
                    stack.push(u);
                }
            }
            if touches {
                warnings.push(format!(
                    "the photodiode of {} touches an n+ contact or another photodiode: it is not isolated",
                    pixel_color(owner as usize)
                ));
            }
        }
        for k in 0..4 {
            if !well.contains(&(k as i8)) {
                warnings.push(format!("no photodiode well in pixel {}", pixel_color(k)));
            }
        }

        let mobility: Vec<f64> = total.iter().map(|&t| mobility_cm(t / PER_CM3) * 1e8).collect();
        let tau: Vec<f64> = total.iter().map(|&t| dev.lifetime / (1.0 + t / PER_CM3 / 5e16)).collect();
        // Surface recombination on free faces: the illuminated surface, the trench walls, the
        // far surface (outside contacts and gates).
        let s_um = dev.surface_recombination * 1e4;
        let mut surface = vec![0.0; n];
        if s_um > 0.0 {
            for v in 0..n {
                if !active[v] || !contact[v].is_nan() {
                    continue;
                }
                let k = v / (nx * ny);
                let mut rate = 0.0;
                for q in 0..4 {
                    if nbr[v][q] == NONE {
                        rate += s_um / grid.dx;
                    }
                }
                if k == 0 || (k + 1 == nz && gate_at[v].is_none()) {
                    rate += s_um / grid.dz;
                }
                surface[v] = rate;
            }
        }
        Ok(DeviceModel {
            grid,
            geom,
            vt,
            ni,
            net,
            active,
            nbr,
            contact,
            well,
            gate_at,
            gates: dev.gate.clone(),
            mobility,
            tau,
            surface,
            psi: vec![0.0; n],
            prob: vec![],
            time: vec![],
            result: DeviceResult { warnings, nodes: n, ..DeviceResult::default() },
        })
    }

    fn coef(&self, q: usize) -> f64 {
        if q < 4 { 1.0 / (self.grid.dx * self.grid.dx) } else { 1.0 / (self.grid.dz * self.grid.dz) }
    }

    /// Solve Poisson's equation for the electron quasi-Fermi levels `phin` and the gate
    /// voltage over each voxel `vg`, starting from `psi`.
    fn poisson(&self, psi: &mut [f64], phin: &[f64], vg: &[f64]) -> Result<(), String> {
        let n = self.grid.len();
        let (vt, ni) = (self.vt, self.ni);
        let fixed = |v: usize| !self.active[v] || !self.contact[v].is_nan();
        for v in 0..n {
            if self.active[v] && !self.contact[v].is_nan() {
                let nn = self.net[v].max(ni);
                psi[v] = self.contact[v] + vt * (nn / ni).ln();
            }
        }
        let carriers = |psi: f64, phin: f64| {
            let e = |x: f64| x.clamp(-80.0, 80.0).exp();
            (ni * e(-psi / vt), ni * e((psi - phin) / vt))
        };
        let mut res = vec![0.0; n];
        let mut diag = vec![0.0; n];
        for iter in 0..200 {
            // Residual F and the Jacobian's diagonal (-J = L + G + K (p + n) / VT).
            let mut rd = vec![(0.0, 1.0); n];
            par_fill(&mut rd, |v| {
                if fixed(v) {
                    return (0.0, 1.0);
                }
                let mut f = 0.0;
                let mut d = 0.0;
                for (q, &u) in self.nbr[v].iter().enumerate() {
                    if u != NONE {
                        let c = self.coef(q);
                        f += c * (psi[u as usize] - psi[v]);
                        d += c;
                    }
                }
                if let Some((g, c)) = self.gate_at[v] {
                    let gate = &self.gates[g];
                    f += c * (vg[v] + gate.work_function - psi[v]);
                    d += c;
                }
                let (pp, nn) = carriers(psi[v], phin[v]);
                f += Q_EPS * (pp - nn + self.net[v]);
                d += Q_EPS * (pp + nn) / vt;
                (f, d)
            });
            let mut worst: f64 = 0.0;
            for (v, &(f, d)) in rd.iter().enumerate() {
                res[v] = f;
                diag[v] = d;
                worst = worst.max((f / d).abs());
            }
            if worst < 1e-7 {
                return Ok(());
            }
            let apply = |x: &[f64], out: &mut [f64]| {
                par_fill(out, |v| {
                    if fixed(v) {
                        return 0.0;
                    }
                    let mut s = diag[v] * x[v];
                    for (q, &u) in self.nbr[v].iter().enumerate() {
                        if u != NONE && !fixed(u as usize) {
                            s -= self.coef(q) * x[u as usize];
                        }
                    }
                    s
                })
            };
            let dpsi = cg(&apply, &res, &diag, |v| !fixed(v), 1e-9, 4000);
            // Damping: large steps are shortened logarithmically.
            let mut step: f64 = 0.0;
            for v in 0..n {
                let d = dpsi[v];
                let d = if d.abs() > vt { d.signum() * vt * (1.0 + (d.abs() / vt).ln()) } else { d };
                psi[v] += d;
                step = step.max(d.abs());
            }
            if step < 1e-8 && iter > 0 {
                return Ok(());
            }
        }
        Err("device: Poisson's equation did not converge".into())
    }

    /// Quasi-Fermi levels: contacts at their voltage, the wells (and their basins, when known)
    /// at `levels[k]`, elsewhere the highest contact voltage (no free electrons).
    fn phin(&self, levels: &[f64; 4], basins: bool) -> Vec<f64> {
        let high = self.contact.iter().filter(|c| !c.is_nan()).cloned().fold(f64::NAN, f64::max);
        let high = if high.is_nan() { 3.0 } else { high };
        (0..self.grid.len())
            .map(|v| {
                if !self.contact[v].is_nan() {
                    return self.contact[v];
                }
                if self.well[v] >= 0 {
                    return levels[self.well[v] as usize];
                }
                if basins && !self.prob.is_empty() {
                    for k in 0..4 {
                        if self.prob[k][v] > 0.5 {
                            return levels[k];
                        }
                    }
                }
                high
            })
            .collect()
    }

    /// Gate voltage over each voxel: every gate off, except over pixel `on` (a gate counts in
    /// the pixel it lies in).
    fn gate_voltages(&self, on: Option<usize>) -> Vec<f64> {
        let g = &self.grid;
        let half = g.nx / 2;
        (0..g.len())
            .map(|v| match self.gate_at[v] {
                Some((gi, _)) => {
                    let gate = &self.gates[gi];
                    let pixel = (v % g.nx) / half + 2 * (((v / g.nx) % g.ny) / half);
                    if on == Some(pixel) { gate.on } else { gate.off }
                }
                None => 0.0,
            })
            .collect()
    }

    fn off_voltages(&self) -> Vec<f64> {
        self.gate_voltages(None)
    }

    /// The highest contact voltage (the electrons' level where there are none to speak of).
    fn high_level(&self) -> f64 {
        let h = self.contact.iter().filter(|c| !c.is_nan()).cloned().fold(f64::NAN, f64::max);
        if h.is_nan() { 3.0 } else { h }
    }

    fn equilibrium(&mut self) -> Result<(), String> {
        let n = self.grid.len();
        let vt = self.vt;
        // Start from charge neutrality with holes at 0; empty wells.
        let mut psi: Vec<f64> = (0..n).map(|v| vt * (self.net[v] / (2.0 * self.ni)).asinh()).collect();
        let phin = self.phin(&[self.high_level(); 4], false);
        self.poisson(&mut psi, &phin, &self.off_voltages())?;
        self.psi = psi;
        Ok(())
    }

    /// Scharfetter-Gummel weights per voxel and direction: D A / (h V) B(-(ψu - ψv) / VT), the
    /// rate at which electrons at v flow toward neighbour u (and, by reciprocity, the weight of
    /// u in v's collection probability).
    fn weights(&self, psi: &[f64]) -> Vec<[f64; 6]> {
        let mut out = vec![[0.0; 6]; self.grid.len()];
        par_fill(&mut out, |v| {
            let mut w = [0.0; 6];
            for (q, &u) in self.nbr[v].iter().enumerate() {
                if u != NONE {
                    w[q] = self.sg(psi, v, u as usize, q);
                }
            }
            w
        });
        out
    }

    /// Edge weight from v toward neighbour u for electrons (adjoint / forward outflow factor),
    /// with the Scharfetter-Gummel Bernoulli factor: D A / (h V) B(-(ψu - ψv) / VT).
    fn sg(&self, psi: &[f64], v: usize, u: usize, q: usize) -> f64 {
        let mu = 2.0 * self.mobility[v] * self.mobility[u] / (self.mobility[v] + self.mobility[u]);
        self.vt * mu * self.coef(q) * bern(-(psi[u] - psi[v]) / self.vt)
    }

    /// SRH recombination rate (1/s) for excess electrons at v, in the state (psi, phin).
    fn recombination(&self, v: usize, p: f64, n: f64) -> f64 {
        p / (self.tau[v] * (n + p + 2.0 * self.ni)) + self.surface[v]
    }

    /// Collection probability of every photodiode and the mean collection time.
    fn collection(&mut self) -> Result<(), String> {
        let n = self.grid.len();
        let psi = self.psi.clone();
        let (vt, ni) = (self.vt, self.ni);
        let phin = self.phin(&[self.high_level(); 4], false);
        let rate: Vec<f64> = (0..n)
            .map(|v| {
                let p = ni * (-psi[v] / vt).clamp(-80.0, 80.0).exp();
                let e = ni * ((psi[v] - phin[v]) / vt).clamp(-80.0, 80.0).exp();
                self.recombination(v, p, e)
            })
            .collect();
        let sink: Vec<bool> = (0..n).map(|v| self.well[v] >= 0 || !self.contact[v].is_nan() || !self.active[v]).collect();
        let w = self.weights(&psi);
        let mut diag = vec![1.0; n];
        for v in 0..n {
            if !sink[v] {
                diag[v] = rate[v] + w[v].iter().sum::<f64>();
            }
        }
        let apply = |x: &[f64], out: &mut [f64]| {
            par_fill(out, |v| {
                if sink[v] {
                    return 0.0;
                }
                let mut s = diag[v] * x[v];
                for (q, &u) in self.nbr[v].iter().enumerate() {
                    if u != NONE && !sink[u as usize] {
                        s -= w[v][q] * x[u as usize];
                    }
                }
                s
            })
        };
        let sink = |v: usize| sink[v];
        let mut prob = vec![];
        let mut time = vec![];
        for k in 0..4 {
            let mut b = vec![0.0; n];
            for v in 0..n {
                if sink(v) {
                    continue;
                }
                for (q, &u) in self.nbr[v].iter().enumerate() {
                    if u != NONE && self.well[u as usize] == k as i8 {
                        b[v] += w[v][q];
                    }
                }
            }
            let x = bicgstab(&apply, &b, &diag, |v| !sink(v), 1e-10, 20_000)?;
            let pk: Vec<f64> = (0..n)
                .map(|v| {
                    if self.well[v] == k as i8 {
                        1.0
                    } else if sink(v) {
                        0.0
                    } else {
                        x[v].clamp(0.0, 1.0)
                    }
                })
                .collect();
            // Expected time × probability: the same operator with P as its source.
            let src: Vec<f64> = (0..n).map(|v| if sink(v) { 0.0 } else { pk[v] }).collect();
            let t = bicgstab(&apply, &src, &diag, |v| !sink(v), 1e-10, 20_000)?;
            time.push((0..n).map(|v| if sink(v) { 0.0 } else { t[v].max(0.0) }).collect());
            prob.push(pk);
        }
        self.prob = prob;
        self.time = time;
        Ok(())
    }

    /// Electrons in each well (and its basin) for the state psi / phin.
    fn stored(&self, psi: &[f64], phin: &[f64]) -> [f64; 4] {
        let vol = self.grid.dx * self.grid.dx * self.grid.dz;
        let mut q = [0.0; 4];
        for v in 0..self.grid.len() {
            if !self.active[v] || !self.contact[v].is_nan() {
                continue;
            }
            let k = if self.well[v] >= 0 {
                self.well[v] as usize
            } else if let Some(k) = (0..4).find(|&k| self.prob[k][v] > 0.5) {
                k
            } else {
                continue;
            };
            q[k] += self.ni * ((psi[v] - phin[v]) / self.vt).clamp(-80.0, 80.0).exp() * vol;
        }
        q
    }

    /// The lowest barrier (highest bottleneck potential) from well k to any other sink, and
    /// what it leads to.
    fn barrier(&self, psi: &[f64], k: usize) -> (f64, String, [f64; 3]) {
        use std::cmp::Ordering;
        use std::collections::BinaryHeap;
        #[derive(PartialEq)]
        struct Item(f64, usize);
        impl Eq for Item {}
        impl PartialOrd for Item {
            fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
                Some(self.cmp(o))
            }
        }
        impl Ord for Item {
            fn cmp(&self, o: &Self) -> Ordering {
                self.0.total_cmp(&o.0)
            }
        }
        let n = self.grid.len();
        let mut best = vec![f64::NEG_INFINITY; n];
        let mut from = vec![u32::MAX; n];
        // The saddle: the lowest point of the path found.
        let saddle = |from: &[u32], mut v: usize| {
            let mut lowest = v;
            while from[v] != u32::MAX {
                v = from[v] as usize;
                if psi[v] < psi[lowest] {
                    lowest = v;
                }
            }
            let g = &self.grid;
            let c = g.centre(lowest % g.nx, (lowest / g.nx) % g.ny, lowest / (g.nx * g.ny));
            [c.0, c.1, c.2]
        };
        let mut heap = BinaryHeap::new();
        for v in 0..n {
            if self.well[v] == k as i8 {
                best[v] = psi[v];
                heap.push(Item(psi[v], v));
            }
        }
        while let Some(Item(val, v)) = heap.pop() {
            if val < best[v] {
                continue;
            }
            if self.well[v] != k as i8 {
                if !self.contact[v].is_nan() {
                    return (val, "an n+ contact".into(), saddle(&from, v));
                }
                if self.well[v] >= 0 {
                    return (val, format!("the {} photodiode", pixel_color(self.well[v] as usize)), saddle(&from, v));
                }
            }
            for &u in &self.nbr[v] {
                if u == NONE {
                    continue;
                }
                let u = u as usize;
                let nv = val.min(psi[u]);
                if nv > best[u] {
                    best[u] = nv;
                    from[u] = v as u32;
                    heap.push(Item(nv, u));
                }
            }
        }
        (f64::NEG_INFINITY, "nothing".into(), [f64::NAN; 3])
    }

    /// Pinning voltage and full well of every photodiode.
    fn wells(&mut self) -> Result<(), String> {
        let n = self.grid.len();
        let vg = self.off_voltages();
        let mut pixels = vec![];
        let vpin: Vec<f64> =
            (0..4).map(|k| (0..n).filter(|&v| self.well[v] == k as i8).map(|v| self.psi[v]).fold(f64::NAN, f64::max)).collect();
        let barrier0: Vec<(f64, String, [f64; 3])> = (0..4).map(|k| self.barrier(&self.psi, k)).collect();
        // With each pixel's gates on (empty wells): the transfer path's lowest point.
        let mut on_barrier = vec![];
        let phin_empty = self.phin(&[self.high_level(); 4], false);
        for k in 0..4 {
            let mut psi_on = self.psi.clone();
            self.poisson(&mut psi_on, &phin_empty, &self.gate_voltages(Some(k)))?;
            let (b, _, at) = self.barrier(&psi_on, k);
            on_barrier.push((b, at));
        }
        // Bisect every pixel's fill level at once: full when the level reaches the barrier.
        let high = vpin.iter().cloned().fold(f64::NAN, f64::max);
        if high.is_nan() {
            return Err("device: no photodiode well".into());
        }
        let mut lo = [-1.0f64; 4];
        let mut hi: [f64; 4] = std::array::from_fn(|k| if vpin[k].is_nan() { high } else { vpin[k] });
        let mut psi = self.psi.clone();
        let mut curves: Vec<Vec<(f64, f64)>> = vec![vec![]; 4];
        let mut at_full: [(f64, f64, f64); 4] = [(0.0, -1.0, f64::NAN); 4];
        for _ in 0..14 {
            let mid: [f64; 4] = std::array::from_fn(|k| 0.5 * (lo[k] + hi[k]));
            let phin = self.phin(&mid, true);
            self.poisson(&mut psi, &phin, &vg)?;
            let q = self.stored(&psi, &phin);
            for k in 0..4 {
                let (b, _, _) = self.barrier(&psi, k);
                curves[k].push((mid[k], q[k]));
                if mid[k] > b {
                    // Not yet full: fill more (lower level).
                    hi[k] = mid[k];
                } else {
                    lo[k] = mid[k];
                    at_full[k] = (q[k], mid[k], b);
                }
            }
        }
        for k in 0..4 {
            curves[k].sort_by(|a, b| b.0.total_cmp(&a.0));
            let (fw, level, _) = at_full[k];
            let volume = (0..n).filter(|&v| self.well[v] == k as i8).count() as f64 * self.grid.dx * self.grid.dx * self.grid.dz;
            let cap = if vpin[k] - level > 1e-6 { Q * fw / (vpin[k] - level) * 1e15 } else { 0.0 };
            pixels.push(PixelDevice {
                color: pixel_color(k).into(),
                pinning_voltage: vpin[k],
                full_well: fw,
                full_well_level: level,
                barrier: barrier0[k].0,
                barrier_to: barrier0[k].1.clone(),
                barrier_at: barrier0[k].2,
                transfer_barrier: on_barrier[k].0,
                transfer_barrier_at: on_barrier[k].1,
                capacitance: cap,
                charge_curve: curves[k].clone(),
                well_volume: volume,
            });
        }
        self.result.pixels = pixels;
        Ok(())
    }

    /// Charge transfer of one pixel: full well (or the given electrons), its gates on for
    /// `on_time`, then off for `settle`.
    fn transfer(&mut self, t: &Transfer) -> Result<(), String> {
        let k = color_index(&t.pixel).unwrap();
        let n = self.grid.len();
        let (vt, ni) = (self.vt, self.ni);
        let vol = self.grid.dx * self.grid.dx * self.grid.dz;
        let pd = &self.result.pixels[k];
        let target = t.electrons.unwrap_or(pd.full_well);
        // Initial fill: bisect the level for the target electrons (gates off).
        let vg_off = self.off_voltages();
        let mut psi = self.psi.clone();
        let (mut lo, mut hi) = (pd.full_well_level - 0.5, pd.pinning_voltage);
        let mut phin = vec![];
        for _ in 0..16 {
            let mid = 0.5 * (lo + hi);
            let mut levels = [f64::NAN; 4];
            for (j, l) in levels.iter_mut().enumerate() {
                *l = if j == k { mid } else { self.result.pixels[j].pinning_voltage + 1.0 };
            }
            phin = self.phin(&levels, true);
            self.poisson(&mut psi, &phin, &vg_off)?;
            if self.stored(&psi, &phin)[k] > target {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let mut el: Vec<f64> = (0..n).map(|v| ni * ((psi[v] - phin[v]) / vt).clamp(-80.0, 80.0).exp()).collect();
        let in_pd = |el: &[f64]| (0..n).filter(|&v| self.well[v] == k as i8).map(|v| el[v]).sum::<f64>() * vol;
        let total_free =
            |el: &[f64]| (0..n).filter(|&v| self.active[v] && self.contact[v].is_nan()).map(|v| el[v]).sum::<f64>() * vol;
        let start = in_pd(&el);
        let start_free = total_free(&el);
        let mut out = TransferResult { color: pixel_color(k).into(), electrons: start, ..TransferResult::default() };
        out.time.push(0.0);
        out.in_photodiode.push(start);
        out.transferred.push(0.0);
        let on = self.gate_voltages(Some(k));
        let contact_n: Vec<f64> = (0..n).map(|v| self.net[v].max(ni)).collect();
        let mut time = 0.0;
        let mut dt: f64 = 1e-12;
        let t_on = t.on_time * 1e-9;
        let t_end = t_on + t.settle * 1e-9;
        let mut arrived = 0.0;
        while time < t_end - 1e-18 {
            let phase_end = if time < t_on - 1e-18 { t_on } else { t_end };
            let step = dt.min(phase_end - time);
            let vg = if time < t_on - 1e-18 { &on } else { &vg_off };
            let old = el.clone();
            // Gummel: Poisson with frozen quasi-Fermi levels, then continuity, twice.
            for _ in 0..2 {
                let phin: Vec<f64> = (0..n)
                    .map(|v| if !self.contact[v].is_nan() { self.contact[v] } else { psi[v] - vt * (el[v].max(1e-30) / ni).ln() })
                    .collect();
                self.poisson(&mut psi, &phin, vg)?;
                el = self.continuity(&psi, &old, &contact_n, step)?;
            }
            // Electrons that left the free silicon went into the contacts.
            arrived = start_free - total_free(&el);
            time += step;
            dt *= 1.35;
            out.time.push(time * 1e9);
            out.in_photodiode.push(in_pd(&el));
            out.transferred.push(arrived);
        }
        let _ = arrived;
        let on_ns = t.on_time;
        let moved = |frac: f64| {
            out.time
                .iter()
                .zip(&out.in_photodiode)
                .find(|(time, q)| **time <= on_ns + 1e-9 && **q <= start * (1.0 - frac))
                .map(|(time, _)| *time)
        };
        out.t90 = moved(0.9);
        out.t99 = moved(0.99);
        out.residual = *out.in_photodiode.last().unwrap();
        self.result.transfer = Some(out);
        Ok(())
    }

    /// One implicit Euler step of the electron continuity equation (no generation), with
    /// n fixed at the contacts.
    fn continuity(&self, psi: &[f64], old: &[f64], contact_n: &[f64], dt: f64) -> Result<Vec<f64>, String> {
        let n = self.grid.len();
        let (vt, ni) = (self.vt, self.ni);
        let fixed: Vec<bool> = (0..n).map(|v| !self.active[v] || !self.contact[v].is_nan()).collect();
        let w = self.weights(psi);
        // The opposite direction of each of the six.
        const OPP: [usize; 6] = [1, 0, 3, 2, 5, 4];
        let mut diag = vec![1.0; n];
        let mut b = vec![0.0; n];
        for v in 0..n {
            if fixed[v] {
                continue;
            }
            let p = ni * (-psi[v] / vt).clamp(-80.0, 80.0).exp();
            // Outflow v → u: w[v][q] n_v - w[u][q'] n_u; recombination with the holes there.
            diag[v] = 1.0 / dt + self.recombination(v, p, old[v]) + w[v].iter().sum::<f64>();
            let mut rhs = old[v] / dt;
            for (q, &u) in self.nbr[v].iter().enumerate() {
                if u != NONE && !self.contact[u as usize].is_nan() {
                    rhs += w[u as usize][OPP[q]] * contact_n[u as usize];
                }
            }
            b[v] = rhs;
        }
        let apply = |x: &[f64], out: &mut [f64]| {
            par_fill(out, |v| {
                if fixed[v] {
                    return 0.0;
                }
                let mut s = diag[v] * x[v];
                for (q, &u) in self.nbr[v].iter().enumerate() {
                    if u != NONE && !fixed[u as usize] {
                        s -= w[u as usize][OPP[q]] * x[u as usize];
                    }
                }
                s
            })
        };
        let x = bicgstab_from(&apply, &b, &diag, |v| !fixed[v], 1e-10, 20_000, Some(old))?;
        Ok((0..n).map(|v| if fixed[v] { if self.active[v] { contact_n[v] } else { 0.0 } } else { x[v].max(0.0) }).collect())
    }

    /// Mean collection time (s) of carriers generated as `gen` (per voxel), for pixel k.
    pub fn mean_time(&self, k: usize, generation: &[f64]) -> f64 {
        let (mut num, mut den) = (0.0, 0.0);
        for v in 0..self.grid.len() {
            num += generation[v] * self.time[k][v];
            den += generation[v] * self.prob[k][v];
        }
        if den > 0.0 { num / den } else { 0.0 }
    }

    /// Display maps: sections through row 0's pixel centres and the photodiodes' plane.
    fn maps(&mut self) {
        let g = &self.grid;
        let j = (g.ny / 4).min(g.ny - 1);
        let section = |name: &str, unit: &str, f: &dyn Fn(usize) -> f64| Map {
            name: name.into(),
            unit: unit.into(),
            nx: g.nx,
            ny: g.nz,
            width: g.nx as f64 * g.dx,
            height: g.nz as f64 * g.dz,
            values: (0..g.nz).flat_map(|k| (0..g.nx).map(move |i| (i, k))).map(|(i, k)| f(g.index(i, j, k))).collect(),
        };
        let active = &self.active;
        let net = &self.net;
        let psi = &self.psi;
        let prob = &self.prob;
        let time = &self.time;
        let mut maps = vec![
            section("Potential (empty wells, gates off)", "V", &|v| if active[v] { psi[v] } else { f64::NAN }),
            section("Net doping", "log10 (|N| / 1e10 cm^-3), n > 0, p < 0", &|v| {
                if !active[v] {
                    return f64::NAN;
                }
                let c = net[v] / PER_CM3;
                c.signum() * (c.abs().max(1e10).log10() - 10.0)
            }),
            section("Collection probability, sum of photodiodes", "", &|v| {
                if active[v] { (0..4).map(|k| prob[k][v]).sum() } else { f64::NAN }
            }),
            section("Collection probability of R", "", &|v| if active[v] { prob[0][v] } else { f64::NAN }),
            section("Collection time of R", "log10 ns", &|v| {
                if active[v] && prob[0][v] > 1e-3 { (time[0][v] / prob[0][v] * 1e9).max(1e-4).log10() } else { f64::NAN }
            }),
        ];
        // Plan view at the wells' depth.
        let kw =
            (0..g.nz).max_by_key(|&k| (0..g.nx * g.ny).filter(|&a| self.well[k * g.nx * g.ny + a] >= 0).count()).unwrap_or(0);
        maps.push(Map {
            name: format!("Potential at depth {:.2} µm", (kw as f64 + 0.5) * g.dz),
            unit: "V".into(),
            nx: g.nx,
            ny: g.ny,
            width: g.nx as f64 * g.dx,
            height: g.ny as f64 * g.dx,
            values: (0..g.nx * g.ny)
                .map(|a| if active[kw * g.nx * g.ny + a] { psi[kw * g.nx * g.ny + a] } else { f64::NAN })
                .collect(),
        });
        self.result.maps = maps;
    }

    pub fn geometry(&self) -> &SiliconGeometry {
        &self.geom
    }
}

/// out[v] = f(v) for every v, in parallel chunks for large grids.
fn par_fill<T: Send>(out: &mut [T], f: impl Fn(usize) -> T + Sync) {
    let n = out.len();
    let threads = std::thread::available_parallelism().map_or(1, |t| t.get()).min(8);
    if n < 20_000 || threads < 2 {
        for (v, o) in out.iter_mut().enumerate() {
            *o = f(v);
        }
        return;
    }
    let chunk = n.div_ceil(threads);
    let f = &f;
    std::thread::scope(|sc| {
        for (c, part) in out.chunks_mut(chunk).enumerate() {
            sc.spawn(move || {
                for (i, o) in part.iter_mut().enumerate() {
                    *o = f(c * chunk + i);
                }
            });
        }
    });
}

/// Jacobi-preconditioned conjugate gradient on the `active` unknowns (SPD operator).
fn cg(
    apply: &dyn Fn(&[f64], &mut [f64]),
    b: &[f64],
    diag: &[f64],
    active: impl Fn(usize) -> bool,
    tol: f64,
    max_iter: usize,
) -> Vec<f64> {
    let n = b.len();
    let mask: Vec<bool> = (0..n).map(active).collect();
    let mut x = vec![0.0; n];
    let mut r: Vec<f64> = b.iter().zip(&mask).map(|(v, m)| if *m { *v } else { 0.0 }).collect();
    let pre = |r: &[f64], z: &mut [f64]| {
        for v in 0..n {
            z[v] = if mask[v] && diag[v] > 0.0 { r[v] / diag[v] } else { 0.0 };
        }
    };
    let mut z = vec![0.0; n];
    pre(&r, &mut z);
    let mut p = z.clone();
    let mut rz: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
    let b_norm = r.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-300);
    let mut ap = vec![0.0; n];
    for _ in 0..max_iter {
        apply(&p, &mut ap);
        let pap: f64 = p.iter().zip(&ap).map(|(a, b)| a * b).sum();
        if pap <= 0.0 {
            break;
        }
        let alpha = rz / pap;
        for v in 0..n {
            x[v] += alpha * p[v];
            r[v] -= alpha * ap[v];
        }
        if r.iter().map(|v| v * v).sum::<f64>().sqrt() < tol * b_norm {
            break;
        }
        pre(&r, &mut z);
        let rz_new: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
        let beta = rz_new / rz;
        rz = rz_new;
        for v in 0..n {
            p[v] = z[v] + beta * p[v];
        }
    }
    x
}

fn bicgstab(
    apply: &dyn Fn(&[f64], &mut [f64]),
    b: &[f64],
    diag: &[f64],
    active: impl Fn(usize) -> bool,
    tol: f64,
    max_iter: usize,
) -> Result<Vec<f64>, String> {
    bicgstab_from(apply, b, diag, active, tol, max_iter, None)
}

/// Jacobi-preconditioned BiCGSTAB on the `active` unknowns, from `x0`.
fn bicgstab_from(
    apply: &dyn Fn(&[f64], &mut [f64]),
    b: &[f64],
    diag: &[f64],
    active: impl Fn(usize) -> bool,
    tol: f64,
    max_iter: usize,
    x0: Option<&[f64]>,
) -> Result<Vec<f64>, String> {
    let n = b.len();
    let mask: Vec<bool> = (0..n).map(active).collect();
    let dot = |a: &[f64], b: &[f64]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f64>();
    let mut x: Vec<f64> = match x0 {
        Some(x0) => (0..n).map(|v| if mask[v] { x0[v] } else { 0.0 }).collect(),
        None => vec![0.0; n],
    };
    let mut ax = vec![0.0; n];
    apply(&x, &mut ax);
    let mut r: Vec<f64> = (0..n).map(|v| if mask[v] { b[v] - ax[v] } else { 0.0 }).collect();
    let b_norm = (0..n).filter(|&v| mask[v]).map(|v| b[v] * b[v]).sum::<f64>().sqrt();
    if b_norm == 0.0 {
        return Ok(vec![0.0; n]);
    }
    let r0 = r.clone();
    let (mut rho, mut alpha, mut omega) = (1.0, 1.0, 1.0);
    let mut v = vec![0.0; n];
    let mut p = vec![0.0; n];
    let mut y = vec![0.0; n];
    let mut z = vec![0.0; n];
    let mut s = vec![0.0; n];
    let mut t = vec![0.0; n];
    let pre = |a: &[f64], out: &mut [f64]| {
        for i in 0..n {
            out[i] = if mask[i] && diag[i] != 0.0 { a[i] / diag[i] } else { 0.0 };
        }
    };
    for _ in 0..max_iter {
        let rho_new = dot(&r0, &r);
        if rho_new == 0.0 {
            break;
        }
        let beta = (rho_new / rho) * (alpha / omega);
        rho = rho_new;
        for i in 0..n {
            p[i] = r[i] + beta * (p[i] - omega * v[i]);
        }
        pre(&p, &mut y);
        apply(&y, &mut v);
        let r0v = dot(&r0, &v);
        if r0v == 0.0 {
            break;
        }
        alpha = rho / r0v;
        for i in 0..n {
            s[i] = r[i] - alpha * v[i];
        }
        if dot(&s, &s).sqrt() < tol * b_norm {
            for i in 0..n {
                x[i] += alpha * y[i];
            }
            return Ok(x);
        }
        pre(&s, &mut z);
        apply(&z, &mut t);
        let tt = dot(&t, &t);
        omega = if tt > 0.0 { dot(&t, &s) / tt } else { 0.0 };
        for i in 0..n {
            x[i] += alpha * y[i] + omega * z[i];
            r[i] = s[i] - omega * t[i];
        }
        if dot(&r, &r).sqrt() < tol * b_norm {
            return Ok(x);
        }
        if omega == 0.0 {
            break;
        }
    }
    let res = dot(&r, &r).sqrt() / b_norm;
    if res < 1e-6 { Ok(x) } else { Err(format!("device: the transport equations did not converge (residual {res:.1e})")) }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A 2 µm back-side illuminated pixel, 3 µm of silicon, full-depth trenches; every pixel the
    /// same (pixel coordinates): deep photodiode, front p-well, pinned surface region next to a
    /// transfer gate onto an n+ floating diffusion.
    pub(crate) const PIXEL: &str = r#"
background = { type = "p", concentration = 1e15 }
transfer = { pixel = "Gr", on_time = 20, settle = 5 }
[[doping]]
name = "back p+"
type = "p"
peak = 2e18
side = "back"
straggle = 0.06
[[doping]]
name = "trench liner p"
type = "p"
peak = 5e17
side = "dti"
straggle = 0.05
[[doping]]
name = "photodiode"
type = "n"
peak = 6e15
depth = 1.4
straggle = 0.45
lateral = 0.1
rects = [[0.2, 0.2, 1.8, 1.8]]
photodiode = true
[[doping]]
name = "photodiode surface"
type = "n"
peak = 4e16
depth = 0.25
straggle = 0.15
rects = [[0.3, 0.3, 1.2, 1.2]]
photodiode = true
[[doping]]
name = "p-well"
type = "p"
peak = 3e17
depth = 0.25
straggle = 0.12
rects = [[0, 0, 2, 0.3], [0, 1.2, 2, 2], [0, 0.3, 0.3, 1.2], [1.35, 0.3, 2, 1.2]]
[[doping]]
name = "pinning"
type = "p"
peak = 5e18
straggle = 0.03
rects = [[0.25, 0.25, 1.05, 1.25]]
[[doping]]
name = "floating diffusion"
type = "n"
peak = 1e20
straggle = 0.04
rects = [[1.55, 0.5, 1.8, 1.0]]
contact = 2.8
[[gate]]
name = "TG"
rects = [[1.1, 0.45, 1.6, 1.05]]
off = -1.0
on = 2.8
"#;

    pub(crate) fn geometry() -> (Silicon, SiliconGeometry) {
        let si = Silicon { thickness: Some(3.0), grid: Some(0.1), ..Silicon::default() };
        let geom =
            SiliconGeometry { pitch: 2.0, depth: 3.0, back_illuminated: true, dti: Some(((0.0, 3.0), 0.15)), cell_rects: false };
        (si, geom)
    }

    #[test]
    fn pinned_photodiode_well_collection_and_transfer() {
        let dev: Device = toml::from_str(PIXEL).unwrap();
        let (si, geom) = geometry();
        let t0 = std::time::Instant::now();
        let m = dev.solve(&si, &geom).unwrap();
        eprintln!("device solve {:?} for {} nodes; {:?}", t0.elapsed(), m.result.nodes, m.result.warnings);
        assert!(m.result.warnings.is_empty(), "{:?}", m.result.warnings);
        for px in &m.result.pixels {
            eprintln!(
                "{}: Vpin {:.3} V, FW {:.0} e- at {:.3} V, barrier {:.3} V to {}, C {:.2} fF, well {:.2} µm³",
                px.color,
                px.pinning_voltage,
                px.full_well,
                px.full_well_level,
                px.barrier,
                px.barrier_to,
                px.capacitance,
                px.well_volume
            );
        }
        let px = &m.result.pixels[1];
        assert!((0.3..2.5).contains(&px.pinning_voltage), "{px:?}");
        assert!((1000.0..100_000.0).contains(&px.full_well), "{px:?}");
        assert_eq!(px.barrier_to, "an n+ contact");
        // Every pixel is the same here.
        for p in &m.result.pixels {
            assert!((p.pinning_voltage - px.pinning_voltage).abs() < 1e-6);
        }
        // Electrons generated in the middle of a pixel end in its photodiode; the trenches keep
        // them from the neighbours.
        let g = &m.grid;
        let v = g.voxel(3.0, 1.0, 1.5).unwrap(); // pixel (1, 0) = Gr
        assert!(m.prob[1][v] > 0.9, "{}", m.prob[1][v]);
        assert!(m.prob[0][v] + m.prob[2][v] + m.prob[3][v] < 1e-6);
        // Electrons generated near the illuminated surface, outside the well, drift and
        // diffuse into it within nanoseconds.
        let v = g.voxel(3.0, 1.0, 0.35).unwrap();
        assert_eq!(m.well[v], -1);
        let t = m.time[1][v] / m.prob[1][v];
        eprintln!("near the back surface: P {:.3}, collection time {:.3} ns", m.prob[1][v], t * 1e9);
        assert!(m.prob[1][v] > 0.8 && t > 1e-12 && t < 1e-8, "P {} time {t}", m.prob[1][v]);
        let tr = m.result.transfer.as_ref().unwrap();
        eprintln!("transfer of {:.0} e-: t90 {:?} ns, t99 {:?} ns, residual {:.1} e-", tr.electrons, tr.t90, tr.t99, tr.residual);
        assert!(tr.t90.is_some_and(|t| t < 5.0) && tr.residual < 0.05 * tr.electrons, "{tr:?}");
    }
}
