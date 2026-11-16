//! Stream layer map (Virtuoso XStream layer map file): which GDSII / OASIS (layer, datatype)
//! pairs technology layers are read from and written as on import and export.
//!
//! A map file has one `layerName purpose streamLayer streamDatatype` row per line; `#` and `;`
//! start comments. Purposes:
//!
//! - `drawing` (or `drw`) names the layer `layerName`;
//! - any other purpose (`pin`, `exclude`, `dummy`, `fill`, `blockage`, ...) names the layer
//!   `layerName.purpose`, `layerNamepurpose` or `layerName_purpose` when the technology has one
//!   of those;
//! - `label` and `text` do too, and otherwise carry the text labels of `layerName` itself
//!   ([`StreamMapEntry::text`]), so the labels name the nets of the drawn shapes.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use crate::tech::{LayerDef, StreamMapEntry, Tech};

/// The stream pair every technology layer's shapes are written as: its first map row that is
/// not a label row, else its own pair.
pub fn export_pairs(tech: &Tech) -> Vec<(u16, u16)> {
    tech.layers
        .iter()
        .map(|l| {
            tech.stream_map
                .iter()
                .find(|e| !e.text && e.layer.eq_ignore_ascii_case(&l.name))
                .map_or((l.gds_layer, l.gds_datatype), |e| (e.stream_layer, e.datatype))
        })
        .collect()
}

/// The stream pair every technology layer's labels are written as: its first label row, else
/// the pair of its shapes ([`export_pairs`]).
pub fn export_text_pairs(tech: &Tech) -> Vec<(u16, u16)> {
    tech.layers
        .iter()
        .zip(export_pairs(tech))
        .map(|(l, shapes)| {
            tech.stream_map
                .iter()
                .find(|e| e.text && e.layer.eq_ignore_ascii_case(&l.name))
                .map_or(shapes, |e| (e.stream_layer, e.datatype))
        })
        .collect()
}

/// `tech` with its layers' stream pairs replaced by the map's, for writing a stream file.
pub fn export_tech(tech: &Tech) -> Cow<'_, Tech> {
    if tech.stream_map.is_empty() {
        return Cow::Borrowed(tech);
    }
    let mut t = tech.clone();
    for ((l, (g, d)), text) in t.layers.iter_mut().zip(export_pairs(tech)).zip(export_text_pairs(tech)) {
        l.gds_layer = g;
        l.gds_datatype = d;
        l.text_stream = (text != (g, d)).then_some(text);
    }
    Cow::Owned(t)
}

/// Import overrides of the map for shapes (label rows excluded): stream pair to technology
/// layer index, or `None` to drop it. Rows naming a layer the technology lacks are ignored
/// ([`problems`] reports them).
pub fn import_pairs(tech: &Tech) -> HashMap<(u16, u16), Option<usize>> {
    let mut out = HashMap::new();
    for e in tech.stream_map.iter().filter(|e| !e.text) {
        let to = if e.layer.trim().is_empty() {
            None
        } else {
            match tech.layer_index(&e.layer) {
                Some(k) => Some(k),
                None => continue,
            }
        };
        out.entry((e.stream_layer, e.datatype)).or_insert(to);
    }
    out
}

/// Import overrides of the map's label rows: stream pair of texts to technology layer index.
pub fn import_text_pairs(tech: &Tech) -> HashMap<(u16, u16), usize> {
    let mut out = HashMap::new();
    for e in tech.stream_map.iter().filter(|e| e.text) {
        if let Some(k) = tech.layer_index(&e.layer) {
            out.entry((e.stream_layer, e.datatype)).or_insert(k);
        }
    }
    out
}

