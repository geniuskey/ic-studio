<!-- seo title="Schematic Editor for IC Design" description="Draw transistor-level schematics in IC Studio: device library, wires, labels, symbols, hierarchy and netlisting for SPICE simulation." -->

# Schematic editor

Open a cell's schematic view (or press `Ctrl+2` with a cell open) to edit it.

![Schematic editor](screenshots/03_schematic.webp)

- **Left panel**: the **Device Library** with one tile per primitive and its shortcut
  letter, and a short shortcut reminder. With the label tool, the net name to place.
- **Toolbar**: Select, Wire and Label, the current tool, undo / redo, rotate, mirror, duplicate, fit, the **Grid**
  and **Annotate OP** checkboxes, and **Simulate** (`F5`).
- **Right panel**: the property inspector.

Schematic coordinates are integer grid points. Keys act on the canvas only while the
mouse is over it and no text field has focus.

![Placing devices, drawing wires, highlighting a net and annotating the operating point](images/schematic-wiring.svg)

## Devices

| Key | Device | Name prefix | Pins | Default parameters |
|---|---|---|---|---|
| `R` | Resistor | R | p, n | `r=1k` |
| `C` | Capacitor | C | p, n | `c=1p` |
| `L` | Inductor | L | p, n | `l=1n` |
| `V` | Voltage Source | V | p, n | `dc=1` |
| `I` | Current Source | I | p, n | `dc=1m` |
| `D` | Diode | D | a, k | `model=dmod` |
| `N` | NMOS | M | d, g, s, b | `model=nmos w=1u l=180n m=1` |
| `P` | PMOS | M | d, g, s, b | `model=pmos w=2u l=180n m=1` |
| `G` | Ground | GND | gnd | (net `0`) |
| `S` | Supply Net | VDD | net | `net=vdd` |

### Placing a device

1. Press the device key (for example `N`) or click its tile in the Device Library.
2. A ghost follows the cursor. Press `Space` to rotate or `X` to mirror it before placing.
3. Click to place. The device snaps to the grid, gets the next free name (`M1`, `M2`, ...)
   and is selected. The tool stays active, so further clicks place more copies.
4. Press `Esc` or right-click to return to Select.

## Wires

1. Press `W` (or choose **Wire**).
2. Click the start point, usually a pin.
3. Click the next point. Each click adds an L-shaped pair of segments, horizontal first;
   press `/` to bend vertical first.
4. The wire ends automatically when a click lands on a pin, a wire end or the interior of
   another wire. Otherwise keep clicking, and end with double-click, right-click or `Esc`.

Connections are live: pins that are not connected are drawn as small squares, and a
junction dot appears where three or more connections meet. Hovering or selecting a wire
highlights its whole net.

## Net labels and net names

1. Press `T` (or choose **Label**). A **Net:** field appears in the left panel (default
   `net1`).
2. Type the name and click on a pin or wire to attach the label. Labels connect only at
   their exact grid point.
3. A trailing number is incremented after each placement (`net1`, `net2`, ...), which is
   handy for buses of labels.

Labels with the same name are the same net, so labels connect parts of a schematic
without wires. Net names are chosen in this order: the **Ground** symbol (net `0`), the
**Supply Net** symbol's `net` parameter (blank means `vdd`), net labels, then automatic
names (`net1`, `net2`, ...).

### Pins

A label becomes a **pin** of the cell (a Virtuoso schematic pin) when you set **Pin** to
`input`, `output` or `inputOutput` in its inspector; it is drawn with a pin outline. Pins
are the cell's ports:

- The CDL netlist (and LVS) uses them as the `.SUBCKT` ports, sorted by name, with their
  `*.PININFO` directions. Without pins, every named net is a port.
- CDL import (and Virtuoso library import) turns `*.PININFO` pins into schematic pins, and
  the SKILL export creates `basic` `ipin` / `opin` / `iopin` pins.
- Check Against Source asks for a layout pin label on each pin's net (on every named net
  when there are no pins).
