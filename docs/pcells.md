<!-- seo title="PCells and Custom Devices" description="Use IC Studio's parametric cells (PCells) to generate correct MOSFETs, resistors and capacitors for generic180, SKY130 or any technology, or define your own." -->

# PCells and custom devices

![PCells](screenshots/10_layout_pcells.webp)

IC Studio has six built-in parametric cells (PCells). They are drawn by code from the
technology, so the same parameters give a correct device in generic180, SKY130 or any
other PDK. You can make your own PCells in two ways:

- Draw the device and turn it into a PCell with the [PCell Designer](#pcell-designer),
  by hand or with the Design Copilot.
- Write a generator in SKILL, for devices that need code: see
  [Your own device generators](#your-own-device-generators).

## Built-in PCells

| Kind | Parameters | Defaults | Terminal labels | Draws |
|---|---|---|---|---|
| `nmos` | W, L, fingers, device | W 1 µm, L 0.18 µm, 1 finger | `S`, `D`, `G` | Active, poly fingers with a poly contact head, diffusion contact columns, n+ implant, PDK marker layers |
| `pmos` | W, L, fingers, device | W 2 µm, L 0.18 µm, 1 finger | `S`, `D`, `G` | As `nmos`, plus p+ implant and an nwell |
| `poly_res` | W, L | W 0.4 µm, L 5 µm | `A`, `B` | Poly body with a poly contact at each end, resistor marker layers |
| `mom_cap` | W, H | 5 µm x 5 µm | `BOT`, `TOP` | metal1 bottom plate and metal2 top plate with a tab each, capacitor marker layers |
| `ntap` | H | 1 µm | `B` | n+ tap in an nwell (PMOS bulk), one contact column wide |
| `ptap` | H | 1 µm | `B` | p+ substrate tap, one contact column wide |

- **W is per finger.** An `nmos` with W 1 µm and 2 fingers is two 1 µm transistors in
  parallel (total width 2 µm); fingers alternate S / D / S. Extraction reports one device
  per finger, and LVS merges parallel devices.
- **Device** appears when the technology is a PDK with several MOS devices of that polarity
  (for example low- and high-Vt variants). The PCell then draws that device's marker layers
  and the variant name ends with the device name. The default is the PDK's first device.
- Lengths snap to the technology grid. The origin is the lower-left corner of the active
  (MOS, taps), of the resistor body or of the bottom plate.
- `ntap` and `ptap` are not in the placement palette: schematic-driven layout creates them,
  and the MCP tool `place_pcell` accepts them.

## Placing and changing PCells

![A MOSFET PCell redrawn as its fingers and width change](images/pcell-params.svg)

| Where | How |
|---|---|
| Layout editor | Press `I`, pick the PCell in the left panel, set its parameters, click to place |
| Inspector | Select a placed PCell, edit the parameters in the **PCell** section, press **Apply** |
| Schematic-driven layout | **Verify > Generate Layout from Schematic** places every MOS as a PCell sized from the schematic (see [Schematic-driven layout](schematic-driven-layout.md)) |
| Generators | **Layout Generators > Matched Placement** and **By Example** place PCell units (see [Tools](tools.md#layout-generators)) |
| MCP | `place_pcell` (see below) |

`place_pcell` takes sizes in µm:

```json
{
  "cell": "work/amp",
  "kind": "nmos",
  "params": { "w": 2, "l": 0.5, "fingers": 4 },
  "x": 0, "y": 0, "rot": 0,
  "name": "M1"
}
```

Name MOS instances after their schematic devices (`M1`) so that flylines and LVS can pair
them.

## How variants are stored

Each parameter set is generated once and stored as an ordinary cell in the `pcells`
library, named after its parameters in dbu:

| PCell | Variant name |
|---|---|
| `nmos` W 1 µm, L 0.18 µm, 2 fingers | `nmos_w1000_l180_nf2` |
| the same with PDK device `sky130_fd_pr__nfet_01v8_lvt` | `nmos_w1000_l180_nf2_sky130_fd_pr__nfet_01v8_lvt` |
| `poly_res` W 0.4 µm, L 5 µm | `poly_res_w400_l5000` |
| `mom_cap` 5 µm x 5 µm | `mom_cap_w5000_h5000` |
| `ntap` H 1 µm | `ntap_w1000` |

Placing the same parameters again reuses the variant. **Apply** in the inspector points the
instance at another variant; the old variant stays in `pcells` (delete it from the project
tree when nothing uses it).

A variant's `cell.json` keeps its parameters (`"pcell": {"kind": "mos", ...}`) next to the
layout, so hierarchy, GDS export, DRC and extraction treat it as a normal cell. When the
technology is replaced (**Tools > Load PDK**, `ics load-pdk`) every variant is regenerated
from its parameters, as long as the database unit stays the same. Do not edit a variant's
layout by hand: the edit is lost on the next regeneration. Copy it to another library
first, or write a generator.

## What the technology must provide

PCells draw only from the technology, so a new PDK needs these entries (see
[Technology](technology.md)):

| Entry | Used for |
|---|---|
| `roles`: `diffusion`, `tap`, `poly`, `nwell`, `nimplant`, `pimplant` | The layers each PCell draws |
| `roles`: `diff_contact`, `poly_contact` | The vias for S / D columns, tap contacts and poly heads |
| Layer `min_width`, `min_spacing`, `min_area` | Minimum W and L, contact plate size, gate-to-contact distance |
| `rules`: `gate_contact_space`, `poly_endcap`, `implant_enc`, `nwell_enc`, `marker_enc` | Spacings and enclosures inside the device |
| `[[device]]` `layers` | Marker layers of each MOS device |
| `[[resistor]]` / `[[capacitor]]` `layers` | Marker layers of `poly_res` (resistor on the poly layer) and `mom_cap` (capacitor between metal1 and metal2) |
| `[[pcell_layer]]` | Any further layer, drawn around `poly_contact`, `diff_contact`, `gate`, `diffusion`, `poly` or a named layer, optionally only for some `kinds` |

Parameters the technology cannot build are refused with a message such as
`width 150 is below the minimum 220 (active width / one contact)` or
`gate length 100 is below the poly minimum width 150`. The PDK tests check that every
PCell of SKY130 and IHP SG13G2 is DRC clean; do the same for a new PDK by placing each
PCell and running DRC.

## Device shapes extraction understands

Devices are recognised from geometry, not from the PCell parameters, so any layout that
draws a transistor the way the rules below expect is extracted and compared by LVS,
whether it came from a PCell, a SKILL script, GDS import or hand drawing.

| Shape | W and L |
|---|---|
| Rectangular gate on rectangular active | L across the gate between source and drain, W = area / L |
| Non-rectangular active (L-, U- or T-shaped) with straight gates | As above for each gate. One poly crossing both arms of a U makes two transistors with a shared gate |
| Bent gate (poly turning a corner over the channel, also U-shaped gates) | L = area / mean edge (the mean of the source and drain edge lengths); W = the mean edge, less a share of the corner squares set by `bend_corner` |
| Ring gate (enclosed layout transistor) | As a bent gate: W is the mean of the inner and outer perimeter |
| 45° or any-angle active or poly | As a bent gate, measured on the exact any-angle geometry |
| Instances rotated by 90° multiples or mirrored | As drawn |

A gate that is not a rectangle must split the diffusion into exactly two pieces (source and
drain); otherwise it is skipped with the warning
`gate ... does not separate two diffusion pieces`. The exception is a gate that touches a
single piece, such as a plate covering the end of a diffusion: it is a MOS capacitor (the
storage capacitor of the planar DRAM example `dram1t1c`), with source and drain on that piece,
W the edge against it and L = area / W. For connectivity, any-angle shapes on
every layer are replaced by a staircase of narrow bands (a grid step, at least 10 dbu), so
wires and contacts on them connect as drawn; device sizes always use the exact geometry.

Resistors follow the same rules: an L-shaped or 45° resistor body must join exactly two
terminal pieces, and its squares are L / W measured like a bent gate. Capacitor areas and
perimeters near any-angle shapes are exact.

**Bend correction.** Counting each corner square of a bent gate fully (W = mean edge) is the
first-order estimate. Foundry decks often count a corner as about half a square; set
`bend_corner` in the PDK `[rules]` to match:

```toml
[rules]
bend_corner = 0.5   # 1 (default): W = mean edge; 0: W = the shorter (inner) edge
```

With 0.5, the L-shaped gate above (edges 2.82 and 3.18 µm, L 0.18 µm) has W = 2.91 µm.

**DRC** checks any-angle geometry too: near any-angle shapes, `width`, `space`, `separation`,
`enclosure` and `extension` are measured on the exact polygons, edge to edge in the
Euclidean metric, so a 45° wire at the minimum width and spacing is clean. Distances within
3/4 dbu of the limit pass (any-angle vertices are rounded to the grid), and acute corners
are reported. The other rules see the staircase (see [Verification](verification.md)).

The built-in PCells always draw rectangular active. For an L- or U-shaped device, draw it
and make it a PCell with the [PCell Designer](#pcell-designer), or write a generator.

## PCell Designer

The PCell Designer turns a drawn layout into a parametric cell without writing code, like
the Virtuoso PCell Designer. Draw the device once at typical values. You then add
parameters and the operations that redraw it when they change. You can also start from
an empty drawing and design the cell from nothing.

Open it from **Tools > PCell Designer…**, or from the command palette with
**PCell designer: make a PCell from the active layout**. The **Start here** tab walks
through the steps below, and the **Design** tab shows the source drawing with every
operation on it.

### Parameters and operations

A parameter is a `length` (µm, snapped to the grid), an `integer` or a `boolean` (0 / 1 in
expressions). Each one has a default, an optional minimum and maximum, and a description.
Expressions are in µm over the parameter names. They take `+ - * / %`, comparisons,
`&& || !` and `min`, `max`, `abs`, `floor`, `ceil`, `round`, `sqrt` and `if(c, a, b)`,
for example `(nf - 1) * 0.72` or `if(guard, 0.5, 0)`.

| Operation | What it does |
|---|---|
| **Stretch X / Y** | A line across the cell. Everything on its moving side (right / top, left / bottom, or both halves) shifts by the change of its amount from the defaults, so shapes crossing it grow |
| **Repeat** | Items wholly inside a region are copied `count - 1` times, `pitch` apart. Shapes that span the region (diffusion, implants, a gate strap) stretch to follow |
| **Condition** | Items inside a region are dropped while an expression is 0 |
| **Contact fill** | A rectangle refilled with the largest centred array of via cuts after stretching, so contacts re-array instead of stretching |

Generation runs conditions first, then the stretches in list order (each also moves the
later lines and regions), then the repeats, then the contact fills. Stretch, repeat and
condition can be limited to some layers. **Earlier** / **Later** reorder the list, and
**Undo** steps back.

Polygons and paths stretch vertex by vertex. An edge that crosses a stretch line at an
angle (a 45° chamfer or wire) keeps its angle: a band along the axis is inserted at the line
when the cell grows, and removed when it shrinks. A cut point on an edge at another angle
than 45° is rounded to the grid (**Issues** notes it). A polygon part squeezed to zero width
is an error, as for a rectangle. A path that a stretch line runs along inside its width is
generated as its outline polygon, so it widens like a rectangle; other paths stay paths.

### Suggestions for a drawn MOS

| Button | Result |
|---|---|
| **Contacts to fills** | Every cluster of via cuts becomes a contact fill |
| **Detect fingers (nf)** | Identical, equally spaced gates become one unit and a repeat driven by a new `nf` (default: the drawn count). Run it before adding stretch lines |
| **Width stretch (w)** | A horizontal line through the diffusion, driven by a new `w` |
| **Length stretch (l)** | A vertical line through the first gate, driven by a new `l`. A finger repeat's pitch follows |

At the defaults, each suggestion reproduces the drawing. Draw two or three identical
fingers so the pitch can be read. Gates, diffusion and metal plates may be rectilinear
polygons (a dogbone gate, an L-shaped active); they are read as their rectangles, and
four-corner polygon cuts count as cuts.

### Checking

- **Preview** generates the cell at any values while you edit.
- **Issues** lists the lint findings:
  - an expression naming a parameter that does not exist
  - a parameter no operation uses, or one without a range
  - a default outside its range or off the grid
  - a stretch line with nothing on its moving side, or one that cuts through contacts
  - a name SKILL reserves
  - a cell that does not generate at its defaults
- **Corners** runs the technology's DRC deck on several parameter sets:
  - the defaults
  - each parameter alone at its minimum and at its maximum (booleans toggled)
  - all parameters at their minimum, and all at their maximum

  A corner that cannot be generated is reported too, for example a stretch whose minimum
  would pull edges past the line.

### Saving and placing

**Save PCell** stores the master cell `lib/<name>`, whose layout is the PCell at its
defaults. In the layout editor, the instance tool (`I`) lists saved masters next to the
built-in PCells. The inspector edits a placed instance's values, and stretch handles
change its length parameters as they do for a built-in PCell. Saving the master again
regenerates every placed variant with its own values.

**Export SKILL…** (or `ics pcell-skill <project> <cell> [--lib L] [-o FILE]`) writes a
SKILL `pcDefinePCell` for Cadence Virtuoso that generates the same layout. The drawing is
stored as data, and a small engine in the PCell body runs the operations.

### With the Design Copilot or an MCP agent

The [Design Copilot](tools.md) and any [MCP](mcp.md) client have the same workflow as
tools:

| Tool | Purpose |
|---|---|
| `pcell_design_create` | Make a master from a drawn cell (`source`, plus `suggest`) or from nothing (`add_shapes`, `set_params`, `add_ops`) |
| `pcell_design_edit` | Change the drawing, the suggestions, the parameters and the operations. The change is all or nothing, and placed variants follow |
| `pcell_design_check` | Template, lint, corner DRC and a preview at given `values`. `list_source` lists the drawing's shapes with ids. Read-only |
| `pcell_design_export_skill` | The SKILL `pcDefinePCell` text, or a file at `path` |
| `place_pcell` with `master` | Place a variant with values by parameter name |

`project_info` lists the masters under `pcell_designs`. In the app, the master opens in the
layout editor after each change, and an open PCell Designer window with no unsaved edits
reloads it.

From a drawn layout:

```json
{"tool": "pcell_design_create", "arguments": {
  "cell": "work/nmos_pc", "source": "work/nmos_drawn",
  "suggest": ["contacts", "fingers", "width", "length"]}}
{"tool": "pcell_design_check", "arguments": {"cell": "nmos_pc", "values": {"nf": 4, "w": 2}}}
{"tool": "place_pcell", "arguments": {"cell": "amp", "master": "nmos_pc",
  "params": {"nf": 4, "w": 2}, "x": 0, "y": 0, "name": "M1"}}
```

From nothing, here a metal plate whose width `w` stretches and which repeats `n` times
upwards:

```json
{"tool": "pcell_design_create", "arguments": {
  "cell": "plate",
  "add_shapes": [{"layer": "metal1", "rect": [0, 0, 1, 1]},
                 {"layer": "metal1", "label": "A", "at": [0.2, 0.5]}],
  "set_params": [{"name": "w", "default": 1, "min": 0.5, "max": 5},
                 {"name": "n", "kind": "int", "default": 1, "min": 1, "max": 4}],
  "add_ops": [{"op": "stretch", "axis": "x", "at": 0.4, "amount": "w"},
              {"op": "repeat", "axis": "y", "region": [-1, -1, 7, 2], "count": "n", "pitch": 1.5}]}}
```

An operation is changed by removing it and inserting the new one at the same place, in one
`pcell_design_edit`:
`{"remove_ops": [0], "add_ops": [{"op": "stretch", ..., "index": 0}]}`.
Then run `pcell_design_check` until the lint shows no errors and every corner is clean.

## Your own device generators

PCells written in SKILL with `pcDefinePCell` work as in Virtuoso: load the file with
**Tools > Load SKILL (.il)**, `ics skill` or the MCP tool `run_skill`. The definition is kept
with its master cell (whose layout is the PCell at its defaults), and each
`dbCreateParamInst` / `dbCreateParamInstByMasterName` parameter set becomes a variant cell
in the same library, made by running the body with `pcCellView` and the parameters bound:

```lisp
pcDefinePCell(list(ddGetObj("mylib") "plate" "layout")
  ((w 1.0) (l 2.0) (layer "metal1"))
  let((half)
    half = w / 2
    dbCreateRect(pcCellView layer list(-half:0 half:l))))
cv = dbOpenCellViewByType("work" "top" "layout" "maskLayout" "a")
dbCreateParamInstByMasterName(cv "mylib" "plate" "layout" "P1" 0:0 "R0" 1
  list(list("w" "float" 2.0)))
```

A plain SKILL procedure that writes one cell per parameter set does the same job; place
the generated cells with `I` (**Place cell**) like any other cell.

This generator draws an NMOS whose active is L-shaped, with the source contact in a
narrow vertical arm (generic180 layer names):

```lisp
; NMOS with an L-shaped active: the source contact sits in a vertical arm.
procedure(nmosL(lib w l)
  let((name cv gx dx top)
    name = sprintf(nil "nmosL_w%d_l%d" round(w*1000) round(l*1000))
    cv = dbOpenCellViewByType(lib name "layout" "maskLayout" "w")
    gx = 1.0                     ; gate left edge
    dx = gx + l + 0.3            ; drain contact left edge
    top = w + 1.2                ; top of the source arm
    dbCreatePolygon(cv "active" list(0:0 dx+0.32:0 dx+0.32:w 0.42:w 0.42:top 0:top))
    dbCreateRect(cv "poly" list(gx:-0.25 gx+l:w+0.4))
    dbCreateRect(cv "nplus" list(-0.2:-0.4 dx+0.52:top+0.2))
    foreach(c list(list(0.1 top-0.32 "S") list(dx w/2-0.11 "D"))
      let((x y)
        x = car(c)  y = cadr(c)
        dbCreateRect(cv "contact" list(x:y x+0.22:y+0.22))
        dbCreateRect(cv "metal1" list(x-0.06:y-0.06 x+0.28:y+0.28))
        dbCreateLabel(cv "metal1" x+0.11:y+0.11 caddr(c) "centerCenter" "R0" "roman" 0.1)))
    dbCreateLabel(cv "poly" gx+l/2:w+0.3 "G" "centerCenter" "R0" "roman" 0.1)
    name))

foreach(p list(list(1.0 0.18) list(2.0 0.5))
  printf("%s\n" nmosL("mylib" car(p) cadr(p))))
```

```sh
ics skill myproj nmos_l.il
ics extract myproj mylib/nmosL_w1000_l180   # M1 D G S sub nmos w=1u l=0.18u
ics drc myproj mylib/nmosL_w1000_l180       # 0 violation(s)
```

Tips:

- Cell names allow letters, digits, `_` and `-`: encode sizes in nm (`w1000`), not `1.0`.
- Mode `"w"` clears the cell, so running the script again regenerates every variant.
- Label the terminals on a routing layer (`metal1`, `poly`): labels in a placed cell are
  local to it, but they name the nets inside the cell for LVS of the cell itself.
- In point lists, write a negative coordinate after another point in parentheses,
  `1.5:1.5 (-0.3):1.5`, or the minus is read as a subtraction.
- Read rules from your PDK rather than hard-coding them, and check each variant with
  `ics drc` and `ics extract`.

## Adding a built-in PCell kind

A new built-in kind is Rust code in `crates/ics-core/src/pcell.rs`:

1. Add a variant to `enum PCell` with its parameters in dbu.
2. Extend `kind_name`, `variant_name`, `summary` and `generate` (write a `gen_*` function
   that draws from the technology roles, vias and rules), and `defaults` if it belongs in
   the placement palette.
3. Add the parameter editor in `pcell_params` (`crates/ics-studio/src/layout_view/edit.rs`)
   and the kind in `pcell_from` and the `place_pcell` schema (`crates/ics-mcp/src/tools.rs`).
4. If extraction should recognise it as a new device type, extend `extract.rs` and `lvs.rs`.
5. Add tests: DRC clean in generic180 and in the PDK tests of `pdk.rs`, and extraction of
   the generated layout.
