//! Pixel geometry from a layout for the pixel optics model: the pixel frame and the shapes of
//! layers or derived layers, as rectangles in µm relative to the frame's lower-left corner.
//!
//! A layer is named as in the technology or given as a derived-layer expression in the DRC deck
//! syntax (see [`crate::drc`]), e.g. `active not interact poly` for diffusion without a gate.

use crate::drc;
use crate::geom::IRect;
use crate::hier::flatten;
use crate::project::{CellRef, Project};
use crate::region::{BoolOp, Region};

/// The flattened layers of a pixel cell.
pub struct PixelLayout<'a> {
    project: &'a Project,
    /// Merged region of each technology layer.
    layers: Vec<Region>,
    /// The pixel frame (dbu).
    pub frame: IRect,
}

impl<'a> PixelLayout<'a> {
    /// The layout of `cell`; the frame is the bounding box of `boundary` (a layer or
    /// expression), else of all shapes.
    pub fn new(project: &'a Project, cell: &CellRef, boundary: Option<&str>) -> Result<PixelLayout<'a>, String> {
        Self::with_params(project, cell, &[], boundary)
    }

    /// [`Self::new`] for the variant of PCell Designer master `cell` with parameter values
    /// `params` (µm), generated on the fly; no values read the cell's own layout.
    pub fn with_params(
        project: &'a Project,
        cell: &CellRef,
        params: &[(String, f64)],
        boundary: Option<&str>,
    ) -> Result<PixelLayout<'a>, String> {
        let c = project.cell(cell).ok_or_else(|| format!("no cell {cell}"))?;
        let generated;
        let lay = if params.is_empty() {
            c.layout.as_ref().ok_or_else(|| format!("{cell} has no layout"))?
        } else {
            let Some(crate::pcell::PCell::Template { def, .. }) = &c.pcell else {
                return Err(format!("{cell} is not a PCell Designer master; its layout takes no parameters"));
            };
            generated = def.generate(&project.tech, params).map_err(|e| format!("{cell}: {e}"))?;
            &generated
        };
        Self::of_layout(project, lay, boundary).map_err(|e| format!("{cell}: {e}"))
    }

