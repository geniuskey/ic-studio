<!-- seo title="Tutorial: Lay Out, Verify and Extract a CMOS Inverter" description="Step-by-step tutorial: generate the layout of a CMOS inverter from its schematic, route it, run DRC and LVS, simulate with extracted parasitics and export GDSII in IC Studio." -->

# Hands-on: lay out the inverter

This tutorial takes the inverter from [the first hands-on](tutorial-inverter.md) to a
verified layout: generate it from the schematic, route it, check it with DRC and LVS,
simulate it with the parasitics of the real wires, and write the GDSII file a fab would
receive. Read [From circuit to silicon](layout-basics.md) first. Allow about 20 minutes.

Start from your `my_inv` cell with the PMOS at **2 µm** (set it back if you tried 4 µm).
No project at hand? Open the demo and use `cmos_inverter` after deleting its layout
(open it with `Ctrl+4`, then `Ctrl+A`, `Delete`).

## Step 1: generate the layout

1. With `my_inv` open, choose **Verify > Generate Layout from Schematic**.
2. The cell gets a layout view and the Layout workspace opens. You see:
   - the PMOS as a PCell in the top row, the NMOS in the bottom row, gates facing each
     other;
   - an **n-well** around the PMOS, and bulk **taps** at the row ends (an `ntap` on
     `vdd`, a `ptap` on ground);
   - dashed **flylines**: the connections still to make.
3. The console lists what happened: the two transistors placed, and the capacitor and the
   sources skipped (`no layout generator`). They are testbench parts, not part of the
   circuit.

Press `F` to fit the view. Zoom with `Ctrl`+wheel and pan with the wheel or the middle
mouse button.

<div class="callout">

**Look before you route.** Click the PMOS and look at the inspector: its W and L come from
the schematic. Move it with the mouse and watch its flylines follow: the instance name
(`MP`, or `M1` in your schematic) links it to the schematic device. `Ctrl+Z` puts it
back.

</div>

## Step 2: route

1. Choose **Verify > Route Flylines**.
2. The flylines become metal wires and vias. The console reports the nets routed (4 nets,
   12 vias for this inverter) and `0 flyline(s) left`.

The router works on a grid aligned to the device terminals and labels the named nets
(`in`, `out`, `vdd`), so what it draws is DRC clean by construction and ready for LVS.

## Step 3: DRC

1. In the right panel, press **Run DRC** (or **Verify > Run DRC**).
2. Result: **No violations**.

Now break a rule on purpose, to see what a violation looks like:

1. Select a metal1 wire near another metal1 shape and nudge it towards it with
   `Shift`+arrow keys.
2. **Live DRC** (on by default) draws a dashed box as soon as the gap is below the
   230 nm metal1 spacing. Hover it to read the rule.
3. Press `Ctrl+.` (**Quick Fix**) on the violation, or simply `Ctrl+Z`.

## Step 4: LVS

1. Press **Run LVS**.
2. The result lists one difference:

   ```text
   [missing] schematic device CL (capacitor C=20fF) has no layout counterpart
   ```

This is expected. The 20 fF load models *the next gate*, which is not part of this cell.
LVS compares everything in the schematic, so it reports it. The nets `in`, `out`, `vdd`
and ground all match, and so do both transistors.

In a real project, the testbench (sources, load) lives in a separate cell that *places*
the inverter as an instance, and the inverter cell itself contains only the circuit.
Then LVS is fully clean. Try it: delete `CL` from the schematic and run LVS again:
**Layout matches schematic**. (Undo with `Ctrl+Z` before continuing; the simulation
needs the load.)

## Step 5: post-layout simulation

1. Press `Ctrl+3` for the Simulation workspace.
2. Tick **Include layout parasitics** and press `F5`.
3. The console summarises the extraction, for example:

   ```text
   post-layout: 3 nets annotated, 2.434 fF to ground, 5 coupling caps (0.547 fF),
   junctions on 2 devices, 40 resistors on 4 nets
   ```

4. Measure the delays again with the Waveform Calculator (as in the
   [first tutorial](tutorial-inverter.md#step-8-the-delay-transient)):

| | t<sub>pHL</sub> | t<sub>pLH</sub> |
|---|---|---|
| Schematic | 36.4 ps | 51.8 ps |
| Post-layout | 39.8 ps | 56.1 ps |

The layout adds about 3 fF of wire and junction capacitance to the 20 fF load and some
wire resistance: about 9 % slower. For a single inverter that is small; for long wires
across a chip, parasitics can dominate the delay entirely.

Under **Compare runs**, tick the schematic run to overlay the two waveforms.

## Step 6: export GDSII

GDSII is the file format fabs accept.

- In the layout toolbar, press **GDS** and choose a file name, or
- from a terminal: `ics gds <project> my_inv -o my_inv.gds`.

The file holds the polygons of every layer, with the GDS layer numbers of the technology
(`poly` is 5/0, `metal1` 7/0, and so on). Any layout viewer, KLayout for example, can open
it.

## What you have done

You have been through the full custom-design loop:

<div class="ics-flow">
<span>Schematic</span><i>→</i>
<span>Simulate</span><i>→</i>
<span>Generate layout</span><i>→</i>
<span>Route</span><i>→</i>
<span>DRC · LVS</span><i>→</i>
<span>Extract parasitics</span><i>→</i>
<span class="loop">Simulate again ↺</span>
</div>

## Exercises

1. **Draw it by hand.** Create a cell `inv_hand` with a layout view only and draw an
   inverter with the Rectangle tool (`R`): active, poly, contacts (Via tool `O`), metal1,
   n-well and implants. Use the demo `cmos_inverter` layout as a model. Run DRC until it
   is clean. Hand layout teaches you why the rules exist.
2. **NAND2.** Draw a NAND2 schematic (two NMOS in series, two PMOS in parallel, see
   [CMOS logic](cmos-logic.md#nand-and-nor)), then generate and route it. Compare with the
   demo's `nand2`. Did the generator share diffusion between the series NMOS?
3. **A bigger circuit.** Open the demo `full_adder`, delete its layout (`Ctrl+A`,
   `Delete`) and run the two commands again: 28 transistors placed and routed, DRC and
   LVS clean.
4. **From the command line.** The same flow, scripted:

   ```sh
   ics route myproj my_inv --place     # generate, route, save
   ics drc myproj my_inv
   ics lvs myproj my_inv
   ics sim myproj my_inv --parasitics
   ics gds myproj my_inv -o my_inv.gds
   ```

Next: the other half of chip design, [analog building blocks](analog-basics.md).
