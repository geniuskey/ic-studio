<!-- seo title="IC Design Glossary: Terms Explained" description="A glossary of integrated circuit design terms for beginners: from active and bias to tape-out, threshold voltage and well taps, with books for further reading." -->

# Glossary

Terms used in this course and in the rest of the guide, in alphabetical order.

| Term | Meaning |
|---|---|
| **AC analysis** | Small-signal simulation over frequency: gain and phase of every node for a 1 V sine at the input. Gives Bode plots. |
| **Active** (diffusion, OD) | The layout layer where silicon is left exposed for transistors and taps. A transistor is poly crossing active. |
| **Bias** | The DC voltages and currents that set where transistors operate. |
| **Bode plot** | Gain (dB) and phase (°) against frequency on a log axis. |
| **Body** (bulk, B) | The fourth MOSFET terminal: the silicon under the channel. Substrate (ground) for NMOS, n-well (V<sub>DD</sub>) for PMOS. |
| **BSIM4** | The industry-standard compact MOSFET model used by most foundry PDKs. |
| **Channel** | The thin inverted layer under the gate that conducts when the transistor is on. |
| **Channel-length modulation** (λ) | The small rise of drain current with V<sub>DS</sub> in saturation; sets r<sub>o</sub>. |
| **CMOS** | Complementary MOS: logic built from NMOS pull-down and PMOS pull-up networks. |
| **Common centroid** | Interleaving matched devices (`ABBA`) so they share the same centre, cancelling process gradients. |
| **Contact** | A metal-filled hole connecting metal1 to active or poly. |
| **Corner** | A model set for a process extreme: `tt` (typical), `ff` (fast), `ss` (slow), `fs`, `sf`. |
| **dB** (decibel) | 20·log₁₀ of a voltage ratio. ×10 = 20 dB, ×0.707 = −3 dB. |
| **dbu** | Database unit of layout coordinates; 1 dbu = 1 nm in the generic technology. |
| **DC sweep** | A series of operating points while one source is stepped. |
| **Design rules** | The fab's geometric limits: width, spacing, enclosure, extension, area, density. |
| **Die** | One chip, cut from the wafer. |
| **Differential pair** | Two matched transistors with a shared tail current; amplifies the difference of two inputs. |
| **Doping** | Adding impurities to silicon: n-type (extra electrons) or p-type (holes). |
| **DRC** | Design Rule Check: verifies a layout against the design rules. |
| **Drain, source** | The two ends of the channel. Carriers flow from source to drain. |
| **Dummy** | An unused device placed at the end of a matched row so every active device has the same surroundings. |
| **Fab** (foundry) | The factory that manufactures the chips. |
| **Finger** | One of several parallel narrow gates that together form a wide transistor. |
| **Flyline** | A straight dashed line in a layout showing a connection still to be routed. |
| **g<sub>m</sub>** (transconductance) | Change of drain current per volt of gate voltage: 2·I<sub>D</sub>/V<sub>ov</sub>. |
| **g<sub>ds</sub>**, **r<sub>o</sub>** | Output conductance and resistance of a transistor in saturation; g<sub>ds</sub> ≈ λ·I<sub>D</sub>. |
| **GDSII** | The binary file format of layouts sent to the fab (OASIS is its newer, smaller successor). |
| **Gate** | The control terminal of a MOSFET, insulated from the channel by the gate oxide. Also: a logic gate (NAND, NOR …). |
| **Gate oxide** | The thin insulator (a few nm of SiO₂) between gate and channel. |
| **Ground** | The 0 V reference net, `0` in SPICE. |
| **Guard ring** | A ring of taps around a device or block that collects stray carriers. |
| **Inverter** | The NOT gate: one PMOS and one NMOS. |
| **KCL / KVL** | Kirchhoff's current and voltage laws, the equations a simulator solves. |
| **Latch-up** | A parasitic thyristor in CMOS that can short the supply; prevented by taps and guard rings. |
| **Layout** | The physical drawing of a circuit: polygons on mask layers. |
| **LVS** | Layout Versus Schematic: verifies the layout is the same circuit as the schematic. |
| **Mask** | A stencil for one lithography step; one layout layer. |
| **Matching** | Making devices that must be equal as identical as possible, by size and layout. |
| **Metal1, metal2 …** | The wiring layers, stacked above the transistors, joined by vias. |
| **Monte Carlo** | Many simulations with random process variation, to see the spread of a result. |
| **MOSFET** | Metal-oxide-semiconductor field-effect transistor. |
| **Net** (node) | A set of connected wires and pins, all at one voltage. |
| **Netlist** | The text description of a circuit: one line per device, listing its nets and values. |
| **NMOS / PMOS** | MOSFET with an n-type (electron) or p-type (hole) channel. |
| **Noise margin** | How much noise a logic input tolerates before it is read wrongly. |
| **N-well** | An n-type region in the p-substrate that holds PMOS transistors. |
| **Operating point (OP)** | The DC solution of a circuit with all inputs held still. |
| **OTA** | Operational transconductance amplifier: differential input, high-impedance output. |
| **Overdrive** (V<sub>ov</sub>) | V<sub>GS</sub> − V<sub>th</sub>. |
| **Parasitics** | Unwanted resistance and capacitance of real wires and junctions. |
| **PCell** | Parameterised cell: a layout generator that draws a device from parameters such as W and L. |
| **PDK** | Process Design Kit: layers, rules, device models and PCells of a fab process. |
| **PEX** | Parasitic extraction: computing parasitics from a layout. |
| **Pelgrom's law** | Random mismatch falls as 1/√(W·L). |
| **Poly** (polysilicon) | The gate layer. |
| **Post-layout simulation** | Simulation with extracted parasitics included. |
| **Propagation delay** (t<sub>p</sub>) | Time from the input crossing 50 % to the output crossing 50 %. |
| **Saturation** | The MOSFET region where V<sub>DS</sub> ≥ V<sub>ov</sub> and the current depends mainly on V<sub>GS</sub>. |
| **Schematic** | The circuit drawn as symbols and wires. |
| **Square law** | I<sub>D</sub> = ½·k'·(W/L)·V<sub>ov</sub>² in saturation. |
| **SPICE** | The standard circuit simulator family and its netlist format. |
| **Substrate** | The wafer silicon under all devices; p-type in the generic process. |
| **Supply** (V<sub>DD</sub>) | The positive power net, 1.8 V in the generic technology. |
| **Tap** | A contact to the substrate or well that ties the transistor bodies to their supply. |
| **Tape-out** | Sending the final layout to the fab. |
| **Testbench** | The sources, loads and analyses around a circuit that exercise it in simulation. |
| **Threshold voltage** (V<sub>th</sub>) | The gate voltage at which the channel forms; 0.45 V for the generic NMOS. |
| **Transient analysis** | Simulation over time. |
| **Triode** (linear) | The MOSFET region where V<sub>DS</sub> < V<sub>ov</sub>; the channel acts as a resistor. |
| **Unity-gain frequency** | The frequency where the gain falls to 1 (0 dB). |
| **Via** | A metal-filled hole connecting two metal layers. |
| **VTC** | Voltage transfer curve: a gate's output against its input. |
| **W, L** | Channel width and length, the sizes a designer chooses. |
| **Wafer** | The silicon disc, up to 300 mm across, on which chips are made. |

