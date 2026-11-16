//! Cadence Virtuoso technology import: an ASCII technology file (`.tf`), optionally with its
//! display resource file (`display.drf`) and a stream layer map.
//!
//! - Layers: every `techLayerPurposePriorities` entry (else every `techDisplays` entry) whose
//!   layer is in `techLayers`. A `drawing` purpose keeps the layer name, other purposes become
//!   `layer.purpose`.
//! - Units: `viewTypeUnits` (`maskLayout`) gives the database units per micron, and
//!   `mfgGridResolution` the grid.
//! - Rules: single-layer `minWidth`, `minSpacing` and `minArea` from `spacings` in
//!   `constraintGroups` (the `foundry` group) or from `physicalRules` / `spacingRules`.
//! - Display: the layer-purpose's `techDisplays` packet, looked up in the display resource file
//!   (`drDefinePacket`), gives the fill colour (`drDefineColor`) and, from its stipple bitmap
//!   (`drDefineStipple`), the closest fill style.
//! - Stream numbers: from the layer map when one is given (it also becomes the technology's
//!   stream map), otherwise the layer number with datatype 0 for `drawing` and the purpose
//!   number for other purposes.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::tech::{FillStyle, LayerDef, Tech, auto_color};

/// A SKILL list or atom.
#[derive(Debug, Clone, PartialEq)]
enum Sx {
    Atom(String),
    List(Vec<Sx>),
}

impl Sx {
    fn atom(&self) -> Option<&str> {
        match self {
            Sx::Atom(a) => Some(a.as_str()),
            Sx::List(_) => None,
        }
    }

    fn items(&self) -> &[Sx] {
        match self {
            Sx::List(v) => v,
            Sx::Atom(_) => &[],
        }
    }

    /// The head atom of a list (`techLayers(` ... reads as `(techLayers ...`).
    fn head(&self) -> Option<&str> {
        self.items().first().and_then(Sx::atom)
    }

    /// Every list headed `name`, at any depth.
    fn find_all<'a>(&'a self, name: &str, out: &mut Vec<&'a Sx>) {
        if let Sx::List(v) = self {
            if self.head().is_some_and(|h| h == name) {
                out.push(self);
            }
            for c in v {
                c.find_all(name, out);
            }
        }
    }

    fn all(&self, name: &str) -> Vec<&Sx> {
        let mut out = vec![];
        self.find_all(name, &mut out);
        out
    }

    /// Atom texts of the list, without the quotes of strings.
    fn strings(&self) -> Vec<String> {
        self.items().iter().filter_map(Sx::atom).map(|s| s.trim_matches('"').to_string()).collect()
    }
}

/// Parse SKILL data: lists in `( ... )` or `name( ... )` form, atoms, strings; `;` comments and
/// `/* */` blocks.
fn parse(text: &str) -> Result<Sx, String> {
    let b = text.as_bytes();
    let mut stack: Vec<Vec<Sx>> = vec![vec![]];
    let mut i = 0;
    let mut line = 1;
    while i < b.len() {
        let c = b[i];
        match c {
            b'\n' => {
                line += 1;
                i += 1;
            }
            b';' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    line += (b[i] == b'\n') as usize;
                    i += 1;
                }
                i += 2;
            }
            b'(' => {
                stack.push(vec![]);
                i += 1;
            }
            b')' => {
                let done = stack.pop().ok_or_else(|| format!("line {line}: unbalanced ')'"))?;
                stack.last_mut().ok_or_else(|| format!("line {line}: unbalanced ')'"))?.push(Sx::List(done));
                i += 1;
            }
            b'"' => {
                let start = i;
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i += 1;
                stack.last_mut().unwrap().push(Sx::Atom(text[start..i.min(b.len())].to_string()));
            }
            c if c.is_ascii_whitespace() => i += 1,
            b'\'' => i += 1, // quoted symbol
            _ => {
                let start = i;
                while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'(' | b')' | b'"' | b';') {
                    i += 1;
                }
                let word = text[start..i].to_string();
                if b.get(i) == Some(&b'(') {
                    // `name(` opens a list headed by the name.
                    stack.push(vec![Sx::Atom(word)]);
                    i += 1;
                } else {
                    stack.last_mut().unwrap().push(Sx::Atom(word));
                }
            }
        }
    }
    if stack.len() != 1 {
        return Err("unbalanced '(' at the end of the file".into());
    }
    Ok(Sx::List(stack.pop().unwrap()))
}

