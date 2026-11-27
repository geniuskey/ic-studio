IC STUDIO PRIMER (what you know before calling a tool; the manual has the details)

The window
- Activity bar (far left): the four workspaces Home, Schematic, Simulation, Layout (Ctrl+1 ... Ctrl+4; Cmd on macOS)
  and the Settings gear. The open cells are tabs shared by all workspaces: switching workspace shows the same cell in
  another view.
- Explorer (left, Ctrl+B): libraries > cells > views (schematic, layout, pcell). Click a cell to open it.
- Right panel: the property inspector in Schematic (select one device to edit Name, values, Model, Width,
  Multiplier; source DC value, AC magnitude, Transient waveform), the Setup panel in Simulation (Backend, Corner,
  Temperature, Include layout parasitics, Analyses, Extra SPICE lines), the layers palette and the Design Rule Check /
  LVS sections (Run DRC, Run LVS) in Layout.
- Bottom panel (Ctrl+J): Console (log), CIW (SKILL prompt), Problems (live schematic check), Netlist.
- Command palette: Ctrl+K (or Search commands... at the top right) finds every command, including the ones Beginner
  Mode hides from the menus (process emulation, pixel optics, sign-off runsets, SKILL, rule decks, extensions). When
  the user cannot find a menu item, suggest the palette or Help > Beginner Mode.
- Tool windows dock inside the main window; the pop-out button moves one to its own OS window.
- Status bar: workspace, zoom, selection, technology, ngspice status, "MCP live" while an agent is connected.

Concepts
- Project = a folder with ic-studio.json, tech.json and one folder per library. Library = group of cells (demo:
  work, pcells). Cell = up to three views: schematic, simulation setup (sim.json), layout. Technology (PDK) = layers,
  GDS numbers, display, design rules and electrical coefficients; new projects use generic180 (illustrative 180 nm
  numbers, not foundry data). Cell notes and specs travel with the cell.
- Units: schematic coordinates are grid points (y down); layout is in database units, 1 dbu = 1 nm (y up); the tools
  take layout coordinates in µm. Values are SPICE strings: 10k, 2.2p, 180n, 1meg. In SPICE `m` and `M` are both milli
  (1e-3): write mega as `meg`. The app displays 1e6 as "1M"; never copy such a display value into a netlist field.

Everyday flow (and where it is in the window)
1. Schematic (Ctrl+2): keys W wire, T net label, R C L resistor/capacitor/inductor, V I sources, D diode, N P
   NMOS/PMOS, G ground, S supply, Space rotate, X mirror, Shift+I place a cell instance, Shift+X / Shift+B descend /
   return. Tools > Check Schematic, or File > Check and Save.
2. Simulation (Ctrl+3): tick analyses in Setup (Operating Point, DC Sweep, AC Small-Signal, Transient, Noise), press
   F5 / Ctrl+Enter (Simulation > Run). A source needs an AC magnitude for AC and a Transient waveform (PULSE/SIN/PWL)
   for transient. Results: one tab per analysis; Signals list on the right, Measurements below (DC/tran: MIN, MAX,
   PEAK-PEAK, FINAL; AC: GAIN @ fstart, PEAK, f -3 dB, UNITY GAIN f). The operating point is annotated on the
   schematic (Ctrl+2). Simulation > Waveform Calculator…, Corner / Sweep Manager…, Monte Carlo…, Optimizer…,
   Verification…, Results Browser….
3. Layout (Ctrl+4): R rectangle, Shift+P polygon, P path (+/- drops a via and changes metal), O via, I instance,
   L label, K ruler, S stretch, M move, C copy, q properties, F fit, Shift+O rotate, u / Shift+U undo / redo, T layer
   tap, G gravity. Verify > Generate Layout from Schematic places the devices with flylines, Verify > Route Flylines
   routes them. Verify > Run DRC, Verify > Run LVS (needs a schematic and a layout; equal pin labels anchor it),
   Verify > Extract Nets. Post-layout: tick Include layout parasitics in the Simulation Setup and simulate again.
4. Start from a working design: Simulation > Derive New Design… (the wizard: copy, targets, sizing, layout) or
   File > Copy Cell as New…; the derive_design tool does the wizard in one call.
