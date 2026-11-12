# Command line (`ics`)

`ics` works on the same project folders as the desktop app. Use it for scripting, batch
runs and CI. Build it with `cargo build --release -p ics-cli` (binary
`target/release/ics`) or run it with `cargo run -p ics-cli -- <args>`.

- `<CELL>` is `lib/cell` or just the cell name (the first cell with that name is used).
- Exit code 0 means success. Errors print `error: <message>` to stderr and exit with 1.
- `ics --help` and `ics <command> --help` describe every option.

## Commands

| Command | Purpose |
|---|---|
| `ics new <DIR> [--template empty\|demo]` | Create a project (default template `demo`) |
| `ics info <PROJECT>` | List libraries, cells and views |
| `ics netlist <PROJECT> <CELL>` | Print the cell's SPICE deck |
| `ics schematic <PROJECT> <CELL> <NETLIST> [--overwrite]` | Create or replace a cell's schematic from a SPICE netlist file and save |
| `ics sim <PROJECT> <CELL> [--backend builtin\|ngspice] [--json] [--parasitics]` | Simulate with the cell's saved setup |
| `ics spice <FILE> [--backend builtin\|ngspice] [--json]` | Simulate a SPICE file with its own analyses |
| `ics gds <PROJECT> <CELL> -o <FILE>` | Export the cell and every cell it places to GDSII |
| `ics gds-info <FILE>` | Print hierarchy statistics and timings of a GDS file |
| `ics import-gds <PROJECT> <FILE> [--lib NAME] [--skip-unknown-layers] [--json]` | Import a GDS file as a new library |
| `ics lib-storage <PROJECT> <LIB> gds\|json` | Change how a library is stored on disk |
| `ics import-tech <PROJECT> <LYP> [--lyt FILE]` | Replace the technology's layers from a KLayout `.lyp` |
| `ics load-pdk <PROJECT> <PDK>` | Replace the technology with a PDK file (see Technology) |
| `ics drc <PROJECT> <CELL> [--deck FILE]` | Run DRC (the deck may be SVRF); exit 1 on violations |
| `ics extract <PROJECT> <CELL>` | Print the extracted netlist as SPICE |
| `ics lvs <PROJECT> <CELL>` | Compare layout and schematic; exit 1 on differences |
| `ics signoff <PROJECT> <CELL> [RUNSET]` | Run an external KLayout / Calibre / netgen runset (list them without RUNSET); exit 1 on violations or a mismatch |
| `ics route <PROJECT> <CELL> [--place]` | Route flylines and save; exit 1 if nets stay unrouted |

## Details

### Schematic from a netlist

`ics schematic` reads a flat SPICE netlist (R, C, L, V, I, D and M lines, and X lines for
PDK MOS subcircuits) and draws it as the cell's schematic: every pin gets a net label
(node `0` or `gnd` gets a ground symbol) and devices are laid out in rows (PMOS, NMOS,
passives, sources). Analysis lines (`.op`, `.dc`, `.ac dec`, `.tran`) and `.temp` set the
cell's simulation setup; other dot lines (`.model`, `.include`, `.param`, `.ic`, `.meas`)
become its extra lines. Lengths are SPICE metres (`w=2u`); a bare `w=2` is read as µm with
a warning. The cell is created in library `work` unless `lib/cell` is given. It prints the
generated deck and saves the project.

```sh
ics schematic myproj ota ota.cir && ics route myproj ota --place && ics lvs myproj ota
```

### Simulation

`ics sim` uses the analyses, temperature, extra lines and backend saved for the cell in the
app. `--backend` overrides the backend; `--parasitics` adds layout parasitics (also enabled
when the saved setup has them on). Without `--json`, it prints each dataset's title and
point count, operating-point values and device regions, and the vector names of sweeps.

With `--json`, the full results go to stdout:

```json
{
  "backend": "built-in",
  "elapsed_ms": 3.1,
  "datasets": [
    { "analysis": "tran", "title": "...", "vectors": { "time": [0, 1e-11], "v(out)": [0, 0.01] } }
  ]
}
```

AC vectors are `{ "re": [...], "im": [...] }`.

`ics spice deck.cir` simulates any SPICE file with the analyses in the file
(default backend built-in).

### GDS

- `ics import-gds` adds layers the technology lacks, unless `--skip-unknown-layers` is
  given. The library name defaults to the file name. By default the library is stored as
  one GDS file in the project; `--json` stores one JSON folder per cell instead (editable,
  diff-friendly, larger). `ics lib-storage` converts an existing library between the two.
- `ics gds-info` reads a GDS file without writing anything and prints the top cells with
  their sizes and the read / parse / index times.

### Verification and routing

- `ics drc` prints each rule with its violation count and up to 50 marker boxes in µm.
  `--deck FILE` uses a deck file instead of the technology's deck.
- `ics lvs` prints the device and net pairs and the differences.
- `ics route --place` first places the schematic devices missing from the layout
  (schematic-driven layout), then routes, then saves the project.

## Examples

```sh
ics new demo --template demo
ics info demo
ics netlist demo cmos_inverter
ics sim demo rc_lowpass --json > rc.json
ics sim demo nand2 --backend ngspice
ics sim demo ring_osc5 --parasitics
ics drc demo nand2 && ics lvs demo nand2
ics extract demo inv_chain
ics gds demo inv_tiles -o tiles.gds
ics import-gds demo chip.gds --lib chip
ics import-tech demo sky130A.lyp --lyt sky130A.lyt
```

A CI check of a cell:

```sh
ics drc myproj work/opamp && ics lvs myproj work/opamp
```
