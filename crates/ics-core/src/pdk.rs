//! PDK packages: one TOML file describing a process for IC Studio.
//!
//! ```toml
//! name = "sky130A"
//! dbu_per_um = 1000            # optional, default 1000
//! grid = 0.005                 # µm
//! drc_extra = ["pcon = licon1 and poly"]   # lines appended to the derived DRC deck
//! lyp = "${PDK_ROOT}/.../sky130A.lyp"   # optional KLayout layer properties (layers, colours)
//! lyt = "..."                  # optional KLayout technology (dbu, grid)
//!
//! [[layer]]                    # adds a layer, or sets rules on a layer of the .lyp
//! name = "diff"
//! gds = 65
//! datatype = 20
//! width = 0.15                 # µm, optional
//! space = 0.27                 # µm, optional
//! area = 0.0                   # µm², optional minimum area
//! routing = false              # optional
//! color = "#4cc85a"            # optional, "#rrggbb" or "#rrggbbaa"
//! fill = "hatch"               # optional: solid hatch backhatch cross dots outline
//!
//! [roles]                      # which layers / vias play the device roles (see LayerRoles)
//! diffusion = ["diff", "tap"]
//! tap = "tap"
//! poly = "poly"
//! nwell = "nwell"
//! nimplant = "nsdm"
//! pimplant = "psdm"
//! diff_contact = "licon"
//! poly_contact = "licon_poly"
//! label = "text"
//!
//! [rules]                      # device rules of the PCells, µm (see DeviceRules)
//! gate_contact_space = 0.08
//! poly_endcap = 0.13
//! implant_enc = 0.125
//! nwell_enc = 0.18
//! marker_enc = 0.18            # device marker layers (MosDef::layers) around the diffusion
//! bend_corner = 0.5            # share of a bent gate's corner squares in W (no unit; default 1)
//!
//! [[pin_label]]                # layer for the net labels of a conducting layer
//! layer = "met1"
//! label = "met1.label"
//!
//! [[pcell_layer]]              # extra layer drawn by the PCells (see PcellLayer)
//! layer = "npc"
//! around = "poly_contact"      # poly_contact, diff_contact, gate, diffusion, poly or a layer
//! enc = 0.1
//! kinds = []                   # PCell kinds (nmos, pmos, poly_res, ...); all when empty
//!
//! [[via]]                      # µm; resistance in Ω per cut
//! name = "licon"
//! bottom = "diff"
//! cut = "licon1"
//! top = "li1"
//! size = 0.17
//! space = 0.17
//! enc_bottom = 0.06
//! enc_top = 0.08
//! resistance = 152
//! em = 0.0001                  # optional EM limit in A per cut
//!
//! [[device]]                   # MOS devices, first match wins (see MosDef)
//! name = "sky130_fd_pr__nfet_01v8_lvt"
//! type = "nmos"
//! layers = ["lvtn"]
//! exclude = []
//! subckt = true
//!
//! [[resistor]]                 # resistors: marked body of a conducting layer (see ResDef)
//! name = "rppoly"
//! layer = "poly"
//! layers = ["polyres"]         # marker layers
//! exclude = []
//! sheet = 7.5                  # Ω per square
//!
//! [[capacitor]]                # capacitors: marked overlap of two conducting layers (see CapDef)
//! name = "cmim"
//! bottom = "met4"
//! top = "capm"
//! layers = []
//! area = 2.0                   # fF/µm²
//! perimeter = 0.0              # fF/µm, optional
//!
//! [models]
//! path = "${PDK_ROOT}/sky130A/libs.tech/ngspice/sky130.lib.spice"
//! corners = ["tt", "ff", "ss", "sf", "fs"]
//! default = "tt"
//! length_unit = "um"           # "m" (default) or "um"
//! extra = []                   # further model files loaded at the same corner
//!
//! drc_deck = "rules.deck"      # optional IC Studio DRC deck (see `drc`)
//!
//! [[signoff]]                  # external DRC / LVS (see `signoff::Runset`)
//! name = "KLayout DRC"
//! tool = "klayout"             # klayout, calibre or netgen
//! check = "drc"                # drc or lvs
//! deck = "${PDK_ROOT}/sky130A/libs.tech/klayout/drc/sky130A_mr.drc"
//! vars = { feol = "true" }     # extra KLayout -rd variables / Calibre #DEFINEs
//!
//! [[cap]]                      # optional parasitic coefficients (see LayerCap)
//! layer = "met1"
//! area = 25.8
//! fringe = 40
//! resistance = 0.125           # optional Ω per square
//! em = 0.001                   # optional EM limit in A per µm of width
//! ```
//!
//! Relative paths are relative to the PDK file. `${VAR}` and `$VAR` expand environment
//! variables; an unset variable is an error for files read while loading (`lyp`, `lyt`,
//! `drc_deck`) and a warning for the model library, which only the simulator reads.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::signoff::Runset;
use crate::tech::{
    CapDef, DeviceRules, FillStyle, LayerCap, LayerDef, LayerRoles, LengthUnit, ModelLib, MosDef, PcellLayer, PinLabel, ResDef,
    Tech, ViaDef,
};
use crate::tech_import::{apply_lyt, from_lyp};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PdkFile {
    name: String,
    #[serde(default = "default_dbu")]
    dbu_per_um: i64,
    grid: f64,
    lyp: Option<String>,
    lyt: Option<String>,
    #[serde(default)]
    layer: Vec<LayerEntry>,
    #[serde(default)]
    roles: LayerRoles,
    rules: Option<RulesEntry>,
    #[serde(default)]
    via: Vec<ViaEntry>,
    #[serde(default)]
    device: Vec<MosDef>,
    #[serde(default)]
    resistor: Vec<ResDef>,
    #[serde(default)]
    capacitor: Vec<CapDef>,
    models: Option<ModelsEntry>,
    drc_deck: Option<String>,
    #[serde(default)]
    cap: Vec<LayerCap>,
    #[serde(default)]
    signoff: Vec<Runset>,
    #[serde(default)]
    pcell_layer: Vec<PcellLayerEntry>,
    #[serde(default)]
    drc_extra: Vec<String>,
    #[serde(default)]
    pin_label: Vec<PinLabel>,
}

