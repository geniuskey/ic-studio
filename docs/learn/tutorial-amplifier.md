<!-- seo title="Tutorial: Design a Common-Source Amplifier" description="Step-by-step tutorial: bias a common-source MOSFET amplifier in IC Studio, check the operating point, read gain and bandwidth from the AC Bode plot and explore the trade-offs." -->

# Hands-on: an amplifier

In this tutorial you design the common-source amplifier from
[Analog building blocks](analog-basics.md#the-common-source-amplifier): bias it, check its
operating point, measure its gain and bandwidth, and see what happens when the bias goes
wrong. Allow about 30 minutes.

**The specification:**

| | Target |
|---|---|
| Supply | 1.8 V |
| Current | about 20 µA |
| Gain | at least 15 dB (×5.6) |
| Bandwidth (−3 dB) | at least 20 MHz into 100 fF |

## Step 1: design on paper

Pick the transistor and resistor with the square law first, then let the simulator check:

1. **Transistor**: W/L = 2 µm / 1 µm. A long channel gives a high r<sub>o</sub> and keeps
   the square law accurate.
2. **Overdrive**: V<sub>ov</sub> = 0.25 V, so V<sub>GS</sub> = 0.45 + 0.25 = **0.7 V**.
3. **Current**: I<sub>D</sub> = ½ · 280 µA/V² · 2 · 0.25² ≈ **17.5 µA**.
4. **g<sub>m</sub>** = 2·I<sub>D</sub>/V<sub>ov</sub> ≈ **140 µS**.
5. **Resistor**: for a gain of 7, R<sub>D</sub> = 7 / 140 µS = **50 kΩ**. It drops
   17.5 µA × 50 kΩ ≈ 0.9 V, leaving the output near 0.9 V: mid-supply, room to swing.
6. **Bandwidth**: 1 / (2π · 50 kΩ · 100 fF) ≈ **32 MHz**.

All targets met on paper. Now verify.

## Step 2: draw the schematic

1. `Ctrl+N`, name the cell `my_cs`, **Create**.
2. Place an NMOS (`N`). Set **Width** `2u` and **Inductance / L** `1u`.
3. Place a resistor (`R`) above it. Set **Resistance** `50k`, and **Name** `RD`.
4. Wire: resistor bottom to the NMOS drain; NMOS source and body to a Ground (`G`);
   resistor top to a Supply Net (`S`).
5. Label the drain net `out` and the gate net `in` (`T`).
6. Place a capacitor (`C`) from `out` to ground: **Capacitance** `100f`.
7. Supply source (`V`): `p` to a Supply Net, `n` to ground. **Name** `VDD`, **DC value**
   `1.8`.
8. Input source: `p` to `in`, `n` to ground. **Name** `VIN`, and:
   - **DC value** `0.7` (the bias),
   - **AC magnitude** `1` (the small-signal input for AC analysis),
   - **Transient** `SIN(0.7 10m 1meg)` (a 10 mV, 1 MHz sine on top of the bias).

The netlist (bottom panel, **Netlist** tab) should read:

```spice
M1 out in 0 0 nmos w=2u l=1u
RD vdd out 50k
CL out 0 100f
VDD vdd 0 DC 1.8
VIN in 0 DC 0.7 AC 1 SIN(0.7 10m 1meg)
```

## Step 3: check the operating point

Press `Ctrl+3`, keep **Operating Point** enabled, and press `F5`. On the **Operating
Point** tab:

```text
v(out)  864.5 mV
m1  saturation  id=18.71µ  vgs=700m  vds=864.5m  vth=450m  gm=149.7µ  gds=1.4µ
```

Check three things, always in this order:

1. **Region**: `saturation`. If it said `triode`, the amplifier would not amplify.
2. **Current**: 18.7 µA, close to the 17.5 µA estimate (the difference is λ, the slope
   of the "flat" curves).
3. **Output voltage**: 0.86 V, well above V<sub>ov</sub> = 0.25 V and below 1.8 V, so it
   can swing both ways.

Now the predicted gain: g<sub>m</sub> · (R<sub>D</sub> ∥ r<sub>o</sub>) = 149.7 µS ×
(50 kΩ ∥ 714 kΩ) = **7.0**.

## Step 4: gain and bandwidth (AC)

1. Enable **AC Small-Signal**: **Start freq** `1k`, **Stop freq** `10g`, **Points/dec**
   `20`.
2. Press `F5` and open the **AC** tab. Plot `v(out)`.

The Bode plot shows a flat gain that rolls off at high frequency, and the phase moving
from 180° (inverted) down by 90° past the corner. The **Measurements** row of `v(out)`:

| Measurement | Value | Meaning |
|---|---|---|
| GAIN @ fstart | 16.9 dB | ×7.0, as calculated |
| f -3 dB | ≈ 35 MHz | The bandwidth |
| UNITY GAIN f | ≈ 250 MHz | Where the gain falls to 1 |

Because the AC source has magnitude 1, `v(out)` *is* the gain. In general, use
`v(out)/v(in)` in the [Waveform Calculator](../tools.md#waveform-calculator):
`bandwidth(v(out)/v(in))` and `ugf(v(out)/v(in))`.

Spec check: gain 16.9 dB ≥ 15 dB, bandwidth 35 MHz ≥ 20 MHz. **Both met.**

## Step 5: see it amplify (transient)

1. Enable **Transient**: **Step** `1n`, **Stop time** `3u`. Press `F5`.
2. On the **Transient** tab, plot `v(in)` and `v(out)`.

The input wiggles by ±10 mV around 0.7 V. The output swings by ±70 mV (from 0.79 to
0.93 V) around 0.86 V, upside down. The **PEAK-PEAK** column reads about 20 mV for the
input and 140 mV for the output: a gain of 7.

Try a 100 mV input (`SIN(0.7 100m 1meg)`). The output is no longer a clean sine: the
square law is not linear for large signals and the bottom of the swing pushes M1 towards
triode. Small-signal analysis is only valid for small signals.

## Step 6: explore the trade-off

Predict first, then simulate. Change **Resistance** of `RD` and fill in the table:

| R<sub>D</sub> | v(out) bias | M1 region | Gain | f -3 dB |
|---|---|---|---|---|
| 30 kΩ | 1.22 V | saturation | 12.9 dB (×4.4) | 56 MHz |
| 50 kΩ | 0.86 V | saturation | 16.9 dB (×7.0) | 35 MHz |
| 70 kΩ | 0.52 V | saturation | 19.4 dB (×9.3) | 25 MHz |
| 100 kΩ | 0.18 V | **triode** | 5.6 dB (×1.9) | — |

- Up to 70 kΩ, gain rises and bandwidth falls in proportion: their product, the
  **gain-bandwidth**, stays near g<sub>m</sub>/(2π·C<sub>L</sub>) ≈ 240 MHz.
- At 100 kΩ the resistor drops too much voltage, M1 is pushed into triode, and the gain
  collapses. **Always check the region after a change.**

The **Corner / Sweep Manager** (**Tools** menu, or **Sweep...** in the setup panel) runs
this table for you: add the variable `RD.r` with the values `30k 50k 70k 100k`, add two
specs on the `ac` dataset, `max(db(v(out)))` for the gain and `bandwidth(v(out))` for the
bandwidth, and press **Run Sweep**.

## Step 7: is it robust?

Real chips vary. Two quick checks:

1. **Temperature**: set **Temperature** to `125` and press `F5`. Two effects compete:
   V<sub>th</sub> falls (to 0.35 V), so the current rises to 24 µA and the output drops
   to 0.6 V; and the carrier mobility falls, so g<sub>m</sub> drops to 136 µS. The gain
   falls to about ×6.2 (15.9 dB), just above the spec.
2. **Monte Carlo**: **Simulation > Monte Carlo...**, add the measurements `op: v(out)`
   and `ac: max(db(v(out)))`, and run 200 samples. The output bias spreads with a σ of
   about **130 mV**, and the gain ranges from 15.3 to 17.9 dB. This sensitivity is the
   weakness of this simple circuit: its bias depends directly on V<sub>th</sub>. Real amplifiers set the bias with a
   [current mirror](analog-basics.md#the-current-mirror) and use differential pairs,
   which reject such shifts.

## Going further: the OTA

Open the demo `ota5t` (five-transistor OTA) and `ota2s` (two-stage OTA):

1. Press `F5` and read the gain: about 42 dB for `ota5t`, 80 dB for `ota2s`.
2. On the schematic (`Ctrl+2`), find the differential pair, the current-mirror load and
   the tail current source. Read each transistor's g<sub>m</sub> and g<sub>ds</sub> on the
   **Operating Point** tab and check the gain formula
   g<sub>m1</sub>/(g<sub>ds2</sub> + g<sub>ds4</sub>).
3. Switch to the layout of `ota2s` (`Ctrl+4`), delete it (`Ctrl+A`, `Delete`) and run
   **Verify > Generate Layout from Schematic**: the pair and the mirror are recognised and
   placed in common-centroid order with dummies, the
   [matching](layout-basics.md#layout-techniques-you-will-see) an analog layout needs.
   Route it, then simulate with parasitics.

## Where to go from here

You now know the whole flow. Next steps:

- The rest of this guide documents every tool: [Simulation](../simulation.md),
  [Layout](../layout.md), [Verification](../verification.md) and the
  [Tools](../tools.md) (calculator, sweeps, Monte Carlo, optimizer).
- Switch to a real open PDK such as SKY130 or IHP SG13G2 in
  [Technology](../technology.md).
- Automate everything from the [command line](../cli.md) or let an
  [AI agent](../mcp.md) drive the app.
- Look up terms in the [glossary](glossary.md), which also lists books for going deeper.
