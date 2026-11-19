//! Pinning voltage and storage capacitance of a pinned photodiode from its vertical doping
//! profile (from the process emulator, [`crate::process::Section::profile`], or measured).
//!
//! The buried n region between the p+ pinning layer above and the p region below is fully
//! depleted at the pinning voltage. Its depletion reaches up into the p+ layer to `z_t` and
//! down into the p region to `z_b`, both p sides at 0 V: the charge between them is zero and the
//! potential is the same at both edges. The pinning voltage is the potential maximum, where
//! the field vanishes (`z*`, inside the n region). Electrons collect there; the storage
//! capacitance per area is that of the two depletion layers in parallel,
//! ε / (z* − z_t) + ε / (z_b − z*). Depletion approximation (no free carriers in the depleted
//! layers), 1D, room temperature permittivity.

/// Permittivity of silicon (F/cm).
const EPS_SI: f64 = 11.7 * 8.854_187_8e-14;
/// Elementary charge (C).
const Q: f64 = 1.602_176_634e-19;
/// Resampling step (µm).
const STEP: f64 = 5e-4;

/// A pinned photodiode worked out from its doping profile.
#[derive(Debug, Clone, PartialEq)]
pub struct PinnedPd {
    /// Pinning voltage (V): the potential maximum of the fully depleted n region.
    pub vpin: f64,
    /// Storage capacitance per area (fF/µm²) at full depletion.
    pub cap_area: f64,
    /// Depths (µm) of the n region (the metallurgical junctions), of the potential maximum,
    /// and of the depletion edges in the p+ layer and the p region at full depletion.
    pub n_top: f64,
    pub n_bottom: f64,
    pub peak: f64,
    pub depletion: (f64, f64),
    /// Donor charge of the n region (cm⁻²): the electrons it could hold, at most.
    pub n_dose: f64,
}

impl PinnedPd {
    /// The technology fields (`[cross_section.photodiode]`) it gives.
    pub fn toml(&self) -> String {
        format!("pinning = {:.3}, cap_area = {:.4}", self.vpin, self.cap_area)
    }
}

