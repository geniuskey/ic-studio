# Layout editor

Open a cell's layout view (or press `Ctrl+4` with a cell open). Layout coordinates are
database units (1 dbu = 1 nm in the generic technology, y pointing up); the inspector shows
µm. Every point snaps to the technology grid.

![Layout editor](screenshots/06_layout.png)

- **Left panel**: **Hierarchy** (drawn levels and the tree of placed cells), **Layers**,
  and the options of the current tool.
- **Toolbar**: undo / redo, the tools (**Select**, **Rectangle**, **Polygon**, **Path**,
  **Label**, **Via**, **Instance**, **Ruler**), fit, the **Grid** and **Flylines**
  checkboxes, and **GDS** to export the cell.
- **Right panel**: cell statistics or the inspector, **Edit** operations, **Layer
  operations**, and the verification sections (see [Verification](verification.md)).

## Layers

- Click a layer row to make it the **active layer**; new shapes are drawn on it.
- Click the eye to show or hide a layer; **all** and **none** switch every layer. Hidden
  layers cannot be selected.
- Each row shows the fill pattern, the name and the GDS `layer/datatype`. Hover a row to
  see its minimum width and spacing.

## Drawing

Keys act on the canvas only while the mouse is over it.

| Tool | Key | How to use |
|---|---|---|
| Select | `S` | Click, `Shift`/`Ctrl`+click, or drag a box |
| Rectangle | `R` | Drag from corner to corner; the size is shown while dragging |
| Polygon | `G` | Click each vertex; `Enter`, double-click or right-click closes it |
| Path | `P` | Click points; `Enter`, double-click or right-click finishes it |
| Label | `L` | Type the **Text** in the left panel, click to place |
| Via | `V` | Pick a via in the left panel; click for one cut, drag to fill an area |
| Instance | `I` | Pick a cell or PCell in the left panel, click to place |
| Ruler | `K` | Drag to measure distance and dx / dy |

While drawing a polygon or path, `Backspace` removes the last point and `Esc` cancels.

- **Polygons** are Manhattan by default: each point is aligned horizontally or vertically
  to the previous one. Hold `Shift` (or `Ctrl`) while clicking for any angle. The closing
  edge gets an extra corner so a Manhattan outline stays Manhattan.
- **Paths** are always Manhattan. **Width (nm)** in the left panel sets the width; `0`
  uses the layer's minimum width. Path ends are extended by half the width.
- **Labels** on a routing layer name the net under them (used by extraction and LVS).
- **Vias** come from the technology (bottom layer, cut, top layer); dragging an area
  fills it with as many cuts as fit.

### Rectangle step by step

1. Click the `metal1` row in **Layers**.
2. Press `R` and drag on the canvas.
3. Press `S` (or `Esc`) to return to Select, click the rectangle and type exact
   coordinates in the inspector (**x0**, **y0**, **x1**, **y1** in µm).

## Selecting and editing

| Action | How |
|---|---|
| Select | Click; the smallest shape under the cursor wins |
| Add / remove | `Shift`+click or `Ctrl`+click |
| Box select | Drag on empty space (only items fully inside; `Shift` adds) |
| Select all | `Ctrl+A` |
| Move | Drag a selected item, or nudge with the arrow keys (10 grid steps, 100 with `Shift`) |
| Reshape | With one shape selected, drag a vertex handle (square) or an edge handle (round); Manhattan shapes stay Manhattan |
| Rotate 90° / flip | `Shift+R` / `Shift+X` / `Shift+Y`, or **Rotate 90°**, **Flip X**, **Flip Y** |
| Duplicate | `Ctrl+D` |
| Delete | `Del` or `Backspace` |
| Undo / redo | `Ctrl+Z` / `Ctrl+Shift+Z` or `Ctrl+Y` |
| Open a placed cell | Double-click the instance, or **Open** in the inspector |

The **Edit** section of the right panel (shown when something is selected) also has:

- **Flatten**: replace the selected instances with their contents (one level).
- **Make cell**: move the selection into a new cell and place an instance of it.
- **Merge**: union the selected shapes per layer.
- **Size** with **Apply**: grow (positive µm) or shrink (negative) the selected shapes.

## Layer operations

With nothing selected, **Layer operations** computes Boolean results:

1. Choose layer **A**, an **Operation** (AND, OR, NOT, XOR, SIZE) and layer **B** (or a
   distance **By** in µm for SIZE).
2. Choose the **Output** layer.
3. Keep **Include hierarchy (flattened)** checked to include placed cells.
4. Press **Run**. The result shapes are added to the output layer and selected.

Boolean operations work on Manhattan geometry; other shapes are skipped and reported.

## Hierarchy

![Hierarchy](screenshots/08_layout_hierarchy.png)

### Placing a cell or an array

1. Press `I`. The left panel lists **PCell** variants and **Place cell** (every other
   cell with a layout that would not create a cycle).
2. Pick a cell, set **Rotation**, **Mirror about x**, **Columns** and **Rows**. Array
   elements abut at the cell's extent.
3. Click to place.

Select an instance to edit its position, rotation and mirroring in the inspector. Check
**Array** to turn it into an array (GDS AREF) with **Columns**, **Rows**, **Column pitch**
and **Row pitch**.

### Drawn depth

The **levels** field in **Hierarchy** (keys `0` to `8`, and `9` for all levels) sets how
many levels of placed cells are drawn with their contents. Deeper instances are drawn as
labelled boxes. Missing cells are drawn in red.

Large designs stay fast: cells that are small on screen collapse to boxes and tiny shapes
to dots.

![8192 inverters](screenshots/09_layout_tiles.png)

The hierarchy tree lists the cells placed in the current cell with their counts; click
one to open it.

## PCells

![PCells](screenshots/10_layout_pcells.png)

Parametric cells are generated from the technology and stored as ordinary cells in the
`pcells` library (for example `nmos_w1000_l180_nf2`).

| PCell | Parameters | Defaults |
|---|---|---|
| `nmos` | W, L, fingers, device | W 1 µm, L 0.18 µm, 1 finger |
| `pmos` | W, L, fingers, device | W 2 µm, L 0.18 µm, 1 finger |
| `poly_res` | W, L | W 0.4 µm, L 5 µm |
| `mom_cap` | W, H | 5 µm x 5 µm |

**Device** appears when the technology is a PDK with several MOS devices of that polarity
(for example low- and high-Vt variants): the PCell then draws that device's marker layers,
and the variant name ends with the device name. The default is the PDK's first device.

To place one, press `I`, choose the PCell in the left panel, set its parameters and click.
To change a placed PCell, select it, edit the parameters in the **PCell** section of the
inspector and press **Apply**. Well and substrate taps (`ntap`, `ptap`) are created by
[schematic-driven layout](schematic-driven-layout.md); their parameters can be edited in
the inspector the same way.

## GDSII

- **File > Import > GDSII...** reads a GDS file into a new library with full hierarchy.
- **GDS** in the toolbar or **File > Export > Cell GDSII...** writes the cell and every
  cell it places; **Export > Library GDSII...** writes every layout in the project.

Layers are mapped by GDS layer / datatype. Layers the technology does not have are added
to it on import, so nothing is lost.