/// Fill style closest to a stipple bitmap (rows of 0 / 1).
fn stipple_style(rows: &[Vec<u8>]) -> FillStyle {
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
    if back > 0.6 && fwd < 0.3 {
        FillStyle::BackHatch
    } else if fwd > 0.6 && back < 0.3 {
        FillStyle::Hatch
    } else if back + fwd > 0.9 || (right > 0.6 && down > 0.6) {
        FillStyle::Cross
    } else if right > 0.6 || down > 0.6 {
        FillStyle::Hatch
    } else {
        FillStyle::Dots
    }
}

/// Display packets of a display resource file: packet name -> (colour, fill).
fn read_drf(drf: &Sx, warnings: &mut Vec<String>) -> HashMap<String, ([u8; 3], FillStyle)> {
    let mut colors: HashMap<String, [u8; 3]> = HashMap::new();
    for def in drf.all("drDefineColor") {
        for e in &def.items()[1..] {
            let s = e.strings();
            if let [_, name, r, g, b, ..] = s.as_slice()
                && let (Ok(r), Ok(g), Ok(b)) = (r.parse::<u8>(), g.parse::<u8>(), b.parse::<u8>())
            {
                colors.insert(name.clone(), [r, g, b]);
            }
        }
    }
    let mut stipples: HashMap<String, FillStyle> = HashMap::new();
    for def in drf.all("drDefineStipple") {
        for e in &def.items()[1..] {
            let it = e.items();
            if let (Some(name), Some(Sx::List(rows))) = (it.get(1).and_then(Sx::atom), it.get(2)) {
                let bits: Vec<Vec<u8>> =
                    rows.iter().map(|r| r.strings().iter().filter_map(|b| b.parse::<u8>().ok()).collect()).collect();
                stipples.insert(name.to_string(), stipple_style(&bits));
            }
        }
    }
    let mut packets = HashMap::new();
    for def in drf.all("drDefinePacket") {
        for e in &def.items()[1..] {
            let s = e.strings();
            // ( display packet stipple lineStyle fillColor outlineColor [fillStyle] )
            if let [_, packet, stipple, _line, fill, ..] = s.as_slice() {
                let Some(color) = colors.get(fill) else {
                    warnings.push(format!("display packet {packet}: unknown colour '{fill}'"));
                    continue;
                };
                let style = match s.get(6).map(String::as_str) {
                    Some("outline") => FillStyle::Outline,
                    Some("solid") => FillStyle::Solid,
                    _ => stipples.get(stipple).copied().unwrap_or(FillStyle::Hatch),
                };
                packets.insert(packet.clone(), (*color, style));
            }
        }
    }
    packets
}

