# Technology

The technology (PDK) of a project lives in `tech.json` in the project folder. It defines
the layers, their GDS numbers and display style, simple width / spacing rules, vias,
electrical coefficients for parasitic extraction, and the DRC rule deck. A project without
`tech.json` uses the built-in `generic180` technology.

## generic180

A generic 180 nm-class technology, used by new projects and the demo. 1 dbu = 1 nm, grid
10 nm.

| Layer | GDS | Layer | GDS |
|---|---|---|---|
| nwell | 1 | via1 | 8 |
| active | 2 | metal2 | 9 |
| nplus | 3 | via2 | 10 |
| pplus | 4 | metal3 | 11 |
| poly | 5 | boundary | 62 |
| contact | 6 | text | 63 |
| metal1 | 7 | | |

Routing layers are poly and metal1 to metal3. Vias: `contact` (active to metal1),
`polycon` (poly to metal1), `via1`, `via2`.

## tech.json

Top level:

| Field | Meaning |
|---|---|
| `name` | Technology name |
| `dbu_per_um` | Database units per µm (1000 means 1 dbu = 1 nm) |
| `grid` | Snap grid in dbu |
| `layers` | Array of layers |
| `vias` | Array of via definitions |
| `drc_deck` | DRC deck text (optional; derived from the layer rules when absent) |
| `caps` | Per-layer parasitic coefficients (optional) |

Layer:

```json
{
  "name": "metal1", "gds_layer": 7, "gds_datatype": 0,
  "color": [80, 140, 255, 160], "fill": "hatch",
  "min_width": 230, "min_spacing": 230, "routing": true
}
```

- `fill`: `solid`, `hatch`, `backhatch`, `cross`, `dots` or `outline`.
- `min_width`, `min_spacing`: in dbu; 0 means not checked.
- `routing`: a routing layer carries net labels and is used by extraction and the router.

Via: `name`, `bottom`, `cut`, `top` (layer names), `cut_size`, `cut_spacing`,
`enc_bottom`, `enc_top` (dbu) and `resistance` (Ω per cut).

Parasitic coefficients (`caps`), one entry per layer: `layer`, `area` (aF/µm²),
`fringe` (aF/µm of edge), `coupling` (aF; adds `coupling * length / spacing` for a
parallel neighbour) and `resistance` (Ω per square).

## DRC rule deck

The deck is plain text, one statement per line. `#` starts a comment. Lengths are in µm,
areas in µm². Keywords and layer names are not case sensitive. Every rule can have an
optional `NAME:` prefix; otherwise a name such as `metal1.W` or `contact.EN.metal1` is
generated.

### Rules

| Statement | Checks |
|---|---|
| `width LAYER MIN` | Minimum width |
| `space LAYER MIN` | Minimum spacing between shapes of one layer |
| `separation A B MIN` | Minimum spacing between A and B (B pieces that do not overlap A) |
| `enclosure INNER OUTER MIN` | OUTER must enclose INNER by MIN (INNER pieces that overlap OUTER) |
| `extension A B MIN` | A must extend past B by MIN (e.g. poly endcap over active) |
| `area LAYER MIN` | Minimum area |
| `antenna METAL RATIO` | Maximum ratio of metal area to connected gate area (needs `poly` and `active` layers) |
| `density LAYER MIN MAX WINDOW [STEP]` | Density between MIN and MAX percent (plain numbers, 0 to 100) in a sliding WINDOW x WINDOW µm window (STEP defaults to WINDOW/2) |

### Derived layers

```text
NAME = A and B
NAME = A or B
NAME = A not B        # A minus B
NAME = A xor B
NAME = A size DIST    # grow (negative DIST shrinks)
NAME = A wide MIN     # the parts of A at least MIN wide
```

Derived layers can use technology layers and derived layers defined earlier, and can be
used in any rule.

### Example

```text
# poly and gates
width poly 0.18
space poly 0.24
gate = poly and active
GATE.EX: extension poly active 0.22
width gate 0.18

# metal1, with a larger spacing next to wide metal
width metal1 0.23
space metal1 0.23
m1w = metal1 wide 10
M1.WIDE.S: separation m1w metal1 0.6
area metal1 0.2

# contacts and vias
enclosure contact active 0.1
enclosure contact metal1 0.06
enclosure via1 metal1 0.06

antenna metal1 400
density metal1 20 80 100
```

When a technology has no `drc_deck`, IC Studio derives one from the layers' `min_width`
and `min_spacing`, the via enclosures, a poly-over-active extension and antenna rules.
Open **Rule deck** in the Layout workspace to see it, edit it, and **Save to technology**.
The command line can use a separate file: `ics drc <project> <cell> --deck rules.drc`.

## Importing a real PDK layer set

IC Studio reads KLayout layer properties (`.lyp`) and, optionally, the KLayout technology
file (`.lyt`):

- GUI: **File > Import > KLayout layer properties...**
- CLI: `ics import-tech myproj sky130A.lyp --lyt sky130A.lyt`

What is imported:

- Every layer entry with a concrete `layer/datatype` source becomes a layer. Its name
  comes from the entry's name (spaces become `_`, duplicates get `_2`, `_3`, ...).
- Colours come from the fill or frame colour; KLayout dither patterns map to the nearest
  fill style.
- Layers named like `poly`, `li`, `met1`, `metal2`, `m3` are marked as routing layers.
- From the `.lyt`: the technology name, the database unit and the default grid.

Not imported: width / spacing rules, vias, parasitic coefficients and a DRC deck. Add them
to `tech.json` (and the deck in the **Rule deck** editor) to use DRC, extraction and
routing with that technology.

Existing layouts are kept: shapes move to the new layer with the same GDS layer /
datatype (or the same name), layers in use that the file lacks are kept, and coordinates
are rescaled if the database unit changes.
