//! SPICE netlist -> schematic: builds a drawable schematic from element lines, so agents and
//! scripts can create schematics without placing symbols by hand.
//!
//! Every pin gets a net label (or a ground symbol for node `0` / `gnd`), so no wires are needed.
//! Devices are laid out in rows: PMOS, NMOS, passives and diodes, sources.

use std::collections::BTreeMap;

use crate::geom::{Orient, Pt};
use crate::schematic::{DeviceKind, Schematic};
use crate::sim_setup::{Analysis, SimSetup};
use crate::tech::{MosType, Tech};
use crate::units::parse_spice;

/// Result of [`import`].
#[derive(Debug, Clone, Default)]
pub struct Imported {
    pub schematic: Schematic,
    /// Analyses from `.op` / `.dc` / `.ac` / `.tran` lines, in deck order.
    pub analyses: Vec<Analysis>,
    /// `.temp` value, if given.
    pub temp: Option<f64>,
    /// Lines kept verbatim for the simulation setup (`.model`, `.include`, `.param`, `.ic`, ...).
    pub extra: Vec<String>,
    pub warnings: Vec<String>,
}

impl Imported {
    /// Apply the deck's analyses, temperature and extra lines to `setup`. Analyses the deck lists
    /// are enabled (replacing their saved parameters); the others are disabled. Without any
    /// analysis line the setup's analyses are left alone.
    pub fn apply_to(&self, setup: &mut SimSetup) {
        if !self.analyses.is_empty() {
            set_analyses(setup, &self.analyses);
        }
        if let Some(t) = self.temp {
            setup.temp = t;
        }
        if !self.extra.is_empty() {
            setup.extra = self.extra.join("\n");
        }
    }
}

/// Enable exactly `analyses` in `setup` (one per kind; a later one of the same kind wins).
pub fn set_analyses(setup: &mut SimSetup, analyses: &[Analysis]) {
    for e in &mut setup.analyses {
        e.enabled = false;
    }
    for a in analyses {
        match setup.analyses.iter_mut().find(|e| e.analysis.kind_name() == a.kind_name()) {
            Some(e) => {
                e.analysis = a.clone();
                e.enabled = true;
            }
            None => setup.analyses.push(crate::sim_setup::AnalysisEntry { enabled: true, analysis: a.clone() }),
        }
    }
}

const COL: i32 = 10;
const ROW: i32 = 10;
/// MOS parameters copied to the schematic instance; the rest are reported.
const MOS_PARAMS: [&str; 8] = ["w", "l", "m", "ad", "as", "pd", "ps", "match"];

struct Dev {
    kind: DeviceKind,
    name: String,
    nets: Vec<String>,
    params: BTreeMap<String, String>,
}

/// Split a deck into logical lines: `+` continuations joined, comments dropped.
fn logical_lines(text: &str) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = vec![];
    for (i, raw) in text.lines().enumerate() {
        let line = raw.split(['$', ';']).next().unwrap_or("").trim();
        if line.is_empty() || line.starts_with('*') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('+')
            && let Some(last) = out.last_mut()
        {
            last.1.push(' ');
            last.1 += rest.trim();
            continue;
        }
        out.push((i + 1, line.to_string()));
    }
    out
}

