<!-- seo title="Formulas and Models" description="The equations IC Studio computes with: the built-in SPICE engine's device and noise models, measurement functions, the optimizer, Monte Carlo and yield, parasitic extraction, IR drop and EM, mismatch, process emulation and pixels." -->

# Formulas and models

This page lists the equations behind the numbers IC Studio shows: the built-in simulator's
models, the measurement functions specs are written in, the optimizer and statistics, and
the layout-side models (extraction, IR drop, process emulation). Each section names the
source module. The generic technology's coefficients are illustrative 180 nm-class
numbers, not foundry data. A PDK replaces them.

The Design Copilot knows a summary of this page. Ask it how a number comes about and it
works your own values through the formula.

## Numbers and units

- SPICE values are SI (V, A, F, Ω, s, m). Scale suffixes are not case sensitive: `t` 1e12,
  `g` 1e9, `meg` 1e6, `k` 1e3, `m` 1e-3, `mil` 25.4e-6, `u`/`µ` 1e-6, `n` 1e-9,
  `p` 1e-12, `f` 1e-15, `a` 1e-18. Any other trailing letter is a unit and is ignored.
  **`M` is milli, as in SPICE.** Write mega as `meg`. The app's displays use `M` for
  1e6, so do not copy a displayed value such as `1M` into a field. (`ics-core/src/units.rs`)
- Displayed values have 4 significant digits and a prefix T, G, M, k, m, µ, n, p, f or a.
- Layout: 1 dbu = 1 nm (`dbu_per_um` 1000, grid 10 dbu in generic180), and y points up.
  Schematic coordinates are grid points, and y points down.
- Netlist lengths are in metres (`w=1u`), or in µm for model libraries that set
  `.option scale=1e-6`.

## Built-in simulator: device models

The built-in engine is in `ics-sim/src/engine.rs`, `circuit.rs` and `bsim4.rs`.

- V<sub>t</sub> = kT/q at the circuit temperature. The temperature is set by `.temp` or
  `.option temp`, and defaults to 27 °C.
- MOS levels other than 1, 14 and 54 are refused, so **Auto** switches to ngspice.
- **Not supported:** `.tf`, `.sens`, `.pz`, `.four`, `.meas` (use the calculator instead),
  EXP and SFFM sources, Gear integration, MOS levels 2, 3, 8 and 49, voltage- or
  temperature-dependent C and L, and lossy lines.

**Passive elements and sources:**

- **Resistor:** R(T) = R·(1 + tc1·ΔT + tc2·ΔT²), with ΔT = T − 27 °C. Its noise is 4kT/R.
- **Capacitor:** C, constant.
- **Inductor:** L, with a branch row. Mutual inductance is M = k·√(L1·L2), with |k| ≤ 1.
- **Sources:**
  - `PULSE(v1 v2 td tr tf pw per)`: tr and tf default to the time step.
  - `SIN(vo va f td θ φ)` = vo + va·e<sup>−θ(t−td)</sup>·sin(2πf(t−td) + φ).
  - `PWL(...)` accepts `td=` and `r=`.
  - `ac mag [phase]`: `ac` alone means 1.
- **Controlled sources:** E, F, G and H are linear. `POLY` and `VALUE=` become B sources.
- **B sources:** linearised each Newton iteration by central differences.
- **Switch:** the conductance moves between 1/roff and 1/ron along a tanh in log G, with
  hysteresis vh.
- **T line:** lossless (Z0, Td).

**Diode** (defaults: is 1e-14, n 1, cjo 0, vj 1, m 0.5, fc 0.5, tt 0, rs 0, bv ∞, ibv 1 mA,
eg 1.11, xti 3):

```
I = is·(exp(V/(n·Vt)) − 1)              linear above 40·n·Vt; breakdown −ibv·exp(−(V+bv)/(n·Vt))
Q_dep = vj·cjo·(1 − (1 − V/vj)^(1−m))/(1 − m)      (SPICE linearisation above fc·vj)
Q_diff = tt·I
is(T) = is·exp( (T/Tnom − 1)·eg/(n·Vt) + (xti/n)·ln(T/Tnom) )
noise: 2q|I| + kf·|I|^af / f
```

**MOSFET level 1** (the default level). V and I are for NMOS; a PMOS is evaluated with
every sign flipped. Defaults: vto 0, kp from u0·ε<sub>ox</sub>/tox (else 2e-5), lambda 0,
gamma 0, phi 0.6, ld 0, nlev 2. An instance without W and L gets 100 µm.

```
Leff = L − 2·ld          β = kp·W/Leff·m
Vth  = vto + γ·( sqrt(φ − Vbs) − sqrt(φ) )         Vov = Vgs − Vth
cutoff     Vov ≤ 0       Id = 0                      (no subthreshold current)
triode     Vds < Vov     Id = β·(Vov·Vds − Vds²/2)·(1 + λ·Vds)
saturation               Id = ½·β·Vov²·(1 + λ·Vds)
gm  = β·Vov·(1 + λ·Vds)  (sat)        gds = λ·½·β·Vov²  (sat)
gmb = gm·γ / (2·sqrt(φ − Vbs))        (used in the solve, not reported)
```

Level 1 has no mobility degradation, velocity saturation or short-channel effects. The
generic cards make up for the length dependence of λ: the netlister writes one card per
drawn L, with λ = λ<sub>card</sub>·0.5 µm/L.

