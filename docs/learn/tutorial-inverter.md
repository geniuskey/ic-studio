<!-- seo title="Tutorial: Design and Simulate a CMOS Inverter" description="Step-by-step tutorial: draw a CMOS inverter schematic in IC Studio, run DC and transient simulations, measure the switching threshold and delay, and size the PMOS." -->

# Hands-on: design an inverter

In this tutorial you design a CMOS inverter from an empty project: draw the schematic, set
up a testbench, simulate it, measure its switching threshold and delay, and resize it.
It puts the [MOSFET](mosfet.md) and [CMOS logic](cmos-logic.md) chapters into practice.
Allow about 30 minutes.

**What you will build:**

| | Value |
|---|---|
| Supply | 1.8 V |
| NMOS | W = 1 µm, L = 180 nm |
| PMOS | W = 2 µm, L = 180 nm, then 4 µm |
| Load | 20 fF (about the input of five similar inverters) |
| Goal | Measure V<sub>M</sub>, t<sub>pHL</sub> and t<sub>pLH</sub>; balance the delays |

## Step 0: a project

Start IC Studio. On the welcome page choose **New project…**, give it a name and a folder,
and keep the generic 180 nm technology. (To follow along without saving, **Open demo
project** works too; everything stays in memory until you press `Ctrl+S`.)

## Step 1: create the cell

<!-- try: New cell | new-cell -->

1. Press `Ctrl+N`.
2. Name the cell `my_inv`, keep **schematic** checked, and press **Create**.

The Schematic workspace opens on an empty canvas. The left panel has the **Device
Library**, with the shortcut key of each device on its tile.

## Step 2: place the transistors

1. Press `P` and click on the canvas to place a PMOS. It is named `M1`. Press `Esc`.
2. Press `N` and click below the PMOS to place an NMOS (`M2`), leaving a few grid points
   between them. Press `Esc`.
3. Click the PMOS. In the inspector on the right, check **Width (W)** is `2u` and
   **Length (L)** (the channel length) is `180n`. Click the NMOS: **Width (W)** `1u`,
   length `180n`. These are the defaults.

Each MOSFET has four pins: `d` (drain), `g` (gate), `s` (source) and `b` (body). Hover a
pin to see its name.

## Step 3: wire the inverter

1. Press `W` for the Wire tool.
2. Click the PMOS drain, then the NMOS drain: the two drains are now one net, the output.
3. Click the PMOS gate, then the NMOS gate: the input net.
4. Press `S` and click above the PMOS to place a **Supply Net** symbol (net `vdd`). Wire
   the PMOS source and the PMOS body to it.
5. Press `G` and place a **Ground** symbol below the NMOS. Wire the NMOS source and body
   to it.

A wire ends by itself when you click on a pin or another wire. Unconnected pins are drawn
as small squares, so a square left on a transistor means a missing wire.

## Step 4: name the nets

1. Press `T` (Label). In the left panel, type `in` in the **Net:** field and click on the
   gate wire.
2. Type `out` and click on the drain wire.

Named nets make the results readable (`v(in)`, `v(out)`) and, later, help LVS match the
layout to the schematic.

## Step 5: build the testbench

A circuit alone does nothing: it needs a power supply, an input signal and a realistic
load. These parts form the **testbench**.

1. **Supply**: press `V` and place a voltage source to the left. Wire its `p` pin to a new
   Supply Net symbol (`S`) and its `n` pin to a Ground symbol. In the inspector set
   **Name** to `VDD` and **DC value** to `1.8`.
2. **Input**: place a second voltage source. Wire `p` to the `in` net (or put an `in`
   label on its `p` pin) and `n` to ground. Set **Name** to `VIN`, **DC value** to `0` and
   **Transient** to:

   ```text
   PULSE(0 1.8 1n 100p 100p 4n 10n)
   ```

   That is: from 0 to 1.8 V, starting at 1 ns, 100 ps rise and fall, 4 ns high, repeating
   every 10 ns.
3. **Load**: press `C`, place a capacitor from `out` to ground and set its
   **Capacitance** to `20f`.

Open the bottom panel (**▸ Console** at the right end of the status bar, or `Ctrl+J`) and
check its **Problems** tab: it should be empty. If it reports a
floating net or an unconnected pin, fix the wire it points to (click the finding to
select the device). The **Netlist** tab now shows something like:

```spice
M1 out in vdd vdd pmos w=2u l=180n
M2 out in 0 0 nmos w=1u l=180n
C1 out 0 20f
VDD vdd 0 DC 1.8
VIN in 0 DC 0 PULSE(0 1.8 1n 100p 100p 4n 10n)
```

