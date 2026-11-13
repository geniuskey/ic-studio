//! Stream layer map (Virtuoso XStream layer map file): which GDSII / OASIS (layer, datatype)
//! pairs technology layers are read from and written as on import and export.
//!
//! A map file has one `layerName purpose streamLayer streamDatatype` row per line; `#` and `;`
//! start comments. A purpose other than `drawing` names the layer `layerName.purpose`,
//! `layerNamepurpose` or `layerName_purpose` when the technology has one of those.

use std::borrow::Cow;
use std::collections::HashMap;

use crate::tech::{StreamMapEntry, Tech};

/// The stream pair every technology layer is written as: its first map row, else its own pair.
pub fn export_pairs(tech: &Tech) -> Vec<(u16, u16)> {
    tech.layers
        .iter()
        .map(|l| {
            tech.stream_map
                .iter()
                .find(|e| e.layer.eq_ignore_ascii_case(&l.name))
                .map_or((l.gds_layer, l.gds_datatype), |e| (e.stream_layer, e.datatype))
        })
        .collect()
}

/// `tech` with its layers' stream pairs replaced by the map's, for writing a stream file.
pub fn export_tech(tech: &Tech) -> Cow<'_, Tech> {
    if tech.stream_map.is_empty() {
        return Cow::Borrowed(tech);
    }
    let mut t = tech.clone();
    for (l, (g, d)) in t.layers.iter_mut().zip(export_pairs(tech)) {
        l.gds_layer = g;
        l.gds_datatype = d;
    }
    Cow::Owned(t)
}

/// Import overrides of the map: stream pair to technology layer index, or `None` to drop it.
/// Rows naming a layer the technology lacks are ignored ([`problems`] reports them).
pub fn import_pairs(tech: &Tech) -> HashMap<(u16, u16), Option<usize>> {
    let mut out = HashMap::new();
    for e in &tech.stream_map {
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

/// Inconsistencies of the map: unknown layers, pairs mapped twice, and pairs two layers export.
pub fn problems(tech: &Tech) -> Vec<String> {
    let mut out = vec![];
    let mut seen: HashMap<(u16, u16), &str> = HashMap::new();
    for e in &tech.stream_map {
        let name = e.layer.trim();
        if !name.is_empty() && tech.layer_index(name).is_none() {
            out.push(format!("'{name}' is not a layer of the technology"));
        }
        let pair = (e.stream_layer, e.datatype);
        match seen.get(&pair) {
            Some(prev) if !prev.eq_ignore_ascii_case(name) => out.push(format!(
                "{}/{} is mapped to both '{}' and '{}'; import uses the first",
                pair.0,
                pair.1,
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

/// Parse a layer map file against `tech`. Returns the rows and warnings for lines skipped.
pub fn parse(text: &str, tech: &Tech) -> (Vec<StreamMapEntry>, Vec<String>) {
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
        let (Ok(l), Ok(d)) = (nums.0.parse::<u16>(), nums.1.parse::<u16>()) else {
            warnings.push(format!("line {}: invalid stream numbers '{} {}'", n + 1, nums.0, nums.1));
            continue;
        };
        match resolve(tech, name, purpose) {
            Some(layer) => rows.push(StreamMapEntry { layer, stream_layer: l, datatype: d }),
            None => warnings.push(format!("line {}: no technology layer for {name} {purpose}", n + 1)),
        }
    }
    (rows, warnings)
}

fn resolve(tech: &Tech, name: &str, purpose: &str) -> Option<String> {
    let p = purpose.to_ascii_lowercase();
    let mut candidates = vec![];
    if p != "drawing" {
        candidates.extend([format!("{name}.{purpose}"), format!("{name}{purpose}"), format!("{name}_{purpose}")]);
    }
    if matches!(p.as_str(), "drawing" | "drw") {
        candidates.push(name.to_string());
    }
    candidates.iter().find_map(|c| tech.layer_index(c)).map(|k| tech.layers[k].name.clone())
}

/// The map as a layer map file (purpose `drawing`; dropped pairs are written as comments).
pub fn write(tech: &Tech) -> String {
    let mut s = format!("# Stream layer map of {}\n# layer purpose streamLayer streamDatatype\n", tech.name);
    let rows: Vec<StreamMapEntry> = if tech.stream_map.is_empty() {
        tech.layers
            .iter()
            .map(|l| StreamMapEntry { layer: l.name.clone(), stream_layer: l.gds_layer, datatype: l.gds_datatype })
            .collect()
    } else {
        tech.stream_map.clone()
    };
    for e in rows {
        if e.layer.trim().is_empty() {
            s += &format!("# dropped {} {}\n", e.stream_layer, e.datatype);
        } else {
            s += &format!("{} drawing {} {}\n", e.layer, e.stream_layer, e.datatype);
        }
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
        StreamMapEntry { layer: layer.into(), stream_layer: l, datatype: d }
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
