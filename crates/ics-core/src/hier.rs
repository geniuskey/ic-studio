//! Layout hierarchy: resolving cell instances, cached per-cell spatial indices, hierarchical
//! bounding boxes, view queries with level-of-detail and flattening.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rstar::{AABB, RTree, RTreeObject};

use crate::geom::IRect;
use crate::layout::{Geometry, Layout, Shape, Trans};
use crate::project::{CellRef, Project};

/// Anything that can resolve a cell reference to its layout.
pub trait LayoutSource {
    fn layout(&self, cell: &CellRef) -> Option<&Layout>;
}

/// A source with no cells: instances stay unresolved.
pub struct NoCells;

impl LayoutSource for NoCells {
    fn layout(&self, _: &CellRef) -> Option<&Layout> {
        None
    }
}

impl LayoutSource for Project {
    fn layout(&self, cell: &CellRef) -> Option<&Layout> {
        self.cell(cell)?.layout.as_ref()
    }
}

impl LayoutSource for HashMap<CellRef, Layout> {
    fn layout(&self, cell: &CellRef) -> Option<&Layout> {
        self.get(cell)
    }
}

#[derive(Clone, Copy)]
struct Entry {
    bbox: IRect,
    idx: u32,
}

impl RTreeObject for Entry {
    type Envelope = AABB<[i64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        aabb(self.bbox)
    }
}

fn aabb(r: IRect) -> AABB<[i64; 2]> {
    AABB::from_corners([r.x0, r.y0], [r.x1, r.y1])
}

/// Spatial index over one cell's shapes (per layer) and instances, plus hierarchical totals.
pub struct CellIndex {
    /// Extent of the cell including everything placed in it.
    pub bbox: Option<IRect>,
    /// Number of shapes in the flattened hierarchy.
    pub flat_shapes: u64,
    /// Cells instantiated directly by this cell.
    pub children: Vec<CellRef>,
    layers: Vec<RTree<Entry>>,
    insts: RTree<Entry>,
    /// Bounding box of each instance's child cell (None if unresolved or empty).
    child_bbox: Vec<Option<IRect>>,
    n_shapes: usize,
    n_insts: usize,
}

impl CellIndex {
    /// Cheap staleness check: an index built for a layout with different item counts is
    /// certainly out of date (other edits must be reported through [`Hier::invalidate`]).
    fn matches(&self, l: &Layout) -> bool {
        self.n_shapes == l.shapes.len() && self.n_insts == l.instances.len()
    }

    /// Placement box of instance `i` (a placeholder box for missing cells).
    pub fn instance_bbox(&self, l: &Layout, i: usize) -> Option<IRect> {
        let inst = l.instances.get(i)?;
        match self.child_bbox(i) {
            Some(cb) => Some(inst.bbox(cb)),
            None => {
                let h = MISSING_CELL_SIZE / 2;
                Some(inst.bbox(IRect::new(-h, -h, h, h)))
            }
        }
    }

    /// Child cell extent for instance `i` of the indexed layout.
    pub fn child_bbox(&self, i: usize) -> Option<IRect> {
        self.child_bbox.get(i).copied().flatten()
    }

    /// Indices into `layout.shapes` on `layer` whose bbox intersects `area`.
    pub fn shapes_in(&self, layer: usize, area: IRect) -> impl Iterator<Item = usize> + '_ {
        self.layers
            .get(layer)
            .into_iter()
            .flat_map(move |t| t.locate_in_envelope_intersecting(aabb(area)).map(|e| e.idx as usize))
    }

    /// Indices into `layout.instances` whose placement box intersects `area`.
    pub fn instances_in(&self, area: IRect) -> impl Iterator<Item = usize> + '_ {
        self.insts.locate_in_envelope_intersecting(aabb(area)).map(|e| e.idx as usize)
    }
}

/// Size of the placeholder drawn for instances of missing cells.
pub const MISSING_CELL_SIZE: i64 = 1000;

