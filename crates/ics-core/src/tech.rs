//! Technology / PDK description: layers, GDS mapping, display style and basic rules.

use serde::{Deserialize, Serialize};

/// How a layer's shapes are filled, after the fill styles of Cadence Virtuoso display packets:
/// no fill, solid, an X from corner to corner, or a stipple bitmap drawn in the fill colour
/// (one bit per screen point, anchored to the window so neighbouring shapes line up).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FillStyle {
    #[default]
    Solid,
    /// `/` lines (Virtuoso `slash`).
    #[serde(alias = "slash")]
    Hatch,
    /// `\` lines (Virtuoso `backSlash`).
    #[serde(alias = "backslash")]
    BackHatch,
    /// Outline with an X from corner to corner (Virtuoso fill style `X`, used for cuts).
    #[serde(alias = "x")]
    Cross,
    /// Staggered dots (Virtuoso `dots`).
    Dots,
    /// No fill, outline only.
    Outline,
    /// Horizontal lines (Virtuoso `hLine`).
    HLine,
    /// Vertical lines (Virtuoso `vLine`).
    VLine,
    /// Horizontal and vertical lines (Virtuoso `grid`).
    Grid,
    /// Both diagonals (Virtuoso `x` stipple).
    CrossHatch,
    /// One-point checkerboard, a 50 % screen (Virtuoso `checker`).
    Checker,
    /// Brick wall (Virtuoso `brick`).
    Brick,
    /// Horizontal zigzag (Virtuoso `hZigZag`).
    ZigZag,
    /// Widely spaced dots (Virtuoso `dot1`).
    SparseDots,
}

impl FillStyle {
    pub const ALL: [FillStyle; 14] = [
        FillStyle::Solid,
        FillStyle::Outline,
        FillStyle::Cross,
        FillStyle::Hatch,
        FillStyle::BackHatch,
        FillStyle::CrossHatch,
        FillStyle::HLine,
        FillStyle::VLine,
        FillStyle::Grid,
        FillStyle::Dots,
        FillStyle::SparseDots,
        FillStyle::Checker,
        FillStyle::Brick,
        FillStyle::ZigZag,
    ];

    /// Whether the style draws a stipple pattern (everything but solid, outline and X).
    pub fn is_stipple(self) -> bool {
        !matches!(self, FillStyle::Solid | FillStyle::Outline | FillStyle::Cross)
    }

    /// The built-in stipple bitmap of a stipple style (rows top to bottom, `1` = set).
    pub fn bitmap(self) -> Option<Stipple> {
        let rows: &[&str] = match self {
            FillStyle::Solid | FillStyle::Outline | FillStyle::Cross => return None,
            FillStyle::Hatch => &["00000001", "00000010", "00000100", "00001000", "00010000", "00100000", "01000000", "10000000"],
            FillStyle::BackHatch => {
                &["10000000", "01000000", "00100000", "00010000", "00001000", "00000100", "00000010", "00000001"]
            }
            FillStyle::CrossHatch => {
                &["10000001", "01000010", "00100100", "00011000", "00011000", "00100100", "01000010", "10000001"]
            }
            FillStyle::HLine => &["1111", "0000", "0000", "0000"],
            FillStyle::VLine => &["1000", "1000", "1000", "1000"],
            FillStyle::Grid => &["11111111", "10000000", "10000000", "10000000", "10000000", "10000000", "10000000", "10000000"],
            FillStyle::Dots => &["1000", "0000", "0010", "0000"],
            FillStyle::SparseDots => {
                &["10000000", "00000000", "00000000", "00000000", "00001000", "00000000", "00000000", "00000000"]
            }
            FillStyle::Checker => &["10", "01"],
            FillStyle::Brick => &["11111111", "10000000", "10000000", "10000000", "11111111", "00001000", "00001000", "00001000"],
            FillStyle::ZigZag => {
                &["10000000", "01000001", "00100010", "00010100", "00001000", "00000000", "00000000", "00000000"]
            }
        };
        Some(Stipple { rows: rows.iter().map(|r| r.to_string()).collect() })
    }

    /// Lower-case name as written in technology files.
    pub fn name(self) -> &'static str {
        match self {
            FillStyle::Solid => "solid",
            FillStyle::Hatch => "hatch",
            FillStyle::BackHatch => "backhatch",
            FillStyle::Cross => "cross",
            FillStyle::Dots => "dots",
            FillStyle::Outline => "outline",
            FillStyle::HLine => "hline",
            FillStyle::VLine => "vline",
            FillStyle::Grid => "grid",
            FillStyle::CrossHatch => "crosshatch",
            FillStyle::Checker => "checker",
            FillStyle::Brick => "brick",
            FillStyle::ZigZag => "zigzag",
            FillStyle::SparseDots => "sparsedots",
        }
    }
}

/// A stipple bitmap, as in a Virtuoso display resource file (`drDefineStipple`): rows from top
/// to bottom of `0` / `1`, repeated over the shape. Short rows are padded with `0`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Stipple {
    pub rows: Vec<String>,
}

impl Stipple {
    /// From rows of 0 / 1 values; `None` when empty.
    pub fn from_bits(rows: &[Vec<u8>]) -> Option<Stipple> {
        let s = Stipple { rows: rows.iter().map(|r| r.iter().map(|&b| if b == 1 { '1' } else { '0' }).collect()).collect() };
        (s.width() > 0 && s.height() > 0).then_some(s)
    }

    pub fn width(&self) -> usize {
        self.rows.iter().map(String::len).max().unwrap_or(0)
    }

    pub fn height(&self) -> usize {
        self.rows.len()
    }

