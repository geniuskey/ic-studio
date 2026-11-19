//! Image sensor pixel characterisation by simulation: node capacitances, conversion gain,
//! source follower gain, charge transfer and full well of a pixel found in a layout
//! ([`ics_core::pixel_circuit`]).
//!
//! The pixel alone is drawn with the charge injection test bench
//! ([`Stimulus::ChargeInjection`]): FD and the photodiodes are reset, RST and TX turn off, a
//! known charge is taken out of FD and then out of the first photodiode, and TX transfers it.
//! With the device models of the technology (PDK models when it has them), C = Q / ΔV takes in
//! every junction, overlap and gate capacitance on the node and the wire capacitance from the
//! layout. The transfer is compared with a run without the photodiode charge (same TX
//! feedthrough and channel charge): what the photodiode still holds beyond it is lag, the rest
//! went to FD. (FD's own step is not used: the TX channel charge it gets back at turn-off
//! depends on its voltage.) A second run ([`Stimulus::FullWell`]) fills the photodiode with
//! light until it saturates, and a noise analysis ([`Stimulus::NoiseBias`]) gives the readout
//! noise at the column.

use ics_core::Tech;
use ics_core::extract::Extraction;
use ics_core::pixel_circuit::{
    FD_WINDOW, FULL_WELL_AT, INJECT_ELECTRONS, NOISE_BAND, PD_WINDOW, PixelCircuit, Q_E, Stimulus, TRANSFER_WINDOW, Testbench,
    charge_net, pixel_only, schematic,
};

use crate::{SimOptions, simulate};

/// Charge transfer, full well and dark current of one photodiode of a pixel.
#[derive(Debug, Clone, PartialEq)]
pub struct PhotodiodeMetrics {
    /// Extracted name ("D1") and its node net.
    pub name: String,
    pub node: String,
    /// Voltage of the charge node after reset (V): the pinning voltage of a pinned photodiode.
    pub v_reset: f64,
    /// Capacitance of the charge node (F).
    pub c: f64,
    /// Of [`INJECT_ELECTRONS`] in the photodiode: those that left it for FD, and those it still
    /// holds after the transfer (lag: left behind, or spilled back when TX turns off), both
    /// against a run without them.
    /// Pinned photodiodes with a TG only: a plain junction shares its charge with FD.
    pub transferred_e: Option<f64>,
    pub lag_e: Option<f64>,
    /// Full well capacity (electrons) and the saturated voltage; `saturated` is false when the
    /// voltage still moved at the end of the run.
    pub full_well_e: f64,
    pub v_saturated: f64,
    pub saturated: bool,
    /// Dark current (A) at the bench temperature and the temperature step (K) that doubles it,
    /// when the technology gives dark current densities.
    pub dark: Option<(f64, f64)>,
}

/// Noise of a pixel's readout.
#[derive(Debug, Clone, PartialEq)]
pub struct NoiseMetrics {
    /// Reset (kTC) noise of FD, electrons rms: sqrt(k T C_FD) / q. Correlated double sampling
    /// removes it in a 4T pixel; a pinned photodiode resets without it.
    pub ktc_e: f64,
    /// Readout noise at the column over [`NOISE_BAND`] with [`Testbench::ccol`] on the column
    /// line (µV rms) and referred to FD charge by the column conversion gain (e⁻ rms), one
    /// sample; correlated double sampling of uncorrelated noise multiplies it by √2.
    pub readout_uv: f64,
    pub readout_e: f64,
    pub ccol: f64,
}

/// Characterisation of one pixel.
#[derive(Debug, Clone, PartialEq)]
pub struct PixelMetrics {
    /// FD voltage after reset (V) and its capacitance (F).
    pub v_fd_reset: f64,
    pub c_fd: f64,
    /// Conversion gain at FD and at the column line (µV per electron).
    pub cg_fd_uv: f64,
    pub cg_col_uv: f64,
    /// Column voltage step over FD voltage step.
    pub sf_gain: f64,
    /// Every photodiode of the pixel (two or more in a shared pixel).
    pub pds: Vec<PhotodiodeMetrics>,
    /// Whether the photodiode was simulated as pinned.
    pub pinned: bool,
    /// Bench temperature (°C).
    pub temp: f64,
    pub noise: NoiseMetrics,
    /// The charge injection deck simulated.
    pub deck: String,
}

