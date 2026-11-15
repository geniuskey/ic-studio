//! Process emulation: a cross-section of a layout along a cut line, built by running a process
//! flow (deposit, etch, implant, planarize, strip) on a 2-D grid of material cells.
//!
//! The cut runs along the wafer surface (x, µm from the cut's start) with z up (µm, 0 at the
//! original silicon surface). Masks are layout layers or derived-layer expressions sampled
//! along the cut. Conformal deposition and isotropic etching use the exact Euclidean distance
//! transform, so sidewalls, spacers, overhangs and voids come out of the geometry; anisotropic
//! steps act per column.
//!
//! The flow is TOML (see `process/generic180.toml`):
//!
//! ```toml
//! name = "generic180 FEOL + 2 metals"
//! substrate = "si"
//! substrate_depth = 1.5      # µm of substrate shown
//! resolution = 0.005         # grid step (µm)
//!
//! [[step]]
//! op = "etch"                # STI trench where there is no active area
//! keep = "active"            # masked: etched where the layer is absent (`mask` = where present)
//! materials = ["si"]
//! depth = 0.35
//! ```

use serde::{Deserialize, Serialize};

use crate::pixel_layout::PixelLayout;
use crate::project::{CellRef, Project};

/// A process flow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessFlow {
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_substrate")]
    pub substrate: String,
    /// Substrate depth shown (µm).
    #[serde(default = "default_depth")]
    pub substrate_depth: f64,
    /// Grid step (µm).
    #[serde(default = "default_resolution")]
    pub resolution: f64,
    /// Material colours (`#rrggbb`) by name, over the defaults.
    #[serde(default)]
    pub colors: std::collections::BTreeMap<String, String>,
    #[serde(rename = "step")]
    pub steps: Vec<Step>,
}

/// A finite positive number.
fn positive(v: f64) -> bool {
    v.is_finite() && v > 0.0
}

fn default_substrate() -> String {
    "si".into()
}
fn default_depth() -> f64 {
    1.0
}
fn default_resolution() -> f64 {
    0.01
}

/// Where a step acts: `mask` (where the layer expression is present), `keep` (where it is
/// absent), or everywhere without either.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Mask {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep: Option<String>,
}

/// One process step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase", deny_unknown_fields)]
pub enum Step {
    /// Deposit `thickness` of `material`: on top of every column, or `conformal` (also on
    /// sidewalls, as CVD).
    Deposit {
        #[serde(default)]
        name: String,
        material: String,
        thickness: f64,
        #[serde(default)]
        conformal: bool,
    },
    /// Etch `materials` where the mask is open: anisotropic from the surface down by `depth`
    /// (until a material that is not etched without a depth), or `isotropic` by `depth`.
    Etch {
        #[serde(default)]
        name: String,
        #[serde(flatten)]
        mask: Mask,
        materials: Vec<String>,
        #[serde(default)]
        depth: Option<f64>,
        #[serde(default)]
        isotropic: bool,
    },
    /// Turn the cells of `into` within `depth` below the surface into `material` where the
    /// mask is open; anything on top counts towards the depth (a gate blocks the implant).
    Implant {
        #[serde(default)]
        name: String,
        #[serde(flatten)]
        mask: Mask,
        material: String,
        into: Vec<String>,
        depth: f64,
    },
    /// Remove everything above `height`, or above the highest point of material `stop`.
    Planarize {
        #[serde(default)]
        name: String,
        #[serde(default)]
        height: Option<f64>,
        #[serde(default)]
        stop: Option<String>,
    },
    /// Remove every cell of `material`.
    Strip {
        #[serde(default)]
        name: String,
        material: String,
    },
}