    /// Bit at column `x`, row `y` (both wrap around).
    pub fn bit(&self, x: usize, y: usize) -> bool {
        let (w, h) = (self.width(), self.height());
        if w == 0 || h == 0 {
            return false;
        }
        self.rows[y % h].as_bytes().get(x % w) == Some(&b'1')
    }

    /// The 0 / 1 rows of the bitmap, padded to its width.
    pub fn bits(&self) -> Vec<Vec<u8>> {
        (0..self.height()).map(|y| (0..self.width()).map(|x| self.bit(x, y) as u8).collect()).collect()
    }

    /// The built-in fill style that looks most like this bitmap.
    pub fn closest_fill(&self) -> FillStyle {
        closest_fill(&self.bits())
    }

    /// Share of set bits (0 to 1).
    pub fn density(&self) -> f64 {
        let (w, h) = (self.width(), self.height());
        if w * h == 0 {
            return 0.0;
        }
        self.rows.iter().map(|r| r.bytes().filter(|&b| b == b'1').count()).sum::<usize>() as f64 / (w * h) as f64
    }
}

/// Fill style closest to a stipple bitmap (rows of 0 / 1).
pub fn closest_fill(rows: &[Vec<u8>]) -> FillStyle {
    let h = rows.len();
    let w = rows.iter().map(Vec::len).max().unwrap_or(0);
    if h == 0 || w == 0 {
        return FillStyle::Solid;
    }
    let at = |r: usize, c: usize| rows[r % h].get(c % w).copied().unwrap_or(0) == 1;
    let on: usize = (0..h).flat_map(|r| (0..w).map(move |c| (r, c))).filter(|&(r, c)| at(r, c)).count();
    let density = on as f64 / (h * w) as f64;
    if density == 0.0 {
        return FillStyle::Outline;
    }
    if density > 0.9 {
        return FillStyle::Solid;
    }
    if let Some(f) = FillStyle::ALL.into_iter().find(|f| f.bitmap().is_some_and(|b| same_tiling(rows, &b))) {
        return f;
    }
    // How often a set bit continues down-right ('\' on screen), down-left ('/'), right and down.
    let (mut back, mut fwd, mut right, mut down) = (0, 0, 0, 0);
    for r in 0..h {
        for c in 0..w {
            if at(r, c) {
                back += at(r + 1, c + 1) as usize;
                fwd += at(r + 1, c + w - 1) as usize;
                right += at(r, c + 1) as usize;
                down += at(r + 1, c) as usize;
            }
        }
    }
    let f = |n: usize| n as f64 / on as f64;
    let (back, fwd, right, down) = (f(back), f(fwd), f(right), f(down));
    let full_row = (0..h).any(|r| (0..w).all(|c| at(r, c)));
    let full_col = (0..w).any(|c| (0..h).all(|r| at(r, c)));
    let off_rows = (0..h).any(|r| !(0..w).all(|c| at(r, c)) && (0..w).any(|c| at(r, c)));
    if full_row && full_col {
        FillStyle::Grid
    } else if full_row {
        if off_rows { FillStyle::Brick } else { FillStyle::HLine }
    } else if full_col {
        FillStyle::VLine
    } else if back > 0.6 && fwd < 0.3 {
        FillStyle::BackHatch
    } else if fwd > 0.6 && back < 0.3 {
        FillStyle::Hatch
    } else if density >= 0.4 {
        FillStyle::Checker
    } else if back + fwd > 0.9 {
        FillStyle::CrossHatch
    } else if right > 0.6 {
        FillStyle::HLine
    } else if down > 0.6 {
        FillStyle::VLine
    } else if density < 0.05 {
        FillStyle::SparseDots
    } else {
        FillStyle::Dots
    }
}

