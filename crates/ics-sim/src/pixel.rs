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
//! light until it saturates.

use ics_core::Tech;
use ics_core::extract::Extraction;
use ics_core::pixel_circuit::{
    FD_WINDOW, FULL_WELL_AT, INJECT_ELECTRONS, PD_WINDOW, PixelCircuit, Q_E, Stimulus, TRANSFER_WINDOW, Testbench, charge_net,
    pixel_only, schematic,
};

use crate::{SimOptions, simulate};

/// Charge transfer and full well of a pixel's first photodiode.
#[derive(Debug, Clone, PartialEq)]
pub struct PhotodiodeMetrics {
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
    /// The first photodiode, when the pixel has one.
    pub pd: Option<PhotodiodeMetrics>,
    /// Whether the photodiode was simulated as pinned.
    pub pinned: bool,
    /// Dark current of the first photodiode (A) at `temp` (°C), and the temperature step that
    /// doubles it, when the technology gives dark current densities.
    pub dark: Option<(f64, f64)>,
    pub temp: f64,
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
        if let Some((i, doubling)) = self.dark {
            s += &format!(
                "Dark current: {:.2} aA = {:.1} e-/s at {} C (doubles every {doubling:.1} C)\n",
                i * 1e18,
                i / Q_E,
                self.temp
            );
        }
        if let Some(p) = &self.pd {
            let transfer = match (p.transferred_e, p.lag_e) {
                (Some(t), Some(l)) => format!("; transfer of {INJECT_ELECTRONS:.0} e-: {t:.1} e- out, lag {l:.1} e-"),
                _ => String::new(),
            };
            s += &format!(
                "PD{}: reset {:.3} V, C = {:.3} fF{transfer}; full well {:.0} e- (saturates at {:.3} V{})\n",
                if self.pinned { " (pinned)" } else { "" },
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
    let (deck, b) = deck_for(Stimulus::ChargeInjection { pd: true }, 0.0);
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
    let first = p.photodiodes.first().map(|&(d, tg)| (&sub.photodiodes[d], tg));
    let pinned = first.is_some_and(|(d, _)| d.n_type && b.pinning().is_some());
    let pd = match first {
        Some((d, tg)) => {
            let qn = charge_net(&sub, d, Some(&b));
            let (a, c_after) = (run.v(&qn, PD_WINDOW.0)?, run.v(&qn, PD_WINDOW.1)?);
            if (a - c_after).abs() < 1e-9 {
                return Err(format!("the photodiode ({qn}) did not move with the injected charge"));
            }
            let c = q / (a - c_after).abs();
            let (transferred_e, lag_e) = match tg {
                // A plain junction shares its charge with FD rather than transferring it.
                Some(_) if !pinned => (None, None),
                Some(_) => {
                    // What the photodiode holds after the transfer beyond the dark run is lag
                    // (left behind or spilled back); the rest left it for FD.
                    let dark = Run::new(&deck_for(Stimulus::ChargeInjection { pd: false }, 0.0).0, opts)?;
                    let left = dark.v(&qn, TRANSFER_WINDOW.1)? - run.v(&qn, TRANSFER_WINDOW.1)?;
                    let lag = (c * left / Q_E).clamp(0.0, INJECT_ELECTRONS);
                    (Some(INJECT_ELECTRONS - lag), Some(lag))
                }
                // 3T: the photodiode is FD.
                None => (None, None),
            };
            // Full well: fill with three times the charge to reach -1 V in 1.5 µs.
            let iph = 3.0 * c * (a + 1.0).max(0.5) / 1.5e-6;
            let (fw_deck, _) = deck_for(Stimulus::FullWell, iph);
            let fw = Run::new(&fw_deck, opts)?;
            let (s0, s1) = (fw.v(&qn, FULL_WELL_AT.0)?, fw.v(&qn, FULL_WELL_AT.1)?);
            let full_well_e = c * (a - s1) / Q_E;
            let saturated = (s0 - s1).abs() <= 0.02 * (a - s1).abs().max(1e-3);
            Some(PhotodiodeMetrics { v_reset: a, c, transferred_e, lag_e, full_well_e, v_saturated: s1, saturated })
        }
        None => None,
    };
    let dark = first.and_then(|(d, _)| {
        let doubling = b.photodiode.as_ref()?.dark_doubling(b.temp);
        b.dark_current(&sub, d).map(|i| (i, doubling))
    });
    Ok(PixelMetrics {
        v_fd_reset: fd0,
        c_fd,
        cg_fd_uv,
        cg_col_uv: cg_fd_uv * sf_gain,
        sf_gain,
        pd,
        pinned,
        dark,
        temp: b.temp,
        deck,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ics_core::CellRef;
    use ics_core::pixel_circuit::classify;

    fn aps4t() -> (ics_core::Project, Extraction, PixelCircuit) {
        let p = ics_core::examples::demo_project();
        let lay = p.cell(&CellRef::new("work", "aps4t")).unwrap().layout.as_ref().unwrap();
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
        let pd = m.pd.as_ref().unwrap();
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
        let pd = m.pd.as_ref().unwrap();
        // Reset to the pinning voltage, less the TX turn-off kick; the storage capacitance
        // 0.5 fF/µm² x 12.825 µm² (and a little of the TG source side).
        assert!(pd.v_reset <= 0.5 && pd.v_reset > 0.35, "{}", m.summary());
        assert!((pd.c - 6.4125e-15).abs() < 0.3e-15, "{}", m.summary());
        // Nearly complete transfer: only the spill-back at TX turn-off stays.
        // Demo dark current: 3 aA/µm² at 60 °C, a tenth of it at 27 °C.
        let (i, doubling) = m.dark.unwrap();
        assert!(i > 2.5e-18 && i < 5e-18 && (doubling - 8.97).abs() < 0.05, "{}", m.summary());
        assert!(m.deck.contains("IDK_D1 pd_q 0 DC "), "{}", m.deck);
        let (t, l) = (pd.transferred_e.unwrap(), pd.lag_e.unwrap());
        assert!(l < 0.1 * INJECT_ELECTRONS && (t + l - INJECT_ELECTRONS).abs() < 1e-9, "{}", m.summary());
        // Full well: from the pinning voltage down to where TX or the substrate junction blooms.
        assert!(pd.saturated, "{}", m.summary());
        assert!(pd.full_well_e > 10_000.0 && pd.full_well_e < 100_000.0, "{}", m.summary());
    }
}
