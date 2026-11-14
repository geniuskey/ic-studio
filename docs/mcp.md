# MCP server for AI agents

`ics mcp` (the `mcp` command of the `ics` command line tool) exposes IC Studio to AI agents
through the Model Context Protocol (stdio). An agent can create and open projects, read
netlists, simulate, edit layouts, generate and route layout from a schematic, and run DRC,
extraction and LVS.

## Setup

The [release installer](install.md#install-a-release) registers the server with Claude
Code when `claude` is on the `PATH`. In the app, **File > Settings... > AI agents (MCP)**
has **Add to Claude Code**, which does the same, and **Copy MCP config**, which copies the JSON
below for other clients.

To register it by hand:

```sh
claude mcp add --scope user ic-studio -- /path/to/ics mcp
```

For other MCP clients, add a stdio server whose command is the `ics` binary with the
argument `mcp`:

```json
{ "mcpServers": { "ic-studio": { "command": "/path/to/ics", "args": ["mcp"] } } }
```

From source, build it with `cargo build --release -p ics-cli` and use
`target/release/ics`.

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

The server has 34 tools:

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
| `place_instance`, `place_pcell` | Place a cell or a [PCell](pcells.md) (`nmos`, `pmos` with an optional PDK `device`, `poly_res`, `mom_cap`, `ntap`, `ptap`) |
| `delete_items` | Delete shapes / instances by id |
| `generate_layout` | Schematic-driven placement with matching (no wiring) |
| `flylines` | List unrouted connections |
| `route` | Route the flylines on the metal layers |
| `run_drc` | DRC with the technology deck |
| `fix_drc` | Apply verified quick fixes to the cell's top-level geometry until nothing more can be fixed |
| `compact` | Slide top-level items toward each direction until the rules stop them; constraints hold |
| `optimize` | Size device parameters to meet specs (optionally after layout); `apply` writes the best values |
| `place_like` | Layout by example: place a cell like a finished cell with the same circuit topology, optionally route |
| `diff_layout` | XOR a cell's layout per layer against a git revision (default `HEAD`) or the saved files |
| `extract` | Extracted nets and devices, plus SPICE |
| `run_lvs` | Compare layout and schematic |
| `run_signoff` | Run an external KLayout / Calibre / netgen runset of the technology |
| `post_layout_parasitics` | Extracted parasitics and the largest net and coupling capacitances |
| `emir` | IR drop and electromigration of a net: worst drop and where, loads, highest current densities, EM violations |
| `opc` | Optical proximity correction of a layer: EPE statistics without and with correction, mask area, worst fragments; `apply` writes the mask into the cell |
| `load_pdk` | Replace the technology with a PDK file |
| `export_gds`, `import_gds` | GDSII / OASIS export and import (import skips layers the technology lacks); both apply the technology's stream layer map |
| `run_skill` | Run Cadence SKILL code (`code`) or a `.il` file (`file`); returns `output`, `value` and the `modified` cells |
| `gui_status` | Whether a desktop app is connected, and its project |

### run_skill

`run_skill` takes either `code` (SKILL source) or `file` (a `.il` path, also the base of
relative `load()` paths) and returns the following. The optional `cell` names the cell that
`geGetEditCellView()` returns.

```json
{ "output": "what the script printed", "value": "value of the last form", "modified": ["work/pads"] }
```

On a SKILL error the tool fails with `SKILL error: <file or line>: <message>`, followed by
the cells already modified (their edits are kept) and the output printed so far. In live
mode the app records undo history for every changed layout and opens the first one when no
cell is open.

## Live mode

While the desktop app is running, the MCP server (`ics mcp`) forwards every tool call to
it, and the agent works on the project open in the window:

- edits appear immediately and layout edits can be undone with `Ctrl+Z`,
- the window opens the affected cell in the matching workspace; `simulate` shows the
  waveforms, and DRC / extraction / LVS results appear in the layout panels with markers,
- the status bar shows **MCP live**, and each call is logged in the console as
  `MCP · <tool> <cell> -> <summary>`.

The app listens on 127.0.0.1 with a per-session token and writes the discovery file
`live.json` to `%LOCALAPPDATA%\ic-studio` (Windows) or `$XDG_RUNTIME_DIR/ic-studio`, falling
back to `~/.local/state/ic-studio` (Linux, macOS). `ics mcp` checks for a running app on
every call, so you can start or close the app at any time.

Without a running app, or with `ICS_MCP_STANDALONE=1`, the server works on its own
in-memory project; start with `open_project` or `new_project`. `ICS_LIVE_DIR` overrides
the folder of the discovery file.
