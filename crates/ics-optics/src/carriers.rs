//! Carrier collection in the silicon of a pixel array: which photodiode collects the carriers
//! generated at each point, from minority-carrier diffusion.
//!
//! The collection probability P(r) of a photodiode solves the steady-state diffusion equation
//! ∇²P - P/L² = 0 (diffusion length L) in the neutral silicon, with P = 1 in the photodiode's
//! depletion region and P = 0 in every other photodiode's, P = 0 at the bottom of the epitaxial
//! layer (carriers lost to the substrate), no flux or P = 0 at the surface (reflecting or
//! recombining) and no flux through deep-trench isolation walls. By reciprocity (Donolato) a
//! photodiode's signal is ∫ G(r) P(r) dV for a generation rate G.
//!
//! The grid covers one 2 × 2 Bayer cell with periodic sides. All pixels share the photodiode
//! layout, so one solve for the pixel at the cell's origin gives every pixel's probability by
//! translation.

use serde::Deserialize;

/// Deep-trench isolation walls on the pixel boundaries.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dti {
    /// Depth from the surface (µm).
    pub depth: f64,
    /// Width centred on the pixel boundary (µm).
    pub width: f64,
}

/// Electrical model of the silicon (the `[silicon]` section of a stack).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Silicon {
    /// Minority-carrier diffusion length in the neutral silicon (µm).
    #[serde(default = "default_l")]
    pub diffusion_length: f64,
    /// Thickness of the epitaxial layer: carriers reaching its bottom are lost (µm); default the
    /// photodiode's bottom plus 2 µm.
    #[serde(default)]
    pub epi: Option<f64>,
    /// `reflecting` (passivated surface, default) or `recombining`.
    #[serde(default = "reflecting")]
    pub surface: String,
    #[serde(default)]
    pub dti: Option<Dti>,
    /// Grid step (µm); default about 1/16 of the pitch, at most 0.1 µm deep.
    #[serde(default)]
    pub grid: Option<f64>,
}

fn default_l() -> f64 {
    20.0
}

fn reflecting() -> String {
    "reflecting".into()
}

impl Default for Silicon {
    fn default() -> Self {
        Silicon { diffusion_length: default_l(), epi: None, surface: reflecting(), dti: None, grid: None }
    }
}

/// A voxel grid over the Bayer cell (2p × 2p) and the epitaxial depth.
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    /// Steps (µm).
    pub dx: f64,
    pub dz: f64,
}

