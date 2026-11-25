<!-- seo title="CMOS Logic Gates: Inverter, NAND, NOR Explained" description="How CMOS logic works: transistors as switches, the CMOS inverter and its transfer curve, NAND and NOR gates, propagation delay, power and transistor sizing." -->

# CMOS logic

Digital chips are built from **CMOS** (complementary MOS) logic: every gate pairs NMOS
transistors that pull the output down with PMOS transistors that pull it up. This chapter
builds the inverter, then NAND and NOR, and explains what makes a gate fast or slow.

## Transistors as switches

For logic, a MOSFET is a switch controlled by its gate:

| | Gate = 0 (0 V) | Gate = 1 (V<sub>DD</sub>) |
|---|---|---|
| **NMOS** | off | on |
| **PMOS** | on | off |

An NMOS passes a good 0 (it connects to ground well) and a PMOS passes a good 1 (it
connects to V<sub>DD</sub> well). CMOS uses each for what it is good at: **NMOS in the
pull-down network to ground, PMOS in the pull-up network to V<sub>DD</sub>.**

## The inverter

![A CMOS inverter drawn as two complementary switches, and a dot moving along its simulated transfer curve](../images/learn-cmos-inverter.svg)

The simplest gate: one PMOS from V<sub>DD</sub> to the output, one NMOS from the output
to ground, both gates tied to the input.

- **in = 0**: the PMOS is on, the NMOS is off. The output is connected to V<sub>DD</sub>:
  **out = 1**.
- **in = 1**: the NMOS is on, the PMOS is off. The output is connected to ground:
  **out = 0**.

In either steady state one of the two transistors is off, so **no current flows from
V<sub>DD</sub> to ground**. A CMOS gate that is not switching burns (almost) no power.
This single property is why CMOS won.

### The voltage transfer curve

Sweep the input slowly from 0 to 1.8 V and plot the output: that is the **voltage
transfer curve** (VTC), shown on the right of the figure (simulated in IC Studio with the
generic technology, NMOS 1 µm, PMOS 2 µm, both 180 nm long).

- The output stays at 1.8 V for low inputs and at 0 V for high inputs: logic levels are
  **restored**, so noise does not accumulate through a chain of gates.
- In the middle the curve is very steep (a gain of about −22): small changes of the input
  around the threshold give large changes of the output.
- The **switching threshold V<sub>M</sub>** is where out = in. Here it is about
  **0.83 V**, a little below the middle (0.9 V), because the NMOS is stronger than the
  PMOS.
- **Noise margins**: inputs below V<sub>IL</sub> ≈ 0.65 V are read as 0 and inputs above
  V<sub>IH</sub> ≈ 0.96 V as 1 (the points where the slope is −1).

Only while the input passes through the middle region are both transistors partly on,
and a short-circuit current flows from V<sub>DD</sub> to ground (about 135 µA at the peak
for this inverter).

<!-- quiz
? The inverter reads inputs above V<sub>IH</sub> ≈ 0.96 V as 1, and its output high is 1.8 V. What is the high noise margin NM<sub>H</sub> = V<sub>OH</sub> − V<sub>IH</sub>, in volts?
= 0.84 ±0.02 V
> NM<sub>H</sub> = 1.8 − 0.96 = 0.84 V. A 1 from a previous gate can pick up 0.84 V of noise before it is misread. The low margin is V<sub>IL</sub> − 0 ≈ 0.65 V.
-->

## NAND and NOR

![A two-input NAND gate stepping through its truth table, with the conducting transistors highlighted](../images/learn-nand2.svg)

Every static CMOS gate follows one recipe:

1. Build the **pull-down network** of NMOS transistors that conducts when the output
   should be 0. **Series** NMOS = AND; **parallel** NMOS = OR.
2. Build the **pull-up network** of PMOS transistors as its **dual**: series becomes
   parallel and parallel becomes series.

| Gate | Output | Pull-down (NMOS) | Pull-up (PMOS) |
|---|---|---|---|
| Inverter | NOT A | one | one |
| NAND2 | NOT (A AND B) | two in **series** | two in **parallel** |
| NOR2 | NOT (A OR B) | two in **parallel** | two in **series** |

The output is always inverted: a single CMOS stage can only make **inverting** functions.
An AND is a NAND followed by an inverter. NAND and NOR are each **universal**: any logic
function can be built from either one alone.

<div class="callout">

**Why designers prefer NAND to NOR:** series transistors are slower (their resistances
add). NOR puts the *PMOS*, already the weaker device, in series, so a NOR needs very wide
PMOS to be fast. NAND puts the NMOS in series, which costs less.

</div>

<!-- quiz
? What is the pull-up network of a two-input NOR?
- Two PMOS in parallel
+ Two PMOS in series
- Two NMOS in parallel
- One PMOS
> The NOR pull-down is two NMOS in parallel (OR); the pull-up is its dual, two PMOS in series. That puts the weaker PMOS in series, why NAND is preferred.
-->

## Delay: why gates take time