- MCP `write_schematic` takes `pins`, e.g. `{"in": "input", "out": "output"}`.

Notes:

- Only the Ground symbol makes net `0`. A label called `gnd` is an ordinary net.
- A Supply Net symbol only names a net. Place a voltage source on it to power it.
- Two different names on one net produce the warning
  "Net names 'a' and 'b' are shorted together".

## Hierarchical schematics

A schematic can place other cells: their schematic pins (see **Pins**) become the pins of
a box symbol, inputs on the left, outputs on the right and inouts on top and bottom. The
demo cell `buffer_tb` places two `inv` cells.

- **Place Cell Instance…** (`Shift+I`, **Tools** menu, or **Place in Active Schematic** on
  a cell in the Explorer) lists the cells whose schematic has pins. Click to place, `Space`
  / `X` rotate and mirror, `Esc` ends. Instances are named `X1`, `X2`, ...
- **Descend into Cell** (`Shift+X` on a selected instance, or its context menu) opens the
  placed cell's schematic; **Return to Parent** (`Shift+B`) goes back.
- **Update Cell Instance Pins** gives the instances the current pins of their cells after
  pins were added or renamed. Until then the netlist warns that the pins changed.
- **Edit Symbol Pins…** (**Tools** menu, on the placed cell's own schematic) sets each pin's
  side and its order along it, with a preview of the box; **Apply** updates every instance
  of the cell in the project, **Default** goes back to the automatic arrangement.
- The SKILL export writes cell instances as instances of the cell's `symbol` view in the
  target library, creating it from the schematic's pins when it is missing (export the
  placed cells first). Cross-probing selects instance `X1` for the layout's `X1/M1`.

Simulation, sweeps, Monte Carlo, LVS, schematic-driven layout and ECO use the flattened
circuit: device `M1` of instance `X1` is `X1/M1`, a net inside it `X1/n3`. A placed cell's
pins take the nets wired to the instance; ground and the nets of supply symbols are
global. A cell that places itself, directly or through other cells, is an error.

## Selecting and editing

| Action | How |
|---|---|
| Select one object | Click it (labels take priority over wires, wires over devices) |
| Add or remove from selection | `Shift`+click or `Ctrl`+click |
| Box select | Drag on empty space (selects objects fully inside; `Shift` adds) |
| Select all | `Ctrl+A` |
| Deselect all | `Ctrl+D` |
| Move | Drag a selected object; attached wires stretch with moved pins |
| Rotate / mirror | `Space` / `X`, or the toolbar buttons |
| Duplicate | `Ctrl+Shift+D` (copies offset by two grid points, renamed) |
| Delete | `Del` or `Backspace` |
| Undo / redo | `Ctrl+Z` / `Ctrl+Shift+Z` or `Ctrl+Y` |
| Fit to window | `F` |
| Cancel | `Esc` cancels a wire, then returns to Select, then clears the selection |

## Property inspector

Select a single device to edit it in the right panel. Values are SPICE strings typed as
they would appear in a netlist, with SPICE scale suffixes: `10k`, `2.2p`, `180n`, `1meg`.

- **Name**: the instance name. It must be unique; it also ties the device to its layout
  instance.
- Device fields: **Resistance**, **Capacitance**, **Inductance / L**, **Model**,
  **Width**, **Multiplier**, **Net name** (supply).
- Sources: **DC value**, **AC magnitude** (for AC analysis, e.g. `1`) and **Transient**
  (e.g. `PULSE(0 1 0 1n 1n 5n 10n)`; `SIN(...)` and `PWL(...)` also work).
- **Orientation** and **Position**, and a **Connections** list showing the net on each pin.

With nothing selected, the inspector shows statistics (instances, wires, nets), the list
of nets with their pin counts, and any **Warnings** (unconnected pins, duplicate names,
shorted net names).

