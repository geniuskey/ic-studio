//! Simulation setup stored with a cell (analyses, backend choice, saved signals).

use serde::{Deserialize, Serialize};

use crate::units::parse_spice;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Analysis {
    Op,
    Dc {
        source: String,
        start: String,
        stop: String,
        step: String,
    },
    Ac {
        /// Points per decade (`dec`), per octave (`oct`) or in all (`lin`).
        points_per_decade: u32,
        fstart: String,
        fstop: String,
        #[serde(default, skip_serializing_if = "AcSweep::is_dec")]
        sweep: AcSweep,
    },
    Tran {
        tstep: String,
        tstop: String,
        /// Results start at `tstart` (empty: 0); `tmax` caps the time step (empty: none).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        tstart: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        tmax: String,
        /// Start from the `.ic` values without an operating point.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        uic: bool,
    },
    /// Noise spectral density at node `output`, also referred to the input `source`.
    Noise {
        output: String,
        source: String,
        points_per_decade: u32,
        fstart: String,
        fstop: String,
    },
}

/// Frequency spacing of an AC sweep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AcSweep {
    #[default]
    Dec,
    Oct,
    Lin,
}

impl AcSweep {
    pub fn is_dec(&self) -> bool {
        *self == AcSweep::Dec
    }

    pub fn keyword(self) -> &'static str {
        match self {
            AcSweep::Dec => "dec",
            AcSweep::Oct => "oct",
            AcSweep::Lin => "lin",
        }
    }

    pub fn parse(s: &str) -> Option<AcSweep> {
        match s.to_ascii_lowercase().as_str() {
            "dec" => Some(AcSweep::Dec),
            "oct" => Some(AcSweep::Oct),
            "lin" => Some(AcSweep::Lin),
            _ => None,
        }
    }

    /// The swept frequencies: `n` per decade or octave, or `n` in all for `lin`.
    pub fn frequencies(self, n: u32, fstart: f64, fstop: f64) -> Vec<f64> {
        let n = n.max(1);
        match self {
            AcSweep::Lin => {
                if n == 1 || fstop == fstart {
                    return vec![fstart];
                }
                (0..n).map(|k| fstart + (fstop - fstart) * k as f64 / (n - 1) as f64).collect()
            }
            AcSweep::Dec | AcSweep::Oct => {
                if fstart <= 0.0 || fstop < fstart {
                    return vec![];
                }
                let base: f64 = if self == AcSweep::Dec { 10.0 } else { 2.0 };
                let steps = ((fstop / fstart).log(base) * n as f64 + 1e-9).floor() as usize;
                let mut v: Vec<f64> = (0..=steps).map(|k| fstart * base.powf(k as f64 / n as f64)).collect();
                if v.last().is_some_and(|f| (fstop - f) / fstop > 1e-9) {
                    v.push(fstop);
                }
                v
            }
        }
    }
}

