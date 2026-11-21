//! Calibration of a pixel stack to measured quantum efficiency: read a measured QE table,
//! compare, and fit physical parameters of the stack (Nelder-Mead on scale factors).
//!
//! The measured table is CSV: a header naming the wavelength column (`wavelength`, `nm`,
//! `lambda`; nm, or µm when every value is below 10) and any of `R`, `Gr`, `Gb`, `B` (`G` for
//! both greens). QE as fractions, or percent when some value exceeds 1.5. `#` starts a comment.
//!
//! Parameters (`Knob`): the colour filters' pigment density (their absorption scale, per colour
//! or all), the colour filter thickness, the anti-reflection films' thickness, the silicon's
//! thickness and the microlens height, each as a factor on the starting stack's value within
//! bounds. The carrier model is solved once unless the silicon's thickness is fitted.

use crate::pixel::{CarrierModel, LayerKind, LensShape, Method, PixelResult, PixelStack, Settings, carrier_model, simulate_with};

/// Measured QE per colour (R, Gr, Gb, B; NaN where a colour was not measured).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Measured {
    /// µm
    pub wavelengths: Vec<f64>,
    pub qe: [Vec<f64>; 4],
}

impl Measured {
    pub fn from_csv(text: &str) -> Result<Measured, String> {
        let mut rows = text.lines().map(|l| l.split('#').next().unwrap_or("").trim()).filter(|l| !l.is_empty());
        let header = rows.next().ok_or("measured QE: empty file")?;
        let split =
            |l: &str| -> Vec<String> { l.split([',', ';', '\t']).map(|c| c.trim().trim_matches('"').to_string()).collect() };
        let cols = split(header);
        let wl = cols
            .iter()
            .position(|c| matches!(c.to_ascii_lowercase().as_str(), "wavelength" | "wavelength_nm" | "nm" | "lambda" | "wl"))
            .ok_or("measured QE: no wavelength column (wavelength, nm, lambda)")?;
        // Column of each colour (0 R, 1 Gr, 2 Gb, 3 B).
        let mut map: [Option<usize>; 4] = [None; 4];
        for (i, c) in cols.iter().enumerate() {
            match c.to_ascii_lowercase().as_str() {
                "r" | "red" => map[0] = Some(i),
                "gr" => map[1] = Some(i),
                "gb" => map[2] = Some(i),
                "g" | "green" => {
                    map[1] = map[1].or(Some(i));
                    map[2] = map[2].or(Some(i));
                }
                "b" | "blue" => map[3] = Some(i),
                _ => {}
            }
        }
        if map.iter().all(Option::is_none) {
            return Err("measured QE: no colour columns (R, Gr, Gb, B or G)".into());
        }
        let mut wavelengths = vec![];
        let mut qe: [Vec<f64>; 4] = Default::default();
        for (k, row) in rows.enumerate() {
            let v = split(row);
            let num = |i: usize| -> Result<f64, String> {
                let s = v.get(i).map(String::as_str).unwrap_or("");
                if s.is_empty() {
                    return Ok(f64::NAN);
                }
                s.parse::<f64>().map_err(|_| format!("measured QE: row {}: '{s}' is not a number", k + 2))
            };
            wavelengths.push(num(wl)?);
            for c in 0..4 {
                qe[c].push(match map[c] {
                    Some(i) => num(i)?,
                    None => f64::NAN,
                });
            }
        }
        if wavelengths.is_empty() || wavelengths.iter().any(|w| !w.is_finite() || *w <= 0.0) {
            return Err("measured QE: needs rows with positive wavelengths".into());
        }
        if qe.iter().flatten().all(|q| !q.is_finite()) {
            return Err("measured QE: the table has no QE values".into());
        }
        if wavelengths.iter().all(|&w| w > 10.0) {
            wavelengths.iter_mut().for_each(|w| *w /= 1000.0);
        }
        if qe.iter().flatten().any(|&q| q > 1.5) {
            qe.iter_mut().flatten().for_each(|q| *q /= 100.0);
        }
        Ok(Measured { wavelengths, qe })
    }

    /// At most `n` of the usable rows, evenly spread over them.
    pub fn thinned(&self, n: usize) -> Measured {
        let rows = self.usable();
        if rows.len() <= n {
            return self.clone();
        }
        let pick: Vec<usize> = (0..n).map(|i| rows[i * (rows.len() - 1) / (n - 1)].0).collect();
        Measured {
            wavelengths: pick.iter().map(|&r| self.wavelengths[r]).collect(),
            qe: std::array::from_fn(|c| pick.iter().map(|&r| self.qe[c][r]).collect()),
        }
    }

