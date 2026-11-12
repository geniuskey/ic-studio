//! Technology import from KLayout layer properties (`.lyp`) and technology (`.lyt`) files.
//!
//! Layer mapping rules of [`from_lyp`]:
//! - every `<properties>` / `<group-members>` entry whose `<source>` holds a concrete
//!   `layer/datatype` becomes a layer. Group headers (`*/*`), name-only or wildcard sources,
//!   entries with `<valid>false</valid>` and repeats of an already seen layer/datatype are skipped;
//! - name: `<name>` without a trailing `L/D` (or ` - L/D`), else the name part of the source,
//!   else `L{l}D{d}`. Whitespace becomes `_`; clashes (case-insensitive) get `_2`, `_3`, ...;
//! - colour: `<fill-color>`, else `<frame-color>`, else the enclosing group's, else a generated
//!   one; alpha is always [`ALPHA`];
//! - fill: from the stock KLayout dither pattern (see [`fill_style`]); custom patterns hatch;
//! - `routing`: the name before any `.purpose` (none or `drawing`) is `poly`, `li`, or `poly`,
//!   `li`, `m`, `met`, `metal` followed by digits (`met1`, `li1`, `Metal2`, `Poly2`);
//! - no width / spacing rules, vias or capacitances; 1000 dbu per µm and a 5 dbu grid.
//!
//! `<visible>` has no counterpart in [`LayerDef`] and is ignored.

use std::collections::HashSet;

use crate::tech::{FillStyle, LayerDef, Tech, auto_color};

/// Display alpha of imported layers.
pub const ALPHA: u8 = 160;

#[derive(Debug, Default)]
pub(crate) struct Node {
    pub(crate) name: String,
    pub(crate) text: String,
    pub(crate) children: Vec<Node>,
}

impl Node {
    pub(crate) fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    pub(crate) fn child_text(&self, name: &str) -> Option<&str> {
        self.child(name).map(|c| c.text.trim())
    }
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let decoded = rest[1..].find(';').filter(|&e| e <= 10).and_then(|e| {
            let ent = &rest[1..1 + e];
            let c = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => match ent.strip_prefix("#x").or_else(|| ent.strip_prefix("#X")) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok().and_then(char::from_u32),
                    None => ent.strip_prefix('#').and_then(|d| d.parse().ok()).and_then(char::from_u32),
                },
            };
            c.map(|c| (c, e + 2))
        });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A small, tolerant XML reader: elements and their text (attributes are skipped), comments,
