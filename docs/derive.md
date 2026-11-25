<!-- seo title="Start from an earlier design" description="Copy a finished cell, set new targets and let IC Studio size the copy, lay it out and check it: the Derive New Design wizard, design notes, spec advice and beginner mode." -->

# Start from an earlier design

Most new circuits begin as a copy of one that already works: an amplifier with more gain,
a current mirror with a different ratio, a faster inverter. IC Studio is built for this,
and you do not need to know every rule of IC design to do it.

## Read the design first

Open the cell's schematic and click an empty spot so that nothing is selected. The panel on
the right shows:

- **Specs (targets)**: what the design must achieve, such as `gain_db >= 30` or
  `tphl <= 50p`. **Check specs** runs them over every corner (the Verification window), and
  **Edit specs...** opens the Corner / Sweep Manager.
- **Design notes**: what the cell is for, why it is sized the way it is and what to change
  for another target. Every demo cell comes with notes. They are stored with the cell
  (`cell.json`), so write your own for the cells you make.

Hover a device parameter in the inspector (W, L, M, R, C, ...) to see what changing it does.
After a simulation, the operating point table counts the transistors in saturation, triode
and cutoff, and hovering a device's region explains what it means for that device.

## Derive a new design

Click **Derive a new design from this...** in the panel (or **Simulation > Derive New
Design...**). The wizard has four parts:

1. **Name**: the new cell, `<cell>_v2` by default. The original is never changed.
2. **Targets**: the original's specs with their limits. Change the limits to the new
   targets, untick the ones you do not need, or add common ones (gain, unity-gain frequency,
   phase margin, bandwidth, delays) with the **+** buttons.
3. **What IC Studio may change**: parameters suggested from the circuit. Each group of
   matched transistors (an input pair, a current mirror) gets one W that all of them share,
   so they stay equal; devices drawn longer than the minimum also get an L, which may grow
   but not shrink below the original (they were made long on purpose). When a spec
   measures a current, the output side of a current mirror gets a W of its own, because the
   mirror ratio sets the current (a supply current spec does not count). Resistors and
   capacitors between signal nets are knobs too (an output set by a divider, a compensation
   capacitor); capacitors to ground are loads and stay as they are. Each line says, in plain
   words, what the parameter does.
4. **Options**: build the layout like the original's, keep the supply current near the
   original's (0.5x to 2x, on by default), and the simulation budget. Without the current
   guard the optimizer may meet a target by starving the circuit (slow) or flooding it
   (power); an `idd` target of your own replaces the guard.

**Create and size** copies the cell with its specs and notes, runs the
[optimizer](tools.md) on the suggested parameters with the **Least change** goal (the passing
point closest to the original sizes, so only what the new targets need changes), writes the best sizes into the copy and,
when asked, lays it out: placed like the original
([layout by example](schematic-driven-layout.md)), or generated from the schematic when the
new sizes no longer fit the original placement, then routed, DRC errors fixed automatically
and LVS run. The generator places transistors only: resistors and capacitors are listed to be
drawn by hand, and LVS then compares the transistors. The result lists every spec on the original and the new sizes, the chosen
values and the layout checks. The copy's notes record where it came from and its new sizes.

Specs only measure what they name, so the result also lists **What else changed**: comparing
the operating points of the original and the new sizes, it flags transistors that left
saturation or sit close to its edge, a large change in supply power or bias current, matched
pairs with less gate area (more random offset) and channel lengths made shorter. The generic
180 nm models scale the channel-length modulation with L (see
[The MOSFET](learn/mosfet.md#w-l-and-the-parameters-in-ic-studio)), so a longer L gives more
gain as in silicon; for sign-off numbers, use the foundry PDK.

**File > Copy Cell as New...** (or **Copy as New Cell...** on a cell in the Explorer) only
copies a cell, with every view, specs and notes, when you want to change it by hand.

## When a spec fails

The Verification window lists failing specs under **What to try**, with advice in plain
words: which way to move which parameter, and why (more gain needs longer L or less current
per stage, more phase margin needs compensation, a slow edge needs a wider driver, ...).
The same advice is in `ics verify` (`try:` lines) and in the MCP `verify` result.

## Ask the copilot

The Design Copilot (**Tools > Design Copilot...**) offers starter requests for the open
cell: explain how it works, derive a new design from it, find out why it misses its specs,
check its operating points. It reads the cell's notes and specs, and its `derive_design`
tool does what the wizard does in one call.

## Beginner mode and language

Beginner mode, on for a first start (an installation that saved its settings before keeps the
full menus), keeps the menus to the everyday flow and hides specialist
tools (process emulation, pixel optics, sign-off runsets, SKILL, rule decks, extensions).
Every command stays in the command palette (**Ctrl+K** / **⌘K**). Turn it off with
**Help > Beginner Mode**.

The welcome page switches the interface between English and Korean when a system font with
Hangul is installed (Malgun Gothic on Windows, Apple SD Gothic Neo on macOS, Noto Sans CJK on
Linux). **Settings** (Ctrl+, / ⌘,) and the palette have the same switch; the choice is kept
for the next start.

## From the command line and agents

```sh
ics copy-cell my_project ota5t my_ota                  # copy every view, specs and notes
ics derive my_project ota5t --target gain_db>=50 --target pm>=60
ics derive my_project current_mirror --name cm2x --target "iout=38u..42u"
ics derive my_project ota5t --target bw>=10meg --dry-run   # show what it would change
```

`ics derive` prints the suggested parameters and targets, then each spec before and after,
the new sizes and the layout checks, and saves. It exits with 1 when a target is missed.
Targets name a spec of the cell or a common one (`gain_db`, `ugf`, `pm`, `bw`, `tphl`,
`tplh`, `idd`); `--no-layout` skips the layout and `--free-power` drops the supply current
guard. The health findings are printed as `Check:` and `Note:` lines.

The MCP tools are `copy_cell` and `derive_design` (see [MCP server](mcp.md)); `get_netlist`
also returns a cell's notes and specs.