    /// Wavelengths inside the optical data (0.3 - 1.2 µm), sorted, with their row indices.
    fn usable(&self) -> Vec<(usize, f64)> {
        let mut v: Vec<(usize, f64)> =
            self.wavelengths.iter().cloned().enumerate().filter(|(_, w)| (0.3..=1.2).contains(w)).collect();
        v.sort_by(|a, b| a.1.total_cmp(&b.1));
        v
    }
}

/// RMS difference (QE) per colour and over all, between a result computed at the measured
/// table's usable wavelengths (see [`settings_for`]) and the table. NaN for unmeasured colours.
pub fn compare(r: &PixelResult, m: &Measured) -> ([f64; 4], f64) {
    let rows = m.usable();
    let mut per = [f64::NAN; 4];
    let (mut sum, mut count) = (0.0, 0usize);
    for c in 0..4 {
        let (mut s, mut n) = (0.0, 0usize);
        for (k, &(row, _)) in rows.iter().enumerate() {
            let meas = m.qe[c][row];
            if meas.is_finite() && k < r.qe[c].len() {
                s += (r.qe[c][k] - meas).powi(2);
                n += 1;
            }
        }
        if n > 0 {
            per[c] = (s / n as f64).sqrt();
            sum += s;
            count += n;
        }
    }
    (per, if count > 0 { (sum / count as f64).sqrt() } else { f64::NAN })
}

/// `base` settings at the measured table's usable wavelengths.
pub fn settings_for(base: &Settings, m: &Measured) -> Settings {
    Settings { wavelengths: m.usable().into_iter().map(|(_, w)| w).collect(), ..base.clone() }
}

/// A fitted parameter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Knob {
    /// Pigment density of one filter (0 R, 1 G, 2 B).
    Density(usize),
    /// All three densities by one factor.
    DensityAll,
    CfThickness,
    Arc,
    SiThickness,
    Lens,
    /// Superellipse microlens: the footprint's exponent n (squareness).
    LensN,
    /// Superellipse microlens: the sag's exponent α.
    LensAlpha,
    /// A factor on the model's QE against the measurement, not a stack parameter: it absorbs an
    /// error of the measurement's absolute calibration (illumination power, the reference
    /// photodiode) so that the other parameters fit the curves' shape.
    QeScale,
}