/// Inconsistencies of the map: unknown layers, pairs mapped twice, and pairs two layers export.
pub fn problems(tech: &Tech) -> Vec<String> {
    let mut out = vec![];
    let mut seen: HashMap<(u16, u16, bool), &str> = HashMap::new();
    for e in &tech.stream_map {
        let name = e.layer.trim();
        if !name.is_empty() && tech.layer_index(name).is_none() {
            out.push(format!("'{name}' is not a layer of the technology"));
        }
        let pair = (e.stream_layer, e.datatype, e.text);
        match seen.get(&pair) {
            Some(prev) if !prev.eq_ignore_ascii_case(name) => out.push(format!(
                "{}/{}{} is mapped to both '{}' and '{}'; import uses the first",
                pair.0,
                pair.1,
                if pair.2 { " (labels)" } else { "" },
                if prev.is_empty() { "(drop)" } else { prev },
                if name.is_empty() { "(drop)" } else { name }
            )),
            Some(_) => {}
            None => {
                seen.insert(pair, name);
            }
        }
    }
    let pairs = export_pairs(tech);
    for (i, p) in pairs.iter().enumerate() {
        if let Some(j) = pairs[..i].iter().position(|q| q == p) {
            out.push(format!("'{}' and '{}' are both written as {}/{}", tech.layers[j].name, tech.layers[i].name, p.0, p.1));
        }
    }
    out
}

/// One row of a layer map file: `layerName purpose streamLayer streamDatatype`.
struct Row<'a> {
    line: usize,
    name: &'a str,
    purpose: &'a str,
    layer: u16,
    datatype: u16,
}

/// The rows of a layer map file, and warnings for the lines that are not rows.
fn rows(text: &str) -> (Vec<Row<'_>>, Vec<String>) {
    let (mut rows, mut warnings) = (vec![], vec![]);
    for (n, raw) in text.lines().enumerate() {
        let line = raw.split(['#', ';']).next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let t: Vec<&str> = line.split_whitespace().collect();
        let (name, purpose, nums) = match t.as_slice() {
            [name, purpose, l, d, ..] => (*name, *purpose, (*l, *d)),
            [name, l, d] => (*name, "drawing", (*l, *d)),
            _ => {
                warnings.push(format!("line {}: expected 'layer purpose streamLayer datatype'", n + 1));
                continue;
            }
        };
        let (Ok(layer), Ok(datatype)) = (nums.0.parse::<u16>(), nums.1.parse::<u16>()) else {
            warnings.push(format!("line {}: invalid stream numbers '{} {}'", n + 1, nums.0, nums.1));
            continue;
        };
        rows.push(Row { line: n + 1, name, purpose, layer, datatype });
    }
    (rows, warnings)
}

/// Parse a layer map file against `tech`. Returns the rows and warnings for lines skipped.
pub fn parse(text: &str, tech: &Tech) -> (Vec<StreamMapEntry>, Vec<String>) {
    let (rows, mut warnings) = rows(text);
    let mut out = vec![];
    for r in rows {
        match resolve(tech, r.name, r.purpose) {
            Some((layer, text)) => out.push(StreamMapEntry { layer, stream_layer: r.layer, datatype: r.datatype, text }),
            None => warnings.push(format!("line {}: no technology layer for {} {}", r.line, r.name, r.purpose)),
        }
    }
    (out, warnings)
}

/// What [`apply_layer_map`] did to a technology.
#[derive(Debug, Default)]
pub struct LayerMapApply {
    /// Map rows read from the file.
    pub rows: usize,
    /// Layers the technology lacked, added for the file's rows.
    pub added_layers: Vec<String>,
    pub warnings: Vec<String>,
}