impl Step {
    /// The step's name, or a description.
    pub fn label(&self) -> String {
        let (name, what) = match self {
            Step::Deposit { name, material, thickness, conformal } => {
                (name, format!("deposit {thickness} µm {material}{}", if *conformal { " (conformal)" } else { "" }))
            }
            Step::Etch { name, materials, depth, isotropic, .. } => (
                name,
                format!(
                    "{}etch {}{}",
                    if *isotropic { "isotropic " } else { "" },
                    materials.join(", "),
                    depth.map_or(String::new(), |d| format!(" {d} µm"))
                ),
            ),
            Step::Implant { name, material, depth, .. } => (name, format!("implant {material} {depth} µm")),
            Step::Planarize { name, height, stop } => (
                name,
                match (height, stop) {
                    (Some(h), _) => format!("planarize to {h} µm"),
                    (None, Some(s)) => format!("planarize to {s}"),
                    _ => "planarize".into(),
                },
            ),
            Step::Strip { name, material } => (name, format!("strip {material}")),
        };
        if name.is_empty() { what } else { format!("{name}: {what}") }
    }
}

impl ProcessFlow {
    pub fn from_toml(text: &str) -> Result<ProcessFlow, String> {
        let f: ProcessFlow = toml::from_str(text).map_err(|e| e.to_string())?;
        if !(f.resolution > 0.0 && f.resolution <= 1.0) {
            return Err("resolution must be between 0 and 1 µm".into());
        }
        if !positive(f.substrate_depth) {
            return Err("substrate_depth must be positive".into());
        }
        for (k, s) in f.steps.iter().enumerate() {
            let bad = match s {
                Step::Deposit { thickness, .. } => !positive(*thickness),
                Step::Etch { depth, isotropic, materials, .. } => {
                    materials.is_empty() || depth.is_some_and(|d| !positive(d)) || (*isotropic && depth.is_none())
                }
                Step::Implant { depth, into, .. } => !positive(*depth) || into.is_empty(),
                Step::Planarize { height, stop, .. } => height.is_none() && stop.is_none(),
                Step::Strip { .. } => false,
            };
            if bad {
                return Err(format!("step {} ({}): missing or invalid values", k + 1, s.label()));
            }
        }
        Ok(f)
    }

    /// Layer expressions the flow's masks use.
    pub fn mask_exprs(&self) -> Vec<String> {
        let mut out: Vec<String> = vec![];
        for s in &self.steps {
            if let Step::Etch { mask, .. } | Step::Implant { mask, .. } = s {
                for e in [&mask.mask, &mask.keep].into_iter().flatten() {
                    if !out.contains(e) {
                        out.push(e.clone());
                    }
                }
            }
        }
        out
    }

    /// Total deposited thickness (µm), an upper bound of the stack height.
    fn deposited(&self) -> f64 {
        self.steps.iter().map(|s| if let Step::Deposit { thickness, .. } = s { *thickness } else { 0.0 }).sum()
    }

    /// Colour of a material: the flow's, else a default.
    pub fn color(&self, material: &str) -> [u8; 3] {
        if let Some(c) = self.colors.get(material).and_then(|c| crate::pdk::parse_color(c)) {
            return [c[0], c[1], c[2]];
        }
        default_color(material)
    }
}

/// Default colours of common materials.
pub fn default_color(material: &str) -> [u8; 3] {
    match material.to_ascii_lowercase().as_str() {
        "si" | "silicon" | "psub" | "p-" => [150, 150, 160],
        "nwell" | "n-" => [120, 150, 200],
        "n+" | "ndiff" => [70, 110, 220],
        "p+" | "pdiff" => [220, 120, 80],
        "sio2" | "oxide" | "pmd" | "imd" | "sti" => [190, 225, 240],
        "si3n4" | "nitride" => [120, 200, 140],
        "poly" | "polysilicon" => [220, 70, 70],
        "w" | "tungsten" => [90, 90, 100],
        "al" | "aluminium" | "aluminum" => [200, 200, 210],
        "cu" | "copper" => [200, 120, 50],
        "tin" | "ti" => [140, 100, 160],
        "resist" | "pr" => [230, 180, 60],
        "silicide" | "cosi2" | "nisi" => [100, 70, 120],
        _ => {
            // A stable colour from the name.
            let h = material.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
            [80 + (h % 150) as u8, 80 + ((h >> 8) % 150) as u8, 80 + ((h >> 16) % 150) as u8]
        }
    }
}

