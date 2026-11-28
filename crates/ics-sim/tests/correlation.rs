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
    // Substrate current, GIDL / GISL and gate tunnelling.
    let leak = "alpha0=0.074 alpha1=0.005 beta0=30 agidl=2e-4 bgidl=2.1e9 cgidl=2e-4 egidl=0.8 igcmod=1 igbmod=1 \
        pigcd=1 aigc=0.012 bigc=0.0028 cigc=0.002 aigsd=0.0087 bigsd=0.0012 cigsd=0.002 toxref=1.25e-9 aigbacc=0.012 \
        bigbacc=0.0028 cigbacc=0.002 aigbinv=0.014 bigbinv=0.004 cigbinv=0.004 dlcig=2.5e-9";
    format!(
        ".model n45 nmos {common} vth0=0.46893 ndep=3.4e18 vfb=-0.55 u0=0.049 ua=6e-10 ub=1.2e-18 vsat=130000 pclm=0.04\n\
         .model p45 pmos {common} vth0=-0.49158 ndep=2.44e18 vfb=0.55 u0=0.0021 ua=2e-9 ub=0.5e-18 vsat=90000 pclm=0.12\n\
         .model n45l nmos {common} vth0=0.46893 ndep=3.4e18 vfb=-0.55 u0=0.049 ua=6e-10 ub=1.2e-18 vsat=130000 pclm=0.04 {leak}\n"
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
            "bjt_common_emitter",
            "* CE amplifier with Early effect, knee, leakage, charges, series R and a PNP diode\nV1 vcc 0 5\n\
             Vin in 0 dc 0.68 ac 1 sin(0.68 5m 1meg)\nRb in b 1k\nQ1 c b 0 qn\nRc vcc c 2k\nQ2 x x vcc qp\nRx x 0 10k\n\
             .model qn npn (is=1e-15 bf=150 br=2 vaf=60 ikf=20m cje=1p cjc=0.5p tf=0.3n rb=50 rc=10 re=2 ise=1e-14 ne=1.6)\n\
             .model qp pnp (is=2e-16 bf=60 vaf=40)\n.op\n.ac dec 10 1k 1g\n.tran 5n 4u\n.end\n"
                .into(),
            1e-2,
        ),
        (
            "bjt_temperature",
            "* vbe over temperature\n.temp 100\nI1 0 b 10u\nQ1 b b 0 qn 4\nQ2 c b 0 qp_like\nV2 c 0 1\n\
             .model qn npn (is=2e-17 bf=120 xtb=1.5 ise=1e-15 ne=1.5 cje=1p vje=0.8)\n.model qp_like npn (is=1e-16 bf=80 eg=1.17 xti=3.5)\n\
             .op\n.end\n"
                .into(),
            1e-3,
        ),
        (
            "bandgap_hot",
            "* bandgap\n.temp 100\nE1 out 0 a b 1e4\nR1 out a 51k\nQ1 a a 0 npn\nR2 out b 51k\nR3 b c2 5k\nQ2 c2 c2 0 npn 8\n\
             .model npn npn (is=1e-16 bf=100 br=1 vaf=50 rb=100 re=1 rc=10 cje=0.5p cjc=0.3p tf=0.2n)\n.op\n.end\n"
                .into(),
            1e-3,
        ),
        (
            "controlled_sources",
            "* E F G H\nV1 in 0 dc 1 ac 1 sin(1 0.5 1meg)\nVs in s 0\nRs s 0 1k\nE1 e 0 in 0 2\nRe e 0 1k\nG1 0 g in 0 1m\nRg g 0 2k\n\
             F1 0 f Vs 3\nRf f 0 1k\nH1 h 0 Vs 500\nRh h 0 1k\n.op\n.ac dec 5 1k 10meg\n.tran 10n 2u\n.end\n"
                .into(),
            1e-3,
        ),
        ("rc_ic", "* rc ic\nV1 in 0 0\nR1 in out 1k\nC1 out 0 1n\n.ic v(out)=1\n.tran 10n 5u\n.end\n".into(), 1e-3),
        (
            "rc_uic",
            "* rc uic\nV1 in 0 0.5\nR1 in out 1k\nC1 out 0 1n\nR2 out x 2k\nC2 x 0 2n\n.ic v(out)=1 v(x)=-0.3\n.tran 10n 8u uic\n.end\n".into(),
            1e-3,
        ),
        (
            "bsim4_latch_nodeset",
            format!(
                "* latch\nV1 vdd 0 1.8\nM1 q qb vdd vdd p45 w=2u l=45n\nM2 q qb 0 0 n45 w=0.5u l=45n\nM3 qb q vdd vdd p45 w=2u l=45n\n\
                 M4 qb q 0 0 n45 w=0.5u l=45n\n{b4}.nodeset v(q)=0 v(qb)=1.8\n.op\n.end\n"
            ),
            0.01,
        ),
        (
            "bsim4_noise",
            format!(
                "* bsim4 noise\nVDD vdd 0 1.8\nVIN in 0 DC 1.0 AC 1\nRD vdd out 5k\nM1 out in 0 0 n45 w=2u l=0.1u nf=2\nCL out 0 5f\n{b4}\
                 .noise v(out) VIN dec 5 1 10g\n.end\n"
            ),
            1e-3,
        ),
        (
            "bsim4_junctions_hot",
            format!(
                "* bsim4 junctions\n.temp 120\nVD d 0 0\nVG g 0 0\nM1 d g 0 0 n45 w=1u l=45n ad=0.2p as=0.2p pd=2.4u ps=2.4u nf=3\n{b4}\
                 .dc VD -1.0 0.5 0.02\n.end\n"
            ),
            1e-3,
        ),
        (
            "bsim4_leakage_id_vg",
            format!("* leakage\nVD d 0 1.2\nVG g 0 0\nVB b 0 0\nM1 d g 0 b n45l w=1u l=45n nf=2\n{b4}.dc VG -0.8 1.2 0.05\n.end\n"),
            1e-3,
        ),
        (
            "bsim4_leakage_reverse",
            format!("* leakage\nVD d 0 0\nVG g 0 1\nVB b 0 -0.3\nM1 d g 0 b n45l w=1u l=45n\n{b4}.dc VD -1 1.2 0.05\n.end\n"),
            1e-3,
        ),
        (
            "dspf_post_layout",
            format!(
                "* DSPF from parasitic extraction\n*|DSPF 1.3\n*|PROGRAM \"Calibre xRC\"\n*|DIVIDER /\n*|DELIMITER :\n.SUBCKT inv VDD VSS A Y\n*|GROUND_NET VSS\n\
                 *|NET Y 1.6e-15PF\n*|P (Y B 0 1.0 2.0)\n*|I (MN:d MN d B 0 1.0 1.5)\nR1 Y Y:1 12.5\nR2 Y:1 MN:d 8.0\nR3 Y:1 MP:d 8.0\n\
                 C1 Y VSS 0.5e-15\nC2 Y:1 VSS 0.3e-15\n*|NET A 1.1e-15PF\nR4 A MN:g 30\nR5 A MP:g 30\nC5 A VSS 0.4e-15\nCc1 A Y 0.1e-15\n\
                 *Instance Section\nMN MN:d MN:g VSS VSS n45 L=0.1u W=1u AD=0.1p AS=0.1p PD=1.2u PS=1.2u\n\
                 MP MP:d MP:g VDD VDD p45 L=0.1u W=2u AD=0.2p AS=0.2p PD=2.2u PS=2.2u\n.ENDS\n{b4}VDD vdd 0 1.8\nVIN in 0 PULSE(0 1.8 0.1n 20p 20p 0.5n 1n)\n\
                 X1 vdd 0 in out inv\nCL out 0 2f\n.tran 1p 2n\n.end\n"
            ),
            0.06,
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
            "behavioural_sources",
            "* B sources\nV1 in 0 dc 1 ac 1 sin(1 0.5 1meg)\nVs in s 0\nRs s 0 1k\n\
             B1 b1 0 V=v(in)*v(in) + 0.5*v(in, s) + 0.1*sin(2*pi*2meg*time)\nRb1 b1 0 1k\n\
             B2 0 b2 I=1m*tanh(v(in)) + 0.2*i(Vs)\nRb2 b2 0 1k\n.op\n.ac dec 5 1k 10meg\n.tran 10n 2u\n.end\n"
                .into(),
            1e-3,
        ),
        (
            "mutual_inductance",
            "* transformer\nV1 in 0 dc 0 ac 1 pulse(0 1 0 10n 10n 1u 2u)\nR1 in a 50\nL1 a 0 10u\nL2 b 0 40u\nK1 L1 L2 0.9\n\
             R2 b 0 200\n.ac dec 10 10k 100meg\n.tran 5n 4u\n.end\n"
                .into(),
            1e-2,
        ),
        (
            "switches",
            "* S and W\nV1 a 0 1\nVc ctl 0 dc 0.2 pulse(0 1 0.5u 10n 10n 1u 2u)\nS1 a out ctl 0 sw\nRo out 0 1k\n\
             Vs ctl m 0\nRm m 0 500\nW1 a out2 Vs csw\nRo2 out2 0 1k\n\
             .model sw sw vt=0.5 ron=10 roff=1meg\n.model csw csw it=1m ron=5 roff=10meg\n\
             .dc Vc 0 0.9 0.15\n.tran 5n 3u\n.end\n"
                .into(),
            1e-2,
        ),
        (
            "switch_hysteresis",
            "* S with hysteresis, driven by a triangle\nV1 a 0 1\nVc ctl 0 dc 0 pwl(0 0 1u 1 2u 0 3u 1 4u 0)\nS1 a out ctl 0 swh\nRo out 0 1k\n\
             .model swh sw vt=0.5 vh=0.2 ron=10 roff=1meg\n.tran 5n 4u\n.end\n"
                .into(),
            // ngspice switches in one step, the built-in engine over a few ns: one point
            // inside each transition differs.
            5e-2,
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
        // CI installs ngspice on every platform and sets ICS_REQUIRE_NGSPICE, so the suite
        // cannot pass there by skipping.
        assert!(std::env::var_os("ICS_REQUIRE_NGSPICE").is_none(), "ngspice not found, and ICS_REQUIRE_NGSPICE is set");
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

/// The built-in engine damps trapezoidal ringing: in the free-running BSIM4 ring the supply
/// current's step-to-step alternation stays small instead of growing (no ngspice needed).
#[test]
fn ring_oscillator_supply_current_does_not_ring() {
    let (_, deck, _) = suite().into_iter().find(|(n, ..)| *n == "bsim4_ring_osc").unwrap();
    let opts = SimOptions { backend: ics_core::Backend::Builtin, ..Default::default() };
    let r = ics_sim::simulate(&deck, &[], &opts).unwrap();
    let ds = &r.datasets[0];
    let i = &ds.vectors.iter().find(|v| v.name.eq_ignore_ascii_case("i(v1)")).unwrap().re;
    let peak = i.iter().fold(0f64, |a, b| a.max(b.abs()));
    // Alternation over the last nanosecond: the distance of each point from its neighbours' mean.
    let tail = &i[i.len() * 2 / 3..];
    let ring = tail.windows(3).map(|w| (w[1] - 0.5 * (w[0] + w[2])).abs()).fold(0f64, f64::max);
    assert!(ring < 0.03 * peak, "ringing {ring:e} A of a {peak:e} A peak");
}
