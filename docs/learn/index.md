<!-- seo title="Learn IC Design: A Beginner's Course" description="Learn how chips are designed, from the transistor up: circuits, MOSFETs, CMOS logic, layout and analog amplifiers, with hands-on IC Studio tutorials." -->

# Learn IC design: start here

This part of the guide is a short course on how integrated circuits (ICs) are designed. It
assumes no background in semiconductors. If you remember that voltage pushes current
through a resistor, you have enough to start. By the end you will have designed a CMOS
inverter and a small amplifier, drawn their layouts, checked them and simulated them with
the parasitics of the real wiring, all in IC Studio.

![Zooming from a wafer to a chip, to a cell, to a single transistor](../images/learn-chip-zoom.svg)

## What is a chip, really?

A chip is a small piece of silicon with **millions to billions of transistors** on it,
wired together by several layers of metal. Each transistor is a tiny **voltage-controlled
switch**. Put switches together and you get logic gates; put gates together and you get
adders, memories and processors. Put a few transistors together carefully and you get
amplifiers, filters and data converters, the analog circuits that connect chips to the
real world.

The picture above zooms in four steps:

| Level | Size | What you design there |
|---|---|---|
| Wafer | 300 mm | Nothing: the factory (fab) prints hundreds of copies of your chip on it |
| Chip (die) | a few mm | The floorplan: where the blocks go and how power and signals reach them |
| Cell | a few µm | A gate, a memory bit or an amplifier: a handful of transistors and their wiring |
| Transistor | ~100 nm | Its width W and length L, the two numbers a circuit designer chooses |

**Full-custom design**, the kind IC Studio is built for, works at the cell and transistor
level: you choose every transistor and draw (or generate) every shape.

<!-- quiz
? At which level does full-custom design, the kind IC Studio is built for, work?
- The wafer: placing copies of the chip
- The die floorplan only
+ The cell and transistor: every transistor and every shape
- Software that runs on the finished chip
> Full-custom design chooses each transistor's W and L and draws (or generates) every shape. The fab handles the wafer; floorplanning is the level above the cells.
-->

## How a chip is designed

![The design loop: schematic, simulate, generate layout, route, DRC and LVS, extract parasitics, simulate again](../images/design-loop.svg)

1. **Specification**: what the circuit must do. "Invert a 1.8 V signal in under 50 ps",
   "amplify by 10 up to 30 MHz".
2. **Schematic**: the circuit as symbols and wires: which transistors, how big, connected
   how.
3. **Simulation**: a SPICE simulator solves the circuit's equations and shows you
   voltages and currents, before anything is built.
4. **Layout**: the circuit drawn as the coloured polygons the factory turns into masks,
   one per layer of the manufacturing process.
5. **Verification**: **DRC** checks the drawing follows the factory's design rules;
   **LVS** checks the drawing is the same circuit as the schematic.
6. **Parasitic extraction**: the real wires add resistance and capacitance. They are
   extracted from the layout and put back into the simulation to see their effect.
7. **Tape-out**: the verified layout is sent to the fab as a GDSII file. Weeks later,
   chips come back.

Steps 2 to 6 repeat until the specification is met. IC Studio keeps all of them on one
design database, so going round the loop is a few key presses.

<!-- quiz
? A layout passes DRC. What does LVS check in addition?
- That the shapes are wide enough and far enough apart
+ That the drawing is the same circuit as the schematic
- That the circuit meets its speed specification
- That the GDSII file can be read by the fab
> DRC checks the geometry against the design rules; LVS extracts the circuit from the layout and compares it with the schematic. Speed is a simulation question.
-->

<!-- quiz
? Why is the circuit simulated again after parasitic extraction?
- Because the first simulation was not accurate enough
+ To see the effect of the real wires' resistance and capacitance
- Because the fab requires two simulations
- To check the design rules
> The schematic has ideal wires. Extraction adds the resistance and capacitance of the drawn wiring, which make the circuit slower.
-->

## The course