/// Cache of per-cell indices. Invalidate a cell whenever its layout changes.
#[derive(Default)]
pub struct Hier {
    cells: HashMap<CellRef, Arc<CellIndex>>,
    building: HashSet<CellRef>,
}

impl Hier {
    pub fn clear(&mut self) {
        self.cells.clear();
    }

    /// Drop the cached index of `cell` and of every cached cell that (transitively) places it.
    pub fn invalidate(&mut self, cell: &CellRef) {
        self.cells.remove(cell);
        let mut gone: HashSet<CellRef> = HashSet::from([cell.clone()]);
        loop {
            let stale: Vec<CellRef> =
                self.cells.iter().filter(|(_, v)| v.children.iter().any(|c| gone.contains(c))).map(|(k, _)| k.clone()).collect();
            if stale.is_empty() {
                break;
            }
            for k in stale {
                self.cells.remove(&k);
                gone.insert(k);
            }
        }
    }

    /// Index of a cell resolved through `src`.
    pub fn get(&mut self, cell: &CellRef, src: &dyn LayoutSource) -> Option<Arc<CellIndex>> {
        let layout = src.layout(cell)?;
        Some(self.get_for(cell, layout, src))
    }

    /// Index of a cell whose layout is supplied directly (e.g. the one being edited).
    pub fn get_for(&mut self, cell: &CellRef, layout: &Layout, src: &dyn LayoutSource) -> Arc<CellIndex> {
        match self.cells.get(cell) {
            Some(i) if i.matches(layout) => i.clone(),
            Some(_) => {
                self.invalidate(cell);
                self.build(cell, layout, src)
            }
            None => self.build(cell, layout, src),
        }
    }

    fn build(&mut self, cell: &CellRef, layout: &Layout, src: &dyn LayoutSource) -> Arc<CellIndex> {
        self.building.insert(cell.clone());
        let mut per_layer: Vec<Vec<Entry>> = vec![];
        let mut bbox: Option<IRect> = None;
        for (i, s) in layout.shapes.iter().enumerate() {
            if per_layer.len() <= s.layer {
                per_layer.resize_with(s.layer + 1, Vec::new);
            }
            let b = s.bbox();
            bbox = Some(bbox.map_or(b, |x| x.union(&b)));
            per_layer[s.layer].push(Entry { bbox: b, idx: i as u32 });
        }
        let mut flat = layout.shapes.len() as u64;
        let mut insts = vec![];
        let mut child_bbox = vec![];
        let mut children: Vec<CellRef> = vec![];
        for (i, inst) in layout.instances.iter().enumerate() {
            if !children.contains(&inst.cell) {
                children.push(inst.cell.clone());
            }
            let child = if self.building.contains(&inst.cell) { None } else { self.get(&inst.cell, src) };
            let cb = child.as_ref().and_then(|c| c.bbox);
            child_bbox.push(cb);
            let b = match (&child, cb) {
                (Some(c), Some(cb)) => {
                    flat += c.flat_shapes * inst.count();
                    inst.bbox(cb)
                }
                (Some(_), None) => continue,
                (None, _) => {
                    let h = MISSING_CELL_SIZE / 2;
                    inst.bbox(IRect::new(-h, -h, h, h))
                }
            };
            bbox = Some(bbox.map_or(b, |x| x.union(&b)));
            insts.push(Entry { bbox: b, idx: i as u32 });
        }
        self.building.remove(cell);
        let idx = Arc::new(CellIndex {
            bbox,
            flat_shapes: flat,
            children,
            layers: per_layer.into_iter().map(RTree::bulk_load).collect(),
            insts: RTree::bulk_load(insts),
            child_bbox,
            n_shapes: layout.shapes.len(),
            n_insts: layout.instances.len(),
        });
        self.cells.insert(cell.clone(), idx.clone());
        idx
    }
}

