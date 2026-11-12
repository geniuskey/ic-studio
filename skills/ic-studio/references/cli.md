# `ics` CLI

`ics` works on the same project folders as the app and the MCP server. You can build it
with `cargo build --release -p ics-cli` (binary `target/release/ics`).

`<CELL>` is `lib/cell` or a bare cell name. On error a command prints `error: ...` and
exits with 1. Checks also exit with 1 on violations, differences or unrouted nets, so
commands can be chained with `&&`.

| Command | Purpose |
|---|---|
| `ics new <DIR> [--template empty\|demo]` | Create a project. The default is `demo` |
| `ics info <PROJECT>` | Libraries, cells and views |
| `ics netlist <PROJECT> <CELL>` | SPICE deck of the schematic, with its analyses |
| `ics sim <PROJECT> <CELL> [--backend builtin\|ngspice] [--json] [--parasitics]` | Simulate with the saved setup |
| `ics spice <FILE> [--backend ...] [--json]` | Simulate a raw SPICE file |
| `ics route <PROJECT> <CELL> [--place]` | Place missing devices (with `--place`), route, **save** |
| `ics drc <PROJECT> <CELL> [--deck FILE]` | DRC. The deck may be SVRF |
| `ics extract <PROJECT> <CELL>` | Extracted netlist as SPICE |
| `ics lvs <PROJECT> <CELL>` | Compare layout and schematic |
| `ics signoff <PROJECT> <CELL> [RUNSET]` | External KLayout, Calibre or netgen runset. Without RUNSET, lists them |
| `ics gds <PROJECT> <CELL> -o <FILE>` | Export a cell and its hierarchy to GDSII |
| `ics import-gds <PROJECT> <FILE> [--lib NAME] [--skip-unknown-layers]` | Import GDS as a library |
| `ics gds-info <FILE>` | Hierarchy statistics of a GDS file |
| `ics load-pdk <PROJECT> <PDK>` | Replace the technology with a PDK `.toml` |
| `ics import-tech <PROJECT> <LYP> [--lyt FILE]` | Layers from a KLayout `.lyp` |

## Full waveforms

```sh
ics sim proj ota5t --json > ota.json
```

The output looks like this:

```json
{"backend": "built-in", "elapsed_ms": 3.1,
 "datasets": [{"analysis": "tran", "title": "...",
               "vectors": {"time": [...], "v(out)": [...]}}]}
```

AC vectors are `{"re": [...], "im": [...]}`. Compute delay, gain or bandwidth from these
arrays with a short script instead of reading values by eye.

## Typical runs

```sh
ics new demo --template demo
ics sim demo cmos_inverter
ics route demo nand2 --place && ics drc demo nand2 && ics lvs demo nand2
ics sim demo ring_osc5 --parasitics
ics gds demo nand2 -o nand2.gds
```

On Windows PowerShell, call the binary as `.\target\release\ics.exe`.