impl Knob {
    pub fn parse(name: &str) -> Result<Vec<Knob>, String> {
        Ok(match name.trim().to_ascii_lowercase().as_str() {
            "cf_red" | "red" => vec![Knob::Density(0)],
            "cf_green" | "green" => vec![Knob::Density(1)],
            "cf_blue" | "blue" => vec![Knob::Density(2)],
            "cf_density" | "density" => vec![Knob::DensityAll],
            "cf" => vec![Knob::Density(0), Knob::Density(1), Knob::Density(2)],
            "cf_thickness" => vec![Knob::CfThickness],
            "arc" => vec![Knob::Arc],
            "si_thickness" | "thickness" => vec![Knob::SiThickness],
            "lens" => vec![Knob::Lens],
            "lens_n" => vec![Knob::LensN],
            "lens_alpha" => vec![Knob::LensAlpha],
            "lens_shape" => vec![Knob::LensN, Knob::LensAlpha],
            "qe_scale" | "scale" => vec![Knob::QeScale],
            other => {
                return Err(format!(
                    "unknown fit parameter '{other}' (cf_red, cf_green, cf_blue, cf, cf_density, cf_thickness, arc, si_thickness, lens, lens_n, lens_alpha, lens_shape, qe_scale)"
                ));
            }
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Knob::Density(0) => "red filter density",
            Knob::Density(1) => "green filter density",
            Knob::Density(_) => "blue filter density",
            Knob::DensityAll => "filter density",
            Knob::CfThickness => "colour filter thickness",
            Knob::Arc => "anti-reflection films",
            Knob::SiThickness => "silicon thickness",
            Knob::Lens => "microlens height",
            Knob::LensN => "microlens squareness n",
            Knob::LensAlpha => "microlens sag exponent alpha",
            Knob::QeScale => "measured / model QE scale",
        }
    }

    /// Bounds of the factor.
    fn bounds(self) -> (f64, f64) {
        match self {
            Knob::Density(_) | Knob::DensityAll => (0.2, 5.0),
            Knob::CfThickness | Knob::Arc | Knob::Lens => (0.5, 1.5),
            Knob::SiThickness => (0.7, 1.3),
            Knob::LensN => (0.5, 4.0),
            Knob::LensAlpha => (0.33, 3.0),
            Knob::QeScale => (0.7, 1.3),
        }
    }

    /// Whether the stack has what this knob changes.
    fn applies(self, s: &PixelStack) -> Result<(), String> {
        let ok = match self {
            Knob::CfThickness => s.layers.iter().any(|l| l.kind == LayerKind::ColorFilter),
            Knob::Arc => !s.arc.is_empty(),
            Knob::SiThickness => s.silicon.as_ref().is_some_and(|si| si.thickness.is_some()),
            Knob::Lens => s.microlens.is_some(),
            Knob::LensN | Knob::LensAlpha => s.microlens.as_ref().is_some_and(|m| m.shape == LensShape::Superellipse),
            _ => true,
        };
        if ok {
            Ok(())
        } else if matches!(self, Knob::LensN | Knob::LensAlpha) {
            Err(format!("fit parameter '{}': needs [microlens] shape = \"superellipse\"", self.name()))
        } else {
            Err(format!("fit parameter '{}': the stack has none", self.name()))
        }
    }

    fn apply(self, s: &mut PixelStack, f: f64) {
        match self {
            Knob::Density(c) => s.color_filters.density[c] *= f,
            Knob::DensityAll => s.color_filters.density.iter_mut().for_each(|d| *d *= f),
            Knob::CfThickness => s.layers.iter_mut().filter(|l| l.kind == LayerKind::ColorFilter).for_each(|l| l.thickness *= f),
            Knob::Arc => s.arc.iter_mut().for_each(|a| a.thickness *= f),
            Knob::SiThickness => {
                if let Some(si) = &mut s.silicon {
                    si.thickness = si.thickness.map(|t| t * f);
                    si.epi = si.epi.map(|t| t * f);
                    if let Some(d) = &mut si.dti
                        && si.thickness.is_some_and(|t| d.depth >= t / f - 1e-9)
                    {
                        // A full-depth trench stays full depth.
                        d.depth *= f;
                    }
                }
            }
            Knob::Lens => {
                if let Some(m) = &mut s.microlens {
                    m.height *= f;
                }
            }
            Knob::LensN | Knob::LensAlpha => {
                if let Some(ml) = &mut s.microlens {
                    // The default radius follows n; keep the lens's actual size while n moves.
                    ml.radius = Some(ml.base_radius(s.pitch));
                    let (n, alpha) = ml.exponents();
                    if self == Knob::LensN {
                        // A footprint exponent below 1 is not convex; the fit stops there.
                        ml.n = Some((n * f).max(1.0));
                    } else {
                        ml.alpha = Some(alpha * f);
                    }
                }
            }
            Knob::QeScale => {}
        }
    }

    /// The fitted value as a line of the stack file.
    fn describe(self, s: &PixelStack) -> String {
        match self {
            Knob::Density(_) | Knob::DensityAll => {
                let d = s.color_filters.density;
                format!("[color_filters] density = [{:.3}, {:.3}, {:.3}]", d[0], d[1], d[2])
            }
            Knob::CfThickness => {
                let t: Vec<String> = s
                    .layers
                    .iter()
                    .filter(|l| l.kind == LayerKind::ColorFilter)
                    .map(|l| format!("'{}' thickness = {:.4}", l.name, l.thickness))
                    .collect();
                t.join(", ")
            }
            Knob::Arc => {
                let t: Vec<String> = s.arc.iter().map(|a| format!("{} {:.4}", a.material, a.thickness)).collect();
                format!("[[arc]] thicknesses: {}", t.join(", "))
            }
            Knob::SiThickness => {
                format!("[silicon] thickness = {:.4}", s.silicon.as_ref().and_then(|si| si.thickness).unwrap_or(0.0))
            }
            Knob::Lens => format!("[microlens] height = {:.4}", s.microlens.as_ref().map_or(0.0, |m| m.height)),
            Knob::LensN | Knob::LensAlpha => {
                let (n, alpha) = s.microlens.as_ref().map_or((2.0, 2.0), |l| l.exponents());
                if self == Knob::LensN { format!("[microlens] n = {n:.3}") } else { format!("[microlens] alpha = {alpha:.3}") }
            }
            Knob::QeScale => "(not in the stack: the measurement's scale against the model)".into(),
        }
    }
}

/// A calibration's outcome.
#[derive(Debug, Clone)]
pub struct Fit {
    /// Parameter, factor on the starting value, the fitted value as a stack line.
    pub knobs: Vec<(String, f64, String)>,
    /// The fitted parameters, in the order of `knobs`.
    pub fitted: Vec<Knob>,
    pub rms_before: ([f64; 4], f64),
    pub rms_after: ([f64; 4], f64),
    pub evaluations: usize,
    pub stack: PixelStack,
    /// The model at the measured wavelengths before and after (QE scale applied after).
    pub before: PixelResult,
    pub result: PixelResult,
    /// The fitted QE scale (1 when not fitted).
    pub qe_scale: f64,
    /// Parameters that ended at a bound, and other doubts.
    pub warnings: Vec<String>,
    /// What each stage did.
    pub stages: Vec<String>,
}

/// Fit `knobs` of `stack` to `measured` (at the measured wavelengths), within
/// `max_evaluations` model runs. With RCWA as `base`'s solver, the ray model fits first and
/// RCWA refines from there with a third of the runs.
pub fn fit(
    stack: &PixelStack,
    base: &Settings,
    measured: &Measured,
    knobs: &[Knob],
    max_evaluations: usize,
) -> Result<Fit, String> {
    if !matches!(base.method, Method::Rcwa { .. }) {
        let mut f = fit_one(stack, base, measured, knobs, max_evaluations, 0.25)?;
        f.stages.push(format!("ray model: {} runs, RMS {:.4} -> {:.4}", f.evaluations, f.rms_before.1, f.rms_after.1));
        return Ok(f);
    }
    let ray = Settings { method: Method::Ray, ..base.clone() };
    let first = fit_one(stack, &ray, measured, knobs, max_evaluations, 0.25)?;
    // RCWA on at most 15 of the measured wavelengths, evenly spread; judged on all at the end.
    let sub = measured.thinned(15);
    let mut second = fit_one(&first.stack, base, &sub, knobs, (max_evaluations / 3).max(knobs.len() + 4), 0.06)?;
    if sub.wavelengths.len() < measured.wavelengths.len() {
        let all = settings_for(base, measured);
        second.result = scaled(run_model(&second.stack, &all, None)?, second.qe_scale);
        second.rms_after = compare(&second.result, measured);
    }
    // Factors on the original stack; the RMS before with RCWA on the original.
    let orig = run_model(stack, &settings_for(base, measured), None)?;
    second.stages = vec![
        format!("ray model: {} runs, RMS {:.4} -> {:.4} (ray)", first.evaluations, first.rms_before.1, first.rms_after.1),
        format!("RCWA from there: {} runs, RMS {:.4} -> {:.4}", second.evaluations, second.rms_before.1, second.rms_after.1),
    ];
    second.rms_before = compare(&orig, measured);
    second.before = orig;
    // Stack factors compound; the QE scale is the RCWA stage's own (the ray stage's also
    // absorbs the ray model's bias, which RCWA does not have).
    for ((k, f1), knob) in second.knobs.iter_mut().zip(&first.knobs).zip(&second.fitted) {
        if *knob != Knob::QeScale {
            k.1 *= f1.1;
        }
    }
    second.qe_scale = second.knobs.iter().zip(&second.fitted).find(|(_, k)| **k == Knob::QeScale).map_or(1.0, |(v, _)| v.1);
    second.warnings = bound_warnings(&second.fitted, &second.knobs);
    second.evaluations += first.evaluations;
    Ok(second)
}

/// Parameters within 2 % of a bound.
fn bound_warnings(knobs: &[Knob], values: &[(String, f64, String)]) -> Vec<String> {
    knobs
        .iter()
        .zip(values)
        .filter_map(|(k, (name, f, _))| {
            let (lo, hi) = k.bounds();
            (*f < lo * 1.02 || *f > hi / 1.02).then(|| {
                format!("{name} ended at its bound (x {f:.3}, range {lo} - {hi}): the stack may lack what the measurement shows")
            })
        })
        .collect()
}

fn run_model(s: &PixelStack, settings: &Settings, cached: Option<&CarrierModel>) -> Result<PixelResult, String> {
    match cached {
        Some(c) => simulate_with(s, settings, Some(c)),
        None => {
            let m = carrier_model(s)?;
            simulate_with(s, settings, m.as_ref())
        }
    }
}

fn scaled(mut r: PixelResult, f: f64) -> PixelResult {
    r.qe.iter_mut().flatten().for_each(|q| *q *= f);
    r
}

fn fit_one(
    stack: &PixelStack,
    base: &Settings,
    measured: &Measured,
    knobs: &[Knob],
    max_evaluations: usize,
    step: f64,
) -> Result<Fit, String> {
    if knobs.is_empty() {
        return Err("no parameters to fit".into());
    }
    for k in knobs {
        k.applies(stack)?;
    }
    let settings = settings_for(base, measured);
    if settings.wavelengths.is_empty() {
        return Err("measured QE: no wavelengths within 300 - 1200 nm".into());
    }
    let fixed_si = !knobs.contains(&Knob::SiThickness);
    let cached: Option<CarrierModel> = if fixed_si { carrier_model(stack)? } else { None };
    let clamp = |k: &Knob, v: f64| {
        let (lo, hi) = k.bounds();
        v.exp().clamp(lo, hi)
    };
    let build = |x: &[f64]| -> (PixelStack, f64) {
        let mut s = stack.clone();
        let mut scale = 1.0;
        for (k, &v) in knobs.iter().zip(x) {
            if *k == Knob::QeScale {
                scale = clamp(k, v);
            } else {
                k.apply(&mut s, clamp(k, v));
            }
        }
        (s, scale)
    };
    let run = |s: &PixelStack| -> Result<PixelResult, String> {
        if fixed_si { run_model(s, &settings, cached.as_ref()) } else { run_model(s, &settings, None) }
    };
    let before = run(stack)?;
    let rms_before = compare(&before, measured);
    let mut evaluations = 1;
    let mut error = None;
    let mut objective = |x: &[f64]| -> f64 {
        evaluations += 1;
        // Outside the bounds: the clamped value plus a penalty, so the simplex comes back.
        let penalty: f64 = knobs
            .iter()
            .zip(x)
            .map(|(k, &v)| {
                let (lo, hi) = k.bounds();
                (lo.ln() - v).max(0.0) + (v - hi.ln()).max(0.0)
            })
            .sum();
        let (s, scale) = build(x);
        match run(&s) {
            Ok(r) => compare(&scaled(r, scale), measured).1 + penalty,
            Err(e) => {
                error.get_or_insert(e);
                f64::INFINITY
            }
        }
    };
    let x = nelder_mead(&mut objective, &vec![0.0; knobs.len()], step, max_evaluations.max(knobs.len() + 2), 1e-6);
    if let Some(e) = error {
        return Err(e);
    }
    let (fitted, qe_scale) = build(&x);
    let result = scaled(run(&fitted)?, qe_scale);
    let rms_after = compare(&result, measured);
    let values: Vec<(String, f64, String)> =
        knobs.iter().zip(&x).map(|(k, &v)| (k.name().to_string(), clamp(k, v), k.describe(&fitted))).collect();
    Ok(Fit {
        warnings: bound_warnings(knobs, &values),
        knobs: values,
        fitted: knobs.to_vec(),
        rms_before,
        rms_after,
        evaluations,
        stack: fitted,
        before,
        result,
        qe_scale,
        stages: vec![],
    })
}

/// The stack file `text` with the fitted values written in (comments and layout kept): filter
/// densities, colour filter and anti-reflection film thicknesses, the silicon's thickness (and a
/// full-depth trench's), the microlens height, as far as `fit` changed them.
pub fn patch_stack_toml(text: &str, fit: &Fit) -> Result<String, String> {
    use toml_edit::{Array, DocumentMut, Item, Table, value};
    let mut doc: DocumentMut = text.parse().map_err(|e| format!("stack file: {e}"))?;
    let st = &fit.stack;
    let r4 = |v: f64| (v * 1e4).round() / 1e4;
    let has = |k: Knob| fit.fitted.contains(&k);
    if fit.fitted.iter().any(|k| matches!(k, Knob::Density(_) | Knob::DensityAll)) {
        if !doc.contains_key("color_filters") {
            doc.insert("color_filters", Item::Table(Table::new()));
        }
        let mut a = Array::new();
        for d in st.color_filters.density {
            a.push((d * 1e3).round() / 1e3);
        }
        doc["color_filters"]["density"] = value(a);
    }
    if has(Knob::CfThickness)
        && let Some(layers) = doc.get_mut("layers").and_then(Item::as_array_of_tables_mut)
    {
        for (t, l) in layers.iter_mut().zip(&st.layers) {
            if l.kind == LayerKind::ColorFilter {
                t["thickness"] = value(r4(l.thickness));
            }
        }
    }
    if has(Knob::Arc)
        && let Some(arc) = doc.get_mut("arc").and_then(Item::as_array_of_tables_mut)
    {
        for (t, a) in arc.iter_mut().zip(&st.arc) {
            t["thickness"] = value(r4(a.thickness));
        }
    }
    if has(Knob::SiThickness)
        && let Some(si) = &st.silicon
    {
        let sec = &mut doc["silicon"];
        if let Some(t) = si.thickness {
            sec["thickness"] = value(r4(t));
        }
        if let Some(e) = si.epi
            && sec.get("epi").is_some()
        {
            sec["epi"] = value(r4(e));
        }
        if let Some(d) = &si.dti
            && sec.get("dti").is_some()
        {
            sec["dti"]["depth"] = value(r4(d.depth));
        }
    }
    if has(Knob::Lens)
        && let Some(m) = &st.microlens
    {
        doc["microlens"]["height"] = value(r4(m.height));
    }
    if let Some(m) = &st.microlens
        && m.shape == LensShape::Superellipse
    {
        let (n, alpha) = m.exponents();
        if has(Knob::LensN) {
            doc["microlens"]["n"] = value(r4(n));
            // The fit held the lens's size while n moved; the default radius would follow n.
            doc["microlens"]["radius"] = value(r4(m.base_radius(st.pitch)));
        }
        if has(Knob::LensAlpha) {
            doc["microlens"]["alpha"] = value(r4(alpha));
        }
    }
    let mut out = doc.to_string();
    if has(Knob::QeScale) {
        out = format!(
            "# Calibrated against measured QE; the fit also found the measurement {:.3} x the model\n# (qe_scale, not a stack parameter).\n{out}",
            fit.qe_scale
        );
    }
    Ok(out)
}

/// The measured QE against the model before and after a fit, per wavelength (CSV, nm).
pub fn comparison_csv(measured: &Measured, fit: &Fit) -> String {
    let rows = measured.usable();
    let colors: Vec<usize> = (0..4).filter(|&c| measured.qe[c].iter().any(|q| q.is_finite())).collect();
    let mut out = String::from("wavelength_nm");
    for &c in &colors {
        let n = crate::pixel::COLORS[c];
        out += &format!(",measured_{n},before_{n},after_{n}");
    }
    out.push('\n');
    for (k, &(row, w)) in rows.iter().enumerate() {
        out += &format!("{:.1}", w * 1e3);
        for &c in &colors {
            let m = measured.qe[c][row];
            let f = |v: f64| if v.is_finite() { format!("{v:.5}") } else { String::new() };
            out += &format!(",{},{},{}", f(m), f(fit.before.qe[c][k]), f(fit.result.qe[c][k]));
        }
        out.push('\n');
    }
    out
}

/// Minimise `f` from `x0` with a simplex of size `step`.
fn nelder_mead(f: &mut dyn FnMut(&[f64]) -> f64, x0: &[f64], step: f64, max_eval: usize, tol: f64) -> Vec<f64> {
    let n = x0.len();
    let mut pts: Vec<(Vec<f64>, f64)> = vec![];
    let mut evals = 0;
    let mut eval = |x: &[f64], evals: &mut usize| {
        *evals += 1;
        f(x)
    };
    pts.push((x0.to_vec(), eval(x0, &mut evals)));
    for i in 0..n {
        let mut x = x0.to_vec();
        x[i] += step;
        let v = eval(&x, &mut evals);
        pts.push((x, v));
    }
    while evals < max_eval {
        pts.sort_by(|a, b| a.1.total_cmp(&b.1));
        if (pts[n].1 - pts[0].1).abs() < tol {
            break;
        }
        let centroid: Vec<f64> = (0..n).map(|j| pts[..n].iter().map(|p| p.0[j]).sum::<f64>() / n as f64).collect();
        let along = |t: f64| -> Vec<f64> { (0..n).map(|j| centroid[j] + t * (pts[n].0[j] - centroid[j])).collect() };
        let xr = along(-1.0);
        let fr = eval(&xr, &mut evals);
        if fr < pts[0].1 {
            let xe = along(-2.0);
            let fe = eval(&xe, &mut evals);
            pts[n] = if fe < fr { (xe, fe) } else { (xr, fr) };
        } else if fr < pts[n - 1].1 {
            pts[n] = (xr, fr);
        } else {
            let xc = if fr < pts[n].1 { along(-0.5) } else { along(0.5) };
            let fc = eval(&xc, &mut evals);
            if fc < pts[n].1.min(fr) {
                pts[n] = (xc, fc);
            } else {
                // Shrink toward the best.
                let best = pts[0].0.clone();
                for p in pts.iter_mut().skip(1) {
                    p.0 = (0..n).map(|j| best[j] + 0.5 * (p.0[j] - best[j])).collect();
                    p.1 = eval(&p.0, &mut evals);
                }
            }
        }
    }
    pts.sort_by(|a, b| a.1.total_cmp(&b.1));
    pts.swap_remove(0).0
}

#[cfg(test)]
mod tests {
    use super::*;

