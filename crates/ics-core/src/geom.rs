//! Integer geometry primitives shared by the schematic and layout databases.

use serde::{Deserialize, Serialize};

/// A point on the schematic grid (y grows downwards, like screen space).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub struct Pt {
    pub x: i32,
    pub y: i32,
}

impl Pt {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

impl std::ops::Add for Pt {
    type Output = Pt;
    fn add(self, o: Pt) -> Pt {
        Pt::new(self.x + o.x, self.y + o.y)
    }
}

impl std::ops::Sub for Pt {
    type Output = Pt;
    fn sub(self, o: Pt) -> Pt {
        Pt::new(self.x - o.x, self.y - o.y)
    }
}

/// Orientation of a placed object: optional mirror about the Y axis, then rotation
/// in 90° clockwise steps (screen space).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct Orient {
    pub rot: u8,
    pub mirror: bool,
}

impl Orient {
    pub fn rotated(self) -> Self {
        Self { rot: (self.rot + 1) % 4, ..self }
    }
    pub fn mirrored(self) -> Self {
        Self { mirror: !self.mirror, ..self }
    }

    /// Transform a local floating point coordinate.
    pub fn apply_f(self, x: f32, y: f32) -> (f32, f32) {
        let x = if self.mirror { -x } else { x };
        match self.rot % 4 {
            0 => (x, y),
            1 => (-y, x),
            2 => (-x, -y),
            _ => (y, -x),
        }
    }

    pub fn apply(self, p: Pt) -> Pt {
        let x = if self.mirror { -p.x } else { p.x };
        match self.rot % 4 {
            0 => Pt::new(x, p.y),
            1 => Pt::new(-p.y, x),
            2 => Pt::new(-x, -p.y),
            _ => Pt::new(p.y, -x),
        }
    }
}

/// Returns true if `p` lies on the segment `a`-`b` (inclusive of the endpoints).
pub fn on_segment(p: Pt, a: Pt, b: Pt) -> bool {
    let cross = (b.x - a.x) as i64 * (p.y - a.y) as i64 - (b.y - a.y) as i64 * (p.x - a.x) as i64;
    if cross != 0 {
        return false;
    }
    p.x >= a.x.min(b.x) && p.x <= a.x.max(b.x) && p.y >= a.y.min(b.y) && p.y <= a.y.max(b.y)
}

/// Axis aligned rectangle in layout database units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct IRect {
    pub x0: i64,
    pub y0: i64,
    pub x1: i64,
    pub y1: i64,
}

impl IRect {
    pub fn new(x0: i64, y0: i64, x1: i64, y1: i64) -> Self {
        Self { x0: x0.min(x1), y0: y0.min(y1), x1: x0.max(x1), y1: y0.max(y1) }
    }
    pub fn width(&self) -> i64 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> i64 {
        self.y1 - self.y0
    }
    pub fn contains(&self, x: i64, y: i64) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }
    pub fn intersects(&self, o: &IRect) -> bool {
        self.x0 < o.x1 && o.x0 < self.x1 && self.y0 < o.y1 && o.y0 < self.y1
    }
    pub fn union(&self, o: &IRect) -> IRect {
        IRect { x0: self.x0.min(o.x0), y0: self.y0.min(o.y0), x1: self.x1.max(o.x1), y1: self.y1.max(o.y1) }
    }
    pub fn translate(&self, dx: i64, dy: i64) -> IRect {
        IRect { x0: self.x0 + dx, y0: self.y0 + dy, x1: self.x1 + dx, y1: self.y1 + dy }
    }
    /// Euclidean-free (Manhattan-gap) spacing between two non-overlapping rects.
    pub fn spacing(&self, o: &IRect) -> i64 {
        let dx = (o.x0 - self.x1).max(self.x0 - o.x1).max(0);
        let dy = (o.y0 - self.y1).max(self.y0 - o.y1).max(0);
        dx.max(dy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orient_roundtrip() {
        let p = Pt::new(1, -2);
        let mut o = Orient::default();
        for _ in 0..4 {
            o = o.rotated();
        }
        assert_eq!(o.apply(p), p);
        assert_eq!(Orient { rot: 1, mirror: false }.apply(Pt::new(0, -2)), Pt::new(2, 0));
    }

    #[test]
    fn segment() {
        assert!(on_segment(Pt::new(2, 0), Pt::new(0, 0), Pt::new(5, 0)));
        assert!(!on_segment(Pt::new(6, 0), Pt::new(0, 0), Pt::new(5, 0)));
        assert!(!on_segment(Pt::new(2, 1), Pt::new(0, 0), Pt::new(5, 0)));
    }
}
