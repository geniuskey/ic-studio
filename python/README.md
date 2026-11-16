# icstudio: Python API for IC Studio

```sh
pip install ./python          # from a checkout (needs Rust; builds with maturin)
```

```python
import icstudio

p = icstudio.Project.open("myproj")          # or Project.create(path), Project.demo()
print(p.cells())                             # ['work/ota5t', ...]
print(p.netlist("ota5t", format="spectre"))

r = p.simulate("rc_lowpass")                 # full waveforms, NumPy arrays if installed
ac = r["ac"]
f, vout = ac["vectors"][ac["scale"]], ac["vectors"]["v(out)"]   # vout is complex

print(p.drc("cmos_inverter")["violations"], p.lvs("cmos_inverter")["matched"])
print(p.verify()["status"])                  # every test's corners and specs

# Every MCP tool, by name, with keyword arguments:
p.call("place_pcell", cell="cmos_inverter", kind="nmos", params={"w": 2, "l": 0.18}, x=10, y=0)
[t["name"] for t in icstudio.tools()]
p.save()
```

Errors raise `icstudio.IcsError`. See the user guide's *Python API* chapter.
