//! Plain-language advice for results a beginner may not know how to act on: a spec that misses
//! its limit, a transistor in an unexpected operating region, a measurement that failed. The
//! advice is a rule of thumb that says which way to move which parameter; the optimizer and
//! sweeps then find the numbers.

use crate::units::parse_spice;

/// Which side of its limits a measured spec value is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Miss {
    TooLow,
    TooHigh,
}

/// Whether `value` is below `min` or above `max` (limits as SPICE numbers; empty means none).
pub fn spec_miss(min: &str, max: &str, value: f64) -> Option<Miss> {
    if parse_spice(min).is_some_and(|lo| value < lo) {
        Some(Miss::TooLow)
    } else if parse_spice(max).is_some_and(|hi| value > hi) {
        Some(Miss::TooHigh)
    } else {
        None
    }
}

/// What kind of quantity a waveform-calculator expression measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quantity {
    PhaseMargin,
    GainMargin,
    Bandwidth,
    Gain,
    Delay,
    Period,
    Overshoot,
    Settling,
    Current,
    Voltage,
    Other,
}

fn quantity(expr: &str) -> Quantity {
    let e = expr.to_ascii_lowercase().replace(' ', "");
    let has = |k: &str| e.contains(k);
    if has("phasemargin") {
        Quantity::PhaseMargin
    } else if has("gainmargin") {
        Quantity::GainMargin
    } else if has("ugf(") || has("bandwidth(") {
        Quantity::Bandwidth
    } else if has("db(") || has("gain") || (has("mag(") && !has("phase")) {
        Quantity::Gain
    } else if has("delay(") || has("risetime(") || has("falltime(") {
        Quantity::Delay
    } else if has("cross(") && e.contains('-') {
        Quantity::Period
    } else if has("overshoot(") {
        Quantity::Overshoot
    } else if has("settling(") {
        Quantity::Settling
    } else if e.starts_with("i(") || e.starts_with("-i(") || has("avg(i(") {
        Quantity::Current
    } else if e.starts_with("v(") || has("avg(v(") || has("value(v(") {
        Quantity::Voltage
    } else {
        Quantity::Other
    }
}

/// Whether a spec expression measures a current (`i(v1)`, `avg(i(vdd))`).
pub fn measures_current(expr: &str) -> bool {
    quantity(expr) == Quantity::Current
}

const SEARCH: &str = "Simulation > Optimizer can search the sizes against this spec, and the Corner / Sweep Manager shows \
                      which way one parameter moves it.";

/// What to try when a spec measured by `expr` misses its limit in direction `miss`.
pub fn spec_advice(expr: &str, miss: Miss) -> String {
    use Miss::*;
    use Quantity::*;
    let tip = match (quantity(expr), miss) {
        (PhaseMargin, TooLow) => {
            "Too little phase margin: in feedback the circuit will ring or oscillate. Add or enlarge the compensation \
             capacitor (a Miller capacitor between the stages of a two-stage amplifier), reduce the load capacitance, or \
             lower the gain-bandwidth (less input-pair current)."
        }
        (PhaseMargin, TooHigh) => "More phase margin than needed: a smaller compensation capacitor would make it faster.",
        (GainMargin, _) => "Gain margin out of range: change the compensation capacitor, as for phase margin.",
        (Bandwidth, TooLow) => {
            "Too slow: raise the bias current (e.g. the tail current source of a differential pair), widen the input \
             transistors for more transconductance gm, or reduce the load capacitance. Speed costs power or gain."
        }
        (Bandwidth, TooHigh) => {
            "Wider than allowed: enlarge the capacitor (or resistor) that sets the pole, or lower the bias current."
        }
        (Gain, TooLow) => {
            "Gain too low: first check that every amplifying transistor is in saturation (Simulation > Run, the \
             operating point table). Then lengthen L of the amplifying transistors and their loads (higher output \
             resistance), or lower the current per stage. Each change slows the circuit a little."
        }
        (Gain, TooHigh) => "Gain higher than allowed: shorter L, a resistive load or source degeneration lowers it.",
        (Delay, TooHigh) => {
            "Too slow: widen the transistors that drive the edge (PMOS for a rising output, NMOS for a falling one), or \
             reduce the load capacitance."
        }
        (Delay, TooLow) => "Faster than the minimum: narrower transistors or more load capacitance slow it down.",
        (Period, TooHigh) => {
            "Period too long (too slow): wider or shorter-L transistors, fewer stages or less load make it faster."
        }
        (Period, TooLow) => "Period too short (too fast): add stages (keep an odd number in a ring), lengthen L or add load.",
        (Overshoot, TooHigh) => "Too much overshoot: the loop is close to unstable; improve the phase margin (compensation).",
        (Settling, TooHigh) => "Settles too slowly: more bandwidth (bias current) and enough phase margin (about 60 degrees).",
        (Current, TooLow) => {
            "Current too low: widen (W) or shorten (L) the transistor that sets it, or raise its gate drive. In a current \
             mirror the output current is the reference times (W/L out) / (W/L reference)."
        }
        (Current, TooHigh) => {
            "Current too high: narrow (W) or lengthen (L) the transistor that sets it, or lower its gate drive. In a \
             current mirror, lower the (W/L out) / (W/L reference) ratio."
        }
        (Voltage, _) => {
            "Voltage out of range: it is set by a ratio (resistors, transistor sizes) or a bias; find the parameter \
             with a sweep and move it the other way."
        }
        _ => "",
    };
    if tip.is_empty() { SEARCH.to_string() } else { format!("{tip} {SEARCH}") }
}

