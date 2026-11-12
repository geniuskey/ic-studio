# MCP server for AI agents

`ics-mcp` exposes IC Studio to AI agents through the Model Context Protocol (stdio). An
agent can create and open projects, read netlists, simulate, edit layouts, generate and
route layout from a schematic, and run DRC, extraction and LVS.

## Setup

```sh
cargo build --release -p ics-mcp
claude mcp add ic-studio -- /path/to/ic-studio/target/release/ics-mcp
```

For other MCP clients, add a stdio server whose command is the `ics-mcp` binary, e.g.:

```json
{ "mcpServers": { "ic-studio": { "command": "/path/to/ic-studio/target/release/ics-mcp" } } }
```

Then ask, for example: "create a demo project in ./demo and run LVS on cmos_inverter".

## Agent skill

The tools tell an agent what it can do; the `ic-studio` skill in `skills/ic-studio` tells
it how to design with them: the schematic → simulation → layout → DRC → LVS → post-layout
flow, units, how to read LVS / DRC results, the `schematic.json` format and the `ics` CLI
as a fallback. Install it with:

```sh
npx skills add geniuskey/ic-studio
```

or copy `skills/ic-studio` into `~/.claude/skills/`.

## Conventions

- Coordinates are in **µm**, y pointing up, snapped to the technology grid.
- A `cell` argument is a cell name or `lib/cell`. Layout edit tools create the cell (in
  library `work`) and its layout view if they do not exist.
- Edits stay in memory until `save_project`.

## Tools

| Tool | Purpose |
|---|---|
| `open_project`, `new_project`, `save_project`, `project_info` | Project management; `project_info` lists the technology layers, vias and all cells |
| `get_netlist` | SPICE deck of a cell |
| `write_schematic` | Create or replace a cell's schematic from a SPICE netlist; its analysis lines set the simulation setup |
| `set_simulation` | Change a cell's analyses, backend, temperature, extra lines, parasitics or corner |
| `simulate` | Run the cell's saved analyses (optional `backend`, `parasitics`); returns a compact summary |
| `run_spice` | Simulate a raw SPICE deck |
| `layout_summary` | Bounding box, shapes per layer, labels and instances |
| `add_rect`, `add_path`, `add_label` | Draw shapes; a label on a routing layer names the net under it |
| `place_instance`, `place_pcell` | Place a cell or a PCell (`nmos`, `pmos` with an optional PDK `device`, `poly_res`, `mom_cap`) |
| `delete_items` | Delete shapes / instances by id |
| `generate_layout` | Schematic-driven placement with matching (no wiring) |
| `flylines` | List unrouted connections |
| `route` | Route the flylines on the metal layers |
| `run_drc` | DRC with the technology deck |
| `extract` | Extracted nets and devices, plus SPICE |
| `run_lvs` | Compare layout and schematic |
| `run_signoff` | Run an external KLayout / Calibre / netgen runset of the technology |
| `post_layout_parasitics` | Extracted parasitics and the largest net and coupling capacitances |
| `load_pdk` | Replace the technology with a PDK file |
| `export_gds`, `import_gds` | GDSII export and import (import skips layers the technology lacks) |
| `gui_status` | Whether a desktop app is connected, and its project |

## Live mode

While the desktop app is running, `ics-mcp` forwards every tool call to it, and the agent
works on the project open in the window:

- edits appear immediately and layout edits can be undone with `Ctrl+Z`,
- the window opens the affected cell in the matching workspace; `simulate` shows the
  waveforms, and DRC / extraction / LVS results appear in the layout panels with markers,
- the status bar shows **MCP live**, and each call is logged in the console as
  `MCP · <tool> <cell> -> <summary>`.

The app listens on 127.0.0.1 with a per-session token and writes the discovery file
`live.json` to `%LOCALAPPDATA%\ic-studio` (Windows) or `$XDG_RUNTIME_DIR/ic-studio`, falling
back to `~/.local/state/ic-studio` (Linux, macOS). `ics-mcp` checks for a running app on
every call, so you can start or close the app at any time.

Without a running app, or with `ICS_MCP_STANDALONE=1`, the server works on its own
in-memory project; start with `open_project` or `new_project`. `ICS_LIVE_DIR` overrides
the folder of the discovery file.