impl Grid {
    pub fn len(&self) -> usize {
        self.nx * self.ny * self.nz
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn index(&self, i: usize, j: usize, k: usize) -> usize {
        (k * self.ny + j) * self.nx + i
    }

    /// Centre of voxel (i, j, k): x, y and depth (µm).
    pub fn centre(&self, i: usize, j: usize, k: usize) -> (f64, f64, f64) {
        ((i as f64 + 0.5) * self.dx, (j as f64 + 0.5) * self.dx, (k as f64 + 0.5) * self.dz)
    }

    /// Voxel holding (x, y) of the cell (periodic) at `depth`; None below the grid.
    pub fn voxel(&self, x: f64, y: f64, depth: f64) -> Option<usize> {
        if !(0.0..self.nz as f64 * self.dz).contains(&depth) {
            return None;
        }
        let w = self.nx as f64 * self.dx;
        let i = ((x.rem_euclid(w) / self.dx) as usize).min(self.nx - 1);
        let j = ((y.rem_euclid(w) / self.dx) as usize).min(self.ny - 1);
        let k = ((depth / self.dz) as usize).min(self.nz - 1);
        Some(self.index(i, j, k))
    }
}

/// The collection model of a stack: grid and the probability of the photodiode of the pixel
/// at the cell's origin.
#[derive(Debug, Clone)]
pub struct Collection {
    pub grid: Grid,
    /// P per voxel for pixel (0, 0).
    pub p: Vec<f64>,
    pitch: f64,
}

#[derive(Clone, Copy, PartialEq)]
enum Cell {
    Free,
    /// Fixed probability (inside a photodiode).
    Fixed(f64),
    /// Inside an isolation wall.
    Wall,
}

impl Collection {
    /// Solve the collection probability for pitch `p`, photodiode area `pd_rects` per pixel
    /// (empty: whole pixel) and depth range `pd_depth` (µm).
    pub fn solve(si: &Silicon, p: f64, pd_rects: &[[f64; 4]], pd_depth: (f64, f64)) -> Result<Collection, String> {
        if si.diffusion_length.is_nan() || si.diffusion_length <= 0.0 {
            return Err("silicon: diffusion_length must be positive".into());
        }
        let epi = si.epi.unwrap_or(pd_depth.1 + 2.0);
        if epi <= pd_depth.1 {
            return Err(format!("silicon: the epitaxial layer ({epi} µm) must be deeper than the photodiode"));
        }
        let recombining = match si.surface.to_ascii_lowercase().as_str() {
            "reflecting" => false,
            "recombining" => true,
            other => return Err(format!("silicon: surface '{other}' (reflecting, recombining)")),
        };
        let h = si.grid.unwrap_or((p / 16.0).min(0.25));
        // An even count puts the pixel boundaries on voxel faces.
        let nx = (((2.0 * p / h / 2.0).round() as usize) * 2).max(4);
        let dx = 2.0 * p / nx as f64;
        let dz = h.min(0.1).min(pd_depth.1.max(0.2) / 8.0);
        let nz = ((epi / dz).round() as usize).max(4);
        let dz = epi / nz as f64;
        let grid = Grid { nx, ny: nx, nz, dx, dz };
        if grid.len() > 4_000_000 {
            return Err(format!("silicon: {} voxels; use a coarser grid", grid.len()));
        }

        // Classify voxels.
        let whole = [[0.0, 0.0, p, p]];
        let pd: &[[f64; 4]] = if pd_rects.is_empty() { &whole } else { pd_rects };
        let in_pd = |x: f64, y: f64| {
            let (lx, ly) = (x.rem_euclid(p), y.rem_euclid(p));
            pd.iter().any(|r| lx >= r[0] && lx < r[2] && ly >= r[1] && ly < r[3])
        };
        // Walls wider than a voxel fill voxels; every wall also blocks the faces on the pixel
        // boundaries (thin trenches).
        let on_wall = |x: f64, y: f64, z: f64| {
            si.dti.as_ref().is_some_and(|d| {
                let near = |v: f64| {
                    let r = v.rem_euclid(p);
                    r < d.width / 2.0 || r > p - d.width / 2.0
                };
                z < d.depth && (near(x) || near(y))
            })
        };
        let dti_depth = si.dti.as_ref().map_or(0.0, |d| d.depth);
        let half = nx / 2;
        // Whether the face between voxel column `a` and `a + 1` (mod nx) is a pixel boundary.
        let boundary_face = |a: usize| (a + 1).is_multiple_of(half);
        let mut cells = vec![Cell::Free; grid.len()];
        for k in 0..nz {
            for j in 0..nx {
                for i in 0..nx {
                    let (x, y, z) = grid.centre(i, j, k);
                    let c = if on_wall(x, y, z) {
                        Cell::Wall
                    } else if z >= pd_depth.0 && z < pd_depth.1 && in_pd(x, y) {
                        let own = (x / p).floor() as i64 == 0 && (y / p).floor() as i64 == 0;
                        Cell::Fixed(if own { 1.0 } else { 0.0 })
                    } else {
                        Cell::Free
                    };
                    cells[grid.index(i, j, k)] = c;
                }
            }
        }

        // A x = b on the free voxels: (Σ face weights + 1/L²) P - Σ w P_nb = fixed neighbours.
        let (wx, wz) = (1.0 / (dx * dx), 1.0 / (dz * dz));
        let sink = 1.0 / (si.diffusion_length * si.diffusion_length);
        let neighbours = |i: usize, j: usize, k: usize| -> [(Option<usize>, f64); 6] {
            let z = (k as f64 + 0.5) * dz;
            let walled = |a: usize| z < dti_depth && boundary_face(a);
            let xm = (!walled((i + nx - 1) % nx)).then(|| grid.index((i + nx - 1) % nx, j, k));
            let xp = (!walled(i)).then(|| grid.index((i + 1) % nx, j, k));
            let ym = (!walled((j + nx - 1) % nx)).then(|| grid.index(i, (j + nx - 1) % nx, k));
            let yp = (!walled(j)).then(|| grid.index(i, (j + 1) % nx, k));
            // Above the surface: none (reflecting) or a P = 0 ghost (recombining, at half a
            // step); below the epi: a P = 0 ghost.
            let zm = if k == 0 { None } else { Some(grid.index(i, j, k - 1)) };
            let zp = if k + 1 == nz { None } else { Some(grid.index(i, j, k + 1)) };
            [(xm, wx), (xp, wx), (ym, wx), (yp, wx), (zm, wz), (zp, wz)]
        };
        let n = grid.len();
        let mut diag = vec![0.0; n];
        let mut b = vec![0.0; n];
        for k in 0..nz {
            for j in 0..nx {
                for i in 0..nx {
                    let v = grid.index(i, j, k);
                    if cells[v] != Cell::Free {
                        continue;
                    }
                    let mut d = sink;
                    for (q, (nb, w)) in neighbours(i, j, k).into_iter().enumerate() {
                        match nb.map(|u| cells[u]) {
                            Some(Cell::Free) => d += w,
                            // The junction edge lies on the shared face, half a step away.
                            Some(Cell::Fixed(val)) => {
                                d += 2.0 * w;
                                b[v] += 2.0 * w * val;
                            }
                            Some(Cell::Wall) => {}
                            // Lateral: an isolation wall on the face. Vertical: the surface
                            // (q = 4) or the epi bottom (q = 5).
                            None if q < 4 => {}
                            None if q == 4 && !recombining => {}
                            None => d += 2.0 * w,
                        }
                    }
                    diag[v] = d;
                }
            }
        }
        let apply = |x: &[f64], out: &mut [f64]| {
            for k in 0..nz {
                for j in 0..nx {
                    for i in 0..nx {
                        let v = grid.index(i, j, k);
                        if cells[v] != Cell::Free {
                            out[v] = 0.0;
                            continue;
                        }
                        let mut s = diag[v] * x[v];
                        for (nb, w) in neighbours(i, j, k) {
                            if let Some(u) = nb
                                && cells[u] == Cell::Free
                            {
                                s -= w * x[u];
                            }
                        }
                        out[v] = s;
                    }
                }
            }
        };
        let x = conjugate_gradient(&apply, &b, &diag, |v| cells[v] == Cell::Free, 1e-10, 20_000);
        let p_out: Vec<f64> = (0..n)
            .map(|v| match cells[v] {
                Cell::Free => x[v].clamp(0.0, 1.0),
                Cell::Fixed(val) => val,
                Cell::Wall => 0.0,
            })
            .collect();
        Ok(Collection { grid, p: p_out, pitch: p })
    }

