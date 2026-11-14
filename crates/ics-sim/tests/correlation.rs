//! Golden correlation suite: each deck runs on the built-in engine and on ngspice (the
//! reference) and must agree within its tolerance. Skipped when ngspice is not installed;
//! point `ICS_NGSPICE` at an ngspice (console) executable to run it.

use ics_core::netlist::DEFAULT_MODELS;
use ics_sim::{SimOptions, correlate, ngspice};

fn ngspice_exe() -> Option<String> {
    let exe = std::env::var("ICS_NGSPICE").unwrap_or_else(|_| ngspice::default_exe());
    ngspice::detect(&exe).map(|_| exe)
}

/// (name, deck, relative tolerance)
fn suite() -> Vec<(&'static str, String, f64)> {
    let m = DEFAULT_MODELS;
    vec![
        ("divider", "* divider\nV1 a 0 10\nR1 a b 1k\nR2 b 0 3k\n.op\n.dc V1 0 10 1\n.end\n".into(), 1e-6),
        ("rc", "* rc\nV1 in 0 DC 0 AC 1 PULSE(0 1 0 1n 1n 1 2)\nR1 in out 1k\nC1 out 0 1n\n.ac dec 10 1k 100meg\n.tran 10n 5u\n.end\n".into(), 0.01),
        ("rlc", "* rlc\nV1 in 0 DC 0 AC 1\nR1 in a 50\nL1 a out 1u\nC1 out 0 1n\nR2 out 0 1k\n.ac dec 20 100k 100meg\n.end\n".into(), 0.01),
        ("diode", format!("* diode\nV1 a 0 5\nR1 a k 1k\nD1 k 0 dmod\n{m}\n.op\n.dc V1 0 5 0.1\n.end\n"), 0.01),
        (
            "inverter_dc",
            format!("* inv\nV1 vdd 0 1.8\nV2 in 0 0\nM1 out in vdd vdd pmos w=2u l=180n\nM2 out in 0 0 nmos w=1u l=180n\n{m}\n.dc V2 0 1.8 0.01\n.end\n"),
            0.03,
        ),
        (
            "inverter_dc_hot",
            format!("* inv 125C\n.temp 125\nV1 vdd 0 1.8\nV2 in 0 0\nM1 out in vdd vdd pmos w=2u l=180n\nM2 out in 0 0 nmos w=1u l=180n\n{m}\n.dc V2 0 1.8 0.01\n.end\n"),
            0.03,
        ),
        (
            "cs_amp",
            format!(
                "* cs\nVDD vdd 0 1.8\nVIN in 0 DC 0.6 AC 1\nRD vdd out 10k\nM1 out in 0 0 nmos w=2u l=1u\nCL out 0 100f\n{m}\n.op\n.ac dec 10 1k 10g\n.end\n"
            ),
            0.03,
        ),
        (
            "cs_noise",
            format!(
                "* cs noise\nVDD vdd 0 1.8\nVIN in 0 DC 0.6 AC 1\nRD vdd out 10k\nM1 out in 0 0 nmos w=2u l=1u\nCL out 0 100f\n{m}\n.noise v(out) VIN dec 10 1k 1g\n.end\n"
            ),
            0.05,
        ),
        (
            "inverter_tran",
            format!(
                "* inv tran\nV1 vdd 0 1.8\nV2 in 0 PULSE(0 1.8 1n 50p 50p 2n 4n)\nM1 out in vdd vdd pmos w=2u l=180n ad=1p as=1p pd=4u ps=4u\n\
                 M2 out in 0 0 nmos w=1u l=180n ad=0.5p as=0.5p pd=3u ps=3u\nC1 out 0 10f\n{m}\n.tran 10p 8n\n.end\n"
            ),
            0.1,
        ),
    ]
}

#[test]
fn builtin_matches_ngspice() {
    let Some(exe) = ngspice_exe() else {
        eprintln!("ngspice not found: skipping the correlation suite (set ICS_NGSPICE)");
        return;
    };
    let opts = SimOptions { ngspice_exe: exe, ..Default::default() };
    let mut failed = vec![];
    for (name, deck, tol) in suite() {
        match correlate(&deck, &[], &opts) {
            Ok((_, _, c)) => {
                eprintln!("[{name}] {}", c.table());
                if !c.passes(tol) || c.diffs.is_empty() {
                    failed.push(format!("{name}: worst {:?} (tol {tol})", c.worst()));
                }
            }
            Err(e) => failed.push(format!("{name}: {e}")),
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}
