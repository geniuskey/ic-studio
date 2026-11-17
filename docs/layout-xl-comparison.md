<!-- markdownlint-disable MD013 -->

# Compared with Virtuoso Layout XL

How the Layout XL (connectivity-driven layout) features of Cadence Virtuoso map onto IC
Studio, as of v0.2.8 plus the changes listed here. **Yes** means the workflow is covered,
**Partial** that a narrower version exists, **No** that it is missing.

## Connectivity and the schematic

| Layout XL | IC Studio | Status |
|---|---|---|
| Generate All From Source (devices) | **Verify > Generate Layout from Schematic**: rows, shared diffusion, taps, matched groups | Yes |
| Generate pins from source | Generate Layout and **Update Components and Nets** place a pin shape and label on the cell boundary per schematic pin (inputs left, outputs right, inouts top); flylines and the router connect them | Yes |
| Update Components and Nets (ECO) | Places new devices, resizes changed ones in place, deletes extra ones (`ics eco --update`) | Yes |
| Check Against Source | Missing, extra and changed devices, missing pins, shorts, opens; Live re-check after every edit | Yes |
| Flight lines / incomplete nets | Flylines from the schematic connectivity | Yes |
| Mark Net, probing | **Mark Net** (`Shift+N`), net highlighting from extraction | Yes |
| Cross-selection with the schematic | Cross-probing of devices both ways | Yes |
| Connectivity-aware Create Wire (net of the start pin, live short markers) | The Path tool takes the net of the conductor it starts on and reports and marks any short the finished wire makes | Yes |
| Short locator / annotation browser | Shorts with their location, LVS and DRC markers listed and zoomable in the Verify panel | Yes |

## Placement

| Layout XL | IC Studio | Status |
|---|---|---|
| Placer (automatic placement) | Schematic-driven rows, compaction, layout by example | Yes |
| Modgen (matched arrays) | Common-centroid / interdigitated units with dummies, from differential pairs, mirrors and `match` | Yes |
| Abutment | **Assist > Abut MOS Devices** and auto-abut on move, net-checked with automatic flip | Yes |
| Constraint Manager | Symmetry, alignment and fixed constraints stored with the layout, inferred from the schematic; the Constraints list flags violated ones, selects their items, snaps them back or removes them | Yes |
| Align / distribute | Align edges and centres, equal spacing | Yes |

## Editing

| Layout XL / L | IC Studio | Status |
|---|---|---|
| Create Rectangle, Polygon, Path, Label, Via, Instance, Ruler | Same commands and Virtuoso bindkeys | Yes |
| Create Wire with via on layer change | `+` / `-` in the Path tool | Yes |
| Bus / multiple wires | **Wires** and **Pitch** in the Path options, a via per wire | Yes |
| Width spacing patterns, track patterns | **Shift+E > Tracks**: uniform or WSP tracks, snapping, conformance check | Yes |
| Stretch, partial select, chop, split, merge, chamfer, convert to polygon | Same | Yes |
| Copy with rows / columns, move by delta | Copy array and typed offset in the Move / Copy options | Yes |
| Search and replace | **Edit > Search**: by kind, layer, name, size; select, zoom, change layer, delete | Yes |
| PCell stretch handles, parameter edit | Stretch edits W / L / H; properties edit all parameters | Yes |
| Multi-part paths, fluid guard rings | Guard ring PCells: rectangular (`pring` / `nring`) and along any Manhattan centre line (`pring_path` / `nring_path`, from a drawn path, polygon or rectangle), stretched side by side; DRC clean by construction | Yes |
| Edit in place, descend, hierarchy tree | Same | Yes |

## Design assistance

| Layout XL / EAD | IC Studio | Status |
|---|---|---|
| Design-rule-driven editing (DRD) | DRD while drawing, live DRC, spacing magnet | Yes |
| DRC quick fixes | Quick Fix (`Ctrl+.`) and Fix All, each fix verified with DRC | Yes (Layout XL has no equivalent) |
| Electrically aware design: R, C, EM while drawing | Per wire: length, R, C and EM limit next to the cursor, EM-driven width. Per net: C to substrate, coupling, worst terminal-to-terminal R and R x C of the highlighted net | Yes |
| Post-layout parasitics into simulation | Wire R / C, coupling, junctions back-annotated | Yes |
| IR drop / EM analysis | **Run IR / EM** in the Verify panel | Yes |

## Data and flows

| Virtuoso | IC Studio | Status |
|---|---|---|
| OpenAccess database | JSON per cell, GDSII / OASIS / LEF / DEF; Virtuoso libraries through a SKILL dump | Partial (no direct OA access) |
| SKILL | A SKILL interpreter for layout scripts with a CIW: the language core, `db*` shapes, instances, parametric instances, vias, nets, pins, properties, layer Booleans, ROD shapes, files | Yes for layout scripting: `pcDefinePCell`, ROD multipart paths, SKILL++ closures, `hi*` forms as dialogs |
| Calibre / PVS sign-off | Calibre, KLayout and netgen runs with results on the canvas; SVRF translated to the built-in DRC | Yes |

## Still missing

- Direct OpenAccess access: reading OA needs the Si2 OpenAccess libraries; Virtuoso
  libraries come in through a SKILL dump or GDSII / OASIS instead.
