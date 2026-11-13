# Keyboard shortcuts

`Ctrl` on Windows and Linux is `Cmd` on macOS. Press `F1` in the app to show this list.
Canvas keys work while the mouse is over the canvas and no text field has focus.

## General

| Keys | Action |
|---|---|
| `Ctrl+K`, `Ctrl+Shift+P` | Command palette |
| `Ctrl+S`, `F2` / `Ctrl+Shift+S` | Save / save as |
| `Ctrl+O` | Open project |
| `Ctrl+N` | New cell |
| `Ctrl+W` | Close tab |
| `Ctrl+Z` | Undo |
| `Ctrl+Shift+Z`, `Ctrl+Y` | Redo |
| `Ctrl+1` ... `Ctrl+4` | Home, Schematic, Simulation, Layout workspace |
| `F5`, `Ctrl+Enter` | Run simulation |
| `Ctrl+J` | Toggle console |
| `Ctrl+,` | Settings |
| `F1` | Keyboard shortcuts |

## Canvas (schematic and layout)

| Keys | Action |
|---|---|
| Wheel | Pan up / down (Conventional profile: zoom at the cursor) |
| `Ctrl`+wheel, pinch | Zoom at the cursor |
| `Shift`+wheel | Pan horizontally |
| Middle drag | Pan |
| Right drag | Layout: zoom into the box (Conventional profile: pan); schematic: pan |

The mouse follows the **Interaction** profile in **Settings** (`Ctrl+,`). **Virtuoso**
(the default) matches the Virtuoso defaults: the wheel pans, `Ctrl`+wheel zooms at the
mouse and a right drag zooms into a box. **Conventional** zooms with the wheel and pans with
a right drag. Bindkeys are the same in both.

| `F` | Fit to window |
| `Esc` | Cancel the current operation, then back to Select, then clear the selection |
| `Del`, `Backspace` | Delete the selection |
| `Ctrl+A` | Select all |
| `Ctrl+D` | Duplicate the selection (layout: deselect all) |
| `Shift`/`Ctrl`+click | Add to or remove from the selection |

## Schematic

| Keys | Action |
|---|---|
| `W` | Wire |
| `T` | Net label |
| `R` `C` `L` | Resistor, capacitor, inductor |
| `V` `I` | Voltage source, current source |
| `D` | Diode |
| `N` `P` | NMOS, PMOS |
| `G` `S` | Ground, supply |
| `Space` / `X` | Rotate / mirror (while placing, or the selection) |
| `/` | Swap the wire bend direction |

## Layout

Layout keys follow the Cadence Virtuoso (IC6) default bindkeys.

| Keys | Action |
|---|---|
| `Esc` | Cancel the pending point, clear a partial selection, then return to Select |
| `S` | Stretch: click an edge or vertex, then the destination; or drag a box over edges (or use a partial selection), then click the reference point and the destination |
| `F4` | Toggle partial selection (select edges and vertices) |
| `M` / `C` | Move / copy: click the reference point, then the destination (copy repeats) |
| Right-click / `Shift`+right-click | While moving or copying: rotate 90° / flip sideways |
| Middle-click, right-click | Pop-up menu (right-click only when no command point is pending) |
| Right drag | Zoom into the box |
| `q` | Edit Object Properties form |
| `F3` | Options form of the current command |
| `e` / `Shift+E` | Display Options / Layout Editor Options form |
| `R` | Rectangle (drag or two clicks) |
| `Shift+P` / `P` | Polygon / path |
| `L` / `O` / `I` / `K` | Label / via / instance / ruler |
| `Shift+C` / `Shift+M` | Chop / merge |
| `Shift+O` | Rotate the selection 90° (or what is being moved) |
| `+` / `-` while drawing a path | Drop a via and continue on the metal above / below |
| `u` / `Shift+U` | Undo / redo (`Ctrl+Z` / `Ctrl+Y` also work) |
| `y` / `Shift+Y` | Yank / paste (`Ctrl+C` / `Ctrl+X` / `Ctrl+V` also work) |
| `T` | Layer tap: make the layer under the cursor active |
| `G` | Toggle gravity snapping |
| `Ctrl+A` / `Ctrl+D` | Select all / deselect all |
| `Shift+X` / `Shift+B` | Edit the instance under the cursor in place / return |
| `Shift+N` | Mark net mode (click a shape to highlight its net after **Extract**) |
| `Enter` | Finish a path or polygon |
| `Backspace` | Remove the last point while drawing |
| `Shift`+click | Any-angle polygon point |
| Arrow keys | Pan |
| `Shift`+arrow keys | Nudge the selection 10 snap steps |
| `Z` / `Shift+Z` | Zoom box / zoom out |
| `[` / `]` | Zoom out / in at the cursor |
| `W` / `Shift+W` | Previous / next view |
| `F` | Fit |
| `Shift+K` | Clear all rulers |
| `Shift+F` / `Ctrl+F` | All levels / top level only |
| `0` ... `8`, `9` | Hierarchy levels drawn (`9` = all) |
| Click the same spot again | Cycle through overlapping items |
| Right-to-left box | Select everything the box touches (left-to-right: only what it encloses) |
| Double-click instance | Open its cell |

`Ctrl+Z` stays undo, as in the rest of the application (Virtuoso binds it to zoom in).

## Simulation plots

Scroll to zoom, drag to pan, double-click to reset the view. Hover a trace to read its
value.