/// A cut line through the layout (µm, layout coordinates).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Cut {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Cut {
    pub fn length(&self) -> f64 {
        (self.x1 - self.x0).hypot(self.y1 - self.y0)
    }

    /// Parse `x0,y0,x1,y1`.
    pub fn parse(s: &str) -> Result<Cut, String> {
        let v: Vec<f64> = s
            .split(',')
            .map(|t| t.trim().parse::<f64>().map_err(|_| format!("cut '{s}': write x0,y0,x1,y1 in µm")))
            .collect::<Result<_, _>>()?;
        match v.as_slice() {
            [x0, y0, x1, y1] => {
                let c = Cut { x0: *x0, y0: *y0, x1: *x1, y1: *y1 };
                if c.length() > 0.0 { Ok(c) } else { Err("the cut has no length".into()) }
            }
            _ => Err(format!("cut '{s}': write x0,y0,x1,y1 in µm")),
        }
    }
}

/// An emulated cross-section: a grid of material cells.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    /// Material names; cell value `k` is `materials[k - 1]`, 0 is air.
    pub materials: Vec<String>,
    pub nx: usize,
    pub nz: usize,
    /// Cell size (µm).
    pub h: f64,
    /// z of the bottom of row 0 (µm).
    pub z0: f64,
    /// Cells, row-major from the bottom: `cells[iz * nx + ix]`.
    pub cells: Vec<u8>,
}

/// Squared Euclidean distance transform (in cells) of `seed` on an `nx` × `nz` grid.
fn edt(seed: &[bool], nx: usize, nz: usize) -> Vec<f64> {
    const INF: f64 = 1e20;
    fn pass(f: &[f64], d: &mut [f64], v: &mut [usize], z: &mut [f64]) {
        let n = f.len();
        let mut k = 0usize;
        v[0] = 0;
        z[0] = -INF;
        z[1] = INF;
        for q in 1..n {
            loop {
                let p = v[k];
                let s = ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64)) / (2.0 * q as f64 - 2.0 * p as f64);
                if s <= z[k] && k > 0 {
                    k -= 1;
                    continue;
                }
                if s <= z[k] {
                    // k == 0: replace the first parabola.
                    v[0] = q;
                    z[0] = -INF;
                    z[1] = INF;
                    break;
                }
                k += 1;
                v[k] = q;
                z[k] = s;
                z[k + 1] = INF;
                break;
            }
        }
        k = 0;
        for (q, dq) in d.iter_mut().enumerate() {
            while z[k + 1] < q as f64 {
                k += 1;
            }
            let p = v[k];
            *dq = (q as f64 - p as f64).powi(2) + f[p];
        }
    }
    let mut g: Vec<f64> = seed.iter().map(|s| if *s { 0.0 } else { INF }).collect();
    let n = nx.max(nz);
    let (mut v, mut z) = (vec![0usize; n], vec![0.0; n + 1]);
    let (mut f, mut d) = (vec![0.0; n], vec![0.0; n]);
    for iz in 0..nz {
        f[..nx].copy_from_slice(&g[iz * nx..(iz + 1) * nx]);
        pass(&f[..nx], &mut d[..nx], &mut v, &mut z);
        g[iz * nx..(iz + 1) * nx].copy_from_slice(&d[..nx]);
    }
    for ix in 0..nx {
        for iz in 0..nz {
            f[iz] = g[iz * nx + ix];
        }
        pass(&f[..nz], &mut d[..nz], &mut v, &mut z);
        for iz in 0..nz {
            g[iz * nx + ix] = d[iz];
        }
    }
    g
}

impl Section {
    fn id(&mut self, material: &str) -> u8 {
        match self.materials.iter().position(|m| m == material) {
            Some(k) => k as u8 + 1,
            None => {
                self.materials.push(material.to_string());
                self.materials.len() as u8
            }
        }
    }

    fn ids(&self, materials: &[String]) -> Vec<u8> {
        materials.iter().filter_map(|m| self.materials.iter().position(|x| x == m).map(|k| k as u8 + 1)).collect()
    }

    /// Material at column `ix`, row `iz` (`None` for air).
    pub fn material(&self, ix: usize, iz: usize) -> Option<&str> {
        let c = self.cells[iz * self.nx + ix];
        (c > 0).then(|| self.materials[c as usize - 1].as_str())
    }