/// Apply a Virtuoso layer map file to `tech`, as XStream In does with a layer map: every row
/// (`RX drawing 2 0`) maps its stream pair onto the named layer and purpose (see the module
/// documentation). A layer the technology lacks is added with the row's stream pair: `RX` for
/// `drawing`, `label` and `text` rows, `RX.exclude` for purpose `exclude` and so on. The
/// file's rows go before the technology's own map rows and replace those for the same pairs,
/// so they decide both import and export.
pub fn apply_layer_map(tech: &mut Tech, text: &str) -> LayerMapApply {
    let (rows, warnings) = rows(text);
    let mut out = LayerMapApply { rows: rows.len(), warnings, ..Default::default() };
    // Drawing rows first, so a label row without its own layer finds the drawn one.
    let mut order: Vec<&Row> = rows.iter().collect();
    order.sort_by_key(|r| !is_drawing(r.purpose));
    let mut resolved = HashMap::new();
    for r in order {
        let (layer, text) = resolve(tech, r.name, r.purpose).unwrap_or_else(|| {
            let mut def = LayerDef::generated(r.layer, r.datatype, tech.layers.len());
            let text = is_text(r.purpose);
            def.name = if is_drawing(r.purpose) || text { r.name.to_string() } else { format!("{}.{}", r.name, r.purpose) };
            out.added_layers.push(def.name.clone());
            let name = def.name.clone();
            tech.layers.push(def);
            (name, text)
        });
        resolved.insert(r.line, StreamMapEntry { layer, stream_layer: r.layer, datatype: r.datatype, text });
    }
    let mut entries: Vec<StreamMapEntry> = rows.iter().filter_map(|r| resolved.remove(&r.line)).collect();
    let keys: HashSet<(u16, u16, bool)> = entries.iter().map(|e| (e.stream_layer, e.datatype, e.text)).collect();
    entries.extend(tech.stream_map.drain(..).filter(|e| !keys.contains(&(e.stream_layer, e.datatype, e.text))));
    tech.stream_map = entries;
    out
}

fn is_drawing(purpose: &str) -> bool {
    purpose.eq_ignore_ascii_case("drawing") || purpose.eq_ignore_ascii_case("drw")
}

/// Purposes that carry the text labels of a layer.
fn is_text(purpose: &str) -> bool {
    purpose.eq_ignore_ascii_case("label") || purpose.eq_ignore_ascii_case("text")
}

/// The technology layer of `name purpose`, and whether the row carries only its labels.
fn resolve(tech: &Tech, name: &str, purpose: &str) -> Option<(String, bool)> {
    let find = |c: &str| tech.layer_index(c).map(|k| tech.layers[k].name.clone());
    if is_drawing(purpose) {
        return find(name).map(|l| (l, false));
    }
    let own = [format!("{name}.{purpose}"), format!("{name}{purpose}"), format!("{name}_{purpose}")];
    if let Some(l) = own.iter().find_map(|c| find(c)) {
        return Some((l, false));
    }
    if is_text(purpose) {
        return find(name).map(|l| (l, true));
    }
    None
}