(The names come from the order you placed the devices: the PMOS first is `M1`, the NMOS
`M2`. The checks below use these names; if yours differ, read the name in the inspector.)

<!-- check: my_inv has a schematic with the inverter and its testbench (5 devices) | my_inv schematic 5 -->

## Step 6: the operating point

1. Press `Ctrl+3` for the Simulation workspace. **Operating Point** is enabled by default.
2. Press `F5`.
3. Go back to the schematic (`Ctrl+2`): every net shows its voltage. With `in` at 0 V,
   `out` is at 1.8 V.
4. In the Simulation workspace, open the **Operating Point** tab: the NMOS is in
   **cutoff**, the PMOS in **triode** with almost no current. As
   [predicted](cmos-logic.md#the-inverter): no static current.

<!-- check: my_inv has been simulated | my_inv simulated -->

<!-- check: With in at 0 V, out is at 1.8 V | my_inv measure op "v(out)" 1.7 1.9 -->

<!-- check: The NMOS M2 is in cutoff | my_inv region M2 cutoff -->

<!-- quiz
? With in at 0 V, why does almost no current flow from vdd to ground?
- The PMOS is off
+ The NMOS is off, so there is no path to ground
- The load capacitor blocks DC
- The supply is too low
> The PMOS is on (triode) and holds out at 1.8 V, but the NMOS is in cutoff: the series path from vdd to ground is open. A capacitor does block DC, but it is not in that path.
-->

## Step 7: the transfer curve (DC sweep)

1. In **Analyses**, enable **DC Sweep**. Set **Source** to `VIN`, **Start** `0`, **Stop**
   `1.8`, **Step** `10m`.
2. Press `F5` and open the **DC Sweep** tab. Tick `v(in)` and `v(out)` in **Signals**.

You see the transfer curve. To find the switching threshold exactly, open **Simulation >
Waveform Calculator…**, choose the `dc` dataset and evaluate:

```text
cross(v(out) - v(in), 0)
```

It returns the input voltage where out = in: **V<sub>M</sub> ≈ 0.830 V**.

Also plot `i(VDD)`: the supply current is zero at both ends and peaks at about 135 µA near
V<sub>M</sub>, where both transistors are on.

<!-- try: Open the Waveform Calculator | calculator -->

<!-- check: The switching threshold V_M is between 0.78 and 0.95 V | my_inv measure dc "cross(v(out) - v(in), 0)" 0.78 0.95 -->

<!-- quiz
? What does `cross(v(out) - v(in), 0)` return on the `dc` dataset?
- The time at which the output crosses the input
+ The input voltage at which out = in, the switching threshold
- The peak supply current
- The gain of the inverter
> On a DC sweep the x axis is the swept source VIN, so `cross` returns the VIN where v(out) − v(in) crosses 0: V<sub>M</sub>. There is no time axis in a DC sweep.
-->

## Step 8: the delay (transient)

1. Enable **Transient** with **Step** `10p` and **Stop time** `10n`. Press `F5`.
2. On the **Transient** tab, plot `v(in)` and `v(out)`. Zoom into the edge at 1 ns by
   scrolling over the plot.
3. In the Waveform Calculator, pick the `tran` dataset and evaluate the two delays:

   ```text
   delay(v(in), 0.9, v(out), 0.9, rise, fall)     → t_pHL ≈ 33 ps
   delay(v(in), 0.9, v(out), 0.9, fall, rise)     → t_pLH ≈ 47 ps
   ```

   Each measures from the input crossing 0.9 V (50 % of 1.8 V) to the output crossing
   0.9 V. The arguments are: the start signal and its level, the end signal and its level,
   then which edge of each to use. `rise, fall` means "from a *rising* input to the *falling*
   output it causes", which is t<sub>pHL</sub>.

The rising output is slower: the PMOS is weaker than the NMOS, as the
[sizing table](cmos-logic.md#sizing-the-transistors) predicted.

<!-- check: t_pHL (output falling) is below 50 ps | my_inv measure tran "delay(v(in), 0.9, v(out), 0.9, rise, fall)" 20p 50p -->

<!-- check: t_pLH (output rising) is below 70 ps | my_inv measure tran "delay(v(in), 0.9, v(out), 0.9, fall, rise)" 20p 70p -->

<!-- quiz
? Why is t<sub>pLH</sub> (output rising) longer than t<sub>pHL</sub> with a 2 µm PMOS?
+ The output rises through the PMOS, which is weaker than the NMOS
- The input pulse rises more slowly than it falls
- The load capacitor is larger when charged
- The NMOS leaks while the output rises
> The pulse has equal 100 ps edges and the load is the same 20 fF both ways. The difference is the driver: the 2 µm PMOS has half the strength of the 1 µm NMOS (k' is four times lower).
-->

## Step 9: balance the inverter

Predict first: if the PMOS is made twice as wide, what happens to V<sub>M</sub>,
t<sub>pLH</sub> and t<sub>pHL</sub>?

1. Back in the schematic, select the PMOS and set **Width (W)** to `4u`.
2. Press `F5`. Calculator results are kept; click each expression in **Results** and
   evaluate it again.

| PMOS W | V<sub>M</sub> | t<sub>pHL</sub> | t<sub>pLH</sub> |
|---|---|---|---|
| 2 µm | 0.83 V | 35 ps | 49 ps |
| 4 µm | 0.90 V | 38 ps | 35 ps |

The threshold moved to the middle and the delays are now balanced. t<sub>pHL</sub> got
slightly *worse*: the bigger PMOS adds drain capacitance to the output that the NMOS
must also discharge. Every sizing decision trades something.

To compare the two runs on one plot, tick the earlier run under **Compare runs**: it is
drawn dashed.

<!-- check: The delays are balanced within 8 ps (PMOS 4 µm) | my_inv measure tran "abs(delay(v(in), 0.9, v(out), 0.9, fall, rise) - delay(v(in), 0.9, v(out), 0.9, rise, fall))" "" 8p -->

<!-- quiz
? After widening the PMOS to 4 µm, why did t<sub>pHL</sub> get slightly worse?
- The NMOS got weaker
+ The bigger PMOS adds drain capacitance that the NMOS must also discharge
- V<sub>M</sub> moved down
- The supply current fell
> The NMOS is unchanged, but the output node now carries the larger drain junction of the 4 µm PMOS. V<sub>M</sub> moved up, to 0.90 V.
-->

## Step 10: save

Press `Ctrl+S`. The cell is saved as human-readable JSON in your project folder: the
schematic and the simulation setup (`sim.json`), so the next `F5` repeats the same
analyses.

## Exercises

1. **Load.** Double the load to `40f`. How do the delays change? (Delay is roughly
   proportional to C<sub>L</sub>.)
2. **Supply.** Lower VDD to `1.2` (and the pulse to `PULSE(0 1.2 ...)`, with the
   measurement levels at `0.6`). The gate gets slower: R<sub>on</sub> grows as the
   overdrive shrinks. The switching energy C·V<sup>2</sup> falls by more than half.
3. **Long channel.** Set both lengths to `360n`. Why does the delay grow so much?
4. **Variation.** Choose **Simulation > Monte Carlo...**, add the measurements
   `dc: cross(v(out) - v(in), 0)` and `tran: delay(v(in), 0.9, v(out), 0.9, rise, fall)`,
   and run 200 samples. Manufacturing variation spreads V<sub>M</sub> by about ±16 mV
   (1σ) and t<sub>pHL</sub> by about ±1.5 ps. Real chips are designed to work across this
   spread.

<!-- quiz
? Exercise 2: lowering V<sub>DD</sub> from 1.8 V to 1.2 V, what fraction of the switching energy C·V² is left, in percent?
= 44.4 ±2 %
> (1.2 / 1.8)² = 0.444: the energy falls to 44 %, by more than half.
-->

<!-- quiz
? Exercise 3: why does the delay grow so much when both lengths go from 180 nm to 360 nm?
- Only because the transistors are weaker
+ The on-resistance doubles (current ∝ W/L) and the gate and drain capacitances grow too
- The threshold voltage doubles
- The load capacitor doubles by itself
> Halving W/L doubles R<sub>on</sub>, and the longer gates add capacitance to the nodes they load: both factors of t<sub>p</sub> ≈ 0.69 · R<sub>on</sub> · C<sub>L</sub> go up. V<sub>th</sub> stays at 0.45 V.
-->

<div class="callout say">

**From the command line.** The same inverter as a netlist file runs without the GUI.
Save the netlist from Step 5 with `.dc VIN 0 1.8 10m` and `.tran 10p 10n` lines added as
`my_inv.cir`, then:

```sh
ics new myproj
ics schematic myproj my_inv my_inv.cir     # netlist → schematic and simulation setup
ics sim myproj my_inv
```

See [Command line](../cli.md).

</div>

Next: learn what the layout of this inverter means in
[From circuit to silicon](layout-basics.md), then [lay it out](tutorial-layout.md).
