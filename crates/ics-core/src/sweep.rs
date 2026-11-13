//! Corner and parametric sweeps (ADE Explorer / Assembler style): every combination of model
//! corners, temperatures and variable values is one simulation point, and specs are checked on
//! the results of each.

use serde::{Deserialize, Serialize};

use crate::netlist::netlist_for;
use crate::schematic::Schematic;
use crate::sim_setup::SimSetup;
use crate::tech::Tech;
use crate::units::parse_spice;

/// Upper bound on the number of points of one sweep.
pub const MAX_POINTS: usize = 2000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SweepSetup {
    /// Model corners to run; empty runs the simulation setup's corner only.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub corners: Vec<String>,
    /// Temperatures in °C as a value list ([`parse_values`]); empty runs the setup's temperature.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub temps: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub vars: Vec<SweepVar>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub specs: Vec<Spec>,
}

/// A swept variable: `INST.param` sets an instance parameter (e.g. `R1.r`, `M1.w`); any other
/// name is a design variable, substituted wherever the deck writes `{name}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SweepVar {
    pub name: String,
    /// Value list ([`parse_values`]).
    pub values: String,
}

/// A measured quantity with optional limits: `expr` is a waveform calculator expression evaluated
/// on the `analysis` dataset (`dc`, `ac`, `tran`, `op`) of each point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Spec {
    pub name: String,
    pub analysis: String,
    pub expr: String,
    /// Lower limit as a SPICE number (empty = none).
    pub min: String,
    /// Upper limit as a SPICE number (empty = none).
    pub max: String,
}

impl Spec {
    /// Whether `v` meets the limits; `None` when the spec has none.
    pub fn check(&self, v: f64) -> Option<bool> {
        let (lo, hi) = (parse_spice(&self.min), parse_spice(&self.max));
        if lo.is_none() && hi.is_none() {
            return None;
        }
        Some(v.is_finite() && lo.is_none_or(|l| v >= l) && hi.is_none_or(|h| v <= h))
    }
}

/// One combination of the sweep.
#[derive(Debug, Clone, PartialEq)]
pub struct SweepPoint {
    pub corner: String,
    pub temp: f64,
    pub vars: Vec<(String, String)>,
}

impl SweepPoint {
    pub fn label(&self) -> String {
        let mut parts = vec![];
        if !self.corner.is_empty() {
            parts.push(self.corner.clone());
        }
        parts.push(format!("{}°C", self.temp));
        parts.extend(self.vars.iter().map(|(n, v)| format!("{n}={v}")));
        parts.join(" ")
    }
}

/// Values separated by spaces or commas; `start:step:stop` expands to a linear range.
pub fn parse_values(s: &str) -> Result<Vec<String>, String> {
    let mut out = vec![];
    for tok in s.split(|c: char| c.is_whitespace() || c == ',').filter(|t| !t.is_empty()) {
        let parts: Vec<&str> = tok.split(':').collect();
        match parts.as_slice() {
            [v] => {
                parse_spice(v).ok_or_else(|| format!("invalid number '{v}'"))?;
                out.push(v.to_string());
            }
            [a, st, b] => {
                let num = |v: &str| parse_spice(v).ok_or_else(|| format!("invalid number '{v}' in '{tok}'"));
                let (a, st, b) = (num(a)?, num(st)?, num(b)?);
                if st == 0.0 || (b - a) * st < 0.0 {
                    return Err(format!("range '{tok}': the step must be non-zero and go from start towards stop"));
                }
                let n = ((b - a) / st + 1e-9).floor() as usize + 1;
                if n > MAX_POINTS {
                    return Err(format!("range '{tok}' has more than {MAX_POINTS} values"));
                }
                out.extend((0..n).map(|k| fmt_value(a + k as f64 * st)));
            }
            _ => return Err(format!("invalid value '{tok}' (use a number or start:step:stop)")),
        }
    }
    Ok(out)
}

