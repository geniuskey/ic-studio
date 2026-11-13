//! IC Studio core database: projects, schematics, layouts, technology and netlisting.
//!
//! This crate is UI-independent so it can back the desktop app, the CLI and future
//! Python / MCP automation interfaces.

pub mod connectivity;
pub mod drc;
pub mod drc_fix;
pub mod examples;
pub mod extract;
pub mod fill;
pub mod gds;
pub mod geom;
pub mod hier;
pub mod layout;
pub mod layout_gen;
pub mod lvs;
pub mod matching;
pub mod netlist;
pub mod oasis;
pub mod pcell;
pub mod pdk;
pub mod pex;
pub mod project;
pub mod region;
pub mod route;
pub mod rule_design;
pub mod sch_check;
pub mod schematic;
pub mod sdl;
pub mod signoff;
pub mod sim_setup;
pub mod skill;
pub mod spice_import;
pub mod stream_map;
pub mod svrf;
pub mod sweep;
pub mod tech;
pub mod tech_import;
pub mod units;

pub use geom::{IRect, Orient, Pt};
pub use layout::{ArraySpec, CellInst, Geometry, Layout, Shape, Trans};
pub use project::{Cell, CellRef, LibStorage, Library, Project, ViewKind};
pub use schematic::{DeviceKind, Instance, NetLabel, SchObj, Schematic, Wire};
pub use sim_setup::{Analysis, AnalysisEntry, Backend, SimSetup};
pub use tech::{FillStyle, LayerDef, Tech};