impl Analysis {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Analysis::Op => "op",
            Analysis::Dc { .. } => "dc",
            Analysis::Ac { .. } => "ac",
            Analysis::Tran { .. } => "tran",
            Analysis::Noise { .. } => "noise",
        }
    }

    pub fn title(&self) -> &'static str {
        match self {
            Analysis::Op => "Operating Point",
            Analysis::Dc { .. } => "DC Sweep",
            Analysis::Ac { .. } => "AC Small-Signal",
            Analysis::Tran { .. } => "Transient",
            Analysis::Noise { .. } => "Noise",
        }
    }

    /// SPICE dot-command / control-command for this analysis (without leading dot).
    pub fn spice_command(&self) -> String {
        match self {
            Analysis::Op => "op".into(),
            Analysis::Dc { source, start, stop, step } => format!("dc {source} {start} {stop} {step}"),
            Analysis::Ac { points_per_decade, fstart, fstop, sweep } => {
                format!("ac {} {points_per_decade} {fstart} {fstop}", sweep.keyword())
            }
            Analysis::Tran { tstep, tstop, tstart, tmax, uic } => {
                let mut s = format!("tran {tstep} {tstop}");
                if !tstart.trim().is_empty() || !tmax.trim().is_empty() {
                    s += &format!(" {}", if tstart.trim().is_empty() { "0" } else { tstart.trim() });
                }
                if !tmax.trim().is_empty() {
                    s += &format!(" {}", tmax.trim());
                }
                if *uic {
                    s += " uic";
                }
                s
            }
            Analysis::Noise { output, source, points_per_decade, fstart, fstop } => {
                format!("noise v({output}) {source} dec {points_per_decade} {fstart} {fstop}")
            }
        }
    }

    /// `.tran tstep tstop` without start, step limit or `uic`.
    pub fn tran(tstep: &str, tstop: &str) -> Analysis {
        Analysis::Tran { tstep: tstep.into(), tstop: tstop.into(), tstart: String::new(), tmax: String::new(), uic: false }
    }

    pub fn validate(&self) -> Result<(), String> {
        let num = |label: &str, v: &str| parse_spice(v).map(|_| ()).ok_or_else(|| format!("{label}: invalid number '{v}'"));
        match self {
            Analysis::Op => Ok(()),
            Analysis::Dc { source, start, stop, step } => {
                if source.trim().is_empty() {
                    return Err("DC sweep: no source selected".into());
                }
                num("start", start)?;
                num("stop", stop)?;
                num("step", step)?;
                if parse_spice(step).unwrap_or(0.0) == 0.0 {
                    return Err("DC sweep: step must be non-zero".into());
                }
                Ok(())
            }
            Analysis::Ac { points_per_decade, fstart, fstop, sweep } => {
                num("fstart", fstart)?;
                num("fstop", fstop)?;
                if *points_per_decade == 0 {
                    return Err("AC: the number of points must be > 0".into());
                }
                let f0 = parse_spice(fstart).unwrap_or(0.0);
                if *sweep == AcSweep::Lin && f0 < 0.0 {
                    return Err("AC: fstart must be >= 0".into());
                }
                if *sweep != AcSweep::Lin && f0 <= 0.0 {
                    return Err("AC: fstart must be > 0 (a lin sweep may start at 0)".into());
                }
                Ok(())
            }
            Analysis::Noise { output, source, points_per_decade, fstart, fstop } => {
                if output.trim().is_empty() || source.trim().is_empty() {
                    return Err("Noise: give the output node and the input source".into());
                }
                num("fstart", fstart)?;
                num("fstop", fstop)?;
                if *points_per_decade == 0 || parse_spice(fstart).unwrap_or(0.0) <= 0.0 {
                    return Err("Noise: points per decade and fstart must be > 0".into());
                }
                Ok(())
            }
            Analysis::Tran { tstep, tstop, tstart, tmax, .. } => {
                num("tstep", tstep)?;
                num("tstop", tstop)?;
                for (label, v) in [("tstart", tstart), ("tmax", tmax)] {
                    if !v.trim().is_empty() {
                        num(label, v)?;
                    }
                }
                if parse_spice(tstep).unwrap_or(0.0) <= 0.0 || parse_spice(tstop).unwrap_or(0.0) <= 0.0 {
                    return Err("Transient: tstep and tstop must be > 0".into());
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// The built-in Rust MNA solver (always available).
    #[default]
    Builtin,
    /// External ngspice executable.
    Ngspice,
    /// The built-in solver when it evaluates the deck exactly, ngspice otherwise (PDK model
    /// libraries, OSDI models, device models or parameters the built-in solver lacks).
    Auto,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Backend::Builtin => "Built-in (ics-sim)",
            Backend::Ngspice => "ngspice",
            Backend::Auto => "Auto (built-in, ngspice when needed)",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnalysisEntry {
    pub enabled: bool,
    pub analysis: Analysis,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimSetup {
    pub backend: Backend,
    pub analyses: Vec<AnalysisEntry>,
    /// Extra raw SPICE lines appended to the deck (models, options, includes).
    #[serde(default)]
    pub extra: String,
    /// Temperature in °C.
    #[serde(default = "default_temp")]
    pub temp: f64,
    /// Include layout parasitics (`pex::post_layout_lines`) in the deck.
    #[serde(default)]
    pub parasitics: bool,
    /// Model corner of the PDK model library (empty = its default).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub corner: String,
    /// Corner / parametric sweep of the Sweep Manager.
    #[serde(default, skip_serializing_if = "crate::sweep::SweepSetup::is_empty")]
    pub sweep: crate::sweep::SweepSetup,
    /// Spec-driven optimizer setup (variables, budget, goal).
    #[serde(default, skip_serializing_if = "crate::optimize::OptSetup::is_empty")]
    pub optimize: crate::optimize::OptSetup,
    /// Process / mismatch Monte Carlo of the built-in engine's device models.
    #[serde(default, skip_serializing_if = "McSetup::is_default")]
    pub montecarlo: McSetup,
    /// Measured data to correlate with and the parameters to fit.
    #[serde(default, skip_serializing_if = "crate::silicon::SiliconSetup::is_empty")]
    pub silicon: crate::silicon::SiliconSetup,
}

/// Device-model Monte Carlo: global shifts per polarity plus Pelgrom mismatch. Numbers are
/// kept as typed (SPICE notation).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct McSetup {
    pub runs: usize,
    pub seed: u64,
    /// Global threshold sigma per polarity (V).
    pub vto_sigma: String,
    /// Global relative transconductance sigma per polarity.
    pub kp_sigma: String,
    /// Threshold mismatch coefficient (V*m): 5n is 5 mV*um.
    pub avt: String,
    /// Current factor mismatch coefficient (m): 10n is 1 %*um.
    pub abeta: String,
    pub measures: Vec<McMeasure>,
}

impl Default for McSetup {
    fn default() -> Self {
        Self {
            runs: 100,
            seed: 1,
            vto_sigma: "20m".into(),
            kp_sigma: "0.03".into(),
            avt: "5n".into(),
            abeta: "10n".into(),
            measures: vec![],
        }
    }
}

impl McSetup {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The four sigmas as numbers: (vto_sigma, kp_sigma, avt, abeta).
    pub fn sigmas(&self) -> Result<(f64, f64, f64, f64), String> {
        let num = |label: &str, v: &str| parse_spice(v).ok_or_else(|| format!("{label}: invalid number '{v}'"));
        Ok((
            num("vto sigma", &self.vto_sigma)?,
            num("kp sigma", &self.kp_sigma)?,
            num("avt", &self.avt)?,
            num("abeta", &self.abeta)?,
        ))
    }
}

/// A scalar waveform-calculator expression evaluated on the dataset of `analysis`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct McMeasure {
    pub analysis: String,
    pub expr: String,
}

fn default_temp() -> f64 {
    27.0
}

impl Default for SimSetup {
    fn default() -> Self {
        Self {
            backend: Backend::Builtin,
            analyses: vec![
                AnalysisEntry { enabled: true, analysis: Analysis::Op },
                AnalysisEntry {
                    enabled: false,
                    analysis: Analysis::Dc { source: String::new(), start: "0".into(), stop: "1.8".into(), step: "10m".into() },
                },
                AnalysisEntry {
                    enabled: false,
                    analysis: Analysis::Ac {
                        points_per_decade: 20,
                        fstart: "1".into(),
                        fstop: "10g".into(),
                        sweep: AcSweep::Dec,
                    },
                },
                AnalysisEntry { enabled: false, analysis: Analysis::tran("10p", "20n") },
                AnalysisEntry {
                    enabled: false,
                    analysis: Analysis::Noise {
                        output: "out".into(),
                        source: String::new(),
                        points_per_decade: 10,
                        fstart: "1".into(),
                        fstop: "1g".into(),
                    },
                },
            ],
            extra: String::new(),
            temp: 27.0,
            parasitics: false,
            corner: String::new(),
            sweep: Default::default(),
            optimize: Default::default(),
            montecarlo: Default::default(),
            silicon: Default::default(),
        }
    }
}

impl SimSetup {
    pub fn enabled(&self) -> impl Iterator<Item = &Analysis> {
        self.analyses.iter().filter(|a| a.enabled).map(|a| &a.analysis)
    }

    pub fn set_enabled(&mut self, kind: &str, on: bool) {
        for a in &mut self.analyses {
            if a.analysis.kind_name() == kind {
                a.enabled = on;
            }
        }
    }

    pub fn get_mut(&mut self, kind: &str) -> Option<&mut Analysis> {
        self.analyses.iter_mut().find(|a| a.analysis.kind_name() == kind).map(|a| &mut a.analysis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn montecarlo_setup_round_trips() {
        let s = SimSetup::default();
        assert!(!serde_json::to_string(&s).unwrap().contains("montecarlo"));
        let mut s = s;
        s.montecarlo.runs = 20;
        s.montecarlo.measures.push(McMeasure { analysis: "dc".into(), expr: "cross(v(out),0.9)".into() });
        let back: SimSetup = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.montecarlo.sigmas().unwrap(), (0.02, 0.03, 5e-9, 1e-8));
    }
}