/// What a measurement error means, for the messages a beginner meets most.
pub fn error_advice(err: &str) -> Option<&'static str> {
    let e = err.to_ascii_lowercase();
    if e.contains("unknown signal") {
        Some(
            "The expression names a net or source the circuit does not have. Net names come from labels (v(out)); currents from voltage sources (i(v1)).",
        )
    } else if e.contains("never crosses") {
        Some(
            "The waveform never reaches that level: the circuit may not switch at all (check the input source and the supply), or the simulation stops too early (longer tstop).",
        )
    } else if e.contains("no ") && e.contains("analysis") || e.contains("not run") {
        Some("The spec reads an analysis that is not enabled: turn it on in the cell's simulation setup.")
    } else {
        None
    }
}

/// Advice for a MOS transistor's operating point: `region` as the simulator reports it and
/// its values (`vgs`, `vds`, `vth`, ...). `None` when there is nothing to warn about.
pub fn mos_advice(region: &str, values: &[(String, f64)]) -> Option<String> {
    let get = |k: &str| values.iter().find(|(n, _)| n == k).map(|(_, v)| v.abs());
    match region {
        "cutoff" => Some(
            "Off: vgs is below the threshold vth, so no current flows. Right for an open switch; otherwise raise its gate \
             voltage (or check the bias that drives it)."
                .into(),
        ),
        "subthreshold" => Some(
            "Weak inversion: vgs is just below vth. Little current and efficient, but slow; raise vgs (or shrink W) if it \
             should be faster."
                .into(),
        ),
        "triode" => Some(
            "Triode: vds is smaller than vgs - vth, so it acts like a resistor or a closed switch. Right for logic and \
             switches. An amplifier, active load or current source needs saturation: give it more vds (headroom) or less \
             overdrive vgs - vth (wider W for the same current)."
                .into(),
        ),
        "saturation" => {
            let (vgs, vds, vth) = (get("vgs")?, get("vds")?, get("vth")?);
            let spare = vds - (vgs - vth);
            (spare < 0.1).then(|| {
                format!(
                    "Saturated, but only {:.0} mV from triode (vds - (vgs - vth)): a small change in the voltages pushes \
                     it out. Give it more vds or less overdrive.",
                    spare * 1e3
                )
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn misses_and_advice() {
        assert_eq!(spec_miss("30", "", 20.0), Some(Miss::TooLow));
        assert_eq!(spec_miss("", "50p", 60e-12), Some(Miss::TooHigh));
        assert_eq!(spec_miss("19u", "21u", 20e-6), None);
        assert!(spec_advice("phasemargin(v(out))", Miss::TooLow).contains("compensation"));
        assert!(spec_advice("max(db(v(out)))", Miss::TooLow).contains("lengthen L"));
        assert!(spec_advice("ugf(v(out))", Miss::TooLow).contains("bias current"));
        assert!(spec_advice("delay(v(in), 0.9, v(out), 0.9, rise, fall)", Miss::TooHigh).contains("widen"));
        assert!(spec_advice("-i(v1)", Miss::TooLow).contains("mirror"));
        assert!(spec_advice("cross(v(n1), 0.9, 3, rise) - cross(v(n1), 0.9, 2, rise)", Miss::TooHigh).contains("Period"));
        assert!(spec_advice("foo(x)", Miss::TooLow).contains("Optimizer"));
        assert!(error_advice("unknown signal 'v(y)'").is_some());
    }

    #[test]
    fn mos_regions() {
        let v = |vgs: f64, vds: f64| vec![("vgs".to_string(), vgs), ("vds".into(), vds), ("vth".into(), 0.45)];
        assert!(mos_advice("triode", &v(1.8, 0.1)).unwrap().contains("Triode"));
        assert!(mos_advice("saturation", &v(0.7, 0.9)).is_none());
        assert!(mos_advice("saturation", &v(0.9, 0.48)).unwrap().contains("30 mV"));
        assert!(mos_advice("saturation", &v(-0.9, -0.48)).is_some(), "PMOS values are negative");
    }
}
