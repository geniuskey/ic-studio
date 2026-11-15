//! Silicon-to-simulation correlation: measured curves (CSV) of a cell set against its simulation,
//! and the parameters (model card, instance or design variable) to fit to them.
//!
//! A [`SiliconSetup`] names the data file, the analysis whose sweep gives the x axis, one
//! [`Trace`] per measured column (the waveform-calculator expression it is compared with and
//! the bias it was measured at) and the [`FitParam`]s. Model parameters are written
//! `model:NAME.PARAM` (e.g. `model:nmos.vto`); they change every card of that name, binned
//! cards (`NAME.1`, `NAME.2`, ...) included.

use serde::{Deserialize, Serialize};

use crate::units::parse_spice;

/// Correlation setup of a cell (`sim.silicon`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SiliconSetup {
    /// Measured data, CSV with a header row; relative to the project directory.
    pub data: String,
    /// Analysis whose sweep is the x axis: `dc`, `ac` or `tran`.
    pub analysis: String,
    /// Column of the x values (empty = the first column).
    pub x: String,
    pub traces: Vec<Trace>,
    pub params: Vec<FitParam>,
}

impl SiliconSetup {
    pub fn is_empty(&self) -> bool {
        self == &SiliconSetup::default()
    }
}

/// One measured column and how to simulate it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Trace {
    pub column: String,
    /// Waveform-calculator expression on the analysis dataset, e.g. `-i(vd)`.
    pub expr: String,
    /// Bias of this curve: `INST.param=value` or `design_var=value`, separated by spaces.
    pub bias: String,
    /// Compare on a log scale (currents over decades).
    pub log: bool,
}

/// A parameter to fit: `model:NAME.PARAM`, `INST.param` or a design variable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct FitParam {
    pub target: String,
    /// Bounds as SPICE numbers.
    pub min: String,
    pub max: String,
    /// Search on a log scale (min > 0).
    pub log: bool,
}

impl FitParam {
    /// The model card and parameter of a `model:NAME.PARAM` target.
    pub fn model_param(&self) -> Option<(&str, &str)> {
        let t = self.target.trim().strip_prefix("model:")?;
        let (m, p) = t.rsplit_once('.')?;
        Some((m.trim(), p.trim()))
    }

    /// The same bounds as an optimizer variable (for [`crate::optimize::Bounds`]).
    pub fn as_opt_var(&self) -> crate::optimize::OptVar {
        crate::optimize::OptVar {
            target: self.target.clone(),
            min: self.min.clone(),
            max: self.max.clone(),
            log: self.log,
            step: String::new(),
        }
    }
}

/// Measured data: named columns of numbers.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Measured {
    pub columns: Vec<String>,
    /// `rows[r][c]`; NaN where a cell is empty.
    pub rows: Vec<Vec<f64>>,
}

impl Measured {
    /// Parse CSV (comma, semicolon or tab separated) with a header row. Lines starting with `#`
    /// are comments; numbers may use SPICE suffixes (`1.2u`).
    pub fn parse(text: &str) -> Result<Measured, String> {
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'));
        let head = lines.next().ok_or("the data file is empty")?;
        let sep = [',', ';', '\t'].into_iter().find(|c| head.contains(*c)).unwrap_or(',');
        let split = |l: &str| l.split(sep).map(|c| c.trim().trim_matches('"').to_string()).collect::<Vec<_>>();
        let columns = split(head);
        if columns.iter().any(|c| c.is_empty()) {
            return Err("every column needs a name in the header row".into());
        }
        let mut rows = vec![];
        for (k, l) in lines.enumerate() {
            let cells = split(l);
            if cells.len() > columns.len() {
                return Err(format!("data row {}: {} values for {} columns", k + 1, cells.len(), columns.len()));
            }
            let mut row = vec![f64::NAN; columns.len()];
            for (c, v) in cells.iter().enumerate() {
                if !v.is_empty() {
                    row[c] = parse_spice(v).ok_or_else(|| format!("data row {}: '{v}' is not a number", k + 1))?;
                }
            }
            rows.push(row);
        }
        if rows.is_empty() {
            return Err("the data file has no rows".into());
        }
        Ok(Measured { columns, rows })
    }

    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.eq_ignore_ascii_case(name.trim()))
    }

    /// The (x, y) points of column `y` over column `x` where both are numbers.
    pub fn points(&self, x: &str, y: &str) -> Result<(Vec<f64>, Vec<f64>), String> {
        let xi = if x.trim().is_empty() { 0 } else { self.column_index(x).ok_or_else(|| format!("no column '{x}'"))? };
        let yi = self.column_index(y).ok_or_else(|| format!("no column '{y}'"))?;
        Ok(self.rows.iter().filter(|r| r[xi].is_finite() && r[yi].is_finite()).map(|r| (r[xi], r[yi])).unzip())
    }
}

/// `NAME=value` assignments separated by spaces or commas.
pub fn parse_bias(s: &str) -> Result<Vec<(String, String)>, String> {
    s.split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .map(|t| {
            let (k, v) = t.split_once('=').ok_or_else(|| format!("bias '{t}': write NAME=value"))?;
            parse_spice(v).ok_or_else(|| format!("bias '{t}': '{v}' is not a number"))?;
            Ok((k.trim().to_string(), v.trim().to_string()))
        })
        .collect()
}

/// Whether the card name `card` is model `name` or one of its bins (`name.1`).
fn card_matches(card: &str, name: &str) -> bool {
    card.eq_ignore_ascii_case(name)
        || card
            .get(..name.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(name) && card[name.len()..].strip_prefix('.').is_some_and(|b| !b.is_empty()))
}

/// `key=value` pairs of a model card.
type ModelParams = Vec<(String, String)>;

