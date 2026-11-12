//! Simulation setup stored with a cell (analyses, backend choice, saved signals).

use serde::{Deserialize, Serialize};

use crate::units::parse_spice;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Analysis {
    Op,
    Dc { source: String, start: String, stop: String, step: String },
    Ac { points_per_decade: u32, fstart: String, fstop: String },
    Tran { tstep: String, tstop: String },
}

impl Analysis {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Analysis::Op => "op",
            Analysis::Dc { .. } => "dc",
            Analysis::Ac { .. } => "ac",
            Analysis::Tran { .. } => "tran",
        }
    }

    pub fn title(&self) -> &'static str {
        match self {
            Analysis::Op => "Operating Point",
            Analysis::Dc { .. } => "DC Sweep",
            Analysis::Ac { .. } => "AC Small-Signal",
            Analysis::Tran { .. } => "Transient",
        }
    }

    /// SPICE dot-command / control-command for this analysis (without leading dot).
    pub fn spice_command(&self) -> String {
        match self {
            Analysis::Op => "op".into(),
            Analysis::Dc { source, start, stop, step } => format!("dc {source} {start} {stop} {step}"),
            Analysis::Ac { points_per_decade, fstart, fstop } => format!("ac dec {points_per_decade} {fstart} {fstop}"),
            Analysis::Tran { tstep, tstop } => format!("tran {tstep} {tstop}"),
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
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Backend::Builtin => "Built-in (ics-sim)",
            Backend::Ngspice => "ngspice",
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
            ],
            extra: String::new(),
            temp: 27.0,
            parasitics: false,
            corner: String::new(),
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