/// Whether the bitmap `rows` is the built-in stipple `b`, tiled and shifted.
fn same_tiling(rows: &[Vec<u8>], b: &Stipple) -> bool {
    let (h, w) = (rows.len(), rows.iter().map(Vec::len).max().unwrap_or(0));
    let (bw, bh) = (b.width(), b.height());
    if w == 0 || h == 0 || bw == 0 || bh == 0 {
        return false;
    }
    let lcm = |a: usize, b: usize| a / gcd(a, b) * b;
    let (tw, th) = (lcm(w, bw), lcm(h, bh));
    if tw * th > 4096 {
        return false;
    }
    let at = |r: usize, c: usize| rows[r % h].get(c % w).copied().unwrap_or(0) == 1;
    (0..bh).any(|dy| (0..bw).any(|dx| (0..th).all(|y| (0..tw).all(|x| at(y, x) == b.bit(x + dx, y + dy)))))
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Outline line style, after the Virtuoso display line styles (`drDefineLineStyle`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LineStyle {
    #[default]
    Solid,
    Dashed,
    #[serde(alias = "dots")]
    Dotted,
    DashDot,
    ShortDash,
    DoubleDash,
    /// Solid, three points wide (Virtuoso `thickLine`).
    #[serde(alias = "thickline")]
    Thick,
    /// No outline (Virtuoso `hidden`, or the fill style `stipple`).
    #[serde(alias = "hidden")]
    None,
}

impl LineStyle {
    pub const ALL: [LineStyle; 8] = [
        LineStyle::Solid,
        LineStyle::Dashed,
        LineStyle::ShortDash,
        LineStyle::Dotted,
        LineStyle::DashDot,
        LineStyle::DoubleDash,
        LineStyle::Thick,
        LineStyle::None,
    ];

    /// Line width in screen points (0 for none).
    pub fn width(self) -> f32 {
        match self {
            LineStyle::None => 0.0,
            LineStyle::Thick => 3.0,
            _ => 1.0,
        }
    }

    /// Repeating on / off pattern along the line, one entry per screen point (empty = solid).
    pub fn pattern(self) -> &'static [u8] {
        match self {
            LineStyle::Solid | LineStyle::Thick | LineStyle::None => &[],
            LineStyle::Dashed => &[1, 1, 1, 1, 1, 1, 0, 0, 0],
            LineStyle::ShortDash => &[1, 1, 1, 0, 0, 0],
            LineStyle::Dotted => &[1, 0, 0],
            LineStyle::DashDot => &[1, 1, 1, 1, 1, 1, 0, 0, 1, 0, 0],
            LineStyle::DoubleDash => &[1, 1, 1, 1, 1, 1, 0, 0, 1, 1, 0, 0],
        }
    }

    /// Closest style to a Virtuoso line style: its width and on / off pattern.
    pub fn from_pattern(width: u32, bits: &[u8]) -> LineStyle {
        let on = bits.iter().filter(|&&b| b == 1).count();
        if !bits.is_empty() && on == 0 {
            return LineStyle::None;
        }
        if on == bits.len() {
            return if width >= 2 { LineStyle::Thick } else { LineStyle::Solid };
        }
        // Lengths of the runs of set bits, taken cyclically from the first gap.
        let start = bits.iter().position(|&b| b == 0).unwrap_or(0);
        let mut runs = vec![];
        let mut n = 0;
        for i in 0..bits.len() {
            if bits[(start + i) % bits.len()] == 1 {
                n += 1;
            } else if n > 0 {
                runs.push(n);
                n = 0;
            }
        }
        if n > 0 {
            runs.push(n);
        }
        let longest = runs.iter().copied().max().unwrap_or(0);
        match runs.as_slice() {
            [_, _, ..] if runs.iter().any(|&r| r <= 1) && longest > 1 => LineStyle::DashDot,
            [_, _, ..] if runs.iter().all(|&r| r > 1) && runs.iter().any(|&r| r != longest) => LineStyle::DoubleDash,
            _ if longest <= 1 => LineStyle::Dotted,
            _ if longest <= 3 => LineStyle::ShortDash,
            _ => LineStyle::Dashed,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            LineStyle::Solid => "solid",
            LineStyle::Dashed => "dashed",
            LineStyle::Dotted => "dotted",
            LineStyle::DashDot => "dashdot",
            LineStyle::ShortDash => "shortdash",
            LineStyle::DoubleDash => "doubledash",
            LineStyle::Thick => "thick",
            LineStyle::None => "none",
        }
    }
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerDef {
    pub name: String,
    pub gds_layer: u16,
    #[serde(default)]
    pub gds_datatype: u16,
    /// RGBA display colour.
    pub color: [u8; 4],
    #[serde(default)]
    pub fill: FillStyle,
    /// Stipple bitmap drawn in place of the fill style's built-in one (from a Virtuoso display
    /// file, say); only used by stipple fill styles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stipple: Option<Stipple>,
    /// Outline colour when it differs from the fill colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outline: Option<[u8; 3]>,
    /// Outline line style.
    #[serde(default, skip_serializing_if = "is_default")]
    pub line: LineStyle,
    /// Minimum width in database units (0 = unchecked).
    #[serde(default)]
    pub min_width: i64,
    /// Minimum same-layer spacing in database units (0 = unchecked).
    #[serde(default)]
    pub min_spacing: i64,
    /// Whether the layer is a routing layer that carries net labels.
    #[serde(default)]
    pub routing: bool,
    /// Minimum area of a piece in dbu² (0 = unchecked).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub min_area: i64,
    /// Stream pair the layer's text labels are written as when it differs from the shapes'
    /// (a stream map row with purpose `label`; set by `stream_map::export_tech`, not stored).
    #[serde(skip)]
    pub text_stream: Option<(u16, u16)>,
}

fn is_zero(v: &i64) -> bool {
    *v == 0
}

impl LayerDef {
    /// With the outline line style `line`.
    pub fn line(mut self, line: LineStyle) -> LayerDef {
        self.line = line;
        self
    }

    /// The stipple pattern the layer is drawn with: its own bitmap or the fill style's
    /// built-in one; `None` for solid, outline and X fills.
    pub fn pattern(&self) -> Option<Stipple> {
        if !self.fill.is_stipple() {
            return None;
        }
        self.stipple.clone().filter(|s| s.width() > 0 && s.height() > 0).or_else(|| self.fill.bitmap())
    }

    /// Placeholder for a GDS layer without a definition: named `L{layer}D{datatype}`,
    /// hatched, with the `k`-th colour of [`auto_color`].
    pub fn generated(gds_layer: u16, gds_datatype: u16, k: usize) -> LayerDef {
        LayerDef {
            name: format!("L{gds_layer}D{gds_datatype}"),
            gds_layer,
            gds_datatype,
            color: auto_color(k),
            fill: FillStyle::Hatch,
            stipple: None,
            outline: None,
            line: LineStyle::Solid,
            min_width: 0,
            min_spacing: 0,
            routing: false,
            min_area: 0,
            text_stream: None,
        }
    }
}

/// The `k`-th of a sequence of well separated colours (golden-angle hue steps).
pub fn auto_color(k: usize) -> [u8; 4] {
    let h = (k as f64 * 137.508 + 20.0) % 360.0 / 60.0;
    let (s, v) = (0.65, 0.95);
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let u = |f: f64| ((f + m) * 255.0).round() as u8;
    [u(r), u(g), u(b), 160]
}

/// A via / contact between two conducting layers, with its cut rules (dbu).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViaDef {
    pub name: String,
    pub bottom: String,
    pub cut: String,
    pub top: String,
    pub cut_size: i64,
    pub cut_spacing: i64,
    pub enc_bottom: i64,
    pub enc_top: i64,
    /// Resistance of one cut in Ω (0 = none).
    #[serde(default)]
    pub resistance: f64,
    /// Electromigration limit: maximum DC current per cut in A (0 = unchecked).
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub em: f64,
}

fn is_zero_f64(v: &f64) -> bool {
    *v == 0.0
}

/// Wire capacitance to substrate of a conducting layer, used by parasitic extraction (`pex`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerCap {
    pub layer: String,
    /// Area capacitance in aF per µm².
    pub area: f64,
    /// Fringe capacitance in aF per µm of edge.
    pub fringe: f64,
    /// Lateral coupling in aF: a parallel neighbour on the same layer at spacing s over a run
    /// length L adds `coupling · L / s` (0 = none).
    #[serde(default)]
    pub coupling: f64,
    /// Sheet resistance in Ω per square (0 = no wire resistance).
    #[serde(default)]
    pub resistance: f64,
    /// Electromigration limit: maximum DC current per µm of wire width in A (0 = unchecked).
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub em: f64,
}