impl PixelMetrics {
    pub fn summary(&self) -> String {
        let mut s = format!(
            "FD: reset {:.3} V, C = {:.3} fF, CG = {:.1} uV/e- at FD, {:.1} uV/e- at the column (SF gain {:.3})\n",
            self.v_fd_reset,
            self.c_fd * 1e15,
            self.cg_fd_uv,
            self.cg_col_uv,
            self.sf_gain
        );
        let n = &self.noise;
        s += &format!(
            "Noise: kTC at FD {:.1} e- rms (removed by CDS); readout {:.1} uV rms = {:.2} e- rms per sample ({:.2} e- with CDS) with {} pF on the column, {} to {}\n",
            n.ktc_e,
            n.readout_uv,
            n.readout_e,
            n.readout_e * std::f64::consts::SQRT_2,
            n.ccol * 1e12,
            hz(NOISE_BAND.0),
            hz(NOISE_BAND.1)
        );
        for p in &self.pds {
            if let Some((i, doubling)) = p.dark {
                s += &format!(
                    "{} dark current: {:.2} aA = {:.1} e-/s at {} C (doubles every {doubling:.1} C)\n",
                    p.name,
                    i * 1e18,
                    i / Q_E,
                    self.temp
                );
            }
            let transfer = match (p.transferred_e, p.lag_e) {
                (Some(t), Some(l)) => format!("; transfer of {INJECT_ELECTRONS:.0} e-: {t:.1} e- out, lag {l:.1} e-"),
                _ => String::new(),
            };
            s += &format!(
                "{} ({}{}): reset {:.3} V, C = {:.3} fF{transfer}; full well {:.0} e- (saturates at {:.3} V{})\n",
                p.name,
                p.node,
                if self.pinned { ", pinned" } else { "" },
                p.v_reset,
                p.c * 1e15,
                p.full_well_e,
                p.v_saturated,
                if p.saturated { "" } else { ", not yet saturated" }
            );
        }
        s
    }
}

/// A frequency with an SI prefix: `1 kHz`, `1 GHz`.
fn hz(f: f64) -> String {
    let (v, p) = [(1e9, "G"), (1e6, "M"), (1e3, "k")].into_iter().find(|(s, _)| f >= *s).map_or((f, ""), |(s, p)| (f / s, p));
    format!("{v} {p}Hz")
}

/// Value of `v` at time `t` (linear interpolation).
fn at(time: &[f64], v: &[f64], t: f64) -> Option<f64> {
    let i = time.partition_point(|&x| x < t);
    match i {
        0 => v.first().copied(),
        _ if i >= time.len() => v.last().copied(),
        _ => {
            let (t0, t1) = (time[i - 1], time[i]);
            let f = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
            Some(v[i - 1] + f * (v[i] - v[i - 1]))
        }
    }
}

/// The transient waveforms of a deck, sampled by net name.
struct Run {
    time: Vec<f64>,
    ds: crate::Dataset,
}

impl Run {
    fn new(deck: &str, opts: &SimOptions) -> Result<Run, String> {
        let res = simulate(deck, &[], opts).map_err(|e| e.to_string())?;
        let ds = res.dataset("tran").ok_or("the simulation gave no transient result")?.clone();
        let time = ds.scale_vector().ok_or("no time vector")?.re.clone();
        Ok(Run { time, ds })
    }

    fn v(&self, net: &str, t: f64) -> Result<f64, String> {
        let v = self.ds.get(&format!("v({net})")).ok_or_else(|| format!("no voltage of net {net} in the result"))?;
        at(&self.time, &v.re, t).ok_or_else(|| format!("no sample of v({net})"))
    }
}