/// Analyse a profile of (depth below the silicon surface in µm, net doping in cm⁻³, donors
/// positive), depth increasing. The first n region with p above it is the photodiode.
pub fn analyse(profile: &[(f64, f64)]) -> Result<PinnedPd, String> {
    if profile.len() < 3 {
        return Err("the profile needs at least three points".into());
    }
    if profile.windows(2).any(|w| w[1].0 <= w[0].0) {
        return Err("the profile depths must increase".into());
    }
    // Resample on a fine uniform grid (linear between the points).
    let (d0, d1) = (profile[0].0, profile[profile.len() - 1].0);
    let n = ((d1 - d0) / STEP).ceil() as usize + 1;
    let mut j = 0;
    let net: Vec<f64> = (0..n)
        .map(|i| {
            let d = d0 + i as f64 * STEP;
            while j + 2 < profile.len() && profile[j + 1].0 < d {
                j += 1;
            }
            let ((a, na), (b, nb)) = (profile[j], profile[j + 1]);
            let f = ((d - a) / (b - a)).clamp(0.0, 1.0);
            na + f * (nb - na)
        })
        .collect();
    // The substrate goes on below the profile with its last doping (a lightly doped epi
    // depletes deep), to 100 µm.
    let mut net = net;
    if let Some(&last) = net.last().filter(|v| **v < 0.0) {
        let more = ((100.0 - (d1 - d0)) / STEP).max(0.0) as usize;
        net.extend(std::iter::repeat_n(last, more));
    }
    let n = net.len();
    let depth = |i: f64| d0 + i * STEP;
    if net[0] > 0.0 {
        return Err("the n region reaches the surface: no pinning layer".into());
    }
    let top = net.iter().position(|&v| v > 0.0).ok_or("no n region in the profile")?;
    let bottom = top + net[top..].iter().position(|&v| v <= 0.0).ok_or("the n region reaches the end of the profile")?;
    // Charge integrals in units of cm⁻³ · cells; positions as fractional cells.
    let h = STEP * 1e-4;
    // Cumulative charge from cell `a` (inclusive) to `b` (exclusive).
    let mut cum = vec![0.0; n + 1];
    for i in 0..n {
        cum[i + 1] = cum[i] + net[i];
    }
    // The bottom edge for top edge `t`: where the charge from t back to zero; None past the end.
    let bottom_edge = |t: usize| -> Option<usize> { (bottom..n).find(|&b| cum[b + 1] - cum[t] <= 0.0) };
    // Potential at cell `z` relative to the top edge `t`: -q/ε ∫∫ N.
    let potential = |t: usize, z: usize| -> f64 {
        let mut e = 0.0;
        let mut phi = 0.0;
        for v in &net[t..z] {
            e += v * h * Q / EPS_SI;
            phi -= e * h;
        }
        phi
    };
    // Whether a top edge at `t` takes too much of the p+ layer: its charge alone outweighs the
    // n region (no bottom edge balances it), or the potential at the balancing bottom edge
    // ends above the top edge's (more p+ charge raises the n region and shortens the bottom
    // depletion). Monotone in `t`: true near the surface, false near the junction.
    let too_high = |t: usize| -> bool {
        if cum[bottom] - cum[t] <= 0.0 {
            return true;
        }
        match bottom_edge(t) {
            Some(b) => potential(t, b + 1) > 0.0,
            None => false,
        }
    };
    let t = if !too_high(0) {
        // Even the whole pinning layer is not too much: the depletion reaches the surface.
        0
    } else if too_high(top) {
        top
    } else {
        let (mut lo, mut hi) = (0usize, top);
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if too_high(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        hi
    };
    let b = bottom_edge(t).ok_or("the p region below is too thin to balance the n region")?;
    // The peak: where the charge from the top edge returns to zero (the field vanishes).
    let peak = (t..=b).find(|&z| cum[z + 1] - cum[t] >= 0.0).unwrap_or(top);
    let vpin = potential(t, peak + 1);
    let (w_top, w_bottom) = ((peak + 1 - t) as f64 * h, (b + 1 - peak) as f64 * h);
    let cap = EPS_SI / w_top + EPS_SI / w_bottom; // F/cm²
    Ok(PinnedPd {
        vpin,
        cap_area: cap * 1e7,
        n_top: depth(top as f64),
        n_bottom: depth(bottom as f64),
        peak: depth(peak as f64 + 0.5),
        depletion: (depth(t as f64), depth(b as f64 + 1.0)),
        n_dose: net[top..bottom].iter().sum::<f64>() * h,
    })
}

/// Analyse the doping column at `x` (µm along the cut) of an emulated cross-section.
pub fn from_section(section: &crate::process::Section, x: f64) -> Result<PinnedPd, String> {
    let col = section.profile(x);
    let top = col.first().ok_or("no silicon at this position")?.0 + section.h / 2.0;
    let profile: Vec<(f64, f64)> = col.iter().map(|(z, net, _)| (top - z, *net)).collect();
    analyse(&profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_photodiode_from_its_implants() {
        // A p-epi wafer, a deep phosphorus implant for the photodiode, a shallow boron one for
        // the pinning layer, annealed: the emulated profile gives a pinning voltage of a few
        // hundred mV to a volt or two and a storage capacitance near 1 fF/µm².
        let flow = crate::process::ProcessFlow::from_toml(
            r#"
resolution = 0.005
substrate_depth = 3.0
substrate_doping = { species = "boron", concentration = 5e14 }
[[step]]
op = "implant"
species = "phosphorus"
dose = 5e11
energy = 300
[[step]]
op = "implant"
species = "boron"
dose = 2e13
energy = 10
[[step]]
op = "anneal"
temperature = 1000
time = 10
"#,
        )
        .unwrap();
        let s = crate::process::emulate(&flow, 1.0, None, &mut |_: &str, xs: &[f64]| Ok(vec![true; xs.len()])).unwrap();
        let pd = from_section(&s, 0.5).unwrap();
        assert!(pd.vpin > 0.1 && pd.vpin < 3.0, "{pd:?}");
        assert!(pd.cap_area > 0.1 && pd.cap_area < 5.0, "{pd:?}");
        assert!(pd.n_top > 0.02 && pd.n_bottom > pd.n_top + 0.1, "{pd:?}");
    }

    fn boxes(layers: &[(f64, f64)]) -> Vec<(f64, f64)> {
        // (thickness µm, net doping) layers, sampled every 1 nm.
        let mut out = vec![];
        let mut z = 0.0;
        for &(t, n) in layers {
            let k = (t / 1e-3).round() as usize;
            for i in 0..k {
                out.push((z + i as f64 * 1e-3, n));
            }
            z += t;
        }
        out
    }

    #[test]
    fn symmetric_box_profile() {
        // p 1e16 | n 1e16 0.4 µm | p 1e16: depletion 0.2 µm into each p side, the peak in the
        // middle; Vpin = q N w² / ε with w = 0.2 µm, C = 2 ε / 0.4 µm.
        let pd = analyse(&boxes(&[(1.0, -1e16), (0.4, 1e16), (1.6, -1e16)])).unwrap();
        let w = 0.2e-4;
        let vpin = Q * 1e16 * w * w / EPS_SI;
        assert!((pd.vpin - vpin).abs() < 0.01 * vpin, "{pd:?} vs {vpin}");
        assert!((pd.cap_area - 2.0 * EPS_SI / 0.4e-4 * 1e7).abs() < 0.02 * pd.cap_area, "{pd:?}");
        assert!(
            (pd.peak - 1.2).abs() < 0.005 && (pd.depletion.0 - 0.8).abs() < 0.005 && (pd.depletion.1 - 1.6).abs() < 0.005,
            "{pd:?}"
        );
        assert!((pd.n_dose - 4e11).abs() < 0.01 * 4e11, "{pd:?}");
    }

    #[test]
    fn heavier_pinning_layer_and_errors() {
        // A heavy shallow p+ layer: the depletion barely enters it, so the n charge is
        // balanced mostly from below and the peak moves up; still neutral and level.
        let pd = analyse(&boxes(&[(0.1, -1e19), (0.4, 2e16), (3.0, -5e15)])).unwrap();
        assert!(pd.depletion.0 > 0.09 && pd.depletion.0 < 0.1, "{pd:?}");
        assert!(pd.peak > pd.n_top && pd.peak < pd.n_bottom, "{pd:?}");
        assert!(pd.vpin > 0.1 && pd.vpin < 5.0 && pd.cap_area > 0.1, "{pd:?}");
        assert!(analyse(&boxes(&[(0.4, 1e16), (1.0, -1e16)])).unwrap_err().contains("surface"));
        // An n region down to the end of the profile has no p region below.
        assert!(analyse(&boxes(&[(1.0, -1e16), (0.5, 1e17)])).unwrap_err().contains("end of the profile"));
        // A thin p layer at the end continues as the substrate: deep depletion, still neutral.
        let deep = analyse(&boxes(&[(1.0, -1e16), (0.5, 1e16), (0.1, -1e15)])).unwrap();
        assert!(deep.depletion.1 > 1.6, "{deep:?}");
    }

    #[test]
    fn demo_pixel_with_the_pinned_photodiode_flow() {
        use crate::process::{Cut, GENERIC180_PPD, ProcessFlow, emulate_cell};
        let p = crate::examples::demo_project();
        let flow = ProcessFlow::from_toml(GENERIC180_PPD).unwrap();
        let cut = Cut::parse("0,1.5,5.6,1.5").unwrap();
        let s = emulate_cell(&p, &crate::CellRef::new("work", "aps4t"), &flow, &cut, None).unwrap();
        // p+ pinning layer, buried n photodiode, p substrate: about 1.1 V and 0.4 fF/µm².
        let pd = from_section(&s, 2.8).unwrap();
        assert!(pd.vpin > 0.6 && pd.vpin < 1.6, "{pd:?}");
        assert!(pd.cap_area > 0.2 && pd.cap_area < 0.6, "{pd:?}");
        assert!(pd.n_top > 0.05 && pd.n_top < 0.3 && pd.n_bottom > 0.4, "{pd:?}");
        // Outside the photodiode (the transistor strip's n+ source / drain) there is no pinning
        // layer: the n region reaches the surface.
        let strip =
            emulate_cell(&p, &crate::CellRef::new("work", "aps4t"), &flow, &Cut::parse("0,4.1,5.6,4.1").unwrap(), None).unwrap();
        assert!(from_section(&strip, 0.5).is_err());
    }
}
