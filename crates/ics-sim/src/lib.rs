//! IC Studio simulation layer.
//!
//! A single entry point, [`simulate`], runs a SPICE deck on either the built-in
//! MNA engine or an external ngspice installation and returns backend-independent
//! [`SimResults`].

// Index-based loops read more naturally in the numerical kernels.
#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

pub mod bsim4;
pub mod calc;
pub mod circuit;
pub mod correlate;
pub mod engine;
pub mod expr;
pub mod frontend;
pub mod linalg;
pub mod montecarlo;
pub mod ngspice;
pub mod optimize;
pub mod results;
pub mod silicon;
pub mod sweep;

use std::path::{Path, PathBuf};
use std::time::Instant;

use ics_core::{Analysis, Backend};

pub use correlate::{Correlation, VecDiff, compare, correlate};
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
    /// Folder that relative `.include` / `.lib` paths of the deck are taken from (the current
    /// folder without one).
    pub deck_dir: Option<PathBuf>,
}

impl Default for SimOptions {
    fn default() -> Self {
        Self { backend: Backend::Builtin, ngspice_exe: ngspice::default_exe(), deck_dir: None }
    }
}

/// The first deck line loading model files (model libraries, compiled Verilog-A models).
pub(crate) fn external_models(deck: &str) -> Option<&str> {
    deck.lines().map(str::trim).find(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with(".lib") || l.starts_with(".include") || l.starts_with(".inc ") || l.starts_with("pre_osdi")
    })
}

/// The first deck line loading compiled (Verilog-A) models, which only ngspice evaluates.
fn compiled_models(deck: &str) -> Option<&str> {
    deck.lines().map(str::trim).find(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("pre_osdi") || l.starts_with(".hdl") || l.starts_with(".osdi")
    })
}

/// The flat circuit of `deck` for the built-in engine: libraries, parameters and subcircuits
/// are resolved first (see [`frontend`]), with errors and warnings pointing at the source lines.
pub fn parse_for_builtin(deck: &str, dir: Option<&Path>) -> Result<circuit::Circuit, SimError> {
    if !frontend::needs_expansion(deck) {
        return circuit::parse_deck(deck);
    }
    let e = frontend::expand(deck, dir)?;
    let mut c = circuit::parse_deck(&e.deck).map_err(|err| e.map_error(err))?;
    c.title = deck.lines().next().unwrap_or("").trim_start_matches('*').trim().to_string();
    c.warnings = e.warnings.iter().cloned().chain(c.warnings.iter().map(|w| e.map_warning(w))).collect();
    Ok(c)
}

/// Why the built-in engine would not evaluate `deck` as written (None when it does).
pub fn builtin_shortfall(deck: &str) -> Option<String> {
    builtin_shortfall_in(deck, None)
}

fn builtin_shortfall_in(deck: &str, dir: Option<&Path>) -> Option<String> {
    if let Some(l) = compiled_models(deck) {
        return Some(format!("the deck loads compiled models ('{l}')"));
    }
    match parse_for_builtin(deck, dir) {
        Err(e) => Some(e.to_string()),
        Ok(c) => c.warnings.first().map(|w| match c.warnings.len() {
            1 => w.clone(),
            n => format!("{w} (and {} more)", n - 1),
        }),
    }
}

/// The backend `opts` resolves to for `deck`, with the reason when [`Backend::Auto`] chose
/// ngspice or had to stay on the built-in engine.
pub fn resolve_backend(deck: &str, opts: &SimOptions) -> (Backend, Option<String>) {
    if opts.backend != Backend::Auto {
        return (opts.backend, None);
    }
    match builtin_shortfall_in(deck, opts.deck_dir.as_deref()) {
        None => (Backend::Builtin, None),
        Some(why) if ngspice::detect(&opts.ngspice_exe).is_some() => {
            (Backend::Ngspice, Some(format!("auto: ngspice, because {why}")))
        }
        Some(why) => (Backend::Builtin, Some(format!("auto: ngspice not found, built-in engine although {why}"))),
    }
}