/// Which technology layers and vias play the fixed roles of device recognition and the PCells.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayerRoles {
    /// Diffusion layers (the first is drawn by the MOS PCell); extraction merges them all.
    pub diffusion: Vec<String>,
    /// Layer drawn for well / substrate taps (empty = the first diffusion layer).
    pub tap: String,
    pub poly: String,
    pub nwell: String,
    /// N+ implant; empty when the process draws none (n+ is diffusion outside the p implant).
    pub nimplant: String,
    pub pimplant: String,
    /// Via from diffusion to the first metal.
    pub diff_contact: String,
    /// Via from poly to the first metal.
    pub poly_contact: String,
    /// Layer for PCell pin labels.
    pub label: String,
}

impl Default for LayerRoles {
    fn default() -> Self {
        LayerRoles {
            diffusion: vec!["active".into()],
            tap: String::new(),
            poly: "poly".into(),
            nwell: "nwell".into(),
            nimplant: "nplus".into(),
            pimplant: "pplus".into(),
            diff_contact: "contact".into(),
            poly_contact: "polycon".into(),
            label: "text".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MosType {
    Nmos,
    Pmos,
}

/// A MOS device of the PDK. A gate (poly over diffusion) of the right polarity is this device
/// when every `layers` marker covers it and no `exclude` marker does; the first match wins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MosDef {
    /// Model (or subcircuit) name used in netlists and LVS.
    pub name: String,
    #[serde(rename = "type")]
    pub kind: MosType,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// The model is a subcircuit: instances are netlisted as `X` elements.
    #[serde(default)]
    pub subckt: bool,
}

/// A resistor of the PDK. Its body is the conducting `layer` where every `layers` marker covers
/// it and no `exclude` marker does; the body is cut out of the layer's conductor, so the pieces
/// of `layer` touching the body at two opposite ends are its terminals. R = `sheet` · L / W.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResDef {
    pub name: String,
    pub layer: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// Sheet resistance in Ω per square.
    pub sheet: f64,
}

/// A capacitor of the PDK: each overlap of conducting layers `bottom` and `top` where every
/// `layers` marker covers it and no `exclude` marker does. C = `area` · A + `perimeter` · P.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapDef {
    pub name: String,
    pub bottom: String,
    pub top: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// Area capacitance in fF per µm².
    pub area: f64,
    /// Perimeter capacitance in fF per µm.
    #[serde(default)]
    pub perimeter: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BjtType {
    Npn,
    Pnp,
}

/// A bipolar transistor of the PDK, recognised under its markers. Extraction knows the vertical
/// PNP of CMOS processes: the emitter is p+ diffusion inside an nwell (not under a gate) that
/// every `layers` marker covers and no `exclude` marker does, the base that nwell and the
/// collector the substrate. With `unit_area` (µm²) the extracted area factor is the emitter
/// area over it, else 1 per emitter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BjtDef {
    /// Model (or subcircuit) name used in netlists and LVS.
    pub name: String,
    #[serde(rename = "type")]
    pub kind: BjtType,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_area: Option<f64>,
}

/// SPICE model library of the PDK.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ModelLib {
    /// Model file path, read by the simulator.
    pub path: String,
    /// `.lib` sections; empty means the file is `.include`d.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub corners: Vec<String>,
    /// Corner used when the simulation setup names none (empty = the first corner).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default_corner: String,
    /// Unit of MOS geometry parameters (`w`, `l`, `ad`, ...) the models expect.
    #[serde(default)]
    pub length_unit: LengthUnit,
    /// Further model files loaded with the same corner (e.g. high-voltage devices).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<String>,
    /// Compiled Verilog-A models (OpenVAF `.osdi`) ngspice loads before reading the deck.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub osdi: Vec<String>,
    /// The device models take the layout-dependent-effect instance parameters `nf sa sb sd`
    /// (BSIM4 stress), which post-layout decks then carry.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub lde: bool,
}

/// Unit of device geometry in netlists: metres (`w=1u`) or microns (`w=1`, for model libraries
/// that set `.option scale=1e-6`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LengthUnit {
    #[default]
    M,
    Um,
}

impl LengthUnit {
    /// Factor from metres to this unit.
    pub fn per_metre(self) -> f64 {
        match self {
            LengthUnit::M => 1.0,
            LengthUnit::Um => 1e6,
        }
    }
}

