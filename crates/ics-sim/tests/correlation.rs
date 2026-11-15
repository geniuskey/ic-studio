//! Golden correlation suite: each deck runs on the built-in engine and on ngspice (the
//! reference) and must agree within its tolerance. Skipped when ngspice is not installed;
//! point `ICS_NGSPICE` at an ngspice (console) executable to run it.

use ics_core::netlist::DEFAULT_MODELS;
use ics_sim::{SimOptions, correlate, ngspice};

fn ngspice_exe() -> Option<String> {
    let exe = std::env::var("ICS_NGSPICE").unwrap_or_else(|_| ngspice::default_exe());
    ngspice::detect(&exe).map(|_| exe)
}

/// A 45 nm-class BSIM4 card (PTM-style parameters) for both polarities.
fn bsim4_models() -> String {
    let common = "level=54 toxe=1.25e-9 toxp=1e-9 toxm=1.25e-9 epsrox=3.9 wint=5e-9 lint=3.75e-9 k1=0.4 k2=0 k3=0 \
        dvt0=1 dvt1=2 dvt2=0 dsub=0.1 minv=0.05 dvtp0=1e-9 dvtp1=0.1 xj=1.4e-8 ngate=2e20 nsd=2e20 cdsc=0 voff=-0.13 \
        nfactor=1.6 eta0=0.0125 etab=0 a0=1 keta=0.04 pdiblc1=0.001 pdiblc2=0.001 pdiblcb=-0.005 drout=0.5 \
        pvag=1e-20 delta=0.01 pscbe1=8.14e8 pscbe2=1e-7 fprout=0.2 pdits=0.08 pditsd=0.23 pditsl=2.3e6 rdsw=155 \
        kt1=-0.11 kt2=0.022 at=33000 ute=-1.5 ua1=4.31e-9 ub1=7.61e-18 uc1=-5.6e-11";
    format!(
        ".model n45 nmos {common} vth0=0.46893 ndep=3.4e18 vfb=-0.55 u0=0.049 ua=6e-10 ub=1.2e-18 vsat=130000 pclm=0.04\n\
         .model p45 pmos {common} vth0=-0.49158 ndep=2.44e18 vfb=0.55 u0=0.0021 ua=2e-9 ub=0.5e-18 vsat=90000 pclm=0.12\n"
    )
}

/// (name, deck, relative tolerance)
fn suite() -> Vec<(&'static str, String, f64)> {
    let m = DEFAULT_MODELS;
    let b4 = bsim4_models();
    vec![
        (
            "bsim4_nmos_id_vd",
            format!("* id-vd\nVD d 0 1\nVG g 0 1\nVB b 0 0\nM1 d g 0 b n45 w=0.5u l=45n nf=2\n{b4}.dc VD 0 1 0.05\n.end\n"),
            1e-3,
        ),
        (
            "bsim4_nmos_id_vg_body",
            format!("* id-vg\nVD d 0 0.05\nVG g 0 0\nVB b 0 -0.4\nM1 d g 0 b n45 w=1u l=0.1u\n{b4}.dc VG 0 1 0.05\n.end\n"),
            1e-3,
        ),
        (
            "bsim4_pmos_hot",
            format!("* pmos 85C\n.temp 85\nVD d 0 -1\nVG g 0 -1\nM1 d g 0 0 p45 w=1u l=45n\n{b4}.dc VD 0 -1 -0.05\n.end\n"),
            1e-3,
        ),
        (
            "bsim4_cs_ac",
            format!(
                "* bsim4 cs\nVDD vdd 0 1.8\nVIN in 0 DC 1.1 AC 1\nRD vdd out 2k\nM1 out in 0 0 n45 w=2u l=0.1u nf=2 ad=0.2p as=0.2p pd=2.4u ps=2.4u\n\
                 CL out 0 5f\n{b4}.op\n.ac dec 10 1meg 1t\n.end\n"
            ),
            0.01,
        ),
        (
            "bsim4_ring_osc",
            format!(
                "* bsim4 ring, kicked\nV1 vdd 0 1.8\nI1 0 n0 PULSE(0 200u 0 10p 10p 20p 10)\nM0p n1 n0 vdd vdd p45 w=2u l=45n\nM0n n1 n0 0 0 n45 w=0.5u l=45n\nM1p n2 n1 vdd vdd p45 w=2u l=45n\nM1n n2 n1 0 0 n45 w=0.5u l=45n\nM2p n3 n2 vdd vdd p45 w=2u l=45n\nM2n n3 n2 0 0 n45 w=0.5u l=45n\nM3p n4 n3 vdd vdd p45 w=2u l=45n\nM3n n4 n3 0 0 n45 w=0.5u l=45n\nM4p n0 n4 vdd vdd p45 w=2u l=45n\nM4n n0 n4 0 0 n45 w=0.5u l=45n\n{b4}.tran 1p 3n\n.end\n"
            ),
            0.06,
        ),
        (
            "bsim4_inverter_dc",
            format!(
                "* bsim4 inv\nV1 vdd 0 1.8\nV2 in 0 0\nM1 out in vdd vdd p45 w=2u l=45n\nM2 out in 0 0 n45 w=0.5u l=45n\n{b4}\
                 .dc V2 0 1.8 0.01\n.end\n"
            ),
            3e-3,
        ),
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