<div class="ics-grid">
<div class="ics-card">
<span class="ics-icon">1</span>
<h3><a href="circuits.html">Circuits in ten minutes</a></h3>
<p>Voltage, current, resistors, capacitors, the RC time constant, frequency response and
decibels. Read a SPICE netlist.</p>
</div>
<div class="ics-card">
<span class="ics-icon">2</span>
<h3><a href="mosfet.html">The MOSFET</a></h3>
<p>How a transistor works: silicon, doping, the channel, threshold voltage, the triode
and saturation regions and the square law.</p>
</div>
<div class="ics-card">
<span class="ics-icon">3</span>
<h3><a href="cmos-logic.html">CMOS logic</a></h3>
<p>Transistors as switches: the inverter, NAND and NOR, the transfer curve, delay and
power, and how to size transistors.</p>
</div>
<div class="ics-card">
<span class="ics-icon">4</span>
<h3><a href="tutorial-inverter.html">Hands-on: design an inverter</a></h3>
<p>Draw the schematic, simulate it, measure the switching point and the delay, and size
the PMOS to balance it.</p>
</div>
<div class="ics-card">
<span class="ics-icon">5</span>
<h3><a href="layout-basics.html">From circuit to silicon</a></h3>
<p>How chips are made layer by layer, what layout layers mean, design rules, and where
parasitics come from.</p>
</div>
<div class="ics-card">
<span class="ics-icon">6</span>
<h3><a href="tutorial-layout.html">Hands-on: lay out the inverter</a></h3>
<p>Generate and route the layout, run DRC and LVS, simulate with parasitics and export
GDSII.</p>
</div>
<div class="ics-card">
<span class="ics-icon">7</span>
<h3><a href="analog-basics.html">Analog building blocks</a></h3>
<p>Small-signal thinking: gm and ro, the common-source amplifier, gain and bandwidth,
current mirrors and the differential pair.</p>
</div>
<div class="ics-card">
<span class="ics-icon">8</span>
<h3><a href="tutorial-amplifier.html">Hands-on: an amplifier</a></h3>
<p>Bias and simulate a common-source amplifier, read its gain and bandwidth from a Bode
plot, and explore the OTA demos.</p>
</div>
<div class="ics-card">
<span class="ics-icon">9</span>
<h3><a href="tutorial-derive.html">Hands-on: start from a working design</a></h3>
<p>Make a faster inverter from the demo one: read its specs and notes, set new targets,
let the Derive wizard size and lay it out, and judge what else changed.</p>
</div>
</div>

A [glossary](glossary.md) explains every term used in the course, with pointers for
further reading.

## How to use the course

- **Read in order** if you are new to the field: 15 to 30 minutes a chapter.
- **Read it inside IC Studio** ([Installation](../install.md)): **Start the course** on the welcome page (or **Help ›
  Learn IC Design…**). There the "Try it" buttons do the step for you, each section ends
  with a quiz, the hands-on chapters check your own design as you work, and the MOSFET and
  CMOS chapters have sliders to play with the equations. Press **Dock** to keep the course
  beside the canvas.
- **When you are stuck**, a failed check says what to try, **Show me where** points at the
  button, and **Ask the tutor** opens the copilot to guide you one step at a time (it needs
  an AI model, **Tools › AI Models…**). Hover an underlined word for its meaning.
- **Predict before you press `F5`.** Guessing the result first is the fastest way to build
  intuition.
- Everything uses the **generic 180 nm technology** of new projects: illustrative, not a
  real foundry process, but with realistic numbers (1.8 V supply, 180 nm gates, 0.45 V
  threshold).

<div class="callout">

**Numbers in this course are real.** Every value quoted (switching thresholds, delays,
gains) was simulated with IC Studio's built-in engine on the generic technology, so you
should see the same numbers on your screen, give or take the last digit.

</div>

<!-- try: Take the 5-minute app tour | tutorial -->

<!-- try: Open the demo project | demo -->

<!-- quiz
? What is the supply voltage of the generic technology the course uses, in volts?
= 1.8 ±0.01 V
> The generic 180 nm technology has a 1.8 V supply, a 180 nm minimum gate length and a 0.45 V threshold voltage.
-->

Ready? Start with [Circuits in ten minutes](circuits.md), or jump straight to
[the MOSFET](mosfet.md) if you know your Ohm's law.

Teachers and teams can add chapters of their own to a project: see
[Your own course chapters](../course-authoring.md).