/// The tokens of a `.model` card: name, type and `key=value` pairs (other words kept as keys
/// with an empty value).
fn model_tokens(line: &str) -> Option<(String, String, ModelParams)> {
    let mut s = line.replace(['(', ')'], " ");
    while s.contains(" =") || s.contains("= ") {
        s = s.replace(" =", "=").replace("= ", "=");
    }
    let mut it = s.split_whitespace();
    if !it.next()?.eq_ignore_ascii_case(".model") {
        return None;
    }
    let (name, kind) = (it.next()?.to_string(), it.next()?.to_string());
    let params =
        it.map(|t| t.split_once('=').map_or((t.to_string(), String::new()), |(k, v)| (k.to_string(), v.to_string()))).collect();
    Some((name, kind, params))
}

/// The deck as logical lines (continuations joined).
fn logical_lines(deck: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for l in deck.lines() {
        match (l.trim_start().strip_prefix('+'), out.last_mut()) {
            (Some(rest), Some(prev)) => {
                prev.push(' ');
                prev.push_str(rest.trim());
            }
            _ => out.push(l.to_string()),
        }
    }
    out
}

/// Set `param` of every `.model` card named `model` (or a bin of it) to `value`, adding the
/// parameter where a card lacks it. Errors when the deck has no such card.
pub fn set_model_param(deck: &str, model: &str, param: &str, value: f64) -> Result<String, String> {
    let mut hit = false;
    let mut out = String::with_capacity(deck.len() + 64);
    for l in logical_lines(deck) {
        match model_tokens(&l) {
            Some((name, kind, mut params)) if card_matches(&name, model) => {
                hit = true;
                let v = format!("{value:.6e}");
                match params.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(param)) {
                    Some(p) => p.1 = v,
                    None => params.push((param.to_ascii_lowercase(), v)),
                }
                let body: Vec<String> =
                    params.iter().map(|(k, v)| if v.is_empty() { k.clone() } else { format!("{k}={v}") }).collect();
                out += &format!(".model {name} {kind} ({})\n", body.join(" "));
            }
            _ => {
                out += &l;
                out.push('\n');
            }
        }
    }
    if hit { Ok(out) } else { Err(format!("no .model card '{model}' in the deck")) }
}

/// The value of `param` in the first `.model` card named `model` (or a bin of it).
pub fn model_param(deck: &str, model: &str, param: &str) -> Option<f64> {
    logical_lines(deck)
        .iter()
        .filter_map(|l| model_tokens(l))
        .filter(|(n, _, _)| card_matches(n, model))
        .find_map(|(_, _, ps)| ps.iter().find(|(k, _)| k.eq_ignore_ascii_case(param)).and_then(|(_, v)| parse_spice(v)))
}

/// The `.model` cards named `model` (or its bins), one line each.
pub fn model_cards(deck: &str, model: &str) -> Vec<String> {
    logical_lines(deck)
        .into_iter()
        .filter(|l| model_tokens(l).is_some_and(|(n, _, _)| card_matches(&n, model)))
        .map(|l| l.trim().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv() {
        let m = Measured::parse("# id-vd\nvd, id_10u;x\n").unwrap_err();
        assert!(m.contains("no rows") || m.contains("name"), "{m}");
        let m = Measured::parse("# id-vd\nvd,id_10u,id_20u\n0,0,0\n0.5,9u,\n1.0,10.2u,20.5u\n").unwrap();
        assert_eq!(m.columns, ["vd", "id_10u", "id_20u"]);
        let (x, y) = m.points("", "id_20u").unwrap();
        assert_eq!((x, y), (vec![0.0, 1.0], vec![0.0, 20.5e-6]));
        assert!(m.points("vd", "nope").is_err());
        assert!(Measured::parse("a;b\n1;2\n").unwrap().column_index("B") == Some(1));
        assert!(Measured::parse("a,b\n1,x\n").is_err());
    }

    #[test]
    fn bias() {
        assert_eq!(parse_bias("I1.dc=10u vg=1.2").unwrap(), [("I1.dc".into(), "10u".into()), ("vg".into(), "1.2".into())]);
        assert!(parse_bias("I1.dc").is_err());
        assert!(parse_bias("").unwrap().is_empty());
    }

    #[test]
    fn model_cards_edit() {
        let deck = "* t\nM1 d g 0 0 nch w=1u l=1u\n.model nch.1 nmos (level=1 vto=0.45\n+ kp=280u)\n.model nch.2 nmos level=1 vto = 0.5\n.model pch pmos (vto=-0.4)\n.end\n";
        let d = set_model_param(deck, "nch", "vto", 0.6).unwrap();
        assert_eq!(model_cards(&d, "nch").len(), 2);
        assert!(d.contains(".model nch.1 nmos (level=1 vto=6.000000e-1 kp=280u)"), "{d}");
        assert!(d.contains(".model nch.2 nmos (level=1 vto=6.000000e-1)"), "{d}");
        assert!(d.contains(".model pch pmos (vto=-0.4)") && d.contains("M1 d g 0 0 nch"));
        let d = set_model_param(&d, "pch", "lambda", 0.1).unwrap();
        assert_eq!(model_param(&d, "pch", "lambda"), Some(0.1));
        assert_eq!(model_param(deck, "nch", "kp"), Some(280e-6));
        assert!(set_model_param(deck, "nc", "vto", 1.0).is_err(), "no partial name match");
        let p = FitParam { target: "model:nch.vto".into(), ..Default::default() };
        assert_eq!(p.model_param(), Some(("nch", "vto")));
        assert_eq!(FitParam { target: "M1.w".into(), ..Default::default() }.model_param(), None);
    }
}
