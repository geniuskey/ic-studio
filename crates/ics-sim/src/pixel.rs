//! Image sensor pixel characterisation by simulation: node capacitances, conversion gain and
//! source follower gain of a pixel found in a layout ([`ics_core::pixel_circuit`]).
//!
//! The pixel alone is drawn with the charge injection test bench
//! ([`Stimulus::ChargeInjection`]): FD and the photodiodes are reset, RST and TX turn off, and
//! a known charge is taken out of FD and then out of the first photodiode. With the device
//! models of the technology (PDK models when it has them), C = Q / ΔV takes in every junction,
//! overlap and gate capacitance on the node and the wire capacitance from the layout.

use ics_core::Tech;
use ics_core::extract::Extraction;
use ics_core::pixel_circuit::{
    FD_WINDOW, INJECT_ELECTRONS, PD_WINDOW, PixelCircuit, Q_E, Stimulus, Testbench, pixel_only, schematic,
};

use crate::{SimOptions, simulate};

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
    /// Photodiode voltage after reset (V), its capacitance (F) and charge per volt (e⁻ / V).
    pub v_pd_reset: Option<f64>,
    pub c_pd: Option<f64>,
    /// The deck simulated.
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
        if let (Some(v), Some(c)) = (self.v_pd_reset, self.c_pd) {
            s += &format!("PD: reset {v:.3} V, C = {:.3} fF ({:.0} e-/V)\n", c * 1e15, c / Q_E);
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

/// Simulate pixel `k` of `pc` (from extraction `x` of a layout in `tech`) with the charge
/// injection bench and measure it. `bench` gives the supply and column load; its stimulus is
/// replaced.
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
    let bench = Testbench { stimulus: Stimulus::ChargeInjection, ..bench.clone() };
    let (sch, setup) = schematic(&sub, &one, Some(&bench));
    let nl = ics_core::netlist::netlist_for(&sch, tech, corner);
    let deck = nl.deck(&format!("pixel {} charge injection", k + 1), &setup);
    let res = simulate(&deck, &[], opts).map_err(|e| e.to_string())?;
    let ds = res.dataset("tran").ok_or("the simulation gave no transient result")?;
    let time = &ds.scale_vector().ok_or("no time vector")?.re;
    let name = |n: usize| sub.nets[n].name.split_whitespace().collect::<Vec<_>>().join("_");
    let probe = |net: &str, t: f64| -> Result<f64, String> {
        let v = ds.get(&format!("v({net})")).ok_or_else(|| format!("no voltage of net {net} in the result"))?;
        at(time, &v.re, t).ok_or_else(|| format!("no sample of v({net})"))
    };
    let q = INJECT_ELECTRONS * Q_E;
    let fd = name(p.fd);
    let col = p.column.or(p.sf_out).map(name).ok_or("the pixel has no column line")?;
    let (fd0, fd1) = (probe(&fd, FD_WINDOW.0)?, probe(&fd, FD_WINDOW.1)?);
    let (col0, col1) = (probe(&col, FD_WINDOW.0)?, probe(&col, FD_WINDOW.1)?);
    let dv = fd0 - fd1;
    if dv.abs() < 1e-9 {
        return Err(format!("FD ({fd}) did not move with the injected charge"));
    }
    let c_fd = q / dv.abs();
    let sf_gain = (col0 - col1) / dv;
    let cg_fd_uv = Q_E / c_fd * 1e6;
    let (mut v_pd_reset, mut c_pd) = (None, None);
    if let Some(&(d, _)) = p.photodiodes.first() {
        let pd = name(sub.photodiodes[d].node());
        let (a, b) = (probe(&pd, PD_WINDOW.0)?, probe(&pd, PD_WINDOW.1)?);
        v_pd_reset = Some(a);
        c_pd = ((a - b).abs() > 1e-9).then(|| q / (a - b).abs());
    }
    Ok(PixelMetrics { v_fd_reset: fd0, c_fd, cg_fd_uv, cg_col_uv: cg_fd_uv * sf_gain, sf_gain, v_pd_reset, c_pd, deck })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ics_core::CellRef;
    use ics_core::pixel_circuit::classify;

    #[test]
    fn aps4t_conversion_gain() {
        let p = ics_core::examples::demo_project();
        let lay = p.cell(&CellRef::new("work", "aps4t")).unwrap().layout.as_ref().unwrap();
        let x = ics_core::extract::extract(lay, &p, &p.tech);
        let pc = classify(&x);
        let bench = Testbench { wire_caps: ics_core::pex::net_caps(&x, &p.tech), ..Testbench::default() };
        let m = characterize(&x, &pc, 0, &p.tech, &bench, "", &SimOptions::default()).unwrap();
        // A small FD (sub-µm² diffusion, short wire) with the generic level-1 models: a few fF.
        assert!(m.c_fd > 0.3e-15 && m.c_fd < 10e-15, "{}", m.summary());
        assert!((m.cg_fd_uv - Q_E / m.c_fd * 1e6).abs() < 1e-9);
        // An NMOS follower with body effect: gain below one.
        assert!(m.sf_gain > 0.5 && m.sf_gain < 1.0, "{}", m.summary());
        // The photodiode (12.8 µm² of junction) is much larger than FD.
        let c_pd = m.c_pd.unwrap();
        assert!(c_pd > 3.0 * m.c_fd, "{}", m.summary());
        assert!(m.v_fd_reset > 0.5 && m.v_fd_reset < 1.8, "{}", m.summary());
        assert!(m.deck.contains("IQ_FD fd 0 DC 0 PULSE("), "{}", m.deck);
    }
}
