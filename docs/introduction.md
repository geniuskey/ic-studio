# Introduction

IC Studio is a cross-platform environment for full-custom integrated circuit design. It
combines schematic capture, circuit simulation, layout editing and physical verification
in one native, GPU-accelerated application for Windows, Linux and macOS.

![Project home](screenshots/02_project_home.png)

The schematic, the simulator and the layout share one design database:

- layouts are **generated from schematics** and routed automatically,
- layouts are **checked against schematics** (LVS),
- **parasitics extracted from the layout** flow back into simulation.

## What is in the box

| Binary | Purpose |
|---|---|
| `ic-studio` | The desktop application |
| `ics` | Command-line interface for scripting and CI |
| `ics-mcp` | Model Context Protocol server for AI agents |

All three use the same core library, so a project edited in the app can be simulated,
checked or exported from the command line, or driven by an agent.

## Main concepts

- **Project**: a folder holding `ic-studio.json`, the technology (`tech.json`) and one
  folder per library.
- **Library**: a named group of cells (the demo project has `work` and `pcells`).
- **Cell**: a design unit with up to three views: **schematic**, **simulation** (the
  analyses set up for the schematic) and **layout**.
- **Workspace**: the app has four workspaces, Home, Schematic, Simulation and Layout.
  The open cells appear as tabs shared by all workspaces; switching workspace shows the
  same cell in a different view.
- **Technology (PDK)**: layers, GDS numbers, display styles, design rules and electrical
  coefficients. New projects start with a generic 180 nm-class technology.

## How to read this guide

- New users: [Installation](install.md), then the [Quick tour](quick-tour.md).
- Designing a circuit: [Schematic](schematic.md) and [Simulation](simulation.md).
- Drawing and checking layout: [Layout](layout.md), [Verification](verification.md) and
  [Schematic-driven layout](schematic-driven-layout.md).
- Using a real PDK: [Technology](technology.md).
- Automation: [Command line](cli.md) and [MCP server](mcp.md).

Keyboard shortcuts are written for Windows and Linux (`Ctrl`). On macOS use `Cmd` instead.
Press `F1` in the app for the full shortcut list, or see [Keyboard shortcuts](shortcuts.md).
