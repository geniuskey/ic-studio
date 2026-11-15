<!-- seo title="Quick Tour: Schematic to Post-Layout Simulation" description="A guided tour of IC Studio: simulate a circuit, browse hierarchical layout, auto place and route, run DRC and LVS, then simulate with extracted parasitics." -->

# Quick tour

This tour walks through the demo project: simulation, hierarchical layout, automatic place
and route, verification and post-layout simulation. It ends by building a small circuit
from scratch.

For a guided start inside the app, choose **Help > Interactive Tutorial** (or **Take the
tutorial** on the welcome page). A guide window walks through nine steps on the demo's
`cmos_inverter`: open the demo project, open the schematic, select a device, simulate,
measure a waveform, open the layout, run DRC, run LVS and find a command in the palette.
Each step completes on its own when you do it; **Show me** does it for you, **Skip** moves
on and **Back** returns.

## Open the demo project

```sh
cargo run --release -- --demo
```

Or choose **Open demo project** on the welcome page (**File > New Demo Project**). The demo
lives in memory until you save it with `Ctrl+S`. All cells are in the `work` library and
use the generic 180 nm technology.

The [demo gallery](demos.md) describes every cell, and has scripts for presenting
IC Studio to others.

The `pcells` library holds the PCell variants the layouts use.

## 1. Schematic and simulation

1. In the Explorer, click `cmos_inverter`. The schematic opens.
2. Press `F5`. The Simulation workspace opens with the DC sweep and transient results.
3. Tick or untick signals in **Signals** to change the plot; read MIN / MAX / FINAL in
   **Measurements**.
4. Press `Ctrl+2` to go back to the schematic: node voltages and drain currents from the
   operating point are annotated on it.

![Schematic with OP annotation](screenshots/07_schematic_op.webp)

Try `rc_lowpass` (AC: the **f -3 dB** column shows the corner frequency) and `ota5t`
(AC: open-loop gain and unity-gain frequency).

## 2. Layout and hierarchy

1. Press `Ctrl+4` with `cmos_inverter` open. This layout was drawn by hand.
2. Open `inv_tiles` (Explorer, or `Ctrl+K` and type `inv_tiles`). Zoom with `Ctrl`+wheel,
   pan with the wheel or the middle mouse button, and press `F` to fit.
3. Press `1`, `2` and `9` to change how many hierarchy levels are drawn.
4. Double-click an instance to open the placed cell.

## 3. Automatic place and route

1. Open `full_adder` and press `Ctrl+4`. This layout was not drawn: it was generated from
   the schematic and routed automatically.
2. Press `Ctrl+A` and `Delete` to clear it.
3. Choose **Verify > Generate Layout from Schematic**. The 28 transistors are placed and
   flylines show the connections to make.
4. Choose **Verify > Route Flylines**. Every net is routed on metal.

![Generated and routed full adder](screenshots/demo_pnr_routed.webp)

## 4. Verification

1. Open `nand2` and switch to the Layout workspace.
2. In the right panel, press **Run DRC**: no violations.
3. Press **Run LVS**: the layout matches the schematic.
4. Open `cmos_inverter` and run LVS: it lists the deliberate differences. Click one to
   zoom to it.

![LVS result](screenshots/12_layout_lvs.webp)

## 5. Post-layout simulation

1. Open `ring_osc5` in the Simulation workspace.
2. Press `F5` and note the oscillation period in the transient plot.
3. Tick **Include layout parasitics** and press `F5` again. The console summarises the
   extracted capacitance and resistance. Compare the period: the added wire
   capacitance and resistance slow the oscillator down.

## 6. Build an inverter from scratch

1. Press `Ctrl+N`, name the cell `my_inv`, keep **schematic** checked, and press
   **Create**.
2. Press `P` and click to place a PMOS, then press `N` and click below it to place an
   NMOS. Press `Esc`.
3. Press `W` and wire the two drains together, then the two gates. `Esc` ends a wire.
4. Press `S` and place a supply symbol on the PMOS source; press `G` and place ground on
   the NMOS source. Wire them.
5. Press `T`, type `in` in the **Net:** field and click on the gate wire; type `out` and
   click on the drain wire.
6. Add the sources: press `V`, place a source, wire its `p` pin to the supply net and its
   `n` pin to a ground symbol; set **DC value** to `1.8` in the inspector. Place a second
   source for `in` with **Transient** `PULSE(0 1.8 1n 100p 100p 2n 4n)`.
7. Press `Ctrl+3`, enable **Transient** (step `10p`, stop `10n`) and press `F5`.
8. Choose **Verify > Generate Layout from Schematic**. The two transistors are placed as
   PCells with well taps, and flylines show the connections to make.
9. Choose **Verify > Route Flylines**. The router wires the nets on metal.
10. Run DRC and LVS from the right panel, then save with `Ctrl+S`.

See [Schematic-driven layout](schematic-driven-layout.md) for what the generator and the
router do, and how to match devices.

## 7. Command line

The same project can be checked from a terminal:

```sh
ics new demo --template demo
ics sim demo ota5t
ics drc demo nand2
ics lvs demo nand2
```

See [Command line](cli.md).