    const BSI: &str = include_str!("../../../pixels/bsi_1p4um.toml");

    #[test]
    fn reads_measured_tables() {
        let m = Measured::from_csv("# lab data\nwavelength,R,G,B\n450,2,10,55\n550,5,60,8\n").unwrap();
        assert_eq!(m.wavelengths, vec![0.45, 0.55]);
        assert_eq!(m.qe[1], m.qe[2]);
        assert!((m.qe[3][0] - 0.55).abs() < 1e-12, "percent read as fractions");
        let u = Measured::from_csv("lambda;Gr\n0.5;0.4\n0.6;\n").unwrap();
        assert!(u.qe[0][0].is_nan() && u.qe[1][1].is_nan() && u.wavelengths[1] == 0.6);
        assert!(Measured::from_csv("nm,X\n500,1\n").is_err());
        assert!(Measured::from_csv("R,G\n1,2\n").is_err());
        assert!(Measured::from_csv("nm,R\n500,abc\n").is_err());
        let template = include_str!("../../../pixels/measured_qe_template.csv");
        assert!(Measured::from_csv(template).unwrap_err().contains("no QE values"));
    }

    #[test]
    fn recovers_filter_density_and_thickness() {
        // "Measured" QE of a stack with denser filters and a thinner filter layer; the fit
        // starts from the nominal stack and finds them again.
        let stack = PixelStack::from_toml(BSI).unwrap();
        let mut truth = stack.clone();
        truth.color_filters.density = [1.4, 0.8, 1.2];
        truth.layers.iter_mut().filter(|l| l.kind == LayerKind::ColorFilter).for_each(|l| l.thickness *= 0.85);
        let base = Settings { rays_per_side: 12, ..Settings::default() };
        let wl: Vec<f64> = (0..=20).map(|i| 0.40 + i as f64 * 0.02).collect();
        let r = crate::simulate(&truth, &Settings { wavelengths: wl.clone(), ..base.clone() }).unwrap();
        let measured = Measured { wavelengths: wl, qe: r.qe.clone() };
        let knobs = [Knob::Density(0), Knob::Density(1), Knob::Density(2), Knob::CfThickness];
        let f = fit(&stack, &base, &measured, &knobs, 400).unwrap();
        assert!(f.rms_before.1 > 0.02, "{:?}", f.rms_before);
        assert!(f.rms_after.1 < 0.003, "{:?} after {} runs", f.rms_after, f.evaluations);
        // Density and thickness trade off (absorption ∝ density × thickness): check products.
        let t = f.knobs[3].1;
        for (c, want) in [1.4, 0.8, 1.2].iter().enumerate() {
            let got = f.knobs[c].1 * t;
            assert!((got - want * 0.85).abs() < 0.05, "{c}: {got} vs {}; {:?}", want * 0.85, f.knobs);
        }
        assert!(f.knobs[0].2.contains("density = ["), "{:?}", f.knobs);
        // Parameters the stack does not have are refused.
        assert!(fit(&stack, &base, &measured, &[Knob::SiThickness], 10).unwrap_err().contains("has none"));
    }

