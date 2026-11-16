//! IC Studio Python API: `import icstudio`.
//!
//! A [`Project`] wraps the same tool layer as the MCP server (`ics mcp`), so every MCP tool is
//! available as `project.call("run_drc", cell="inv")` with its JSON result as Python objects.
//! The common ones also have methods (`netlist`, `drc`, `lvs`, `verify`, ...), and
//! `simulate` returns full waveforms (NumPy arrays when NumPy is installed).

use pyo3::create_exception;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::{PyComplex, PyDict, PyList};
use serde_json::{Map, Value, json};

use ics_core::{Backend, CellRef};
use ics_mcp::tools::{State, definitions, find, simulate_cell};
use ics_sim::SimResults;

create_exception!(icstudio, IcsError, PyRuntimeError, "An IC Studio tool reported an error.");

fn err(e: impl ToString) -> PyErr {
    IcsError::new_err(e.to_string())
}

/// JSON to Python objects (through the `json` module, so numbers, lists and dicts are plain
/// Python values).
fn to_py(py: Python<'_>, v: &Value) -> PyResult<PyObject> {
    Ok(py.import("json")?.call_method1("loads", (v.to_string(),))?.unbind())
}

/// Python objects to JSON.
fn from_py(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<Value> {
    let text: String = py.import("json")?.call_method1("dumps", (obj,))?.extract()?;
    serde_json::from_str(&text).map_err(err)
}

fn backend(name: Option<&str>) -> PyResult<Option<Backend>> {
    Ok(match name.map(str::to_ascii_lowercase).as_deref() {
        None => None,
        Some("builtin") => Some(Backend::Builtin),
        Some("ngspice") => Some(Backend::Ngspice),
        Some("auto") => Some(Backend::Auto),
        Some(b) => return Err(err(format!("unknown backend '{b}' (builtin, ngspice or auto)"))),
    })
}

/// A list of floats, as a NumPy array when NumPy is available.
fn array<'py>(py: Python<'py>, list: Bound<'py, PyList>) -> PyResult<Bound<'py, PyAny>> {
    match py.import("numpy") {
        Ok(np) => np.call_method1("asarray", (list,)),
        Err(_) => Ok(list.into_any()),
    }
}

/// Simulation results as `{analysis: {"title", "scale", "vectors": {name: values}, "op": {...}}}`.
/// AC vectors are complex.
fn results_py(py: Python<'_>, res: &SimResults) -> PyResult<PyObject> {
    let out = PyDict::new(py);
    for ds in &res.datasets {
        let d = PyDict::new(py);
        d.set_item("title", &ds.title)?;
        d.set_item("scale", ds.scale_vector().map(|v| v.name.clone()))?;
        let vectors = PyDict::new(py);
        for v in &ds.vectors {
            let list = match &v.im {
                Some(im) => PyList::new(py, v.re.iter().zip(im).map(|(r, i)| PyComplex::from_doubles(py, *r, *i)))?,
                None => PyList::new(py, &v.re)?,
            };
            vectors.set_item(&v.name, array(py, list)?)?;
        }
        d.set_item("vectors", vectors)?;
        if !ds.devices.is_empty() {
            let devs = PyDict::new(py);
            for dev in &ds.devices {
                let vals = PyDict::new(py);
                for (k, x) in &dev.values {
                    vals.set_item(k, x)?;
                }
                if let Some(r) = &dev.region {
                    vals.set_item("region", r)?;
                }
                devs.set_item(&dev.name, vals)?;
            }
            d.set_item("devices", devs)?;
        }
        let mut key = ds.analysis.clone();
        let mut k = 2;
        while out.contains(&key)? {
            key = format!("{}_{k}", ds.analysis);
            k += 1;
        }
        out.set_item(key, d)?;
    }
    Ok(out.into_any().unbind())
}

/// An IC Studio project: libraries of cells with schematic, simulation and layout views.
///
/// Open one with `Project.open(path)`, create one with `Project.create(path)`, or use the
/// in-memory demo with `Project.demo()`. Changes stay in memory until `save()`.
#[pyclass(unsendable, module = "icstudio")]
struct Project {
    st: State,
}

impl Project {
    fn tool(&mut self, py: Python<'_>, name: &str, args: Value) -> PyResult<PyObject> {
        let Value::Object(map) = args else { return Err(err("tool arguments must be a dict")) };
        let res = self.st.call(name, &map).ok_or_else(|| err(format!("no tool '{name}' (see icstudio.tools())")))?;
        to_py(py, &res.map_err(err)?)
    }

    fn cell(&self, name: &str) -> PyResult<CellRef> {
        let p = self.st.current().ok_or_else(|| err("no project"))?;
        find(p, name).ok_or_else(|| err(format!("no cell '{name}'")))
    }
}

#[pymethods]
impl Project {
    /// Open the project in directory `path`.
    #[staticmethod]
    fn open(path: &str) -> PyResult<Self> {
        let p = ics_core::Project::open(std::path::Path::new(path)).map_err(err)?;
        Ok(Project { st: State::with_project(p) })
    }

    /// Create a project in directory `path` from a template (`demo` or `empty`) and save it.
    #[staticmethod]
    #[pyo3(signature = (path, template = "demo"))]
    fn create(py: Python<'_>, path: &str, template: &str) -> PyResult<Self> {
        let mut p = Project { st: State::default() };
        p.tool(py, "new_project", json!({"path": path, "template": template}))?;
        Ok(p)
    }

    /// The demo project (in memory, not saved): inverters, filters, OTAs, SRAM, pixels.
    #[staticmethod]
    fn demo() -> Self {
        Project { st: State::with_project(ics_core::examples::demo_project()) }
    }

    #[getter]
    fn name(&self) -> String {
        self.st.current().map_or(String::new(), |p| p.name.clone())
    }

    /// The project directory (`None` for an unsaved project).
    #[getter]
    fn root(&self) -> Option<String> {
        self.st.current().and_then(|p| p.root.as_ref()).map(|r| r.display().to_string())
    }

    /// Every cell as `lib/cell`.
    fn cells(&self) -> Vec<String> {
        self.st.current().map(|p| p.all_cells().map(|(r, _)| r.to_string()).collect()).unwrap_or_default()
    }

    /// Libraries, cells with their views, and the technology.
    fn info(&mut self, py: Python<'_>) -> PyResult<PyObject> {
        self.tool(py, "project_info", json!({}))
    }

    /// Save the project (to `path` when given: save as).
    #[pyo3(signature = (path = None))]
    fn save(&mut self, path: Option<&str>) -> PyResult<()> {
        match path {
            Some(dir) => {
                let p = self.st.current_mut().ok_or_else(|| err("no project"))?;
                p.save_as(std::path::Path::new(dir)).map_err(err)
            }
            None => self.st.call("save_project", &Map::new()).expect("tool exists").map(|_| ()).map_err(err),
        }
    }

    /// Run any MCP tool by name with keyword arguments; returns its JSON result as Python
    /// objects. `icstudio.tools()` lists the tools with their arguments.
    #[pyo3(signature = (tool, **kwargs))]
    fn call(&mut self, py: Python<'_>, tool: &str, kwargs: Option<&Bound<'_, PyDict>>) -> PyResult<PyObject> {
        let args = match kwargs {
            Some(k) => from_py(py, k.as_any())?,
            None => json!({}),
        };
        self.tool(py, tool, args)
    }

    /// The netlist of a cell: `spice` (with its analyses), `spectre` or `cdl`.
    #[pyo3(signature = (cell, format = "spice"))]
    fn netlist(&mut self, py: Python<'_>, cell: &str, format: &str) -> PyResult<String> {
        let v = self.tool(py, "get_netlist", json!({"cell": cell, "format": format}))?;
        v.bind(py).get_item("deck")?.extract()
    }

    /// Create or replace a cell's schematic from a flat SPICE netlist (R, C, L, V, I, D, M
    /// lines; analysis lines set its simulation setup).
    #[pyo3(signature = (cell, netlist, overwrite = false))]
    fn write_schematic(&mut self, py: Python<'_>, cell: &str, netlist: &str, overwrite: bool) -> PyResult<PyObject> {
        self.tool(py, "write_schematic", json!({"cell": cell, "netlist": netlist, "overwrite": overwrite}))
    }

    /// Simulate a cell with its saved analyses. Returns `{analysis: {"title", "scale",
    /// "vectors": {name: values}, "devices": {...}}}` with full waveforms (complex for AC).
    #[pyo3(signature = (cell, parasitics = false, backend = None))]
    fn simulate(&self, py: Python<'_>, cell: &str, parasitics: bool, backend: Option<&str>) -> PyResult<PyObject> {
        let r = self.cell(cell)?;
        let b = self::backend(backend)?;
        let p = self.st.current().ok_or_else(|| err("no project"))?;
        let sim = simulate_cell(p, &r, parasitics, b).map_err(err)?;
        results_py(py, &sim.results)
    }

    /// Run the technology's DRC deck on a cell's layout.
    fn drc(&mut self, py: Python<'_>, cell: &str) -> PyResult<PyObject> {
        self.tool(py, "run_drc", json!({"cell": cell}))
    }

    /// Compare a cell's layout with its schematic.
    fn lvs(&mut self, py: Python<'_>, cell: &str) -> PyResult<PyObject> {
        self.tool(py, "run_lvs", json!({"cell": cell}))
    }

    /// Run the verification plan (every test's corners, sweeps and Monte Carlo); `tests`
    /// limits it to some tests, `save` adds the run to the project's history.
    #[pyo3(signature = (tests = None, save = false))]
    fn verify(&mut self, py: Python<'_>, tests: Option<Vec<String>>, save: bool) -> PyResult<PyObject> {
        let mut a = json!({"save": save});
        if let Some(t) = tests {
            a["tests"] = json!(t);
        }
        self.tool(py, "verify", a)
    }

    /// Run Cadence SKILL code on the project (layout scripting); returns what it printed and
    /// its value.
    fn skill(&mut self, py: Python<'_>, code: &str) -> PyResult<PyObject> {
        self.tool(py, "run_skill", json!({"code": code}))
    }

    fn __repr__(&self) -> String {
        match self.st.current() {
            Some(p) => format!("<icstudio.Project '{}' with {} cell(s)>", p.name, p.all_cells().count()),
            None => "<icstudio.Project (none)>".into(),
        }
    }
}

/// The tools `Project.call` runs: name, description and JSON schema of the arguments.
#[pyfunction]
fn tools(py: Python<'_>) -> PyResult<PyObject> {
    to_py(py, &Value::Array(definitions()))
}

/// Simulate a SPICE (or Spectre) deck with its own analyses; same result form as
/// `Project.simulate`.
#[pyfunction]
#[pyo3(signature = (deck, backend = None))]
fn run_spice(py: Python<'_>, deck: &str, backend: Option<&str>) -> PyResult<PyObject> {
    let opts = ics_sim::SimOptions { backend: self::backend(backend)?.unwrap_or_default(), ..Default::default() };
    let res = ics_sim::simulate(deck, &[], &opts).map_err(err)?;
    results_py(py, &res)
}

/// The SKILL script that dumps a Virtuoso library for `Project.call("import_virtuoso", ...)`.
#[pyfunction]
fn virtuoso_dump_script() -> &'static str {
    ics_core::virtuoso_dump::DUMP_SKILL
}

#[pymodule]
fn icstudio(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Project>()?;
    m.add_function(wrap_pyfunction!(tools, m)?)?;
    m.add_function(wrap_pyfunction!(run_spice, m)?)?;
    m.add_function(wrap_pyfunction!(virtuoso_dump_script, m)?)?;
    m.add("IcsError", m.py().get_type::<IcsError>())?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