The **Problems** tab of the bottom panel runs the schematic checker on the active cell as
you edit: missing ground, bad or duplicate names, missing or invalid values, shorted
devices, voltage-source loops, floating nets, dangling wires and more. Click a finding to
select its devices. **Tools > Check Schematic** runs the check on demand, and **File >
Check and Save** saves only when there are no errors. See
[Check Schematic](tools.md#check-schematic) for every check.

## Netlist

The **Netlist** tab of the bottom panel shows the SPICE deck of the active cell, updated
as you edit. Export it with **File > Export > SPICE netlist...**. The netlister writes:

- `R1 p n value`, `C...`, `L...`
- `V1 p n DC <dc> [AC <ac>] [<tran>]`, likewise for current sources
- `D1 a k dmod`
- `M1 d g s b <model> w=... l=... [m=...]`; an unconnected bulk pin is tied to the source
- default level-1 models `nmos`, `pmos` and `dmod` when they are used. With a PDK model
  library, MOS models map to the PDK devices (subcircuit devices get an `X` prefix) and the
  library is loaded at the chosen corner.

## Exchanging schematics with Virtuoso

OpenAccess is a binary C++ database, so schematics move between IC Studio and Cadence
Virtuoso through the text formats Virtuoso writes and reads itself:

- **From Virtuoso: CDL.** Export a CDL netlist in Virtuoso (the auCdl netlister, as for
  Calibre LVS). In IC Studio, use **File > Import > CDL netlist (Virtuoso)...** (or
  `ics import-cdl`, MCP `import_cdl`). The importer reads:

  - `.SUBCKT` with `*.PININFO` directions;
  - `X` instances with or without `/ cell`;
  - subcircuit parameters with expressions (`W='wp/2'`);
  - `.GLOBAL` and `!` global nets (`gnd!` is ground);
  - `$` annotations.

  When its subcircuits place each other, **File > Import** keeps the hierarchy: a new
  library named after the file gets a cell per subcircuit, with the subcircuit's pins as
  schematic pins and each `X` instance as an instance of that cell (`ics import-cdl --hier`,
  MCP `import_cdl` with `hierarchical`). An instance that passes other parameter values
  places a variant cell (`inv_wp4u`). Otherwise, and with `ics import-cdl` without `--hier`,
  it flattens the netlist from the top subcircuit into one schematic. Element names carry
  the instance path (`MI0/M0` is `MM0` inside `XI0`), and local nets get it too
  (`I0/net7`). Every pin gets a net label, as with SPICE netlists. A resistor or capacitor
  given only by a `$[model]` and its size gets a placeholder value and a warning.
  `ics import-cdl --all` imports every subcircuit as its own cell.
- **To Virtuoso: SKILL.** **File > Export > Virtuoso schematic (SKILL)...** (or
  `ics export-skill`, MCP `export_skill_schematic`) writes a script. Loaded in the CIW
  (`load("inv.il")`), it creates the schematic with analogLib symbols (`nmos4`, `pmos4`,
  `res`, `cap`, `ind`, `vdc`, `idc`, `diode`) and sets their parameters. Each terminal gets a
  short wire and a wire label with its net, placed at the pin the symbol defines (looked up
  when the script runs), so connectivity does not depend on symbol geometry. Run
  **Check and Save** afterwards and tidy the placement if you like.

  For PDK symbols, put a `virtuoso_symbols.toml` next to the project (or pass `--map`):

  ```toml
  [[symbol]]
  kind = "nmos"            # nmos, pmos, resistor, capacitor, inductor, vsource, isource, diode
  model = "nch"            # optional: only instances of this model
  lib = "gpdk180"
  cell = "nmos1v"
  terms = { d = "D", g = "G", s = "S", b = "B" }
  params = { w = "w", l = "l", m = "m" }   # IC Studio parameter = CDF parameter
  ```

  IC Studio's own **CDL** netlist (`ics netlist --format cdl`) also goes the other way
  through Virtuoso's CDL import.

### Whole libraries: the Virtuoso dump

CDL carries connectivity only. To bring schematics *and* layouts over together, run a small
SKILL script inside Virtuoso that writes the library as text, then import that file:

1. Save the script with **File > Export > Virtuoso dump script (icsDump.il)...** (or
   `ics dump-skill -o icsDump.il`).
2. In the Virtuoso CIW:

   ```skill
   load("icsDump.il")
   icsDumpLib("myLib" "/tmp/myLib.icsdump")                          ; every cell
   icsDumpLib("myLib" "/tmp/amp.icsdump" ?cells list("amp" "bias"))  ; some cells
   icsDumpLib("myLib" "/tmp/sch.icsdump" ?views list("schematic"))   ; schematics only
   ```

3. In IC Studio: **File > Import > Virtuoso library dump...** (or
   `ics import-virtuoso <project> /tmp/myLib.icsdump [--map virtuoso_symbols.toml] [--scale S] [--overwrite]`,
   MCP `import_virtuoso`).

**In one step, with Virtuoso installed**: pick the library's `cds.lib` in **File > Import >
Virtuoso library (dump or cds.lib)...** and choose a library, or run
`ics import-virtuoso <project> path/to/cds.lib --lib myLib` (also a directory holding a
`cds.lib`, or the library directory itself). IC Studio runs Virtuoso in batch mode
(`virtuoso -nograph -replay`) in a scratch directory whose `cds.lib` includes yours, with the
dump script, and imports the dump; steps 1 to 3 happen on their own. Virtuoso is
`$ICS_VIRTUOSO`, else `virtuoso` on the `PATH`; `cds.lib` files may use `INCLUDE`,
`SOFTINCLUDE`, `UNDEFINE` and environment variables. OpenAccess itself is not linked.

What comes over:

- **Schematics**: each one becomes a cell. Instances are matched to IC Studio devices by
  the same symbol map as the SKILL export, read in reverse (analogLib by default). Their
  parameters are the instance's CDF values, so defaults the instance does not override are
  included. The net on each terminal comes from Virtuoso's own connectivity. An instance of
  another dumped schematic stays an instance of that schematic's cell (a hierarchical
  schematic); one passing other CDF values places a variant cell. `pPar("wp")` takes the
  parent's value, or the cell's CDF default.
  A schematic with no subcircuits keeps the Virtuoso placement and orientation, at `--scale`
  user units per grid point, as long as that gives the same connectivity. Otherwise its
  devices are laid out in rows, and a warning says so. Pin, supply and `basic` symbols are
  not imported as devices, because their nets are already on the terminals. Iterated
  instances (`I0<0:3>`) are skipped with a warning.
