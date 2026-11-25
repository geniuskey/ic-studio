<!-- seo title="Tutorial: Derive a Faster Inverter from a Working Design" description="Step-by-step tutorial: start a new design from a finished one in IC Studio. Read the specs and design notes, set new delay targets, let the Derive wizard size, lay out and check the copy, and judge what else changed." -->

# Hands-on: start from a working design

Engineers rarely start from an empty schematic. Most new circuits begin as a copy of one
that already works: the same inverter, but faster; the same amplifier, with more gain. In
this tutorial you make a faster version of the demo inverter and let IC Studio do the
sizing, the layout and the checks, while you decide the targets and judge the result.
Allow about 20 minutes.

You need the [inverter chapter](tutorial-inverter.md) (what t<sub>pHL</sub> and
t<sub>pLH</sub> are) and the idea of [sizing](cmos-logic.md#sizing-the-transistors): a
wider transistor carries more current, so it charges the load faster.

**The specification:**

| | Original `cmos_inverter` | New design |
|---|---|---|
| t<sub>pHL</sub> (output falling) | ≤ 50 ps (it does 33 ps) | **≤ 25 ps** |
| t<sub>pLH</sub> (output rising) | ≤ 70 ps (it does 47 ps) | **≤ 35 ps** |
| Load | 20 fF | 20 fF |

## Step 1: read the design first

<!-- try: Open the demo project | demo -->

<!-- try: Open the inverter schematic | open cmos_inverter schematic -->

1. Open the demo project and the `cmos_inverter` schematic.
2. Click an empty spot of the canvas so that nothing is selected. The panel on the right
   now shows the cell's **Specs** (`tphl <= 50p`, `tplh <= 70p`) and its **Design notes**.
3. Read the notes. They say what the cell is for, why it is sized as it is (the PMOS is
   twice as wide because holes are slower than electrons) and what to change for another
   target: *for a faster gate, scale both W by the same factor and keep the 2:1 ratio*.
4. Hover the PMOS **Width** in the inspector: the tooltip says what changing it does.

<!-- quiz
? The notes say "scale both W by the same factor". Why both, and not only the NMOS?
- The NMOS cannot be made wider than the PMOS
+ Each transistor drives one edge: the NMOS the falling one, the PMOS the rising one
- A wider NMOS makes the PMOS faster too
- DRC requires the same factor
> The NMOS discharges the load (t<sub>pHL</sub>) and the PMOS charges it (t<sub>pLH</sub>). Both targets are tighter, so both transistors need more current; keeping the ratio keeps V<sub>M</sub> where it was.
-->

## Step 2: guess first

Before you let the tool work, estimate on paper. The delay is roughly
t<sub>p</sub> ≈ C·V<sub>DD</sub> / (2·I), and the current grows with W. Both targets ask for
about 1.4 times less delay (33 → 25 ps, 47 → 35 ps), so both widths must grow about 1.4
times; a bit more, because a wider transistor also adds its own drain capacitance to the
load.

<!-- quiz
? The PMOS is 2 µm wide. Roughly how wide must it become for 1.4 times less delay (ignoring its own extra capacitance), in µm?
= 2.8 ±0.3 µm
> Delay scales as 1/I and I as W, so 1.4 times less delay needs 1.4 times the width: 2 µm × 1.4 = 2.8 µm. The real answer comes out larger, because the wider drain adds capacitance.
-->

## Step 3: derive the new design

<!-- try: Derive a new design from cmos_inverter | derive cmos_inverter -->

Click **Derive a new design from this...** in the panel (or **Simulation > Derive New
Design...**). The wizard opens with four parts:

1. **Name**: keep `cmos_inverter_v2`. The original is never changed.
2. **Targets**: set `tphl` to max `25p` and `tplh` to max `35p`.
3. **What IC Studio may change**: the W of M1 (PMOS) and the W of M2 (NMOS), each with a
   plain sentence on what it does. Leave both ticked.
4. **Options**: keep **Build the layout like the original** and **Keep the supply current
   near the original** on.

Click **Create and size**. The optimizer simulates a few dozen candidates and keeps the
passing one closest to the original sizes. A few seconds later the result lists each spec
before and after:

```text
tphl   <= 25p    33.2p -> 23.8p
tplh   <= 35p    47.0p -> 33.2p
M1.w = 4.21µ   (PMOS, was 2µ)
M2.w = 1.95µ   (NMOS, was 1µ)
Layout: DRC clean; LVS: the transistors match the schematic
```

Both widths roughly doubled (more than your 1.4 guess: the self-loading), and the ratio
stayed near 2:1, as the notes said.

<!-- check: The new cell cmos_inverter_v2 exists, with a schematic | cmos_inverter_v2 schematic 2 -->

<!-- check: cmos_inverter_v2 has a layout | cmos_inverter_v2 layout -->

## Step 4: check it yourself

Never trust a number you have not seen. Open the new cell and simulate it:

<!-- try: Simulate cmos_inverter_v2 | simulate cmos_inverter_v2 -->

<!-- check: t_pHL of the new design is at most 25 ps | cmos_inverter_v2 measure tran "delay(v(in), 0.9, v(out), 0.9, rise, fall)" "" 25p -->

<!-- check: t_pLH of the new design is at most 35 ps | cmos_inverter_v2 measure tran "delay(v(in), 0.9, v(out), 0.9, fall, rise)" "" 35p -->

<!-- check: The new layout is DRC clean | cmos_inverter_v2 drc -->

If a check fails, its **What to try** line says which width to change, and **Ask the
tutor** opens the copilot with the failing check.

## Step 5: what else changed?

Specs only measure what they name. The wizard's **What else changed** list compares the
operating points of the original and the new design: transistors that left saturation,
a big change in supply current, matched pairs that got smaller. For this inverter, think
about what the specs do not see:

- **Input capacitance**: the new inverter's gates are twice as wide, so it is twice as
  heavy a load for the gate that drives *it*. Speed here costs speed one stage earlier.
- **Power**: the switching energy C·V<sub>DD</sub>² grows with the capacitance, so each
  edge costs more.
- **Area**: the layout is bigger.

<!-- quiz
? The faster inverter's input capacitance doubled. What happens to the gate that drives it?
- Nothing: input capacitance only matters for analog circuits
- It gets faster, because the new inverter switches sooner
+ It gets slower, because it now has a bigger load to charge
- Its V<sub>M</sub> moves to V<sub>DD</sub>/2
> Delay grows with the load capacitance. A chain of gates is only as fast as its slowest stage, which is why real designs scale up a chain step by step (a "buffer chain") rather than making one gate huge.
-->

## Step 6: write your own notes

Select nothing on the new schematic and read its **Design notes**: the wizard recorded
where the cell came from and its new sizes. Add a line of your own, in your words: why it
is faster and what it costs. The next person to derive from your cell (perhaps you, in
three months) will start from what you learned.

## Going further

- Derive the amplifier: open `ota5t`, **Derive a new design**, and ask for `gain_db >= 50`.
  Watch which knobs the wizard suggests (the input pair shares one W, the mirror another)
  and read **What else changed**.
- In the [copilot](../tools.md#design-copilot), choose **Tutor** and ask *"Help me derive a
  faster inverter"*: it walks you through the same steps one at a time and checks each one.
- The full reference is [Start from an earlier design](../derive.md), including
  `ics derive` on the command line.