<!-- quiz
? A voltage gain of 1000 is how many dB?
= 60 ±0.5 dB
> 20 · log₁₀(1000) = 60 dB. Each factor of 10 adds 20 dB.
-->

<!-- quiz
? What does a tap do?
- Connects metal1 to metal2
+ Ties the substrate or a well to its supply, keeping the transistor bodies biased
- Measures a voltage in simulation
- Splits a wide transistor into fingers
> A tap is a contact to the substrate (ground) or the n-well (V<sub>DD</sub>). Taps and guard rings also prevent latch-up. Metal layers are joined by vias.
-->

<!-- quiz
? By Pelgrom's law, a transistor with four times the gate area has a threshold mismatch …
- four times smaller
+ half as large
- the same
- twice as large
> Mismatch falls as 1/√(W·L): √4 = 2, so σ halves.
-->

<!-- quiz
? What is a PCell?
- A memory cell of the PDK
+ A layout generator that draws a device from parameters such as W and L
- A cell that has passed LVS
- A power cell for the supply rails
> Parameterised cell: change W or L and the layout redraws itself. Schematic-driven layout places PCells for the schematic's devices.
-->

## Further reading

- Behzad Razavi, *Design of Analog CMOS Integrated Circuits* — the standard analog text,
  very readable.
- Neil Weste and David Harris, *CMOS VLSI Design: A Circuits and Systems Perspective* —
  digital CMOS, layout and design rules.
- Jan Rabaey, Anantha Chandrakasan and Borivoje Nikolić, *Digital Integrated Circuits* —
  inverters, gates, delay and power in depth.
- Tony Chan Carusone, David Johns and Ken Martin, *Analog Integrated Circuit Design*.
- R. Jacob Baker, *CMOS: Circuit Design, Layout, and Simulation* — with many practical
  layout examples.
- Open PDKs to practise on real processes: SKY130 (SkyWater 130 nm) and IHP SG13G2, both
  supported by IC Studio (see [Technology](../technology.md)).
