//! IC Studio desktop application (library part, so UI tests can drive the app headlessly).

pub mod app;
pub mod canvas;
pub mod history;
pub mod home_view;
pub mod icons;
pub mod layout_view;
pub mod schematic_view;
pub mod sim_view;
pub mod theme;

/// Command line: `ics-studio [PROJECT_DIR] [--demo] [--workspace home|schematic|simulation|layout] [--run]`
pub struct LaunchArgs {
    pub project: Option<std::path::PathBuf>,
    pub demo: bool,
    pub workspace: Option<String>,
    pub cell: Option<String>,
    pub run: bool,
}

pub fn parse_args() -> LaunchArgs {
    let mut a = LaunchArgs { project: None, demo: false, workspace: None, cell: None, run: false };
    let mut it = std::env::args().skip(1);
    while let Some(x) = it.next() {
        match x.as_str() {
            "--demo" => a.demo = true,
            "--run" => a.run = true,
            "--workspace" | "-w" => a.workspace = it.next(),
            "--cell" | "-c" => a.cell = it.next(),
            "--help" | "-h" => {
                println!(
                    "ics-studio [PROJECT_DIR] [--demo] [--cell NAME] [--workspace home|schematic|simulation|layout] [--run]"
                );
                std::process::exit(0);
            }
            other => a.project = Some(other.into()),
        }
    }
    a
}
