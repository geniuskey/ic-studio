"""IC Studio Python API: projects, schematics, simulation, layout, DRC / LVS and verification."""

from typing import Any, Optional

__version__: str

class IcsError(RuntimeError):
    """An IC Studio tool reported an error."""

class Project:
    """An IC Studio project: libraries of cells with schematic, simulation and layout views.

    Changes stay in memory until `save()`.
    """

    @staticmethod
    def open(path: str) -> "Project":
        """Open the project in directory `path`."""
    @staticmethod
    def create(path: str, template: str = "demo") -> "Project":
        """Create a project in `path` from a template (`demo` or `empty`) and save it."""
    @staticmethod
    def demo() -> "Project":
        """The demo project, in memory."""
    @property
    def name(self) -> str: ...
    @property
    def root(self) -> Optional[str]:
        """The project directory (`None` when not saved)."""
    def cells(self) -> list[str]:
        """Every cell as `lib/cell`."""
    def info(self) -> dict[str, Any]:
        """Libraries, cells with their views, and the technology."""
    def save(self, path: Optional[str] = None) -> None:
        """Save the project (to `path` when given)."""
    def call(self, tool: str, **kwargs: Any) -> Any:
        """Run any MCP tool by name (see `tools()`); returns its JSON result."""
    def netlist(self, cell: str, format: str = "spice") -> str:
        """The netlist of a cell: `spice`, `spectre` or `cdl`."""
    def write_schematic(self, cell: str, netlist: str, overwrite: bool = False) -> dict[str, Any]:
        """Create or replace a cell's schematic from a flat SPICE netlist."""
    def simulate(self, cell: str, parasitics: bool = False, backend: Optional[str] = None) -> dict[str, Any]:
        """Simulate a cell with its saved analyses.

        Returns `{analysis: {"title", "scale", "vectors": {name: values}, "devices"}}` with full
        waveforms (NumPy arrays when NumPy is installed; complex for AC).
        `backend` is `builtin`, `ngspice` or `auto`.
        """
    def drc(self, cell: str) -> dict[str, Any]:
        """Run the technology's DRC deck on a cell's layout."""
    def lvs(self, cell: str) -> dict[str, Any]:
        """Compare a cell's layout with its schematic."""
    def verify(self, tests: Optional[list[str]] = None, save: bool = False) -> dict[str, Any]:
        """Run the verification plan; `save` adds the run to the project's history."""
    def skill(self, code: str) -> dict[str, Any]:
        """Run Cadence SKILL code on the project."""

def tools() -> list[dict[str, Any]]:
    """The tools `Project.call` runs, with their argument schemas."""

def run_spice(deck: str, backend: Optional[str] = None) -> dict[str, Any]:
    """Simulate a SPICE or Spectre deck with its own analyses."""

def virtuoso_dump_script() -> str:
    """The SKILL script (icsDump.il) that dumps a Virtuoso library for `import_virtuoso`."""