/// Options for a view query.
pub struct ViewQuery<'v> {
    /// Visible area in top-cell coordinates.
    pub view: IRect,
    /// Deepest hierarchy level whose contents are drawn (0 = top cell only).
    pub max_depth: u32,
    /// Instances smaller than this (dbu, larger side) are drawn as boxes.
    pub min_cell: i64,
    /// Shapes whose larger side is below this (dbu) are reported as `tiny` points only.
    pub min_shape: i64,
    /// Layer visibility (missing entries count as visible).
    pub visible: &'v [bool],
    /// Stop collecting shapes after this many.
    pub limit: usize,
}

/// A shape to draw, with its placement transform into top-cell coordinates.
pub struct Placed<'a> {
    pub shape: &'a Shape,
    pub trans: Trans,
    /// 0 for shapes of the top cell itself.
    pub depth: u32,
}

/// An instance drawn as a box rather than its contents.
pub struct CellBox<'a> {
    pub rect: IRect,
    pub cell: &'a CellRef,
    pub depth: u32,
    pub missing: bool,
}

#[derive(Default)]
pub struct Visible<'a> {
    /// Shapes per layer, in top-cell coordinates via `trans`.
    pub shapes: Vec<Vec<Placed<'a>>>,
    /// Shapes too small to draw individually, per layer, as world-space boxes.
    pub tiny: Vec<Vec<IRect>>,
    pub boxes: Vec<CellBox<'a>>,
    pub truncated: bool,
    pub count: usize,
}

/// Collect everything visible in `q.view` for the cell `top`, descending through instances.
pub fn query<'a>(hier: &mut Hier, src: &'a dyn LayoutSource, top: &'a Layout, top_idx: &CellIndex, q: &ViewQuery) -> Visible<'a> {
    let mut out = Visible::default();
    walk(hier, src, top, top_idx, Trans::IDENTITY, 0, q, &mut out);
    out
}

#[allow(clippy::too_many_arguments)]
fn walk<'a>(
    hier: &mut Hier,
    src: &'a dyn LayoutSource,
    layout: &'a Layout,
    idx: &CellIndex,
    trans: Trans,
    depth: u32,
    q: &ViewQuery,
    out: &mut Visible<'a>,
) {
    let local = trans.inverse().apply_rect(q.view);
    for (li, tree) in idx.layers.iter().enumerate() {
        if !q.visible.get(li).copied().unwrap_or(true) || tree.size() == 0 {
            continue;
        }
        if out.shapes.len() <= li {
            out.shapes.resize_with(li + 1, Vec::new);
            out.tiny.resize_with(li + 1, Vec::new);
        }
        for e in tree.locate_in_envelope_intersecting(aabb(local)) {
            if out.count >= q.limit {
                out.truncated = true;
                return;
            }
            let Some(shape) = layout.shapes.get(e.idx as usize) else { continue };
            out.count += 1;
            let b = e.bbox;
            if b.width().max(b.height()) < q.min_shape {
                out.tiny[li].push(trans.apply_rect(b));
            } else {
                out.shapes[li].push(Placed { shape, trans, depth });
            }
        }
    }
    for e in idx.insts.locate_in_envelope_intersecting(aabb(local)) {
        let i = e.idx as usize;
        let Some(inst) = layout.instances.get(i) else { continue };
        let Some(cb) = idx.child_bbox(i) else {
            out.boxes.push(CellBox { rect: trans.apply_rect(e.bbox), cell: &inst.cell, depth: depth + 1, missing: true });
            continue;
        };
        let (Some(child), Some(cidx)) = (src.layout(&inst.cell), hier.get(&inst.cell, src)) else { continue };
        let elem = trans.compose(&inst.trans).apply_rect(cb);
        let small = elem.width().max(elem.height()) < q.min_cell;
        if small || depth + 1 > q.max_depth {
            if inst.count() > 64 && small {
                out.boxes.push(CellBox { rect: trans.apply_rect(e.bbox), cell: &inst.cell, depth: depth + 1, missing: false });
            } else {
                for (ci, ri) in inst.visible_elements(cb, local) {
                    let r = trans.compose(&inst.element_trans(ci, ri)).apply_rect(cb);
                    out.boxes.push(CellBox { rect: r, cell: &inst.cell, depth: depth + 1, missing: false });
                }
            }
            continue;
        }
        for (ci, ri) in inst.visible_elements(cb, local) {
            if out.truncated {
                return;
            }
            let t = trans.compose(&inst.element_trans(ci, ri));
            walk(hier, src, child, &cidx, t, depth + 1, q, out);
        }
    }
}

