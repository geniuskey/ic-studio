# IC Studio

A modern, cross-platform environment for full-custom integrated circuit design —
schematic capture, circuit simulation and layout in one native, GPU-accelerated app.

IC Studio is an early **prototype**. It demonstrates the overall design philosophy,
the UI/UX, project management and working foundations for the Schematic, Simulation
and Layout workflows. It is written in Rust and renders through `wgpu` (Metal, Vulkan,
DX12), so it runs on Windows, Linux and macOS.

![Project home](docs/screenshots/02_project_home.png)

| Schematic with DC operating point back-annotation | Transient / DC sweep waveforms |
|---|---|
| ![Schematic](docs/screenshots/07_schematic_op.png) | ![Simulation](docs/screenshots/04_simulation.png) |
| **AC analysis with automatic measurements** | **Full-custom layout with layer palette** |
| ![AC](docs/screenshots/05_simulation_rc.png) | ![Layout](docs/screenshots/06_layout.png) |

## Quick start

```sh
# Rust 1.95+ (https://rustup.rs)
cargo run --release                     # launch the desktop app
cargo run --release -- --demo           # ... with the bundled demo designs
cargo run --release -- path/to/project  # ... opening a project folder
```

Optional: install [ngspice](https://ngspice.sourceforge.io/) (`brew install ngspice`,
`apt install ngspice`, or the Windows build) to use it as a simulation backend. Without
it, IC Studio uses its own built-in simulator.

Linux needs the usual windowing libraries:
`libxkbcommon-dev libwayland-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev`.

## What works today

**Application**
- Project → library → cell → view model (schematic / simulation / layout), stored as
  human-readable JSON that works well with version control
- Activity bar with four workspaces (Home, Schematic, Simulation, Layout), each with its
  own accent colour, and cell tabs shared across workspaces
- Project explorer with view creation, rename and delete, plus a command palette (⌘K / Ctrl+K)
- Console, Problems (netlist warnings) and live Netlist panels; dark and light themes;
  UI scaling; recent projects; prompts for unsaved changes

**Schematic**
- analogLib-style primitives: R, C, L, V, I, diode, NMOS, PMOS, ground, supply net
- Place, wire (L-routing), net label, select / rubber-band, move with wire stretching,
  rotate, mirror, duplicate, delete, undo / redo
- Live connectivity with T-junction detection, junction dots, unconnected-pin markers,
  hover net highlighting and a property inspector
- SPICE netlister; DC operating point back-annotation of node voltages and device currents

**Simulation**
- Analyses: operating point, DC sweep, AC (decade sweep) and transient
- Backends:
  - **Built-in `ics-sim`**: a Rust MNA engine with Newton–Raphson, gmin and source
    stepping, trapezoidal integration, breakpoint-aware time stepping, a diode model and a
    level-1 MOSFET with body effect and gate capacitances
  - **ngspice**: batch mode, with ASCII raw-file parsing
- Waveform viewer: signal browser, linked Bode magnitude and phase plots, hover readout,
  CSV export
- Automatic measurements: min / max / peak-to-peak / final; for AC, gain, peak, f−3 dB
  and unity-gain frequency
- Results from the two backends agree closely. On the demo inverter, the built-in engine
  gives tpHL = 35.1 ps and ngspice 36.3 ps; both give VM = 0.83 V and an RC corner of
  −2.992 dB.

**Layout**
- Technology/PDK file (`tech.json`) with layers, GDS numbers, colours, stipple patterns
  and width/spacing rules. A generic 180 nm-class demo technology is included.
- Rectangle, path (Manhattan), label and ruler tools; select, move, nudge, duplicate,
  undo / redo; µm-accurate inspector editing
- Layer palette with visibility toggles, adaptive grid, scale bar, and DRC markers that
  zoom to the violation when clicked
- Basic built-in DRC (min width / spacing) as a placeholder for KLayout, Magic or Calibre
- GDSII export (per cell or the whole library) and import

**Automation**
- The `ics` CLI exposes the same core:

  ```sh
  ics new myproj --template demo
  ics info myproj
  ics netlist myproj cmos_inverter
  ics sim myproj cmos_inverter --backend ngspice --json   # machine-readable results
  ics spice deck.cir                                      # simulate any SPICE deck
  ics gds myproj cmos_inverter -o inv.gds
  ics drc myproj cmos_inverter                            # non-zero exit on violations
  ```

## Architecture

```
crates/
  ics-core    UI-independent design database
              geom · schematic (+ symbol library) · connectivity · netlist
              layout · tech (PDK) · gds · project storage · sim_setup · examples
  ics-sim     simulation layer: SPICE parser, MNA engine, ngspice backend,
              backend-independent results (SimResults / Dataset / Vector)
  ics-studio  desktop app (eframe/egui on wgpu)
              app (shell, commands, dialogs) · schematic_view · sim_view
              layout_view · home_view · canvas (pan/zoom) · theme · icons · history
  ics-cli     `ics` command line front-end
```

Design decisions:

- **The core is UI-free.** Everything a script, the CLI or a future Python or MCP
  interface needs lives in `ics-core` / `ics-sim`. The GUI is one client among several.
- **Commands are the single path for user actions.** Menus, shortcuts and the command
  palette all queue an `app::Command`. `IcStudio::command()` is the hook for future
  automation and AI-agent control of a running session.
- **External engines over reimplementation.** ngspice is a first-class backend. The
  built-in engine exists so the tool works with no setup. DRC, LVS and PEX are meant to
  plug in the same way (KLayout, Magic, Netgen).
- **Snapshot undo** per editor keeps the editing code simple and robust.
- **Plain-JSON project format** (`ic-studio.json`, `tech.json`, `<lib>/<cell>/*.json`)
  is easy to diff, review and generate.

## Testing

```sh
cargo test --workspace         # unit tests + headless UI tests
ICS_SHOT_DIR=shots cargo test -p ics-studio --test ui   # also write screenshots
```

The UI tests drive the full application with a real `wgpu` renderer (egui_kittest).
They walk through every workspace, run simulations, check accessibility labels and
glyph coverage, and perform a place-and-undo edit through simulated keyboard and mouse
input.

## Roadmap

- Hierarchy: symbols generated from schematics, instances in layout, and
  hierarchy-aware netlisting
- PDK plug-ins: SKY130 / GF180 tech import, BSIM models through ngspice `.lib`
- DRC / LVS / PEX through KLayout, Magic and Netgen
- Python API (PyO3) and an MCP server built on the command layer
- Parametric sweeps, corners and optimisation loops
- Layout: polygon editing, vias / contacts generators, connectivity and routing helpers
