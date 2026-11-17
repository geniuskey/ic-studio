//! Routing tracks (Virtuoso width spacing patterns, WSP): per cell and layer, wires of one
//! direction run on the centre lines of a periodic pattern of tracks, each with its width.
//! The layout editor snaps drawn wires to them, and [`off_track`] lists the wires that are not
//! on a track or not of its width.

use serde::{Deserialize, Serialize};

use crate::geom::IRect;
use crate::layout::{Geometry, Layout};
use crate::tech::Tech;

/// A periodic pattern of tracks on one layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackPattern {
    pub layer: usize,
    /// Tracks run horizontally (wires along x, centres at y values); else vertically.
    pub horizontal: bool,
    /// Coordinate (dbu) where the first period starts.
    pub start: i64,
    /// Length of one period (dbu).
    pub period: i64,
    /// Tracks of a period: centre offset from the period start and wire width (dbu).
    pub tracks: Vec<(i64, i64)>,
}

impl TrackPattern {
    /// Equal tracks of `width` every `pitch`, the first centred on `start`.
    pub fn uniform(layer: usize, horizontal: bool, start: i64, pitch: i64, width: i64) -> TrackPattern {
        TrackPattern { layer, horizontal, start, period: pitch.max(1), tracks: vec![(0, width)] }
    }

    /// A width spacing pattern: tracks of `widths` separated by `spaces` (the space after each
    /// track, wrapping to the next period), the first track's lower edge at `start`.
    pub fn wsp(layer: usize, horizontal: bool, start: i64, widths: &[i64], spaces: &[i64]) -> Option<TrackPattern> {
        if widths.is_empty() || widths.len() != spaces.len() || widths.iter().chain(spaces).any(|v| *v <= 0) {
            return None;
        }
        let mut at = 0;
        let mut tracks = Vec::with_capacity(widths.len());
        for (w, s) in widths.iter().zip(spaces) {
            tracks.push((at + w / 2, *w));
            at += w + s;
        }
        Some(TrackPattern { layer, horizontal, start, period: at, tracks })
    }

    /// Minimum-width tracks at the layer's minimum pitch, the first centred on 0.
    pub fn from_rules(tech: &Tech, layer: usize, horizontal: bool) -> Option<TrackPattern> {
        let d = tech.layer(layer)?;
        let w = d.min_width.max(tech.grid);
        let pitch = w + d.min_spacing.max(tech.grid);
        Some(TrackPattern::uniform(layer, horizontal, 0, pitch, w))
    }

    /// Track nearest to coordinate `v` (across the tracks): its centre and width.
    pub fn nearest(&self, v: i64) -> Option<(i64, i64)> {
        if self.tracks.is_empty() || self.period <= 0 {
            return None;
        }
        let k = (v - self.start).div_euclid(self.period);
        (k - 1..=k + 1)
            .flat_map(|p| self.tracks.iter().map(move |&(o, w)| (self.start + p * self.period + o, w)))
            .min_by_key(|(c, _)| (c - v).abs())
    }

    /// Track centres and widths between `lo` and `hi`, at most `max` of them.
    pub fn centres(&self, lo: i64, hi: i64, max: usize) -> Vec<(i64, i64)> {
        let mut out = Vec::new();
        if self.tracks.is_empty() || self.period <= 0 {
            return out;
        }
        let mut p = (lo - self.start).div_euclid(self.period) - 1;
        while out.len() < max {
            let base = self.start + p * self.period;
            if base > hi {
                break;
            }
            for &(o, w) in &self.tracks {
                let c = base + o;
                if (lo..=hi).contains(&c) && out.len() < max {
                    out.push((c, w));
                }
            }
            p += 1;
        }
        out
    }

    /// Whether a wire along the tracks, centred at `c` with width `w`, is on a track.
    pub fn fits(&self, c: i64, w: i64) -> bool {
        self.nearest(c).is_some_and(|(tc, tw)| tc == c && tw == w)
    }

