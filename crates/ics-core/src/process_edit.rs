//! Edits of a process flow file (`ics_core::process`) that keep its comments and layout: the
//! flow settings and the steps, one at a time (the step editor of the Process Emulation window).

use crate::process::{Doping, Mask, ProcessFlow, Step};
use toml_edit::{Array, DocumentMut, Item, Table, Value};

/// The `op` names of the steps, in menu order.
pub const OPS: [&str; 7] = ["deposit", "etch", "implant", "anneal", "oxidize", "planarize", "strip"];

/// A new step of kind `op` (one of [`OPS`]) with typical values.
pub fn new_step(op: &str) -> Option<Step> {
    let name = String::new();
    Some(match op {
        "deposit" => Step::Deposit { name, material: "sio2".into(), thickness: 0.1, conformal: false },
        "etch" => Step::Etch { name, mask: Mask::default(), materials: vec!["sio2".into()], depth: None, isotropic: false },
        "implant" => Step::Implant {
            name,
            mask: Mask::default(),
            material: None,
            into: vec![],
            depth: None,
            species: Some("boron".into()),
            dose: Some(1e13),
            energy: Some(20.0),
        },
        "anneal" => Step::Anneal { name, temperature: 1000.0, time: 10.0 },
        "oxidize" => Step::Oxidize { name, temperature: 1000.0, time: 10.0, wet: false, material: "sio2".into() },
        "planarize" => Step::Planarize { name, height: Some(0.0), stop: None },
        "strip" => Step::Strip { name, material: "sio2".into() },
        _ => return None,
    })
}

/// The `op` name of `step`.
pub fn op(step: &Step) -> &'static str {
    match step {
        Step::Deposit { .. } => "deposit",
        Step::Etch { .. } => "etch",
        Step::Implant { .. } => "implant",
        Step::Anneal { .. } => "anneal",
        Step::Oxidize { .. } => "oxidize",
        Step::Planarize { .. } => "planarize",
        Step::Strip { .. } => "strip",
    }
}

/// A float as TOML writes it by hand: `0.35`, `1.0`, `5e15`.
fn float(v: f64) -> Value {
    let a = v.abs();
    let s = if a != 0.0 && !(1e-3..1e5).contains(&a) {
        format!("{v:e}")
    } else {
        let s = format!("{v}");
        if s.contains(['.', 'e', 'i', 'N']) { s } else { format!("{s}.0") }
    };
    s.parse().unwrap_or_else(|_| Value::from(v))
}

fn strings(v: &[String]) -> Value {
    Value::Array(v.iter().map(|s| s.as_str()).collect::<Array>())
}