    /// Row of the highest solid cell of column `ix`.
    fn top(&self, ix: usize) -> Option<usize> {
        (0..self.nz).rev().find(|&iz| self.cells[iz * self.nx + ix] != 0)
    }

    /// z (µm) of the top of row `iz`.
    pub fn z_top(&self, iz: usize) -> f64 {
        self.z0 + (iz + 1) as f64 * self.h
    }

    /// The material stack of the column at `x` (µm along the cut), bottom up, as (material,
    /// thickness µm); air gaps inside the stack are listed as "air".
    pub fn stack_at(&self, x: f64) -> Vec<(String, f64)> {
        let ix = ((x / self.h).floor().max(0.0) as usize).min(self.nx - 1);
        let top = self.top(ix).unwrap_or(0);
        let mut out: Vec<(String, f64)> = vec![];
        for iz in 0..=top {
            let m = self.material(ix, iz).unwrap_or("air").to_string();
            match out.last_mut() {
                Some((n, t)) if *n == m => *t += self.h,
                _ => out.push((m, self.h)),
            }
        }
        out
    }

    /// Height (µm) of the surface at `x`.
    pub fn surface_at(&self, x: f64) -> f64 {
        let ix = ((x / self.h).floor().max(0.0) as usize).min(self.nx - 1);
        self.top(ix).map_or(self.z0, |iz| self.z_top(iz))
    }

    /// The cells merged into rectangles per material: (material index, [x0, z0, x1, z1] µm).
    pub fn rects(&self) -> Vec<(usize, [f64; 4])> {
        // Runs per row, then runs with the same span stacked over consecutive rows.
        let mut open: Vec<(u8, usize, usize, usize)> = vec![]; // (id, x0, x1, first row)
        let mut out = vec![];
        let h = self.h;
        let mut flush = |id: u8, x0: usize, x1: usize, r0: usize, r1: usize| {
            out.push((id as usize - 1, [x0 as f64 * h, self.z0 + r0 as f64 * h, x1 as f64 * h, self.z0 + r1 as f64 * h]));
        };
        for iz in 0..=self.nz {
            let mut runs = vec![];
            if iz < self.nz {
                let row = &self.cells[iz * self.nx..(iz + 1) * self.nx];
                let mut ix = 0;
                while ix < self.nx {
                    let id = row[ix];
                    let s = ix;
                    while ix < self.nx && row[ix] == id {
                        ix += 1;
                    }
                    if id != 0 {
                        runs.push((id, s, ix));
                    }
                }
            }
            let mut next = vec![];
            for o in open.drain(..) {
                if let Some(k) = runs.iter().position(|r| (r.0, r.1, r.2) == (o.0, o.1, o.2)) {
                    runs.remove(k);
                    next.push(o);
                } else {
                    flush(o.0, o.1, o.2, o.3, iz);
                }
            }
            next.extend(runs.into_iter().map(|r| (r.0, r.1, r.2, iz)));
            open = next;
        }
        out
    }

    /// The section as SVG, `scale` pixels per µm.
    pub fn svg(&self, flow: &ProcessFlow, scale: f64, title: &str) -> String {
        let (w, top) = (self.nx as f64 * self.h, self.z0 + self.nz as f64 * self.h);
        let (pw, ph) = (w * scale, (top - self.z0) * scale);
        let mut s = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{:.0}\" height=\"{:.0}\" viewBox=\"0 0 {pw:.1} {:.1}\" shape-rendering=\"crispEdges\">\n<title>{}</title>\n",
            pw,
            ph + 24.0 + 18.0 * self.materials.len().div_ceil(6) as f64,
            ph + 24.0 + 18.0 * self.materials.len().div_ceil(6) as f64,
            title.replace('&', "&amp;").replace('<', "&lt;")
        );
        s += &format!("<rect width=\"{pw:.1}\" height=\"{ph:.1}\" fill=\"#ffffff\"/>\n");
        for (m, [x0, z0, x1, z1]) in self.rects() {
            let c = flow.color(&self.materials[m]);
            s += &format!(
                "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" fill=\"#{:02x}{:02x}{:02x}\"/>\n",
                x0 * scale,
                (top - z1) * scale,
                (x1 - x0) * scale,
                (z1 - z0) * scale,
                c[0],
                c[1],
                c[2]
            );
        }
        for (k, m) in self.materials.iter().enumerate() {
            let (x, y) = ((k % 6) as f64 * 110.0 + 4.0, ph + 8.0 + (k / 6) as f64 * 18.0);
            let c = flow.color(m);
            s += &format!(
                "<rect x=\"{x:.0}\" y=\"{y:.0}\" width=\"12\" height=\"12\" fill=\"#{:02x}{:02x}{:02x}\" stroke=\"#555\"/><text x=\"{:.0}\" y=\"{:.0}\" font-family=\"sans-serif\" font-size=\"12\">{}</text>\n",
                c[0],
                c[1],
                c[2],
                x + 16.0,
                y + 11.0,
                m
            );
        }
        s + "</svg>\n"
    }
}

