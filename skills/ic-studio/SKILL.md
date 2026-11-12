---
name: ic-studio
description: Design and verify full-custom ICs with IC Studio through its MCP server (ic-studio tools) or its `ics` CLI. It covers writing schematics, SPICE simulation, layout from a schematic, routing, DRC, LVS, parasitic extraction and GDS/OASIS. Use it when the user works on an IC Studio project (a folder with ic-studio.json), asks to design, simulate, lay out or verify a transistor-level circuit (inverter, NAND, OTA, current mirror, SRAM cell, ...), or mentions IC Studio, ics-mcp or the `ics` command.
---

# IC Studio

IC Studio is a full-custom IC design environment covering schematic, SPICE simulation,
layout, DRC, LVS and parasitic extraction. You can drive it in two ways:

- **MCP tools** (`ic-studio` server, `ics-mcp`). Prefer these when they are connected.
- **`ics` CLI**. Use it for batch runs, when MCP is not connected, and for the full JSON
  waveforms that the MCP `simulate` tool does not return. See [references/cli.md](references/cli.md).

Both work on the same project folders.

## Before anything else

1. Call `gui_status`.
   - **Live** means the desktop app is open. Every tool then runs on the project in the
     user's window, and the user sees edits, markers and waveforms there.
     A project is usually open already, so call `project_info` and work on it. Do not call
     `open_project` or `new_project` unless the user asks, because they replace what the
     user is looking at.
   - **Not live** means the server holds its own in-memory project. Start with
     `open_project` or `new_project` (template `demo` gives working example cells).
2. Call `project_info` to learn the cell, layer and via names and the technology. Do not
   guess layer names. They differ per PDK (`metal1` in generic180, `met1` in sky130A,
   `Metal1` in SG13G2).
3. Edits stay in memory until you call `save_project`. Save when a step is done and
   verified, and tell the user.

## Units (the most common source of errors)

| Where | Unit | Axis |
|---|---|---|
| MCP layout tools (`add_rect`, `place_pcell`, markers, bboxes) | µm | y up |
| `layout.json` on disk | dbu (1 dbu = 1 nm in all bundled technologies) | y up |
| `schematic.json` | integer grid points | **y down** |
| SPICE parameters (`w`, `l`, `c`, ...) | SPICE suffixes: `2u`, `180n`, `20f` | n/a |

Layout coordinates snap to the technology grid (10 nm in generic180, 5 nm in sky130A).

## The standard flow

Follow these steps in order and check each one before moving on.

1. **Schematic.** MCP has no tool to edit schematics. Use an existing cell, ask the user to
   draw it in the app, or write `schematic.json` yourself. See
   [references/schematic-json.md](references/schematic-json.md). Net labels placed on pin
   positions are enough; you do not need wires.
2. **Check the netlist.** `get_netlist`. Read it to confirm that every device connects to
   the nets you intended. This is the cheapest check and catches most schematic mistakes.
3. **Simulate.** `simulate` runs the analyses saved in the cell's `sim.json`. Check:
   - **OP**: every transistor is in the intended region (`saturation` for analog bias,
     `triode`/`cutoff` at logic levels), and node voltages are plausible.
   - **Sweeps**: the summary gives min/max/first/final per vector. For full waveforms use
     `ics sim <proj> <cell> --json`.
   - For an ad-hoc deck that is not a cell, use `run_spice`.
4. **Generate the layout.** `generate_layout` places every MOS as a PCell named after its
   schematic device. PMOS go in a row above the NMOS, with taps and wells. Differential
   pairs and current mirrors are detected and placed common-centroid (ABBA) with dummies.
   R, C and sources are skipped; that is expected.
5. **Route.** `route`, then `flylines`. An empty flyline list means every connection is
   made. For nets that are left, the reason is given. Fix them by hand (`add_path` on
   metal, plus vias) and route again.
6. **DRC.** `run_drc`. Violations are grouped by rule, with marker boxes in µm. Fix them
   and rerun until the count is 0.
7. **LVS.** `run_lvs`. The goal is "layout matches schematic". See the LVS section below.
8. **Post-layout.** `post_layout_parasitics` shows the largest net capacitances.
   `simulate` with `parasitics: true` reruns with them. Compare against step 3 (delay,
   gain, bandwidth) and report the difference.
9. `save_project`, and `export_gds` if the user wants a GDS or OASIS file.

If the CLI is available, steps 4 to 7 collapse to:

```sh
ics route <proj> <cell> --place && ics drc <proj> <cell> && ics lvs <proj> <cell>
```

`ics route` **saves the project**. Each command exits with 1 on unrouted nets, violations
or differences.

## Reading LVS results

- `[missing] C1 (capacitor): not extracted`: only MOS transistors are extracted. A
  schematic R, C or diode always shows up here. This is expected, not a layout bug.
- `[param] W 2µm vs 1.2µm`: the layout device size differs. Delete the instance and
  regenerate it, or place a PCell with the right `w`/`l`/`fingers`.
- `[net]` difference: a short or an open. Use the marker location, `extract` and
  `layout_summary` to find where two nets touch or where a connection is missing.
- Parallel fingers are merged and `m` multiplies W, so a multi-finger layout matches a
  single schematic device. Bulk terminals are ignored. Dummies (gate tied to bulk) are
  dropped.
- Net names compare case-insensitively, and `0`, `gnd` and `vss` are the same net. A label
  on metal in the layout anchors a net name. Add pin labels (`add_label` on a routing
  layer) when LVS pairs the wrong nets.

## Fixing DRC violations

- Spacing or width on metal: move or widen the shape. Get ids from
  `layout_summary list_shapes=true`, use `delete_items`, then redraw it.
- Rules come from the technology deck. Do not "fix" a violation by editing the deck
  unless the user asks.
- Non-Manhattan shapes are skipped and counted, not checked.

## Hand layout

- `place_pcell`: `nmos`/`pmos` with `w` (per finger), `l`, `fingers` and an optional PDK
  `device` (e.g. a low-Vt model). Also `poly_res` and `mom_cap`. Name the instance after the
  schematic device (`name: "M1"`) so flylines, LVS and cross-probing link them.
- `place_instance` places another cell that has a layout. Use `rot`/`mirror` for
  orientation.
- `add_path` draws a centre-line wire with a width. `add_rect` draws boxes and via cuts.
  Look up via names and layers in `project_info`.
- `add_label` on a routing layer names the net under it.

## Technology and PDKs

- New projects use `generic180`, a generic 180 nm process with built-in level-1 models.
  These are good for learning and for relative comparisons, not for real numbers.
- `load_pdk` (or `ics load-pdk`) switches to a PDK file such as `pdks/sky130A.toml` or
  `pdks/ihp-sg13g2.toml`. PDK SPICE models come from an installed PDK through `$PDK_ROOT`,
  and simulating them usually needs `backend: "ngspice"`.
- `ngspice` is optional. If a run fails with "program not found", use the built-in backend
  and tell the user.
- `run_signoff` runs the PDK's external KLayout, Calibre or netgen runsets, if any.

## Live mode and files on disk

The app does not watch files. If you edit `schematic.json` or `sim.json` on disk while the
app has the project open, call `save_project` first if there are unsaved changes, then
`open_project` on the same folder to reload. When you write files, do it before the app
saves again, or the app overwrites them.

## Reporting to the user

Report results as numbers, not adjectives. For example: DRC 0 violations; LVS match;
inverter t_pd 38 ps pre-layout and 45 ps post-layout; OTA DC gain 32 dB. Name anything you
skipped and why, for example "ngspice not installed, used the built-in engine".
