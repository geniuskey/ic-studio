//! IC Studio simulation layer.
//!
//! A single entry point, [`simulate`], runs a SPICE deck on either the built-in
//! MNA engine or an external ngspice installation and returns backend-independent
//! [`SimResults`].

// Index-based loops read more naturally in the numerical kernels.
#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

pub mod calc;
pub mod circuit;
pub mod engine;
pub mod linalg;
pub mod ngspice;
pub mod optimize;
pub mod results;

use std::time::Instant;

use ics_core::{Analysis, Backend};

pub use results::{Dataset, DeviceOp, SimResults, VecKind, Vector};

#[derive(Debug, thiserror::Error)]
pub enum SimError {
    #[error("netlist line {line}: {msg}")]
    Parse { line: usize, msg: String },
    #[error("{0}")]
    Engine(String),
    #[error("Newton iteration did not converge")]
    NoConvergence,
    #[error("{0}")]
    Backend(String),
}

#[derive(Debug, Clone)]
pub struct SimOptions {
    pub backend: Backend,
    pub ngspice_exe: String,
}

impl Default for SimOptions {
    fn default() -> Self {
        Self { backend: Backend::Builtin, ngspice_exe: ngspice::default_exe() }
    }
}

/// Run `analyses` on `deck`. If `analyses` is empty the deck's own dot-analyses are used.
pub fn simulate(deck: &str, analyses: &[Analysis], opts: &SimOptions) -> Result<SimResults, SimError> {
    let start = Instant::now();
    if opts.backend == Backend::Builtin
        && let Some(l) = deck.lines().map(str::trim).find(|l| {
            let l = l.to_ascii_lowercase();
            l.starts_with(".lib") || l.starts_with(".include") || l.starts_with(".inc ")
        })
    {
        return Err(SimError::Engine(format!(
            "the built-in engine cannot load model libraries ('{l}'); select ngspice for PDK models"
        )));
    }
    let mut circuit = circuit::parse_deck(deck)?;
    let analyses: Vec<Analysis> = if analyses.is_empty() { std::mem::take(&mut circuit.analyses) } else { analyses.to_vec() };
    if analyses.is_empty() {
        return Err(SimError::Engine("no analyses to run".into()));
    }
    for a in &analyses {
        a.validate().map_err(SimError::Engine)?;
    }
    let mut res = SimResults::default();
    match opts.backend {
        Backend::Builtin => {
            res.backend = "built-in".into();
            let mut log = vec![format!("ics-sim: {} nodes, {} elements", circuit.node_count(), circuit.elements.len())];
            let mut eng = engine::Engine::new(circuit)?;
            for a in &analyses {
                let t0 = Instant::now();
                let ds = eng.run(a)?;
                log.append(&mut eng.log);
                log.push(format!("{:<6} {:>7} points  {:>8.2} ms", a.kind_name(), ds.len(), t0.elapsed().as_secs_f64() * 1e3));
                res.datasets.push(ds);
            }
            res.log = log.join("\n");
        }
        Backend::Ngspice => {
            res.backend = ngspice::detect(&opts.ngspice_exe).unwrap_or_else(|| "ngspice".into());
            let (ds, log) = ngspice::run(&opts.ngspice_exe, deck, &analyses)?;
            res.datasets = ds;
            res.log = log;
        }
    }
    res.elapsed_ms = start.elapsed().as_secs_f64() * 1e3;
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_rejects_model_libraries() {
        let deck = "* t\n.lib \"/pdk/models.lib\" tt\nV1 a 0 1\nR1 a 0 1k\n.op\n.end\n";
        let e = simulate(deck, &[], &SimOptions::default()).unwrap_err().to_string();
        assert!(e.contains("select ngspice"), "{e}");
    }
}