- **Layouts**: rectangles, polygons, paths (`path` and `pathSeg`), labels, instances and
  mosaics (as arrays). Layer-purpose pairs map to technology layers. A purpose other than
  `drawing` looks for `metal1.pin` / `metal1_pin` first; `pin`, `net`, `label` and
  `boundary` purposes then fall back to the layer itself. Shapes on unknown layers are
  counted and skipped. Masters from other libraries (standard cells, PDK devices) come in as
  cells of their own libraries. Each PCell variant becomes a fixed cell named `<cell>_pc<n>`.
  Load the PDK's technology (`.tf`, `display.drf`, layer map) first, so the layers and the
  database units match.

Existing schematics and layouts are kept unless you pass `--overwrite`.

The CDL import is tested on hierarchical auCdl-style netlists and on CDL round trips of the
demo cells. The dump reader is tested on dumps in the script's format. The layout half of
the script (`icsDumpLay`) is also run by IC Studio's SKILL interpreter on the demo layouts,
and the result reads back as the same shapes and instances. Neither the SKILL export nor
the dump script has been run in a Virtuoso session yet. The schematic half uses
`cdfGetInstCDF`, `instTerms` and `terminals`, which the built-in interpreter does not
emulate.

## Operating point annotation

After an operating point simulation, **Annotate OP** (on by default) shows each node
voltage as a tag on the schematic and the current `id=...` under each transistor and
diode.

![OP annotation](screenshots/07_schematic_op.webp)

## Cross-probing

When a cell has both a schematic and a layout, selecting devices in one editor selects the
devices with the same names in the other. Unit fingers of a matched device (`M1.1`,
`M1.2`, ...) map back to `M1`.
