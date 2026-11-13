# Installation

Install a release build with one command, or build IC Studio from source with the Rust
toolchain.

## Install a release

Windows (PowerShell):

```powershell
irm https://github.com/geniuskey/ic-studio/releases/latest/download/install.ps1 | iex
```

macOS (Apple silicon) and Linux (x86_64):

```sh
curl -fsSL https://github.com/geniuskey/ic-studio/releases/latest/download/install.sh | sh
```

The installer downloads the latest release and installs two programs into one folder:
the desktop app `ic-studio` and the command line tool `ics`, which is also the MCP server
for AI agents (`ics mcp`).

| | Windows | macOS / Linux |
|---|---|---|
| Folder | `%LOCALAPPDATA%\Programs\IC Studio` | `~/.local/share/ic-studio` |
| On the PATH | folder added to the user `PATH` | links in `~/.local/bin` |
| Launcher | Start Menu shortcut **IC Studio** | `~/.local/share/applications/ic-studio.desktop` (Linux) |

The installer also:

- registers the MCP server with Claude Code if `claude` is on the `PATH`
  (`claude mcp add --scope user ic-studio -- <folder>/ics mcp`); otherwise it prints that
  command and the JSON config for other clients (see [MCP server](mcp.md));
- on macOS, removes the download quarantine attribute;
- on macOS / Linux, prints the line to add to your shell profile if `~/.local/bin` is not
  on the `PATH`. On Windows, open a new terminal to pick up the `PATH` change.

To update, run the installer again; this works while the app or an agent is running
(restart them afterwards). The app's own updater (**Help > Check for Updates...**) also
replaces `ics` together with the app.

Options are environment variables:

| Variable | Effect |
|---|---|
| `ICS_VERSION` | Install this release tag, e.g. `v0.2.0` (default: latest) |
| `ICS_INSTALL_DIR` | Install into this folder |
| `ICS_UNINSTALL=1` | Remove the folder, the `PATH` entry or links, the launcher and the MCP registration |

```powershell
$env:ICS_VERSION="v0.2.0"; irm https://github.com/geniuskey/ic-studio/releases/latest/download/install.ps1 | iex
```

```sh
curl -fsSL https://github.com/geniuskey/ic-studio/releases/latest/download/install.sh | ICS_UNINSTALL=1 sh
```

Release builds exist for Windows x86_64, Linux x86_64 and macOS on Apple silicon. On an
Intel Mac, build from source.

**Manual download.** Each [release](https://github.com/geniuskey/ic-studio/releases) has
an archive with both programs, `ic-studio-<platform>.zip` (Windows) or
`ic-studio-<platform>.tar.gz` (macOS, Linux), where `<platform>` is `windows-x86_64`,
`linux-x86_64` or `macos-aarch64`. Unpack it into a folder of your choice and keep the two
programs together: the updater and **Add to Claude Code** look for `ics` next to
`ic-studio`. Put the folder on the `PATH` to use `ics` in a terminal.

## Build from source

### Requirements

- **Rust 1.95 or newer** from [rustup.rs](https://rustup.rs).
- **Windows**: the MSVC toolchain (`x86_64-pc-windows-msvc`), which needs the Visual
  Studio Build Tools with the "Desktop development with C++" workload.
- **Linux**: the windowing libraries
  `libxkbcommon-dev libwayland-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev`.
- **macOS**: the Xcode command line tools.
- A GPU driver supported by `wgpu` (DX12 on Windows, Vulkan on Linux, Metal on macOS).

### Build and run

```sh
git clone https://github.com/geniuskey/ic-studio
cd ic-studio
cargo run --release                     # the desktop app
cargo run --release -- --demo           # with the bundled demo designs
cargo run --release -- path/to/project  # open a project folder
```

To build both programs (`ic-studio`, `ics`) into `target/release/`:

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

**File > Settings...** (`Ctrl+,`, also in the **Tools** menu, or the gear at the bottom of the
activity bar):

| Setting | Meaning |
|---|---|
| Theme | Dark or Light (also **View > Light Theme / Dark Theme**) |
| UI scale | 0.75 to 1.75 |
| ngspice | Path to the ngspice executable, with **Detect** |
| Interaction | Mouse profile: **Virtuoso** (default) or **Conventional** (see [Keyboard shortcuts](shortcuts.md#canvas-schematic-and-layout)) |
| Startup | **Reopen last project**; **Check for updates** on startup |
| AI | **AI Models...**: provider, model and API key of the Design Copilot |
| AI agents (MCP) | **Add to Claude Code** registers `ics mcp` with Claude Code; **Copy MCP config** copies the JSON for other clients (see [MCP server](mcp.md)) |

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