impl ModelLib {
    /// The deck lines loading the models at `corner` (empty = default corner).
    pub fn spice_line(&self, corner: &str) -> String {
        let corner = [corner.trim(), self.default_corner.as_str()]
            .into_iter()
            .find(|c| !c.is_empty())
            .or(self.corners.first().map(String::as_str));
        let osdi = self.osdi.iter().map(|p| {
            let p = p.replace('\\', "/");
            let p = if p.contains(char::is_whitespace) { format!("\"{p}\"") } else { p };
            format!(".control\npre_osdi {p}\n.endc")
        });
        osdi.chain(std::iter::once(&self.path).chain(&self.extra).map(|p| match corner {
            Some(c) => format!(".lib \"{p}\" {c}"),
            None => format!(".include \"{p}\""),
        }))
        .collect::<Vec<_>>()
        .join("\n")
    }
}

/// [`Tech::mos_device`] over a device list.
pub fn mos_device_in<'a>(devices: &'a [MosDef], pmos: bool, model: &str) -> Option<&'a MosDef> {
    let model = model.trim();
    if let Some(d) = devices.iter().find(|d| d.name.eq_ignore_ascii_case(model)) {
        return Some(d);
    }
    let kind = if pmos { MosType::Pmos } else { MosType::Nmos };
    let generic = model.is_empty() || model.eq_ignore_ascii_case("nmos") || model.eq_ignore_ascii_case("pmos");
    generic.then(|| devices.iter().find(|d| d.kind == kind)).flatten()
}

/// Device layout rules used by the PCells that single-layer width / spacing cannot express (dbu).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceRules {
    /// Diffusion contact to gate spacing.
    pub gate_contact_space: i64,
    /// Poly extension past diffusion.
    pub poly_endcap: i64,
    /// Implant enclosure of diffusion.
    pub implant_enc: i64,
    /// Nwell enclosure of p diffusion.
    pub nwell_enc: i64,
    /// Enclosure of the diffusion by a device's marker layers ([`MosDef::layers`]).
    pub marker_enc: i64,
    /// Share of each corner square a bent gate's W counts, from 0 (W is the inner, shorter
    /// diffusion edge) to 1 (W is the mean of the source and drain edges); used by extraction.
    pub bend_corner: f64,
}

impl Default for DeviceRules {
    fn default() -> Self {
        DeviceRules {
            gate_contact_space: 160,
            poly_endcap: 220,
            implant_enc: 180,
            nwell_enc: 430,
            marker_enc: 180,
            bend_corner: 1.0,
        }
    }
}

/// The layer that carries net names (text labels) for a conducting layer, e.g. `Metal1.text`
/// for `Metal1`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PinLabel {
    pub layer: String,
    pub label: String,
}

/// An extra layer the PCells draw: `layer` covering every piece of `around` grown by `enc` (dbu),
/// for the PCell kinds in `kinds` (all when empty).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PcellLayer {
    pub layer: String,
    /// `poly_contact` (poly contact cuts on poly outside diffusion), `diff_contact` (diffusion
    /// contact cuts on diffusion), `gate` (poly over diffusion), `diffusion`, `poly`, or a layer
    /// name.
    pub around: String,
    pub enc: i64,
    /// PCell kind names ([`crate::pcell::PCell::kind_name`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tech {
    pub name: String,
    /// Database units per micron (1000 => 1 dbu = 1 nm).
    pub dbu_per_um: i64,
    /// Layout manufacturing / snap grid in dbu.
    pub grid: i64,
    pub layers: Vec<LayerDef>,
    #[serde(default)]
    pub vias: Vec<ViaDef>,
    /// DRC rule deck (see `drc`); `None` uses the deck derived from the layer rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drc_deck: Option<String>,
    /// Shared DRC deck file this technology's deck comes from (absolute, or relative to the
    /// project directory). When set, `drc_deck` holds the file's deck in memory: it is read when
    /// the project opens, written back when the deck is changed (see `Project::set_drc_deck`) and
    /// not saved in `tech.json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drc_deck_file: Option<String>,
    /// Parasitic capacitance coefficients per conducting layer (layers without one are ignored).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub caps: Vec<LayerCap>,
    #[serde(default)]
    pub roles: LayerRoles,
    #[serde(default)]
    pub rules: DeviceRules,
    /// PDK MOS devices; empty means generic `nmos` / `pmos`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<MosDef>,
    /// PDK resistors recognised by extraction and LVS.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resistors: Vec<ResDef>,
    /// PDK capacitors recognised by extraction and LVS.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capacitors: Vec<CapDef>,
    /// PDK bipolar transistors recognised by extraction and LVS.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bjts: Vec<BjtDef>,
    /// PDK model library; `None` uses the built-in generic models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<ModelLib>,
    /// External sign-off DRC / LVS runsets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signoff: Vec<crate::signoff::Runset>,
    /// Extra layers drawn by the PCells.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pcell_layers: Vec<PcellLayer>,
    /// Deck lines appended to the deck derived from the layer rules (when `drc_deck` is unset).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drc_extra: Vec<String>,
    /// Net label layers per conducting layer; others carry their labels themselves.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pin_labels: Vec<PinLabel>,
    /// Stream (GDSII / OASIS) layer map used by File > Import / Export (see `stream_map`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stream_map: Vec<StreamMapEntry>,
    /// Vertical structure over the silicon, for image sensor pixel optics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross_section: Option<CrossSection>,
    /// Global process variation of model parameters (from WAT data, `ics corners --variation`),
    /// drawn by Monte Carlo in place of its generic threshold / current factor sigmas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variation: Option<crate::process_data::ProcessVariation>,
    /// Device mismatch measured on matched pairs (Pelgrom coefficients per polarity, from
    /// `ics mismatch`), drawn by Monte Carlo in place of its generic Avt / Abeta.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mismatch: Option<crate::mismatch::MismatchModel>,
}