/// Technology named `name` from a Virtuoso technology file, its display resource file and a
/// stream layer map; with notes on what was guessed or skipped.
pub fn from_techfile(tf: &str, drf: Option<&str>, layer_map: Option<&str>, name: &str) -> Result<(Tech, Vec<String>), String> {
    let root = parse(tf).map_err(|e| format!("technology file: {e}"))?;
    let mut warnings = vec![];
    let packets = match drf {
        Some(d) => read_drf(&parse(d).map_err(|e| format!("display file: {e}"))?, &mut warnings),
        None => HashMap::new(),
    };

    // Layers and purposes with their numbers.
    let mut layer_no: BTreeMap<String, u16> = BTreeMap::new();
    for def in root.all("techLayers") {
        for e in &def.items()[1..] {
            let s = e.strings();
            if let [layer, num, ..] = s.as_slice()
                && let Ok(n) = num.parse::<u16>()
            {
                layer_no.insert(layer.clone(), n);
            }
        }
    }
    if layer_no.is_empty() {
        return Err("the technology file has no techLayers".into());
    }
    let mut purpose_no: HashMap<String, u16> = [("drawing".to_string(), 252)].into();
    for def in root.all("techPurposes") {
        for e in &def.items()[1..] {
            let s = e.strings();
            if let [p, num, ..] = s.as_slice()
                && let Ok(n) = num.parse::<u16>()
            {
                purpose_no.insert(p.clone(), n);
            }
        }
    }
    // Display packet per layer-purpose.
    let mut display: HashMap<(String, String), String> = HashMap::new();
    let mut display_order = vec![];
    for def in root.all("techDisplays") {
        for e in &def.items()[1..] {
            let s = e.strings();
            if let [l, p, packet, ..] = s.as_slice() {
                display.insert((l.clone(), p.clone()), packet.clone());
                display_order.push((l.clone(), p.clone()));
            }
        }
    }
    let mut pairs: Vec<(String, String)> = vec![];
    for def in root.all("techLayerPurposePriorities") {
        for e in &def.items()[1..] {
            if let [l, p, ..] = e.strings().as_slice() {
                pairs.push((l.clone(), p.clone()));
            }
        }
    }
    if pairs.is_empty() {
        pairs = display_order;
    }
    let mut seen = HashSet::new();
    pairs.retain(|(l, _)| layer_no.contains_key(l));
    pairs.retain(|p| seen.insert(p.clone()));
    if pairs.is_empty() {
        return Err("the technology file defines no layer-purpose pairs".into());
    }

    // Units.
    let mut dbu_per_um = 1000;
    for def in root.all("viewTypeUnits") {
        for e in &def.items()[1..] {
            let s = e.strings();
            if let [view, unit, dbu, ..] = s.as_slice()
                && view == "maskLayout"
                && let Ok(d) = dbu.parse::<f64>()
            {
                dbu_per_um = match unit.as_str() {
                    "micron" => d.round() as i64,
                    "millimeter" => (d / 1000.0).round() as i64,
                    other => {
                        warnings.push(format!("maskLayout unit '{other}' is read as micron"));
                        d.round() as i64
                    }
                };
            }
        }
    }
    let um = |v: f64| (v * dbu_per_um as f64).round() as i64;
    let mut grid = 5.min(dbu_per_um.max(1));
    for def in root.all("mfgGridResolution") {
        if let Some(v) = def
            .items()
            .iter()
            .skip(1)
            .flat_map(|e| e.strings().into_iter().chain(e.atom().map(str::to_string)))
            .find_map(|v| v.parse::<f64>().ok())
        {
            grid = um(v).max(1);
        }
    }

    // Single-layer rules (micron values).
    let mut rules: HashMap<(String, &str), f64> = HashMap::new();
    let groups = root.all("constraintGroups");
    let mut rule_lists: Vec<&Sx> = vec![];
    for g in &groups {
        for group in &g.items()[1..] {
            let s = group.strings();
            if s.first().is_some_and(|n| n == "foundry") {
                rule_lists.extend(group.all("spacings"));
            }
        }
    }
    rule_lists.extend(root.all("spacingRules"));
    for list in rule_lists {
        for e in &list.items()[1..] {
            let s = e.strings();
            if let [kind, layer, value] = s.as_slice()
                && let Ok(v) = value.parse::<f64>()
            {
                let kind = match kind.as_str() {
                    "minWidth" => "w",
                    "minSpacing" => "s",
                    "minArea" => "a",
                    _ => continue,
                };
                rules.insert((layer.clone(), kind), v);
            }
        }
    }

    let mut layers = vec![];
    for (k, (l, p)) in pairs.iter().enumerate() {
        let name = if p == "drawing" { l.clone() } else { format!("{l}.{p}") };
        let packet = display.get(&(l.clone(), p.clone()));
        let (color, fill) = match packet.and_then(|pk| packets.get(pk)) {
            Some(&(c, f)) => (c, f),
            None => {
                let [r, g, b, _] = auto_color(k);
                ([r, g, b], if p == "drawing" { FillStyle::Hatch } else { FillStyle::Outline })
            }
        };
        let drawing = p == "drawing";
        let rule = |kind| if drawing { rules.get(&(l.clone(), kind)).copied() } else { None };
        layers.push(LayerDef {
            routing: drawing && looks_routing(l),
            name,
            gds_layer: layer_no[l],
            gds_datatype: if drawing { 0 } else { purpose_no.get(p).copied().unwrap_or(0) % 256 },
            color: [color[0], color[1], color[2], 160],
            fill,
            min_width: rule("w").map_or(0, um),
            min_spacing: rule("s").map_or(0, um),
            min_area: rule("a").map_or(0, |a| (a * (dbu_per_um * dbu_per_um) as f64).round() as i64),
            text_stream: None,
        });
    }
    if drf.is_some() {
        let missing = pairs.iter().filter(|p| display.get(*p).is_none_or(|pk| !packets.contains_key(pk))).count();
        if missing > 0 {
            warnings.push(format!("{missing} layer-purpose pairs have no display packet; they get generated colours"));
        }
    }

    let mut tech =
        Tech { name: name.into(), dbu_per_um, grid, layers, vias: vec![], drc_deck: None, caps: vec![], ..Tech::generic() };
    match layer_map {
        Some(map) => {
            let (entries, mut notes) = crate::stream_map::parse(map, &tech);
            warnings.append(&mut notes);
            for l in &mut tech.layers {
                if let Some(e) = entries.iter().find(|e| !e.text && e.layer.eq_ignore_ascii_case(&l.name)) {
                    l.gds_layer = e.stream_layer;
                    l.gds_datatype = e.datatype;
                }
            }
            tech.stream_map = entries;
        }
        None => warnings.push(
            "no layer map: stream numbers are the techLayers numbers (datatype 0 for drawing, the purpose number otherwise)"
                .into(),
        ),
    }
    Ok((tech, warnings))
}