fn fmt_value(v: f64) -> String {
    let s = format!("{v:.9}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

impl SweepSetup {
    pub fn is_empty(&self) -> bool {
        self == &SweepSetup::default()
    }

    /// Every combination of corners, temperatures and variable values, corners outermost.
    pub fn points(&self, base: &SimSetup) -> Result<Vec<SweepPoint>, String> {
        let corners = if self.corners.is_empty() { vec![base.corner.clone()] } else { self.corners.clone() };
        let temps: Vec<f64> = if self.temps.trim().is_empty() {
            vec![base.temp]
        } else {
            parse_values(&self.temps).map_err(|e| format!("temperatures: {e}"))?.iter().filter_map(|t| parse_spice(t)).collect()
        };
        let mut axes: Vec<(String, Vec<String>)> = vec![];
        for v in self.vars.iter().filter(|v| !v.name.trim().is_empty()) {
            let vals = parse_values(&v.values).map_err(|e| format!("{}: {e}", v.name))?;
            if vals.is_empty() {
                return Err(format!("{}: no values", v.name));
            }
            axes.push((v.name.trim().to_string(), vals));
        }
        let total = axes.iter().fold(corners.len() * temps.len(), |n, (_, v)| n.saturating_mul(v.len()));
        if total > MAX_POINTS {
            return Err(format!("{total} points exceed the limit of {MAX_POINTS}"));
        }
        let mut out = Vec::with_capacity(total);
        for c in &corners {
            for &t in &temps {
                let mut idx = vec![0usize; axes.len()];
                'combos: loop {
                    let vars = axes.iter().zip(&idx).map(|((n, v), &k)| (n.clone(), v[k].clone())).collect();
                    out.push(SweepPoint { corner: c.clone(), temp: t, vars });
                    for d in (0..axes.len()).rev() {
                        idx[d] += 1;
                        if idx[d] < axes[d].1.len() {
                            continue 'combos;
                        }
                        idx[d] = 0;
                    }
                    break;
                }
            }
        }
        Ok(out)
    }
}

/// The deck of one sweep point: instance parameters set, corner and temperature applied and
/// design variables substituted. Returns the deck and the netlister's warnings.
pub fn point_deck(
    sch: &Schematic,
    tech: &Tech,
    title: &str,
    base: &SimSetup,
    pt: &SweepPoint,
) -> Result<(String, Vec<String>), String> {
    let mut sch = sch.clone();
    let mut design = vec![];
    for (name, value) in &pt.vars {
        match name.split_once('.') {
            Some((inst, param)) => {
                let i = sch
                    .instances
                    .iter_mut()
                    .find(|i| i.name.eq_ignore_ascii_case(inst))
                    .ok_or_else(|| format!("{name}: no instance '{inst}' in the schematic"))?;
                i.params.insert(param.to_ascii_lowercase(), value.clone());
            }
            None => design.push((name, value)),
        }
    }
    let mut setup = base.clone();
    setup.corner = pt.corner.clone();
    setup.temp = pt.temp;
    let nl = netlist_for(&sch, tech, &setup.corner);
    let mut deck = nl.deck(title, &setup);
    for (name, value) in design {
        deck = deck.replace(&format!("{{{name}}}"), value);
    }
    Ok((deck, nl.warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Orient, Pt};
    use crate::schematic::DeviceKind;

    #[test]
    fn values_and_ranges() {
        assert_eq!(parse_values("1k, 2k 5k").unwrap(), ["1k", "2k", "5k"]);
        assert_eq!(parse_values("0:0.5:2").unwrap(), ["0", "0.5", "1", "1.5", "2"]);
        assert_eq!(parse_values("-40:82.5:125").unwrap(), ["-40", "42.5", "125"]);
        assert!(parse_values("1:0:2").is_err());
        assert!(parse_values("2:1:1").is_err());
        assert!(parse_values("x").is_err());
    }

    #[test]
    fn points_cover_every_combination() {
        let base = SimSetup::default();
        let s = SweepSetup {
            corners: vec!["tt".into(), "ff".into()],
            temps: "-40 125".into(),
            vars: vec![SweepVar { name: "R1.r".into(), values: "1k 2k 3k".into() }],
            specs: vec![],
        };
        let pts = s.points(&base).unwrap();
        assert_eq!(pts.len(), 12);
        assert_eq!(pts[0].label(), "tt -40°C R1.r=1k");
        assert_eq!(pts[11].label(), "ff 125°C R1.r=3k");
        let only_base = SweepSetup::default().points(&base).unwrap();
        assert_eq!(only_base, [SweepPoint { corner: String::new(), temp: 27.0, vars: vec![] }]);
    }

    #[test]
    fn two_axes() {
        let s = SweepSetup {
            vars: vec![
                SweepVar { name: "a".into(), values: "1 2".into() },
                SweepVar { name: "b".into(), values: "3 4 5".into() },
            ],
            ..Default::default()
        };
        let pts = s.points(&SimSetup::default()).unwrap();
        let got: Vec<String> = pts.iter().map(|p| p.vars.iter().map(|(_, v)| v.as_str()).collect::<Vec<_>>().join(",")).collect();
        assert_eq!(got, ["1,3", "1,4", "1,5", "2,3", "2,4", "2,5"]);
    }

    #[test]
    fn deck_applies_point() {
        let mut sch = Schematic::default();
        let v = sch.add_instance(DeviceKind::VSource, Pt::new(0, 0), Orient::default());
        sch.instance_mut(v).unwrap().params.insert("dc".into(), "{vdd}".into());
        let r = sch.add_instance(DeviceKind::Resistor, Pt::new(4, 0), Orient::default());
        let rname = sch.instance(r).unwrap().name.clone();
        let pt = SweepPoint {
            corner: String::new(),
            temp: 85.0,
            vars: vec![("vdd".into(), "1.2".into()), (format!("{rname}.r"), "4.7k".into())],
        };
        let (deck, _) = point_deck(&sch, &Tech::generic(), "t", &SimSetup::default(), &pt).unwrap();
        assert!(deck.contains("DC 1.2"), "{deck}");
        assert!(deck.contains(" 4.7k"), "{deck}");
        assert!(deck.contains(".temp 85"), "{deck}");
        let bad = SweepPoint { vars: vec![("X9.r".into(), "1".into())], ..pt };
        assert!(point_deck(&sch, &Tech::generic(), "t", &SimSetup::default(), &bad).is_err());
    }

    #[test]
    fn spec_limits() {
        let s = Spec { min: "1m".into(), max: "".into(), ..Default::default() };
        assert_eq!(s.check(2e-3), Some(true));
        assert_eq!(s.check(0.5e-3), Some(false));
        assert_eq!(s.check(f64::NAN), Some(false));
        assert_eq!(Spec::default().check(1.0), None);
    }
}