/// Tokens of an element line; parenthesised groups (`PULSE(0 1 ...)`) stay one token and
/// `key = value` is joined to `key=value`.
fn tokens(line: &str) -> Vec<String> {
    let mut toks: Vec<String> = vec![];
    let mut cur = String::new();
    let mut depth = 0;
    for c in line.chars() {
        match c {
            '(' => {
                depth += 1;
                cur.push(c);
            }
            ')' => {
                depth -= 1;
                cur.push(c);
            }
            c if c.is_whitespace() && depth == 0 => {
                if !cur.is_empty() {
                    toks.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        toks.push(cur);
    }
    let mut out: Vec<String> = vec![];
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        if t == "=" && !out.is_empty() && i + 1 < toks.len() {
            let v = toks[i + 1].clone();
            out.last_mut().expect("checked").push('=');
            *out.last_mut().expect("checked") += &v;
            i += 2;
        } else if let Some(stripped) = t.strip_suffix('=').filter(|_| i + 1 < toks.len()) {
            out.push(format!("{stripped}={}", toks[i + 1]));
            i += 2;
        } else if t.starts_with('=') && !out.is_empty() {
            *out.last_mut().expect("checked") += t;
            i += 1;
        } else {
            out.push(t.clone());
            i += 1;
        }
    }
    out
}

fn is_ground(net: &str) -> bool {
    net == "0" || net.eq_ignore_ascii_case("gnd")
}

/// Parse `text` into a schematic. `tech` decides MOS polarity for PDK device names.
pub fn import(text: &str, tech: &Tech) -> Result<Imported, String> {
    let mut out = Imported::default();
    let mut model_types: BTreeMap<String, MosType> = BTreeMap::new();
    let lines = logical_lines(text);
    // .model cards first, so polarity is known for MOS lines that come before them.
    for (_, l) in &lines {
        let t = tokens(l);
        if t.first().is_some_and(|k| k.eq_ignore_ascii_case(".model")) && t.len() >= 3 {
            let ty = t[2].split('(').next().unwrap_or("").to_ascii_lowercase();
            match ty.as_str() {
                "nmos" => _ = model_types.insert(t[1].to_ascii_lowercase(), MosType::Nmos),
                "pmos" => _ = model_types.insert(t[1].to_ascii_lowercase(), MosType::Pmos),
                _ => {}
            }
        }
    }
    let mut devs: Vec<Dev> = vec![];
    let mut errors: Vec<String> = vec![];
    for (no, l) in &lines {
        let t = tokens(l);
        let head = t[0].to_ascii_lowercase();
        let err = |m: String| format!("line {no}: {m}");
        if let Some(cmd) = head.strip_prefix('.') {
            match dot_line(cmd, &t, l, &mut out) {
                Ok(()) => {}
                Err(m) => errors.push(err(m)),
            }
            continue;
        }
        match element(&t, tech, &model_types, &mut out.warnings) {
            Ok(Some(d)) => devs.push(d),
            Ok(None) => {}
            Err(m) => errors.push(err(m)),
        }
    }
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    if devs.is_empty() {
        return Err("no elements found (expected lines such as 'M1 out in 0 0 nmos w=1u l=180n')".into());
    }
    let mut seen = std::collections::HashSet::new();
    for d in &devs {
        if !seen.insert(d.name.to_ascii_lowercase()) {
            return Err(format!("duplicate element name '{}'", d.name));
        }
    }
    for d in &mut devs {
        for k in ["w", "l"] {
            if let Some(v) = d.params.get(k).cloned()
                && parse_spice(&v).is_some_and(|x| x >= 1e-3)
            {
                out.warnings.push(format!("{}: {k}={v} read as {v} µm (SPICE lengths are in metres; write {v}u)", d.name));
                d.params.insert(k.into(), format!("{v}u"));
            }
        }
    }
    out.schematic = place(&devs);
    Ok(out)
}

fn dot_line(cmd: &str, t: &[String], line: &str, out: &mut Imported) -> Result<(), String> {
    let arg = |i: usize| t.get(i).cloned().ok_or_else(|| format!(".{cmd}: missing argument {i}"));
    let analysis = match cmd {
        "op" => Analysis::Op,
        "dc" => Analysis::Dc { source: arg(1)?, start: arg(2)?, stop: arg(3)?, step: arg(4)? },
        "ac" => {
            if !arg(1)?.eq_ignore_ascii_case("dec") {
                return Err(".ac: only 'dec' sweeps are supported (.ac dec <points> <fstart> <fstop>)".into());
            }
            let n = arg(2)?;
            let points_per_decade = n.parse().map_err(|_| format!(".ac: points per decade '{n}' is not an integer"))?;
            Analysis::Ac { points_per_decade, fstart: arg(3)?, fstop: arg(4)? }
        }
        "tran" => Analysis::Tran { tstep: arg(1)?, tstop: arg(2)? },
        "noise" => {
            let out_node = arg(1)?;
            let output = out_node.strip_prefix(['v', 'V']).and_then(|s| s.strip_prefix('(')).and_then(|s| s.strip_suffix(')'));
            let output = output.filter(|o| !o.contains(',')).ok_or(".noise: the output must be a node voltage v(node)")?;
            if !arg(3)?.eq_ignore_ascii_case("dec") {
                return Err(
                    ".noise: only 'dec' sweeps are supported (.noise v(out) <source> dec <points> <fstart> <fstop>)".into()
                );
            }
            let n = arg(4)?;
            let points_per_decade = n.parse().map_err(|_| format!(".noise: points per decade '{n}' is not an integer"))?;
            Analysis::Noise { output: output.into(), source: arg(2)?, points_per_decade, fstart: arg(5)?, fstop: arg(6)? }
        }
        "temp" => {
            let v = arg(1)?;
            out.temp = Some(v.parse().map_err(|_| format!(".temp: '{v}' is not a number"))?);
            return Ok(());
        }
        "end" | "title" => return Ok(()),
        "subckt" | "ends" => {
            return Err("subcircuits are not supported; give the flat netlist of one cell".into());
        }
        "model" if t.get(1).is_some_and(|n| ["nmos", "pmos", "dmod"].iter().any(|d| n.eq_ignore_ascii_case(d))) => {
            out.warnings.push(format!(
                "'.model {}' dropped: the technology provides this model (use another name for custom parameters)",
                t[1]
            ));
            return Ok(());
        }
        _ => {
            out.extra.push(line.to_string());
            return Ok(());
        }
    };
    analysis.validate()?;
    out.analyses.push(analysis);
    Ok(())
}

fn mos_type(model: &str, tech: &Tech, model_types: &BTreeMap<String, MosType>) -> Option<MosType> {
    let m = model.to_ascii_lowercase();
    if let Some(t) = model_types.get(&m) {
        return Some(*t);
    }
    if let Some(d) = tech.devices.iter().find(|d| d.name.eq_ignore_ascii_case(model)) {
        return Some(d.kind);
    }
    if m.contains("pmos") || m.contains("pfet") || m.starts_with('p') {
        Some(MosType::Pmos)
    } else if m.contains("nmos") || m.contains("nfet") || m.starts_with('n') {
        Some(MosType::Nmos)
    } else {
        None
    }
}

fn element(
    t: &[String],
    tech: &Tech,
    model_types: &BTreeMap<String, MosType>,
    ignored: &mut Vec<String>,
) -> Result<Option<Dev>, String> {
    let name = t[0].clone();
    let letter = name.chars().next().expect("non-empty token").to_ascii_uppercase();
    let (positional, kv): (Vec<&String>, Vec<&String>) = t[1..].iter().partition(|s| !s.contains('='));
    let params_kv = || -> BTreeMap<String, String> {
        kv.iter().filter_map(|s| s.split_once('=')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect()
    };
    let need = |n: usize, form: &str| {
        if positional.len() < n { Err(format!("{name}: expected '{form}'")) } else { Ok(()) }
    };
    let nets = |n: usize| positional[..n].iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let dev = |kind, nets, params| Ok(Some(Dev { kind, name: name.clone(), nets, params }));
    match letter {
        'R' | 'C' | 'L' => {
            let (kind, key) = match letter {
                'R' => (DeviceKind::Resistor, "r"),
                'C' => (DeviceKind::Capacitor, "c"),
                _ => (DeviceKind::Inductor, "l"),
            };
            let kvs = params_kv();
            let value = positional.get(2).map(|s| s.to_string()).or_else(|| kvs.get(key).cloned());
            let Some(value) = value.filter(|_| positional.len() >= 2) else {
                return Err(format!("{name}: expected '{name} <n+> <n-> <value>'"));
            };
            dev(kind, nets(2), BTreeMap::from([(key.to_string(), value)]))
        }
        'V' | 'I' => {
            need(2, &format!("{name} <n+> <n-> [DC <v>] [AC <mag>] [PULSE(...)]"))?;
            let kind = if letter == 'V' { DeviceKind::VSource } else { DeviceKind::ISource };
            let mut params = BTreeMap::new();
            let rest: Vec<&String> = positional[2..].to_vec();
            let mut i = 0;
            while i < rest.len() {
                let tok = rest[i];
                let low = tok.to_ascii_lowercase();
                if low == "dc" || low == "ac" {
                    let v = rest.get(i + 1).ok_or_else(|| format!("{name}: {tok} needs a value"))?;
                    params.insert(low, v.to_string());
                    i += 2;
                } else if low.contains('(') {
                    params.insert("tran".into(), tok.to_string());
                    i += 1;
                } else if parse_spice(tok).is_some() && !params.contains_key("dc") {
                    params.insert("dc".into(), tok.to_string());
                    i += 1;
                } else {
                    return Err(format!("{name}: cannot read '{tok}'"));
                }
            }
            dev(kind, nets(2), params)
        }
        'D' => {
            need(3, &format!("{name} <anode> <cathode> <model>"))?;
            dev(DeviceKind::Diode, nets(2), BTreeMap::from([("model".to_string(), positional[2].to_string())]))
        }
        'M' | 'X' => {
            if positional.len() < 5 {
                return if letter == 'X' {
                    Err(format!("{name}: only PDK MOS subcircuits are supported ('{name} <d> <g> <s> <b> <device> w=.. l=..')"))
                } else {
                    Err(format!("{name}: expected '{name} <d> <g> <s> <b> <model> w=<W> l=<L>'"))
                };
            }
            let model = positional[4].to_string();
            let Some(ty) = mos_type(&model, tech, model_types) else {
                return Err(format!(
                    "{name}: cannot tell whether model '{model}' is NMOS or PMOS; add '.model {model} nmos' (or pmos)"
                ));
            };
            if letter == 'X' && !tech.devices.iter().any(|d| d.name.eq_ignore_ascii_case(&model)) {
                return Err(format!("{name}: '{model}' is not a MOS device of the technology"));
            }
            let mut params = BTreeMap::from([("model".to_string(), model)]);
            for (k, v) in params_kv() {
                if MOS_PARAMS.contains(&k.as_str()) || k == "match_rows" {
                    params.insert(k, v);
                } else {
                    ignored.push(format!("{name}: parameter {k}={v} ignored (supported: w, l, m, ad, as, pd, ps, match)"));
                }
            }
            params.entry("w".into()).or_insert_with(|| if ty == MosType::Pmos { "2u".into() } else { "1u".into() });
            params.entry("l".into()).or_insert_with(|| "180n".into());
            params.entry("m".into()).or_insert_with(|| "1".into());
            let name = if letter == 'X' { name[1..].to_string() } else { name.clone() };
            let kind = if ty == MosType::Pmos { DeviceKind::Pmos } else { DeviceKind::Nmos };
            Ok(Some(Dev { kind, name, nets: nets(4), params }))
        }
        _ => Err(format!("{name}: element type '{letter}' is not supported (use R, C, L, V, I, D, M, or X for PDK MOS devices)")),
    }
}

fn row_of(kind: DeviceKind) -> i32 {
    match kind {
        DeviceKind::Pmos => 0,
        DeviceKind::Nmos => 1,
        DeviceKind::VSource | DeviceKind::ISource => 3,
        _ => 2,
    }
}

/// Pin names of a symbol in SPICE node order.
fn pin_order(kind: DeviceKind) -> &'static [&'static str] {
    match kind {
        DeviceKind::Nmos | DeviceKind::Pmos => &["d", "g", "s", "b"],
        DeviceKind::Diode => &["a", "k"],
        _ => &["p", "n"],
    }
}

fn place(devs: &[Dev]) -> Schematic {
    let mut s = Schematic::default();
    let mut next_col = [0i32; 4];
    for d in devs {
        let row = row_of(d.kind);
        let pos = Pt::new(next_col[row as usize] * COL, row * ROW);
        next_col[row as usize] += 1;
        let id = s.add_instance(d.kind, pos, Orient::default());
        let inst = s.instance_mut(id).expect("just added");
        inst.name = d.name.clone();
        inst.params = d.params.clone();
        let pins = inst.pin_positions();
        for (pin, net) in pin_order(d.kind).iter().zip(&d.nets) {
            let at = pins.iter().find(|(n, _)| n == pin).map(|(_, p)| *p).expect("symbol pin");
            if is_ground(net) {
                let g = s.add_instance(DeviceKind::Ground, at, Orient::default());
                let gname = s.next_name(DeviceKind::Ground);
                s.instance_mut(g).expect("just added").name = gname;
            } else {
                s.add_label(net.clone(), at);
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netlist::netlist;

    const INV: &str = "\
* inverter
M1 out in vdd vdd pmos w=2u l=180n
M2 out in 0 0 nmos w=1u l=180n
C1 out 0 20f
V1 vdd 0 1.8
V2 in 0 DC 0 PULSE(0 1.8 1n 100p 100p 4n 10n)
.op
.tran 10p 20n
.end
";

    #[test]
    fn inverter_round_trip() {
        let imp = import(INV, &Tech::generic()).unwrap();
        assert!(imp.warnings.is_empty(), "{:?}", imp.warnings);
        assert_eq!(imp.analyses.len(), 2);
        let nl = netlist(&imp.schematic);
        assert!(nl.warnings.is_empty(), "{:?}", nl.warnings);
        for l in [
            "M1 out in vdd vdd pmos w=2u l=180n",
            "M2 out in 0 0 nmos w=1u l=180n",
            "C1 out 0 20f",
            "V1 vdd 0 DC 1.8",
            "V2 in 0 DC 0 PULSE(0 1.8 1n 100p 100p 4n 10n)",
        ] {
            assert!(nl.body.contains(l), "missing '{l}' in\n{}", nl.body);
        }
        let mut setup = SimSetup::default();
        imp.apply_to(&mut setup);
        let kinds: Vec<_> = setup.enabled().map(Analysis::kind_name).collect();
        assert_eq!(kinds, ["op", "tran"]);
    }

    #[test]
    fn continuation_spaced_params_and_models() {
        let deck = "M1 d g 0 0 mynfet\n+ w = 4u l=0.5u m=2 nf=2\n.model mynfet nmos (level=1 vto=0.5)\nVd d 0 1\nVg g 0 1\n";
        let imp = import(deck, &Tech::generic()).unwrap();
        let m = imp.schematic.instance_by_name("M1").unwrap();
        assert_eq!(m.kind, DeviceKind::Nmos);
        assert_eq!((m.param("w"), m.param("l"), m.param("m")), ("4u", "0.5u", "2"));
        assert_eq!(imp.extra, [".model mynfet nmos (level=1 vto=0.5)"]);
        assert!(imp.warnings.iter().any(|w| w.contains("nf=2 ignored")), "{:?}", imp.warnings);
    }

    #[test]
    fn errors_name_the_line() {
        let e = import("R1 a 0 1k\nE1 a 0 b 0 10\n", &Tech::generic()).unwrap_err();
        assert!(e.contains("line 2") && e.contains("'E'"), "{e}");
        assert!(import("M1 d g s b weird w=1u l=1u\n", &Tech::generic()).unwrap_err().contains("NMOS or PMOS"));
        assert!(import("R1 a 0 1k\nR1 b 0 1k\n", &Tech::generic()).unwrap_err().contains("duplicate"));
    }

    #[test]
    fn micron_lengths_are_converted() {
        let imp = import("M1 d g 0 0 nmos w=2 l=0.18\n", &Tech::generic()).unwrap();
        let m = imp.schematic.instance_by_name("M1").unwrap();
        assert_eq!((m.param("w"), m.param("l")), ("2u", "0.18u"));
        assert_eq!(imp.warnings.len(), 2);
    }
}