fn looks_routing(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let numbered = |p: &str| lower.strip_prefix(p).is_some_and(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_digit()));
    lower == "poly" || ["poly", "li", "m", "met", "metal"].into_iter().any(numbered)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TF: &str = r#"
; Example technology file
controls(
  techParams( ( lambda 1.0 ) )
  viewTypeUnits( ( maskLayout "micron" 2000 ) )
  mfgGridResolution( ( 0.005 ) )
)
layerDefinitions(
  techPurposes(
   ;( PurposeName Purpose# Abbreviation )
    ( drawing 252 drw )
    ( pin     251 pin )
    ( label   237 lbl )
  )
  techLayers(
   ;( LayerName Layer# Abbreviation )
    ( Poly    10 PO )
    ( Metal1  30 M1 )
    ( Via1    31 V1 )
  )
  techLayerPurposePriorities(
    ( Poly drawing )
    ( Metal1 drawing )
    ( Metal1 pin )
    ( Via1 drawing )
    ( Ghost drawing )
  )
  techDisplays(
   ;( LayerName Purpose Packet Vis Sel Con2ChgLy DrgEnbl Valid )
    ( Poly   drawing poly    t t t t t )
    ( Metal1 drawing metal1  t t t t t )
    ( Metal1 pin     m1pin   t t t t t )
    ( Via1   drawing via1    t t t t t )
  )
)
constraintGroups(
  ( "foundry" nil
    spacings(
      ( minWidth   "Metal1" 0.09 )
      ( minSpacing "Metal1" 0.1 )
      ( minArea    "Metal1" 0.042 )
      ( minSpacing "Metal1" "Poly" 0.05 )
    )
  )
)
physicalRules( spacingRules( ( minWidth "Poly" 0.05 ) ) )
"#;

    const DRF: &str = r#"
drDefineDisplay( ( display ) )
drDefineColor(
  ( display red    255 0 0 )
  ( display blue   0 0 255 )
  ( display green  0 204 102 )
)
drDefineStipple(
  ( display solid ( (1 1 1 1) (1 1 1 1) (1 1 1 1) (1 1 1 1) ) )
  ( display diag  ( (1 0 0 0) (0 1 0 0) (0 0 1 0) (0 0 0 1) ) )
  ( display cross ( (1 0 0 1) (0 1 1 0) (0 1 1 0) (1 0 0 1) ) )
  ( display dots  ( (1 0 0 0) (0 0 0 0) (0 0 1 0) (0 0 0 0) ) )
)
drDefinePacket(
  ;( DisplayName PacketName Stipple LineStyle Fill Outline )
  ( display poly   diag  solid red   red )
  ( display metal1 solid solid blue  blue )
  ( display m1pin  cross solid blue  blue outlineStipple )
  ( display via1   dots  solid green green )
)
"#;

    #[test]
    fn reads_layers_units_rules_and_display() {
        let (t, w) = from_techfile(TF, Some(DRF), None, "demo").unwrap();
        let names: Vec<&str> = t.layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["Poly", "Metal1", "Metal1.pin", "Via1"]);
        assert_eq!((t.dbu_per_um, t.grid), (2000, 10));
        let m1 = &t.layers[1];
        assert_eq!((m1.gds_layer, m1.gds_datatype, m1.min_width, m1.min_spacing, m1.min_area), (30, 0, 180, 200, 168000));
        assert!(m1.routing && !t.layers[3].routing);
        assert_eq!((t.layers[0].min_width, t.layers[0].fill), (100, FillStyle::BackHatch));
        assert_eq!(&m1.color[..3], &[0, 0, 255]);
        assert_eq!(m1.fill, FillStyle::Solid);
        assert_eq!((t.layers[2].gds_datatype, t.layers[2].fill), (251, FillStyle::Cross));
        assert_eq!(t.layers[3].fill, FillStyle::Dots);
        assert!(w.iter().any(|w| w.contains("no layer map")), "{w:?}");
    }

    #[test]
    fn layer_map_sets_stream_numbers() {
        let map = "# layer map\nMetal1 drawing 31 0\nMetal1 pin 31 2\nPoly drawing 7 0\n";
        let (t, _) = from_techfile(TF, None, Some(map), "demo").unwrap();
        let get = |n: &str| t.layers.iter().find(|l| l.name == n).map(|l| (l.gds_layer, l.gds_datatype));
        assert_eq!(get("Metal1"), Some((31, 0)));
        assert_eq!(get("Metal1.pin"), Some((31, 2)));
        assert_eq!(get("Poly"), Some((7, 0)));
        assert_eq!(t.stream_map.len(), 3);
        // Without a display file the colours are generated.
        assert!(t.layers.iter().all(|l| l.color[3] == 160));
    }

    #[test]
    fn errors() {
        assert!(from_techfile("layerDefinitions( techLayers( ) )", None, None, "x").unwrap_err().contains("no techLayers"));
        assert!(from_techfile("techLayers( ( M1 1 M1 )", None, None, "x").unwrap_err().contains("unbalanced"));
    }

    #[test]
    fn stipple_classification() {
        let s = |rows: &[&[u8]]| stipple_style(&rows.iter().map(|r| r.to_vec()).collect::<Vec<_>>());
        assert_eq!(s(&[&[0, 0], &[0, 0]]), FillStyle::Outline);
        assert_eq!(s(&[&[0, 0, 0, 1], &[0, 0, 1, 0], &[0, 1, 0, 0], &[1, 0, 0, 0]]), FillStyle::Hatch);
    }
}