/// `deck` with the relative file of each `.include` / `.inc` / `.lib file section` line made
/// absolute against `dir`.
fn absolute_includes(deck: &str, dir: &Path) -> String {
    let mut out = String::with_capacity(deck.len());
    for line in deck.lines() {
        let t = line.trim_start();
        let lower = t.to_ascii_lowercase();
        let cmd = lower.split_whitespace().next().unwrap_or("");
        let args: Vec<&str> = t.split_whitespace().collect();
        let file_arg = match cmd {
            ".include" | ".inc" if args.len() >= 2 => Some(1),
            ".lib" if args.len() >= 3 => Some(1),
            _ => None,
        };
        match file_arg {
            Some(k) => {
                let file = args[k].trim_matches(['"', '\'']);
                let path = Path::new(file);
                if path.is_relative() {
                    let mut a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
                    a[k] = format!("\"{}\"", dir.join(path).display());
                    out += &a.join(" ");
                } else {
                    out += line;
                }
            }
            None => out += line,
        }
        out.push('\n');
    }
    out
}

/// Run `analyses` on `deck`. If `analyses` is empty the deck's own dot-analyses are used.
pub fn simulate(deck: &str, analyses: &[Analysis], opts: &SimOptions) -> Result<SimResults, SimError> {
    // ngspice does not read the Spectre language: it gets the flattened SPICE deck.
    if opts.backend != Backend::Builtin && ics_core::spectre::is_spectre(deck, None) {
        let (backend, _) = resolve_backend(deck, opts);
        if backend == Backend::Ngspice {
            let flat = frontend::expand(deck, opts.deck_dir.as_deref())?;
            let mut r = simulate(&flat.deck, analyses, &SimOptions { backend: Backend::Ngspice, ..opts.clone() })?;
            r.warnings.splice(0..0, flat.warnings);
            return Ok(r);
        }
    }
    let start = Instant::now();
    let (backend, why) = resolve_backend(deck, opts);
    if backend == Backend::Builtin
        && let Some(l) = compiled_models(deck)
    {
        return Err(SimError::Engine(format!("the built-in engine cannot load compiled models ('{l}'); select ngspice")));
    }
    // ngspice reads the deck itself (PDK subcircuits, model libraries); only the built-in
    // engine needs the parsed circuit.
    let mut circuit = match backend {
        Backend::Builtin => Some(parse_for_builtin(deck, opts.deck_dir.as_deref())?),
        _ => None,
    };
    let analyses: Vec<Analysis> = match (&mut circuit, analyses.is_empty()) {
        (_, false) => analyses.to_vec(),
        (Some(c), true) => std::mem::take(&mut c.analyses),
        (None, true) => circuit::deck_analyses(deck),
    };
    if analyses.is_empty() {
        return Err(SimError::Engine("no analyses to run".into()));
    }
    for a in &analyses {
        a.validate().map_err(SimError::Engine)?;
    }
    let mut res = SimResults::default();
    res.warnings.extend(why);
    match (backend, circuit) {
        (Backend::Builtin, Some(circuit)) => {
            res.backend = "built-in".into();

            res.warnings.extend(circuit.warnings.iter().cloned());
            let mut log: Vec<String> = res.warnings.iter().map(|w| format!("warning: {w}")).collect();
            log.push(format!("ics-sim: {} nodes, {} elements", circuit.node_count(), circuit.elements.len()));
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
        _ => {
            res.backend = ngspice::detect(&opts.ngspice_exe).unwrap_or_else(|| "ngspice".into());
            // ngspice runs in a work folder: relative library paths are taken from the deck's.
            let deck = match &opts.deck_dir {
                Some(dir) => absolute_includes(deck, dir),
                None => deck.to_string(),
            };
            let (ds, log) = ngspice::run(&opts.ngspice_exe, &deck, &analyses)?;
            res.datasets = ds;
            res.log = res.warnings.iter().map(|w| format!("{w}\n")).collect::<String>() + &log;
        }
    }
    res.elapsed_ms = start.elapsed().as_secs_f64() * 1e3;
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_includes_become_absolute_for_ngspice() {
        let d = absolute_includes("* t\n.include models.sp\n.lib 'lib/x.lib' tt\n.inc /abs/y.sp\nR1 a 0 1\n", Path::new("/w"));
        assert_eq!(d, "* t\n.include \"/w/models.sp\"\n.lib \"/w/lib/x.lib\" tt\n.inc /abs/y.sp\nR1 a 0 1\n");
    }

    #[test]
    fn builtin_reads_model_libraries_and_rejects_compiled_models() {
        let deck = "* t\n.lib \"/nonexistent/models.lib\" tt\nV1 a 0 1\nR1 a 0 1k\n.op\n.end\n";
        let e = simulate(deck, &[], &SimOptions::default()).unwrap_err().to_string();
        assert!(e.contains("line 2") && e.contains("cannot read '/nonexistent/models.lib'"), "{e}");
        let deck = "* t\n.control\npre_osdi m.osdi\n.endc\nR1 a 0 1k\n.op\n.end\n";
        let e = simulate(deck, &[], &SimOptions::default()).unwrap_err().to_string();
        assert!(e.contains("select ngspice"), "{e}");
    }

    #[test]
    fn options_initial_conditions_and_nodesets() {
        let c = circuit::parse_deck(
            "* t\n.option reltol=1e-6 abstol=1e-15 gmin=1e-13 temp=50 itl4=80 method=gear\nR1 a 0 1\n.ic v(a)=1 v(zz)=2\n.end\n",
        )
        .unwrap();
        assert_eq!(
            (c.options.reltol, c.options.lte_reltol, c.options.abstol, c.options.gmin, c.options.itl4),
            (1e-6, 1e-6, 1e-15, 1e-13, 80)
        );
        assert_eq!(c.temp, 50.0);
        assert_eq!(c.ic, vec![(1, 1.0)]);
        let w = c.warnings.join("\n");
        assert!(w.contains("option 'method'") && w.contains("unknown node 'zz'"), "{w}");

        // A latch settles where the nodeset points it.
        let latch = |q: f64| {
            let deck = format!(
                "* latch\n.model nm nmos (vto=0.5 kp=100u)\n.model pm pmos (vto=-0.5 kp=40u)\nV1 vdd 0 1.8\n\
                 M1 q qb vdd vdd pm w=2u l=1u\nM2 q qb 0 0 nm w=1u l=1u\nM3 qb q vdd vdd pm w=2u l=1u\nM4 qb q 0 0 nm w=1u l=1u\n\
                 .nodeset v(q)={q} v(qb)={}\n.op\n.end\n",
                1.8 - q
            );
            let r = simulate(&deck, &[], &SimOptions::default()).unwrap();
            r.datasets[0].get("v(q)").unwrap().re[0]
        };
        assert!(latch(1.8) > 1.7 && latch(0.0) < 0.1, "{} {}", latch(1.8), latch(0.0));

        // .ic holds the node for the initial point; uic starts from it without one.
        for uic in ["", " uic"] {
            let deck = format!("* rc\nV1 in 0 0\nR1 in out 1k\nC1 out 0 1n\n.ic v(out)=1\n.tran 10n 2u{uic}\n.end\n");
            let r = simulate(&deck, &[], &SimOptions::default()).unwrap();
            let v = &r.datasets[0].get("v(out)").unwrap().re;
            assert!(
                (v[0] - 1.0).abs() < 1e-6 && (v.last().unwrap() - (-2.0f64).exp()).abs() < 2e-3,
                "{uic}: {} {}",
                v[0],
                v.last().unwrap()
            );
        }
    }

    #[test]
    fn bsim4_decks_run_on_the_builtin_engine() {
        let deck = "* b4\n.model n1 nmos level=54 toxe=4e-9 vth0=0.4 u0=0.04 vsat=1e5 ndep=2e17\n\
                    .model p1 pmos level=54 toxe=4e-9 vth0=-0.4 u0=0.015 vsat=8e4 ndep=2e17\n\
                    V1 vdd 0 1.2\nV2 in 0 0.6\nM1 out in vdd vdd p1 w=2u l=0.1u nf=2\nM2 out in 0 0 n1 w=1u l=0.1u sa=1u\n.op\n.end\n";
        let r = simulate(deck, &[], &SimOptions::default()).unwrap();
        let ds = &r.datasets[0];
        let out = ds.get("v(out)").unwrap().re[0];
        assert!(out > 0.0 && out < 1.2, "{out}");
        let m2 = ds.devices.iter().find(|d| d.name == "m2").unwrap();
        let id = m2.values.iter().find(|(k, _)| k == "id").unwrap().1;
        assert!(id > 1e-9 && id < 1e-3, "{id}");
        let w = r.warnings.join("\n");
        assert!(w == "m2: instance parameters not evaluated: sa", "{w}");
        // Auto stays on the built-in engine when it evaluates the whole deck.
        assert!(builtin_shortfall(deck).is_some());
        assert_eq!(builtin_shortfall(&deck.replace(" sa=1u", "")), None);
        let tran = deck.replace(".op", ".tran 10p 1n").replace("V2 in 0 0.6", "V2 in 0 PULSE(0 1.2 0.1n 20p 20p 0.3n 1n)");
        let r = simulate(&tran, &[], &SimOptions::default()).unwrap();
        let out = &r.datasets[0].get("v(out)").unwrap().re;
        assert!(out.iter().cloned().fold(0.0, f64::max) > 1.0 && out.iter().cloned().fold(2.0, f64::min) < 0.2, "{out:?}");
        let e = simulate(&deck.replace("nmos level=54", "nmos level=54 lint=0.06u"), &[], &SimOptions::default()).unwrap_err();
        assert!(e.to_string().contains("m2: BSIM4: effective channel"), "{e}");
    }

    #[test]
    fn hierarchical_decks_simulate_on_the_builtin_engine() {
        // A divider written as a parameterised subcircuit: 1.8 V * 3k / (1k + 3k).
        let deck = "* div\n.param vdd=1.8 rt=1k\n.subckt div top mid bot ratio=1\nR1 top mid {rt}\nR2 mid bot {rt*ratio}\n.ends\n\
                    V1 a 0 {vdd}\nX1 a m 0 div ratio=3\n.op\n.end\n";
        let r = simulate(deck, &[], &SimOptions::default()).unwrap();
        let v = r.datasets[0].vectors.iter().find(|v| v.name == "v(m)").unwrap();
        assert!((v.re[0] - 1.35).abs() < 1e-6, "{:?}", v.re);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let e = simulate(&deck.replace("{rt*ratio}", "{rt*nope}"), &[], &SimOptions::default()).unwrap_err().to_string();
        assert!(e.contains("netlist line 5") && e.contains("nope"), "{e}");
    }

    #[test]
    fn auto_stays_builtin_for_supported_decks() {
        let opts = SimOptions { backend: Backend::Auto, ngspice_exe: "/nonexistent/ngspice".into(), deck_dir: None };
        let r = simulate("* t\nV1 a 0 1\nR1 a 0 1k\n.op\n.end\n", &[], &opts).unwrap();
        assert_eq!(r.backend, "built-in");
        assert!(r.warnings.is_empty());
        let deck = "* t\nV1 a 0 1\nR1 a 0 1k\n.option reltol=1e-5 method=gear\n.op\n.end\n";
        assert_eq!(resolve_backend(deck, &opts).0, Backend::Builtin);
        let r = simulate(deck, &[], &opts).unwrap();
        assert!(r.warnings[0].contains("ngspice not found"), "{:?}", r.warnings);
        assert!(r.log.contains("warning: line 4: option 'method' is ignored"), "{}", r.log);
        assert!(builtin_shortfall("* t\n.control\npre_osdi x.osdi\n.endc\nR1 a 0 1\n.end\n").is_some());
    }

    #[test]
    fn ngspice_decks_are_not_parsed_by_the_builtin_engine() {
        let deck = "* pdk\n.lib \"/pdk/models.lib\" tt\nXM1 d g 0 0 sky130_fd_pr__nfet_01v8 w=1 l=0.15\n\
                    V1 d 0 1.8\nV2 g 0 0.9\n.control\n.tran 1n 2n\n.endc\n.op\n.dc V2 0 1.8 0.1\n.end\n.tran 1 2\n";
        assert_eq!(
            circuit::deck_analyses(deck),
            vec![Analysis::Op, Analysis::Dc { source: "V2".into(), start: "0".into(), stop: "1.8".into(), step: "0.1".into() }]
        );
        let opts = SimOptions { backend: Backend::Ngspice, ngspice_exe: "/nonexistent/ngspice".into(), deck_dir: None };
        let e = simulate(deck, &[], &opts).unwrap_err();
        assert!(!matches!(e, SimError::Parse { .. }), "{e}");
    }
}