/// A shape of the flattened hierarchy in top-cell coordinates.
pub struct FlatShape {
    pub layer: usize,
    pub geom: Geometry,
    /// Id of the shape when it belongs to the top cell itself.
    pub id: Option<u64>,
}

/// Flatten `top` down to `max_depth` levels. Missing cells are skipped.
pub fn flatten(top: &Layout, src: &dyn LayoutSource, max_depth: u32) -> Vec<FlatShape> {
    fn rec(l: &Layout, src: &dyn LayoutSource, t: Trans, depth: u32, max: u32, top: bool, out: &mut Vec<FlatShape>) {
        for s in &l.shapes {
            let geom = if t.is_identity() { s.geom.clone() } else { s.geom.transformed(&t) };
            out.push(FlatShape { layer: s.layer, geom, id: top.then_some(s.id) });
        }
        if depth >= max {
            return;
        }
        for inst in &l.instances {
            let Some(child) = src.layout(&inst.cell) else { continue };
            let (c, r) = inst.dims();
            for j in 0..r {
                for i in 0..c {
                    rec(child, src, t.compose(&inst.element_trans(i, j)), depth + 1, max, false, out);
                }
            }
        }
    }
    let mut out = vec![];
    rec(top, src, Trans::IDENTITY, 0, max_depth, true, &mut out);
    out
}

/// `top` followed by every cell it places, directly or indirectly (each once, cycle safe).
pub fn descendants(src: &dyn LayoutSource, top: &CellRef) -> Vec<CellRef> {
    let mut out = vec![top.clone()];
    let mut k = 0;
    while k < out.len() {
        if let Some(l) = src.layout(&out[k]) {
            for inst in &l.instances {
                if !out.contains(&inst.cell) {
                    out.push(inst.cell.clone());
                }
            }
        }
        k += 1;
    }
    out
}

