"""Design report: an example IC Studio extension in Python.

It calls built-in IC Studio tools on the open project (get_netlist, run_drc, run_lvs,
project_info) and writes the results as Markdown."""

import os
from collections import Counter
from datetime import date

from ics_extension import Extension, ToolError

ext = Extension()

# Sources (V, I) are the testbench, not the circuit.
KINDS = {"M": "MOSFET", "R": "resistor", "C": "capacitor", "L": "inductor", "D": "diode", "Q": "BJT",
         "X": "subcircuit instance", "E": "VCVS", "F": "CCCS", "G": "VCCS", "H": "CCVS"}


def devices(cell):
    deck = ext.call("get_netlist", cell=cell)["deck"]
    count = Counter()
    for line in deck.splitlines():
        line = line.strip()
        if line and not line.startswith(("*", ".", "+", "V", "v", "I", "i")):
            count[KINDS.get(line[0].upper(), line[0].upper())] += 1
    return count


@ext.tool("report")
def report(cell, write=True):
    lines = [f"# {cell}", "", f"Report of {date.today().isoformat()}.", ""]
    summary = {"cell": cell}

    try:
        count = devices(cell)
        summary["devices"] = dict(count)
        lines += ["## Devices", ""] + [f"- {k}: {n}" for k, n in sorted(count.items())] + [""]
    except ToolError as e:
        lines += ["## Devices", "", f"No schematic: {e}", ""]

    try:
        drc = ext.call("run_drc", cell=cell)
        summary["drc_violations"] = drc["violations"]
        lines += ["## DRC", "", "Clean." if drc["clean"] else f"{drc['violations']} violations:", ""]
        lines += [f"- `{r['rule']}`: {r['count']} ({r['description']})" for r in drc["by_rule"]]
        lines.append("")
    except ToolError as e:
        lines += ["## DRC", "", f"Not run: {e}", ""]

    try:
        lvs = ext.call("run_lvs", cell=cell)
        summary["lvs_matched"] = lvs["matched"]
        lines += ["## LVS", "", "Layout matches the schematic." if lvs["matched"] else f"{lvs['differences']} differences:", ""]
        lines += [f"- {e['kind']}: {e['message']}" for e in lvs["errors"]]
        lines.append("")
    except ToolError as e:
        lines += ["## LVS", "", f"Not run: {e}", ""]

    text = "\n".join(lines)
    root = ext.call("project_info").get("root")
    if write and root:
        path = os.path.join(root, "reports", cell.replace("/", "_") + ".md")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as f:
            f.write(text)
        summary["written"] = path
        ext.log(f"design report written to {path}")
    summary["markdown"] = text
    parts = [f"{sum(summary.get('devices', {}).values())} devices"]
    if "drc_violations" in summary:
        parts.append(f"DRC {summary['drc_violations']} violation(s)")
    if "lvs_matched" in summary:
        parts.append("LVS clean" if summary["lvs_matched"] else "LVS differences")
    # Shown in IC Studio's console when run from the Extensions menu.
    summary["message"] = f"{cell}: " + ", ".join(parts) + (f"; saved {summary['written']}" if "written" in summary else "")
    return summary


if __name__ == "__main__":
    ext.run()
