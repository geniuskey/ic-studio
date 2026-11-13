# IC Studio — notes for Claude

A cross-platform, full-custom IC design environment: schematic, simulation and layout,
written in Rust (egui/eframe 0.37 on wgpu). See README.md for features and architecture.

## Development environment

- **Primary development machine: Windows.** Use PowerShell syntax in commands
  (`$env:VAR="x"`, not `VAR=x cmd`). Paths use `\`. The project was originally built on
  macOS and has **not yet been verified on Windows**. If something breaks, fix it here
  first.
- Toolchain: rustup stable with the MSVC target (`x86_64-pc-windows-msvc`), which needs
  the Visual Studio Build Tools (the "Desktop development with C++" workload).
  Rust 1.95 or newer (required by egui_plot 0.38).
- ngspice (optional): the Windows build installs to `C:\Spice64\bin\`.
  `ics_sim::ngspice::default_exe()` looks for `C:\Spice64\bin\ngspice.exe`.
  Unverified: batch mode on Windows probably needs the console binary `ngspice_con.exe`,
  because `ngspice.exe` is the GUI build. Check this, and adjust `default_exe` / `detect`
  if needed.

## Commands

```powershell
cargo run --release -- --demo                         # app with demo designs
cargo run --release -- --demo --cell cmos_inverter --workspace layout
cargo run -p ics-cli -- sim <project> cmos_inverter --json   # CLI ("ics")
cargo test --workspace                                # unit + headless UI tests
$env:ICS_SHOT_DIR="shots"; cargo test -p ics-studio --test ui   # also write PNG screenshots
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings # CI enforces zero warnings
```

To check UI changes visually, render the screenshots with `ICS_SHOT_DIR` and look at the
PNGs. The UI tests use egui_kittest with a real wgpu renderer (DX12 on Windows).

## Layout

- `crates/ics-core`: UI-free design database (schematic, connectivity, netlist, layout,
  tech/PDK, GDS, project JSON storage). Keep it free of UI dependencies.
- `crates/ics-sim`: SPICE parser, built-in MNA engine and ngspice backend, behind
  `ics_sim::simulate()`.
- `crates/ics-studio`: the desktop app. It is also a lib target so `tests/ui.rs` can
  drive it.
- `crates/ics-cli`: the `ics` binary.
- `crates/ics-mcp`: MCP tools and stdio server (library), run as `ics mcp`.

## Conventions

- **Git: commit and push directly to `main`. No PRs, no feature branches.** Only
  commit or push when asked.
- Commit messages end with a `Co-Authored-By: Claude …` line.
- rustfmt `max_width = 130` (see `rustfmt.toml`).
- Every user action goes through `app::Command` (menus, shortcuts, command palette).
  Add new actions as commands. `IcStudio::command()` is the automation hook.
- UI tests enforce two rules. Keep both passing:
  - **Glyph coverage:** any non-ASCII character in UI text must exist in the bundled
    fonts (`glyph_coverage` test). egui 0.37 ships no emoji by default. We enable
    `monochrome_emoji_fonts` and use Hack as a fallback. Prefer the painted icons in
    `icons.rs`.
  - **Accessibility:** inputs need labels, e.g.
    `let l = ui.label("X"); ui.add(widget).labelled_by(l.id);`
- egui 0.37 API differs from older egui. Panels are `egui::Panel::left/right/top/bottom(id).show(ui, …)`.
  `App::ui(&mut self, ui, frame)` replaces `update`. Use `Frame::new()`, `CornerRadius`,
  `StrokeKind`, `Id::unique`, and `ui.scope_id()`. When unsure, read the crate source
  under `~/.cargo/registry/src`.
- Units: schematic coordinates are integer grid points (y points down). Layout
  coordinates are database units, where 1 dbu = 1 nm (y points up).