    pub fn describe(&self, tech: &Tech) -> String {
        let um = |v: i64| v as f64 / tech.dbu_per_um.max(1) as f64;
        let name = tech.layer(self.layer).map_or("?", |d| d.name.as_str());
        let widths: Vec<String> = self.tracks.iter().map(|t| format!("{}", um(t.1))).collect();
        format!(
            "{name} {} · period {} µm · widths {} µm",
            if self.horizontal { "horizontal" } else { "vertical" },
            um(self.period),
            widths.join(" / ")
        )
    }
}

/// Wires (paths, and rectangles longer than wide along the tracks) on patterned layers whose
/// segments along the tracks are off a track or not of its width: (shape id, where).
pub fn off_track(layout: &Layout, patterns: &[TrackPattern]) -> Vec<(u64, IRect)> {
    let mut out = Vec::new();
    for s in &layout.shapes {
        let mine: Vec<&TrackPattern> = patterns.iter().filter(|p| p.layer == s.layer).collect();
        if mine.is_empty() {
            continue;
        }
        let bad = match &s.geom {
            Geometry::Path { points, width } => points.windows(2).any(|w| {
                let (a, b) = (w[0], w[1]);
                mine.iter().any(|p| {
                    if p.horizontal && a.1 == b.1 && a.0 != b.0 {
                        !p.fits(a.1, *width)
                    } else if !p.horizontal && a.0 == b.0 && a.1 != b.1 {
                        !p.fits(a.0, *width)
                    } else {
                        false
                    }
                })
            }),
            Geometry::Rect(r) => mine.iter().any(|p| {
                if p.horizontal && r.width() > r.height() {
                    !p.fits((r.y0 + r.y1) / 2, r.height())
                } else if !p.horizontal && r.height() > r.width() {
                    !p.fits((r.x0 + r.x1) / 2, r.width())
                } else {
                    false
                }
            }),
            _ => false,
        };
        if bad {
            out.push((s.id, s.bbox()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_and_wsp_tracks() {
        let u = TrackPattern::uniform(3, true, 100, 400, 200);
        assert_eq!(u.nearest(90), Some((100, 200)));
        assert_eq!(u.nearest(-280), Some((-300, 200)));
        assert_eq!(u.centres(0, 1000, 10), vec![(100, 200), (500, 200), (900, 200)]);
        // 200 wide, 200 space, 600 wide, 200 space: period 1200, centres 100 and 700.
        let w = TrackPattern::wsp(3, false, 0, &[200, 600], &[200, 200]).unwrap();
        assert_eq!(w.period, 1200);
        assert_eq!(w.tracks, vec![(100, 200), (700, 600)]);
        assert_eq!(w.nearest(1250), Some((1300, 200)));
        assert!(w.fits(1900, 600) && !w.fits(1900, 200));
        assert!(TrackPattern::wsp(3, true, 0, &[200], &[]).is_none());
    }

    #[test]
    fn off_track_wires_are_listed() {
        let p = TrackPattern::uniform(3, true, 0, 400, 200);
        let mut l = Layout::default();
        let on = l.add(3, Geometry::Path { points: vec![(0, 400), (5000, 400), (5000, 2000)], width: 200 });
        let off = l.add(3, Geometry::Path { points: vec![(0, 450), (5000, 450)], width: 200 });
        let wide = l.add(3, Geometry::Path { points: vec![(0, 800), (5000, 800)], width: 300 });
        let rect_ok = l.add_rect(3, 0, 1100, 3000, 1300);
        let other_layer = l.add(4, Geometry::Path { points: vec![(0, 450), (5000, 450)], width: 200 });
        let bad: Vec<u64> = off_track(&l, &[p]).into_iter().map(|(id, _)| id).collect();
        assert_eq!(bad, vec![off, wide]);
        let _ = (on, rect_ok, other_layer);
    }
}