/// The mask of a layer expression at the column centres `xs` (µm along the cut).
pub type MaskFn<'a> = dyn FnMut(&str, &[f64]) -> Result<Vec<bool>, String> + 'a;

/// Run the first `steps` steps of `flow` (all with `None`) on a cut of `length` µm, the mask of
/// a layer expression given per column by `mask`.
pub fn emulate(flow: &ProcessFlow, length: f64, steps: Option<usize>, mask: &mut MaskFn) -> Result<Section, String> {
    let h = flow.resolution;
    let nx = ((length / h).round() as usize).max(1);
    let z0 = -flow.substrate_depth;
    let nz = (((flow.substrate_depth + flow.deposited()) / h).ceil() as usize + 4).max(2);
    if nx.saturating_mul(nz) > 40_000_000 {
        return Err(format!("{nx} x {nz} cells: use a coarser resolution or a shorter cut"));
    }
    let mut s = Section { materials: vec![], nx, nz, h, z0, cells: vec![0; nx * nz] };
    let sub = s.id(&flow.substrate);
    let surface = (flow.substrate_depth / h).round() as usize; // rows of substrate
    for iz in 0..surface.min(nz) {
        s.cells[iz * nx..(iz + 1) * nx].fill(sub);
    }
    let xs: Vec<f64> = (0..nx).map(|ix| (ix as f64 + 0.5) * h).collect();
    let open = |m: &Mask, mask: &mut MaskFn| -> Result<Vec<bool>, String> {
        match (&m.mask, &m.keep) {
            (Some(e), _) => mask(e, &xs),
            (None, Some(e)) => Ok(mask(e, &xs)?.into_iter().map(|b| !b).collect()),
            (None, None) => Ok(vec![true; xs.len()]),
        }
    };
    let n = steps.unwrap_or(flow.steps.len()).min(flow.steps.len());
    for step in &flow.steps[..n] {
        match step {
            Step::Deposit { material, thickness, conformal, .. } => {
                let id = s.id(material);
                let t = thickness / h;
                if *conformal {
                    let seed: Vec<bool> = s.cells.iter().map(|c| *c != 0).collect();
                    let d = edt(&seed, nx, nz);
                    let r2 = (t + 1e-9) * (t + 1e-9);
                    for (c, d) in s.cells.iter_mut().zip(&d) {
                        if *c == 0 && *d <= r2 {
                            *c = id;
                        }
                    }
                } else {
                    let rows = t.round().max(1.0) as usize;
                    for ix in 0..nx {
                        let start = s.top(ix).map_or(0, |iz| iz + 1);
                        for iz in start..(start + rows).min(nz) {
                            s.cells[iz * nx + ix] = id;
                        }
                    }
                }
            }
            Step::Etch { mask: m, materials, depth, isotropic, .. } => {
                let cols = open(m, mask)?;
                let ids = s.ids(materials);
                if *isotropic {
                    let d = depth.unwrap_or(0.0) / h;
                    // Seed: the air above the surface of the open columns.
                    let mut seed = vec![false; nx * nz];
                    for ix in (0..nx).filter(|&ix| cols[ix]) {
                        let start = s.top(ix).map_or(0, |iz| iz + 1);
                        for iz in start..nz {
                            seed[iz * nx + ix] = true;
                        }
                    }
                    let dist = edt(&seed, nx, nz);
                    let r2 = (d + 1e-9) * (d + 1e-9);
                    for (c, dd) in s.cells.iter_mut().zip(&dist) {
                        if *dd <= r2 && ids.contains(c) {
                            *c = 0;
                        }
                    }
                } else {
                    for ix in (0..nx).filter(|&ix| cols[ix]) {
                        let Some(top) = s.top(ix) else { continue };
                        let floor = depth.map_or(0, |d| (top as f64 + 1.0 - d / h).round().max(0.0) as usize);
                        for iz in (floor..=top).rev() {
                            let c = &mut s.cells[iz * nx + ix];
                            if *c == 0 {
                                continue;
                            }
                            if !ids.contains(c) {
                                break;
                            }
                            *c = 0;
                        }
                    }
                }
            }
            Step::Implant { mask: m, material, into, depth, .. } => {
                let cols = open(m, mask)?;
                let ids = s.ids(into);
                let id = s.id(material);
                let rows = (depth / h).round() as usize;
                for ix in (0..nx).filter(|&ix| cols[ix]) {
                    let Some(top) = s.top(ix) else { continue };
                    for iz in (top + 1).saturating_sub(rows)..=top {
                        let c = &mut s.cells[iz * nx + ix];
                        if ids.contains(c) {
                            *c = id;
                        }
                    }
                }
            }
            Step::Planarize { height, stop, .. } => {
                let cut_row = match (height, stop) {
                    (Some(z), _) => ((z - z0) / h).round().max(0.0) as usize,
                    (None, Some(m)) => {
                        let ids = s.ids(std::slice::from_ref(m));
                        let top = (0..nz).rev().find(|&iz| (0..nx).any(|ix| ids.contains(&s.cells[iz * nx + ix])));
                        match top {
                            Some(t) => t + 1,
                            None => continue,
                        }
                    }
                    _ => continue,
                };
                for c in s.cells.iter_mut().skip(cut_row * nx) {
                    *c = 0;
                }
            }
            Step::Strip { material, .. } => {
                let ids = s.ids(std::slice::from_ref(material));
                for c in s.cells.iter_mut() {
                    if ids.contains(c) {
                        *c = 0;
                    }
                }
            }
        }
    }
    Ok(s)
}