/// CDATA, processing instructions, DOCTYPE and the predefined / numeric entities.
pub(crate) fn parse_xml(src: &str) -> Result<Node, String> {
    let line = |rest: &str| src[..src.len() - rest.len()].matches('\n').count() + 1;
    let mut stack = vec![Node::default()];
    let mut rest = src.strip_prefix('\u{feff}').unwrap_or(src);
    let unterminated = |what: &str, at: &str| format!("line {}: unterminated {what}", line(at));
    while !rest.is_empty() {
        let lt = rest.find('<').unwrap_or(rest.len());
        let top = stack.last_mut().expect("document node");
        top.text.push_str(&decode_entities(&rest[..lt]));
        rest = &rest[lt..];
        if rest.is_empty() {
            break;
        }
        if let Some(r) = rest.strip_prefix("<!--") {
            let end = r.find("-->").ok_or_else(|| unterminated("comment", rest))?;
            rest = &r[end + 3..];
        } else if let Some(r) = rest.strip_prefix("<![CDATA[") {
            let end = r.find("]]>").ok_or_else(|| unterminated("CDATA section", rest))?;
            top.text.push_str(&r[..end]);
            rest = &r[end + 3..];
        } else if let Some(r) = rest.strip_prefix("<?") {
            let end = r.find("?>").ok_or_else(|| unterminated("processing instruction", rest))?;
            rest = &r[end + 2..];
        } else if let Some(r) = rest.strip_prefix("<!") {
            let mut depth = 0i32;
            let end = r
                .char_indices()
                .find(|&(_, c)| {
                    match c {
                        '[' => depth += 1,
                        ']' => depth -= 1,
                        _ => {}
                    }
                    c == '>' && depth <= 0
                })
                .ok_or_else(|| unterminated("declaration", rest))?
                .0;
            rest = &r[end + 1..];
        } else if let Some(r) = rest.strip_prefix("</") {
            let end = r.find('>').ok_or_else(|| unterminated("end tag", rest))?;
            let name = r[..end].trim();
            if stack.len() == 1 {
                return Err(format!("line {}: unexpected </{name}>", line(rest)));
            }
            let node = stack.pop().expect("open element");
            if node.name != name {
                return Err(format!("line {}: expected </{}>, found </{name}>", line(rest), node.name));
            }
            stack.last_mut().expect("parent").children.push(node);
            rest = &r[end + 1..];
        } else {
            let r = &rest[1..];
            let mut quote = None;
            let mut end = None;
            for (i, c) in r.char_indices() {
                match quote {
                    Some(q) if c == q => quote = None,
                    Some(_) => {}
                    None if c == '"' || c == '\'' => quote = Some(c),
                    None if c == '>' => {
                        end = Some(i);
                        break;
                    }
                    None if c == '<' => return Err(format!("line {}: '<' inside a tag", line(rest))),
                    None => {}
                }
            }
            let end = end.ok_or_else(|| unterminated("tag", rest))?;
            let (tag, empty) = match r[..end].strip_suffix('/') {
                Some(t) => (t, true),
                None => (&r[..end], false),
            };
            let name = tag.split_whitespace().next().unwrap_or_default();
            if name.is_empty() {
                return Err(format!("line {}: tag without a name", line(rest)));
            }
            let node = Node { name: name.to_string(), ..Node::default() };
            if empty {
                stack.last_mut().expect("parent").children.push(node);
            } else {
                stack.push(node);
            }
            rest = &r[end + 1..];
        }
    }
    if stack.len() > 1 {
        return Err(format!("unexpected end of file: <{}> is not closed", stack.last().expect("open element").name));
    }
    let mut doc = stack.pop().expect("document node");
    match doc.children.len() {
        0 => Err("no XML root element".into()),
        1 => Ok(doc.children.pop().expect("root")),
        _ => Err("more than one XML root element".into()),
    }
}

fn layer_datatype(tok: &str) -> Option<(u16, u16)> {
    let (l, d) = tok.split('@').next()?.split_once('/')?;
    Some((l.parse().ok()?, d.parse().ok()?))
}

/// The concrete `(layer, datatype)` and the layer name of a KLayout source specification
/// such as `met1.drawing 68/20@1`, `68/20` or `nwell (64/20)`.
fn parse_source(src: &str) -> (Option<(u16, u16)>, Option<&str>) {
    let mut name = None;
    for tok in src.split_whitespace() {
        let t = tok.trim_matches(['(', ')']);
        let core = t.split('@').next().unwrap_or_default();
        if core.contains('/') {
            return (layer_datatype(core), name);
        }
        if let Ok(l) = core.parse::<u16>() {
            return (Some((l, 0)), name);
        }
        if !core.is_empty() && name.is_none() {
            name = Some(core);
        }
    }
    (None, name)
}

/// `<name>` without the ` - L/D` suffix KLayout files often carry; empty if only numbers remain.
fn clean_name(name: &str) -> String {
    let mut n = name.trim();
    if let Some((head, tail)) = n.rsplit_once(char::is_whitespace)
        && layer_datatype(tail.trim_matches(['(', ')'])).is_some()
    {
        n = head.trim_end().trim_end_matches('-').trim_end();
    }
    if layer_datatype(n).is_some() {
        return String::new();
    }
    n.split_whitespace().collect::<Vec<_>>().join("_")
}