    #[test]
    fn writes_the_fit_into_the_stack_file() {
        let stack = PixelStack::from_toml(BSI).unwrap();
        let mut truth = stack.clone();
        truth.color_filters.density = [1.2, 0.9, 1.1];
        truth.arc.iter_mut().for_each(|a| a.thickness *= 1.2);
        let base = Settings { rays_per_side: 8, ..Settings::default() };
        let wl: Vec<f64> = (0..=10).map(|i| 0.40 + i as f64 * 0.04).collect();
        let r = crate::simulate(&truth, &Settings { wavelengths: wl.clone(), ..base.clone() }).unwrap();
        let measured = Measured { wavelengths: wl, qe: r.qe.clone() };
        let knobs = [Knob::Density(0), Knob::Density(1), Knob::Density(2), Knob::Arc, Knob::QeScale];
        let f = fit(&stack, &base, &measured, &knobs, 300).unwrap();
        let text = patch_stack_toml(BSI, &f).unwrap();
        // Comments stay; the file reads back as the fitted stack.
        assert!(text.contains("# Back-side illuminated 1.4 µm pixel") && text.contains("qe_scale"), "{text}");
        let back = PixelStack::from_toml(&text).unwrap();
        assert_eq!(back.arc.len(), 2);
        for c in 0..3 {
            assert!((back.color_filters.density[c] - f.stack.color_filters.density[c]).abs() < 1e-3);
        }
        for (a, b) in back.arc.iter().zip(&f.stack.arc) {
            assert!((a.thickness - b.thickness).abs() < 1e-4);
        }
        let csv = comparison_csv(&measured, &f);
        assert!(csv.starts_with("wavelength_nm,measured_R,before_R,after_R,measured_Gr"), "{csv}");
        assert_eq!(csv.lines().count(), 12);
    }

