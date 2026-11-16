"""Tests of the IC Studio Python API (run with pytest after `pip install ./python`)."""

import math
import os
import tempfile

import icstudio


def test_demo_project_and_netlist():
    p = icstudio.Project.demo()
    assert "work/cmos_inverter" in p.cells()
    assert "inverter" in repr(p) or "cell(s)" in repr(p)
    deck = p.netlist("cmos_inverter")
    assert "pmos" in deck.lower()
    assert "MI0/M0" not in deck
    cdl = p.netlist("cmos_inverter", format="cdl")
    assert ".SUBCKT" in cdl.upper()


def test_simulate_returns_full_waveforms():
    p = icstudio.Project.demo()
    r = p.simulate("rc_lowpass", backend="builtin")
    ac = r["ac"]
    f = list(ac["vectors"][ac["scale"]])
    vout = list(ac["vectors"]["v(out)"])
    assert len(f) == len(vout) > 10
    assert isinstance(vout[0], complex)
    # First-order low-pass: |H| = 1/sqrt(2) at fc = 1 / (2 pi R C) = 159 kHz.
    k = min(range(len(f)), key=lambda i: abs(abs(vout[i]) - 1 / math.sqrt(2)))
    assert abs(f[k] - 159.15e3) / 159.15e3 < 0.1
    tran = r["tran"]
    assert len(tran["vectors"]["time"]) > 10


def test_run_spice():
    r = icstudio.run_spice("* divider\nV1 a 0 1\nR1 a b 1k\nR2 b 0 1k\n.op\n.end\n", backend="builtin")
    assert abs(float(r["op"]["vectors"]["v(b)"][0]) - 0.5) < 1e-6


def test_any_tool_and_errors():
    p = icstudio.Project.demo()
    names = {t["name"] for t in icstudio.tools()}
    assert {"run_drc", "verify", "import_virtuoso"} <= names
    drc = p.drc("cmos_inverter")
    assert drc["clean"] == (drc["violations"] == 0)
    s = p.call("layout_summary", cell="cmos_inverter")
    assert s["cell"] == "work/cmos_inverter"
    try:
        p.call("no_such_tool")
    except icstudio.IcsError as e:
        assert "no tool" in str(e)
    else:
        raise AssertionError("expected IcsError")
    try:
        p.netlist("nope")
    except icstudio.IcsError:
        pass
    else:
        raise AssertionError("expected IcsError")


def test_schematic_skill_verify_and_save():
    with tempfile.TemporaryDirectory() as d:
        root = os.path.join(d, "proj")
        p = icstudio.Project.create(root, template="empty")
        assert p.root == root
        p.write_schematic("work/div", "V1 a 0 2\nR1 a b 1k\nR2 b 0 3k\n.op\n")
        r = p.simulate("div", backend="builtin")
        assert abs(float(r["op"]["vectors"]["v(b)"][0]) - 1.5) < 1e-6
        out = p.skill('printf("hello %d" 42)')
        assert "hello 42" in out["output"]
        p.save()
        again = icstudio.Project.open(root)
        assert "work/div" in again.cells()
    v = icstudio.Project.demo().verify(tests=["rc_lowpass"])
    assert v["status"] == "pass" and v["pass"] == 2
    assert "icsDumpLib" in icstudio.virtuoso_dump_script()


if __name__ == "__main__":
    for name, f in list(globals().items()):
        if name.startswith("test_"):
            f()
            print("ok", name)