/// Emulate `flow` on the layout of `cell` along `cut` (µm, layout coordinates).
pub fn emulate_cell(
    project: &Project,
    cell: &CellRef,
    flow: &ProcessFlow,
    cut: &Cut,
    steps: Option<usize>,
) -> Result<Section, String> {
    let px = PixelLayout::new(project, cell, None)?;
    let u = project.tech.dbu_per_um as f64;
    let (fx, fy) = (px.frame.x0 as f64 / u, px.frame.y0 as f64 / u);
    let len = cut.length();
    let (dx, dy) = ((cut.x1 - cut.x0) / len, (cut.y1 - cut.y0) / len);
    let mut cache: Vec<(String, Vec<[f64; 4]>)> = vec![];
    let mut mask = |expr: &str, xs: &[f64]| -> Result<Vec<bool>, String> {
        let rects = match cache.iter().find(|(e, _)| e == expr) {
            Some((_, r)) => r.clone(),
            None => {
                let r: Vec<[f64; 4]> =
                    px.rects_um(expr)?.into_iter().map(|r| [r[0] + fx, r[1] + fy, r[2] + fx, r[3] + fy]).collect();
                cache.push((expr.to_string(), r.clone()));
                r
            }
        };
        Ok(xs
            .iter()
            .map(|t| {
                let (x, y) = (cut.x0 + dx * t, cut.y0 + dy * t);
                rects.iter().any(|r| x >= r[0] && x < r[2] && y >= r[1] && y < r[3])
            })
            .collect())
    };
    emulate(flow, len, steps, &mut mask)
}