    #[test]
    fn fits_the_shape_of_a_superellipse_lens() {
        let text = BSI.replace("height = 0.55", "height = 0.55\nshape = \"superellipse\"");
        let stack = PixelStack::from_toml(&text).unwrap();
        assert!(Knob::LensN.applies(&PixelStack::from_toml(BSI).unwrap()).is_err());
        let mut truth = stack.clone();
        let ml = truth.microlens.as_mut().unwrap();
        ml.radius = Some(ml.base_radius(truth.pitch));
        ml.n = Some(5.0);
        let base = Settings { rays_per_side: 10, cra: 20.0, ..Settings::default() };
        let wl: Vec<f64> = (0..=8).map(|i| 0.42 + i as f64 * 0.04).collect();
        let r = crate::simulate(&truth, &Settings { wavelengths: wl.clone(), ..base.clone() }).unwrap();
        let measured = Measured { wavelengths: wl, qe: r.qe.clone() };
        let f = fit(&stack, &base, &measured, &Knob::parse("lens_n").unwrap(), 60).unwrap();
        assert!(f.rms_after.1 < 0.5 * f.rms_before.1, "{:?} -> {:?}", f.rms_before, f.rms_after);
        let out = patch_stack_toml(&text, &f).unwrap();
        let back = PixelStack::from_toml(&out).unwrap();
        let (a, b) = (back.microlens.unwrap(), f.stack.microlens.clone().unwrap());
        assert!(
            (a.exponents().0 - b.exponents().0).abs() < 1e-3 && (a.base_radius(1.4) - b.base_radius(1.4)).abs() < 1e-3,
            "{out}"
        );
    }

