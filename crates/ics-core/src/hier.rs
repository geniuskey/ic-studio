//! Layout hierarchy: resolving cell instances, cached per-cell spatial indices, hierarchical
//! bounding boxes, view queries with level-of-detail and flattening.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

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

/// Shapes of one layer whose [`raster_key`] falls into one power-of-two class.
struct Bucket {
    key: i32,
    tree: RTree<Entry>,
}

/// Spatial index over one cell's shapes (per layer) and instances, plus hierarchical totals.
pub struct CellIndex {
    /// Extent of the cell including everything placed in it.
    pub bbox: Option<IRect>,
    /// Number of shapes in the flattened hierarchy.
    pub flat_shapes: u64,
    /// Cells instantiated directly by this cell.
    pub children: Vec<CellRef>,
    /// Deepest instance nesting below this cell (0 for a leaf).
    pub height: u32,
    layers: Vec<Vec<Bucket>>,
    /// Instances bucketed by [sprite_key] of their element size.
    insts: Vec<Bucket>,
    /// Bounding box of each instance's child cell (None if unresolved or empty).
    child_bbox: Vec<Option<IRect>>,
    n_shapes: usize,
    n_insts: usize,
    /// Worth drawing through a coverage sprite when zoomed out (many shapes or instances).
    dense: bool,
    sprites: Mutex<HashMap<(u32, bool), Arc<Sprite>>>,
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
            .flatten()
            .flat_map(move |b| b.tree.locate_in_envelope_intersecting(aabb(area)).map(|e| e.idx as usize))
    }

    /// Indices into `layout.instances` whose placement box intersects `area`.
    pub fn instances_in(&self, area: IRect) -> impl Iterator<Item = usize> + '_ {
        self.insts.iter().flat_map(move |b| b.tree.locate_in_envelope_intersecting(aabb(area)).map(|e| e.idx as usize))
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
        let bucketed = layout.shapes.len() >= BUCKET_MIN_SHAPES;
        let mut per_layer: Vec<HashMap<i32, Vec<Entry>>> = vec![];
        let mut bbox: Option<IRect> = None;
        for (i, s) in layout.shapes.iter().enumerate() {
            if per_layer.len() <= s.layer {
                per_layer.resize_with(s.layer + 1, HashMap::new);
            }
            let b = s.bbox();
            bbox = Some(bbox.map_or(b, |x| x.union(&b)));
            let key = if bucketed { raster_key(&s.geom) } else { i32::MAX };
            per_layer[s.layer].entry(key).or_default().push(Entry { bbox: b, idx: i as u32 });
        }
        let mut height = 0;
        let mut elements = 0;
        let mut flat = layout.shapes.len() as u64;
        let mut insts: HashMap<i32, Vec<Entry>> = HashMap::new();
        let inst_buckets = layout.instances.len() >= DENSE_MIN_ELEMENTS as usize;
        let mut child_bbox = vec![];
        let mut children: Vec<CellRef> = vec![];
        for (i, inst) in layout.instances.iter().enumerate() {
            if !children.contains(&inst.cell) {
                children.push(inst.cell.clone());
            }
            let child = if self.building.contains(&inst.cell) { None } else { self.get(&inst.cell, src) };
            let cb = child.as_ref().and_then(|c| c.bbox);
            child_bbox.push(cb);
            if let Some(c) = &child {
                height = height.max(c.height + 1);
            }
            elements += inst.count();
            let mut key = i32::MAX;
            let b = match (&child, cb) {
                (Some(c), Some(cb)) => {
                    flat += c.flat_shapes * inst.count();
                    if inst_buckets {
                        let e = inst.trans.apply_rect(cb);
                        key = sprite_key(e.width().max(e.height()));
                    }
                    inst.bbox(cb)
                }
                (Some(_), None) => continue,
                (None, _) => {
                    let h = MISSING_CELL_SIZE / 2;
                    inst.bbox(IRect::new(-h, -h, h, h))
                }
            };
            bbox = Some(bbox.map_or(b, |x| x.union(&b)));
            insts.entry(key).or_default().push(Entry { bbox: b, idx: i as u32 });
        }
        self.building.remove(cell);
        let idx = Arc::new(CellIndex {
            bbox,
            flat_shapes: flat,
            children,
            height,
            layers: per_layer
                .into_iter()
                .map(|m| m.into_iter().map(|(key, v)| Bucket { key, tree: RTree::bulk_load(v) }).collect())
                .collect(),
            insts: insts.into_iter().map(|(key, v)| Bucket { key, tree: RTree::bulk_load(v) }).collect(),
            child_bbox,
            n_shapes: layout.shapes.len(),
            n_insts: layout.instances.len(),
            dense: bucketed || elements >= DENSE_MIN_ELEMENTS,
            sprites: Mutex::default(),
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
    /// Sprite level (pixel = `1 << lod` dbu, see [`lod_level`]): small instances and the
    /// rasterized shapes of dense cells are reported as [`Stamp`]s instead of shapes.
    pub lod: Option<u32>,
}

/// Sprite level for drawing into a coverage grid of `cell_px` dbu per cell: the coarsest
/// power of two not above it.
pub fn lod_level(cell_px: f64) -> Option<u32> {
    (cell_px >= 1.0).then(|| cell_px.log2().floor() as u32)
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

/// A coverage sprite placed into top-cell coordinates.
pub struct Stamp {
    pub sprite: Arc<Sprite>,
    pub trans: Trans,
}

#[derive(Default)]
pub struct Visible<'a> {
    /// Shapes per layer, in top-cell coordinates via `trans`.
    pub shapes: Vec<Vec<Placed<'a>>>,
    /// Shapes too small to draw individually, per layer, as world-space boxes.
    pub tiny: Vec<Vec<IRect>>,
    pub boxes: Vec<CellBox<'a>>,
    pub stamps: Vec<Stamp>,
    pub truncated: bool,
    /// Shapes collected individually.
    pub count: usize,
    /// Shapes represented by `stamps` (estimated for the visible part).
    pub stamped: u64,
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
    let lod = q.lod.filter(|_| depth + idx.height <= q.max_depth);
    let pyramid = lod.filter(|&e| idx.dense && idx.bbox.is_some_and(|b| sprite_fits(b, e))).and_then(|e| {
        let sp = sprite(hier, src, layout, idx, e, false)?;
        out.stamped += sp.shapes_in(local);
        out.stamps.push(Stamp { sprite: sp, trans });
        Some(e)
    });
    let cut = pyramid.map_or(i32::MIN, |e| e as i32 - 1);
    for (li, buckets) in idx.layers.iter().enumerate() {
        if !q.visible.get(li).copied().unwrap_or(true) || buckets.is_empty() {
            continue;
        }
        if out.shapes.len() <= li {
            out.shapes.resize_with(li + 1, Vec::new);
            out.tiny.resize_with(li + 1, Vec::new);
        }
        for bucket in buckets.iter().filter(|b| b.key >= cut) {
            for e in bucket.tree.locate_in_envelope_intersecting(aabb(local)) {
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
    }
    let inst_cut = pyramid.map_or(i32::MIN, |e| e as i32);
    for e in idx.insts.iter().filter(|b| b.key >= inst_cut).flat_map(|b| b.tree.locate_in_envelope_intersecting(aabb(local))) {
        let i = e.idx as usize;
        let Some(inst) = layout.instances.get(i) else { continue };
        let Some(cb) = idx.child_bbox(i) else {
            out.boxes.push(CellBox { rect: trans.apply_rect(e.bbox), cell: &inst.cell, depth: depth + 1, missing: true });
            continue;
        };
        let elem = trans.compose(&inst.trans).apply_rect(cb);
        let size = elem.width().max(elem.height());
        let sprited = lod.filter(|&e| size < SPRITE_PIXELS << e);
        let small = size < lod.map_or(q.min_cell, |e| BOX_PIXELS << e);
        if sprited.is_some() && pyramid.is_some() {
            continue;
        }
        if small || depth + 1 > q.max_depth {
            let cell_box = |rect| CellBox { rect, cell: &inst.cell, depth: depth + 1, missing: false };
            if inst.count() == 1 {
                out.boxes.push(cell_box(elem));
            } else if inst.count() > 64 && small {
                out.boxes.push(cell_box(trans.apply_rect(e.bbox)));
            } else {
                for (ci, ri) in inst.visible_elements(cb, local) {
                    out.boxes.push(cell_box(trans.compose(&inst.element_trans(ci, ri)).apply_rect(cb)));
                }
            }
            continue;
        }
        let (Some(child), Some(cidx)) = (src.layout(&inst.cell), hier.get(&inst.cell, src)) else { continue };
        if let Some(e) = sprited {
            let elements = inst.visible_elements(cb, local);
            if elements.len() <= MAX_STAMPS
                && let Some(sp) = sprite(hier, src, child, &cidx, e, true)
            {
                out.stamped += sp.shapes * elements.len() as u64;
                for (ci, ri) in elements {
                    out.stamps.push(Stamp { sprite: sp.clone(), trans: trans.compose(&inst.element_trans(ci, ri)) });
                }
                continue;
            }
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

/// Shapes per cell from which shapes are bucketed by [`raster_key`].
const BUCKET_MIN_SHAPES: usize = 1024;
/// Instance elements per cell from which a cell counts as dense.
const DENSE_MIN_ELEMENTS: u64 = 256;
/// Instances smaller than this many sprite pixels are stamped as sprites.
pub const SPRITE_PIXELS: i64 = 16;
/// Instances smaller than this many sprite pixels are drawn as frames only.
pub const BOX_PIXELS: i64 = 4;
/// Array elements stamped individually at most; larger arrays fall back to boxes.
const MAX_STAMPS: usize = 4096;
/// Side (pixels) of the tiles in which sprites count their shapes.
const TILE: usize = 32;
/// Largest sprite (pixels) built for a dense cell.
const MAX_SPRITE_PIXELS: i64 = 1 << 22;

/// Power-of-two class of an instance element size: elements are stamped as sprites at the
/// levels above it.
fn sprite_key(size: i64) -> i32 {
    let k = size / SPRITE_PIXELS;
    if k < 1 { i32::MIN } else { 63 - k.leading_zeros() as i32 }
}

fn sprite_fits(b: IRect, level: u32) -> bool {
    let n = |lo: i64, hi: i64| hi.div_euclid(1 << level) - lo.div_euclid(1 << level) + 1;
    n(b.x0, b.x1).saturating_mul(n(b.y0, b.y1)) <= MAX_SPRITE_PIXELS
}

/// Power-of-two class of the screen resolution (dbu per pixel) below which a shape is drawn
/// as coverage rather than as a shape: rectangles and polygons thinner than 3 px, paths
/// narrower than 3 px and labels shorter than 1.5 px.
pub fn raster_key(g: &Geometry) -> i32 {
    let b = g.bbox();
    let k = match g {
        Geometry::Path { width, .. } => *width as f64 / 3.0,
        Geometry::Label { .. } => b.width().max(b.height()) as f64 / 1.5,
        _ => b.width().min(b.height()) as f64 / 3.0,
    };
    if k < 1.0 { i32::MIN } else { k.log2().floor() as i32 }
}

/// Set bits `from..to` of a bit row.
pub fn set_bits(row: &mut [u64], from: usize, to: usize) {
    let mut x = from;
    while x < to {
        let (w, b) = (x / 64, x % 64);
        let n = (64 - b).min(to - x);
        row[w] |= (u64::MAX >> (64 - n)) << b;
        x += n;
    }
}

/// Call `f(start, end)` for every run of set bits of `row` within `from..to`.
pub fn bit_runs(row: &[u64], from: usize, to: usize, mut f: impl FnMut(usize, usize)) {
    let mut x = from;
    while x < to {
        let w = row[x / 64] >> (x % 64);
        if w == 0 {
            x = (x / 64 + 1) * 64;
            continue;
        }
        x += w.trailing_zeros() as usize;
        if x >= to {
            return;
        }
        let start = x;
        loop {
            let n = (!(row[x / 64] >> (x % 64))).trailing_zeros() as usize;
            x += n;
            if n == 0 || !x.is_multiple_of(64) || x >= to {
                break;
            }
        }
        f(start, x.min(to));
    }
}
/// Sprite plane of shapes drawn as plain coverage.
pub const PLANE_SMALL: usize = 0;
/// Sprite plane of the interior of shapes large enough to draw as shapes.
pub const PLANE_FILL: usize = 1;
/// Sprite plane of the outline of shapes large enough to draw as shapes.
pub const PLANE_EDGE: usize = 2;
const PLANES: usize = 3;

/// Coverage bitmaps of a cell on a grid of `1 << level` dbu aligned to multiples of the pixel
/// size, one per layer and plane. Row 0 is the bottom row.
pub struct Sprite {
    pub level: u32,
    /// Cell-local coordinate of the lower left corner of pixel (0, 0).
    pub origin: (i64, i64),
    pub w: usize,
    pub h: usize,
    words: usize,
    bbox: IRect,
    /// Bitmaps indexed by `layer * PLANES + plane`.
    slots: Vec<Vec<u64>>,
    /// Outlines of instances too small to draw, on a grid twice as fine (one plane).
    frames: Option<Box<Sprite>>,
    /// Shapes per tile of `TILE` × `TILE` pixels, by their centre.
    tiles: Vec<u64>,
    /// Number of shapes the sprite stands for.
    pub shapes: u64,
}

impl Sprite {
    pub fn new(bbox: IRect, level: u32) -> Self {
        let p = 1i64 << level;
        let (x0, y0) = (bbox.x0.div_euclid(p), bbox.y0.div_euclid(p));
        let w = (bbox.x1.div_euclid(p) - x0 + 1) as usize;
        let h = (bbox.y1.div_euclid(p) - y0 + 1) as usize;
        Self {
            level,
            origin: (x0 * p, y0 * p),
            w,
            h,
            words: w.div_ceil(64),
            bbox,
            slots: vec![],
            frames: None,
            tiles: vec![],
            shapes: 0,
        }
    }

    /// The instance frame grid and its bits, if any frame is set.
    pub fn frames(&self) -> Option<(&Sprite, &[u64])> {
        let f = self.frames.as_deref()?;
        Some((f, f.bits(0)?))
    }

    fn tiles_w(&self) -> usize {
        self.w.div_ceil(TILE)
    }

    /// Count `n` shapes centred in the cell-local rectangle `r`.
    fn count(&mut self, r: IRect, n: u64) {
        let p = self.pixel();
        let at = |v: i64, o: i64, len: usize| (((v - o).div_euclid(p)).clamp(0, len as i64 - 1) as usize) / TILE;
        let (tx, ty) = (at((r.x0 + r.x1) / 2, self.origin.0, self.w), at((r.y0 + r.y1) / 2, self.origin.1, self.h));
        if self.tiles.is_empty() {
            self.tiles = vec![0; self.tiles_w() * self.h.div_ceil(TILE)];
        }
        let tw = self.tiles_w();
        self.tiles[ty * tw + tx] += n;
        self.shapes += n;
    }

    /// Approximate number of shapes in the cell-local area `r` (whole tiles).
    pub fn shapes_in(&self, r: IRect) -> u64 {
        if self.tiles.is_empty() {
            return 0;
        }
        let span = |lo: i64, hi: i64, o: i64, len: usize| {
            let t = |v: i64| ((v - o).div_euclid(self.pixel()).clamp(0, len as i64 - 1) as usize) / TILE;
            t(lo)..=t(hi)
        };
        let tw = self.tiles_w();
        span(r.y0, r.y1, self.origin.1, self.h)
            .flat_map(|ty| span(r.x0, r.x1, self.origin.0, self.w).map(move |tx| ty * tw + tx))
            .map(|i| self.tiles[i])
            .sum()
    }

    fn frames_mut(&mut self) -> &mut Sprite {
        let (bbox, level) = (self.bbox, self.level.saturating_sub(1));
        self.frames.get_or_insert_with(|| Box::new(Sprite::new(bbox, level)))
    }

    pub fn pixel(&self) -> i64 {
        1 << self.level
    }

    fn slot(layer: usize, plane: usize) -> usize {
        layer * PLANES + plane
    }

    fn bits(&self, slot: usize) -> Option<&[u64]> {
        self.slots.get(slot).filter(|l| !l.is_empty()).map(Vec::as_slice)
    }

    /// Bit rows of one plane of `layer`, if anything is set on it.
    pub fn plane(&self, layer: usize, plane: usize) -> Option<&[u64]> {
        self.bits(Self::slot(layer, plane))
    }

    pub fn row<'s>(&self, bits: &'s [u64], y: usize) -> &'s [u64] {
        &bits[y * self.words..(y + 1) * self.words]
    }

    pub fn get(&self, layer: usize, plane: usize, x: usize, y: usize) -> bool {
        self.plane(layer, plane).is_some_and(|b| self.row(b, y)[x / 64] >> (x % 64) & 1 == 1)
    }

    /// Set the pixels `x0..=x1` × `y0..=y1`, clipped to the sprite.
    fn fill(&mut self, slot: usize, (x0, y0): (i64, i64), (x1, y1): (i64, i64)) {
        let (w, h) = (self.w as i64, self.h as i64);
        let (xa, xb, ya, yb) = (x0.min(x1).max(0), x0.max(x1).min(w - 1), y0.min(y1).max(0), y0.max(y1).min(h - 1));
        if xa > xb || ya > yb {
            return;
        }
        if self.slots.len() <= slot {
            self.slots.resize_with(slot + 1, Vec::new);
        }
        let words = self.words;
        let bits = &mut self.slots[slot];
        if bits.is_empty() {
            *bits = vec![0; words * self.h];
        }
        for y in ya as usize..=yb as usize {
            set_bits(&mut bits[y * words..(y + 1) * words], xa as usize, xb as usize + 1);
        }
    }

    /// Mark every pixel overlapping the cell-local rectangle `r` (at least one per axis).
    fn mark_rect(&mut self, slot: usize, r: IRect) {
        let p = self.pixel();
        let span = |lo: i64, hi: i64, o: i64| {
            let a = (lo - o).div_euclid(p);
            (a, ((hi - o - 1).div_euclid(p)).max(a))
        };
        let (x0, x1) = span(r.x0, r.x1, self.origin.0);
        let (y0, y1) = span(r.y0, r.y1, self.origin.1);
        self.fill(slot, (x0, y0), (x1, y1));
    }

    /// Mark every pixel of one plane of `layer` overlapping the cell-local rectangle `r`.
    pub fn mark(&mut self, layer: usize, plane: usize, r: IRect) {
        self.mark_rect(Self::slot(layer, plane), r);
    }

    fn mark_border(&mut self, slot: usize, r: IRect) {
        let IRect { x0, y0, x1, y1 } = r;
        for side in [
            IRect::new(x0, y0, x1, y0 + 1),
            IRect::new(x0, y1 - 1, x1, y1),
            IRect::new(x0, y0, x0 + 1, y1),
            IRect::new(x1 - 1, y0, x1, y1),
        ] {
            self.mark_rect(slot, side);
        }
    }

    /// Rasterize a shape: as coverage when its [`raster_key`] is below `cut`, else as fill
    /// and outline the way it would be drawn.
    fn mark_shape(&mut self, layer: usize, g: &Geometry, cut: i32) {
        let small = raster_key(g) < cut;
        let plane = |p| Self::slot(layer, p);
        match g {
            Geometry::Path { points, width } if points.len() > 1 => {
                for w in points.windows(2) {
                    let seg = Geometry::Path { points: w.to_vec(), width: *width }.bbox();
                    if small {
                        self.mark_rect(plane(PLANE_SMALL), seg);
                    } else {
                        self.mark_rect(plane(PLANE_FILL), seg);
                        self.mark_border(plane(PLANE_EDGE), seg);
                    }
                }
            }
            _ if small => self.mark_rect(plane(PLANE_SMALL), g.bbox()),
            Geometry::Rect(r) => {
                self.mark_rect(plane(PLANE_FILL), *r);
                self.mark_border(plane(PLANE_EDGE), *r);
            }
            Geometry::Polygon { .. } | Geometry::Path { .. } => self.mark_border(plane(PLANE_EDGE), g.bbox()),
            Geometry::Label { .. } => {}
        }
    }

    /// OR `child` (same level) placed by `t` into this sprite; each child pixel lands on the
    /// pixel holding its centre.
    pub fn stamp(&mut self, child: &Sprite, t: &Trans) {
        debug_assert_eq!(child.level, self.level);
        let p = self.pixel();
        let t2 = Trans { x: 2 * t.x, y: 2 * t.y, ..*t };
        let o = self.origin;
        let at = |i: usize, j: usize| {
            let c = (2 * child.origin.0 + (2 * i as i64 + 1) * p, 2 * child.origin.1 + (2 * j as i64 + 1) * p);
            let (x, y) = t2.apply(c);
            ((x - 2 * o.0).div_euclid(2 * p), (y - 2 * o.1).div_euclid(2 * p))
        };
        for (slot, bits) in child.slots.iter().enumerate() {
            if bits.is_empty() {
                continue;
            }
            for j in 0..child.h {
                bit_runs(child.row(bits, j), 0, child.w, |a, b| self.fill(slot, at(a, j), at(b - 1, j)));
            }
        }
        if let Some(f) = &child.frames {
            self.frames_mut().stamp(f, t);
        }
    }
}

/// Coverage sprite of a cell at `level`: with `full` everything in it, otherwise only what is
/// drawn as coverage at that level (own shapes below the bucket cut plus every instance
/// smaller than [`SPRITE_PIXELS`]); instances below [`BOX_PIXELS`] only as frames. Built on
/// first use and cached in the cell index.
fn sprite(
    hier: &mut Hier,
    src: &dyn LayoutSource,
    layout: &Layout,
    idx: &CellIndex,
    level: u32,
    full: bool,
) -> Option<Arc<Sprite>> {
    if let Some(s) = idx.sprites.lock().ok()?.get(&(level, full)) {
        return Some(s.clone());
    }
    let mut sp = Sprite::new(idx.bbox?, level);
    let cut = level as i32 - 1;
    for (li, buckets) in idx.layers.iter().enumerate() {
        for b in buckets.iter().filter(|b| full || b.key < cut) {
            for e in b.tree.iter() {
                if let Some(s) = layout.shapes.get(e.idx as usize) {
                    sp.mark_shape(li, &s.geom, cut);
                    sp.count(e.bbox, 1);
                }
            }
        }
    }
    for (i, inst) in layout.instances.iter().enumerate() {
        let Some(cb) = idx.child_bbox(i) else { continue };
        let elem = inst.trans.apply_rect(cb);
        let size = elem.width().max(elem.height());
        if !full && size >= SPRITE_PIXELS << level {
            continue;
        }
        let (c, r) = inst.dims();
        if size < BOX_PIXELS << level {
            let frames = sp.frames_mut();
            if inst.count() > 64 {
                frames.mark_border(0, inst.bbox(cb));
            } else {
                for ri in 0..r {
                    for ci in 0..c {
                        frames.mark_border(0, inst.element_trans(ci, ri).apply_rect(cb));
                    }
                }
            }
            continue;
        }
        let (Some(child), Some(cidx)) = (src.layout(&inst.cell), hier.get(&inst.cell, src)) else { continue };
        let Some(cs) = sprite(hier, src, child, &cidx, level, true) else { continue };
        for ri in 0..r {
            for ci in 0..c {
                let t = inst.element_trans(ci, ri);
                sp.stamp(&cs, &t);
                sp.count(t.apply_rect(cb), cs.shapes);
            }
        }
    }
    let sp = Arc::new(sp);
    idx.sprites.lock().ok()?.insert((level, full), sp.clone());
    Some(sp)
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
        let q = |view, max_depth, min_cell| ViewQuery {
            view,
            max_depth,
            min_cell,
            min_shape: 0,
            visible: &[],
            limit: 1_000_000,
            lod: None,
        };
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
            let q = ViewQuery { view: all, max_depth: 32, min_cell, min_shape: 780, visible: &[], limit: usize::MAX, lod: None };
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
        let q = ViewQuery { view, max_depth: 32, min_cell: 0, min_shape: 0, visible: &[], limit: usize::MAX, lod: None };
        let t = Instant::now();
        let v = query(&mut h, &NoCells, &flat, &idx, &q);
        println!("1M flat rects zoomed query: {:?} ({} shapes)", t.elapsed(), v.count);
    }

    #[test]
    fn bit_helpers_agree_with_naive_bits() {
        let mut row = vec![0u64; 3];
        let ranges = [(0, 1), (5, 70), (63, 65), (100, 128), (130, 131), (150, 192)];
        for &(a, b) in &ranges {
            set_bits(&mut row, a, b);
        }
        let naive: Vec<bool> = (0..192).map(|x| ranges.iter().any(|&(a, b)| (a..b).contains(&x))).collect();
        assert!((0..192).all(|x| (row[x / 64] >> (x % 64) & 1 == 1) == naive[x]));
        let mut runs = vec![];
        bit_runs(&row, 0, 192, |a, b| runs.push((a, b)));
        assert_eq!(runs, vec![(0, 1), (5, 70), (100, 128), (130, 131), (150, 192)]);
        let mut clipped = vec![];
        bit_runs(&row, 64, 101, |a, b| clipped.push((a, b)));
        assert_eq!(clipped, vec![(64, 70), (100, 101)]);
    }

    /// A placed child sprite equals the sprite of the flattened shapes for every orientation.
    #[test]
    fn sprites_stamp_children_in_every_orientation() {
        let leaf = CellRef::new("w", "leaf");
        let top = CellRef::new("w", "top");
        let mut l = Layout::default();
        l.add_rect(0, 0, 0, 96, 16);
        l.add_rect(0, 0, 0, 16, 64);
        l.add_rect(1, 32, 32, 96, 64);
        let mut t = Layout::default();
        for (k, (rot, mirror)) in (0..4).flat_map(|r| [(r, false), (r, true)]).enumerate() {
            t.add_instance(leaf.clone(), Trans::with(k as i64 * 160, 48, rot, mirror), None);
        }
        let m = HashMap::from([(leaf, l), (top.clone(), t)]);
        let mut h = Hier::default();
        let idx = h.get(&top, &m).unwrap();
        let level = 4;
        let sp = sprite(&mut h, &m, m.layout(&top).unwrap(), &idx, level, true).unwrap();
        let mut want = Sprite::new(idx.bbox.unwrap(), level);
        for s in flatten(m.layout(&top).unwrap(), &m, 8) {
            want.mark_shape(s.layer, &s.geom, level as i32 - 1);
        }
        assert_eq!(sp.shapes, 24);
        assert_eq!(sp.slots, want.slots);
        let at = |x: i64, y: i64| (((x - sp.origin.0) / 16) as usize, ((y - sp.origin.1) / 16) as usize);
        let (x, y) = at(56, 88);
        assert!(sp.get(1, PLANE_FILL, x, y) && sp.get(1, PLANE_EDGE, x - 1, y) && !sp.get(1, PLANE_SMALL, x, y));
    }

    /// With a sprite level, a dense cell reports its small shapes through one stamp and only
    /// the large ones individually.
    #[test]
    fn dense_cells_split_shapes_between_sprite_and_query() {
        let mut l = Layout::default();
        for i in 0..2000 {
            l.add_rect(0, i * 100, 0, i * 100 + 10, 10);
        }
        for i in 0..5 {
            l.add_rect(1, i * 50_000, 1000, i * 50_000 + 40_000, 41_000);
        }
        let cell = CellRef::new("w", "dense");
        let mut h = Hier::default();
        let idx = h.get_for(&cell, &l, &NoCells);
        let q = ViewQuery {
            view: idx.bbox.unwrap(),
            max_depth: 8,
            min_cell: 0,
            min_shape: 0,
            visible: &[],
            limit: usize::MAX,
            lod: lod_level(256.0),
        };
        let v = query(&mut h, &NoCells, &l, &idx, &q);
        assert_eq!((v.count, v.stamps.len(), v.stamped), (5, 1, 2000));
        assert!(v.stamps[0].sprite.plane(0, PLANE_SMALL).is_some());
        let v = query(&mut h, &NoCells, &l, &idx, &ViewQuery { lod: None, ..q });
        assert_eq!((v.count, v.stamps.len()), (2005, 0));
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
