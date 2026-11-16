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
        let lay = project.cell(cell).and_then(|c| c.layout.as_ref()).ok_or_else(|| format!("{cell} has no layout"))?;
        let tech = &project.tech;
        let mut geoms: Vec<Vec<crate::layout::Geometry>> = vec![vec![]; tech.layers.len()];
        for s in flatten(lay, project, 64) {
            if s.layer < geoms.len() {
                geoms[s.layer].push(s.geom);
            }
        }
        let layers = geoms
            .iter()
            .map(|g| Region::from_rects(g.iter().filter_map(|g| Region::from_geometry(g).ok()).flat_map(|r| r.rects().to_vec())))
            .collect::<Vec<_>>();
        let mut px = PixelLayout { project, layers, frame: IRect { x0: 0, y0: 0, x1: 0, y1: 0 } };
        let all = Region::from_rects(px.layers.iter().flat_map(|r| r.rects().iter().copied()));
        let frame = match boundary {
            Some(b) => px.region(b)?.bbox(),
            None => all.bbox(),
        };
        px.frame = frame.ok_or_else(|| format!("{cell}: the layout is empty"))?;
        Ok(px)
    }

    /// The region of a layer name or derived-layer expression, clipped to the frame.
    pub fn region(&self, expr: &str) -> Result<Region, String> {
        let tech = &self.project.tech;
        let r = match tech.layer_index(expr.trim()) {
            Some(i) => self.layers[i].clone(),
            None => {
                let deck = drc::parse_deck(&format!("_pixel_ = {expr}\n"), tech)
                    .map_err(|e| e.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("; "))?;
                let d = deck.derived.last().ok_or_else(|| format!("'{expr}' is not a layer expression"))?;
                d.expr.evaluate(&self.layers)
            }
        };
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
}
