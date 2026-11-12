# Writing a schematic by hand

A cell is a folder `<project>/<lib>/<cell>/` that holds:

- `cell.json`, for example `{"description": "..."}`
- `schematic.json`
- `sim.json` (optional)
- `layout.json` (optional, created by layout tools)

IC Studio finds new cell folders when the project is opened.

## schematic.json

```json
{
  "instances": [
    {"id": 1, "kind": "Pmos", "name": "M1", "pos": {"x": 0, "y": 0},
     "params": {"model": "pmos", "w": "2u", "l": "180n", "m": "1"}},
    {"id": 2, "kind": "Nmos", "name": "M2", "pos": {"x": 0, "y": 10},
     "params": {"model": "nmos", "w": "1u", "l": "180n", "m": "1"}},
    {"id": 3, "kind": "Ground", "name": "G1", "pos": {"x": 1, "y": 12}},
    {"id": 4, "kind": "Ground", "name": "G2", "pos": {"x": 2, "y": 10}},
    {"id": 5, "kind": "VSource", "name": "V1", "pos": {"x": 10, "y": 0}, "params": {"dc": "1.8"}},
    {"id": 6, "kind": "Ground", "name": "G3", "pos": {"x": 10, "y": 2}},
    {"id": 7, "kind": "VSource", "name": "V2", "pos": {"x": 14, "y": 0},
     "params": {"dc": "0", "tran": "PULSE(0 1.8 1n 100p 100p 4n 10n)"}},
    {"id": 8, "kind": "Ground", "name": "G4", "pos": {"x": 14, "y": 2}}
  ],
  "wires": [],
  "labels": [
    {"id": 20, "name": "in",  "pos": {"x": -2, "y": 0}},
    {"id": 21, "name": "in",  "pos": {"x": -2, "y": 10}},
    {"id": 22, "name": "out", "pos": {"x": 1,  "y": 2}},
    {"id": 23, "name": "out", "pos": {"x": 1,  "y": 8}},
    {"id": 24, "name": "vdd", "pos": {"x": 1,  "y": -2}},
    {"id": 25, "name": "vdd", "pos": {"x": 2,  "y": 0}},
    {"id": 26, "name": "vdd", "pos": {"x": 10, "y": -2}},
    {"id": 27, "name": "in",  "pos": {"x": 14, "y": -2}}
  ],
  "next_id": 30
}
```

This is a complete CMOS inverter with its supply and a pulse input. It goes through
netlist, simulation, `ics route --place`, DRC 0 and LVS match unchanged.

## Connection rules

- Coordinates are integer grid points with **y pointing down**.
- A pin connects to anything at the same point: another pin, a wire end, or a **net
  label**. Labels with the same name are one net. Putting a label on every pin is the
  simplest way to write a correct schematic, with no wires needed.
- A wire `{"id", "a": {x,y}, "b": {x,y}}` connects its two ends. A point that lies on a
  wire's interior also joins it (T-junction).
- A `Ground` symbol is net `0`. A `Supply` symbol names its net with `params.net`
  (default `vdd`). Net-naming priority: ground, then supply symbols, then labels, then
  automatic names.
- Ids must be unique across instances, wires and labels. `next_id` must be larger than
  all of them.
- Instance names give the SPICE names (`M1`, `R1`, `V1`) and the layout instance names,
  so keep them unique.
- Keep `orient` at the default (`rot` 0, no mirror) so that the pin offsets below hold.

## Pin offsets (from `pos`, at rot 0)

| kind | pins |
|---|---|
| `Nmos` | d (1,-2), g (-2,0), s (1,2), b (2,0) |
| `Pmos` | d (1,2), g (-2,0), s (1,-2), b (2,0). Source on top, toward vdd |
| `Resistor`, `Capacitor`, `Inductor` | p (0,-2), n (0,2) |
| `VSource`, `ISource` | p (0,-2) is the + terminal, n (0,2) |
| `Diode` | a (0,-2) anode, k (0,2) cathode |
| `Ground` | gnd (0,0) |
| `Supply` | net (0,0) |

## Parameters

| kind | params (defaults) |
|---|---|
| `Nmos` / `Pmos` | `model` (`nmos`/`pmos`, or a PDK device name), `w` (`1u`/`2u`), `l` (`180n`), `m` (`1`) |
| `Resistor` | `r` (`1k`) |
| `Capacitor` | `c` (`1p`) |
| `Inductor` | `l` (`1n`) |
| `VSource` / `ISource` | `dc`, `ac` (AC magnitude, e.g. `1`), `tran` (`PULSE(...)`, `SIN(...)`, `PWL(...)`) |
| `Diode` | `model` (`dmod`) |

Layout matching hints for MOS: `match` (`"pair1"` groups devices for common-centroid
placement, `"none"` opts a device out) and `match_rows` (`1` or `2`).

## sim.json

```json
{
  "backend": "builtin",
  "analyses": [
    {"enabled": true, "analysis": {"type": "op"}},
    {"enabled": true, "analysis": {"type": "dc", "source": "V2", "start": "0", "stop": "1.8", "step": "10m"}},
    {"enabled": false, "analysis": {"type": "ac", "points_per_decade": 20, "fstart": "1", "fstop": "10g"}},
    {"enabled": true, "analysis": {"type": "tran", "tstep": "10p", "tstop": "20n"}}
  ],
  "extra": "",
  "temp": 27.0,
  "parasitics": false
}
```

- `backend` is `builtin` or `ngspice`.
- `extra` holds raw SPICE lines appended to the deck, for example `.measure`, `.ic` or
  `.include`.
- AC analysis needs a source with an `ac` value.

## After writing the files

1. In live mode, reload with `open_project` on the project folder.
2. Check the result with `get_netlist`. Every device should list the nets you intended, and
   no `netN` should appear where you meant a named net.
