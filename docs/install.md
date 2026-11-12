# Installation

IC Studio is built from source with the Rust toolchain.

## Requirements

- **Rust 1.95 or newer** from [rustup.rs](https://rustup.rs).
- **Windows**: the MSVC toolchain (`x86_64-pc-windows-msvc`), which needs the Visual
  Studio Build Tools with the "Desktop development with C++" workload.
- **Linux**: the windowing libraries
  `libxkbcommon-dev libwayland-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev`.
- **macOS**: the Xcode command line tools.
- A GPU driver supported by `wgpu` (DX12 on Windows, Vulkan on Linux, Metal on macOS).

## Build and run

```sh
git clone https://github.com/geniuskey/ic-studio
cd ic-studio
cargo run --release                     # the desktop app
cargo run --release -- --demo           # with the bundled demo designs
cargo run --release -- path/to/project  # open a project folder
```

To build all three binaries (`ic-studio`, `ics`, `ics-mcp`) into `target/release/`:

```sh
cargo build --release --workspace
```

## Launch options

```text
ic-studio [PROJECT_DIR] [--demo] [--cell NAME] [--workspace home|schematic|simulation|layout] [--run]
```

| Option | Effect |
|---|---|
| `PROJECT_DIR` | Open this project folder |
| `--demo` | Open the demo project (in memory, not saved until you save it) |
| `--cell NAME`, `-c NAME` | Open the cell with this name |
| `--workspace W`, `-w W` | Start in this workspace |
| `--run` | Run the cell's simulation on startup |

Example: `cargo run --release -- --demo --cell inv_tiles --workspace layout`.

Without arguments, the app reopens the most recent project if **Reopen last project** is
checked in Settings (the default), otherwise it shows the welcome page.

## ngspice (optional)

IC Studio has its own built-in simulator, so ngspice is not required. To use ngspice as a
backend, install it:

- Windows: the official build, which installs to `C:\Spice64\bin\`
- macOS: `brew install ngspice`
- Linux: `apt install ngspice` (or your distribution's package)

IC Studio looks for `/opt/homebrew/bin/ngspice`, `/usr/local/bin/ngspice`,
`/usr/bin/ngspice` and `C:\Spice64\bin\ngspice.exe`, then for `ngspice` on the `PATH`.
To use another location, open **File > Settings...** (`Ctrl+,`), enter the path in the
**ngspice** field and press **Detect**. The status bar and the console show whether
ngspice was found.

On Windows, `ngspice.exe` is the GUI build; if detection or batch simulation fails, point
the setting at the console binary `ngspice_con.exe` in the same folder.

## Settings

**File > Settings...** (`Ctrl+,`, or the gear at the bottom of the activity bar):

| Setting | Meaning |
|---|---|
| Theme | Dark or Light (also **View > Light Theme / Dark Theme**) |
| UI scale | 0.75 to 1.75 |
| ngspice | Path to the ngspice executable, with **Detect** |
| Startup | **Reopen last project** |

Settings and the recent-projects list are stored by the app between sessions.

## Example projects with real PDKs

Scripts download open-source designs into `examples/` (git-ignored) as ready-to-open
projects:

```sh
./scripts/fetch-examples.sh             # macOS / Linux (curl, gunzip)
./scripts/fetch-examples.ps1            # Windows PowerShell
#   --caravel / -Caravel  also the full caravel chip (52 MB download, 278 MB unpacked)
#   --force   / -Force    rebuild the projects from the downloaded files
```

- `examples/sky130`: SKY130 layer set with the efabless caravel libraries
  `caravel_clocking`, `housekeeping` and `chip_io` (and `caravel`).
- `examples/gf180`: GF180MCU layer set with the `gf180mcu_fd_sc_mcu7t5v0` standard cells
  and the `gf180mcu_fd_ip_sram` 512 x 8 SRAM macro.

Open them with **File > Open Project...** or `ic-studio examples/sky130`.
