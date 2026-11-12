//! IC Studio core database: projects, schematics, layouts, technology and netlisting.
//!
//! This crate is UI-independent so it can back the desktop app, the CLI and future
//! Python / MCP automation interfaces.

pub mod connectivity;
pub mod examples;
pub mod gds;
pub mod geom;
pub mod hier;
pub mod layout;
pub mod netlist;
pub mod project;
pub mod schematic;
pub mod sim_setup;
pub mod tech;
pub mod units;

pub use geom::{IRect, Orient, Pt};
pub use layout::{ArraySpec, CellInst, Geometry, Layout, Shape, Trans};
pub use project::{Cell, CellRef, Library, Project, ViewKind};
pub use schematic::{DeviceKind, Instance, NetLabel, SchObj, Schematic, Wire};
pub use sim_setup::{Analysis, AnalysisEntry, Backend, SimSetup};
pub use tech::{FillStyle, LayerDef, Tech};