fn default_dbu() -> i64 {
    1000
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LayerEntry {
    name: String,
    gds: Option<u16>,
    datatype: Option<u16>,
    width: Option<f64>,
    space: Option<f64>,
    /// µm²
    area: Option<f64>,
    routing: Option<bool>,
    color: Option<String>,
    fill: Option<FillStyle>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RulesEntry {
    gate_contact_space: Option<f64>,
    poly_endcap: Option<f64>,
    implant_enc: Option<f64>,
    nwell_enc: Option<f64>,
    marker_enc: Option<f64>,
    bend_corner: Option<f64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PcellLayerEntry {
    layer: String,
    around: String,
    enc: f64,
    #[serde(default)]
    kinds: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ViaEntry {
    name: String,
    bottom: String,
    cut: String,
    top: String,
    size: f64,
    space: f64,
    enc_bottom: f64,
    enc_top: f64,
    #[serde(default)]
    resistance: f64,
    #[serde(default)]
    em: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelsEntry {
    path: String,
    #[serde(default)]
    corners: Vec<String>,
    #[serde(default)]
    default: String,
    #[serde(default)]
    length_unit: LengthUnit,
    #[serde(default)]
    extra: Vec<String>,
}

/// A loaded PDK: the technology plus non-fatal problems found while loading it.
#[derive(Debug, Clone)]
pub struct Pdk {
    pub tech: Tech,
    pub warnings: Vec<String>,
    /// The resolved path of the PDK's `drc_deck` file, if it names one.
    pub drc_deck: Option<PathBuf>,
}

/// Load the PDK file at `path` with the process environment.
pub fn load(path: &Path) -> Result<Pdk, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let base = path.parent().unwrap_or(Path::new("."));
    parse(&text, base, &|v| std::env::var(v).ok()).map_err(|e| format!("{}: {e}", path.display()))
}

/// Parse a PDK file whose relative paths resolve against `base`, expanding variables with `env`.
pub fn parse(text: &str, base: &Path, env: &dyn Fn(&str) -> Option<String>) -> Result<Pdk, String> {
    let f: PdkFile = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut warnings = Vec::new();
    if f.dbu_per_um <= 0 {
        return Err(format!("dbu_per_um must be positive, not {}", f.dbu_per_um));
    }
    let dbu = f.dbu_per_um;
    let to_dbu = |v: f64| (v * dbu as f64).round() as i64;
    let read = |p: &str| -> Result<String, String> {
        let path = resolve(p, base, env)?;
        std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))
    };

    let mut tech = match &f.lyp {
        Some(p) => from_lyp(&read(p)?, &f.name)?,
        None => Tech { name: f.name.clone(), layers: vec![], vias: vec![], caps: vec![], ..Tech::generic() },
    };
    if let Some(p) = &f.lyt {
        apply_lyt(&mut tech, &read(p)?)?;
    }
    tech.name = f.name.clone();
    tech.dbu_per_um = dbu;
    tech.grid = to_dbu(f.grid);
    if tech.grid <= 0 {
        return Err(format!("grid {} µm is below one database unit", f.grid));
    }

    for (k, e) in f.layer.iter().enumerate() {
        let idx = match tech.layer_index(&e.name) {
            Some(i) => i,
            None => {
                let Some(gds) = e.gds else {
                    return Err(format!("layer '{}' needs a gds number", e.name));
                };
                let mut l = LayerDef::generated(gds, e.datatype.unwrap_or(0), tech.layers.len() + k);
                l.name = e.name.clone();
                tech.layers.push(l);
                tech.layers.len() - 1
            }
        };
        let l = &mut tech.layers[idx];
        if let Some(g) = e.gds {
            l.gds_layer = g;
        }
        if let Some(d) = e.datatype {
            l.gds_datatype = d;
        }
        if let Some(w) = e.width {
            l.min_width = to_dbu(w);
        }
        if let Some(s) = e.space {
            l.min_spacing = to_dbu(s);
        }
        if let Some(a) = e.area {
            l.min_area = (a * (dbu * dbu) as f64).round() as i64;
        }
        if let Some(r) = e.routing {
            l.routing = r;
        }
        if let Some(c) = &e.color {
            l.color = parse_color(c).ok_or_else(|| format!("layer '{}': bad colour '{c}'", e.name))?;
        }
        if let Some(fill) = e.fill {
            l.fill = fill;
        }
    }
    if tech.layers.is_empty() {
        return Err("the PDK defines no layers".into());
    }
    for (i, l) in tech.layers.iter().enumerate() {
        if let Some(j) = tech.layers[..i].iter().position(|o| (o.gds_layer, o.gds_datatype) == (l.gds_layer, l.gds_datatype)) {
            warnings
                .push(format!("layers '{}' and '{}' share GDS {}/{}", tech.layers[j].name, l.name, l.gds_layer, l.gds_datatype));
        }
    }

    tech.vias = f
        .via
        .iter()
        .map(|v| ViaDef {
            name: v.name.clone(),
            bottom: v.bottom.clone(),
            cut: v.cut.clone(),
            top: v.top.clone(),
            cut_size: to_dbu(v.size),
            cut_spacing: to_dbu(v.space),
            enc_bottom: to_dbu(v.enc_bottom),
            enc_top: to_dbu(v.enc_top),
            resistance: v.resistance,
            em: v.em,
        })
        .collect();
    tech.roles = f.roles;
    let d = DeviceRules::default();
    tech.rules = match &f.rules {
        Some(r) => {
            let or = |v: Option<f64>, def: i64| v.map_or(def, to_dbu);
            DeviceRules {
                gate_contact_space: or(r.gate_contact_space, d.gate_contact_space),
                poly_endcap: or(r.poly_endcap, d.poly_endcap),
                implant_enc: or(r.implant_enc, d.implant_enc),
                nwell_enc: or(r.nwell_enc, d.nwell_enc),
                marker_enc: or(r.marker_enc, d.marker_enc),
                bend_corner: r.bend_corner.unwrap_or(d.bend_corner).clamp(0.0, 1.0),
            }
        }
        None => d,
    };
    tech.devices = f.device;
    tech.resistors = f.resistor;
    tech.capacitors = f.capacitor;
    tech.pcell_layers = f
        .pcell_layer
        .into_iter()
        .map(|p| PcellLayer { layer: p.layer, around: p.around, enc: to_dbu(p.enc), kinds: p.kinds })
        .collect();
    tech.drc_extra = f.drc_extra;
    tech.pin_labels = f.pin_label;
    tech.caps = f.cap;
    let deck_path = f.drc_deck.as_deref().map(|p| resolve(p, base, env)).transpose()?;
    tech.drc_deck = match &deck_path {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let (text, failed) = crate::drc::deck_file_text(&text, path)?;
            warnings.extend(failed);
            Some(text)
        }
        None => None,
    };
    tech.models = f.models.map(|m| {
        let mut model_path = |raw: &str| match resolve(raw, base, env) {
            Ok(p) => {
                if !p.exists() {
                    warnings.push(format!("model library {} does not exist", p.display()));
                }
                p.to_string_lossy().into_owned()
            }
            Err(e) => {
                warnings.push(format!("model library: {e}"));
                raw.to_string()
            }
        };
        let path = model_path(&m.path);
        let extra: Vec<String> = m.extra.iter().map(|p| model_path(p)).collect();
        if !m.default.is_empty() && !m.corners.contains(&m.default) {
            warnings.push(format!("default corner '{}' is not one of {:?}", m.default, m.corners));
        }
        ModelLib { path, corners: m.corners, default_corner: m.default, length_unit: m.length_unit, extra }
    });
    tech.signoff = f
        .signoff
        .into_iter()
        .map(|mut r| {
            match resolve(&r.deck, base, env) {
                Ok(p) => {
                    if !p.exists() {
                        warnings.push(format!("sign-off runset '{}': deck {} does not exist", r.name, p.display()));
                    }
                    r.deck = p.to_string_lossy().into_owned();
                }
                Err(e) => warnings.push(format!("sign-off runset '{}': {e}", r.name)),
            }
            r
        })
        .collect();
    warnings.extend(check(&tech));
    Ok(Pdk { tech, warnings, drc_deck: deck_path })
}

/// Consistency problems of a technology's vias, roles and devices.
pub fn check(tech: &Tech) -> Vec<String> {
    let mut w = Vec::new();
    let has = |n: &str| tech.layer_index(n).is_some();
    for v in &tech.vias {
        for n in [&v.bottom, &v.cut, &v.top] {
            if !has(n) {
                w.push(format!("via '{}' uses unknown layer '{n}'", v.name));
            }
        }
    }
    let r = &tech.roles;
    for (role, n) in r
        .diffusion
        .iter()
        .map(|n| ("diffusion", n))
        .chain([("poly", &r.poly), ("nwell", &r.nwell), ("pimplant", &r.pimplant), ("label", &r.label)])
        .chain((!r.nimplant.is_empty()).then_some(("nimplant", &r.nimplant)))
        .chain((!r.tap.is_empty()).then_some(("tap", &r.tap)))
    {
        if !has(n) {
            w.push(format!("role {role}: unknown layer '{n}'"));
        }
    }
    for (role, n) in [("diff_contact", &r.diff_contact), ("poly_contact", &r.poly_contact)] {
        if tech.via(n).is_none() {
            w.push(format!("role {role}: unknown via '{n}'"));
        }
    }
    for d in &tech.devices {
        for n in d.layers.iter().chain(&d.exclude) {
            if !has(n) {
                w.push(format!("device '{}' uses unknown layer '{n}'", d.name));
            }
        }
    }
    for r in &tech.resistors {
        for n in std::iter::once(&r.layer).chain(&r.layers).chain(&r.exclude) {
            if !has(n) {
                w.push(format!("resistor '{}' uses unknown layer '{n}'", r.name));
            }
        }
    }
    for c in &tech.capacitors {
        for n in [&c.bottom, &c.top].into_iter().chain(&c.layers).chain(&c.exclude) {
            if !has(n) {
                w.push(format!("capacitor '{}' uses unknown layer '{n}'", c.name));
            }
        }
    }
    const AROUND: [&str; 5] = ["poly_contact", "diff_contact", "gate", "diffusion", "poly"];
    for p in &tech.pcell_layers {
        if !has(&p.layer) {
            w.push(format!("pcell layer uses unknown layer '{}'", p.layer));
        }
        if !AROUND.contains(&p.around.as_str()) && !has(&p.around) {
            w.push(format!("pcell layer '{}': unknown 'around' '{}'", p.layer, p.around));
        }
    }
    for p in &tech.pin_labels {
        for n in [&p.layer, &p.label] {
            if !has(n) {
                w.push(format!("pin label uses unknown layer '{n}'"));
            }
        }
    }
    for c in &tech.caps {
        if !has(&c.layer) {
            w.push(format!("capacitance for unknown layer '{}'", c.layer));
        }
    }
    w
}

/// `p` with variables expanded, made absolute against `base`.
fn resolve(p: &str, base: &Path, env: &dyn Fn(&str) -> Option<String>) -> Result<PathBuf, String> {
    let p = expand(p, env)?;
    let path = PathBuf::from(&p);
    Ok(if path.is_absolute() { path } else { base.join(path) })
}

/// Expand `${VAR}` and `$VAR` (letters, digits, `_`).
fn expand(s: &str, env: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let (name, tail) = match after.strip_prefix('{') {
            Some(b) => {
                let j = b.find('}').ok_or_else(|| format!("unclosed '${{' in '{s}'"))?;
                (&b[..j], &b[j + 1..])
            }
            None => {
                let j = after.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(after.len());
                (&after[..j], &after[j..])
            }
        };
        if name.is_empty() {
            out.push('$');
        } else {
            out.push_str(&env(name).ok_or_else(|| format!("environment variable {name} is not set ('{s}')"))?);
        }
        rest = tail;
    }
    out.push_str(rest);
    Ok(out)
}