/// A horizontal cut across a cell's layout through the middle of its largest diffusion
/// rectangle (across the gates), or through the middle of the layout.
pub fn default_cut(project: &Project, cell: &CellRef) -> Result<Cut, String> {
    let px = PixelLayout::new(project, cell, None)?;
    let u = project.tech.dbu_per_um as f64;
    let f = px.frame;
    let diff = project.tech.diffusion_layers().first().and_then(|&l| project.tech.layer(l)).map(|l| l.name.clone());
    let largest = diff
        .and_then(|d| px.region(&d).ok())
        .and_then(|r| r.rects().iter().max_by_key(|r| (r.x1 - r.x0) as i128 * (r.y1 - r.y0) as i128).copied());
    let y = match largest {
        Some(r) => (r.y0 + r.y1) as f64 / 2.0 / u,
        None => (f.y0 + f.y1) as f64 / 2.0 / u,
    };
    Ok(Cut { x0: f.x0 as f64 / u, y0: y, x1: f.x1 as f64 / u, y1: y })
}

/// The process flow of the generic 180 nm technology.
pub const GENERIC180: &str = include_str!("../../../process/generic180.toml");

#[cfg(test)]
mod tests {
    use super::*;

    /// Masks from intervals along the cut.
    fn masks(defs: &[(&str, &[(f64, f64)])]) -> impl FnMut(&str, &[f64]) -> Result<Vec<bool>, String> {
        let defs: Vec<(String, Vec<(f64, f64)>)> = defs.iter().map(|(n, iv)| (n.to_string(), iv.to_vec())).collect();
        move |e: &str, xs: &[f64]| {
            let iv = &defs.iter().find(|(n, _)| n == e).ok_or(format!("no mask {e}"))?.1;
            Ok(xs.iter().map(|x| iv.iter().any(|(a, b)| x >= a && x < b)).collect())
        }
    }

    fn flow(steps: &str) -> ProcessFlow {
        ProcessFlow::from_toml(&format!("resolution = 0.01\nsubstrate_depth = 0.5\n{steps}")).unwrap()
    }

    fn thickness(s: &Section, x: f64, m: &str) -> f64 {
        s.stack_at(x).iter().filter(|(n, _)| n == m).map(|(_, t)| t).sum()
    }

    #[test]
    fn sti_and_gate() {
        let f = flow(
            r#"
[[step]]
op = "etch"
keep = "active"
materials = ["si"]
depth = 0.3
[[step]]
op = "deposit"
material = "sio2"
thickness = 0.4
conformal = true
[[step]]
op = "planarize"
stop = "si"
[[step]]
op = "deposit"
material = "poly"
thickness = 0.2
[[step]]
op = "etch"
keep = "poly"
materials = ["poly"]
[[step]]
op = "implant"
mask = "active"
material = "n+"
into = ["si"]
depth = 0.1
"#,
        );
        let mut m = masks(&[("active", &[(0.5, 2.5)]), ("poly", &[(1.3, 1.7)])]);
        let s = emulate(&f, 3.0, None, &mut m).unwrap();
        // STI oxide 0.3 deep beside the active area, flush with the silicon.
        assert!((thickness(&s, 0.2, "sio2") - 0.3).abs() < 0.011, "{:?}", s.stack_at(0.2));
        assert!((s.surface_at(0.2) - 0.0).abs() < 0.011);
        // The gate: poly on silicon, no implant under it; n+ beside it.
        assert!((thickness(&s, 1.5, "poly") - 0.2).abs() < 0.011, "{:?}", s.stack_at(1.5));
        assert_eq!(thickness(&s, 1.5, "n+"), 0.0);
        assert!((thickness(&s, 1.0, "n+") - 0.1).abs() < 0.011, "{:?}", s.stack_at(1.0));
        assert_eq!(thickness(&s, 1.0, "poly"), 0.0);
        // Stopping after the first step leaves the open trench.
        let s1 = emulate(&f, 3.0, Some(1), &mut m).unwrap();
        assert!((s1.surface_at(0.2) + 0.3).abs() < 0.011);
        assert_eq!(s1.materials, ["si"]);
        // Rectangles cover every solid cell exactly once.
        let area: f64 = s.rects().iter().map(|(_, r)| (r[2] - r[0]) * (r[3] - r[1])).sum();
        let cells = s.cells.iter().filter(|c| **c != 0).count() as f64 * 0.01 * 0.01;
        assert!((area - cells).abs() < 1e-9, "{area} vs {cells}");
        assert!(s.svg(&f, 100.0, "t").contains("<rect"));
    }