/// The map as a layer map file; dropped pairs are written as comments. A layer named
/// `name.purpose` is written as `name purpose`, label rows with purpose `label`.
pub fn write(tech: &Tech) -> String {
    let mut s = format!("# Stream layer map of {}\n# layer purpose streamLayer streamDatatype\n", tech.name);
    let rows: Vec<StreamMapEntry> = if tech.stream_map.is_empty() {
        tech.layers
            .iter()
            .map(|l| StreamMapEntry { layer: l.name.clone(), stream_layer: l.gds_layer, datatype: l.gds_datatype, text: false })
            .collect()
    } else {
        tech.stream_map.clone()
    };
    for e in rows {
        if e.layer.trim().is_empty() {
            s += &format!("# dropped {} {}\n", e.stream_layer, e.datatype);
            continue;
        }
        let (name, purpose) = match e.layer.split_once('.') {
            _ if e.text => (e.layer.as_str(), "label"),
            Some((n, p)) if !n.is_empty() && !p.is_empty() && !p.contains(char::is_whitespace) => (n, p),
            _ => (e.layer.as_str(), "drawing"),
        };
        s += &format!("{name} {purpose} {} {}\n", e.stream_layer, e.datatype);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gds::{ReadOptions, read_gds_with, write_gds};
    use crate::layout::{Geometry, Layout};
    use crate::project::CellRef;

    fn entry(layer: &str, l: u16, d: u16) -> StreamMapEntry {
        StreamMapEntry { layer: layer.into(), stream_layer: l, datatype: d, text: false }
    }

    fn label(layer: &str, l: u16, d: u16) -> StreamMapEntry {
        StreamMapEntry { text: true, ..entry(layer, l, d) }
    }

    #[test]
    fn parse_resolves_purposes() {
        let mut tech = Tech::generic();
        let mut pin = tech.layers[tech.layer_index("metal1").unwrap()].clone();
        pin.name = "metal1pin".into();
        tech.layers.push(pin);
        let text = "# map\nmetal1 drawing 31 0\nmetal1 pin 31 2 ; pins\npoly drw 13 0\nbogus drawing 1 1\nmetal2 x y z\n";
        let (rows, warnings) = parse(text, &tech);
        assert_eq!(rows, [entry("metal1", 31, 0), entry("metal1pin", 31, 2), entry("poly", 13, 0)]);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
    }

    #[test]
    fn export_and_import_use_the_map() {
        let mut tech = Tech::generic();
        let m1 = tech.layer_index("metal1").unwrap();
        let poly = tech.layer_index("poly").unwrap();
        tech.stream_map = vec![entry("metal1", 31, 0), entry("metal1", 31, 5), entry("", 63, 0)];
        assert_eq!(export_pairs(&tech)[m1], (31, 0));
        let mut l = Layout::default();
        l.add(m1, Geometry::Rect(crate::IRect::new(0, 0, 100, 100)));
        l.add(poly, Geometry::Rect(crate::IRect::new(0, 0, 50, 50)));
        let top = CellRef::new("lib", "top");
        let bytes = write_gds("lib", &[(&top, &l)], &export_tech(&tech));
        let plain = read_gds_with(&bytes, &Tech::generic(), "lib", ReadOptions::default()).unwrap();
        assert_eq!(plain.skipped_unknown_layers, 1, "metal1 went out as 31/0");
        let opts = ReadOptions { use_stream_map: true, ..Default::default() };
        let back = read_gds_with(&bytes, &tech, "lib", opts).unwrap();
        let layers: Vec<usize> = back.cells[0].1.shapes.iter().map(|s| s.layer).collect();
        assert_eq!(layers, [m1, poly]);

        let mut extra = Layout::default();
        let mut wtech = Tech::generic();
        wtech.layers[m1].gds_layer = 31;
        wtech.layers[m1].gds_datatype = 5;
        wtech.layers[poly].gds_layer = 63;
        extra.add(m1, Geometry::Rect(crate::IRect::new(0, 0, 100, 100)));
        extra.add(poly, Geometry::Rect(crate::IRect::new(0, 0, 50, 50)));
        let bytes = write_gds("lib", &[(&top, &extra)], &wtech);
        let opts = ReadOptions { use_stream_map: true, add_unknown_layers: true };
        let back = read_gds_with(&bytes, &tech, "lib", opts).unwrap();
        let layers: Vec<usize> = back.cells[0].1.shapes.iter().map(|s| s.layer).collect();
        assert_eq!(layers, [m1], "31/5 reads onto metal1, 63/0 is dropped");
        assert_eq!(back.skipped_unknown_layers, 1);
        assert!(back.new_layers.is_empty());
    }

    #[test]
    fn layer_map_file_adds_layers_and_maps_import() {
        let mut tech = Tech::generic();
        let m1 = tech.layer_index("metal1").unwrap();
        tech.stream_map = vec![entry("poly", 120, 0), entry("poly", 13, 0)];
        let text = "# Virtuoso layer map\nRX label 120 10\nRX drawing 120 0\nRX exclude 120 5\nRX pin 120 2\n\
                    metal1 drawing 31 0\nmetal1 label 31 10\nbad line\n";
        let r = apply_layer_map(&mut tech, text);
        assert_eq!(r.rows, 6);
        assert_eq!(r.added_layers, ["RX", "RX.exclude", "RX.pin"]);
        assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
        assert_eq!(
            tech.stream_map,
            [
                label("RX", 120, 10),
                entry("RX", 120, 0),
                entry("RX.exclude", 120, 5),
                entry("RX.pin", 120, 2),
                entry("metal1", 31, 0),
                label("metal1", 31, 10),
                entry("poly", 13, 0)
            ]
        );
        assert!(problems(&tech).is_empty(), "{:?}", problems(&tech));

        // Shapes on 120/0, a text on 120/10 and an exclude marker on 120/5.
        let rx = tech.layer_index("RX").unwrap();
        let ex = tech.layer_index("RX.exclude").unwrap();
        let mut wtech = Tech::generic();
        let poly = wtech.layer_index("poly").unwrap();
        let m2 = wtech.layer_index("metal2").unwrap();
        let pairs = [(m1, (120, 0)), (poly, (120, 10)), (m2, (120, 5))];
        for (k, (g, d)) in pairs {
            (wtech.layers[k].gds_layer, wtech.layers[k].gds_datatype) = (g, d);
        }
        let mut l = Layout::default();
        l.add(m1, Geometry::Rect(crate::IRect::new(0, 0, 100, 100)));
        l.add(poly, Geometry::Label { text: "d".into(), at: (10, 10), size: 100 });
        l.add(m2, Geometry::Rect(crate::IRect::new(0, 0, 500, 500)));
        let top = CellRef::new("lib", "top");
        let bytes = write_gds("lib", &[(&top, &l)], &wtech);
        let opts = ReadOptions { use_stream_map: true, add_unknown_layers: true };
        let back = read_gds_with(&bytes, &tech, "lib", opts).unwrap();
        let layers: Vec<usize> = back.cells[0].1.shapes.iter().map(|s| s.layer).collect();
        assert_eq!(layers, [rx, rx, ex], "the label lands on RX itself");
        assert!(back.new_layers.is_empty());

        // Export writes RX shapes as 120/0 and RX labels as 120/10, for GDSII and OASIS.
        assert_eq!(export_pairs(&tech)[rx], (120, 0));
        assert_eq!(export_text_pairs(&tech)[rx], (120, 10));
        assert_eq!(export_text_pairs(&tech)[ex], (120, 5));
        let shapes = back.cells[0].1.clone();
        let out = export_tech(&tech);
        let gds = write_gds("lib", &[(&top, &shapes)], &out);
        let plain = |data: &[u8]| {
            let mut t = Tech::generic();
            t.layers.clear();
            let opts = ReadOptions { add_unknown_layers: true, use_stream_map: false };
            let imp = crate::gds::read_stream_with(data, &t, "lib", opts).unwrap();
            let mut v: Vec<(u16, u16)> = imp.new_layers.iter().map(|d| (d.gds_layer, d.gds_datatype)).collect();
            v.sort();
            v
        };
        assert_eq!(plain(&gds), [(120, 0), (120, 5), (120, 10)]);
        let src: HashMap<CellRef, Layout> = HashMap::from([(top.clone(), shapes)]);
        let oas = crate::oasis::write_oasis_hier(&src, std::slice::from_ref(&top), &out);
        assert_eq!(plain(&oas), [(120, 0), (120, 5), (120, 10)]);
        let back = crate::gds::read_stream_with(&oas, &tech, "lib", opts).unwrap();
        assert_eq!(back.cells[0].1.shapes.iter().map(|s| s.layer).collect::<Vec<_>>(), [rx, rx, ex]);

        // The map file round-trips with its purposes.
        let written = write(&tech);
        assert!(written.contains("RX label 120 10\nRX drawing 120 0\nRX exclude 120 5\n"), "{written}");
        assert_eq!(parse(&written, &tech).0, tech.stream_map);

        let again = apply_layer_map(&mut tech, text);
        assert!(again.added_layers.is_empty(), "a second apply reuses the layers");
        assert_eq!(tech.stream_map.len(), 7);
    }

    #[test]
    fn problems_found() {
        let mut tech = Tech::generic();
        tech.stream_map = vec![entry("metal1", 9, 0), entry("nope", 1, 1), entry("poly", 1, 1)];
        let p = problems(&tech);
        assert!(p.iter().any(|m| m.contains("'nope'")), "{p:?}");
        assert!(p.iter().any(|m| m.contains("both written as 9/0")), "{p:?}");
        assert!(p.iter().any(|m| m.contains("mapped to both")), "{p:?}");
    }

    #[test]
    fn write_round_trips() {
        let mut tech = Tech::generic();
        let text = write(&tech);
        let (rows, warnings) = parse(&text, &tech);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(rows.len(), tech.layers.len());
        tech.stream_map = rows;
        assert!(problems(&tech).is_empty());
        assert_eq!(export_pairs(&tech), tech.layers.iter().map(|l| (l.gds_layer, l.gds_datatype)).collect::<Vec<_>>());
    }
}