    /// Collection probability of the photodiode of pixel (pi, pj) of the cell at voxel `v`.
    pub fn prob(&self, pi: usize, pj: usize, v: usize) -> f64 {
        let g = &self.grid;
        let k = v / (g.nx * g.ny);
        let j = (v / g.nx) % g.ny;
        let i = v % g.nx;
        // Shift by one pixel = half the cell.
        let (si, sj) = (pi * g.nx / 2, pj * g.ny / 2);
        self.p[g.index((i + g.nx - si) % g.nx, (j + g.ny - sj) % g.ny, k)]
    }

    pub fn pitch(&self) -> f64 {
        self.pitch
    }
}

/// Jacobi-preconditioned conjugate gradient on the `active` unknowns.
fn conjugate_gradient(
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
    let mut pdir = z.clone();
    let mut rz: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
    let b_norm = r.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-300);
    let mut ap = vec![0.0; n];
    for _ in 0..max_iter {
        apply(&pdir, &mut ap);
        let pap: f64 = pdir.iter().zip(&ap).map(|(a, b)| a * b).sum();
        if pap <= 0.0 {
            break;
        }
        let alpha = rz / pap;
        for v in 0..n {
            x[v] += alpha * pdir[v];
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
            pdir[v] = z[v] + beta * pdir[v];
        }
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_dimensional_collection_matches_the_analytic_profile() {
        // Photodiode over the whole pixel from 0 to 1 µm, neutral silicon to 4 µm, P = 0 at the
        // bottom: P(z) = sinh((epi - z) / L) / sinh((epi - d) / L) below the photodiode.
        let si = Silicon { diffusion_length: 2.0, epi: Some(4.0), grid: Some(0.25), ..Silicon::default() };
        let c = Collection::solve(&si, 1.0, &[], (0.0, 1.0)).unwrap();
        let g = &c.grid;
        for k in 0..g.nz {
            let z = (k as f64 + 0.5) * g.dz;
            if z < 1.0 {
                continue;
            }
            // Lateral uniformity: the sum over the four photodiodes is the 1-D profile.
            let v = g.index(0, 0, k);
            let total: f64 = (0..2).flat_map(|i| (0..2).map(move |j| (i, j))).map(|(i, j)| c.prob(i, j, v)).sum();
            let exact = ((4.0 - z) / 2.0).sinh() / (3.0f64 / 2.0).sinh();
            assert!((total - exact).abs() < 0.02, "z {z}: {total} vs {exact}");
        }
    }

    #[test]
    fn neighbours_share_the_carriers_and_isolation_keeps_them() {
        // A small photodiode in each pixel: carriers generated between them split; with a long
        // diffusion length and a reflecting bottom almost all are collected by someone.
        let pd = [[0.4, 0.4, 1.0, 1.0]];
        let open = Silicon { diffusion_length: 50.0, epi: Some(3.0), grid: Some(0.1), ..Silicon::default() };
        let c = Collection::solve(&open, 1.4, &pd, (0.0, 1.0)).unwrap();
        let g = &c.grid;
        // A point under pixel (0, 0) near its boundary to pixel (1, 0): collected partly by (1, 0).
        let v = g.voxel(1.3, 0.7, 1.5).unwrap();
        let own = c.prob(0, 0, v);
        let nb = c.prob(1, 0, v);
        assert!(own > 0.15 && nb > 0.15, "own {own} neighbour {nb}");
        // Deep trench isolation on the boundaries keeps more of it home.
        let dti = Silicon { dti: Some(Dti { depth: 3.0, width: 0.1 }), ..open.clone() };
        let d = Collection::solve(&dti, 1.4, &pd, (0.0, 1.0)).unwrap();
        let v2 = d.grid.voxel(1.3, 0.7, 1.5).unwrap();
        assert!(d.prob(1, 0, v2) < 0.2 * nb, "{} vs {nb}", d.prob(1, 0, v2));
        assert!(d.prob(0, 0, v2) > own);
        // Probabilities stay within [0, 1] and add up to at most 1.
        for v in 0..g.len() {
            let s: f64 = (0..2).flat_map(|i| (0..2).map(move |j| (i, j))).map(|(i, j)| c.prob(i, j, v)).sum();
            assert!(s <= 1.0 + 1e-6, "{s}");
        }
    }

    #[test]
    fn a_recombining_surface_loses_shallow_carriers() {
        let pd = [[0.0, 0.0, 1.0, 1.0]];
        let mk = |surface: &str| Silicon {
            diffusion_length: 10.0,
            epi: Some(3.0),
            grid: Some(0.1),
            surface: surface.into(),
            ..Silicon::default()
        };
        let refl = Collection::solve(&mk("reflecting"), 1.0, &pd, (0.5, 1.5)).unwrap();
        let rec = Collection::solve(&mk("recombining"), 1.0, &pd, (0.5, 1.5)).unwrap();
        let v = refl.grid.voxel(0.5, 0.5, 0.05).unwrap();
        assert!(rec.prob(0, 0, v) < 0.5 * refl.prob(0, 0, v), "{} vs {}", rec.prob(0, 0, v), refl.prob(0, 0, v));
        assert!(Collection::solve(&mk("sticky"), 1.0, &pd, (0.5, 1.5)).is_err());
    }
}