fn mask_fields(m: &Mask, f: &mut Vec<(&'static str, Value)>) {
    if let Some(e) = &m.mask {
        f.push(("mask", e.as_str().into()));
    }
    if let Some(e) = &m.keep {
        f.push(("keep", e.as_str().into()));
    }
}

/// The keys of `step` as written in a flow file, defaults left out.
fn step_fields(step: &Step) -> Vec<(&'static str, Value)> {
    let mut f: Vec<(&'static str, Value)> = vec![];
    let name = match step {
        Step::Deposit { name, .. }
        | Step::Etch { name, .. }
        | Step::Implant { name, .. }
        | Step::Anneal { name, .. }
        | Step::Oxidize { name, .. }
        | Step::Planarize { name, .. }
        | Step::Strip { name, .. } => name,
    };
    if !name.is_empty() {
        f.push(("name", name.as_str().into()));
    }
    f.push(("op", op(step).into()));
    match step {
        Step::Deposit { material, thickness, conformal, .. } => {
            f.push(("material", material.as_str().into()));
            f.push(("thickness", float(*thickness)));
            if *conformal {
                f.push(("conformal", true.into()));
            }
        }
        Step::Etch { mask, materials, depth, isotropic, .. } => {
            mask_fields(mask, &mut f);
            f.push(("materials", strings(materials)));
            if let Some(d) = depth {
                f.push(("depth", float(*d)));
            }
            if *isotropic {
                f.push(("isotropic", true.into()));
            }
        }
        Step::Implant { mask, material, into, depth, species, dose, energy, .. } => {
            mask_fields(mask, &mut f);
            if let Some(s) = species {
                f.push(("species", s.as_str().into()));
            }
            if let Some(d) = dose {
                f.push(("dose", float(*d)));
            }
            if let Some(e) = energy {
                f.push(("energy", float(*e)));
            }
            if let Some(m) = material {
                f.push(("material", m.as_str().into()));
            }
            if !into.is_empty() {
                f.push(("into", strings(into)));
            }
            if let Some(d) = depth {
                f.push(("depth", float(*d)));
            }
        }
        Step::Anneal { temperature, time, .. } => {
            f.push(("temperature", float(*temperature)));
            f.push(("time", float(*time)));
        }
        Step::Oxidize { temperature, time, wet, material, .. } => {
            f.push(("temperature", float(*temperature)));
            f.push(("time", float(*time)));
            if *wet {
                f.push(("wet", true.into()));
            }
            if material != "sio2" {
                f.push(("material", material.as_str().into()));
            }
        }
        Step::Planarize { height, stop, .. } => {
            if let Some(h) = height {
                f.push(("height", float(*h)));
            }
            if let Some(s) = stop {
                f.push(("stop", s.as_str().into()));
            }
        }
        Step::Strip { material, .. } => f.push(("material", material.as_str().into())),
    }
    f
}

/// Whether two values mean the same (`1` and `1.0` do).
fn same(a: &Value, b: &Value) -> bool {
    let num = |v: &Value| v.as_float().or_else(|| v.as_integer().map(|i| i as f64));
    match (a, b) {
        (Value::String(x), Value::String(y)) => x.value() == y.value(),
        (Value::Boolean(x), Value::Boolean(y)) => x.value() == y.value(),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| same(p, q)),
        (Value::InlineTable(x), Value::InlineTable(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => matches!((num(a), num(b)), (Some(x), Some(y)) if x == y),
    }
}

/// Set the keys of `t` to `fields`, leaving the ones that already hold the same value (and
/// their formatting) alone; keys of `owned` not in `fields` are removed.
fn set_fields(t: &mut Table, fields: &[(&'static str, Value)], owned: &[&str]) {
    for k in owned {
        if !fields.iter().any(|(f, _)| f == k) {
            t.remove(k);
        }
    }
    for (k, v) in fields {
        if t.get(k).and_then(Item::as_value).is_some_and(|old| same(old, v)) {
            continue;
        }
        match t.get_mut(k).and_then(Item::as_value_mut) {
            // Keep the decoration (comments) of the key.
            Some(old) => {
                let decor = old.decor().clone();
                *old = v.clone();
                *old.decor_mut() = decor;
            }
            None => {
                t.insert(k, Item::Value(v.clone()));
            }
        }
    }
}

const STEP_KEYS: [&str; 19] = [
    "name",
    "op",
    "material",
    "thickness",
    "conformal",
    "mask",
    "keep",
    "materials",
    "depth",
    "isotropic",
    "into",
    "species",
    "dose",
    "energy",
    "temperature",
    "time",
    "wet",
    "height",
    "stop",
];

fn parse(text: &str) -> Result<DocumentMut, String> {
    text.parse::<DocumentMut>().map_err(|e| e.to_string())
}

fn render(doc: &DocumentMut, original: &str) -> String {
    let edited = doc.to_string();
    // toml_edit emits LF line endings, including for preserved comments and spacing.
    // Follow the first line ending in the input when writing the edited document.
    if original.split_once('\n').is_some_and(|(first_line, _)| first_line.ends_with('\r')) {
        edited.replace('\n', "\r\n")
    } else {
        edited
    }
}

/// The steps of `doc`, created when missing.
fn steps(doc: &mut DocumentMut) -> Result<&mut toml_edit::ArrayOfTables, String> {
    if doc.get("step").is_none() {
        doc.insert("step", Item::ArrayOfTables(Default::default()));
    }
    doc["step"].as_array_of_tables_mut().ok_or_else(|| "`step` is not an array of tables".to_string())
}

/// Give every step table one document position so that they print in array order.
fn renumber(doc: &mut DocumentMut) {
    let Ok(a) = steps(doc) else { return };
    let pos = a.iter().filter_map(Table::position).min();
    for t in a.iter_mut() {
        t.set_position(pos);
    }
}

fn step_table(step: &Step) -> Table {
    let mut t = Table::new();
    set_fields(&mut t, &step_fields(step), &[]);
    t.decor_mut().set_prefix("\n");
    t
}

/// Replace step `k` (from 0) of the flow file `text` with `step`.
pub fn set_step(text: &str, k: usize, step: &Step) -> Result<String, String> {
    let mut doc = parse(text)?;
    let a = steps(&mut doc)?;
    let t = a.get_mut(k).ok_or_else(|| format!("no step {}", k + 1))?;
    set_fields(t, &step_fields(step), &STEP_KEYS);
    Ok(render(&doc, text))
}

/// Insert `step` as step `k` (from 0; the number of steps appends it).
pub fn insert_step(text: &str, k: usize, step: &Step) -> Result<String, String> {
    let mut doc = parse(text)?;
    let a = steps(&mut doc)?;
    if k > a.len() {
        return Err(format!("no step {}", k + 1));
    }
    a.insert(k, step_table(step));
    renumber(&mut doc);
    Ok(render(&doc, text))
}

/// Remove step `k` (from 0) with the comments above it.
pub fn remove_step(text: &str, k: usize) -> Result<String, String> {
    let mut doc = parse(text)?;
    let a = steps(&mut doc)?;
    if k >= a.len() {
        return Err(format!("no step {}", k + 1));
    }
    a.remove(k);
    renumber(&mut doc);
    Ok(render(&doc, text))
}

/// Move step `from` to index `to` (from 0), with the comments above it.
pub fn move_step(text: &str, from: usize, to: usize) -> Result<String, String> {
    let mut doc = parse(text)?;
    let a = steps(&mut doc)?;
    if from >= a.len() || to >= a.len() {
        return Err("no such step".into());
    }
    let t = a.remove(from);
    a.insert(to, t);
    renumber(&mut doc);
    Ok(render(&doc, text))
}

/// Set the settings of the flow file `text` (name, substrate and its doping, depth,
/// resolution, semiconductors) to those of `flow`; the steps and colours are left alone.
pub fn set_header(text: &str, flow: &ProcessFlow) -> Result<String, String> {
    let mut doc = parse(text)?;
    let mut f: Vec<(&'static str, Value)> = vec![];
    if !flow.name.is_empty() {
        f.push(("name", flow.name.as_str().into()));
    }
    f.push(("substrate", flow.substrate.as_str().into()));
    f.push(("substrate_depth", float(flow.substrate_depth)));
    f.push(("resolution", float(flow.resolution)));
    if let Some(Doping { species, concentration }) = &flow.substrate_doping {
        let mut t = toml_edit::InlineTable::new();
        t.insert("species", species.as_str().into());
        t.insert("concentration", float(*concentration));
        t.fmt();
        f.push(("substrate_doping", Value::InlineTable(t)));
    }
    if flow.semiconductors != ["si", "poly"] {
        f.push(("semiconductors", strings(&flow.semiconductors)));
    }
    let root = doc.as_table_mut();
    // New keys go before the tables: toml_edit prints root values first anyway.
    set_fields(root, &f, &["name", "substrate", "substrate_depth", "resolution", "substrate_doping", "semiconductors"]);
    Ok(render(&doc, text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::GENERIC180;

    fn flow(text: &str) -> ProcessFlow {
        ProcessFlow::from_toml(text).unwrap()
    }

    #[test]
    fn edits_keep_comments_and_round_trip() {
        let f0 = flow(GENERIC180);
        // Setting a step to itself changes nothing.
        assert_eq!(set_step(GENERIC180, 3, &f0.steps[3]).unwrap(), GENERIC180);
        assert_eq!(set_header(GENERIC180, &f0).unwrap(), GENERIC180);
        // A changed value is rewritten, everything else kept.
        let Step::Etch { mask, materials, depth: _, isotropic, name } = f0.steps[0].clone() else { panic!() };
        let etch = Step::Etch { name, mask, materials, depth: Some(0.4), isotropic };
        let t = set_step(GENERIC180, 0, &etch).unwrap();
        assert!(t.contains("depth = 0.4") && t.contains("# Shallow trench isolation"), "{t}");
        assert_eq!(flow(&t).steps[0], etch);
        assert_eq!(flow(&t).steps[1..], f0.steps[1..]);
        // Insert, move, remove.
        let n = f0.steps.len();
        let ann = new_step("anneal").unwrap();
        let t = insert_step(GENERIC180, 2, &ann).unwrap();
        let f = flow(&t);
        assert_eq!(f.steps.len(), n + 1);
        assert_eq!(f.steps[2], ann);
        let t = move_step(&t, 2, n).unwrap();
        let f = flow(&t);
        assert_eq!(f.steps[n], ann);
        assert_eq!(f.steps[..n], f0.steps[..]);
        let t = move_step(&t, n, 0).unwrap();
        assert_eq!(flow(&t).steps[0], ann);
        let t = remove_step(&t, 0).unwrap();
        assert_eq!(flow(&t), f0);
        // Every kind of step round-trips.
        for op in OPS {
            let s = new_step(op).unwrap();
            let t = insert_step(GENERIC180, n, &s).unwrap();
            assert_eq!(flow(&t).steps[n], s, "{op}");
        }
        // Settings.
        let mut g = f0.clone();
        g.resolution = 0.01;
        g.substrate_doping = Some(Doping { species: "phosphorus".into(), concentration: 2e15 });
        g.semiconductors = vec!["si".into()];
        let t = set_header(GENERIC180, &g).unwrap();
        assert_eq!(flow(&t), g);
        assert!(t.contains("concentration = 2e15"), "{t}");
    }

    #[test]
    fn edits_preserve_crlf_line_endings() {
        let lf = GENERIC180.replace("\r\n", "\n");
        let crlf = lf.replace('\n', "\r\n");
        let mut settings = flow(&lf);
        let unchanged_step = &settings.steps[3];
        assert_eq!(set_step(&crlf, 3, unchanged_step).unwrap(), crlf);
        assert_eq!(set_header(&crlf, &settings).unwrap(), crlf);

        settings.resolution = 0.01;
        let anneal = new_step("anneal").unwrap();
        for (lf_edit, crlf_edit) in [
            (set_header(&lf, &settings), set_header(&crlf, &settings)),
            (set_step(&lf, 0, &anneal), set_step(&crlf, 0, &anneal)),
            (insert_step(&lf, 0, &anneal), insert_step(&crlf, 0, &anneal)),
            (move_step(&lf, 0, 2), move_step(&crlf, 0, 2)),
            (remove_step(&lf, 0), remove_step(&crlf, 0)),
        ] {
            let lf_edit = lf_edit.unwrap();
            let crlf_edit = crlf_edit.unwrap();
            assert_eq!(crlf_edit, lf_edit.replace('\n', "\r\n"));
            assert_eq!(flow(&crlf_edit), flow(&lf_edit));
        }
    }

    #[test]
    fn floats_read_like_hand_written() {
        assert_eq!(float(0.35).to_string(), "0.35");
        assert_eq!(float(1.0).to_string(), "1.0");
        assert_eq!(float(5e15).to_string(), "5e15");
        assert_eq!(float(1000.0).to_string(), "1000.0");
    }
}