/// Simulate pixel `k` of `pc` (from extraction `x` of a layout in `tech`) with the charge
/// injection and full-well benches and measure it. `bench` gives the supply, column load and
/// photodiode ([`Testbench::for_tech`]); its stimulus and photocurrent are replaced.
pub fn characterize(
    x: &Extraction,
    pc: &PixelCircuit,
    k: usize,
    tech: &Tech,
    bench: &Testbench,
    corner: &str,
    opts: &SimOptions,
) -> Result<PixelMetrics, String> {
    let (sub, one) = pixel_only(x, pc, k).ok_or_else(|| format!("no pixel {}", k + 1))?;
    let p = &one.pixels[0];
    if p.sf.is_none() {
        return Err("the pixel has no source follower".into());
    }
    let deck_for = |st: Stimulus, iph: f64| {
        let b = Testbench { stimulus: st, iph, ..bench.clone() };
        let (sch, setup) = schematic(&sub, &one, Some(&b));
        let nl = ics_core::netlist::netlist_for(&sch, tech, corner);
        (nl.deck(&format!("pixel {} {st:?}", k + 1), &setup), b)
    };
    let (deck, b) = deck_for(Stimulus::ChargeInjection { pd: Some(0) }, 0.0);
    let run = Run::new(&deck, opts)?;
    let name = |n: usize| sub.nets[n].name.split_whitespace().collect::<Vec<_>>().join("_");
    let q = INJECT_ELECTRONS * Q_E;
    let fd = name(p.fd);
    let col = p.column.or(p.sf_out).map(name).ok_or("the pixel has no column line")?;
    let (fd0, fd1) = (run.v(&fd, FD_WINDOW.0)?, run.v(&fd, FD_WINDOW.1)?);
    let (col0, col1) = (run.v(&col, FD_WINDOW.0)?, run.v(&col, FD_WINDOW.1)?);
    let dv = fd0 - fd1;
    if dv.abs() < 1e-9 {
        return Err(format!("FD ({fd}) did not move with the injected charge"));
    }
    let c_fd = q / dv.abs();
    let sf_gain = (col0 - col1) / dv;
    let cg_fd_uv = Q_E / c_fd * 1e6;
    let pinned = p.photodiodes.iter().any(|&(d, _)| sub.photodiodes[d].n_type) && b.pinning().is_some();
    // The full-well run fills every photodiode at once, at a rate set by the first one.
    let mut pds = Vec::new();
    let mut fw: Option<Run> = None;
    let mut dark_run: Option<Run> = None;
    for (j, &(d, tg)) in p.photodiodes.iter().enumerate() {
        let d = &sub.photodiodes[d];
        let qn = charge_net(&sub, d, Some(&b));
        let inj = if j == 0 { None } else { Some(Run::new(&deck_for(Stimulus::ChargeInjection { pd: Some(j) }, 0.0).0, opts)?) };
        let run_j = inj.as_ref().unwrap_or(&run);
        let (a, c_after) = (run_j.v(&qn, PD_WINDOW.0)?, run_j.v(&qn, PD_WINDOW.1)?);
        if (a - c_after).abs() < 1e-9 {
            return Err(format!("the photodiode ({qn}) did not move with the injected charge"));
        }
        let c = q / (a - c_after).abs();
        let (transferred_e, lag_e) = match tg {
            // A plain junction shares its charge with FD rather than transferring it.
            Some(_) if !(pinned && d.n_type) => (None, None),
            Some(_) => {
                // What the photodiode holds after the transfer beyond the dark run is lag
                // (left behind or spilled back); the rest left it for FD.
                if dark_run.is_none() {
                    dark_run = Some(Run::new(&deck_for(Stimulus::ChargeInjection { pd: None }, 0.0).0, opts)?);
                }
                let dark = dark_run.as_ref().expect("just made");
                let left = dark.v(&qn, TRANSFER_WINDOW.1)? - run_j.v(&qn, TRANSFER_WINDOW.1)?;
                let lag = (c * left / Q_E).clamp(0.0, INJECT_ELECTRONS);
                (Some(INJECT_ELECTRONS - lag), Some(lag))
            }
            // 3T: the photodiode is FD.
            None => (None, None),
        };
        if fw.is_none() {
            // Full well: three times the charge to reach -1 V in 1.5 µs.
            let iph = 3.0 * c * (a + 1.0).max(0.5) / 1.5e-6;
            fw = Some(Run::new(&deck_for(Stimulus::FullWell, iph).0, opts)?);
        }
        let fwr = fw.as_ref().expect("just made");
        let (s0, s1) = (fwr.v(&qn, FULL_WELL_AT.0)?, fwr.v(&qn, FULL_WELL_AT.1)?);
        let dark = b.photodiode.as_ref().and_then(|def| b.dark_current(&sub, d).map(|i| (i, def.dark_doubling(b.temp))));
        pds.push(PhotodiodeMetrics {
            name: d.name.clone(),
            node: name(d.node()),
            v_reset: a,
            c,
            transferred_e,
            lag_e,
            full_well_e: c * (a - s1) / Q_E,
            v_saturated: s1,
            saturated: (s0 - s1).abs() <= 0.02 * (a - s1).abs().max(1e-3),
            dark,
        });
    }
    // Readout noise at the reset levels.
    let charge_v = pds.first().map_or(0.0, |d| d.v_reset);
    let (nd, _) = deck_for(Stimulus::NoiseBias { fd: fd0, charge: charge_v }, 0.0);
    let res = simulate(&nd, &[], opts).map_err(|e| e.to_string())?;
    let ns = res.dataset("noise").ok_or("the simulation gave no noise result")?;
    let f = &ns.scale_vector().ok_or("no frequency vector")?.re;
    let sd = &ns.get("onoise_spectrum").ok_or("no output noise spectrum")?.re;
    let power: f64 = f.windows(2).zip(sd.windows(2)).map(|(f, s)| (f[1] - f[0]) * (s[0] * s[0] + s[1] * s[1]) / 2.0).sum();
    let readout_uv = power.sqrt() * 1e6;
    let cg_col_uv = cg_fd_uv * sf_gain;
    const K_B: f64 = 1.380_649e-23;
    let noise = NoiseMetrics {
        ktc_e: (K_B * (b.temp + 273.15) * c_fd).sqrt() / Q_E,
        readout_uv,
        readout_e: readout_uv / cg_col_uv.abs().max(1e-12),
        ccol: b.ccol,
    };
    Ok(PixelMetrics { v_fd_reset: fd0, c_fd, cg_fd_uv, cg_col_uv, sf_gain, pds, pinned, temp: b.temp, noise, deck })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ics_core::CellRef;
    use ics_core::pixel_circuit::classify;

    fn aps4t() -> (ics_core::Project, Extraction, PixelCircuit) {
        demo("aps4t")
    }

    fn demo(cell: &str) -> (ics_core::Project, Extraction, PixelCircuit) {
        let p = ics_core::examples::demo_project();
        let lay = p.cell(&CellRef::new("work", cell)).unwrap().layout.as_ref().unwrap();
        let x = ics_core::extract::extract(lay, &p, &p.tech);
        let pc = classify(&x);
        (p, x, pc)
    }

    #[test]
    fn plain_junction_photodiode() {
        let (p, x, pc) = aps4t();
        let bench = Testbench { photodiode: None, ..Testbench::for_tech(&x, &p.tech) };
        let m = characterize(&x, &pc, 0, &p.tech, &bench, "", &SimOptions::default()).unwrap();
        assert!(!m.pinned);
        // A small FD (sub-µm² diffusion, short wire) with the generic level-1 models: a few fF.
        assert!(m.c_fd > 0.3e-15 && m.c_fd < 10e-15, "{}", m.summary());
        assert!((m.cg_fd_uv - Q_E / m.c_fd * 1e6).abs() < 1e-9);
        // An NMOS follower with body effect: gain below one.
        assert!(m.sf_gain > 0.5 && m.sf_gain < 1.0, "{}", m.summary());
        assert!(m.v_fd_reset > 0.5 && m.v_fd_reset < 1.8, "{}", m.summary());
        // The photodiode (12.8 µm² of junction) is much larger than FD.
        let pd = &m.pds[0];
        assert!(pd.c > 3.0 * m.c_fd, "{}", m.summary());
        assert_eq!((pd.transferred_e, pd.lag_e), (None, None));
        assert!(pd.saturated && pd.full_well_e > 10_000.0, "{}", m.summary());
        assert!(m.deck.contains("IQ_FD fd 0 DC 0 PULSE("), "{}", m.deck);
    }

    #[test]
    fn pinned_photodiode_transfers_completely() {
        let (p, x, pc) = aps4t();
        let bench = Testbench::for_tech(&x, &p.tech);
        assert_eq!(bench.pinning(), Some(0.5));
        let m = characterize(&x, &pc, 0, &p.tech, &bench, "", &SimOptions::default()).unwrap();
        assert!(m.pinned);
        assert!(m.deck.contains("BPIN_D1 pd pd_q I=") && m.deck.contains("IQ_PD pd_q 0"), "{}", m.deck);
        let pd = &m.pds[0];
        // Reset to the pinning voltage, less the TX turn-off kick; the storage capacitance
        // 0.5 fF/µm² x 12.825 µm² (and a little of the TG source side).
        assert!(pd.v_reset <= 0.5 && pd.v_reset > 0.35, "{}", m.summary());
        assert!((pd.c - 6.4125e-15).abs() < 0.3e-15, "{}", m.summary());
        // Nearly complete transfer: only the spill-back at TX turn-off stays.
        // kTC of 1.6 fF: 16 e-. Follower thermal noise on 1 pF: about sqrt(2/3 kT / C), some
        // 50 µV, below one electron at 80 µV/e-.
        let n = &m.noise;
        assert!((n.ktc_e - 16.1).abs() < 0.5, "{}", m.summary());
        assert!(n.readout_uv > 20.0 && n.readout_uv < 150.0, "{}", m.summary());
        assert!((n.readout_e - n.readout_uv / m.cg_col_uv).abs() < 1e-9);
        // Demo dark current: 3 aA/µm² at 60 °C, a tenth of it at 27 °C.
        let (i, doubling) = pd.dark.unwrap();
        assert!(i > 2.5e-18 && i < 5e-18 && (doubling - 8.97).abs() < 0.05, "{}", m.summary());
        assert!(m.deck.contains("IDK_D1 pd_q 0 DC "), "{}", m.deck);
        let (t, l) = (pd.transferred_e.unwrap(), pd.lag_e.unwrap());
        assert!(l < 0.1 * INJECT_ELECTRONS && (t + l - INJECT_ELECTRONS).abs() < 1e-9, "{}", m.summary());
        // Full well: from the pinning voltage down to where TX or the substrate junction blooms.
        assert!(pd.saturated, "{}", m.summary());
        assert!(pd.full_well_e > 10_000.0 && pd.full_well_e < 100_000.0, "{}", m.summary());
    }

    #[test]
    fn shared_pixel_measures_both_photodiodes() {
        let (p, x, pc) = demo("aps2s");
        let m = characterize(&x, &pc, 0, &p.tech, &Testbench::for_tech(&x, &p.tech), "", &SimOptions::default()).unwrap();
        assert_eq!(m.pds.iter().map(|d| d.node.as_str()).collect::<Vec<_>>(), ["pd1", "pd2"], "{}", m.summary());
        for d in &m.pds {
            // The same photodiode and gate drawn twice: the same storage, transfer and well.
            assert!((d.c - m.pds[0].c).abs() < 0.05 * m.pds[0].c, "{}", m.summary());
            assert!(d.lag_e.unwrap() < 0.1 * INJECT_ELECTRONS && d.saturated, "{}", m.summary());
            assert!((d.full_well_e - m.pds[0].full_well_e).abs() < 0.05 * m.pds[0].full_well_e, "{}", m.summary());
        }
        // Two transfer gates on FD: more capacitance than the 4T pixel's, less gain.
        let (_, x4, pc4) = aps4t();
        let m4 = characterize(&x4, &pc4, 0, &p.tech, &Testbench::for_tech(&x4, &p.tech), "", &SimOptions::default()).unwrap();
        assert!(m.c_fd > 1.2 * m4.c_fd, "{} vs {}", m.summary(), m4.summary());
    }
}