fn parse_color(s: &str) -> Option<[u8; 3]> {
    let hex = s.trim().strip_prefix('#')?;
    let hex = hex.get(hex.len().checked_sub(6)?..)?;
    let v = u32::from_str_radix(hex, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

/// Our fill for a KLayout dither pattern: `I0` solid, `I1` hollow, dotted / checkerboard / bubble
/// patterns dots, left-hatched (and 22.5°/67.5° down) back-hatch, right-hatched (and up) hatch,
/// cross-hatched / plus patterns cross. Everything else, including custom `C<n>`, hatches.
pub fn fill_style(pattern: &str) -> FillStyle {
    let Some(n) = pattern.trim().strip_prefix('I').and_then(|n| n.parse::<u32>().ok()) else {
        return FillStyle::Hatch;
    };
    match n {
        0 => FillStyle::Solid,
        1 => FillStyle::Outline,
        2 | 3 | 14 | 16 | 17 | 20 | 21 => FillStyle::Dots,
        4..=7 | 26 | 28 => FillStyle::BackHatch,
        12 | 13 | 15 | 18 | 24 | 30 => FillStyle::Cross,
        _ => FillStyle::Hatch,
    }
}

fn looks_routing(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let (base, purpose) = lower.split_once('.').unwrap_or((&lower, ""));
    if !(purpose.is_empty() || purpose == "drawing") {
        return false;
    }
    let numbered = |p: &str| base.strip_prefix(p).is_some_and(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_digit()));
    base == "poly" || base == "li" || ["poly", "li", "m", "met", "metal"].into_iter().any(numbered)
}

#[derive(Clone, Default)]
struct Style {
    color: Option<[u8; 3]>,
    pattern: Option<String>,
}

struct Entry {
    ld: (u16, u16),
    name: String,
    style: Style,
}

fn collect(node: &Node, inherited: &Style, out: &mut Vec<Entry>) {
    for c in &node.children {
        if c.name != "properties" && c.name != "group-members" {
            collect(c, inherited, out);
            continue;
        }
        let color = |tag: &str| c.child_text(tag).and_then(parse_color);
        let style = Style {
            color: color("fill-color").or_else(|| color("frame-color")).or(inherited.color),
            pattern: c.child_text("dither-pattern").filter(|p| !p.is_empty()).map(str::to_string).or(inherited.pattern.clone()),
        };
        let (ld, src_name) = parse_source(c.child_text("source").unwrap_or_default());
        if let Some(ld) = ld
            && c.child_text("valid") != Some("false")
        {
            let name = Some(clean_name(c.child_text("name").unwrap_or_default()))
                .filter(|n| !n.is_empty())
                .or_else(|| src_name.map(str::to_string))
                .unwrap_or_else(|| format!("L{}D{}", ld.0, ld.1));
            out.push(Entry { ld, name, style: style.clone() });
        }
        collect(c, &style, out);
    }
}

/// Build a technology named `name` from the text of a KLayout layer properties file.
pub fn from_lyp(xml: &str, name: &str) -> Result<Tech, String> {
    let root = parse_xml(xml)?;
    if root.name != "layer-properties" && root.name != "layer-properties-tabs" {
        return Err(format!("not a KLayout layer properties file (root element <{}>)", root.name));
    }
    let mut entries = vec![];
    collect(&root, &Style::default(), &mut entries);
    let mut seen_ld = HashSet::new();
    let mut used_names = HashSet::new();
    let mut layers = vec![];
    for e in entries {
        if !seen_ld.insert(e.ld) {
            continue;
        }
        let mut name = e.name.clone();
        let mut k = 1;
        while !used_names.insert(name.to_ascii_lowercase()) {
            k += 1;
            name = format!("{}_{k}", e.name);
        }
        let [r, g, b, _] = auto_color(layers.len());
        let [r, g, b] = e.style.color.unwrap_or([r, g, b]);
        layers.push(LayerDef {
            routing: looks_routing(&name),
            name,
            gds_layer: e.ld.0,
            gds_datatype: e.ld.1,
            color: [r, g, b, ALPHA],
            fill: e.style.pattern.as_deref().map_or(FillStyle::Hatch, fill_style),
            min_width: 0,
            min_spacing: 0,
        });
    }
    if layers.is_empty() {
        return Err("the layer properties file defines no layer with a concrete layer/datatype".into());
    }
    Ok(Tech {
        name: name.into(),
        dbu_per_um: 1000,
        grid: 5,
        layers,
        vias: vec![],
        drc_deck: None,
        caps: vec![],
        ..Tech::generic()
    })
}

/// Apply a KLayout technology file: `<name>`, `<dbu>` (µm per dbu; the grid keeps its physical
/// size) and, if present, `<default-grids>` (µm, comma separated; the entry marked `!`, else
/// the first, becomes the grid).
pub fn apply_lyt(tech: &mut Tech, xml: &str) -> Result<(), String> {
    let root = parse_xml(xml)?;
    if root.name != "technology" {
        return Err(format!("not a KLayout technology file (root element <{}>)", root.name));
    }
    if let Some(d) = root.child_text("dbu").filter(|d| !d.is_empty()) {
        let um: f64 = d.parse().map_err(|_| format!("invalid <dbu> '{d}'"))?;
        let per_um = 1.0 / um;
        let n = per_um.round();
        if um <= 0.0 || n < 1.0 || (per_um - n).abs() > 1e-6 * n {
            return Err(format!("<dbu> {d} µm is not a whole fraction of a micron"));
        }
        let grid_um = tech.grid as f64 / tech.dbu_per_um.max(1) as f64;
        tech.dbu_per_um = n as i64;
        tech.grid = ((grid_um * n).round() as i64).max(1);
    }
    if let Some(n) = root.child_text("name").filter(|n| !n.is_empty()) {
        tech.name = n.to_string();
    }
    if let Some(g) = root.child_text("default-grids").filter(|g| !g.is_empty()) {
        let grids: Vec<&str> = g.split(',').map(str::trim).collect();
        let pick = grids.iter().find(|s| s.ends_with('!')).or(grids.first()).map(|s| s.trim_end_matches('!'));
        if let Some(um) = pick.and_then(|s| s.parse::<f64>().ok()).filter(|v| *v > 0.0) {
            tech.grid = ((um * tech.dbu_per_um as f64).round() as i64).max(1);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Excerpt in the style of the SkyWater SKY130 `sky130A.lyp`.
    const LYP: &str = r##"<?xml version="1.0" encoding="utf-8"?>
<!-- generated by a script; do not edit -->
<layer-properties>
 <properties>
  <frame-color>#ffffcc</frame-color>
  <fill-color>#ffffcc</fill-color>
  <frame-brightness>0</frame-brightness>
  <fill-brightness>0</fill-brightness>
  <dither-pattern>I1</dither-pattern>
  <line-style/>
  <valid>true</valid>
  <visible>true</visible>
  <transparent>false</transparent>
  <width>1</width>
  <marked>false</marked>
  <xfill>false</xfill>
  <animation>0</animation>
  <name>nwell.drawing - 64/20</name>
  <source>64/20@1</source>
 </properties>
 <properties>
  <frame-color>#00cc66</frame-color>
  <fill-color>#00cc66</fill-color>
  <dither-pattern>I5</dither-pattern>
  <valid>true</valid>
  <visible>true</visible>
  <name>diff.drawing - 65/20</name>
  <source>65/20@1</source>
 </properties>
 <properties>
  <frame-color>#ff0000</frame-color>
  <fill-color>#ff0000</fill-color>
  <dither-pattern>I9</dither-pattern>
  <valid>true</valid>
  <visible>true</visible>
  <name>poly.drawing - 66/20</name>
  <source>66/20@1</source>
 </properties>
 <properties>
  <frame-color>#ffffff</frame-color>
  <fill-color>#ffffff</fill-color>
  <dither-pattern>I0</dither-pattern>
  <valid>true</valid>
  <visible>false</visible>
  <name/>
  <source>licon1.drawing 66/44@1</source>
 </properties>
 <properties>
  <frame-color>#ffccff</frame-color>
  <dither-pattern>I12</dither-pattern>
  <valid>true</valid>
  <name>li1.drawing - 67/20</name>
  <source>67/20@1</source>
 </properties>
 <properties>
  <frame-color>#0000ff</frame-color>
  <fill-color>#0000ff</fill-color>
  <dither-pattern>I2</dither-pattern>
  <valid>true</valid>
  <visible>true</visible>
  <name>Metal 1 &amp; pins</name>
  <source>*/*@*</source>
  <group-members>
   <frame-color>#0000ff</frame-color>
   <fill-color>#3366ff</fill-color>
   <dither-pattern>I8</dither-pattern>
   <valid>true</valid>
   <name>met1.drawing - 68/20</name>
   <source>68/20@1</source>
  </group-members>
  <group-members>
   <dither-pattern>C0</dither-pattern>
   <valid>true</valid>
   <name>met1.pin - 68/16</name>
   <source>68/16@1</source>
  </group-members>
  <group-members>
   <valid>true</valid>
   <name>met1.drawing</name>
   <source>68/5@1</source>
  </group-members>
 </properties>
 <properties>
  <fill-color>#cccccc</fill-color>
  <dither-pattern>I3</dither-pattern>
  <valid>false</valid>
  <name>areaid.sc - 81/4</name>
  <source>81/4@1</source>
 </properties>
 <properties>
  <fill-color>#9900ff</fill-color>
  <dither-pattern>I15</dither-pattern>
  <valid>true</valid>
  <source>69/20</source>
 </properties>
 <properties>
  <fill-color>#aaaaaa</fill-color>
  <valid>true</valid>
  <name>duplicate of nwell</name>
  <source>64/20@1</source>
 </properties>
 <properties>
  <fill-color>#123456</fill-color>
  <valid>true</valid>
  <source>met2.drawing 69/44@1</source>
 </properties>
 <properties>
  <valid>true</valid>
  <name>named only</name>
  <source>met3</source>
 </properties>
 <name>sky130</name>
</layer-properties>
"##;

    fn find<'a>(t: &'a Tech, name: &str) -> &'a LayerDef {
        t.layers.iter().find(|l| l.name == name).unwrap_or_else(|| panic!("no layer {name}"))
    }

    #[test]
    fn lyp_layers_groups_and_styles() {
        let t = from_lyp(LYP, "sky130").unwrap();
        let names: Vec<&str> = t.layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "nwell.drawing",
                "diff.drawing",
                "poly.drawing",
                "licon1.drawing",
                "li1.drawing",
                "met1.drawing",
                "met1.pin",
                "met1.drawing_2",
                "L69D20",
                "met2.drawing"
            ]
        );
        assert_eq!((t.dbu_per_um, t.grid, t.name.as_str()), (1000, 5, "sky130"));
        let nwell = find(&t, "nwell.drawing");
        assert_eq!((nwell.gds_layer, nwell.gds_datatype), (64, 20));
        assert_eq!(nwell.color, [0xff, 0xff, 0xcc, ALPHA]);
        assert_eq!(nwell.fill, FillStyle::Outline);
        assert!(!nwell.routing);
        assert_eq!(find(&t, "diff.drawing").fill, FillStyle::BackHatch);
        assert_eq!(find(&t, "poly.drawing").fill, FillStyle::Hatch);
        assert_eq!(find(&t, "licon1.drawing").fill, FillStyle::Solid);
        let li = find(&t, "li1.drawing");
        assert_eq!((li.color, li.fill), ([0xff, 0xcc, 0xff, ALPHA], FillStyle::Cross), "frame colour as fallback");
        let m1 = find(&t, "met1.drawing");
        assert_eq!((m1.gds_layer, m1.gds_datatype, m1.color, m1.fill), (68, 20, [0x33, 0x66, 0xff, ALPHA], FillStyle::Hatch));
        let pin = find(&t, "met1.pin");
        assert_eq!((pin.color, pin.fill), ([0, 0, 0xff, ALPHA], FillStyle::Hatch), "colour inherited from the group");
        let label = find(&t, "met1.drawing_2");
        assert_eq!((label.gds_datatype, label.fill), (5, FillStyle::Dots), "pattern inherited from the group");
        assert_eq!(find(&t, "L69D20").fill, FillStyle::Cross);
        assert_eq!(find(&t, "met2.drawing").color, [0x12, 0x34, 0x56, ALPHA]);
        let routing: Vec<&str> = t.layers.iter().filter(|l| l.routing).map(|l| l.name.as_str()).collect();
        assert_eq!(routing, ["poly.drawing", "li1.drawing", "met1.drawing", "met2.drawing"]);
        assert!(t.layers.iter().all(|l| l.min_width == 0 && l.min_spacing == 0));
        assert_eq!(t.layer_by_gds(64, 20), Some(0), "duplicate layer/datatype entries are dropped");
        assert!(t.layer_by_gds(81, 4).is_none(), "invalid entries are skipped");
    }

    #[test]
    fn source_and_name_parsing() {
        assert_eq!(parse_source("met1.drawing 68/20@1"), (Some((68, 20)), Some("met1.drawing")));
        assert_eq!(parse_source("68/20"), (Some((68, 20)), None));
        assert_eq!(parse_source("nwell (64/20)"), (Some((64, 20)), Some("nwell")));
        assert_eq!(parse_source("17@2"), (Some((17, 0)), None));
        assert_eq!(parse_source("*/*@*"), (None, None));
        assert_eq!(parse_source("68/*"), (None, None));
        assert_eq!(clean_name("met1.drawing - 68/20"), "met1.drawing");
        assert_eq!(clean_name("Metal 1 (34/0)"), "Metal_1");
        assert_eq!(clean_name("68/20"), "");
        assert_eq!(fill_style("I27"), FillStyle::Hatch);
        assert_eq!(fill_style("I28"), FillStyle::BackHatch);
        assert_eq!(fill_style("C3"), FillStyle::Hatch);
        assert!(looks_routing("Metal2") && looks_routing("M1") && looks_routing("Poly2") && looks_routing("li1.drawing"));
        assert!(!looks_routing("met1.pin") && !looks_routing("mim") && !looks_routing("licon1"));
    }

    #[test]
    fn xml_reader_is_tolerant_but_rejects_broken_files() {
        let doc = parse_xml(
            "\u{feff}<?xml version=\"1.0\"?>\n<!DOCTYPE x [<!ENTITY e \"v\">]>\n<a k='1>2' j=\"x\"><b>&lt;&#65;&#x42;&quot;&apos;&gt; &bogus; & </b><![CDATA[<raw>]]><c/></a>",
        )
        .unwrap();
        assert_eq!(doc.name, "a");
        assert_eq!(doc.child_text("b"), Some("<AB\"'> &bogus; &"));
        assert_eq!(doc.text, "<raw>");
        assert!(doc.child("c").is_some());
        for bad in ["", "plain text", "<a><b></a>", "<a>", "<a></a><b/>", "<a><!-- open</a>", "</a>", "<a <b>"] {
            assert!(parse_xml(bad).is_err(), "{bad:?} should fail");
        }
        let err = from_lyp("<layer-properties>\n<properties>\n</layer-properties>", "x").unwrap_err();
        assert!(err.contains("line 3"), "{err}");
        assert!(from_lyp("<technology/>", "x").is_err());
        assert!(from_lyp("<layer-properties><properties><source>*/*</source></properties></layer-properties>", "x").is_err());
    }

    #[test]
    fn lyt_dbu_name_and_grid() {
        let mut t = from_lyp(LYP, "x").unwrap();
        let lyt = r#"<?xml version="1.0" encoding="utf-8"?>
<technology>
 <name>sky130A</name>
 <description>SkyWater 130nm</description>
 <dbu>0.001</dbu>
 <layer-properties_file>sky130A.lyp</layer-properties_file>
 <reader-options><gds2><name>ignored</name></gds2></reader-options>
</technology>"#;
        apply_lyt(&mut t, lyt).unwrap();
        assert_eq!((t.name.as_str(), t.dbu_per_um, t.grid), ("sky130A", 1000, 5));
        apply_lyt(&mut t, "<technology><dbu>0.005</dbu></technology>").unwrap();
        assert_eq!((t.name.as_str(), t.dbu_per_um, t.grid), ("sky130A", 200, 1));
        apply_lyt(&mut t, "<technology><dbu>0.0005</dbu><default-grids>0.01,0.005!,0.001</default-grids></technology>").unwrap();
        assert_eq!((t.dbu_per_um, t.grid), (2000, 10));
        assert!(apply_lyt(&mut t, "<technology><dbu>0.0003</dbu></technology>").is_err());
        assert!(apply_lyt(&mut t, "<technology><dbu>-1</dbu></technology>").is_err());
        assert!(apply_lyt(&mut t, "<technology><dbu>abc</dbu></technology>").is_err());
        assert!(apply_lyt(&mut t, "<layer-properties/>").is_err());
        assert_eq!(t.dbu_per_um, 2000, "failed applications leave the technology unchanged");
    }
}