    #[test]
    fn conformal_deposition_makes_spacers() {
        let f = flow(
            r#"
[[step]]
op = "deposit"
material = "poly"
thickness = 0.2
[[step]]
op = "etch"
keep = "poly"
materials = ["poly"]
[[step]]
op = "deposit"
material = "si3n4"
thickness = 0.05
conformal = true
[[step]]
op = "etch"
materials = ["si3n4"]
depth = 0.05
"#,
        );
        let mut m = masks(&[("poly", &[(1.0, 1.4)])]);
        let s = emulate(&f, 2.4, None, &mut m).unwrap();
        // Spacers stand beside the gate, nothing is left on the flat parts or the gate top.
        assert!(thickness(&s, 0.98, "si3n4") > 0.15, "{:?}", s.stack_at(0.98));
        assert!(thickness(&s, 1.42, "si3n4") > 0.15, "{:?}", s.stack_at(1.42));
        assert_eq!(thickness(&s, 0.5, "si3n4"), 0.0);
        assert_eq!(thickness(&s, 1.2, "si3n4"), 0.0);
        // Conformal coverage of a trench: the wall is coated to the thickness.
        let g = flow(
            r#"
[[step]]
op = "etch"
mask = "trench"
materials = ["si"]
depth = 0.3
[[step]]
op = "deposit"
material = "sio2"
thickness = 0.05
conformal = true
"#,
        );
        let mut m = masks(&[("trench", &[(1.0, 1.5)])]);
        let s = emulate(&g, 2.5, None, &mut m).unwrap();
        assert!((thickness(&s, 1.02, "sio2") - 0.3).abs() < 0.06, "the wall: {:?}", s.stack_at(1.02));
        assert!((thickness(&s, 1.25, "sio2") - 0.05).abs() < 0.011, "the floor: {:?}", s.stack_at(1.25));
    }

    #[test]
    fn isotropic_etch_undercuts() {
        let f = flow(
            r#"
[[step]]
op = "deposit"
material = "resist"
thickness = 0.1
[[step]]
op = "etch"
mask = "open"
materials = ["resist"]
[[step]]
op = "etch"
mask = "open"
materials = ["si"]
depth = 0.2
isotropic = true
[[step]]
op = "strip"
material = "resist"
"#,
        );
        let mut m = masks(&[("open", &[(1.0, 1.2)])]);
        let s = emulate(&f, 2.2, None, &mut m).unwrap();
        // The opening is etched 0.2 deep and 0.2 under the resist edge.
        assert!((s.surface_at(1.1) + 0.2).abs() < 0.011, "{}", s.surface_at(1.1));
        assert!(s.surface_at(0.85) < -0.05, "undercut: {}", s.surface_at(0.85));
        assert!(s.surface_at(0.7).abs() < 0.011);
        assert!(!s.materials.iter().any(|m| m == "resist") || thickness(&s, 0.5, "resist") == 0.0);
    }

    #[test]
    fn generic_flow_on_the_inverter() {
        let f = ProcessFlow::from_toml(GENERIC180).unwrap();
        let p = crate::examples::demo_project();
        let r = CellRef::new("work", "cmos_inverter");
        let cut = default_cut(&p, &r).unwrap();
        let s = emulate_cell(&p, &r, &f, &cut, None).unwrap();
        for m in ["si", "sio2", "poly", "al"] {
            assert!(s.materials.iter().any(|x| x == m), "{m} missing: {:?}", s.materials);
        }
        assert!(f.mask_exprs().iter().any(|e| e == "active"));
        assert!(ProcessFlow::from_toml("[[step]]\nop = \"deposit\"\nmaterial = \"x\"\nthickness = -1\n").is_err());
        assert!(Cut::parse("0,1,2").is_err() && Cut::parse("0,0,0,0").is_err());
    }
}