/// Whether placing `child` inside `parent` would create a cycle.
pub fn would_cycle(src: &dyn LayoutSource, parent: &CellRef, child: &CellRef) -> bool {
    descendants(src, child).contains(parent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::ArraySpec;

    fn lib() -> (HashMap<CellRef, Layout>, CellRef, CellRef) {
        let leaf = CellRef::new("w", "leaf");
        let top = CellRef::new("w", "top");
        let mut m = HashMap::new();
        let mut l = Layout::default();
        l.add_rect(1, 0, 0, 100, 200);
        m.insert(leaf.clone(), l);
        let mut t = Layout::default();
        t.add_rect(2, -50, -50, 0, 0);
        t.add_instance(
            leaf.clone(),
            Trans::with(1000, 0, 1, false),
            Some(ArraySpec { cols: 100, rows: 100, col: (500, 0), row: (0, 500) }),
        );
        m.insert(top.clone(), t);
        (m, leaf, top)
    }

    #[test]
    fn hierarchical_bbox_and_counts() {
        let (m, _, top) = lib();
        let mut h = Hier::default();
        let idx = h.get(&top, &m).unwrap();
        // leaf rotated 90° spans x [-200, 0], y [0, 100]; placed at x=1000.
        assert_eq!(idx.bbox, Some(IRect::new(-50, -50, 1000 + 99 * 500, 100 + 99 * 500)));
        assert_eq!(idx.flat_shapes, 1 + 10_000);
        assert_eq!(flatten(m.layout(&top).unwrap(), &m, 10).len(), 10_001);
    }

    #[test]
    fn query_culls_and_collapses() {
        let (m, _, top) = lib();
        let mut h = Hier::default();
        let t = m.layout(&top).unwrap();
        let idx = h.get(&top, &m).unwrap();
        let q = |view, max_depth, min_cell| ViewQuery { view, max_depth, min_cell, min_shape: 0, visible: &[], limit: 1_000_000 };
        let v = query(&mut h, &m, t, &idx, &q(IRect::new(900, 0, 1400, 400), 10, 0));
        let n: usize = v.shapes.iter().map(Vec::len).sum();
        assert_eq!(n, 2);
        let v = query(&mut h, &m, t, &idx, &q(IRect::new(-100_000, -100_000, 100_000, 100_000), 0, 0));
        assert_eq!(v.boxes.len(), 10_000);
        let v = query(&mut h, &m, t, &idx, &q(IRect::new(-100_000, -100_000, 100_000, 100_000), 10, 1_000));
        assert_eq!(v.boxes.len(), 1, "a big array of tiny cells collapses to one box");
    }

    #[test]
    fn invalidation_reaches_parents() {
        let (mut m, leaf, top) = lib();
        let mut h = Hier::default();
        h.get(&top, &m).unwrap();
        m.get_mut(&leaf).unwrap().add_rect(1, 0, 0, 1000, 1000);
        h.invalidate(&leaf);
        let idx = h.get(&top, &m).unwrap();
        assert_eq!(idx.flat_shapes, 1 + 20_000);
    }

    /// `cargo test --release -p ics-core bench_ -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_large_layouts() {
        use std::time::Instant;
        let p = crate::examples::demo_project();
        let top = CellRef::new("work", "inv_tiles");
        let mut h = Hier::default();
        let t = Instant::now();
        let idx = h.get(&top, &p).unwrap();
        println!("inv_tiles index: {:?} ({} flat shapes)", t.elapsed(), idx.flat_shapes);
        let all = idx.bbox.unwrap();
        for (name, min_cell) in [("full detail", 0), ("8px LOD", 4687)] {
            let q = ViewQuery { view: all, max_depth: 32, min_cell, min_shape: 780, visible: &[], limit: usize::MAX };
            let t = Instant::now();
            let v = query(&mut h, &p, p.layout(&top).unwrap(), &idx, &q);
            println!("inv_tiles query {name}: {:?} ({} shapes, {} boxes)", t.elapsed(), v.count, v.boxes.len());
        }

        let mut flat = Layout::default();
        for i in 0..1000i64 {
            for j in 0..1000i64 {
                flat.add_rect(((i + j) % 8) as usize, i * 1000, j * 1000, i * 1000 + 500, j * 1000 + 300);
            }
        }
        let cell = CellRef::new("w", "flat");
        let mut h = Hier::default();
        let t = Instant::now();
        let idx = h.get_for(&cell, &flat, &NoCells);
        println!("1M flat rects index: {:?}", t.elapsed());
        let view = IRect::new(400_000, 400_000, 450_000, 430_000);
        let q = ViewQuery { view, max_depth: 32, min_cell: 0, min_shape: 0, visible: &[], limit: usize::MAX };
        let t = Instant::now();
        let v = query(&mut h, &NoCells, &flat, &idx, &q);
        println!("1M flat rects zoomed query: {:?} ({} shapes)", t.elapsed(), v.count);
    }

    #[test]
    fn cycles_are_detected_and_do_not_hang() {
        let (mut m, leaf, top) = lib();
        assert!(would_cycle(&m, &leaf, &top));
        assert!(!would_cycle(&m, &top, &leaf));
        m.get_mut(&leaf).unwrap().add_instance(top.clone(), Trans::IDENTITY, None);
        let mut h = Hier::default();
        assert!(h.get(&top, &m).is_some());
        assert_eq!(descendants(&m, &top).len(), 2);
    }
}