/// The process cross-section over the silicon, from which a pixel's optical stack follows
/// (see [`CrossSection::pixel_stack_toml`]). Lengths in µm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossSection {
    /// Levels from the silicon surface up. Thin films directly on the silicon are `film`
    /// (treated coherently); metal levels name the layout layer (or derived-layer expression)
    /// whose shapes are `metal` within the level.
    pub levels: Vec<Level>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub photodiode: Option<PhotodiodeDef>,
    /// Colour filter, planarisation and microlens over the top level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pixel: Option<PixelOptics>,
    /// Carrier diffusion in the silicon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub silicon: Option<SiliconDef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Level {
    pub name: String,
    pub material: String,
    pub thickness: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metal: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub film: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhotodiodeDef {
    /// Layer or derived-layer expression of the photodiode's area.
    pub layer: String,
    /// Collection depth range below the silicon surface.
    pub top: f64,
    pub bottom: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PixelOptics {
    pub color_filter: f64,
    pub planarisation: f64,
    pub microlens_height: f64,
    /// Centre the microlens over the photodiode.
    #[serde(default)]
    pub center_on_photodiode: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SiliconDef {
    pub diffusion_length: f64,
    pub epi: f64,
}

impl CrossSection {
    /// The pixel stack (TOML for the pixel optics model) of `cell`, read from its layout through
    /// `boundary` (the pixel frame).
    pub fn pixel_stack_toml(&self, tech_name: &str, cell: &str, boundary: Option<&str>) -> Result<String, String> {
        let films = self.levels.iter().take_while(|l| l.film).count();
        if self.levels.iter().skip(films).any(|l| l.film) {
            return Err("cross section: thin films must sit directly on the silicon".into());
        }
        for l in &self.levels {
            if l.thickness.is_nan() || l.thickness <= 0.0 || l.layer.is_some() != l.metal.is_some() {
                return Err(format!("cross section level '{}': needs a positive thickness, and layer with metal", l.name));
            }
        }
        let q = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
        let mut t = format!(
            "# Pixel stack of {cell} from the {tech_name} cross section.\nname = {}\n\n[layout]\ncell = {}\n",
            q(&format!("{cell} ({tech_name})")),
            q(cell)
        );
        if let Some(b) = boundary {
            t += &format!("boundary = {}\n", q(b));
        }
        if let Some(px) = &self.pixel {
            t += &format!("\n[microlens]\nheight = {}\n", px.microlens_height);
            if px.center_on_photodiode && self.photodiode.is_some() {
                t += "center = \"photodiode\"\n";
            }
            t += &format!("\n[[layers]]\nname = \"planarisation\"\nmaterial = \"resin\"\nthickness = {}\n", px.planarisation);
            t += &format!("\n[[layers]]\nname = \"colour filter\"\nkind = \"color_filter\"\nthickness = {}\n", px.color_filter);
        }
        for l in self.levels.iter().skip(films).rev() {
            t += &format!("\n[[layers]]\nname = {}\nmaterial = {}\nthickness = {}\n", q(&l.name), q(&l.material), l.thickness);
            if let (Some(layer), Some(metal)) = (&l.layer, &l.metal) {
                t += &format!("pattern = {{ material = {}, layout = {} }}\n", q(metal), q(layer));
            }
        }
        for l in self.levels.iter().take(films).rev() {
            t += &format!("\n[[arc]]\nmaterial = {}\nthickness = {}\n", q(&l.material), l.thickness);
        }
        let pd = self.photodiode.as_ref().ok_or("cross section: no photodiode definition")?;
        t += &format!("\n[photodiode]\ntop = {}\nbottom = {}\nlayout = {}\n", pd.top, pd.bottom, q(&pd.layer));
        if let Some(si) = &self.silicon {
            t += &format!("\n[silicon]\ndiffusion_length = {}\nepi = {}\n", si.diffusion_length, si.epi);
        }
        Ok(t)
    }

    /// A generic 180 nm-class back end over a pinned photodiode, for the demo technology.
    pub fn generic180() -> CrossSection {
        let lv = |name: &str, material: &str, thickness: f64, metal: Option<(&str, &str)>, film: bool| Level {
            name: name.into(),
            material: material.into(),
            thickness,
            layer: metal.map(|m| m.0.into()),
            metal: metal.map(|m| m.1.into()),
            film,
        };
        CrossSection {
            levels: vec![
                lv("anti-reflection nitride", "si3n4", 0.06, None, true),
                lv("PMD", "sio2", 0.8, None, false),
                lv("metal 1", "sio2", 0.5, Some(("metal1", "al")), false),
                lv("IMD 1", "sio2", 0.8, None, false),
                lv("metal 2", "sio2", 0.5, Some(("metal2", "al")), false),
                lv("IMD 2", "sio2", 0.8, None, false),
                lv("metal 3", "sio2", 0.6, Some(("metal3", "al")), false),
                lv("passivation", "si3n4", 0.5, None, false),
            ],
            photodiode: Some(PhotodiodeDef { layer: "active wide 1".into(), top: 0.1, bottom: 3.0 }),
            pixel: Some(PixelOptics { color_filter: 0.9, planarisation: 0.4, microlens_height: 2.5, center_on_photodiode: true }),
            silicon: Some(SiliconDef { diffusion_length: 20.0, epi: 5.0 }),
        }
    }
}

/// A row of the stream layer map: shapes on stream `(stream_layer, datatype)` are read onto
/// technology layer `layer`, and the first row naming a layer sets the pair it is written as.
/// An empty `layer` drops the pair on import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StreamMapEntry {
    pub layer: String,
    pub stream_layer: u16,
    #[serde(default)]
    pub datatype: u16,
    /// The row carries the layer's text labels only (Virtuoso purpose `label` or `text`):
    /// texts on the pair are read onto the layer, and its labels are written as the pair.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub text: bool,
}

impl Tech {
    pub fn layer(&self, idx: usize) -> Option<&LayerDef> {
        self.layers.get(idx)
    }

    pub fn layer_index(&self, name: &str) -> Option<usize> {
        self.layers.iter().position(|l| l.name.eq_ignore_ascii_case(name))
    }

    pub fn via(&self, name: &str) -> Option<&ViaDef> {
        self.vias.iter().find(|v| v.name.eq_ignore_ascii_case(name))
    }

    /// The DRC deck: `drc_deck`, or the deck derived from the layer and via rules.
    pub fn deck_text(&self) -> String {
        self.drc_deck.clone().unwrap_or_else(|| crate::drc::default_deck_text(self))
    }

    pub fn layer_cap(&self, name: &str) -> Option<&LayerCap> {
        self.caps.iter().find(|c| c.layer.eq_ignore_ascii_case(name))
    }

    /// Diffusion layers of [`LayerRoles::diffusion`] present in the technology.
    pub fn diffusion_layers(&self) -> Vec<usize> {
        self.roles.diffusion.iter().filter_map(|n| self.layer_index(n)).collect()
    }

    pub fn diffusion(&self) -> Option<usize> {
        self.diffusion_layers().first().copied()
    }

    pub fn poly(&self) -> Option<usize> {
        self.layer_index(&self.roles.poly)
    }

    pub fn nwell(&self) -> Option<usize> {
        self.layer_index(&self.roles.nwell)
    }

    pub fn implant(&self, pmos: bool) -> Option<usize> {
        self.layer_index(if pmos { &self.roles.pimplant } else { &self.roles.nimplant })
    }

    pub fn tap_layer(&self) -> Option<usize> {
        if self.roles.tap.is_empty() { self.diffusion() } else { self.layer_index(&self.roles.tap) }
    }

    pub fn label_layer(&self) -> Option<usize> {
        self.layer_index(&self.roles.label)
    }

    /// Layer for net labels on conducting layer `layer`: its [`PinLabel`] layer, else the
    /// conducting layer itself.
    pub fn net_label_layer(&self, layer: usize) -> Option<usize> {
        let name = &self.layer(layer)?.name;
        match self.pin_labels.iter().find(|p| p.layer.eq_ignore_ascii_case(name)) {
            Some(p) => self.layer_index(&p.label),
            None => Some(layer),
        }
    }

    /// The conducting layer whose net labels `label_layer` carries ([`Tech::pin_labels`]).
    pub fn labelled_layer(&self, label_layer: usize) -> Option<usize> {
        let name = &self.layer(label_layer)?.name;
        self.pin_labels.iter().find(|p| p.label.eq_ignore_ascii_case(name)).and_then(|p| self.layer_index(&p.layer))
    }

    pub fn diff_contact(&self) -> Option<&ViaDef> {
        self.via(&self.roles.diff_contact)
    }

    pub fn poly_contact(&self) -> Option<&ViaDef> {
        self.via(&self.roles.poly_contact)
    }

    /// The first metal: top layer of the diffusion contact.
    pub fn metal1(&self) -> Option<usize> {
        self.diff_contact().and_then(|v| self.layer_index(&v.top))
    }

    /// The metal above [`Tech::metal1`]: top of the via whose bottom is metal1.
    pub fn metal2(&self) -> Option<usize> {
        let m1 = self.metal1()?;
        self.vias.iter().filter(|v| self.layer_index(&v.bottom) == Some(m1)).find_map(|v| self.layer_index(&v.top))
    }

    /// PDK device for a schematic MOS with model `model`: the device of that name, else (for the
    /// generic names `nmos` / `pmos` or an empty model) the first PDK device of that polarity.
    /// `None` when the technology defines no devices.
    /// Unit of device geometry in netlists for this technology's models.
    pub fn length_unit(&self) -> LengthUnit {
        self.models.as_ref().map_or_else(LengthUnit::default, |m| m.length_unit)
    }

    pub fn mos_device(&self, pmos: bool, model: &str) -> Option<&MosDef> {
        mos_device_in(&self.devices, pmos, model)
    }

    pub fn layer_by_gds(&self, layer: u16, datatype: u16) -> Option<usize> {
        self.layers.iter().position(|l| l.gds_layer == layer && l.gds_datatype == datatype)
    }

    /// A generic, illustrative 180nm-class CMOS technology. Not a real foundry PDK.
    pub fn generic() -> Tech {
        let l = |name: &str, gds: u16, color: [u8; 4], fill: FillStyle, w: i64, s: i64, routing: bool| LayerDef {
            name: name.into(),
            gds_layer: gds,
            gds_datatype: 0,
            color,
            fill,
            stipple: None,
            outline: None,
            line: LineStyle::Solid,
            min_width: w,
            min_spacing: s,
            routing,
            min_area: 0,
            text_stream: None,
        };
        let v = |name: &str, bottom: &str, cut: &str, top: &str, size: i64, space: i64, eb: i64, et: i64, ohms: f64, em: f64| {
            ViaDef {
                name: name.into(),
                bottom: bottom.into(),
                cut: cut.into(),
                top: top.into(),
                cut_size: size,
                cut_spacing: space,
                enc_bottom: eb,
                enc_top: et,
                resistance: ohms,
                em,
            }
        };
        Tech {
            name: "generic180".into(),
            dbu_per_um: 1000,
            grid: 10,
            layers: vec![
                // Display packets in the manner of a Virtuoso display.drf: stipples drawn in the
                // layer colour, X for cuts, dashed outlines for wells, implants and the boundary.
                l("nwell", 1, [120, 200, 120, 70], FillStyle::SparseDots, 860, 1400, false).line(LineStyle::Dashed),
                l("active", 2, [70, 190, 90, 150], FillStyle::Dots, 220, 280, false),
                l("nplus", 3, [230, 120, 60, 60], FillStyle::Outline, 440, 440, false).line(LineStyle::ShortDash),
                l("pplus", 4, [200, 160, 60, 60], FillStyle::Outline, 440, 440, false).line(LineStyle::DashDot),
                l("poly", 5, [230, 60, 60, 170], FillStyle::BackHatch, 180, 240, true),
                l("contact", 6, [235, 235, 235, 220], FillStyle::Cross, 220, 250, false),
                l("metal1", 7, [80, 140, 250, 140], FillStyle::Hatch, 230, 230, true),
                l("via1", 8, [250, 210, 80, 220], FillStyle::Cross, 260, 260, false),
                l("metal2", 9, [210, 100, 230, 130], FillStyle::BackHatch, 280, 280, true),
                l("via2", 10, [120, 230, 230, 220], FillStyle::Cross, 260, 260, false),
                l("metal3", 11, [240, 170, 80, 120], FillStyle::CrossHatch, 280, 280, true),
                l("text", 63, [255, 255, 255, 255], FillStyle::Outline, 0, 0, false),
                l("boundary", 62, [160, 160, 170, 255], FillStyle::Outline, 0, 0, false).line(LineStyle::Dashed),
                // Second poly (control gates of floating-gate cells, poly-poly capacitors), last so
                // the indices of the layers above stay those of earlier projects.
                l("poly2", 12, [240, 120, 170, 150], FillStyle::Hatch, 180, 240, false),
            ],
            vias: vec![
                v("contact", "active", "contact", "metal1", 220, 250, 100, 60, 10.0, 0.1e-3),
                v("polycon", "poly", "contact", "metal1", 220, 250, 100, 60, 10.0, 0.1e-3),
                v("via1", "metal1", "via1", "metal2", 260, 260, 60, 60, 5.0, 0.2e-3),
                v("via2", "metal2", "via2", "metal3", 260, 260, 60, 60, 5.0, 0.2e-3),
                v("poly2con", "poly2", "contact", "metal1", 220, 250, 100, 60, 10.0, 0.1e-3),
            ],
            drc_deck: None,
            drc_deck_file: None,
            // Illustrative 180nm-class values (field oxide to substrate), not foundry data.
            caps: [
                ("poly", 100.0, 45.0, 7.0, 7.5, 0.05e-3),
                ("poly2", 90.0, 40.0, 7.0, 7.5, 0.05e-3),
                ("metal1", 38.0, 40.0, 17.0, 0.08, 1.0e-3),
                ("metal2", 16.0, 32.0, 20.0, 0.08, 1.0e-3),
                ("metal3", 10.0, 28.0, 20.0, 0.06, 1.5e-3),
            ]
            .map(|(layer, area, fringe, coupling, resistance, em)| LayerCap {
                layer: layer.into(),
                area,
                fringe,
                coupling,
                resistance,
                em,
            })
            .into(),
            roles: LayerRoles::default(),
            rules: DeviceRules::default(),
            devices: vec![],
            resistors: vec![],
            // Interpoly dielectric (ONO, 13 nm equivalent oxide): the control-gate to
            // floating-gate capacitance of flash cells, or a poly-poly capacitor.
            capacitors: vec![CapDef {
                name: "cpip".into(),
                bottom: "poly".into(),
                top: "poly2".into(),
                layers: vec![],
                exclude: vec![],
                area: 2.65,
                perimeter: 0.0,
            }],
            bjts: vec![],
            models: None,
            signoff: vec![],
            pcell_layers: vec![],
            drc_extra: vec![],
            pin_labels: vec![],
            stream_map: vec![],
            cross_section: Some(CrossSection::generic180()),
            variation: None,
            mismatch: None,
        }
    }
}

impl Default for Tech {
    fn default() -> Self {
        Tech::generic()
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn cross_section_gives_a_pixel_stack() {
        let t = Tech::generic().cross_section.unwrap().pixel_stack_toml("generic180", "aps4t", Some("boundary")).unwrap();
        let v: toml::Value = toml::from_str(&t).unwrap();
        assert_eq!(v["layout"]["cell"].as_str(), Some("aps4t"));
        let layers = v["layers"].as_array().unwrap();
        // Planarisation and colour filter first, then the back end from the top down.
        let names: Vec<&str> = layers.iter().map(|l| l["name"].as_str().unwrap()).collect();
        assert_eq!(names[..3], ["planarisation", "colour filter", "passivation"]);
        assert_eq!(*names.last().unwrap(), "PMD");
        assert_eq!(layers.iter().filter(|l| l.get("pattern").is_some()).count(), 3);
        assert_eq!(v["arc"].as_array().unwrap().len(), 1);
        assert_eq!(v["photodiode"]["layout"].as_str(), Some("active wide 1"));
        assert_eq!(v["microlens"]["center"].as_str(), Some("photodiode"));
        // Films above other levels are rejected.
        let mut bad = CrossSection::generic180();
        bad.levels[2].film = true;
        assert!(bad.pixel_stack_toml("x", "c", None).is_err());
    }

    use super::*;

    #[test]
    fn json_round_trip_with_caps() {
        let t = Tech::generic();
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains("\"caps\""));
        assert_eq!(serde_json::from_str::<Tech>(&json).unwrap(), t);
    }

    #[test]
    fn old_json_without_caps_loads() {
        let mut v = serde_json::to_value(Tech::generic()).unwrap();
        v.as_object_mut().unwrap().remove("caps");
        let t: Tech = serde_json::from_value(v).unwrap();
        assert!(t.caps.is_empty());
        assert_eq!(t.layers, Tech::generic().layers);
    }
}