    #[test]
    fn rcwa_refines_a_ray_fit() {
        // "Measured" by RCWA, with the measurement's scale 5 % low: the ray model alone cannot
        // reach it (diffraction), the RCWA stage gets closer, and the scale is found.
        let stack = PixelStack::from_toml(BSI).unwrap();
        let mut truth = stack.clone();
        truth.color_filters.density = [1.3, 0.85, 1.15];
        let wl: Vec<f64> = (0..=8).map(|i| 0.42 + i as f64 * 0.035).collect();
        let rc =
            Settings { wavelengths: wl.clone(), method: Method::Rcwa { harmonics: 7 }, rays_per_side: 10, ..Settings::default() };
        let r = crate::simulate(&truth, &rc).unwrap();
        let measured = Measured { wavelengths: wl, qe: r.qe.map(|q| q.iter().map(|v| v * 0.95).collect()) };
        let knobs = [Knob::Density(0), Knob::Density(1), Knob::Density(2), Knob::QeScale];
        let ray_only = fit(&stack, &Settings { method: Method::Ray, ..rc.clone() }, &measured, &knobs, 120).unwrap();
        let both = fit(&stack, &rc, &measured, &knobs, 120).unwrap();
        assert_eq!(both.stages.len(), 2, "{:?}", both.stages);
        // The ray fit judged by RCWA, the model the data came from.
        let judged = compare(&scaled(crate::simulate(&ray_only.stack, &rc).unwrap(), ray_only.qe_scale), &measured).1;
        assert!(both.rms_after.1 < 0.5 * judged, "{:?}: ray fit {judged}", both.stages);
        assert!(both.rms_after.1 < 0.01, "{:?}", both.stages);
        assert!((both.qe_scale - 0.95).abs() < 0.03, "scale {}", both.qe_scale);
    }
}