fn parse_color(s: &str) -> Option<[u8; 4]> {
    let h = s.strip_prefix('#')?;
    let b = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    match h.len() {
        6 => Some([b(0)?, b(2)?, b(4)?, crate::tech_import::ALPHA]),
        8 => Some([b(0)?, b(2)?, b(4)?, b(6)?]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drc::run_tech_deck;
    use crate::extract::extract;
    use crate::lvs::{LvsOptions, compare};
    use crate::netlist::netlist_for;
    use crate::project::{CellRef, Project};

    const SKY130: &str = include_str!("../../../pdks/sky130A.toml");

    fn sky130(env: Option<&str>) -> Pdk {
        let root = env.map(String::from);
        parse(SKY130, Path::new("."), &move |v| if v == "PDK_ROOT" { root.clone() } else { None }).unwrap()
    }

    #[test]
    fn sky130_loads_consistently() {
        let p = sky130(None);
        assert!(p.warnings.iter().all(|w| w.contains("PDK_ROOT is not set")), "only unresolved PDK paths: {:?}", p.warnings);
        assert_eq!(p.tech.signoff.len(), 2);
        let t = &p.tech;
        assert_eq!((t.name.as_str(), t.dbu_per_um, t.grid), ("sky130A", 1000, 5));
        let diff = t.layer_index("diff").unwrap();
        assert_eq!((t.layers[diff].gds_layer, t.layers[diff].gds_datatype, t.layers[diff].min_width), (65, 20, 150));
        assert_eq!(t.diffusion_layers().len(), 2);
        assert_eq!(t.metal1().and_then(|l| t.layer(l)).map(|l| l.name.as_str()), Some("li1"));
        assert_eq!(t.metal2().and_then(|l| t.layer(l)).map(|l| l.name.as_str()), Some("met1"));
        assert_eq!(t.mos_device(false, "nmos").map(|d| d.name.as_str()), Some("sky130_fd_pr__nfet_01v8"));
        assert_eq!(t.mos_device(true, "").map(|d| d.name.as_str()), Some("sky130_fd_pr__pfet_01v8"));
        let m = t.models.as_ref().unwrap();
        assert_eq!(m.spice_line("ss"), format!(".lib \"{}\" ss", m.path));
        let with_root = sky130(Some("/pdk"));
        assert!(with_root.tech.models.unwrap().path.replace('\\', "/").starts_with("/pdk/sky130A/libs.tech/ngspice/"));
    }

    #[test]
    fn errors_and_expansion() {
        let env = |v: &str| (v == "A").then(|| "x".to_string());
        assert_eq!(expand("${A}/$A.lib/$$", &env).unwrap(), "x/x.lib/$$");
        assert!(expand("${B}", &env).is_err());
        let bad = parse("name = \"t\"\ngrid = 0.005\n[[layer]]\nname = \"m1\"\n", Path::new("."), &env);
        assert!(bad.unwrap_err().contains("needs a gds number"));
        let unknown = parse("name = \"t\"\ngrid = 0.005\nfoo = 1\n", Path::new("."), &env);
        assert!(unknown.is_err());
        assert_eq!(parse_color("#102030"), Some([16, 32, 48, crate::tech_import::ALPHA]));
        assert_eq!(parse_color("#10203040"), Some([16, 32, 48, 64]));
        assert_eq!(parse_color("102030"), None);
    }

    #[test]
    fn resistor_and_capacitor_tables() {
        let text = r#"
name = "t"
grid = 0.005
[[layer]]
name = "poly"
gds = 1
[[layer]]
name = "rmark"
gds = 2
[[resistor]]
name = "rp"
layer = "poly"
layers = ["rmark"]
sheet = 7.5
[[capacitor]]
name = "cm"
bottom = "poly"
top = "m9"
area = 2.0
"#;
        let p = parse(text, Path::new("."), &|_| None).unwrap();
        assert_eq!((p.tech.resistors[0].name.as_str(), p.tech.resistors[0].sheet), ("rp", 7.5));
        assert_eq!((p.tech.capacitors[0].area, p.tech.capacitors[0].perimeter), (2.0, 0.0));
        assert!(p.warnings.iter().any(|w| w == "capacitor 'cm' uses unknown layer 'm9'"), "{:?}", p.warnings);
    }

    /// The demo inverter in SKY130, placed and routed from its schematic.
    fn sky130_inverter() -> (Project, crate::Schematic, crate::Layout) {
        sky130_inverter_with(None)
    }

    /// The demo inverter in SKY130 with the NMOS model set to `nmos_model`.
    fn sky130_inverter_with(nmos_model: Option<&str>) -> (Project, crate::Schematic, crate::Layout) {
        inverter_in(sky130(Some("/pdk")).tech, nmos_model)
    }

    /// The demo inverter in `tech`, placed and routed from its schematic, with the NMOS model
    /// set to `nmos_model`.
    fn inverter_in(tech: Tech, nmos_model: Option<&str>) -> (Project, crate::Schematic, crate::Layout) {
        let mut p = crate::examples::demo_project();
        p.set_tech(tech).unwrap();
        let mut sch = p.cell(&CellRef::new("work", "cmos_inverter")).and_then(|c| c.schematic.clone()).unwrap();
        if let Some(m) = nmos_model {
            let n = sch.instances.iter_mut().find(|i| i.kind == crate::DeviceKind::Nmos).unwrap();
            n.params.insert("model".into(), m.into());
        }
        let plan = crate::sdl::plan(&sch, &p.tech);
        let mut lay = crate::Layout::default();
        crate::sdl::apply(&mut p, &mut lay, &plan).unwrap();
        let routed = crate::route::route(&sch, &mut lay, &p, &p.tech).unwrap();
        assert!(routed.failed.is_empty(), "{routed:?}");
        (p, sch, lay)
    }

    /// Schematic-driven layout of the demo inverter in SKY130: DRC clean, LVS match with the
    /// PDK device names, netlist with subcircuit models in microns.
    #[test]
    fn sky130_inverter_end_to_end() {
        let (p, sch, lay) = sky130_inverter();
        let drc = run_tech_deck(&lay, &p, &p.tech).unwrap();
        assert!(drc.violations.is_empty(), "{:?}", drc.violations.iter().map(|v| &v.rule).collect::<Vec<_>>());
        let x = extract(&lay, &p, &p.tech);
        let models: Vec<&str> = x.devices.iter().map(|d| d.model.as_str()).collect();
        assert_eq!(models, ["sky130_fd_pr__nfet_01v8", "sky130_fd_pr__pfet_01v8"]);
        assert!(x.warnings.is_empty(), "{:?}", x.warnings);
        assert_net_labels(&p.tech, &lay, &x, "li1.label");
        let rep = compare(&sch, &x, &lvs_opts(&p.tech));
        assert!(rep.matched, "{:?}", rep.errors);
        let nl = netlist_for(&sch, &p.tech, "");
        assert!(nl.warnings.is_empty(), "{:?}", nl.warnings);
        let nmos = nl.body.lines().find(|l| l.contains(" sky130_fd_pr__nfet_01v8 ")).unwrap_or_default();
        assert!(nmos.starts_with("XM") && nmos.ends_with(" w=1 l=0.18"), "{}", nl.body);
        assert!(nl.body.contains("sky130.lib.spice\" tt"), "{}", nl.body);
        let ss = netlist_for(&sch, &p.tech, "ss");
        assert!(ss.body.contains("sky130.lib.spice\" ss"), "{}", ss.body);
    }

    #[test]
    fn lvs_reports_wrong_pdk_model() {
        let (p, mut sch, lay) = sky130_inverter();
        let x = extract(&lay, &p, &p.tech);
        let n = sch.instances.iter_mut().find(|i| i.kind == crate::DeviceKind::Nmos).unwrap();
        n.params.insert("model".into(), "sky130_fd_pr__nfet_01v8_lvt".into());
        let rep = compare(&sch, &x, &lvs_opts(&p.tech));
        assert!(!rep.matched);
        assert!(
            rep.errors.iter().any(|e| e.message.contains("model sky130_fd_pr__nfet_01v8_lvt (schematic)")),
            "{:?}",
            rep.errors
        );
    }

    /// The demo inverter's load capacitor has no layout.
    fn lvs_opts(tech: &Tech) -> LvsOptions {
        let mut o = LvsOptions::for_tech(tech);
        o.ignore_kinds.push(crate::DeviceKind::Capacitor);
        o
    }

    /// A low-Vt NMOS in the schematic becomes a PCell with the lvtn marker, which extraction
    /// and LVS recognise as that device.
    #[test]
    fn sky130_low_vt_device_end_to_end() {
        let (p, sch, lay) = sky130_inverter_with(Some("sky130_fd_pr__nfet_01v8_lvt"));
        let drc = run_tech_deck(&lay, &p, &p.tech).unwrap();
        assert!(drc.violations.is_empty(), "{:?}", drc.violations.iter().map(|v| &v.rule).collect::<Vec<_>>());
        let x = extract(&lay, &p, &p.tech);
        let models: Vec<&str> = x.devices.iter().map(|d| d.model.as_str()).collect();
        assert_eq!(models, ["sky130_fd_pr__nfet_01v8_lvt", "sky130_fd_pr__pfet_01v8"], "{:?}", x.warnings);
        assert!(compare(&sch, &x, &lvs_opts(&p.tech)).matched);
        let pcells: Vec<&str> = p.libraries.iter().flat_map(|l| &l.cells).map(|c| c.name.as_str()).collect();
        assert!(pcells.iter().any(|n| n.ends_with("_sky130_fd_pr__nfet_01v8_lvt")), "{pcells:?}");
    }

    /// PCells draw npc over poly contacts; without it the technology's NPC check fires.
    #[test]
    fn sky130_npc_layer_and_check() {
        let mut tech = sky130(Some("/pdk")).tech;
        let npc = tech.layer_index("npc").unwrap();
        let nmos = crate::pcell::PCell::mos(false, 1000, 150, 2);
        let lay = nmos.generate(&tech).unwrap();
        let npcs: Vec<_> = lay.shapes.iter().filter(|s| s.layer == npc).collect();
        assert_eq!(npcs.len(), 1, "one merged npc over the gate contact head");
        let v0 = run_tech_deck(&lay, &crate::hier::NoCells, &tech).unwrap().violations;
        assert!(v0.is_empty(), "{:?}", v0.iter().map(|v| (&v.rule, v.marker)).collect::<Vec<_>>());
        tech.pcell_layers.clear();
        let bare = nmos.generate(&tech).unwrap();
        let v = run_tech_deck(&bare, &crate::hier::NoCells, &tech).unwrap().violations;
        assert!(!v.is_empty() && v.iter().all(|v| v.rule == "NPC.EN"), "{:?}", v.iter().map(|v| &v.rule).collect::<Vec<_>>());
        let lvt =
            crate::pcell::PCell::Mos { pmos: false, w: 1000, l: 150, fingers: 1, device: "sky130_fd_pr__nfet_01v8_lvt".into() };
        let lvtn = tech.layer_index("lvtn").unwrap();
        assert_eq!(lvt.generate(&tech).unwrap().shapes.iter().filter(|s| s.layer == lvtn).count(), 1);
        let wrong =
            crate::pcell::PCell::Mos { pmos: true, w: 1000, l: 150, fingers: 1, device: "sky130_fd_pr__nfet_01v8_lvt".into() };
        assert!(wrong.generate(&tech).unwrap_err().contains("not a PMOS"));
        let unknown = crate::pcell::PCell::Mos { pmos: false, w: 1000, l: 150, fingers: 1, device: "x".into() };
        assert!(unknown.generate(&tech).unwrap_err().contains("no device 'x'"));
    }

    /// Every SKY130 MOS PCell variant (sizes, fingers, devices) and the other PCells are DRC
    /// clean with the PDK's derived deck.
    #[test]
    fn sky130_pcells_drc_clean() {
        let tech = sky130(Some("/pdk")).tech;
        let mut cells = vec![
            crate::pcell::PCell::PolyRes { w: 400, l: 5000 },
            crate::pcell::PCell::MomCap { w: 5000, h: 5000 },
            crate::pcell::PCell::Tap { well: true, w: 1000 },
            crate::pcell::PCell::Tap { well: false, w: 1000 },
        ];
        for d in &tech.devices {
            let pmos = d.kind == crate::tech::MosType::Pmos;
            for (w, l) in [(420, 150), (1000, 150), (2000, 180), (1000, 500)] {
                for fingers in 1..=3 {
                    cells.push(crate::pcell::PCell::Mos { pmos, w, l, fingers, device: d.name.clone() });
                }
            }
        }
        for c in cells {
            let lay = c.generate(&tech).unwrap_or_else(|e| panic!("{}: {e}", c.variant_name()));
            let v = run_tech_deck(&lay, &crate::hier::NoCells, &tech).unwrap().violations;
            assert!(v.is_empty(), "{}: {:?}", c.variant_name(), v.iter().map(|v| (&v.rule, v.marker)).collect::<Vec<_>>());
            let x = extract(&lay, &crate::hier::NoCells, &tech);
            if let crate::pcell::PCell::Mos { device, fingers, .. } = &c {
                assert_eq!(x.devices.len(), *fingers as usize, "{}", c.variant_name());
                assert!(x.devices.iter().all(|d| &d.model == device), "{}: {:?}", c.variant_name(), x.warnings);
            }
        }
    }

    const SG13G2: &str = include_str!("../../../pdks/ihp-sg13g2.toml");

    fn sg13g2() -> Pdk {
        parse(SG13G2, Path::new("."), &|v| (v == "PDK_ROOT").then(|| "/pdk".to_string())).unwrap()
    }

    #[test]
    fn sg13g2_loads_consistently() {
        let p = sg13g2();
        assert!(p.warnings.iter().all(|w| w.contains("does not exist")), "only missing PDK files: {:?}", p.warnings);
        let t = &p.tech;
        assert_eq!((t.name.as_str(), t.grid, t.devices.len(), t.signoff.len()), ("ihp-sg13g2", 5, 4, 3));
        assert_eq!(t.implant(false), None);
        assert_eq!(t.metal1().and_then(|l| t.layer(l)).map(|l| l.name.as_str()), Some("Metal1"));
        assert_eq!(t.layer(t.layer_index("Metal2").unwrap()).unwrap().min_area, 144_000);
        assert_eq!(t.mos_device(true, "pmos").map(|d| d.name.as_str()), Some("sg13_lv_pmos"));
        let lines = t.models.as_ref().unwrap().spice_line("mos_ss");
        let lines: Vec<String> = lines.replace('\\', "/").lines().map(String::from).collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with("cornerMOSlv.lib\" mos_ss") && lines[1].ends_with("cornerMOShv.lib\" mos_ss"), "{lines:?}");
    }

    /// The demo inverter in SG13G2: placed, routed, DRC clean, extracted as the 1.2 V devices,
    /// LVS clean, netlisted as subcircuits in metres; a 3.3 V NMOS gets ThickGateOx.
    #[test]
    fn sg13g2_inverter_end_to_end() {
        for (model, expect) in [(None, "sg13_lv_nmos"), (Some("sg13_hv_nmos"), "sg13_hv_nmos")] {
            let (p, sch, lay) = inverter_in(sg13g2().tech, model);
            let drc = run_tech_deck(&lay, &p, &p.tech).unwrap();
            assert!(
                drc.violations.is_empty(),
                "{expect}: {:?}",
                drc.violations.iter().map(|v| (&v.rule, v.marker)).collect::<Vec<_>>()
            );
            let x = extract(&lay, &p, &p.tech);
            assert!(x.warnings.is_empty(), "{:?}", x.warnings);
            let models: Vec<&str> = x.devices.iter().map(|d| d.model.as_str()).collect();
            assert_eq!(models, [expect, "sg13_lv_pmos"]);
            assert_net_labels(&p.tech, &lay, &x, "Metal1.text");
            let rep = compare(&sch, &x, &lvs_opts(&p.tech));
            assert!(rep.matched, "{:?}", rep.errors);
            let nl = netlist_for(&sch, &p.tech, "");
            assert!(nl.warnings.is_empty(), "{:?}", nl.warnings);
            let nmos = nl.body.lines().find(|l| l.contains(&format!(" {expect} "))).unwrap_or_default();
            assert!(nmos.starts_with("XM") && nmos.ends_with(" w=1u l=180n"), "{}", nl.body);
            assert_eq!(nl.body.matches(".lib ").count(), 2, "{}", nl.body);
        }
    }

    /// Every SG13G2 PCell variant is DRC clean and extracts as its device.
    #[test]
    fn sg13g2_pcells_drc_clean() {
        let tech = sg13g2().tech;
        let mut cells = vec![
            crate::pcell::PCell::PolyRes { w: 400, l: 5000 },
            crate::pcell::PCell::MomCap { w: 5000, h: 5000 },
            crate::pcell::PCell::Tap { well: true, w: 1000 },
            crate::pcell::PCell::Tap { well: false, w: 1000 },
        ];
        for d in &tech.devices {
            let pmos = d.kind == crate::tech::MosType::Pmos;
            let l_min = if d.name.contains("hv") { if pmos { 400 } else { 450 } } else { 130 };
            for (w, l) in [(400, l_min), (1000, l_min), (2000, 500)] {
                for fingers in 1..=3 {
                    cells.push(crate::pcell::PCell::Mos { pmos, w, l, fingers, device: d.name.clone() });
                }
            }
        }
        for c in cells {
            let lay = c.generate(&tech).unwrap_or_else(|e| panic!("{}: {e}", c.variant_name()));
            let v = run_tech_deck(&lay, &crate::hier::NoCells, &tech).unwrap().violations;
            assert!(v.is_empty(), "{}: {:?}", c.variant_name(), v.iter().map(|v| (&v.rule, v.marker)).collect::<Vec<_>>());
            let x = extract(&lay, &crate::hier::NoCells, &tech);
            if let crate::pcell::PCell::Mos { device, fingers, .. } = &c {
                assert_eq!(x.devices.len(), *fingers as usize, "{}", c.variant_name());
                assert!(x.devices.iter().all(|d| &d.model == device), "{}: {:?}", c.variant_name(), x.warnings);
            }
        }
    }

    /// Routing labels the named nets on `label_layer`, and extraction names the nets after them.
    fn assert_net_labels(tech: &Tech, lay: &crate::Layout, x: &crate::extract::Extraction, label_layer: &str) {
        let ll = tech.layer_index(label_layer).unwrap();
        let mut texts: Vec<&str> = lay
            .shapes
            .iter()
            .filter(|s| s.layer == ll)
            .filter_map(|s| if let crate::Geometry::Label { text, .. } = &s.geom { Some(text.as_str()) } else { None })
            .collect();
        texts.sort_unstable();
        assert_eq!(texts, ["gnd", "in", "out", "vdd"]);
        let mut named: Vec<&str> = x.nets.iter().filter(|n| n.labelled).map(|n| n.name.as_str()).collect();
        named.sort_unstable();
        assert_eq!(named, ["gnd", "in", "out", "vdd"], "{:?}", x.warnings);
    }
}
