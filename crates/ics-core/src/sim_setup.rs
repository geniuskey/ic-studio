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
        points_per_decade: u32,
        fstart: String,
        fstop: String,
    },
    Tran {
        tstep: String,
        tstop: String,
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
            Analysis::Ac { points_per_decade, fstart, fstop } => format!("ac dec {points_per_decade} {fstart} {fstop}"),
            Analysis::Tran { tstep, tstop } => format!("tran {tstep} {tstop}"),
            Analysis::Noise { output, source, points_per_decade, fstart, fstop } => {
                format!("noise v({output}) {source} dec {points_per_decade} {fstart} {fstop}")
            }
        }
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
            Analysis::Ac { points_per_decade, fstart, fstop } => {
                num("fstart", fstart)?;
                num("fstop", fstop)?;
                if *points_per_decade == 0 {
                    return Err("AC: points per decade must be > 0".into());
                }
                if parse_spice(fstart).unwrap_or(0.0) <= 0.0 {
                    return Err("AC: fstart must be > 0".into());
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
            Analysis::Tran { tstep, tstop } => {
                num("tstep", tstep)?;
                num("tstop", tstop)?;
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
                    analysis: Analysis::Ac { points_per_decade: 20, fstart: "1".into(), fstop: "10g".into() },
                },
                AnalysisEntry { enabled: false, analysis: Analysis::Tran { tstep: "10p".into(), tstop: "20n".into() } },
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
