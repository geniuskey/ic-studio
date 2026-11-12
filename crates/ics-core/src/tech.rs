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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tech {
    pub name: String,
    /// Database units per micron (1000 => 1 dbu = 1 nm).
    pub dbu_per_um: i64,
    /// Layout manufacturing / snap grid in dbu.
    pub grid: i64,
    pub layers: Vec<LayerDef>,
}

impl Tech {
    pub fn layer(&self, idx: usize) -> Option<&LayerDef> {
        self.layers.get(idx)
    }

    pub fn layer_index(&self, name: &str) -> Option<usize> {
        self.layers.iter().position(|l| l.name.eq_ignore_ascii_case(name))
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
        }
    }
}

impl Default for Tech {
    fn default() -> Self {
        Tech::generic()
    }
}