| generic180 card | vto | kp | λ (at 0.5 µm) | γ | φ | tox | cgso = cgdo | cj | cjsw |
|---|---|---|---|---|---|---|---|---|---|
| nmos | 0.45 | 280 µA/V² | 0.08 | 0.4 | 0.8 | 4 nm | 0.3 nF/m | 1 mF/m² | 0.3 nF/m |
| pmos | −0.45 | 70 µA/V² | 0.1 | 0.4 | 0.8 | 4 nm | 0.3 nF/m | 1 mF/m² | 0.3 nF/m |

Level 1 capacitances:

- The overlaps are constant: C<sub>gs</sub> = cgso·W·m and C<sub>gd</sub> = cgdo·W·m.
- The junctions are cj·AD + cjsw·PD (and the same for the source), using SPICE depletion
  with pb and fc. When a schematic MOS does not give AD/AS/PD/PS, simulation decks estimate
  them from its PCell (see [Layout geometry](#layout-geometry)).
- `capmod=0` (the default) uses Meyer's gate capacitances, with C<sub>ox</sub> =
  ε<sub>ox</sub>/tox·W·L<sub>eff</sub>·m. In saturation C<sub>gs</sub> = ⅔C<sub>ox</sub> and
  C<sub>gd</sub> = 0. In triode, C<sub>gs</sub> = ⅔C<sub>ox</sub>[1 − ((V<sub>gst</sub> −
  V<sub>ds</sub>)/(2V<sub>gst</sub> − V<sub>ds</sub>))²]. Meyer is not charge conserving.
- `capmod=1` uses charge-conserving Ward–Dutton charges, with a 40/60 split in saturation.

Level 1 temperature: kp(T) = kp·(T/Tnom)<sup>−1.5</sup>, and vto and φ follow SPICE's
band-gap rule. λ is not scaled.

**BSIM4** (`level=14` or `54`) follows the BSIM4.8 equations as ngspice has them:

- binning, short-channel and narrow-width V<sub>th</sub>, DIBL, V<sub>gsteff</sub>;
- mobility (mobmod 0–2), velocity saturation, CLM, DIBL, DITS and SCBE output resistance;
- Isub, GIDL/GISL, gate tunnelling;
- capmod 2 charges, and diomod 1 junctions.

Derivatives are exact, computed with dual numbers. It does not evaluate rdsmod ≠ 0, rbodymod,
rgatemod, NQS, or stress and WPE (`sa`, `sb`). It warns about each. Junction area and
perimeter come from AD/AS/PD/PS, or else from dmcg/dmci, which default to 0.

**BJT** (a Gummel–Poon subset; defaults: is 1e-16, bf 100, br 1, vaf = ikf = ∞):

```
If = is·(exp(Vbe/(nf·Vt)) − 1)     Ir = is·(exp(Vbc/(nr·Vt)) − 1)
qb = q1·½·(1 + sqrt(1 + 4·q2)),  q1 = 1/(1 − Vbc/vaf − Vbe/var),  q2 = If/ikf + Ir/ikr
Ic = (If − Ir)/qb − Ibc            Ib = If/bf + ise·(…) + Ir/br + isc·(…)
```

The BJT has no rb current dependence, no tf modulation and no quasi-saturation.

**JFET level 1** (SPICE convention, with no ½): in saturation
I<sub>D</sub> = β(1 + λV<sub>DS</sub>)(V<sub>GS</sub> − vto)².

## Built-in simulator: analyses

**MNA.** The unknowns are the node voltages, plus branch currents for V, L, E, H and B(V)
elements. gmin (1e-12 S) is placed from every node to ground in DC, transient and AC. `.ic`
and `.nodeset` hold a node through 1e10 S.

**DC Newton.** An unknown has converged when

```
|Δx| ≤ (vntol or abstol) + reltol·max(|x_new|, |x_old|)
reltol 1e-4, vntol 1 µV, abstol 1 pA (.option reltol=x also sets the transient tolerance)
```

- With any MOS in the circuit, node steps are clipped to ±0.5 V.
- Diodes and BJTs use SPICE `pnjlim`.
- If the operating point fails, the engine tries in turn:
  1. gmin stepping (1e-2 down to 1e-12 S);
  2. source stepping (k/20).
- A DC sweep starts each point from the previous solution.

**Transient:**

- Integration is trapezoidal, except backward Euler at the start, after each breakpoint,
  after a Newton failure and when ringing is detected. There is no Gear.
- The step is controlled by the local truncation error, ngspice style:

```
tol = max(abstol + lte_reltol·|i|, lte_reltol·max(|q|, chgtol)/h)     lte_reltol 1e-3, trtol 7
del = trtol·tol / (factor·|DD|)        factor 1/12 (trap), ½ (BE); DD a divided difference of q
h_LTE = sqrt(del) (trap), del (BE)     next h = min(h_LTE, 2·h)
```

- The maximum step is min(tstep, tstop/100, tmax).
- On a Newton failure the step is divided by 8.
- Every accepted point is output, with no resampling.
- `uic` skips the operating point.

**AC** is linearised at the operating point. Frequencies run f<sub>k</sub> =
f<sub>start</sub>·b<sup>k/n</sup> (dec or oct), with the stop frequency appended, or `lin`.

**Noise** uses the adjoint method: S<sub>out</sub> = Σ|z<sub>a</sub> −
z<sub>b</sub>|²·S<sub>i</sub>.

- `onoise_spectrum` is √S<sub>out</sub> (V/√Hz).
- `inoise_spectrum` is √S<sub>out</sub>/|H| at the input source. For a current source it is
  in A/√Hz.
- The integrated output noise is written to the log.

| Source | PSD (A²/Hz) |
|---|---|
| resistor | 4kT/R |
| MOS level 1, channel | (8/3)·kT·g<sub>m</sub> |
| MOS level 1 flicker, `nlev=2` (default) | kf·g<sub>m</sub>²/(f<sup>af</sup>·W·L<sub>eff</sub>·C<sub>ox</sub>') |
| `nlev=1` | kf·I<sub>D</sub><sup>af</sup>/(f·W·L<sub>eff</sub>·C<sub>ox</sub>') |
| `nlev=0` | kf·I<sub>D</sub><sup>af</sup>/(f·C<sub>ox</sub>'·L<sub>eff</sub>²) |
| diode, BJT | 2q\|I\| + kf\|I\|<sup>af</sup>/f |
| BSIM4 | thermal tnoimod 0 (4kT·μ\|Q<sub>inv</sub>\|/L<sub>eff</sub>² with R<sub>ds</sub>); flicker fnoimod 1 (unified noia/noib/noic) or 0; gate shot 2q\|I<sub>g</sub>\| |

With `nlev` 2 or 3, `af` is the frequency exponent, so kf has different units than with
nlev 0 or 1. C<sub>ox</sub>' = ε<sub>ox</sub>/tox, and is taken as 3.453e-4 F/m² when tox is
not given.

**Operating point report:**

- MOS: region, id, vgs, vds, vth, gm, gds, cgs/cgd/cgb (overlaps included).
- BJT: ic, ib, vbe, vce, gm, β = ic/ib.
- Diode: vd, id, gd.
- Resistor: v, i, p = v²/R.

## Expressions

These apply to `.param`, `{…}` and B sources (`ics-sim/src/expr.rs`).

- Operator precedence, from lowest to highest: `?:`, `||`, `&&`, comparisons (giving 1 or 0),
  `+ −`, `* / %`, unary `− + !`, then `^` or `**`. The power operators are
  right-associative, so −2^2 = −4.
- Functions:
  - sqrt, exp, `log` (= ln), ln, log10, abs;
  - trigonometric and hyperbolic functions, atan2;
  - floor, ceil, int, nint/round;
  - sgn, sign, pow, pwr(x,y) = sgn(x)·|x|<sup>y</sup>;
  - min and max (any number of arguments), limit(x,a,b), if(c,a,b).
- **`agauss`, `gauss`, `aunif` and `unif` return their nominal value.** Statistical
  variation comes from Monte Carlo, not from the deck.

## Measurement functions

These are the functions of the Waveform Calculator and of spec expressions
(`ics-sim/src/calc.rs`). Measurements of complex (AC) data use the magnitude. A rising
crossing of level L is a < L ≤ b, and a falling one is a > L ≥ b. Crossings are interpolated
linearly, or in log f for the AC functions.

| Function | Definition |
|---|---|
| `db(w)` / `db10(w)` | 20·log10\|w\| / 10·log10\|w\| |
| `phase(w)` | atan2 in degrees, unwrapped wherever consecutive points jump by more than 180° |
| `deriv(w)` / `integ(w)` | central differences / cumulative trapezoid |
| `avg(w)` / `rms(w)` | ∫y dx / span, and √(∫y² dx / span), by the trapezoid rule |
| `min`, `max`, `pp`, `xmin`, `xmax` | extremes, pp = max − min, and where they occur |
| `value(w, x)` | linear interpolation at x |
| `cross(w, L[, n[, edge]])` | x of the n-th crossing (n < 0 counts from the end) |
| `delay(w1,L1,w2,L2[,e1,e2])` | t(first crossing of w2 at L2) − t(first crossing of w1 at L1) |
| `risetime(w[,p1,p2])` / `falltime` | between 10 % and 90 % (default) of the **whole waveform's** min..max |
| `overshoot(w)` | 100·(peak − y<sub>final</sub>)/(y<sub>final</sub> − y<sub>0</sub>) |
| `settling(w[,pct])` | the **absolute time** when w last leaves ±pct % (default 2 %) of the step around y<sub>final</sub> |
| `bandwidth(w[,drop])` | first falling crossing of g<sub>0</sub> − drop, where g<sub>0</sub> is the gain in dB at the **lowest frequency** (not the peak) and drop defaults to 3.0103 dB |
| `ugf(w)` | first 0 dB crossing |
| `phasemargin(w)` | 180 + φ(w) at the ugf |
| `gainmargin(w)` | −db(w) where φ(w) first crosses −180° |

For the margins, φ is the unwrapped phase minus the multiple of 180° nearest its value at the
lowest frequency. A response measured from the inverting input (about ±180° at DC) then
reads as the loop gain (0° at DC), and both inputs give the same margin.

A spec passes when its value is finite and min ≤ v ≤ max. An empty limit is no limit.
The measurement table under the plots shows MIN, MAX, PEAK-PEAK and FINAL. For AC it shows
GAIN @ fstart, PEAK, f −3 dB and UNITY GAIN f.

## Spec advice and derive

**Spec advice** (`ics-core/src/advice.rs`) classifies a spec from its expression:
phasemargin, gainmargin, ugf/bandwidth, gain (`db(`, `mag(`), delay
(`delay(`/`risetime(`/`falltime(`), period, overshoot, settling, current (`i(`) and
voltage (`v(`). It then gives text rules for the direction of the miss. For example, low
gain means: first check that every amplifying device is saturated, then use a longer L
(higher r<sub>o</sub>) or less current per stage. Low bandwidth means more tail current,
wider input devices or less C<sub>L</sub>. A current mirror gives
I<sub>out</sub> = I<sub>ref</sub>·(W/L)<sub>out</sub>/(W/L)<sub>ref</sub>. No sensitivity is
computed; the optimizer and sweeps find the numbers.

Operating-point regions:

- Level 1: cutoff when V<sub>ov</sub> ≤ 0, triode when V<sub>DS</sub> < V<sub>ov</sub>,
  otherwise saturation.
- BSIM4 also reports subthreshold.
- The advice warns when the saturation margin |V<sub>DS</sub>| − (|V<sub>GS</sub>| − |V<sub>th</sub>|)
  is below 0.1 V.

**Derive** (`ics-core/src/derive.rs`, `ics-sim/src/derive.rs`) picks the knobs.

- MOS devices with the same polarity, source net, W, L and m form a group. A group is then
  given a role:
  - **input pair:** two devices with different gates and a source that is not a rail;
  - **mirror:** devices sharing a gate, one of them diode connected;
  - **current source:** source on a rail, gate not driven by a drain;
  - **device:** anything else.
- Matched devices share one variable (`M1.w,M2.w`).
- All variables are log scaled. MOS values snap to 10 nm.
  - W ranges over [max(W<sub>0</sub>/4, W<sub>min</sub>), 4·W<sub>0</sub>].
  - L is offered only when L<sub>0</sub> ≥ 1.2·L<sub>min</sub>. Its range is
    [L<sub>0</sub>, 4·L<sub>0</sub>], so it never shrinks.
  - Circuit R and C range over [x/4, 4x]. A capacitor to ground is a load, not a knob.
- The power guard adds an `idd` spec with the limits [0.5·I<sub>0</sub>, 2·I<sub>0</sub>].

After sizing, derive checks the result:

- It warns about devices that left saturation or whose margin fell below 0.1 V.
- It warns when supply power rose more than 1.5×. Power is
  P = Σ −(V<sub>p</sub> − V<sub>n</sub>)·I over the sources, counting positive terms only.
- It warns about matched pairs that shrank. The offset grows by
  k = √(W<sub>0</sub>L<sub>0</sub>/(W<sub>1</sub>L<sub>1</sub>)) (Pelgrom), and k > 1.2 warns.
- The tphl/tplh presets measure at half the DC value of the supply source (the source on a
  `vdd*` net), so 0.9 V for 1.8 V. Without a supply source they use 0.9 V.

## Optimizer

The optimizer is in `ics-core/src/optimize.rs`, and its evaluation in
`ics-sim/src/optimize.rs`.

Only specs with a limit count. With norm(l) = |l| (or 1 when l = 0):

- **Score** (how far it fails):
  S = Σ (min − v)/norm(min) [v < min] + (v − max)/norm(max) [v > max].
  A failed measurement adds 1e3.
- **Margin:** M = min over specs of min((v − min)/norm(min), (max − v)/norm(max)).

The objective is minimised. A failing point scores f = S. A passing point scores in
[−1, 0], depending on the goal:

| Goal | Objective of a passing point |
|---|---|
| Margin (default) | −clamp(M, 0, 1) |
| Min area | −1/(1 + A/A<sub>ref</sub>), with A = Σ W·L·m·nf over the MOS devices |
| Least change (derive) | −(0.5 + 0.5·clamp(M/0.05, 0, 1))/(1 + 5·Δ), with Δ the mean distance from the start in unit coordinates |

How the search runs:

- Variables map to u ∈ [0, 1]. A linear variable is x = min + u(max − min). A log variable
  is x = exp(ln min + u·(ln max − ln min)). Then x snaps to the step.
- The algorithm is a deterministic compass (pattern) search.
  - It starts from the schematic's values with step δ = 0.25.
  - It polls ±δ along each axis and moves on the first improvement.
  - After a poll with no improvement it halves δ, and it stops when δ < 1/1024.
  - It also stops at the budget: 60 evaluations by default, 80 in derive.
- The only constraints are the box bounds and the step. Spec limits enter as penalties.

## Sweeps, corners, Monte Carlo and yield

**Sweeps** (`ics-core/src/sweep.rs`):

- The points are corners × temperatures × the Cartesian product of the variables. The last
  variable varies fastest, and the limit is 2000 points.
- `a:step:b` gives floor((b − a)/step) + 1 values.

**Built-in Monte Carlo** (`ics-sim/src/montecarlo.rs`). The defaults are 100 runs, seed 1,
vto_sigma 20 mV, kp_sigma 3 %, avt 5 mV·µm and abeta 1 %·µm. For each MOS device:

```
dV = dV_global(polarity) + N(0,1) · avt   / sqrt(W · Leff · m)
dB = dB_global(polarity) + N(0,1) · abeta / sqrt(W · Leff · m)
delvto = +dV (NMOS), −dV (PMOS)        mulu0 = max(1 + dB, 0.05)
level 1: vto += delvto, kp *= mulu0     BSIM4: delvto, mulu0
```

The avt you type (or the default) is a per-device coefficient. One fitted from pair data
(see [Mismatch](#mismatch-from-matched-pairs)) is a pair coefficient, and it is divided by
√2 for each device.

**Sweep-manager Monte Carlo variables** (`McVar`):

- A value is value = nominal + Σ coef·draw. A draw is N(0,1) for Gauss or U(−1,1) for
  Uniform; for Uniform, sigma is the half range.
- Sigma can be written three ways:
  - `5%` means |nominal|·0.05.
  - A plain number is used as it is.
  - `A/sqrt(wl)` means A/√(W<sub>µm</sub>·L<sub>µm</sub>).
- Mismatch variables draw once per instance. Others share one draw.
- The random generator is splitmix64 with Box–Muller.

**Corners from silicon** (`ics-sim/src/corners.rs`). Corner tt is the measured mean. For
another corner, each primary parameter P is moved to its mean + z·σ, with
δ<sub>P</sub> = z·σ<sub>P</sub>. Every other parameter a follows its conditional mean:

```
t_a = μ_a + Σ_aP · Σ_PP⁻¹ · δ_P
```

The model knobs are then fitted with Levenberg–Marquardt.

**Yield prediction** (`ics-core/src/predict.rs`). Each spec gets a surrogate over the
standardised draws.

- The surrogate is quadratic when there are at least 2·(1 + n + n(n+1)/2) runs. Otherwise
  it is linear, which needs n + 3 runs.
- It is a least-squares fit with a 1e-6·m ridge.
- Fit quality:
  - Q² = 1 − PRESS/SST (leave one out), where PRESS = Σ(e<sub>i</sub>/(1 − h<sub>i</sub>))².
  - RMSE<sub>LOO</sub> = √(PRESS/m), in the spec's units.
  - The window warns when Q² < 0.9.
- Prediction draws 100 000 samples, each y = f(x) + RMSE<sub>LOO</sub>·N(0,1).
  - Yield = the pass share × (runs with a value / runs).
  - Joint yield needs every limited spec to pass.
- Cpk = min((μ − LSL)/3σ, (USL − μ)/3σ).
- The sensitivity of input i is its main-effect variance over Var(y). The main effect is
  b² + 2c² for Gauss inputs and b²/3 + 4c²/45 for Uniform inputs.
- The simulated yield's 95 % interval is the Wilson interval, with z = 1.96:

```
centre = (p + z²/2n) / (1 + z²/n)
half   = z · sqrt(p(1−p)/n + z²/4n²) / (1 + z²/n)
```

## Mismatch from matched pairs

`ics-core/src/mismatch.rs` (Simulation > Silicon Correlation) fits the Pelgrom law to
measured pairs (`device, w, l, dvt[, dbeta]`):

```
σ²(ΔX) = A² / (W·L) + σ0²          A in V·µm (Vt) or µm (β)
```

The fit works as follows.

1. Rows are grouped by (W, L). The fit needs at least 3 pairs per size and at least 2 sizes.
2. Each group gives a sample variance s², with x = 1/(WL).
3. The fit is weighted least squares, with weights (n − 1)/(2·(A²x)²) (that is,
   1/Var(s²)).
4. If σ0² < 0, the fit falls back to a line through the origin.

A is the coefficient of a **pair difference**. One device has σ = A/√(2WL). The model the
simulator uses drops σ0.

Learn-course rule of thumb: σ(V<sub>th</sub>) = A<sub>Vt</sub>/√(WL) per device, with
A<sub>Vt</sub> ≈ 5 mV·µm. A pair is √2 larger. A 2 × 1 µm device gives 3.5 mV.

## Layout geometry

Values here are for generic180, in nm (`ics-core/src/pcell.rs`, `ics-core/src/tech.rs`).

| Layer | min width | min spacing |
|---|---|---|
| active | 220 | 280 |
| poly | 180 | 240 |
| metal1 | 230 | 230 |
| metal2, metal3 | 280 | 280 |
| nwell | 860 | 1400 |
| nplus, pplus | 440 | 440 |

- Contacts are 220 nm cuts with 250 nm spacing and 100/60 nm enclosure. via1 and via2 are
  260 nm cuts with 260 nm spacing and 60 nm enclosure.
- The device rules are: gate to contact 160, poly endcap 220, implant enclosure 180, nwell
  enclosure 430.

**MOS PCell** (w per finger, nf fingers):

```
plate      = max(cut + 2·enc, m1 min width)                         = 420
sd_end     = enc_bottom + cut + gate_contact                        = 480
sd_in      = cut + 2·gate_contact                                   = 540
pitch      = L + sd_in
active_len = 2·sd_end + nf·L + (nf − 1)·sd_in
w_min      = max(active min width, cut + 2·enc)                     = 420
```

- The implant is the active grown by 180. For a PMOS the nwell is the active grown by 430.
- From the schematic, the finger count is ceil(W/5 µm)·m, so a finger is at most 5 µm wide.
- Stretching the PCell in y changes W. Stretching it in x changes L by d/nf per finger.

**Junction parameters** (AD, AS, PD, PS):

- Before there is a layout, the simulation deck estimates them for every MOS that does not
  give them, from the PCell it would be drawn as (as a PDK's CDF callbacks do): ceil(W/5 µm)
  fingers per device of its m, the pieces alternating source, drain, source. The CDL / LVS
  netlist keeps the schematic as drawn.
- Post-layout netlists replace the estimate with the extracted values.
- Each diffusion piece gives its area, and its perimeter without the gate edges. A piece
  shared by several terminals is split equally.
- When drain and source are on one net, each gets half.
- For generic180, one finger has AS = AD = 480·w and PS = PD = 960 + w.
- An inner piece is A = 540·w, P = 1080.
- The generic cards give cj = 1 mF/m² and cjsw = 0.3 nF/m, so a 1 µm finger's drain is about
  1 fF at zero bias.

Extracted devices:

- MOS: on a rectangular gate, L is across the diffusion and W = area/L. On a bent gate,
  L = area/mean edge and W = mean edge − (1 − bend_corner)·|a − b|/2.
- Resistor: R = R<sub>s</sub>·L/W.
- Capacitor: C = (C<sub>area</sub>·A + C<sub>perim</sub>·P). The generic cpip is 2.65 fF/µm².

**Generators:**

- Via array: n = floor((len − cut)/(cut + space)) + 1 cuts per axis, centred.
- Guard ring: width max(active, cut + 2·enc, m1) = 420, gap max(active space,
  2·implant enclosure) = 360.
- Tracks: pitch = min width + min spacing (metal1: 460 nm).
- Matched placement:
  - The unit is the gcd of the widths, halved while it is larger than 5 µm, with at most
    32 units.
  - The order is interdigitated or common centroid. For example, [2,2] gives ABBA and
    [4,4] gives ABABBABA.
  - Two rows are used from 8 units, point-symmetric about the centre, with a dummy at each
    row end.

**Router** (`ics-core/src/route.rs`). The router is A* on a grid.

- The pitch is max(pad + spacing, cut + cut space) (generic 660 nm).
- Nets are routed in order of their pin bounding-box half-perimeter, shortest first.
- Step cost:
  - metal1: 4× in either direction;
  - metal2: 1× horizontal, 3× vertical;
  - metal3: 1× vertical, 3× horizontal.
- A via costs 2 pitches.
- Rip-up:
  - Crossing another net costs 20·(1 + rips)·pitch per node.
  - A net can be ripped at most 3 times.

## Parasitic extraction

`ics-core/src/pex.rs`. Each conducting layer has these coefficients:

- `area` in aF/µm²;
- `fringe` in aF/µm;
- `coupling` in aF;
- `resistance` R<sub>s</sub> in Ω/□;
- `em` in A/µm.

Each via has a resistance per cut and an EM limit per cut.

| generic180 | area | fringe | coupling | R<sub>s</sub> | em |
|---|---|---|---|---|---|
| poly | 100 | 45 | 7 | 7.5 | 0.05 mA/µm |
| metal1 | 38 | 40 | 17 | 0.08 | 1.0 mA/µm |
| metal2 | 16 | 32 | 20 | 0.08 | 1.0 mA/µm |
| metal3 | 10 | 28 | 20 | 0.06 | 1.5 mA/µm |

A contact cut is 10 Ω and 0.1 mA, and a via1 or via2 cut is 5 Ω and 0.2 mA.

**Ground capacitance:**

```
C_gnd = 1e-18 · Σ_layers ( area · A_µm² + fringe · P_µm )
```

A and P are measured on the net's merged shapes. Nothing shields this term: a wire under
another layer keeps its full ground capacitance.

**Lateral coupling** (same layer):

- C<sub>c</sub> = coupling·L/s aF, where L is the facing length and s the gap.
- It applies to facing parallel edges of different nets, with s ≤ 5 × min spacing, and only
  to the nearest unobstructed neighbour.
- Corner-to-corner coupling is ignored.

**Vertical coupling:**

- The plate coefficient between an upper layer (c<sub>hi</sub>) and a lower one
  (c<sub>lo</sub>) is 1/(1/c<sub>hi</sub> − 1/c<sub>lo</sub>) aF/µm². In generic180 this is
  m1–m2 27.6 and m2–m3 26.7.
- The overlap area excludes shapes on the layers between the two.
- An overlap extracted as a capacitor device (the generic `cpip` on poly / poly2) is in the
  netlist with its own value and is not counted again as coupling.

**Resistance.** Wires are split into tiles of at most 10 squares. Two tiles touching in x
are joined by

```
R = Rs · ( w_a/(2·h_a) + w_b/(2·h_b) )
```

and likewise in y with w and h swapped. Each via cut is one resistor. Values below
1 mΩ are merged.

**Post-layout netlist:**

- Caps below 1 aF are dropped.
- A net's ground C is spread over its RC nodes in proportion to wire area.
- The port is the node with the most wire area.
- MOS terminals move to the node nearest the gate centre, so gate resistance is in series.

**Net figures:**

- R<sub>ab</sub> is solved on the conductance network.
- The "RC" figure is the worst R × (C<sub>gnd</sub> + the five largest couplings). It is a
  rough bound, not an Elmore delay.

**While drawing a wire:**

- R = R<sub>s</sub>·len/w;
- C = (area·(len + w)·w + fringe·2·(len + w))·1e-18;
- the EM limit is I<sub>max</sub> = em·w.

## IR drop and electromigration

`ics-core/src/emir.rs` uses the same tiled network.

- Branch R is at least 1 mΩ.
- Pads are top-cell labels with the net name, held at drop 0.
- A MOS with one terminal on the net draws its current where its contacts land: the
  nearest metal1 tile within the gate's size + 1 µm of the gate, else the nearest tile of
  any layer. A metal2 rail over the gate therefore still takes the current through its via.
  The current comes from the schematic's operating point, split equally over the layout
  fingers.
- The solver finds G·drop = I by preconditioned conjugate gradients, to a relative
  residual of 1e-11.
- Branch current is I = Δdrop/R.
  - In a wire, J = |I|/(shared edge µm) is checked against the layer's `em` (A/µm).
  - In a via, |I| per cut is checked against the via's `em`.
- The analysis is DC only: no RMS or peak current and no temperature derating. On a ground
  net the drop reads as a rise.

## Process emulation

`ics-core/src/process.rs` and `ics-core/src/dopants.rs` simulate a 2-D cross section on a
uniform grid (default 10 nm). Constants: k = 8.617333e-5 eV/K, q = 1.602e-19 C.

**Implant.** The profile is a Gaussian:

```
C(d) = Q / (sqrt(2π) · ΔRp) · exp( −(d − Rp)² / (2·ΔRp²) )     cm⁻³
```

- Q is the dose in cm⁻², and d is the depth below the surface.
- Rp and ΔRp come from LSS tables for silicon, interpolated log-log. BF₂ uses boron at
  E·11/49.
- The lateral spread is σ = 0.8·ΔRp.
- Every material stops ions like silicon.

**Diffusion.** D = D<sub>0</sub>·exp(−E<sub>a</sub>/kT). The solver is backward Euler,
one dimension at a time, with no flux into non-semiconductors.

| Dopant | D<sub>0</sub> (cm²/s) | E<sub>a</sub> (eV) |
|---|---|---|
| B | 0.76 | 3.46 |
| P | 3.85 | 3.66 |
| As | 0.066 | 3.44 |
| Sb | 0.214 | 3.65 |

The model is intrinsic only. It has no TED, OED, concentration dependence or segregation.

**Oxidation.** The model is Deal-Grove on (100) Si:

```
B   = C1 · exp(−E1/kT)  µm²/h        B/A = C2 · exp(−E2/kT)  µm/h
dry: C1 772, E1 1.23, C2 3.71e6, E2 2.00      wet: C1 386, E1 0.78, C2 9.70e7, E2 2.05
τ   = (x0² + A·x0)/B                  x(t) = ( −A + sqrt(A² + 4B·(t + τ)) ) / 2
```

- 44 % of the grown oxide consumes silicon.
- The model is vertical only. It has no bird's beak and no thin-oxide correction.

**Readouts:**

- Mobility follows Caughey–Thomas at 300 K:
  µ = µ<sub>min</sub> + (µ<sub>max</sub> − µ<sub>min</sub>)/(1 + (N/N<sub>ref</sub>)<sup>α</sup>).
  - Electrons: 68.5, 1414, 9.2e16, 0.711.
  - Holes: 44.9, 470.5, 2.23e17, 0.719.
- Sheet resistance is R<sub>s</sub> = 1/Σ(q·µ·|N<sub>net</sub>|·h) over a layer of one type.
- Junctions are where N<sub>net</sub> changes sign, linearly interpolated.

**Deposit and etch:**

- A conformal deposit or isotropic etch uses an exact distance transform.
- An anisotropic etch works per column and stops at the first material not in its list.

## Pixels and photodiodes

**Pinned photodiode** (`ics-core/src/pinned_pd.rs`). The model is a 1-D depletion
approximation of the doping profile, with ε<sub>Si</sub> = 11.7·ε<sub>0</sub>:

- E(z) = (q/ε)∫N dz and φ = −∫E dz.
- The depletion edges are where the charge balances.
- The pinning voltage is V<sub>pin</sub> = φ at E = 0.
- C/A = ε/(z* − z<sub>t</sub>) + ε/(z<sub>b</sub> − z*).
- The n dose ΣN·h is an upper bound on the electrons it holds.

**Photodiode technology parameters** (`PhotodiodeDef`):

```
C_PD = (cap_area · A + cap_perimeter · P) · 1e-15                        F
I_dark = (dark_area · A + dark_perimeter · P) · 1e-18 · exp( (Ea/k)·(1/T_ref − 1/T) )   A
doubling temperature ΔT = k·T²·ln2 / Ea
```

E<sub>a</sub> is 0.6 eV for depletion generation and 1.12 eV for diffusion. T<sub>ref</sub>
defaults to 60 °C. In generic180, cap_area is 0.5 fF/µm² and dark_area is 3 aA/µm².

**Pixel test bench** (Verify > Pixel Circuit, `ics-sim/src/pixel.rs`). The bench injects
500 e⁻ into the FD and then the PD. From the response:

```
C_FD = 500·q / |ΔV_FD|          CG = q / C_FD  (µV/e⁻)        A_SF = ΔV_col / ΔV_FD
C_PD = 500·q / |ΔV_PD|          full well N_FW = C_PD·(V_reset − V_sat)/q
lag  = C_PD·(V_dark − V_q)/q    (electrons left after transfer)
kTC noise = sqrt(k·T·C_FD)/q    read noise = sqrt(∫ S² df), f ≥ 1 kHz
CDS noise = sqrt(∫ S² · 4·sin²(π·f·Δt) df)       in e⁻: µV / CG_col
```

The pinned PD is modelled in the circuit as its storage cap plus a branch
I = G·(min(V, V<sub>pin</sub>) − V<sub>q</sub>), so it never fills above V<sub>pin</sub>.

## OPC and dummy fill

**OPC** (`ics-core/src/opc.rs`). The litho model is a Gaussian blur followed by a constant
threshold, computed exactly per rectangle:

```
I(x,y) = Σ ¼ [erf((x1−x)/(√2σ)) − erf((x0−x)/(√2σ))] · [erf((y1−y)/(√2σ)) − erf((y0−y)/(√2σ))]
```

- Resist prints where I ≥ threshold.
- EPE is the signed distance to the contour along the outward normal.
- Correction steps:
  - The first step is −gain·EPE.
  - After that, each step is a secant step with the slope clamped to [0.3, 4].
- Defaults, with w and s the layer's min width and spacing:

| Parameter | Default |
|---|---|
| σ | 0.45·w |
| threshold | 0.5 |
| fragment | 2·w |
| max outward move | s/3 |
| max inward move | w/4 |
| iterations | 12 |
| gain | 0.6 |

**Dummy fill** (`ics-core/src/fill.rs`). Density = covered area / window area. The defaults
are:

- square size max(4w, 1 µm, √min_area);
- spacing max(2s, 0.5 µm);
- keepout max(2s, 1 µm);
- window 100 µm, step 50 µm, unless the deck has a density rule;
- target min(rule min + 5 %, (min + max)/2).

## Process data (WAT/SPC)

`ics-core/src/process_data.rs`:

- Cp = (USL − LSL)/6σ and Cpk = min((μ − LSL)/3σ, (USL − μ)/3σ), with σ using n − 1.
- The control chart is an individuals chart of the wafer means.
  - σ̂ = mean moving range/1.128, and the limits are centre ± 3σ̂.
- Western Electric rules:
  - 1 point beyond 3σ;
  - 2 of 3 beyond 2σ;
  - 4 of 5 beyond 1σ;
  - 8 in a row on one side.
- Variance propagation:
  - backward: Σ<sub>k</sub> = A·Σ<sub>m</sub>·Aᵀ, with A = (SᵀWS)⁻¹SᵀW and
    W = diag(1/σ²);
  - forward: Σ<sub>m</sub> = S·Σ<sub>k</sub>·Sᵀ.

## Formulas the course teaches

These are the hand formulas of the [Learn course](learn/index.md), with the course's numbers.

| Topic | Formula | Course example |
|---|---|---|
| Ohm, power | V = IR, P = VI | 1.8 V across 9 kΩ → 200 µA, 360 µW |
| RC | τ = RC, f<sub>−3dB</sub> = 1/(2πRC) | 1 kΩ, 1 nF → 1 µs, 159 kHz |
| Overdrive | V<sub>ov</sub> = V<sub>GS</sub> − V<sub>th</sub> | V<sub>th</sub> = 0.45 V |
| Square law | I<sub>D</sub> = ½k'(W/L)V<sub>ov</sub>²(1 + λV<sub>DS</sub>) | k'<sub>n</sub> 280 µA/V², k'<sub>p</sub> 70 µA/V²; W/L 2/1, V<sub>GS</sub> 0.7 V → 17.5 µA |
| Transconductance | g<sub>m</sub> = 2I<sub>D</sub>/V<sub>ov</sub> = √(2k'(W/L)I<sub>D</sub>) | 20 µA at 0.2 V → 200 µS |
| Output resistance | r<sub>o</sub> = 1/(λI<sub>D</sub>), λ = 0.08·0.5 µm/L | L = 2 µm → λ = 0.02 |
| Common source | A<sub>v</sub> = −g<sub>m</sub>(R<sub>D</sub> ‖ r<sub>o</sub>) | 145 µS, 1.43 MΩ, 50 kΩ → −7.0 |
| Bandwidth | f<sub>−3dB</sub> = 1/(2π(R<sub>D</sub>‖r<sub>o</sub>)C<sub>L</sub>), f<sub>u</sub> ≈ g<sub>m</sub>/(2πC<sub>L</sub>) | 100 fF → 33 MHz, 230 MHz |
| Mirror | I<sub>out</sub> = I<sub>ref</sub>·(W/L)<sub>2</sub>/(W/L)<sub>1</sub> | 20 µA → 20.1 µA |
| 5T OTA | A<sub>v</sub> = g<sub>m1</sub>(r<sub>o2</sub> ‖ r<sub>o4</sub>) | ≈ 121 (42 dB) |
| Inverter threshold | V<sub>M</sub> = (V<sub>tn</sub> + r(V<sub>DD</sub> − \|V<sub>tp</sub>\|))/(1 + r), r = √(k'<sub>p</sub>W<sub>p</sub>/k'<sub>n</sub>W<sub>n</sub>) | W<sub>p</sub> = 2W<sub>n</sub> → 0.83 V |
| Gate delay | t<sub>p</sub> ≈ 0.69·R<sub>on</sub>C<sub>L</sub> ≈ C·V<sub>DD</sub>/(2I) | 5 kΩ, 20 fF → 69 ps |
| Dynamic power | P = α·C<sub>L</sub>·V<sub>DD</sub>²·f | 0.1·20 fF·3.24·1 GHz = 6.48 µW |
| Ring oscillator | T = 2·N·t<sub>p</sub> | 5 × 14 ps → 140 ps |
| Pelgrom | σ(V<sub>th</sub>) = A<sub>Vt</sub>/√(WL) | 5 mV·µm, 2 × 1 µm → 3.5 mV |
| Miller OTA | f<sub>u</sub> = g<sub>m1</sub>/(2πC<sub>c</sub>), output pole g<sub>m6</sub>/(2πC<sub>L</sub>) 2–3× above, R<sub>z</sub> ≈ 1/g<sub>m6</sub> | demo `ota_miller` |
| LDO | V<sub>out</sub> = V<sub>ref</sub>(1 + R1/R2) | 0.9 V, R1/R2 = 1/3 → 1.2 V |
| DRAM read | ΔV = (V<sub>DD</sub>/2)·C<sub>s</sub>/(C<sub>s</sub> + C<sub>BL</sub>) | 10 fF, 50 fF → ±150 mV |

## Known simplifications

- Level-1 MOS has no subthreshold current: below V<sub>th</sub> only gmin conducts. Use a
  BSIM4 PDK for leakage and weak inversion.
- Level-1 Meyer capacitances (capmod 0) do not conserve charge. Use capmod 1 for
  charge-sensitive circuits.
- Ground capacitance is not shielded, and fringe-to-other-layer capacitance is not modelled.
- The wire "RC" is R<sub>max</sub> × C<sub>total</sub>, not an Elmore delay.
- EM is a DC current-density check only.
- Pre-layout junctions are estimated from the PCell; the drawn layout's diffusion can be
  larger (shared or extended diffusion), which only the post-layout netlist sees.
- Process emulation uses intrinsic diffusion and vertical Deal-Grove oxidation.
- The optimizer is a local pattern search. It finds a nearby passing point, not a global
  optimum.
