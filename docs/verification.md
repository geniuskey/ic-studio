<!-- seo title="DRC, LVS and Parasitic Extraction" description="Verify IC layouts in IC Studio: design rule check (DRC), connectivity extraction, layout versus schematic (LVS), parasitic extraction and IR drop / EM analysis." -->

# Verification: DRC, extraction and LVS

The verification tools are in the **Verify** menu and in the right panel of the Layout
workspace. They run in the background on the flattened layout of the active cell; while
one runs, the panel shows its progress and a **Cancel** button. After you edit the layout,
the panel notes that results are out of date.

## Design rule check (DRC)

![A spacing violation is marked, the shape is moved and the check comes back clean](images/drc-check.svg)

![DRC](screenshots/11_layout_drc.webp)

1. Open the cell's layout.
2. Press **Run DRC** in the **Design Rule Check** section (or **Verify > Run DRC**).
3. The result is either **No violations** or a list grouped by rule, with a count per
   rule. Markers are drawn on the canvas.
4. Expand a rule to see its description and the violation locations; click one to zoom to
   it.
5. **Clear** removes the results and markers.

The rules come from the technology's deck. Expand **Rule deck** to view and edit it in
place: **Run DRC** uses the edited text right away, **Save to technology** stores it in
the project's `tech.json`, and **Reset** replaces it with the deck derived from the
technology's layer and via rules. Syntax errors are listed by line. See
[Technology](technology.md#drc-rule-deck) for the deck syntax. **Rule Designer...** (also
**Tools > DRC Rule Designer**) edits the deck as forms and self-tests every rule on
generated patterns; see [DRC Rule Designer](tools.md#drc-rule-designer).

A layer that fails a `density` rule can be filled with **Tools > Dummy Fill** (see
[Dummy Fill](tools.md#dummy-fill)).

Distances between corners are Euclidean. Any-angle (for example 45°) shapes are checked
exactly by `width`, `space`, `separation`, `enclosure` and `extension`: distances are
measured edge to edge on the true polygons, distances within 3/4 dbu of the limit pass
(vertices off 45° lines are rounded to the grid) and acute corners are flagged. The other
rules (`area`, `density`, `antenna` and derived selections) see any-angle shapes as a
staircase of 10 dbu bands.

### Live DRC

The **Live DRC** checkbox in the layout toolbar re-checks the deck around every edit in the
background and draws the violations as dashed boxes as they appear; hover one to see its
rule. **Quick Fix** (`Ctrl+.`) on a violation offers candidate edits of top-level shapes
(reshape, move or add), each checked with DRC first and marked **clean** or with the number
of new violations it causes; it turns live DRC on when it is off. **Fix All Violations**
(command palette) applies fixes until none helps any more, as one undoable step. Geometry
inside placed cells is not changed.

Live DRC is incremental and hierarchical, so it stays fast on large layouts:

- It keeps the result of the last check and, after an edit, re-checks only the area within
  the deck's reach of the changed region.
- Each placed cell is checked once and its result reused for every placement. In a parent
  cell, only the interaction zones are checked again on the flattened geometry: where
  placements overlap or come near each other or the parent's own shapes. Rules on whole
  pieces (`area`, `separation`, `enclosure`) widen the zone until every piece in it is
  complete, so the violations equal those of a flat check.
- Global rules (`antenna`, `density`) are left to **Run DRC**, which checks the whole
  flattened cell.

## Connectivity extraction

![Layout wires turn into an RC network of parasitics, and the resimulated waveform is slower](images/pex-backannotate.svg)

1. Press **Extract** in the **Connectivity** section (or **Verify > Extract Nets**).
2. The summary shows the number of nets and NMOS / PMOS transistors.
3. Click a net in **Nets** to highlight it. Check **Pick nets** and click a shape on the
   canvas to highlight the net it belongs to.
4. **Devices** lists each transistor with W, L and its terminal nets; click one to zoom to
   its gate.

**Tools > Net Browser** lists the extracted nets in a sortable table with their layers,
area, transistor terminals and wire capacitance; click one to highlight it and zoom to it
(see [Net Browser](tools.md#net-browser)).

How the extractor works:

- Conducting layers are the layers joined by vias. Diffusion is `active` minus `poly`.
- A transistor is `poly` over `active`. It is PMOS when it is inside `pplus` (or inside
  `nwell` when there is no implant). L is measured across the gate; W is area / L. Bent,
  ring and 45° gates take W from the mean of the source and drain edge lengths, with the
  PDK's `bend_corner` correction (see
  [Device shapes](pcells.md#device-shapes-extraction-understands)).
- Any-angle shapes are approximated by a fine staircase for connectivity; gate W / L use
  the exact geometry.
- **Resistors** are the technology's `[[resistor]]` bodies: a conducting layer under its
  marker layers, between two terminal pieces. R = sheet resistance × squares (L / W, bent
  bodies measured like bent gates).
- **Capacitors** are the `[[capacitor]]` overlaps of a bottom and a top plate under the
  marker layers. C = area × area capacitance (+ perimeter × perimeter capacitance). See
  [Technology](technology.md#pdk-files) and the `poly_res` / `mom_cap`
  [PCells](pcells.md).
- **Bipolar transistors** are the technology's `[[bjt]]` devices under their marker layers:
  vertical PNPs (a p+ emitter inside an `nwell`, the well as base, the substrate as
  collector) and, in a triple-well technology (a `dnwell` role), vertical NPNs (an n+
  emitter inside an isolated p-well, the p-well as base, the deep n-well as collector). The
  area factor is the emitter area over `unit_area` (1 per emitter without it). LVS compares
  them with the schematic's `Npn` / `Pnp` instances: base, emitter and collector, area, and
  the model unless the schematic keeps the generic `npn` / `pnp`.
- **Triple well**: with a `dnwell` role, the area inside the deep n-well and outside `nwell`
  is an isolated p-well (`pwellN` when unlabelled). Its p+ taps and the bodies of the NMOS
  inside it go to the p-well instead of the substrate, and the nwell over the deep n-well
  joins it.
- **Well taps are soft connections**: an n+ tap in an `nwell` (or a p+ tap in the substrate)
  gives the well the net it is on, but never joins two nets through the well, as Calibre's
  `SOFT CONNECT`. A well or substrate tapped from several nets (`vss` and `agnd`, two
  supplies in one well) is taken as the net with the most taps, and LVS warns
  `soft connection: ... is tapped from nets ...`.
- Net names come from labels in the top cell on conducting layers. Unnamed nets are called
  `netN`.
- Warnings report labels not on conducting geometry, two labels shorted together, and a
  label on several unconnected pieces.

Highlighting a net (click it in **Nets**, or **Mark Net**) shows its electrical view under
**Net ...: electrical**: capacitance to substrate, the largest coupling capacitances to
other nets, the worst resistance of its wiring from the first device terminal to the others
(solved on the extracted RC network), and R x C as a rough delay bound.

### Photodiodes and pixels (image sensors)

With a photodiode in the technology's cross section (`photodiode` of the
[pixel cross section](technology.md#pixel-cross-section)), extraction also finds the photodiodes, and from them the
pixel transistors, by connectivity alone:

- **Photodiode**: the diffusion under the photodiode `layer`, one per diffusion piece: n+ in
  a p-well or the substrate (anode = well, cathode = the piece), or p+ in an nwell. A layer
  of its own (a PD marker or implant, as image sensor PDKs have) is taken as drawn, and
  conducts as diffusion even where no active is drawn under it. A
  derived expression such as the demo's `active wide 1` is a heuristic: then only
  uncontacted diffusion without a MOS capacitor plate counts, as a pinned photodiode is.
  The photodiode is listed with its area and perimeter (`* photodiode D1 sub pd area=...`
  in the SPICE); its junction is already the source junction of the transfer gate.
- **TG**: a transistor with its source or drain on the photodiode; the other side is the
  floating diffusion **FD**. Photodiodes whose TGs share one FD are one shared pixel. Other
  gates on the photodiode (anti-blooming, overflow) are listed as `other`. Without a TG, a
  photodiode that drives a gate itself is a 3T pixel.
- **SF** is gated by FD; **RST** runs from FD to one side of SF, the supply; **SEL** runs
  from the follower output to the column line. When RST reaches FD only through another
  device (FD, a switch, a node, RST, the supply), that switch is the dual conversion gain
  gate **DCG**; the test bench holds it off (high gain).

**Verify > Schematic from Layout** draws the extraction as a schematic: each pixel in the
textbook arrangement (devices named `M1_TG`, `M4_RST`, ... with W / L and `as ps ad pd`
from the layout, each photodiode as its photocurrent source `IPH_D1`), other devices in rows,
net labels on every pin, and a test bench: the supply, reset and transfer pulses, row
select, a column current load, the FD wire capacitance as a setup line and a transient
analysis, so the cell simulates at once. It goes into the cell when it has no schematic yet
(then LVS compares the two) and into a new `<cell>_from_layout` otherwise. From the command
line:

```bash
ics extract demo aps4t --pixel                      # roles, W / L, junctions, PD and FD geometry
ics extract demo aps4t --pixel --characterize       # + C_FD, conversion gain, SF gain, C_PD, ...
ics extract demo aps4t --pixel --characterize --temp 60 --ccol 2e-12
ics extract demo aps4t --schematic --into aps4t_sch # draw it, with the test bench
ics sim demo aps4t_sch
```

`--characterize` (MCP `extract` with `characterize: true`; in the app **Verify > Pixel
Circuit…**, which **Schematic from Layout** also opens: tables of the pixels and the
photodiodes, worked out in the background with the temperature, supply and column
capacitance set there) simulates each pixel alone with the device models of the technology (one pixel of an array
keeps its column load: the column line's wire capacitance and the other rows' select
transistors, off):
FD and every photodiode are reset, RST and TX turn off, then 500 electrons are taken out of
FD and out of each photodiode in turn (every one of a shared pixel). C = Q / ΔV includes every junction, overlap and gate
capacitance on the node and its wire capacitance from the layout:

```
pixel 1 FD: reset 0.991 V, C = 1.496 fF, CG = 107.1 uV/e- at FD, 89.6 uV/e- at the column (SF gain 0.836)
  Noise: kTC at FD 15.5 e- rms (removed by CDS); readout 58.5 uV rms = 0.65 e- rms per sample from 1 kHz, CDS 82.8 uV = 0.92 e- (2 us apart), 1 pF on the column
  D1 dark current: 3.87 aA = 24.1 e-/s at 27 C (doubles every 9.0 C)
  D1 (pd, pinned): reset 0.478 V, C = 6.511 fF; transfer of 500 e-: 494.3 e- out, lag 5.7 e-; full well 34177 e- (saturates at -0.363 V)
```

**Pinned photodiodes.** With `pinning` set on the technology's photodiode, the drawing and
the simulations model it as pinned: its charge sits on a storage node `<pd>_q` with the
storage capacitance (`cap_area` × area + `cap_perimeter` × perimeter, `CPD_D1`), joined to the
photodiode node by a conductance that never charges it above the pinning voltage
(`BPIN_D1 pd pd_q I=1m*(min(V(pd),0.5)-V(pd_q))`). Emptied, it holds no charge whatever TX
and FD do, so it resets to the pinning voltage without kTC noise and transfers completely;
the TG keeps only the junction of the diffusion outside the photodiode region. Blooming
(over TX, or forward bias into the substrate) comes from the device models.

The characterisation then also reports, for each photodiode:

- **Transfer and lag**: TX transfers the 500 electrons; against a run without them (same TX
  feedthrough and channel charge) the photodiode's remaining charge is the lag, left behind
  or spilled back when TX turns off. A plain junction shares its charge with FD instead and
  keeps most of it, about C_PD / (C_PD + C_FD). The spill-back depends on how the channel
  charge splits at turn-off, which Meyer capacitances do not conserve: the characterisation
  runs level-1 models of the deck with `capmod=1` (charge-conserving gate charges, see
  [Simulation](simulation.md)); PDK BSIM4 models are charge based already.
- **Noise**: the reset (kTC) noise of FD, sqrt(k T C_FD) / q, which correlated double
  sampling removes in a 4T pixel (a pinned photodiode resets without it); and the readout
  noise from a noise analysis at the reset levels (RST and TX off, SEL on, FD and the
  photodiode held by a noiseless 1 nS transconductance), at the column with the column line
  capacitance (`--ccol`, 1 pF by default) setting the bandwidth, in µV and in electrons
  through the column conversion gain: one sample, integrated from 1 kHz to 1 GHz, and the
  correlated double sample, two samples `--cds-dt` apart (2 µs by default), integrated from
  1 Hz with |H(f)|² = 4 sin²(π f Δt). For white noise that is √2 times one sample; flicker
  noise (when the models have `kf`) it cancels at low frequencies.
- **Dark current** (with the photodiode's `dark_area` / `dark_perimeter`): the densities at
  `dark_temp` scaled to the bench temperature (`--temp`, 27 °C by default) by
  exp(Ea / k (1 / T_ref − 1 / T)), in aA and electrons per second, with the temperature step
  that doubles it (about 9 °C for 0.6 eV). The drawn test bench adds it as `IDK_D1` from the
  charge node, at the bench temperature (a later change of the setup temperature does not
  update it).
- **Full well**: FD held in reset, the photodiode filled by light at three times the rate
  needed to empty it in 1.5 µs; FWC = C × (reset − saturated voltage). `not yet saturated`
  warns when it still moves at the end.

A transfer gate with its own threshold implant takes its PDK model when the technology lists
that device with the implant as a marker layer (`[[device]]` `layers`), as any transistor does.

## Layout versus schematic (LVS)

![The layout and schematic netlists side by side, devices matched one by one](images/lvs-compare.svg)

![LVS](screenshots/12_layout_lvs.webp)

1. The cell needs both a schematic and a layout. Device names do not have to match, but
   labels on the layout's pins (`in`, `out`, `vdd`, ...) help: equal net names anchor
   the comparison.
2. Press **Run LVS** (or **Verify > Run LVS**).
3. The result is **Layout matches schematic** or a list of differences, each tagged:
   - `[missing]`: a schematic device not found in the layout,
   - `[extra]`: a layout device not in the schematic,
   - `[param]`: W or L, a resistance or capacitance, or the device model differ (sizes
     and values by more than 1 %),
   - `[net]`: a net whose connections differ.

   Click a difference with a location to zoom to it.

Comparison rules:

- MOS transistors, resistors and capacitors are compared. Voltage and current sources are
  ignored; other schematic devices (diodes, inductors) are reported as not extracted.
- Parallel devices with the same terminals (and L for MOS) are merged: MOS widths and
  capacitances add, resistances combine as conductances. Multi-finger layouts match
  single schematic devices; the schematic `m` multiplies W.
- Resistors of one model in series through an unnamed net that nothing else touches merge
  into one, so a resistor drawn as several segments matches one schematic resistor. A
  labelled net between them stops the merge. Merged devices are reported with their names
  joined by `+`.
- A resistor's or capacitor's model is compared only when the schematic names one.
- Bulk terminals are ignored.
- Devices that are always off (gate tied to bulk), such as dummy fingers, are left out of
  the matching, and layout ones need no schematic counterpart. A schematic one whose drain
  or source is on another net, such as a GGNMOS ESD clamp, must still have off layout
  devices of its type between the same nets with the same total W; otherwise it is reported
  `[missing]` (or `[param]` for the W).
- Net names are compared case-insensitively; `0`, `gnd` and `vss` are treated as the same.

The demo cell `cmos_inverter` deliberately has LVS findings (a PMOS width and the load
capacitor); `nand2`, `nor2`, `ring_osc5`, `current_mirror`, `ota5t`, `sram6t`, `aps4t`, `aps2s`, `full_adder` and `ota2s` are clean.

## IR drop and electromigration

![IR drop heatmap of a power grid: blue near the pins, red where the drop is largest](images/emir-heatmap.svg)

The **IR Drop / EM** section of the right panel (also **IR drop / EM analysis** in the
command palette) analyses one net, typically a supply such as `vdd` or `gnd`:

1. Type the **Net** name. The net needs labels in the top cell: they are its pads, held at
   the pad voltage.
2. Set **mA / device**: the current every device on the net draws. With `0`, the
   schematic's operating point is simulated and each MOS draws its own drain current.
3. Press **Run IR / EM**.

The net's wires on layers with a sheet resistance are cut into tiles, joined by the
resistance between them and by the resistance of each via cut. Every MOS with its source or
drain on the net draws its current from the tile nearest to its gate, and the network is
solved for the voltage drop from the pads. A current returned into a ground net gives the
same rise as a drawn current gives drop.

- The summary lists nodes, resistors, pads, loads and their total current, and the worst
  drop and where it is. Tiles not connected to any pad are reported.
- The canvas shows a heatmap of the drop, from blue (0 V) to red (the worst drop).
- **EM** checks the current per µm of wire width against the layer's limit and the current
  per via cut against the via's limit. Violations are listed (click one to zoom to it) and
  outlined in red on the canvas.

The limits come from the technology: `em` (A per µm of width) and `resistance` (Ω per
square) in the parasitic coefficients of a layer (`[[cap]]` in a PDK file), and `em`
(A per cut) and `resistance` (Ω per cut) of a `[[via]]`. A limit of 0 is not checked. The
generic technology has illustrative values. From the command line:
`ics emir <project> <cell> --net vdd [--currents FILE] [--json]` (exit code 1 on EM
violations), and over MCP the `emir` tool.

## Sign-off with external tools

The built-in checks are fast and good for iterating. For sign-off, a technology can list
**runsets** that run a foundry or PDK deck in an external tool:

| Tool | Checks | Deck |
|---|---|---|
| KLayout | DRC, LVS | KLayout `.drc` / `.lvs` script |
| Siemens Calibre | DRC, LVS | SVRF rule file |
| netgen | LVS | netgen setup file (compares IC Studio's extraction with the schematic) |
| built-in | DRC | IC Studio or SVRF deck file, run on the built-in engine |

In the Layout workspace, the **Sign-off** section appears when the technology has
runsets. Pick a **Runset** and press **Run <tool>** (or **Verify > Run Sign-off Runset**). IC Studio
writes the cell and every cell it places to `<cell>.gds` in a new temporary folder, plus
the schematic as a `.subckt` for LVS, runs the tool there and reads its report:

- DRC violations appear as markers on the canvas, like the built-in DRC. Violations the
  tool reports inside sub-cells keep sub-cell coordinates and name the cell.
- LVS shows whether the layout matches and the tool's messages.
- The folder (shown under the result) keeps the inputs, reports and the tool's log.

How each tool is called:

- KLayout: `klayout -b -r DECK -rd input=CELL.gds -rd topcell=CELL -rd report=FILE`, plus
  `-rd schematic=CELL_sch.cir` for LVS and the runset's `vars`. The variable names can be
  changed with `klayout = { input = "...", topcell = "...", report = "...", schematic = "..." }`.
- Calibre: a control file sets `LAYOUT PATH`, `LAYOUT PRIMARY`, `LAYOUT SYSTEM`, the ASCII
  results database (DRC) or `SOURCE PATH` / `LVS REPORT` (LVS), adds `#DEFINE` for each var
  and `INCLUDE`s the deck. The deck itself must not set these statements. Then
  `calibre -drc -hier` or `calibre -lvs -hier -spice`.
- netgen: `netgen -batch lvs "CELL_lay.cir CELL" "CELL_sch.cir CELL" SETUP REPORT`.

The schematic netlist leaves out the element letters in the runset's `skip` (default
`V` and `I`) and names the ground net `ground` (default `gnd`). Executables are found on
the usual install paths and `PATH`; set `ICS_KLAYOUT`, `ICS_CALIBRE` or `ICS_NETGEN` to
override them.

Runsets come from a [PDK file](technology.md#pdk-files) (`[[signoff]]`) or the `signoff`
array in `tech.json`:

```json
"signoff": [
  {"name": "KLayout DRC", "tool": "klayout", "check": "drc", "deck": "/pdk/rules.drc",
   "vars": {"feol": "true"}},
  {"name": "Calibre LVS", "tool": "calibre", "check": "lvs", "deck": "/pdk/calibre.lvs",
   "skip": ["V", "I", "C"]}
]
```

### Results of runs made elsewhere

DRC and LVS often run on a compute farm. Their results show on the cell like a local run:

- GUI: **Import Sign-off Results...** in the Sign-off section (or the command palette)
- CLI: `ics results <project> <cell> <file>`, non-zero exit on violations or an LVS mismatch

The format is recognised from the file: a Calibre ASCII DRC results database, a Calibre LVS
report, a KLayout report database (`.lyrdb`) or LVS database (`.lvsdb`), or a netgen
report. DRC markers are placed in the cell's coordinates.

## From the command line

```sh
ics drc myproj nand2 [--deck rules.drc]   # exit code 1 on violations
ics extract myproj nand2                  # extracted SPICE netlist
ics lvs myproj nand2                      # exit code 1 on differences
ics emir myproj nand2 --net vdd           # exit code 1 on EM violations
ics signoff myproj nand2                  # list the sign-off runsets
ics signoff myproj nand2 "KLayout DRC"    # run one; exit code 1 on violations / mismatch
```

These exit codes make the checks usable in CI. See [Command line](cli.md).