    fn of_layout(project: &'a Project, lay: &crate::layout::Layout, boundary: Option<&str>) -> Result<PixelLayout<'a>, String> {
        let tech = &project.tech;
        let mut geoms: Vec<Vec<crate::layout::Geometry>> = vec![vec![]; tech.layers.len()];
        for s in flatten(lay, project, 64) {
            if s.layer < geoms.len() {
                geoms[s.layer].push(s.geom);
            }
        }
        // Any-angle shapes (45° corners, diagonal gates) as a staircase of grid-high bands.
        let step = tech.grid.max(10);
        let layers = geoms
            .iter()
            .map(|g| {
                Region::from_rects(g.iter().flat_map(|g| {
                    let r = Region::from_geometry(g)
                        .unwrap_or_else(|_| crate::polybool::PolySet::from_geometry(g).manhattanize(step));
                    r.rects().to_vec()
                }))
            })
            .collect::<Vec<_>>();
        let mut px = PixelLayout { project, layers, frame: IRect { x0: 0, y0: 0, x1: 0, y1: 0 } };
        let all = Region::from_rects(px.layers.iter().flat_map(|r| r.rects().iter().copied()));
        let frame = match boundary {
            Some(b) => px.region(b)?.bbox(),
            None => all.bbox(),
        };
        px.frame = frame.ok_or_else(|| "the layout is empty".to_string())?;
        Ok(px)
    }

    /// The region of a layer name or derived-layer expression, clipped to the frame.
    pub fn region(&self, expr: &str) -> Result<Region, String> {
        let tech = &self.project.tech;
        let r = drc::derived_region(expr, tech, &self.layers)?;
        if self.frame.x1 > self.frame.x0 { Ok(r.boolean(&Region::from_rects([self.frame]), BoolOp::And)) } else { Ok(r) }
    }

    /// Frame width and height (µm).
    pub fn size_um(&self) -> (f64, f64) {
        let u = self.project.tech.dbu_per_um as f64;
        ((self.frame.x1 - self.frame.x0) as f64 / u, (self.frame.y1 - self.frame.y0) as f64 / u)
    }

    /// The disjoint rectangles of `expr` as [x0, y0, x1, y1] in µm from the frame's corner.
    pub fn rects_um(&self, expr: &str) -> Result<Vec<[f64; 4]>, String> {
        let u = self.project.tech.dbu_per_um as f64;
        let (fx, fy) = (self.frame.x0, self.frame.y0);
        Ok(self
            .region(expr)?
            .rects()
            .iter()
            .map(|r| [(r.x0 - fx) as f64 / u, (r.y0 - fy) as f64 / u, (r.x1 - fx) as f64 / u, (r.y1 - fy) as f64 / u])
            .collect())
    }
}

/// Write a device simulation's photodiode figures into the technology's cross-section
/// photodiode (the pinned photodiode model of the circuit simulation): the pinning voltage
/// `vpin` (V) and the storage capacitance `cap_ff` (fF per photodiode) as `cap_area`, per area
/// of the technology's photodiode layer in `cell`'s layout (`pixels` photodiodes in its frame;
/// the variant of `params` for a PCell Designer master). Returns a summary of the change.
pub fn apply_device_photodiode(
    project: &mut Project,
    cell: &CellRef,
    params: &[(String, f64)],
    boundary: Option<&str>,
    pixels: usize,
    vpin: f64,
    cap_ff: f64,
) -> Result<String, String> {
    if !(vpin.is_finite() && cap_ff.is_finite() && cap_ff > 0.0 && pixels > 0) {
        return Err("no pinning voltage or storage capacitance to apply".into());
    }
    let layer = project
        .tech
        .cross_section
        .as_ref()
        .and_then(|c| c.photodiode.as_ref())
        .map(|d| d.layer.clone())
        .ok_or("the technology has no cross-section photodiode to set")?;
    let area = {
        let px = PixelLayout::with_params(project, cell, params, boundary)?;
        px.region(&layer)?.area() as f64 / (project.tech.dbu_per_um as f64).powi(2) / pixels as f64
    };
    if area <= 0.0 {
        return Err(format!("{cell}: no photodiode ('{layer}') in the layout"));
    }
    let def = project.tech.cross_section.as_mut().and_then(|c| c.photodiode.as_mut()).unwrap();
    let old = (def.pinning, def.cap_area);
    def.pinning = Some((vpin * 1000.0).round() / 1000.0);
    def.cap_area = Some((cap_ff / area * 1e4).round() / 1e4);
    def.cap_perimeter = None;
    let f = |v: Option<f64>| v.map_or("unset".to_string(), |v| format!("{v}"));
    Ok(format!(
        "technology photodiode: pinning {} -> {} V, cap_area {} -> {} fF/um^2 ({cap_ff:.3} fF over {area:.3} um^2 of '{layer}')",
        f(old.0),
        f(def.pinning),
        f(old.1),
        f(def.cap_area)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_pixel() {
        let p = crate::examples::demo_project();
        let px = PixelLayout::new(&p, &CellRef::new("work", "aps4t"), Some("boundary")).unwrap();
        let (w, h) = px.size_um();
        assert!((w - 5.6).abs() < 1e-9 && (h - 5.6).abs() < 1e-9, "{w} x {h}");
        // The photodiode is the wide diffusion (the transfer-gate neck and the strip are narrower):
        // one 4.75 x 2.7 µm rectangle.
        let pd = px.rects_um("active wide 1").unwrap();
        let area: f64 = pd.iter().map(|r| (r[2] - r[0]) * (r[3] - r[1])).sum();
        assert!((area - 4.75 * 2.7).abs() < 1e-6, "{pd:?}");
        assert!(pd.iter().all(|r| r[0] >= 0.0 && r[1] >= 0.0 && r[2] <= w && r[3] <= h));
        // Metal shapes come as disjoint rectangles inside the frame.
        let m2 = px.rects_um("metal2").unwrap();
        assert!(!m2.is_empty());
        assert!(px.rects_um("nosuch and metal1").is_err());
        assert!(PixelLayout::new(&p, &CellRef::new("work", "rc_lowpass"), None).is_err());
    }

    #[test]
    fn device_figures_into_the_technology() {
        let mut p = crate::examples::demo_project();
        let cell = CellRef::new("work", "bayer_fdti");
        let msg = apply_device_photodiode(&mut p, &cell, &[], Some("boundary"), 4, 1.6937, 0.741).unwrap();
        let pd = p.tech.cross_section.as_ref().unwrap().photodiode.as_ref().unwrap();
        assert_eq!(pd.pinning, Some(1.694));
        // 0.741 fF over each 1.05 x 1.05 µm photodiode.
        assert!((pd.cap_area.unwrap() - 0.741 / (1.05 * 1.05)).abs() < 1e-4, "{msg}");
        assert!((pd.storage_cap(1.05 * 1.05, 4.2) - 0.741e-15).abs() < 1e-19);
        assert!(apply_device_photodiode(&mut p, &cell, &[], Some("boundary"), 4, f64::NAN, 0.7).is_err());
    }
}
