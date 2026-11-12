//! Technology / PDK description: layers, GDS mapping, display style and basic rules.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FillStyle {
    #[default]
    Solid,
    Hatch,
    BackHatch,
    Cross,
    Dots,
    Outline,
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
    /// Minimum width in database units (0 = unchecked).
    #[serde(default)]
    pub min_width: i64,
    /// Minimum same-layer spacing in database units (0 = unchecked).
    #[serde(default)]
    pub min_spacing: i64,
    /// Whether the layer is a routing layer that carries net labels.
    #[serde(default)]
    pub routing: bool,
}

impl LayerDef {
    /// Placeholder for a GDS layer without a definition: named `L{layer}D{datatype}`,
    /// hatched, with the `k`-th colour of [`auto_color`].
    pub fn generated(gds_layer: u16, gds_datatype: u16, k: usize) -> LayerDef {
        LayerDef {
            name: format!("L{gds_layer}D{gds_datatype}"),
            gds_layer,
            gds_datatype,
            color: auto_color(k),
            fill: FillStyle::Hatch,
            min_width: 0,
            min_spacing: 0,
            routing: false,
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
    /// Parasitic capacitance coefficients per conducting layer (layers without one are ignored).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub caps: Vec<LayerCap>,
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
            min_width: w,
            min_spacing: s,
            routing,
        };
        let v = |name: &str, bottom: &str, cut: &str, top: &str, size: i64, space: i64, eb: i64, et: i64| ViaDef {
            name: name.into(),
            bottom: bottom.into(),
            cut: cut.into(),
            top: top.into(),
            cut_size: size,
            cut_spacing: space,
            enc_bottom: eb,
            enc_top: et,
        };
        Tech {
            name: "generic180".into(),
            dbu_per_um: 1000,
            grid: 10,
            layers: vec![
                l("nwell", 1, [120, 200, 120, 70], FillStyle::Dots, 860, 1400, false),
                l("active", 2, [70, 190, 90, 150], FillStyle::Hatch, 220, 280, false),
                l("nplus", 3, [230, 120, 60, 60], FillStyle::Outline, 440, 440, false),
                l("pplus", 4, [200, 160, 60, 60], FillStyle::Outline, 440, 440, false),
                l("poly", 5, [230, 60, 60, 170], FillStyle::BackHatch, 180, 240, true),
                l("contact", 6, [235, 235, 235, 220], FillStyle::Cross, 220, 250, false),
                l("metal1", 7, [80, 140, 250, 140], FillStyle::Hatch, 230, 230, true),
                l("via1", 8, [250, 210, 80, 220], FillStyle::Cross, 260, 260, false),
                l("metal2", 9, [210, 100, 230, 130], FillStyle::BackHatch, 280, 280, true),
                l("via2", 10, [120, 230, 230, 220], FillStyle::Cross, 260, 260, false),
                l("metal3", 11, [240, 170, 80, 120], FillStyle::Hatch, 280, 280, true),
                l("text", 63, [255, 255, 255, 255], FillStyle::Outline, 0, 0, false),
                l("boundary", 62, [160, 160, 170, 255], FillStyle::Outline, 0, 0, false),
            ],
            vias: vec![
                v("contact", "active", "contact", "metal1", 220, 250, 100, 60),
                v("polycon", "poly", "contact", "metal1", 220, 250, 100, 60),
                v("via1", "metal1", "via1", "metal2", 260, 260, 60, 60),
                v("via2", "metal2", "via2", "metal3", 260, 260, 60, 60),
            ],
            drc_deck: None,
            // Illustrative 180nm-class values (field oxide to substrate), not foundry data.
            caps: [
                ("poly", 100.0, 45.0, 7.0),
                ("metal1", 38.0, 40.0, 17.0),
                ("metal2", 16.0, 32.0, 20.0),
                ("metal3", 10.0, 28.0, 20.0),
            ]
            .map(|(layer, area, fringe, coupling)| LayerCap { layer: layer.into(), area, fringe, coupling })
            .into(),
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
