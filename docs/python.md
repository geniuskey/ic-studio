<!-- seo title="Python API" description="Script IC Studio from Python: open projects, write schematics, simulate with full waveforms, run DRC, LVS and verification, and call every MCP tool." -->

# Python API

The `icstudio` Python package opens IC Studio projects in your own scripts and notebooks.
Use it to write a schematic from a netlist, simulate with full waveforms, run DRC, LVS and
the verification plan, or call any of the [MCP tools](mcp.md). It runs the same Rust code as
the app and the `ics` command line, in-process, and needs no running app.

## Install

The package is built from the repository with [maturin](https://www.maturin.rs). You need
Rust (see [Installation](install.md)) and Python 3.9 or newer:

```sh
pip install ./python            # from a checkout of the repository
pip install numpy               # optional: waveforms as NumPy arrays
```

## A first script

```python
import icstudio

p = icstudio.Project.open("myproj")      # an existing project
# p = icstudio.Project.create("newproj", template="empty")   # or a new one, saved at once
# p = icstudio.Project.demo()            # or the demo cells, in memory

print(p.cells())                         # ['work/ota5t', 'work/rc_lowpass', ...]

p.write_schematic("work/divider", """
V1 a 0 1.8
R1 a b 10k
R2 b 0 10k
.op
""")
r = p.simulate("divider")
print(r["op"]["vectors"]["v(b)"])        # [0.9]
p.save()
```

Changes stay in memory until `save()` (or `save("other/dir")` to save as).

## Simulation results

`simulate(cell)` runs the cell's saved analyses and returns the full waveforms. The result is
a dict keyed by analysis (`op`, `dc`, `ac`, `tran`, `noise`):

```python
r = p.simulate("rc_lowpass", backend="builtin")    # backend: builtin, ngspice or auto
ac = r["ac"]
f = ac["vectors"][ac["scale"]]           # the sweep variable: frequency
h = ac["vectors"]["v(out)"]              # complex for AC

import numpy as np
fc = f[np.argmin(abs(abs(h) - 1 / np.sqrt(2)))]    # -3 dB frequency
```

Each analysis has `title`, `scale` (the name of the sweep vector), `vectors` (name to
values) and, for the operating point, `devices` (each device's operating-point values and
region). Vectors are NumPy arrays when NumPy is installed and lists otherwise.
`simulate(cell, parasitics=True)` includes the layout's extracted parasitics.
`icstudio.run_spice(deck)` simulates a SPICE or Spectre deck with its own analyses and
returns the same form.

## Methods

| Method | Does |
| --- | --- |
| `Project.open(path)`, `Project.create(path, template)`, `Project.demo()` | Open, create (`demo` or `empty`) or load the demo project |
| `name`, `root`, `cells()`, `info()` | The project name, directory, cells (`lib/cell`), libraries and technology |
| `netlist(cell, format)` | The netlist: `spice` (with analyses), `spectre` or `cdl` |
| `write_schematic(cell, netlist, overwrite)` | A schematic from a flat SPICE netlist |
| `simulate(cell, parasitics, backend)` | Full waveforms of the saved analyses |
| `drc(cell)`, `lvs(cell)` | DRC with the technology's deck; layout versus schematic |
| `verify(tests, save)` | The [verification plan](simulation.md#verification-all-tests): each spec's worst value, margin and status |
| `skill(code)` | Run Cadence SKILL on the project |
| `call(tool, **args)` | Any MCP tool by name |
| `save(path)` | Save (as) |

## Every MCP tool

`call` runs any tool of the [MCP server](mcp.md#tools) with keyword arguments and returns
its JSON result as Python dicts and lists. `icstudio.tools()` lists the tools with their
descriptions and argument schemas:

```python
p = icstudio.Project.demo()
p.call("place_pcell", cell="cmos_inverter", kind="nmos", params={"w": 2, "l": 0.18}, x=10, y=0)
p.call("route", cell="cmos_inverter")
p.call("export_gds", cell="cmos_inverter", path="inv.gds")
[t["name"] for t in icstudio.tools()]
```

A failing tool raises `icstudio.IcsError`, with the tool's message.

## Example: a regression check in CI

```python
import sys
import icstudio

run = icstudio.Project.open("myproj").verify(save=True)
for t in run["tests"]:
    for s in t["specs"]:
        print(f'{t["name"]:12} {s["name"]:10} {s["status"]:5} worst {s["worst"]} at {s["at"]}')
regressions = [c for c in run["changes"] if c["regression"]]
sys.exit(1 if run["status"] in ("FAIL", "error") or regressions else 0)
```

`ics verify` does the same from the shell and also writes JUnit XML (see
[Command line](cli.md#verification)).