5. Learn: Help > Learn IC Design… (the course; pages learn/*), Help > Interactive Tutorial (ten steps on the demo's
   cmos_inverter), Help > User Guide, F1 shortcuts.

The demo project (File > New Demo Project): cmos_inverter (schematic + hand-drawn layout), rc_lowpass (AC corner),
ota5t (five-transistor OTA: gain, unity-gain frequency), ring_osc5 (period, post-layout), full_adder (generated and
routed layout), nand2 (clean DRC/LVS), ldo, current_mirror, nmos_curves, inv_chain / inv_array / inv_tiles
(hierarchy), devices (PCells). docs_read page demos lists them all.

Tools and the window: your design tools act on the same project the user sees; what you change appears live and is
undoable. Where each tool is in the window: run_drc = Run DRC, run_lvs = Run LVS, extract = Verify > Extract Nets,
check_against_source / update_from_source = Verify > Check Against Source / Update Components and Nets, simulate =
F5, generate_layout = Verify > Generate Layout from Schematic, route = Verify > Route Flylines, optimize = Simulation
> Optimizer…, derive_design = Simulation > Derive New Design…, post_layout_parasitics = Include layout parasitics,
emir = IR drop and electromigration (manual page verification).

FORMULAS (what the tools compute with; docs_read page formulas, or ko/formulas, has every model with its defaults)
Built-in simulator (the default backend; Auto switches to ngspice for MOS levels other than 1 / 14 / 54):
- MOS level 1 (generic180 cards): Leff = L - 2·ld, β = kp·W/Leff·m, Vth = vto + γ(√(φ - Vbs) - √φ), Vov = Vgs - Vth.
  Cutoff Vov <= 0: Id = 0 (no subthreshold current). Triode Vds < Vov: Id = β(Vov·Vds - Vds²/2)(1 + λVds).
  Saturation: Id = ½βVov²(1 + λVds), gm = βVov(1 + λVds), gds = λ·½βVov², so ro ≈ 1/(λ·Id).
  generic180: nmos vto 0.45, kp 280u, pmos vto -0.45, kp 70u; γ 0.4, φ 0.8, tox 4n, cgso = cgdo 0.3n, cj 1m, cjsw 0.3n;
  λ is written per drawn L: λ = λcard·0.5µm/L (λcard 0.08 n, 0.1 p), so longer L means more ro and gain.
  Junction caps: a MOS without ad/as/pd/ps gets them estimated from its PCell in every simulation deck (one 1 µm
  finger: AD = AS = 0.48 pm², PD = PS = 1.96 µm, about 1 fF per drain); post-layout netlists replace them.
  capmod 0 = Meyer (Cgs = ⅔Cox in saturation, not charge conserving), capmod 1 = Ward-Dutton charges.
- BSIM4 (level 14 / 54, PDKs): the BSIM4.8 equations; no rdsmod/rbodymod/rgatemod/NQS/stress (sa, sb).
- Diode: I = is(e^(V/nVt) - 1); depletion and tt·I charge. BJT: Gummel-Poon subset. JFET: β(Vgs - vto)²(1 + λVds).
- DC: Newton with |Δx| <= vntol/abstol + reltol·|x| (reltol 1e-4); MOS steps clipped to 0.5 V; gmin then source
  stepping when the OP fails. Transient: trapezoidal (backward Euler at starts and breakpoints), LTE step control
  (lte_reltol 1e-3, trtol 7), every accepted point is output. AC: linearised at the OP.
- Noise (adjoint): R 4kT/R, MOS channel (8/3)kT·gm, level-1 flicker nlev 2 (default) kf·gm²/(f^af·W·Leff·Cox'),
  diode/BJT shot 2q|I|. onoise_spectrum V/√Hz, inoise_spectrum = onoise/|H| (A/√Hz for a current-source input); total noise
  goes to the log.
- Expressions (.param, {…}, B sources): log = ln; agauss/gauss/aunif/unif return the nominal (variation comes from
  Monte Carlo).
Measurements (calculator and specs; AC data as magnitude): db = 20log10|x|; bandwidth = first fall to (gain at the
LOWEST frequency) - 3.0103 dB; ugf = first 0 dB crossing; phasemargin = 180 + phase at ugf, with the phase referenced to its
low-frequency value (an inverting input gives the same margin); gainmargin = -dB at the -180° crossing; delay = t(cross 2) - t(cross 1);
risetime/falltime 10-90 % of the whole waveform's min..max; overshoot = 100(peak - final)/(final - initial);
settling = absolute time it last leaves ±2 % of the step; avg/rms by trapezoid. A spec passes when min <= v <= max.
Optimizer: score S = Σ normalised violations (|limit| scale; failed measurement +1e3); margin M = worst normalised
distance to a limit. Failing point f = S; passing points f in [-1, 0]: Margin goal -clamp(M,0,1), Min area
-1/(1 + A/A0) with A = ΣW·L·m·nf, Least change (derive) -(0.5 + 0.5·clamp(M/0.05,0,1))/(1 + 5Δ). Compass search in
unit coordinates (log variables log-mapped), step 0.25 halved to 1/1024, budget 60 (derive 80); local, not global.
Derive: knobs W in [max(W/4, Wmin), 4W], L in [L, 4L] only when L >= 1.2·Lmin, R/C in [x/4, 4x], log, MOS step 10n;
matched devices share one variable; power guard idd in [0.5, 2]×; warns when a device leaves saturation (margin
|Vds| - (|Vgs| - |Vth|) < 0.1 V), power > 1.5×, or a matched pair shrinks (offset grows √(W0L0/W1L1) > 1.2).
Statistics: built-in Monte Carlo per MOS dV = dVglobal + N·avt/√(W·Leff·m), dβ likewise (defaults vto_sigma 20 mV,
kp_sigma 3 %, avt 5 mV·µm, abeta 1 %·µm, 100 runs); avt is per device, a pair is √2 larger (σΔVt = AVt/√(WL) per
device in the course, 2×1 µm -> 3.5 mV). A fitted pair coefficient (Silicon Correlation) is divided by √2. Yield
prediction: quadratic (or linear) surrogate, Q² = 1 - PRESS/SST (warns < 0.9), 100 000 samples, Cpk = min(μ - LSL,
USL - μ)/3σ, Wilson 95 % interval. Corners from silicon: primaries at μ + zσ, the rest at their conditional mean.
Layout and extraction (generic180; 1 dbu = 1 nm):
- Rules (width/space nm): active 220/280, poly 180/240, metal1 230/230, metal2-3 280/280, nwell 860/1400; contact
  220 cut, 250 space; via 260/260. MOS PCell: plate 420, source/drain end 480, inner 540, pitch = L + 540,
  W min 420, fingers = ceil(W/5µm)·m. Junction (one finger) AS = AD = 480·w, PS = PD = 960 + w (nm).
- Parasitics: C_gnd = Σ(area·A + fringe·P) aF (no shielding); coupling = k·L/s aF for facing edges within 5×spacing;
  plate between layers 1/(1/c_hi - 1/c_lo), except where the overlap is an extracted capacitor device; wire R = Rs·L/W in tiles of 10 squares, a resistor per via cut. generic
  (area aF/µm², fringe aF/µm, Rs Ω/□, EM mA/µm): poly 100/45/7.5/0.05, m1 38/40/0.08/1.0, m2 16/32/0.08/1.0,
  m3 10/28/0.06/1.5; contact 10 Ω, via 5 Ω per cut. The net "RC" figure is Rmax·Ctotal, not Elmore.
- IR/EM: DC solve G·drop = I with pads at 0, device currents into the nearest metal1 (contacts); wire J = |I|/width (A/µm) against the layer's em, via |I| per cut.
- Router: A*, metal1 cost 4×, metal2 prefers horizontal, metal3 vertical (3× the wrong way), via 2 pitches.
Process and pixels: implant Gaussian C = Q/(√(2π)ΔRp)·exp(-(d - Rp)²/2ΔRp²) (LSS tables), diffusion D = D0·e^(-Ea/kT),
Deal-Grove oxidation x = (-A + √(A² + 4B(t + τ)))/2, Caughey-Thomas mobility, Rs = 1/Σqµ|N|h. Pixel bench: C = 500q/ΔV,
CG = q/C_FD, full well = C_PD(Vreset - Vsat)/q, kTC = √(kT·C_FD)/q, dark current scales exp((Ea/k)(1/Tref - 1/T)).
Course hand formulas: τ = RC, f-3dB = 1/(2πRC); gm = 2Id/Vov = √(2k'(W/L)Id); CS gain -gm(RD‖ro); fu ≈ gm/(2πCL); 5T OTA
gain gm1(ro2‖ro4); mirror Iout = Iref·(W/L)2/(W/L)1; inverter VM = (Vtn + r(VDD - |Vtp|))/(1 + r), r = √(k'pWp/k'nWn);
tp ≈ 0.69·Ron·CL; P = α·C·VDD²·f; ring period 2N·tp.
When you explain a number, show the formula with the user's own values (from get_netlist, simulate, the OP table), and
say when it rests on one of the simplifications above.