A gate's output drives a **load capacitance** C<sub>L</sub>: the gates of the next stages,
the wires and its own drain junctions. Switching the output means charging or
discharging C<sub>L</sub> through the transistor that is on, which acts like a resistor
R<sub>on</sub>. As in the [RC circuit](circuits.md#capacitors-and-the-rc-time-constant),
that takes time:

```text
t_p ≈ 0.69 · R_on · C_L
```

The **propagation delay** t<sub>p</sub> is measured from the input crossing 50 % of the
swing to the output crossing 50 %. There are two of them: **t<sub>pHL</sub>** (output
falling, through the NMOS) and **t<sub>pLH</sub>** (output rising, through the PMOS).

For the inverter above with a 20 fF load, IC Studio simulates **t<sub>pHL</sub> ≈ 33 ps**
and **t<sub>pLH</sub> ≈ 47 ps**: rising is slower because the PMOS is weaker.

To make a gate faster, lower R<sub>on</sub> (wider transistors) or lower C<sub>L</sub>
(fewer or smaller gates to drive, shorter wires). But wider transistors also have larger
gates, which load the stage *before*. Sizing is a balance.

<!-- quiz
? A gate with R<sub>on</sub> = 5 kΩ drives C<sub>L</sub> = 20 fF. Estimate t<sub>p</sub>, in ps.
= 69 ±3 ps
> t<sub>p</sub> ≈ 0.69 · 5 kΩ · 20 fF = 0.69 · 100 ps = 69 ps.
-->

## Power

A CMOS chip burns power in three ways:

| Kind | Cause | Formula |
|---|---|---|
| **Dynamic** (switching) | Charging and discharging load capacitances | P = α · C<sub>L</sub> · V<sub>DD</sub>² · f |
| **Short-circuit** | Both transistors briefly on during an input transition | Small with sharp input edges |
| **Leakage** (static) | Transistors that are "off" still leak a little | Matters in advanced nodes |

α is the fraction of clock cycles the node switches. Because power goes with
V<sub>DD</sub>², lowering the supply voltage is the most effective way to save power, which
is why supplies have dropped from 5 V to below 1 V over the decades.

<!-- quiz
? A node with C<sub>L</sub> = 20 fF at V<sub>DD</sub> = 1.8 V switches in 10 % of the cycles of a 1 GHz clock. What is its dynamic power, in µW?
= 6.48 ±0.2 µW
> P = α · C<sub>L</sub> · V<sub>DD</sub>² · f = 0.1 · 20 fF · 3.24 V² · 1 GHz = 6.48 µW. At 0.9 V it would be a quarter of that.
-->

## Sizing the transistors

The PMOS of the generic technology has k' = 70 µA/V² against 280 µA/V² for the NMOS: it
is four times weaker for the same W/L. Two common choices:

| PMOS width | V<sub>M</sub> | t<sub>pHL</sub> / t<sub>pLH</sub> | Comment |
|---|---|---|---|
| W<sub>p</sub> = 2·W<sub>n</sub> (2 µm) | 0.83 V | 33 / 47 ps | A common compromise: smaller, a bit unbalanced |
| W<sub>p</sub> = 4·W<sub>n</sub> (4 µm) | 0.90 V | 35 / 33 ps | Balanced: symmetric threshold and delays |

(Simulated with a 20 fF load. The balanced version has equal rise and fall delays, but
its bigger PMOS also loads the stage driving it, so a 2:1 ratio is often the better
choice inside a chain of gates.)

From the square law, setting the NMOS and PMOS currents equal at the switching point
gives

```text
V_M = (V_th,n + r · (V_DD − |V_th,p|)) / (1 + r),   r = √(k'_p·W_p/L_p / (k'_n·W_n/L_n))
```

With W<sub>p</sub> = 4·W<sub>n</sub>, r = 1 and V<sub>M</sub> = V<sub>DD</sub>/2 exactly.

In IC Studio's Learn window, sliders here let you change the two widths and watch V<sub>M</sub> move along the transfer curve.

<!-- explore: inverter -->

<!-- quiz
? Which PMOS width puts V<sub>M</sub> at V<sub>DD</sub>/2 with a 1 µm NMOS?
- 1 µm
- 2 µm
+ 4 µm
- 8 µm
> k'<sub>p</sub> is a quarter of k'<sub>n</sub>, so W<sub>p</sub> = 4·W<sub>n</sub> makes r = 1 and V<sub>M</sub> = 0.9 V. The common 2 µm gives 0.83 V.
-->

<div class="callout say">

**Try it:** the demo has a ready-made inverter, NAND and NOR, and a ring oscillator:

```sh
ic-studio --demo --cell cmos_inverter --workspace simulation
```

Press `F5` and look at the **DC Sweep** and **Transient** tabs. Then open `nand2` and
`nor2` (Explorer, or `Ctrl+K`), and `ring_osc5`: five inverters in a loop, which
oscillates because each stage's delay keeps the loop from ever settling. Its period is
2 × 5 × t<sub>p</sub>.

</div>

<!-- try: Simulate cmos_inverter | simulate cmos_inverter -->

<!-- try: Open the nand2 schematic | open nand2 schematic -->

<!-- try: Open the nor2 schematic | open nor2 schematic -->

<!-- try: Simulate ring_osc5 | simulate ring_osc5 -->

<!-- quiz
? Each stage of a five-stage ring oscillator has t<sub>p</sub> = 14 ps. What is the period, in ps?
= 140 ±5 ps
> Period = 2 × 5 × t<sub>p</sub> = 140 ps: the edge goes round the loop twice (once rising, once falling) per period.
-->

Now build one yourself: [Hands-on: design an inverter](tutorial-inverter.md).
